//! Árvore de valores de um JSONL: a lista plana de linhas visíveis que a interface desenha.
//!
//! Cada registro (linha do arquivo) começa recolhido e ocupa 1 linha. Expandir analisa só aquele
//! registro (sonic-rs, ver docs/decisoes.md) e insere os nós abertos logo abaixo dele. A
//! interface só pede janelas `(início, quantidade)` dessa lista.
//!
//! Fidelidade: o valor escalar é mostrado como o texto cru do arquivo (número de 400 dígitos
//! fica intacto); chaves saem na ordem do arquivo, inclusive repetidas.

use std::collections::BTreeMap;
use std::io;
use std::ops::Range;
use std::path::Path;

use serde::Serialize;

use crate::ArquivoLinhas;

/// Aninhamento máximo de um registro. Acima disso o registro vira linha de erro: protege a pilha
/// (a análise é recursiva) e mantém a interface legível.
pub const MAX_PROFUNDIDADE: usize = 1_000;
/// Bytes de texto cru na prévia de um valor.
pub const MAX_PREVIA: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tipo {
    Objeto,
    Lista,
    Texto,
    Numero,
    Booleano,
    Nulo,
    /// Registro que não é JSON válido; `erro` traz posição e motivo, `previa` o texto cru.
    Erro,
}

/// Por que um registro não abre. A interface traduz o código; o texto do parser não chega lá.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Motivo {
    Utf8,
    /// Aninhamento acima de `MAX_PROFUNDIDADE`.
    Profundidade,
    FimInesperado,
    EsperavaDoisPontos,
    EsperavaVirgulaOuFimDaLista,
    EsperavaVirgulaOuFimDoObjeto,
    EsperavaChaveOuFimDoObjeto,
    Literal,
    Valor,
    Escape,
    Numero,
    Unicode,
    Controle,
    VirgulaSobrando,
    TextoDepoisDoValor,
    Outro,
}

/// Erro de um registro: byte (base 0, na linha) e motivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ErroLinha {
    pub byte: usize,
    pub motivo: Motivo,
}

/// Uma linha pronta para desenhar.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Linha {
    /// Linha do arquivo (base 0) a que o nó pertence.
    pub registro: usize,
    /// 0 = o registro; 1 = seus campos; e assim por diante.
    pub prof: usize,
    /// Nome do campo; `None` no registro e em item de lista.
    pub chave: Option<String>,
    /// Posição do item na lista ou do campo no objeto.
    pub pos: usize,
    pub tipo: Tipo,
    /// Texto cru do valor, cortado em `MAX_PREVIA` bytes (com "…").
    pub previa: String,
    /// Só em `Tipo::Erro`.
    pub erro: Option<ErroLinha>,
    /// Quantos campos ou itens tem (objeto e lista).
    pub filhos: usize,
    /// Quantos campos ou itens tem o pai (0 no registro).
    pub irmaos: usize,
    pub expansivel: bool,
    pub aberto: bool,
    /// Caminho no estilo jq, relativo ao registro: `.pedidos[0]["nome completo"]`.
    pub caminho: String,
    /// Rótulos do caminho para as migalhas: `["pedidos", "[0]", "nome completo"]`.
    pub migalhas: Vec<String>,
}

const SEM_PAI: usize = usize::MAX;

struct No {
    pai: usize,
    prof: usize,
    chave: Option<Box<str>>,
    pos: usize,
    tipo: Tipo,
    /// Bytes do valor dentro da linha.
    faixa: Range<usize>,
    filhos: Range<usize>,
    aberto: bool,
}

/// Registro expandido: todos os nós (o `nos[0]` é o próprio registro) e, em ordem, os que
/// aparecem abaixo dele.
struct Aberto {
    nos: Vec<No>,
    visiveis: Vec<usize>,
}

/// Documento aberto: o arquivo e a lista plana de linhas visíveis.
pub struct Arvore {
    arquivo: ArquivoLinhas,
    // shortcut: localizar uma linha percorre os registros abertos (O(abertos)); trocar por
    // árvore de Fenwick se surgir "expandir todos os registros".
    abertos: BTreeMap<usize, Aberto>,
    total: usize,
}

