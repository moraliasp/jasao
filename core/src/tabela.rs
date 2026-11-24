//! Visão Tabela: cada registro que é objeto vira uma linha e cada campo do 1º nível, uma coluna.
//!
//! - Registros: as linhas do JSONL ou os elementos de um JSON único que é lista.
//! - Colunas: os campos do topo da estrutura inferida (`estrutura::inferir`), na ordem em que
//!   aparecem pela primeira vez, até `MAX_COLUNAS` (o resto é contado em `omitidas`).
//! - A interface pede janelas de linhas; o arquivo nunca vai inteiro.
//! - Ordenar extrai a chave de todos os registros em paralelo (com progresso e cancelamento) e
//!   guarda só a permutação. Ausente e inválido vão sempre para o fim, na ordem do arquivo.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use rayon::prelude::*;
use serde::Serialize;

use crate::arvore::{self, ESPACO, ErroLinha, MAX_PREVIA, Tipo as TipoValor};
use crate::estrutura::{self, Entrada, Inferencia, Tipo};
use crate::linhas::{ArquivoLinhas, BOM};

/// Colunas mostradas no máximo; as demais ficam contadas em `Tabela::omitidas`.
pub const MAX_COLUNAS: usize = 200;
/// Teto de registros para ordenar e para listar os elementos de um JSON único. Ordenar custa
/// ~30 B por registro (chave de 24 B, permutação de 4 B e o buffer do `par_sort_by`): 50 M dão
/// ~1,5 GB no pico. Listar os elementos custa 16 B por elemento (800 MB no teto).
pub const MAX_REGISTROS: usize = 50_000_000;
const _: () = assert!(MAX_REGISTROS <= u32::MAX as usize); // a permutação é `u32`
/// Registros por tarefa na extração das chaves de ordenação (e por aviso de progresso).
const BLOCO: usize = 1 << 16;

/// Por que o arquivo não vira tabela. A interface traduz o código.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "motivo")]
pub enum Indisponivel {
    /// Nenhum registro válido.
    Vazio,
    /// JSON único que não é lista (um objeto só ou um escalar).
    NaoLista,
    /// `n` registros válidos não são objetos (lista mista, JSONL de escalares).
    NaoObjetos {
        n: u64,
    },
    /// Os objetos são mapas: as chaves variam de registro para registro.
    Mapa,
    /// Só objetos vazios.
    SemCampos,
    /// JSON único com erro de sintaxe: os elementos não se separam.
    JsonInvalido,
    /// JSON único com mais elementos que `max` (`MAX_REGISTROS`).
    Grande {
        max: u64,
    },
    Cancelado,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Coluna {
    /// Chave como está no arquivo (escapes intactos).
    pub nome: String,
    /// `.chave` ou `["chave"]`, como na Estrutura.
    pub caminho: String,
    /// Em quantos registros o campo aparece (de `Tabela::objetos`).
    pub presenca: u64,
    /// Tipos vistos no campo, na ordem de `estrutura::Tipo::TODOS`.
    pub tipos: Vec<&'static str>,
    /// Chave decodificada, para casar com o parser.
    #[serde(skip)]
    chave: String,
}

/// Valor de uma célula. Célula ausente é `None` na linha (diferente de `null`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Celula {
    pub tipo: TipoValor,
    /// Texto cru do escalar, cortado em `MAX_PREVIA` bytes (com "…"); vazio em objeto e lista.
    pub texto: String,
    /// Campos ou itens (objeto e lista).
    pub filhos: usize,
    /// Byte do valor na linha `Linha::linha` do arquivo, para revelar na Árvore.
    pub byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Linha {
    /// Posição do registro no arquivo (base 0), antes de ordenar.
    pub registro: usize,
    /// Linha do arquivo onde o registro começa.
    pub linha: usize,
    /// Byte, nessa linha, onde o registro começa.
    pub byte: usize,
    /// Uma por coluna pedida; vazio no registro inválido.
    pub celulas: Vec<Option<Celula>>,
    /// Registro que não é JSON válido: byte (no registro) e motivo.
    pub erro: Option<ErroLinha>,
    /// Texto cru do registro inválido, cortado.
    pub previa: Option<String>,
}

/// Arquivo visto como tabela.
pub struct Tabela {
    arquivo: Arc<ArquivoLinhas>,
    /// JSON único: bytes de cada elemento da lista. JSONL: `None` (registro = linha).
    elementos: Option<Vec<Range<usize>>>,
    pub colunas: Vec<Coluna>,
    /// Campos do topo além de `MAX_COLUNAS`.
    pub omitidas: usize,
    /// Registros válidos (todos objetos): base de `Coluna::presenca`.
    pub objetos: u64,
}

/// Permutação dos registros por uma coluna.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ordem {
    pub coluna: usize,
    /// Registros com o campo; os seguintes (ausente ou inválido) ficam na ordem do arquivo.
    pub presentes: usize,
    perm: Vec<u32>,
}

