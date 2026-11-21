//! Busca no arquivo inteiro, sobre os bytes crus do arquivo mapeado.
//!
//! - Texto literal com `memchr::memmem`; ignorar caixa e expressão regular usam `regex::bytes`
//!   (Unicode: `ç` casa com `Ç`). UTF-8 inválido no arquivo não quebra nada: só não casa com `.`.
//! - A busca é por linha, com as regras de `linhas.rs` (BOM fora, CRLF sem o `\r`): achado nunca
//!   cruza quebra de linha; `^` e `$` valem no início e no fim de cada linha (e, pelo modo CRLF
//!   da `regex`, também em volta de um `\r` solto, que para o índice é dado).
//! - Achado vazio (ex.: `a*` onde não há `a`) é ignorado: não há o que destacar.
//! - Varredura paralela por blocos que terminam em `\n`; os achados chegam ao chamador em lotes,
//!   em ordem de arquivo, junto com o progresso. Cancelamento por `AtomicBool`.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use memchr::{memchr, memchr_iter, memmem, memrchr};
use regex::bytes::{Regex, RegexBuilder};

use crate::linhas::BOM;

/// Tamanho-alvo de um bloco de varredura (o bloco se estende até a próxima quebra de linha).
pub const BLOCO_PADRAO: usize = 256 << 10;
/// Quantos blocos um trabalhador pode adiantar além do último entregue; limita a memória
/// dos achados que esperam o bloco anterior terminar.
const ADIANTE_MAX: usize = 64;

/// Um achado: linha (a partir de 0, como em `ArquivoLinhas::janela`), deslocamento em bytes
/// dentro da linha e comprimento em bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Achado {
    pub linha: usize,
    pub coluna: usize,
    pub comprimento: usize,
}

/// O que o chamador recebe durante a varredura: os achados novos, em ordem, e o progresso.
#[derive(Debug, Default)]
pub struct Lote {
    pub achados: Vec<Achado>,
    /// Bytes já varridos (tudo antes deste ponto já foi entregue).
    pub varridos: usize,
    pub total: usize,
}

/// Fim da varredura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resumo {
    pub achados: u64,
    pub varridos: usize,
    pub total: usize,
    pub cancelado: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErroBusca {
    PadraoVazio,
    QuebraDeLinha,
    RegexInvalida(String),
}

impl fmt::Display for ErroBusca {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PadraoVazio => write!(f, "padrão vazio: diga o que procurar"),
            Self::QuebraDeLinha => write!(
                f,
                "o padrão tem quebra de linha, mas a busca é dentro de cada linha: tire o \\n"
            ),
            Self::RegexInvalida(e) => write!(f, "expressão regular inválida:\n{e}"),
        }
    }
}

impl std::error::Error for ErroBusca {}

enum Motor {
    Literal(memmem::Finder<'static>),
    Regex(Regex),
}

/// Padrão compilado, pronto para varrer um ou mais arquivos.
pub struct Busca {
    motor: Motor,
    bloco: usize,
}

impl Busca {
    pub fn nova(padrao: &str, regex: bool, ignorar_caixa: bool) -> Result<Self, ErroBusca> {
        if padrao.is_empty() {
            return Err(ErroBusca::PadraoVazio);
        }
        let motor = if regex {
            let re = RegexBuilder::new(padrao)
                .multi_line(true)
                .crlf(true)
                .case_insensitive(ignorar_caixa)
                .build()
                .map_err(|e| ErroBusca::RegexInvalida(e.to_string()))?;
            Motor::Regex(re)
        } else if padrao.contains('\n') {
            return Err(ErroBusca::QuebraDeLinha);
        } else if ignorar_caixa {
            let re = RegexBuilder::new(&regex::escape(padrao))
                .case_insensitive(true)
                .build()
                .map_err(|e| ErroBusca::RegexInvalida(e.to_string()))?;
            Motor::Regex(re)
        } else {
            Motor::Literal(memmem::Finder::new(padrao.as_bytes()).into_owned())
        };
        Ok(Self {
            motor,
            bloco: BLOCO_PADRAO,
        })
    }

    /// Troca o tamanho-alvo do bloco (testes e fuzz usam blocos minúsculos).
    pub fn com_bloco(mut self, bytes: usize) -> Self {
        self.bloco = bytes.max(1);
        self
    }