impl Arvore {
    pub fn abrir(caminho: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self::new(ArquivoLinhas::abrir(caminho)?))
    }

    pub fn new(arquivo: ArquivoLinhas) -> Self {
        let total = arquivo.linhas();
        Self {
            arquivo,
            abertos: BTreeMap::new(),
            total,
        }
    }

    pub fn registros(&self) -> usize {
        self.arquivo.linhas()
    }

    /// Total de linhas visíveis.
    pub fn total(&self) -> usize {
        self.total
    }

    fn bytes(&self, registro: usize) -> &[u8] {
        self.arquivo
            .janela(registro, 1)
            .first()
            .copied()
            .unwrap_or_default()
    }

    /// Linha visível `v` → (registro, posição em `visiveis` ou `None` se é a linha do registro).
    fn localizar(&self, v: usize) -> Option<(usize, Option<usize>)> {
        if v >= self.total {
            return None;
        }
        let mut extra = 0;
        for (&r, a) in &self.abertos {
            let linha_r = r + extra;
            if v <= linha_r {
                return Some((v - extra, None));
            }
            if v <= linha_r + a.visiveis.len() {
                return Some((r, Some(v - linha_r - 1)));
            }
            extra += a.visiveis.len();
        }
        Some((v - extra, None))
    }

    /// Linha visível do registro `r` (para "ir para linha").
    pub fn linha_do_registro(&self, r: usize) -> Option<usize> {
        (r < self.registros()).then(|| {
            r + self
                .abertos
                .range(..r)
                .map(|(_, a)| a.visiveis.len())
                .sum::<usize>()
        })
    }

    /// Até `quantidade` linhas a partir da linha visível `inicio`.
    pub fn janela(&self, inicio: usize, quantidade: usize) -> Vec<Linha> {
        let mut saida = Vec::with_capacity(quantidade.min(self.total.saturating_sub(inicio)));
        let Some((mut r, mut pos)) = self.localizar(inicio) else {
            return saida;
        };
        while saida.len() < quantidade && r < self.registros() {
            let aberto = self.abertos.get(&r);
            match (pos, aberto) {
                (None, _) => {
                    saida.push(self.linha_registro(r));
                    pos = aberto.map(|_| 0);
                    if aberto.is_none() {
                        r += 1;
                    }
                }
                (Some(p), Some(a)) if p < a.visiveis.len() => {
                    saida.push(linha_no(r, self.bytes(r), &a.nos, a.visiveis[p]));
                    pos = Some(p + 1);
                }
                _ => {
                    r += 1;
                    pos = None;
                }
            }
        }
        saida
    }

    fn linha_registro(&self, r: usize) -> Linha {
        let bytes = self.bytes(r);
        if let Some(a) = self.abertos.get(&r) {
            return linha_no(r, bytes, &a.nos, 0);
        }
        let mut linha = Linha {
            registro: r,
            prof: 0,
            chave: None,
            pos: 0,
            tipo: Tipo::Erro,
            previa: String::new(),
            erro: None,
            filhos: 0,
            irmaos: 0,
            expansivel: false,
            aberto: false,
            caminho: ".".into(),
            migalhas: Vec::new(),
        };
        match validar(bytes) {
            Err(e) => {
                linha.erro = Some(e);
                linha.previa = previa(String::from_utf8_lossy(bytes).trim_matches(ESPACO));
            }
            Ok(texto) => {
                let raw = texto.trim_matches(ESPACO);
                linha.tipo = tipo_de(raw);
                linha.filhos = contar(raw);
                linha.expansivel = linha.filhos > 0;
                linha.previa = previa(raw);
            }
        }
        linha
    }

    /// Expande ou recolhe a linha visível `v`. Devolve `false` se ela não é expansível.
    pub fn alternar(&mut self, v: usize) -> bool {
        let Some((r, pos)) = self.localizar(v) else {
            return false;
        };
        let Some(p) = pos else {
            if let Some(a) = self.abertos.remove(&r) {
                self.total -= a.visiveis.len();
                return true;
            }
            let Ok(nos) = construir(self.bytes(r)) else {
                return false;
            };
            if nos[0].filhos.is_empty() {
                return false;
            }
            let mut visiveis = Vec::new();
            descendentes_visiveis(&nos, 0, &mut visiveis);
            self.total += visiveis.len();
            self.abertos.insert(r, Aberto { nos, visiveis });
            return true;
        };
        let a = self
            .abertos
            .get_mut(&r)
            .expect("localizar só devolve pos de aberto");
        let id = a.visiveis[p];
        if a.nos[id].filhos.is_empty() {
            return false;
        }
        let antes = a.visiveis.len();
        if a.nos[id].aberto {
            let prof = a.nos[id].prof;
            let fim = a.visiveis[p + 1..]
                .iter()
                .position(|&n| a.nos[n].prof <= prof)
                .map_or(a.visiveis.len(), |i| p + 1 + i);
            a.visiveis.drain(p + 1..fim);
            a.nos[id].aberto = false;
        } else {
            a.nos[id].aberto = true;
            let mut novos = Vec::new();
            descendentes_visiveis(&a.nos, id, &mut novos);
            a.visiveis.splice(p + 1..p + 1, novos);
        }
        self.total = self.total + a.visiveis.len() - antes;
        true
    }

    /// Linha visível do pai de `v` (seta para a esquerda); `None` para linha de registro.
    pub fn pai(&self, v: usize) -> Option<usize> {
        let (r, Some(p)) = self.localizar(v)? else {
            return None;
        };
        let a = &self.abertos[&r];
        let prof = a.nos[a.visiveis[p]].prof;
        let acima = a.visiveis[..p]
            .iter()
            .rposition(|&n| a.nos[n].prof < prof)
            .map_or(0, |i| i + 1);
        Some(v - p - 1 + acima)
    }
}