impl Ordem {
    /// Registro na posição `i` da tabela ordenada. Decrescente só inverte os presentes: troca
    /// de direção é instantânea (e empates saem na ordem inversa do arquivo).
    pub fn registro(&self, i: usize, desc: bool) -> Option<usize> {
        let p = if desc && i < self.presentes {
            self.presentes - 1 - i
        } else {
            i
        };
        self.perm.get(p).map(|&r| r as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FalhaOrdem {
    Cancelada,
    /// Mais registros que `MAX_REGISTROS`.
    Grande,
    Coluna,
}

impl Tabela {
    /// Monta a tabela a partir da estrutura já inferida. JSON único lista os elementos numa
    /// passada (cancelável).
    pub fn montar(
        arquivo: Arc<ArquivoLinhas>,
        inf: &Inferencia,
        cancelar: &AtomicBool,
    ) -> Result<Self, Indisponivel> {
        Self::montar_ate(arquivo, inf, cancelar, MAX_REGISTROS)
    }

    fn montar_ate(
        arquivo: Arc<ArquivoLinhas>,
        inf: &Inferencia,
        cancelar: &AtomicBool,
        teto: usize,
    ) -> Result<Self, Indisponivel> {
        if inf.cancelado {
            return Err(Indisponivel::Cancelado);
        }
        let raiz = &inf.resumo.raiz;
        let dados = arquivo.bytes();
        let topo = dados
            .iter()
            .skip(if dados.starts_with(BOM) { BOM.len() } else { 0 })
            .find(|b| !b" \t\r\n".contains(b));
        if inf.entrada == Entrada::JsonUnico && topo != Some(&b'[') {
            return Err(Indisponivel::NaoLista);
        }
        if inf.resumo.registros() == 0 {
            return Err(Indisponivel::Vazio);
        }
        let n = inf.resumo.registros() - raiz.tipo(Tipo::Objeto).n;
        if n > 0 {
            return Err(Indisponivel::NaoObjetos { n });
        }
        let campos = raiz.campos();
        if campos.is_empty() {
            return Err(if raiz.mapa.is_some() {
                Indisponivel::Mapa
            } else {
                Indisponivel::SemCampos
            });
        }
        let elementos = match inf.entrada {
            Entrada::Jsonl => None,
            Entrada::JsonUnico if inf.resumo.invalidas.n > 0 => {
                return Err(Indisponivel::JsonInvalido);
            }
            Entrada::JsonUnico => Some(elementos(dados, cancelar, teto)?),
        };
        let colunas = campos
            .iter()
            .take(MAX_COLUNAS)
            .map(|(k, f)| Coluna {
                nome: String::from_utf8_lossy(k).into_owned(),
                caminho: estrutura::segmento(k),
                presenca: f.presenca,
                tipos: Tipo::TODOS
                    .iter()
                    .filter(|&&t| f.tipo(t).n > 0)
                    .map(|t| t.nome())
                    .collect(),
                chave: decodificar(k),
            })
            .collect();
        Ok(Self {
            arquivo,
            elementos,
            colunas,
            omitidas: campos.len().saturating_sub(MAX_COLUNAS),
            objetos: raiz.presenca,
        })
    }

    pub fn registros(&self) -> usize {
        self.elementos
            .as_ref()
            .map_or(self.arquivo.linhas(), Vec::len)
    }

    /// Bytes do registro `r`, linha do arquivo onde começa e byte nessa linha.
    pub fn registro(&self, r: usize) -> Option<(&[u8], usize, usize)> {
        let dados = self.arquivo.bytes();
        match &self.elementos {
            None => Some((self.arquivo.janela(r, 1).into_iter().next()?, r, 0)),
            Some(e) => {
                let faixa = e.get(r)?.clone();
                let (linha, ini) = self.arquivo.linha_do_byte(faixa.start)?;
                Some((&dados[faixa.clone()], linha, faixa.start - ini))
            }
        }
    }

    /// Até `quantidade` linhas a partir da posição `inicio` (na `ordem`, se houver), com as
    /// células das `colunas` pedidas (índices em `self.colunas`; fora dele, célula ausente).
    pub fn janela(
        &self,
        ordem: Option<(&Ordem, bool)>,
        filtro: Option<&[u32]>,
        inicio: usize,
        quantidade: usize,
        colunas: &[usize],
    ) -> Vec<Linha> {
        let alvo: HashMap<&str, usize> = colunas
            .iter()
            .enumerate()
            .rev() // coluna pedida duas vezes: vale a primeira posição
            .filter_map(|(pos, &c)| Some((self.colunas.get(c)?.chave.as_str(), pos)))
            .collect();
        let n = filtro.map_or(self.registros(), <[u32]>::len);
        let fim = inicio.saturating_add(quantidade).min(n);
        // A ordem, com filtro, já é só dos filtrados (`ordenar` recebe o mesmo filtro).
        (inicio..fim)
            .filter_map(|i| match (ordem, filtro) {
                (Some((o, desc)), _) => o.registro(i, desc),
                (None, Some(f)) => f.get(i).map(|&r| r as usize),
                (None, None) => Some(i),
            })
            .filter_map(|r| {
                let (bytes, linha, byte) = self.registro(r)?;
                Some(self.linha(r, bytes, linha, byte, &alvo, colunas.len()))
            })
            .collect()
    }

    fn linha(
        &self,
        registro: usize,
        bytes: &[u8],
        linha: usize,
        byte: usize,
        alvo: &HashMap<&str, usize>,
        n: usize,
    ) -> Linha {
        let mut l = Linha {
            registro,
            linha,
            byte,
            celulas: Vec::new(),
            erro: None,
            previa: None,
        };
        let texto = match arvore::validar(bytes) {
            Ok(t) => t,
            Err(e) => {
                l.erro = arvore::validar(self.arquivo.cru_de(bytes))
                    .err()
                    .or(Some(e));
                l.previa = Some(
                    String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PREVIA)]).into_owned(),
                );
                return l;
            }
        };
        l.celulas = vec![None; n];
        let raw = texto.trim_matches(ESPACO);
        // Válido mas não objeto não acontece (a tabela só existe com todos objetos); se o
        // parser da estrutura e o da validação divergirem, a linha sai com tudo ausente.
        if arvore::tipo_de(raw) != TipoValor::Objeto {
            return l;
        }
        let mut faltam = alvo.len();
        for par in sonic_rs::to_object_iter(raw) {
            let Ok((k, v)) = par else { break };
            let Some(&pos) = alvo.get(k.as_ref()) else {
                continue;
            };
            if l.celulas[pos].is_some() {
                continue; // chave repetida: vale a primeira, como na Estrutura
            }
            let raw_v = v.as_raw_str();
            let tipo = arvore::tipo_de(raw_v);
            let composto = matches!(tipo, TipoValor::Objeto | TipoValor::Lista);
            l.celulas[pos] = Some(Celula {
                tipo,
                texto: if composto {
                    String::new()
                } else {
                    arvore::previa(raw_v)
                },
                filhos: if composto { arvore::contar(raw_v) } else { 0 },
                byte: byte + (raw_v.as_ptr() as usize - bytes.as_ptr() as usize),
            });
            faltam -= 1;
            if faltam == 0 {
                break;
            }
        }
        l
    }