    /// Varre `dados` inteiro. `ao_lote` roda na thread do chamador, em ordem de arquivo; recebe
    /// um lote sempre que há achados novos e, sem achados, a cada ~1% varrido e no fim.
    /// Ligar `cancelar` (de qualquer thread) para a varredura antes do próximo bloco: depois que
    /// o sinal é visto, nenhum lote é entregue, e `Resumo::varridos` para no último bloco aceito.
    pub fn executar(
        &self,
        dados: &[u8],
        cancelar: &AtomicBool,
        mut ao_lote: impl FnMut(Lote),
    ) -> Resumo {
        let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
        let total = dados.len();
        let blocos = fatiar(dados, base, self.bloco);
        let mut resumo = Resumo {
            achados: 0,
            varridos: base.max(blocos.first().map_or(total, |b| b.start)),
            total,
            cancelado: false,
        };
        let trabalhadores = thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(blocos.len())
            .max(1);
        let proximo = AtomicUsize::new(0);
        let entregues = AtomicUsize::new(0);

        thread::scope(|s| {
            let (tx, rx) = mpsc::sync_channel::<(usize, Vec<Achado>, usize)>(trabalhadores * 2);
            for _ in 0..trabalhadores {
                let tx = tx.clone();
                let (blocos, proximo, entregues) = (&blocos, &proximo, &entregues);
                s.spawn(move || {
                    loop {
                        if cancelar.load(Relaxed) {
                            return;
                        }
                        let i = proximo.fetch_add(1, Relaxed);
                        let Some(faixa) = blocos.get(i) else { return };
                        while i >= entregues.load(Relaxed) + ADIANTE_MAX {
                            if cancelar.load(Relaxed) {
                                return;
                            }
                            thread::sleep(Duration::from_micros(100));
                        }
                        let (achados, quebras) = self.varrer_bloco(&dados[faixa.clone()]);
                        if tx.send((i, achados, quebras)).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(tx);

            let mut pendentes = BTreeMap::new();
            let (mut prox, mut linha0, mut ultimo_aviso) = (0, 0, 0);
            'receber: for (i, achados, quebras) in &rx {
                pendentes.insert(i, (achados, quebras));
                while let Some((mut achados, quebras)) = pendentes.remove(&prox) {
                    // Conferido a cada bloco, não a cada mensagem: uma mensagem pode liberar
                    // vários blocos que esperavam, e nenhum lote sai depois do cancelamento.
                    if cancelar.load(Relaxed) {
                        break 'receber;
                    }
                    for a in &mut achados {
                        a.linha += linha0;
                    }
                    linha0 += quebras;
                    resumo.achados += achados.len() as u64;
                    resumo.varridos = blocos[prox].end;
                    prox += 1;
                    entregues.store(prox, Relaxed);
                    let fim = prox == blocos.len();
                    if !achados.is_empty() || fim || resumo.varridos - ultimo_aviso >= total / 100 {
                        ultimo_aviso = resumo.varridos;
                        ao_lote(Lote {
                            achados,
                            varridos: resumo.varridos,
                            total,
                        });
                    }
                }
            }
            // Sem o receptor, quem estiver preso no `send` desiste.
            drop(rx);
            resumo.cancelado = prox < blocos.len();
        });
        resumo
    }

    /// Próximo achado em `texto` a partir de `pos` (`pos <= texto.len()`).
    fn proximo(&self, texto: &[u8], pos: usize) -> Option<(usize, usize)> {
        match &self.motor {
            Motor::Literal(f) => f
                .find(&texto[pos..])
                .map(|i| (pos + i, pos + i + f.needle().len())),
            Motor::Regex(re) => re.find_at(texto, pos).map(|m| (m.start(), m.end())),
        }
    }

    /// Achados de um bloco que começa no início de uma linha, com a linha relativa ao bloco;
    /// devolve também quantas quebras de linha o bloco tem.
    fn varrer_bloco(&self, b: &[u8]) -> (Vec<Achado>, usize) {
        let mut achados = Vec::new();
        // Linha atual: número relativo, início e posição do `\n` que a termina (ou o fim).
        let (mut linha, mut ini_linha) = (0, 0);
        let mut nl = memchr(b'\n', b).unwrap_or(b.len());
        let mut pos = 0;
        while pos <= b.len() {
            let Some((mut i, mut f)) = self.proximo(b, pos) else {
                break;
            };
            if i > nl {
                linha += 1 + memchr_iter(b'\n', &b[nl + 1..i]).count();
                ini_linha = memrchr(b'\n', &b[..i]).map_or(0, |p| p + 1);
                nl = memchr(b'\n', &b[i..]).map_or(b.len(), |p| i + p);
            }
            // `\r` de CRLF não é conteúdo da linha (mesma regra do índice).
            let fim_linha = if nl < b.len() && nl > ini_linha && b[nl - 1] == b'\r' {
                nl - 1
            } else {
                nl
            };
            if f > fim_linha {
                // Passou do fim da linha: refaz a busca só dentro dela.
                let refeito = (i < fim_linha)
                    .then(|| self.proximo(&b[..fim_linha], i))
                    .flatten();
                match refeito {
                    Some(m) => (i, f) = m,
                    None => {
                        pos = nl + 1;
                        continue;
                    }
                }
            }
            if f == i {
                pos = i + 1;
                continue;
            }
            achados.push(Achado {
                linha,
                coluna: i - ini_linha,
                comprimento: f - i,
            });
            pos = f;
        }
        (achados, memchr_iter(b'\n', b).count())
    }
}

/// Blocos de ~`alvo` bytes a partir de `base`, cada um terminando logo depois de um `\n`
/// (o último pode terminar no fim do arquivo).
pub(crate) fn fatiar(dados: &[u8], base: usize, alvo: usize) -> Vec<Range<usize>> {
    let mut blocos = Vec::with_capacity((dados.len() - base) / alvo + 1);
    let mut ini = base;
    while ini < dados.len() {
        let corte = (ini + alvo).min(dados.len()) - 1;
        let fim = memchr(b'\n', &dados[corte..]).map_or(dados.len(), |p| corte + p + 1);
        blocos.push(ini..fim);
        ini = fim;
    }
    blocos
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::IndiceLinhas;

    fn achar(dados: &[u8], padrao: &str, regex: bool, caixa: bool, bloco: usize) -> Vec<Achado> {
        let busca = Busca::nova(padrao, regex, caixa).unwrap().com_bloco(bloco);
        let mut v = Vec::new();
        let r = busca.executar(dados, &AtomicBool::new(false), |l| v.extend(l.achados));
        assert_eq!(r.achados, v.len() as u64);
        assert!(!r.cancelado);
        assert_eq!(r.varridos, r.total);
        v
    }

    fn a(linha: usize, coluna: usize, comprimento: usize) -> Achado {
        Achado {
            linha,
            coluna,
            comprimento,
        }
    }

    /// Oráculo ingênuo: linha a linha, pelo índice, sem blocos nem threads.
    fn ingenuo(dados: &[u8], agulha: &[u8]) -> Vec<Achado> {
        let ind = IndiceLinhas::construir(dados);
        let mut v = Vec::new();
        for l in 0..ind.len() {
            let linha = &dados[ind.faixa(l, dados).unwrap()];
            let mut p = 0;
            while let Some(i) = memmem::find(&linha[p..], agulha) {
                v.push(a(l, p + i, agulha.len()));
                p += i + agulha.len();
            }
        }
        v
    }

    #[test]
    fn igual_ao_ingenuo_em_qualquer_tamanho_de_bloco() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 50_000, 3).unwrap();
        for padrao in ["erro", "\"id\":1", "Ana Exemplo", "}"] {
            let esperado = ingenuo(&dados, padrao.as_bytes());
            assert!(!esperado.is_empty(), "{padrao}");
            for bloco in [1, 7, 100, 4096, BLOCO_PADRAO] {
                assert_eq!(achar(&dados, padrao, false, false, bloco), esperado);
                let re = regex::escape(padrao);
                assert_eq!(achar(&dados, &re, true, false, bloco), esperado);
            }
        }
    }

    #[test]
    fn achado_na_fronteira_de_blocos() {
        // Com bloco de 8 bytes, um corte ingênuo cairia no meio de "agulha".
        let dados = b"xxxxxagulha\nyy agulha agulha\n";
        let esperado = [a(0, 5, 6), a(1, 3, 6), a(1, 10, 6)];
        for bloco in 1..dados.len() + 2 {
            assert_eq!(achar(dados, "agulha", false, false, bloco), esperado);
            assert_eq!(achar(dados, "agu.ha", true, false, bloco), esperado);
        }
    }

    #[test]
    fn padrao_vazio_e_quebra_de_linha() {
        for regex in [false, true] {
            assert_eq!(
                Busca::nova("", regex, false).err(),
                Some(ErroBusca::PadraoVazio)
            );
        }
        assert_eq!(
            Busca::nova("a\nb", false, true).err(),
            Some(ErroBusca::QuebraDeLinha)
        );
    }

    #[test]
    fn regex_invalida_com_mensagem_clara() {
        let e = Busca::nova("(abc", true, false).err().unwrap();
        let msg = e.to_string();
        assert!(msg.starts_with("expressão regular inválida"), "{msg}");
        assert!(msg.contains("unclosed group"), "{msg}");
    }

    #[test]
    fn regex_nao_cruza_linha_e_ancora_por_linha() {
        let dados = b"a\nb a\n";
        assert_eq!(achar(dados, r"a\s+b", true, false, 64), []);
        assert_eq!(
            achar(dados, r"a\s*", true, false, 64),
            [a(0, 0, 1), a(1, 2, 1)]
        );
        assert_eq!(
            achar(dados, "^.", true, false, 64),
            [a(0, 0, 1), a(1, 0, 1)]
        );
        assert_eq!(
            achar(dados, "a$", true, false, 64),
            [a(0, 0, 1), a(1, 2, 1)]
        );
        // Achado vazio é ignorado.
        assert_eq!(achar(dados, "x*", true, false, 64), []);
    }

    #[test]
    fn ignorar_caixa_com_unicode() {
        let dados = "Ação\nAÇÃO\naçao\n".as_bytes();
        assert_eq!(
            achar(dados, "ação", false, true, 4),
            [a(0, 0, 6), a(1, 0, 6)]
        );
        assert_eq!(achar(dados, "ação", false, false, 4), []);
        assert_eq!(achar(dados, "(?i)AÇ", true, false, 4).len(), 3);
        // Metacaractere em busca literal é texto.
        assert_eq!(achar(b"a.b axb\n", "a.b", false, true, 4), [a(0, 0, 3)]);
    }

    #[test]
    fn utf8_invalido_no_arquivo() {
        let dados = b"\xff\xfeagulha\xc3\n\x80agulha\n";
        let esperado = [a(0, 2, 6), a(1, 1, 6)];
        assert_eq!(achar(dados, "agulha", false, false, 3), esperado);
        assert_eq!(achar(dados, "agulha", true, false, 3), esperado);
        assert_eq!(achar(dados, "AGULHA", false, true, 3), esperado);
        // `.` não casa com byte inválido; o resto do arquivo segue.
        assert_eq!(achar(dados, "^.agulha", true, false, 3), []);
    }

    #[test]
    fn crlf_e_bom() {
        let dados = b"\xEF\xBB\xBFab\r\ncab\r\nab\r";
        assert_eq!(
            achar(dados, "ab", false, false, 2),
            [a(0, 0, 2), a(1, 1, 2), a(2, 0, 2)]
        );
        // Limite do modo CRLF da `regex`: `$` também casa antes de `\r` solto.
        assert_eq!(
            achar(dados, "b$", true, false, 2),
            [a(0, 1, 1), a(1, 2, 1), a(2, 1, 1)]
        );
        // O `\r` de CRLF não é da linha; o `\r` final sem `\n` é.
        assert_eq!(achar(dados, "b\r", false, false, 2), [a(2, 1, 2)]);
        assert_eq!(achar(dados, r"b\s", true, false, 2), [a(2, 1, 2)]);
        assert_eq!(achar(dados, r"a\w+\s*", true, false, 2).len(), 3);
    }

    #[test]
    fn arquivo_vazio_e_so_bom() {
        for dados in [&b""[..], b"\xEF\xBB\xBF"] {
            let busca = Busca::nova("a", false, false).unwrap();
            let mut lotes = 0;
            let r = busca.executar(dados, &AtomicBool::new(false), |_| lotes += 1);
            assert_eq!(
                r,
                Resumo {
                    achados: 0,
                    varridos: dados.len(),
                    total: dados.len(),
                    cancelado: false
                }
            );
            assert_eq!(lotes, 0);
        }
    }

    #[test]
    fn cancelar_no_meio() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 4 << 20, 5).unwrap();
        let busca = Busca::nova("evento", false, false).unwrap().com_bloco(4096);
        let cancelar = AtomicBool::new(false);
        let (mut lotes, mut ultimo) = (0, 0);
        let r = busca.executar(&dados, &cancelar, |l| {
            assert!(l.varridos > ultimo);
            ultimo = l.varridos;
            lotes += 1;
            if lotes == 3 {
                cancelar.store(true, Relaxed);
            }
        });
        assert!(r.cancelado);
        assert_eq!(lotes, 3);
        assert!(r.varridos < r.total);
        assert_eq!(r.varridos, ultimo);
    }

    #[test]
    fn lotes_em_ordem_com_progresso() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 1 << 20, 9).unwrap();
        let busca = Busca::nova("sensor", false, false).unwrap().com_bloco(1000);
        let mut anterior = (0, 0);
        let mut ultimo = Lote::default();
        busca.executar(&dados, &AtomicBool::new(false), |l| {
            for x in &l.achados {
                assert!((x.linha, x.coluna) > anterior || anterior == (0, 0));
                anterior = (x.linha, x.coluna);
            }
            ultimo = l;
        });
        assert_eq!(ultimo.varridos, dados.len());
        assert_eq!(ultimo.total, dados.len());
    }
}