/// Filhos de `id` que aparecem, em ordem, descendo nos que estão abertos.
fn descendentes_visiveis(nos: &[No], id: usize, saida: &mut Vec<usize>) {
    for f in nos[id].filhos.clone() {
        saida.push(f);
        if nos[f].aberto {
            descendentes_visiveis(nos, f, saida);
        }
    }
}

const ESPACO: &[char] = &[' ', '\t', '\n', '\r'];

fn tipo_de(raw: &str) -> Tipo {
    match raw.as_bytes().first() {
        Some(b'{') => Tipo::Objeto,
        Some(b'[') => Tipo::Lista,
        Some(b'"') => Tipo::Texto,
        Some(b't' | b'f') => Tipo::Booleano,
        Some(b'n') => Tipo::Nulo,
        _ => Tipo::Numero,
    }
}

/// Campos ou itens de um valor já validado.
fn contar(raw: &str) -> usize {
    match tipo_de(raw) {
        Tipo::Objeto => sonic_rs::to_object_iter(raw).count(),
        Tipo::Lista => sonic_rs::to_array_iter(raw).count(),
        _ => 0,
    }
}

fn previa(raw: &str) -> String {
    if raw.len() <= MAX_PREVIA {
        return raw.to_owned();
    }
    let mut fim = MAX_PREVIA;
    while !raw.is_char_boundary(fim) {
        fim -= 1;
    }
    format!("{}…", &raw[..fim])
}

/// Confere UTF-8, profundidade e sintaxe.
fn validar(bytes: &[u8]) -> Result<&str, ErroLinha> {
    let erro = |byte, motivo| ErroLinha { byte, motivo };
    let texto = std::str::from_utf8(bytes).map_err(|e| erro(e.valid_up_to(), Motivo::Utf8))?;
    if let Some(pos) = passa_da_profundidade(bytes) {
        return Err(erro(pos, Motivo::Profundidade));
    }
    // IgnoredAny no sonic-rs valida o valor inteiro e o que vem depois dele.
    // shortcut: não confere os dígitos de `\uXXXX` (ex.: `"\uZZZZ"` passa); validar com
    // `sonic_rs::Value` se isso aparecer em arquivo real.
    sonic_rs::from_str::<serde::de::IgnoredAny>(texto).map_err(|e| do_sonic(&e))?;
    Ok(texto)
}