    /// Ordena todos os registros pela coluna (crescente). `ao_progresso` recebe quantos
    /// registros já foram lidos e roda nas threads de trabalho (pode chegar fora de ordem).
    /// Acima de `MAX_REGISTROS`, recusa (`FalhaOrdem::Grande`).
    pub fn ordenar(
        &self,
        coluna: usize,
        filtro: Option<&[u32]>,
        cancelar: &AtomicBool,
        ao_progresso: &(dyn Fn(u64) + Sync),
    ) -> Result<Ordem, FalhaOrdem> {
        self.ordenar_ate(coluna, filtro, cancelar, ao_progresso, MAX_REGISTROS)
    }

    fn ordenar_ate(
        &self,
        coluna: usize,
        filtro: Option<&[u32]>,
        cancelar: &AtomicBool,
        ao_progresso: &(dyn Fn(u64) + Sync),
        teto: usize,
    ) -> Result<Ordem, FalhaOrdem> {
        let chave = self
            .colunas
            .get(coluna)
            .ok_or(FalhaOrdem::Coluna)?
            .chave
            .as_str();
        // Com filtro, ordena só os filtrados: `k` é a posição no filtro, `alvo(k)` o registro.
        let alvo = |k: usize| filtro.map_or(k, |f| f[k] as usize);
        let n = filtro.map_or(self.registros(), <[u32]>::len);
        if n > teto {
            return Err(FalhaOrdem::Grande);
        }
        let dados = self.arquivo.bytes();
        let base = dados.as_ptr() as usize;
        let lidos = AtomicU64::new(0);
        let mut chaves = vec![Chave::AUSENTE; n];
        chaves
            .par_chunks_mut(BLOCO)
            .enumerate()
            .for_each(|(b, bloco)| {
                if cancelar.load(Relaxed) {
                    return;
                }
                for (j, c) in bloco.iter_mut().enumerate() {
                    if let Some((bytes, _, _)) = self.registro(alvo(b * BLOCO + j)) {
                        *c = Chave::de(bytes, chave, base);
                    }
                }
                ao_progresso(lidos.fetch_add(bloco.len() as u64, Relaxed) + bloco.len() as u64);
            });
        if cancelar.load(Relaxed) {
            return Err(FalhaOrdem::Cancelada);
        }
        let mut perm: Vec<u32> = (0..n as u32)
            .filter(|&r| chaves[r as usize].classe != Classe::Ausente)
            .collect();
        let presentes = perm.len();
        // Estável: empate fica na ordem do arquivo.
        perm.par_sort_by(|&a, &b| chaves[a as usize].comparar(&chaves[b as usize], dados));
        perm.extend((0..n as u32).filter(|&r| chaves[r as usize].classe == Classe::Ausente));
        if filtro.is_some() {
            perm.iter_mut().for_each(|k| *k = alvo(*k as usize) as u32);
        }
        // O sort não para no meio: cancelado durante ele, o resultado é descartado aqui.
        if cancelar.load(Relaxed) {
            return Err(FalhaOrdem::Cancelada);
        }
        Ok(Ordem {
            coluna,
            presentes,
            perm,
        })
    }
}

/// Elementos do JSON único que é lista, em bytes do arquivo.
fn elementos(
    dados: &[u8],
    cancelar: &AtomicBool,
    teto: usize,
) -> Result<Vec<Range<usize>>, Indisponivel> {
    let base = dados.as_ptr() as usize;
    let ini = if dados.starts_with(BOM) { BOM.len() } else { 0 };
    let mut v = Vec::new();
    for (i, e) in sonic_rs::to_array_iter(&dados[ini..]).enumerate() {
        if i % 4096 == 0 && cancelar.load(Relaxed) {
            return Err(Indisponivel::Cancelado);
        }
        if i == teto {
            return Err(Indisponivel::Grande { max: teto as u64 });
        }
        let e = e.map_err(|_| Indisponivel::JsonInvalido)?;
        let raw = e.as_raw_str();
        let p = raw.as_ptr() as usize - base;
        v.push(p..p + raw.len());
    }
    Ok(v)
}

/// Chave crua do arquivo → texto decodificado (o parser entrega a chave já sem escapes).
fn decodificar(k: &[u8]) -> String {
    if !k.contains(&b'\\') {
        return String::from_utf8_lossy(k).into_owned();
    }
    sonic_rs::from_slice::<String>(&[b"\"", k, b"\""].concat())
        .unwrap_or_else(|_| String::from_utf8_lossy(k).into_owned())
}

/// Ordem das classes no crescente: número, texto, booleano, objeto, lista, null. Ausente e
/// inválido não entram na comparação (vão para o fim).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Classe {
    Numero,
    Texto,
    Booleano,
    Objeto,
    Lista,
    Nulo,
    Ausente,
}

/// Chave de ordenação de um registro: 24 bytes, sem cópia do texto (aponta para o arquivo).
#[derive(Debug, Clone, Copy)]
struct Chave {
    classe: Classe,
    /// Número (ou 0/1 no booleano).
    num: f64,
    /// Bytes crus no arquivo: conteúdo do texto (sem aspas) ou o número como escrito.
    ini: usize,
    len: u32,
}

impl Chave {
    const AUSENTE: Chave = Chave {
        classe: Classe::Ausente,
        num: 0.0,
        ini: 0,
        len: 0,
    };

    fn de(bytes: &[u8], chave: &str, base: usize) -> Chave {
        let Ok(texto) = arvore::validar(bytes) else {
            return Chave::AUSENTE;
        };
        let raw = texto.trim_matches(ESPACO);
        if arvore::tipo_de(raw) != TipoValor::Objeto {
            return Chave::AUSENTE;
        }
        let Some((_, lv)) = sonic_rs::to_object_iter(raw)
            .map_while(Result::ok)
            .find(|(k, _)| k == chave)
        else {
            return Chave::AUSENTE;
        };
        let v = lv.as_raw_str();
        let mut c = Chave {
            classe: Classe::Ausente,
            num: 0.0,
            ini: v.as_ptr() as usize - base,
            // shortcut: texto acima de 4 GB compara só os primeiros 4 GB.
            len: u32::try_from(v.len()).unwrap_or(u32::MAX),
        };
        c.classe = match arvore::tipo_de(v) {
            TipoValor::Numero => {
                // Atalho: o f64 decide quando difere (o arredondamento é monótono); no empate,
                // `decimal` compara o valor exato (ver `comparar`).
                c.num = v.parse().unwrap_or(f64::NAN);
                Classe::Numero
            }
            TipoValor::Texto => {
                c.ini += 1;
                c.len = c.len.saturating_sub(2);
                Classe::Texto
            }
            TipoValor::Booleano => {
                c.num = f64::from(u8::from(v.starts_with('t')));
                Classe::Booleano
            }
            TipoValor::Objeto => Classe::Objeto,
            TipoValor::Lista => Classe::Lista,
            TipoValor::Nulo | TipoValor::Erro => Classe::Nulo,
        };
        c
    }