/// O sonic-rs não expõe o código do erro, só a mensagem; o mapa é pelo começo dela
/// (versão fixada no Cargo.lock; o teste `motivos_do_sonic` acusa se mudar).
fn do_sonic(e: &sonic_rs::Error) -> ErroLinha {
    const MAPA: &[(&str, Motivo)] = &[
        ("EOF while parsing", Motivo::FimInesperado),
        (
            "Expected this character to be a ':'",
            Motivo::EsperavaDoisPontos,
        ),
        (
            "Expected this character to be either a ',' or a ']'",
            Motivo::EsperavaVirgulaOuFimDaLista,
        ),
        (
            "Expected this character to be either a ',' or a '}'",
            Motivo::EsperavaVirgulaOuFimDoObjeto,
        ),
        (
            "Expected this character to be '\"' or '}'",
            Motivo::EsperavaChaveOuFimDoObjeto,
        ),
        ("Invalid literal", Motivo::Literal),
        ("Invalid JSON value", Motivo::Valor),
        ("Invalid escape", Motivo::Escape),
        ("Invalid number", Motivo::Numero),
        ("Invalid unicode", Motivo::Unicode),
        ("Invalid surrogate", Motivo::Unicode),
        ("Invalid UTF-8", Motivo::Utf8),
        ("Control character", Motivo::Controle),
        (
            "JSON has a comma after the last value",
            Motivo::VirgulaSobrando,
        ),
        (
            "JSON has non-whitespace trailing characters",
            Motivo::TextoDepoisDoValor,
        ),
    ];
    let texto = e.to_string();
    let motivo = MAPA
        .iter()
        .find(|(m, _)| texto.starts_with(m))
        .map_or(Motivo::Outro, |&(_, m)| m);
    ErroLinha {
        byte: e.offset(),
        motivo,
    }
}