    fn cru<'a>(&self, dados: &'a [u8]) -> &'a [u8] {
        &dados[self.ini..self.ini + self.len as usize]
    }

    fn comparar(&self, o: &Chave, dados: &[u8]) -> Ordering {
        self.classe.cmp(&o.classe).then_with(|| match self.classe {
            // f64 diferente ⇒ valor diferente na mesma direção; f64 igual (acima de 2^53, -0,
            // fora do alcance) ⇒ decimal exato e, no empate de valor, o texto cru.
            Classe::Numero => match self.num.partial_cmp(&o.num) {
                Some(Ordering::Equal) | None => {
                    let (a, b) = (self.cru(dados), o.cru(dados));
                    decimal(a).cmp(&decimal(b)).then_with(|| a.cmp(b))
                }
                Some(m) => m,
            },
            Classe::Booleano => self.num.total_cmp(&o.num),
            // Ordem de code point (= ordem dos bytes UTF-8), sem regra de idioma.
            Classe::Texto => {
                let (a, b) = (self.cru(dados), o.cru(dados));
                if a.contains(&b'\\') || b.contains(&b'\\') {
                    decodificar(a).cmp(&decodificar(b))
                } else {
                    a.cmp(b)
                }
            }
            _ => Ordering::Equal,
        })
    }
}

/// Número JSON como decimal exato, sem alocar: `0,d₁d₂… × 10^pos`.
#[derive(Clone, Copy)]
struct Decimal<'a> {
    neg: bool,
    inteiro: &'a [u8],
    fracao: &'a [u8],
    /// Zeros à esquerda (em `inteiro` + `fracao`) antes do 1º dígito significativo.
    zeros: usize,
    pos: i64,
}

fn decimal(v: &[u8]) -> Decimal<'_> {
    let (neg, v) = match v.split_first() {
        Some((b'-', r)) => (true, r),
        _ => (false, v),
    };
    let (m, e) = match v.iter().position(|&b| b == b'e' || b == b'E') {
        Some(i) => (&v[..i], &v[i + 1..]),
        None => (v, &b""[..]),
    };
    let exp = std::str::from_utf8(e.strip_prefix(b"+").unwrap_or(e))
        .ok()
        .and_then(|e| e.parse::<i64>().ok())
        .unwrap_or(if e.starts_with(b"-") {
            i64::MIN / 2
        } else if e.is_empty() {
            0
        } else {
            i64::MAX / 2
        });
    let (inteiro, fracao) = match m.iter().position(|&b| b == b'.') {
        Some(i) => (&m[..i], &m[i + 1..]),
        None => (m, &b""[..]),
    };
    let zeros = inteiro
        .iter()
        .chain(fracao)
        .take_while(|&&b| b == b'0')
        .count();
    Decimal {
        neg,
        inteiro,
        fracao,
        zeros,
        pos: (inteiro.len() as i64 - zeros as i64).saturating_add(exp),
    }
}

impl Decimal<'_> {
    fn digitos(&self) -> impl Iterator<Item = u8> + '_ {
        self.inteiro
            .iter()
            .chain(self.fracao)
            .skip(self.zeros)
            .copied()
    }
    fn zero(&self) -> bool {
        self.zeros == self.inteiro.len() + self.fracao.len()
    }
}

impl Ord for Decimal<'_> {
    fn cmp(&self, o: &Self) -> Ordering {
        let sinal = |d: &Decimal| {
            if d.zero() {
                0
            } else if d.neg {
                -1
            } else {
                1
            }
        };
        let s = sinal(self);
        s.cmp(&sinal(o)).then_with(|| {
            if s == 0 {
                return Ordering::Equal;
            }
            let (mut a, mut b) = (self.digitos(), o.digitos());
            // Mesma posição do 1º dígito: compara dígito a dígito (o que falta vale 0).
            let m = self.pos.cmp(&o.pos).then_with(|| {
                loop {
                    match (a.next(), b.next()) {
                        (None, None) => break Ordering::Equal,
                        (x, y) => match x.unwrap_or(b'0').cmp(&y.unwrap_or(b'0')) {
                            Ordering::Equal => {}
                            m => break m,
                        },
                    }
                }
            });
            if s < 0 { m.reverse() } else { m }
        })
    }
}
impl PartialOrd for Decimal<'_> {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl PartialEq for Decimal<'_> {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Decimal<'_> {}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::estrutura::inferir;

    fn tabela(dados: &[u8]) -> Result<Tabela, Indisponivel> {
        tabela_ate(dados, MAX_REGISTROS)
    }

    fn tabela_ate(dados: &[u8], teto: usize) -> Result<Tabela, Indisponivel> {
        let dados: &'static [u8] = Vec::leak(dados.to_vec());
        let arq = Arc::new(ArquivoLinhas::estatico(dados));
        let nao = AtomicBool::new(false);
        let inf = inferir(arq.bytes(), &nao, |_| {});
        Tabela::montar_ate(arq, &inf, &nao, teto)
    }

    fn todas(t: &Tabela) -> Vec<usize> {
        (0..t.colunas.len()).collect()
    }

    /// Texto de cada célula: `-` ausente, `{n}`/`[n]` composto, senão o texto cru.
    fn celulas(l: &Linha) -> Vec<String> {
        l.celulas
            .iter()
            .map(|c| match c {
                None => "-".into(),
                Some(c) if c.tipo == TipoValor::Objeto => format!("{{{}}}", c.filhos),
                Some(c) if c.tipo == TipoValor::Lista => format!("[{}]", c.filhos),
                Some(c) => c.texto.clone(),
            })
            .collect()
    }

    #[test]
    fn colunas_por_caminho_e_celulas_fieis() {
        let t = tabela(
            br#"{"id":1,"nome":"a\u00e9","x y":null}
{"id":12345678901234567890123,"extra":{"a":1,"b":2,"c":3},"tags":[1,2,3,4,5]}
{"id":3,"id":4,"\u0063":true}
"#,
        )
        .unwrap();
        let nomes: Vec<_> = t.colunas.iter().map(|c| c.caminho.as_str()).collect();
        assert_eq!(
            nomes,
            [
                ".id",
                ".nome",
                r#"["x y"]"#,
                ".extra",
                ".tags",
                r#"["\u0063"]"#
            ]
        );
        assert_eq!(t.colunas[0].presenca, 3);
        assert_eq!(t.colunas[0].tipos, ["inteiro"]);
        assert_eq!(t.objetos, 3);
        let j = t.janela(None, None, 0, 10, &todas(&t));
        assert_eq!(celulas(&j[0]), ["1", r#""a\u00e9""#, "null", "-", "-", "-"]);
        // Número grande e texto com escape saem como estão; nulo ≠ ausente.
        assert_eq!(j[0].celulas[2].as_ref().unwrap().tipo, TipoValor::Nulo);
        assert_eq!(
            celulas(&j[1]),
            ["12345678901234567890123", "-", "-", "{3}", "[5]", "-"]
        );
        // Chave repetida: vale a primeira. Chave com escape casa com a decodificada.
        assert_eq!(celulas(&j[2]), ["3", "-", "-", "-", "-", "true"]);
        // Byte da célula aponta para o valor na linha.
        let c = j[1].celulas[3].as_ref().unwrap();
        assert_eq!(
            &br#"{"a":1"#[..],
            &t.arquivo.janela(1, 1)[0][c.byte..c.byte + 6]
        );
        // Projeção: só as colunas pedidas, na ordem pedida.
        let j = t.janela(None, None, 1, 1, &[4, 0]);
        assert_eq!(celulas(&j[0]), ["[5]", "12345678901234567890123"]);
    }

    #[test]
    fn janela_e_linha_invalida() {
        let t = tabela(b"{\"a\":1}\n{\"a\":tru}\n{\"a\":3}\n").unwrap();
        assert_eq!(t.registros(), 3);
        let j = t.janela(None, None, 1, 5, &[0]);
        assert_eq!(j.len(), 2);
        assert_eq!(j[0].registro, 1);
        assert!(j[0].celulas.is_empty());
        assert_eq!(j[0].erro.unwrap().motivo, arvore::Motivo::Literal);
        assert_eq!(j[0].previa.as_deref(), Some("{\"a\":tru}"));
        assert_eq!(celulas(&j[1]), ["3"]);
        assert!(t.janela(None, None, 9, 5, &[0]).is_empty());
    }

    #[test]
    fn json_unico_lista_de_objetos() {
        let t = tabela(br#"[{"a":1},{"a":2,"b":"x"}]"#).unwrap();
        assert_eq!(t.registros(), 2);
        let j = t.janela(None, None, 0, 2, &todas(&t));
        assert_eq!((j[1].linha, j[1].byte), (0, 9));
        assert_eq!(celulas(&j[1]), ["2", "\"x\""]);
        assert_eq!(j[1].celulas[1].as_ref().unwrap().byte, 20);
        // Formatado em várias linhas: a linha de cada registro é onde ele começa.
        let t = tabela(b"[\n  {\"a\": 1},\n  {\"a\": 2}\n]\n").unwrap();
        let j = t.janela(None, None, 0, 2, &[0]);
        assert_eq!((j[1].linha, j[1].byte), (2, 2));
        assert_eq!(celulas(&j[1]), ["2"]);
    }

    #[test]
    fn indisponivel_com_motivo() {
        let casos: &[(&[u8], Indisponivel)] = &[
            (b"", Indisponivel::Vazio),
            (b"42", Indisponivel::NaoObjetos { n: 1 }),
            (b"{\"a\":1}", Indisponivel::NaoLista),
            (b"[{\"a\":1},2,\"x\"]", Indisponivel::NaoObjetos { n: 2 }),
            (b"{\"a\":1}\n[1]\n", Indisponivel::NaoObjetos { n: 1 }),
            (b"{}\n{}\n", Indisponivel::SemCampos),
            (b"[{\"a\":1},", Indisponivel::JsonInvalido),
        ];
        for (dados, motivo) in casos {
            assert_eq!(
                tabela(dados).err(),
                Some(*motivo),
                "{:?}",
                String::from_utf8_lossy(dados)
            );
        }
        // Chaves que variam de registro para registro (ids como chave): mapa.
        let mapa: String = (0..40).map(|i| format!("{{\"k{i}\":1}}\n")).collect();
        assert_eq!(tabela(mapa.as_bytes()).err(), Some(Indisponivel::Mapa));
    }

    #[test]
    fn teto_de_colunas() {
        let campos: Vec<String> = (0..MAX_COLUNAS + 7)
            .map(|i| format!("\"c{i}\":{i}"))
            .collect();
        let t = tabela(format!("{{{0}}}\n{{{0}}}\n", campos.join(",")).as_bytes()).unwrap();
        assert_eq!(t.colunas.len(), MAX_COLUNAS);
        assert_eq!(t.omitidas, 7);
    }

    fn ordem(t: &Tabela, coluna: usize, desc: bool) -> Vec<String> {
        let nao = AtomicBool::new(false);
        let o = t.ordenar(coluna, None, &nao, &|_| {}).unwrap();
        t.janela(Some((&o, desc)), None, 0, 100, &[coluna])
            .iter()
            .map(|l| match &l.erro {
                Some(_) => format!("erro{}", l.registro),
                None => celulas(l).remove(0),
            })
            .collect()
    }

    #[test]
    fn ordena_tipos_mistos_ausente_e_null() {
        let linhas = [
            r#"{"v":"b"}"#,
            r#"{"v":10}"#,
            r#"{"w":1}"#,
            r#"{"v":null}"#,
            r#"{"v":true}"#,
            r#"{"v":2}"#,
            r#"{"v":tru}"#,
            r#"{"v":"\u0041"}"#,
            r#"{"v":[1]}"#,
            r#"{"v":false}"#,
            r#"{"v":-1.5}"#,
            r#"{"v":{"a":1}}"#,
            r#"{"v":"a"}"#,
            r#"{"v":1.0}"#,
            r#"{"v":1}"#,
        ];
        let t = tabela(linhas.join("\n").as_bytes()).unwrap();
        let v = t.colunas.iter().position(|c| c.nome == "v").unwrap();
        assert_eq!(
            ordem(&t, v, false),
            [
                "-1.5",
                "1",
                "1.0",
                "2",
                "10", // número; empate pelo texto cru
                r#""\u0041""#,
                r#""a""#,
                r#""b""#, // texto por code point (A < a < b)
                "false",
                "true",
                "{1}",
                "[1]",
                "null", //
                "-",
                "erro6", // ausente e inválido no fim, na ordem do arquivo
            ]
        );
        assert_eq!(
            ordem(&t, v, true),
            [
                "null",
                "[1]",
                "{1}",
                "true",
                "false",
                r#""b""#,
                r#""a""#,
                r#""\u0041""#,
                "10",
                "2",
                "1.0",
                "1",
                "-1.5",
                "-",
                "erro6",
            ]
        );
    }

    #[test]
    fn ordena_em_blocos_e_cancela() {
        let n = BLOCO * 2 + 10;
        let dados: String = (0..n).map(|i| format!("{{\"n\":{}}}\n", n - i)).collect();
        let t = tabela(dados.as_bytes()).unwrap();
        let visto = AtomicU64::new(0);
        let nao = AtomicBool::new(false);
        let o = t
            .ordenar(0, None, &nao, &|v| {
                visto.fetch_max(v, Relaxed);
            })
            .unwrap();
        assert_eq!(visto.load(Relaxed), n as u64);
        assert_eq!(o.presentes, n);
        assert_eq!(o.registro(0, false), Some(n - 1));
        assert_eq!(o.registro(n - 1, false), Some(0));
        assert_eq!(o.registro(0, true), Some(0));
        let sim = AtomicBool::new(true);
        assert_eq!(
            t.ordenar(0, None, &sim, &|_| {}),
            Err(FalhaOrdem::Cancelada)
        );
        assert_eq!(t.ordenar(99, None, &nao, &|_| {}), Err(FalhaOrdem::Coluna));
    }

    #[test]
    fn filtro_na_janela_e_na_ordem() {
        let t = tabela(b"{\"n\":5}\n{\"n\":1}\n{\"n\":4}\n{\"m\":0}\n{\"n\":2}\n").unwrap();
        let nao = AtomicBool::new(false);
        let f: &[u32] = &[0, 2, 3, 4];
        let regs = |o: Option<(&Ordem, bool)>| -> Vec<usize> {
            t.janela(o, Some(f), 0, 100, &[0])
                .iter()
                .map(|l| l.registro)
                .collect()
        };
        // Sem ordem: só os filtrados, na ordem do arquivo; a janela conta posições do filtro.
        assert_eq!(regs(None), [0, 2, 3, 4]);
        let j: Vec<usize> = t
            .janela(None, Some(f), 1, 2, &[0])
            .iter()
            .map(|l| l.registro)
            .collect();
        assert_eq!(j, [2, 3]);
        // Ordenar com filtro = ordenar tudo e tirar quem não passa (ausente no fim).
        let o = t.ordenar(0, Some(f), &nao, &|_| {}).unwrap();
        assert_eq!(regs(Some((&o, false))), [4, 2, 0, 3]);
        assert_eq!(regs(Some((&o, true))), [0, 2, 4, 3]);
        let todo = t.ordenar(0, None, &nao, &|_| {}).unwrap();
        let esperado: Vec<usize> = (0..5)
            .filter_map(|i| todo.registro(i, false))
            .filter(|r| f.contains(&(*r as u32)))
            .collect();
        assert_eq!(regs(Some((&o, false))), esperado);
        // O teto vale para o tamanho do filtro, não do arquivo.
        assert_eq!(
            t.ordenar_ate(0, Some(f), &nao, &|_| {}, 3),
            Err(FalhaOrdem::Grande)
        );
        assert!(t.ordenar_ate(0, Some(f), &nao, &|_| {}, 4).is_ok());
    }

    #[test]
    fn teto_de_registros() {
        // Ordenar: acima do teto recusa antes de alocar; no teto, ordena.
        let t = tabela(b"{\"n\":3}\n{\"n\":1}\n{\"n\":2}\n").unwrap();
        let nao = AtomicBool::new(false);
        assert_eq!(
            t.ordenar_ate(0, None, &nao, &|_| {}, 2),
            Err(FalhaOrdem::Grande)
        );
        assert!(t.ordenar_ate(0, None, &nao, &|_| {}, 3).is_ok());
        // JSON único: a lista de elementos para no teto.
        let lista = b"[{\"a\":1},{\"a\":2},{\"a\":3}]";
        assert_eq!(
            tabela_ate(lista, 2).err(),
            Some(Indisponivel::Grande { max: 2 })
        );
        assert_eq!(tabela_ate(lista, 3).unwrap().registros(), 3);
    }

    #[test]
    fn cancela_no_meio_da_ordenacao() {
        let n = BLOCO * 3;
        let dados: String = (0..n).map(|i| format!("{{\"n\":{i}}}\n")).collect();
        let t = tabela(dados.as_bytes()).unwrap();
        // Uma thread: os blocos rodam em sequência e o cancelar chega depois do 1º.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let cancelar = AtomicBool::new(false);
        let avisos = AtomicU64::new(0);
        let r = pool.install(|| {
            t.ordenar(0, None, &cancelar, &|_| {
                avisos.fetch_add(1, Relaxed);
                cancelar.store(true, Relaxed);
            })
        });
        assert_eq!(r, Err(FalhaOrdem::Cancelada));
        assert_eq!(avisos.load(Relaxed), 1, "blocos seguintes não leem nada");
    }

    #[test]
    fn numero_compara_pelo_valor_exato() {
        let linhas = [
            r#"{"v":100000000000000000001}"#,
            r#"{"v":1e20}"#,
            r#"{"v":9007199254740993}"#,
            r#"{"v":9007199254740992}"#,
            r#"{"v":-1e20}"#,
            r#"{"v":-100000000000000000001}"#,
            r#"{"v":1e400}"#,
            r#"{"v":2e400}"#,
            r#"{"v":0.5}"#,
            r#"{"v":5E-1}"#,
            r#"{"v":-0}"#,
            r#"{"v":0.0}"#,
        ];
        let t = tabela(linhas.join("\n").as_bytes()).unwrap();
        assert_eq!(
            ordem(&t, 0, false),
            [
                "-100000000000000000001",
                "-1e20",
                "-0",
                "0.0", // mesmo valor: pelo texto
                "0.5",
                "5E-1",
                "9007199254740992",
                "9007199254740993",
                "1e20",
                "100000000000000000001",
                "1e400",
                "2e400",
            ]
        );
    }
}