/// Primeiro byte em que o aninhamento passa de `MAX_PROFUNDIDADE`, ignorando o que está em texto.
fn passa_da_profundidade(bytes: &[u8]) -> Option<usize> {
    let (mut prof, mut em_texto, mut escape) = (0usize, false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if em_texto {
            match (escape, b) {
                (true, _) => escape = false,
                (false, b'\\') => escape = true,
                (false, b'"') => em_texto = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => em_texto = true,
            b'{' | b'[' => {
                prof += 1;
                if prof > MAX_PROFUNDIDADE {
                    return Some(i);
                }
            }
            b'}' | b']' => prof = prof.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// Analisa um registro inteiro em nós; filhos de cada nó ficam contíguos em `nos`.
fn construir(bytes: &[u8]) -> Result<Vec<No>, ErroLinha> {
    let texto = validar(bytes)?;
    let raw = texto.trim_matches(ESPACO);
    let ini = raw.as_ptr() as usize - texto.as_ptr() as usize;
    let mut nos = vec![No {
        pai: SEM_PAI,
        prof: 0,
        chave: None,
        pos: 0,
        tipo: tipo_de(raw),
        faixa: ini..ini + raw.len(),
        filhos: 0..0,
        aberto: true,
    }];
    preencher(texto, &mut nos, 0).map_err(|e| do_sonic(&e))?;
    Ok(nos)
}

fn preencher(texto: &str, nos: &mut Vec<No>, id: usize) -> sonic_rs::Result<()> {
    let raw = &texto[nos[id].faixa.clone()];
    let base = texto.as_ptr() as usize;
    let ini = nos.len();
    let prof = nos[id].prof + 1;
    let tipo = nos[id].tipo;
    let mut novo = |chave: Option<Box<str>>, valor: &str| {
        let inicio = valor.as_ptr() as usize - base;
        nos.push(No {
            pai: id,
            prof,
            chave,
            pos: nos.len() - ini,
            tipo: tipo_de(valor),
            faixa: inicio..inicio + valor.len(),
            filhos: 0..0,
            aberto: false,
        });
    };
    match tipo {
        Tipo::Objeto => {
            for par in sonic_rs::to_object_iter(raw) {
                let (k, v) = par?;
                novo(Some(Box::from(&*k)), v.as_raw_str());
            }
        }
        Tipo::Lista => {
            for v in sonic_rs::to_array_iter(raw) {
                novo(None, v?.as_raw_str());
            }
        }
        _ => return Ok(()),
    }
    let fim = nos.len();
    nos[id].filhos = ini..fim;
    for f in ini..fim {
        preencher(texto, nos, f)?;
    }
    Ok(())
}

fn linha_no(registro: usize, bytes: &[u8], nos: &[No], id: usize) -> Linha {
    // A linha já passou por `validar` em `construir`.
    let texto = std::str::from_utf8(bytes).unwrap_or_default();
    let no = &nos[id];
    let mut cadeia = Vec::with_capacity(no.prof);
    let mut n = id;
    while nos[n].pai != SEM_PAI {
        cadeia.push(n);
        n = nos[n].pai;
    }
    let (mut caminho, mut migalhas) = (String::new(), Vec::with_capacity(cadeia.len()));
    for &n in cadeia.iter().rev() {
        match &nos[n].chave {
            Some(k) => {
                migalhas.push(k.to_string());
                if eh_identificador(k) {
                    caminho.push('.');
                    caminho.push_str(k);
                } else {
                    if caminho.is_empty() {
                        caminho.push('.');
                    }
                    caminho.push('[');
                    caminho.push_str(&sonic_rs::to_string(&**k).unwrap_or_default());
                    caminho.push(']');
                }
            }
            None => {
                let seg = format!("[{}]", nos[n].pos);
                if caminho.is_empty() {
                    caminho.push('.');
                }
                caminho.push_str(&seg);
                migalhas.push(seg);
            }
        }
    }
    if caminho.is_empty() {
        caminho.push('.');
    }
    let filhos = no.filhos.len();
    Linha {
        registro,
        prof: no.prof,
        chave: no.chave.as_deref().map(str::to_owned),
        pos: no.pos,
        tipo: no.tipo,
        previa: previa(texto.get(no.faixa.clone()).unwrap_or_default()),
        erro: None,
        filhos,
        irmaos: nos.get(no.pai).map_or(0, |p| p.filhos.len()),
        expansivel: filhos > 0,
        aberto: filhos > 0 && no.aberto,
        caminho,
        migalhas,
    }
}

fn eh_identificador(k: &str) -> bool {
    let mut c = k.bytes();
    c.next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && c.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::io::Write;

    fn arvore(conteudo: &str) -> Arvore {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(conteudo.as_bytes()).unwrap();
        Arvore::abrir(f.path()).unwrap()
    }

    /// (prof, chave ou caminho, prévia) de toda a lista visível.
    fn tudo(a: &Arvore) -> Vec<(usize, String, String)> {
        a.janela(0, usize::MAX)
            .into_iter()
            .map(|l| (l.prof, l.caminho, l.previa))
            .collect()
    }

    const DOIS: &str = "{\"a\":1,\"b\":{\"c\":[true,null]},\"d\":\"x\"}\n[1,2]\n";

    #[test]
    fn expandir_e_recolher() {
        let mut a = arvore(DOIS);
        assert_eq!(a.total(), 2);
        assert!(a.alternar(0));
        assert_eq!(a.total(), 5);
        let linhas = a.janela(0, 10);
        let caminhos: Vec<_> = linhas.iter().map(|l| l.caminho.as_str()).collect();
        assert_eq!(caminhos, [".", ".a", ".b", ".d", "."]);
        assert_eq!(linhas[1].previa, "1");
        assert_eq!(linhas[1].tipo, Tipo::Numero);
        assert_eq!(linhas[2].filhos, 1);
        assert_eq!(linhas[4].registro, 1);
        assert_eq!(linhas[4].filhos, 2);

        // Abre .b e depois .b.c; recolher .b esconde os dois, reabrir mostra .c ainda aberto.
        assert!(a.alternar(2));
        assert!(a.alternar(3));
        assert_eq!(a.total(), 8);
        assert_eq!(a.janela(4, 1)[0].caminho, ".b.c[0]");
        assert_eq!(a.janela(4, 1)[0].migalhas, ["b", "c", "[0]"]);
        assert_eq!(a.pai(5), Some(3));
        assert_eq!(a.pai(3), Some(2));
        assert_eq!(a.pai(2), Some(0));
        assert_eq!(a.pai(0), None);
        assert!(a.alternar(2));
        assert_eq!(a.total(), 5);
        assert!(a.alternar(2));
        assert_eq!(a.total(), 8);
        assert!(!a.alternar(1), "escalar não expande");

        // Registro seguinte, depois do primeiro aberto.
        assert_eq!(a.linha_do_registro(1), Some(7));
        assert!(a.alternar(7));
        assert_eq!(
            tudo(&a)[7..],
            [
                (0, ".".into(), "[1,2]".into()),
                (1, ".[0]".into(), "1".into()),
                (1, ".[1]".into(), "2".into())
            ]
        );
        assert_eq!(a.pai(9), Some(7));

        // Recolher o primeiro registro tira seus 6 nós; o segundo continua aberto.
        assert!(a.alternar(0));
        assert_eq!(a.total(), 4);
        assert_eq!(a.linha_do_registro(1), Some(1));
    }

    #[test]
    fn janelas_nas_bordas() {
        let mut a = arvore(DOIS);
        a.alternar(0);
        assert_eq!(a.janela(5, 10), vec![]);
        assert_eq!(a.janela(usize::MAX, 10), vec![]);
        assert_eq!(a.janela(0, 0), vec![]);
        assert_eq!(a.janela(4, 10).len(), 1);
        assert_eq!(a.janela(3, 2)[1].registro, 1);
        // Janela começando no meio do registro aberto.
        assert_eq!(a.janela(2, 1)[0].caminho, ".b");
        assert!(!a.alternar(99));
        assert_eq!(arvore("").janela(0, 10), vec![]);
    }

    #[test]
    fn linha_invalida_nao_impede_o_resto() {
        let mut a = arvore("{\"a\":1}\n{\"a\":tru}\n[1,]\n{\"a\":1} x\n\"\\x\"\n[1]\n");
        let linhas = a.janela(0, 10);
        let tipos: Vec<_> = linhas.iter().map(|l| l.tipo).collect();
        assert_eq!(
            tipos,
            [
                Tipo::Objeto,
                Tipo::Erro,
                Tipo::Erro,
                Tipo::Erro,
                Tipo::Erro,
                Tipo::Lista
            ]
        );
        let erros: Vec<_> = linhas[1..5].iter().map(|l| l.erro.unwrap()).collect();
        let e = |byte, motivo| ErroLinha { byte, motivo };
        assert_eq!(
            erros,
            [
                e(8, Motivo::Literal),
                e(3, Motivo::Valor),
                e(8, Motivo::TextoDepoisDoValor),
                e(2, Motivo::Escape),
            ]
        );
        assert_eq!(linhas[1].previa, "{\"a\":tru}", "erro mostra o texto cru");
        assert!(!a.alternar(1), "linha inválida não expande");
        assert!(a.alternar(5));
        assert_eq!(a.total(), 7);
    }

    #[test]
    fn utf8_invalido() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"\"a\xff\"\n").unwrap();
        let a = Arvore::abrir(f.path()).unwrap();
        let l = &a.janela(0, 1)[0];
        assert_eq!(
            (l.tipo, l.erro, l.previa.as_str()),
            (
                Tipo::Erro,
                Some(ErroLinha {
                    byte: 2,
                    motivo: Motivo::Utf8
                }),
                "\"a\u{fffd}\""
            )
        );
    }

    #[test]
    fn numero_de_400_digitos_fica_intacto() {
        let n = "9".repeat(400);
        let mut a = arvore(&format!("{{\"n\":{n}.5e-3}}\n"));
        a.alternar(0);
        let l = &a.janela(1, 1)[0];
        assert_eq!(l.tipo, Tipo::Numero);
        // A prévia corta em MAX_PREVIA, mas sem arredondar nada.
        assert_eq!(l.previa, format!("{}…", &n[..MAX_PREVIA]));
        let curto = "123456789012345678901234567890.000";
        let mut a = arvore(&format!("[{curto}]"));
        a.alternar(0);
        assert_eq!(a.janela(1, 1)[0].previa, curto);
    }

    #[test]
    fn chave_duplicada_e_ordem_preservadas() {
        let mut a = arvore("{\"z\":1,\"a\":2,\"z\":3,\"nome completo\":4,\"\":5}");
        a.alternar(0);
        let l: Vec<_> = a
            .janela(1, 10)
            .into_iter()
            .map(|l| (l.chave.unwrap(), l.caminho, l.previa))
            .collect();
        let s = |a: &str, b: &str, c: &str| (a.to_owned(), b.to_owned(), c.to_owned());
        assert_eq!(
            l,
            [
                s("z", ".z", "1"),
                s("a", ".a", "2"),
                s("z", ".z", "3"),
                s("nome completo", ".[\"nome completo\"]", "4"),
                s("", ".[\"\"]", "5"),
            ]
        );
    }

    #[test]
    fn profundidade_no_limite_abre_e_acima_vira_erro() {
        let fundo = |n: usize| format!("{}{}\n", "[".repeat(n), "]".repeat(n));
        let mut a = arvore(&(fundo(MAX_PROFUNDIDADE) + &fundo(MAX_PROFUNDIDADE + 1)));
        let linhas = a.janela(0, 2);
        assert_eq!(linhas[0].tipo, Tipo::Lista);
        assert_eq!(linhas[1].tipo, Tipo::Erro);
        assert_eq!(
            linhas[1].erro,
            Some(ErroLinha {
                byte: 1000,
                motivo: Motivo::Profundidade
            })
        );
        // Abre até o fundo, um nível por vez.
        for v in 0..MAX_PROFUNDIDADE - 1 {
            assert!(a.alternar(v), "nível {v}");
        }
        let ultimo = &a.janela(MAX_PROFUNDIDADE - 1, 1)[0];
        assert_eq!((ultimo.prof, ultimo.filhos), (MAX_PROFUNDIDADE - 1, 0));
        // Colchete dentro de texto não conta.
        assert_eq!(
            passa_da_profundidade(format!("[\"{}\"]", "[".repeat(5000)).as_bytes()),
            None
        );
    }

    #[test]
    fn escalar_e_vazios_no_registro() {
        let a = arvore("42\n{}\n[]\n  {\"a\" : 1}  \n");
        let l = a.janela(0, 4);
        assert_eq!(
            l.iter()
                .map(|l| (l.tipo, l.expansivel, l.previa.as_str()))
                .collect::<Vec<_>>(),
            [
                (Tipo::Numero, false, "42"),
                (Tipo::Objeto, false, "{}"),
                (Tipo::Lista, false, "[]"),
                (Tipo::Objeto, true, "{\"a\" : 1}"),
            ]
        );
    }

    #[test]
    fn motivos_do_sonic() {
        let casos = [
            ("{\"a\":1", Motivo::FimInesperado),
            ("{\"a\" 1}", Motivo::EsperavaDoisPontos),
            ("[1 2]", Motivo::EsperavaVirgulaOuFimDaLista),
            ("{\"a\":1 \"b\":2}", Motivo::EsperavaVirgulaOuFimDoObjeto),
            ("{1:2}", Motivo::EsperavaChaveOuFimDoObjeto),
            ("[nul]", Motivo::Literal),
            ("[-]", Motivo::Numero),
            ("\"a\tb\"", Motivo::Controle),
            ("{\"a\":1,}", Motivo::EsperavaChaveOuFimDoObjeto),
        ];
        for (json, esperado) in casos {
            let e = validar(json.as_bytes()).expect_err(json);
            assert_eq!(e.motivo, esperado, "{json}");
        }
    }

    #[test]
    fn irmaos_contam_os_filhos_do_pai() {
        let mut a = arvore("{\"a\":[1,2,3],\"b\":0}");
        a.alternar(0);
        a.alternar(1);
        let l: Vec<_> = a.janela(0, 10).iter().map(|l| (l.pos, l.irmaos)).collect();
        assert_eq!(l, [(0, 0), (0, 2), (0, 3), (1, 3), (2, 3), (1, 2)]);
    }
}
