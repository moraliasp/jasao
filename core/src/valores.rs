//! Diff de valores: o que foi adicionado, removido ou alterado, por caminho, entre dois registros
//! ou entre dois arquivos (registro N de um contra registro N do outro).
//!
//! Fiel ao byte: dois valores são iguais se o texto cru for igual (`1` ≠ `1.0`, `"é"` ≠
//! `"\u00e9"`); objeto é comparado por chave (chave repetida: vale a primeira, como na
//! Estrutura) e lista por posição. Arquivo inteiro em fluxo: cada lado é lido por uma thread que
//! entrega lotes por um canal limitado, então a memória não cresce com o arquivo; só os primeiros
//! `MAX_REGISTROS` registros diferentes ficam guardados, com até `MAX_POR_REGISTRO` diferenças
//! cada. As contagens são sempre do arquivo inteiro. Registro acima de `MAX_BYTES_REGISTRO`
//! não é comparado (a análise segura índices dele em memória). Critérios: `docs/decisoes.md`.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread;

use memchr::memchr_iter;
use serde::Serialize;

use crate::arvore::{Tipo, previa, tipo_de, validar};
use crate::estrutura::{Entrada, Progresso, juntar, registros_json_unico, segmento};
use crate::linhas::BOM;

/// Registros diferentes guardados (os primeiros do arquivo).
pub const MAX_REGISTROS: usize = 500;
/// Diferenças guardadas por registro (as primeiras na ordem do registro de antes).
pub const MAX_POR_REGISTRO: usize = 50;
/// Registro maior que isto (de algum lado) não é comparado: validar e andar custam ~2× o
/// registro em memória, por lado; 16 MiB deixa o par bem abaixo da meta de 300 MB.
/// shortcut: um JSON único com objeto no topo é um registro só; acima do teto, comparar em
/// fluxo sem materializar listas e campos.
pub const MAX_BYTES_REGISTRO: usize = 16 << 20;
/// Registros por lote no canal de cada lado; 4 lotes em voo por lado.
const LOTE: usize = 1024;
const EM_VOO: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Acao {
    Adicionado,
    Removido,
    Alterado,
}

impl Acao {
    /// Identificador estável (saída `--json` e interface).
    pub fn nome(self) -> &'static str {
        match self {
            Acao::Adicionado => "adicionado",
            Acao::Removido => "removido",
            Acao::Alterado => "alterado",
        }
    }
}

/// Uma diferença num caminho. `antes`/`depois`: prévia do texto cru (até `arvore::MAX_PREVIA`
/// bytes), `None` do lado em que o caminho não existe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diferenca {
    pub caminho: String,
    pub acao: Acao,
    pub antes: Option<String>,
    pub depois: Option<String>,
}

/// `alterado    .a.b  1 → 2`
impl fmt::Display for Diferenca {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (a, d) = (self.antes.as_deref(), self.depois.as_deref());
        write!(f, "{:<11} {}  ", self.acao.nome(), self.caminho)?;
        match (a, d) {
            (Some(a), Some(d)) => write!(f, "{a} → {d}"),
            (Some(x), None) | (None, Some(x)) => write!(f, "{x}"),
            (None, None) => Ok(()),
        }
    }
}

/// Resultado da comparação de dois registros.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Par {
    Iguais,
    /// `total` conta todas; `diferencas` guarda as primeiras `MAX_POR_REGISTRO`.
    Diferentes {
        diferencas: Vec<Diferenca>,
        total: u64,
    },
    /// Algum lado não é JSON válido (regra da Árvore): não comparado.
    Invalido,
    /// Bytes diferentes e algum lado acima de `MAX_BYTES_REGISTRO`: não comparado.
    Grande,
}

/// Compara dois registros (bytes crus de uma linha ou de um elemento). Bytes iguais (fora o
/// espaço nas pontas) são iguais sem ler o JSON, mesmo se inválidos ou grandes.
pub fn par(a: &[u8], b: &[u8]) -> Par {
    let (a, b) = (a.trim_ascii(), b.trim_ascii());
    if a == b {
        return Par::Iguais;
    }
    if a.len().max(b.len()) > MAX_BYTES_REGISTRO {
        return Par::Grande;
    }
    let (Ok(x), Ok(y)) = (validar(a), validar(b)) else {
        return Par::Invalido;
    };
    let mut c = Coleta {
        v: Vec::new(),
        total: 0,
    };
    // `validar` limita a profundidade: a recursão de `andar` tem fundo.
    c.andar(x.trim_ascii(), y.trim_ascii(), ".");
    if c.total == 0 {
        Par::Iguais // só espaço ou formatação diferente
    } else {
        Par::Diferentes {
            diferencas: c.v,
            total: c.total,
        }
    }
}

struct Coleta {
    v: Vec<Diferenca>,
    total: u64,
}

impl Coleta {
    fn anotar(&mut self, caminho: String, acao: Acao, a: Option<&str>, b: Option<&str>) {
        self.total += 1;
        if self.v.len() < MAX_POR_REGISTRO {
            self.v.push(Diferenca {
                caminho,
                acao,
                antes: a.map(previa),
                depois: b.map(previa),
            });
        }
    }

    fn andar(&mut self, a: &str, b: &str, caminho: &str) {
        if a == b {
            return;
        }
        match (tipo_de(a), tipo_de(b)) {
            (Tipo::Objeto, Tipo::Objeto) => {
                let (ca, cb) = (campos(a), campos(b));
                let mb: HashMap<&str, &str> = cb.iter().map(|(k, v)| (k.as_str(), *v)).collect();
                for (k, va) in &ca {
                    let c = juntar(caminho, &chave(k));
                    match mb.get(k.as_str()) {
                        Some(vb) => self.andar(va, vb, &c),
                        None => self.anotar(c, Acao::Removido, Some(va), None),
                    }
                }
                let ma: HashMap<&str, ()> = ca.iter().map(|(k, _)| (k.as_str(), ())).collect();
                for (k, vb) in &cb {
                    if !ma.contains_key(k.as_str()) {
                        self.anotar(juntar(caminho, &chave(k)), Acao::Adicionado, None, Some(vb));
                    }
                }
            }
            (Tipo::Lista, Tipo::Lista) => {
                let (la, lb) = (itens(a), itens(b));
                for i in 0..la.len().max(lb.len()) {
                    let c = juntar(caminho, &format!("[{i}]"));
                    match (la.get(i), lb.get(i)) {
                        (Some(x), Some(y)) => self.andar(x, y, &c),
                        (Some(x), None) => self.anotar(c, Acao::Removido, Some(x), None),
                        (None, Some(y)) => self.anotar(c, Acao::Adicionado, None, Some(y)),
                        (None, None) => {}
                    }
                }
            }
            _ => self.anotar(caminho.to_owned(), Acao::Alterado, Some(a), Some(b)),
        }
    }
}

/// O trecho de `todo` que o parser devolveu como `parte` (o sonic-rs empresta do texto lido).
fn dentro<'a>(todo: &'a str, parte: &str) -> &'a str {
    let p = parte.as_ptr() as usize - todo.as_ptr() as usize;
    &todo[p..p + parte.len()]
}

/// Campos do objeto já validado, na ordem do arquivo; chave repetida: vale a primeira.
fn campos(raw: &str) -> Vec<(String, &str)> {
    let mut v: Vec<(String, &str)> = Vec::new();
    let mut vistas = std::collections::HashSet::new();
    for (k, x) in sonic_rs::to_object_iter(raw).flatten() {
        if vistas.insert(k.to_string()) {
            v.push((k.to_string(), dentro(raw, x.as_raw_str())));
        }
    }
    v
}

fn itens(raw: &str) -> Vec<&str> {
    sonic_rs::to_array_iter(raw)
        .flatten()
        .map(|x| dentro(raw, x.as_raw_str()))
        .collect()
}

/// Segmento do caminho para a chave já decodificada pelo parser: reescapa (aspas, controle) e
/// usa a mesma regra de `.chave`/`["chave"]` da Estrutura.
fn chave(k: &str) -> String {
    let s = serde_json::to_string(k).unwrap_or_default();
    segmento(&s.as_bytes()[1..s.len() - 1])
}

// ---- Arquivo inteiro ----

/// Um registro diferente: o número dele (a partir de 0, na ordem do arquivo) e a linha em cada
/// lado (`None` do lado que acabou antes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistroDiferente {
    pub registro: u64,
    pub linha_antes: Option<u64>,
    pub linha_depois: Option<u64>,
    pub total: u64,
    pub diferencas: Vec<Diferenca>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiffArquivos {
    pub registros_antes: u64,
    pub registros_depois: u64,
    pub iguais: u64,
    /// Pares comparados com alguma diferença.
    pub diferentes: u64,
    /// Registros além do fim do outro arquivo.
    pub so_antes: u64,
    pub so_depois: u64,
    /// Pares com registro inválido em algum lado (não comparados).
    pub invalidos: u64,
    /// Pares com registro acima de `MAX_BYTES_REGISTRO` em algum lado (não comparados).
    pub grandes: u64,
    /// Diferenças somadas nos pares comparados.
    pub diferencas: u64,
    /// Os primeiros `MAX_REGISTROS` diferentes (só antes/só depois entram como `.` removido ou
    /// adicionado).
    pub guardados: Vec<RegistroDiferente>,
    pub cancelado: bool,
}

/// Compara o registro N de `a` com o registro N de `b` (registros como a inferência os lê:
/// `entrada` de cada um). Progresso a cada ~4 mil pares, em linhas lidas dos dois lados.
pub fn arquivos(
    a: &[u8],
    ea: Entrada,
    b: &[u8],
    eb: Entrada,
    cancelar: &AtomicBool,
    mut ao_progresso: impl FnMut(Progresso),
) -> DiffArquivos {
    let total = linhas(a) + linhas(b);
    let mut r = DiffArquivos::default();
    thread::scope(|s| {
        let (ta, ra) = sync_channel(EM_VOO);
        let (tb, rb) = sync_channel(EM_VOO);
        s.spawn(move || produzir(a, ea, cancelar, ta));
        s.spawn(move || produzir(b, eb, cancelar, tb));
        let (mut ia, mut ib) = (ra.iter().flatten(), rb.iter().flatten());
        let (mut la, mut lb) = (0, 0);
        for i in 0u64.. {
            if i % 4096 == 0 {
                if cancelar.load(Relaxed) {
                    r.cancelado = true;
                    break;
                }
                ao_progresso(Progresso {
                    varridas: la + lb,
                    total,
                });
            }
            let (x, y) = (ia.next(), ib.next());
            if let Some((l, _)) = x {
                la = l;
                r.registros_antes += 1;
            }
            if let Some((l, _)) = y {
                lb = l;
                r.registros_depois += 1;
            }
            let guardar = |r: &mut DiffArquivos, total, diferencas| {
                if r.guardados.len() < MAX_REGISTROS {
                    r.guardados.push(RegistroDiferente {
                        registro: i,
                        linha_antes: x.map(|p| p.0),
                        linha_depois: y.map(|p| p.0),
                        total,
                        diferencas,
                    });
                }
            };
            let inteiro = |acao, a: Option<&[u8]>, b: Option<&[u8]>| {
                let p = |v: &[u8]| previa(&String::from_utf8_lossy(v.trim_ascii()));
                vec![Diferenca {
                    caminho: ".".into(),
                    acao,
                    antes: a.map(p),
                    depois: b.map(p),
                }]
            };
            match (x, y) {
                (None, None) => break,
                (Some((_, x)), Some((_, y))) => match par(x, y) {
                    Par::Iguais => r.iguais += 1,
                    Par::Invalido => r.invalidos += 1,
                    Par::Grande => r.grandes += 1,
                    Par::Diferentes { diferencas, total } => {
                        r.diferentes += 1;
                        r.diferencas += total;
                        guardar(&mut r, total, diferencas);
                    }
                },
                (Some((_, x)), None) => {
                    r.so_antes += 1;
                    guardar(&mut r, 1, inteiro(Acao::Removido, Some(x), None));
                }
                (None, Some((_, y))) => {
                    r.so_depois += 1;
                    guardar(&mut r, 1, inteiro(Acao::Adicionado, None, Some(y)));
                }
            }
        }
        if !r.cancelado {
            ao_progresso(Progresso {
                varridas: total,
                total,
            });
        }
        // Soltar os receptores faz os produtores pararem no próximo envio (cancelado).
    });
    r
}

type Lote<'a> = Vec<(u64, &'a [u8])>;

/// Linhas do arquivo (a última sem `\n` conta), como em `estrutura::inferir`.
fn linhas(d: &[u8]) -> u64 {
    let d = d.strip_prefix(BOM).unwrap_or(d);
    (memchr_iter(b'\n', d).count() + usize::from(!d.is_empty() && !d.ends_with(b"\n"))) as u64
}

/// Chama `f(linha, bytes)` para cada registro de `dados`, em ordem (linha base 0 onde ele
/// começa), como a inferência os lê. Para quando `f` devolve `false`.
fn cada_registro<'a>(
    dados: &'a [u8],
    entrada: Entrada,
    cancelar: &AtomicBool,
    mut f: impl FnMut(u64, &'a [u8]) -> bool,
) {
    match entrada {
        Entrada::Jsonl => {
            let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
            let mut ini = base;
            let fins = memchr_iter(b'\n', &dados[base..]).map(|p| p + base);
            for (linha, fim) in fins.chain(std::iter::once(dados.len())).enumerate() {
                if ini > dados.len() {
                    break;
                }
                let l = &dados[ini..fim];
                ini = fim + 1;
                // Linha só de espaço não é registro (como na inferência).
                if !l.trim_ascii().is_empty() && !f(linha as u64, l) {
                    return;
                }
            }
        }
        Entrada::JsonUnico => {
            registros_json_unico(dados, cancelar, f);
        }
    }
}

/// Manda os registros em lotes. Para quando o receptor some ou ao cancelar.
fn produzir<'a>(
    dados: &'a [u8],
    entrada: Entrada,
    cancelar: &AtomicBool,
    tx: SyncSender<Lote<'a>>,
) {
    let mut lote = Vec::with_capacity(LOTE);
    let mut vivo = true;
    cada_registro(dados, entrada, cancelar, |linha, r| {
        lote.push((linha, r));
        if lote.len() == LOTE {
            vivo = tx
                .send(std::mem::replace(&mut lote, Vec::with_capacity(LOTE)))
                .is_ok();
        }
        vivo && !cancelar.load(Relaxed)
    });
    if vivo && !lote.is_empty() {
        let _ = tx.send(lote);
    }
}

/// O registro `n` (base 0) e a linha onde começa.
fn registro<'a>(
    dados: &'a [u8],
    entrada: Entrada,
    n: u64,
    cancelar: &AtomicBool,
) -> Option<(u64, &'a [u8])> {
    let (mut i, mut achado) = (0, None);
    cada_registro(dados, entrada, cancelar, |linha, r| {
        if i == n {
            achado = Some((linha, r));
        }
        i += 1;
        achado.is_none() && !cancelar.load(Relaxed)
    });
    achado
}

/// Compara o registro `na` de `a` com o `nb` de `b` (base 0), no mesmo formato de `arquivos`:
/// o que falta de um lado conta como só antes/só depois.
pub fn registros(
    a: &[u8],
    ea: Entrada,
    na: u64,
    b: &[u8],
    eb: Entrada,
    nb: u64,
    cancelar: &AtomicBool,
) -> DiffArquivos {
    let (x, y) = (registro(a, ea, na, cancelar), registro(b, eb, nb, cancelar));
    let mut r = DiffArquivos {
        registros_antes: u64::from(x.is_some()),
        registros_depois: u64::from(y.is_some()),
        cancelado: cancelar.load(Relaxed),
        ..DiffArquivos::default()
    };
    let inteiro = |v: &[u8]| Some(previa(&String::from_utf8_lossy(v.trim_ascii())));
    let (total, diferencas) = match (x, y) {
        (None, None) => return r,
        (Some((_, x)), Some((_, y))) => match par(x, y) {
            Par::Iguais => {
                r.iguais = 1;
                return r;
            }
            Par::Invalido => {
                r.invalidos = 1;
                return r;
            }
            Par::Grande => {
                r.grandes = 1;
                return r;
            }
            Par::Diferentes { diferencas, total } => {
                r.diferentes = 1;
                r.diferencas = total;
                (total, diferencas)
            }
        },
        (Some((_, x)), None) => {
            r.so_antes = 1;
            (
                1,
                vec![Diferenca {
                    caminho: ".".into(),
                    acao: Acao::Removido,
                    antes: inteiro(x),
                    depois: None,
                }],
            )
        }
        (None, Some((_, y))) => {
            r.so_depois = 1;
            (
                1,
                vec![Diferenca {
                    caminho: ".".into(),
                    acao: Acao::Adicionado,
                    antes: None,
                    depois: inteiro(y),
                }],
            )
        }
    };
    r.guardados.push(RegistroDiferente {
        registro: na,
        linha_antes: x.map(|p| p.0),
        linha_depois: y.map(|p| p.0),
        total,
        diferencas,
    });
    r
}

#[cfg(test)]
mod testes {
    use super::*;

    fn dif(a: &str, b: &str) -> Vec<String> {
        match par(a.as_bytes(), b.as_bytes()) {
            Par::Iguais => vec![],
            Par::Invalido => vec!["inválido".into()],
            Par::Grande => vec!["grande".into()],
            Par::Diferentes { diferencas, total } => {
                assert_eq!(total, diferencas.len() as u64);
                diferencas.iter().map(ToString::to_string).collect()
            }
        }
    }

    /// Dourados: a linha exata de cada ação, aninhado, lista e chave hostil.
    #[test]
    fn dourados() {
        assert!(dif("{\"a\":1}", "{\"a\":1}").is_empty());
        // Formatação diferente, mesmo valor: igual.
        assert!(dif("{\"a\": [1, 2]}", "{\"a\":[1,2]}").is_empty());
        assert_eq!(dif("{\"a\":1}", "{\"a\":2}"), ["alterado    .a  1 → 2"]);
        assert_eq!(
            dif("{\"a\":1,\"b\":2}", "{\"b\":2,\"c\":3}"),
            ["removido    .a  1", "adicionado  .c  3"]
        );
        assert_eq!(
            dif("{\"o\":{\"x\":[1,2,3]}}", "{\"o\":{\"x\":[1,5]}}"),
            ["alterado    .o.x[1]  2 → 5", "removido    .o.x[2]  3"]
        );
        assert_eq!(
            dif("[1]", "[1,{\"k\":true}]"),
            ["adicionado  .[1]  {\"k\":true}"]
        );
        // Tipo diferente: o valor inteiro muda, sem descer.
        assert_eq!(
            dif("{\"o\":{\"x\":1}}", "{\"o\":[1]}"),
            ["alterado    .o  {\"x\":1} → [1]"]
        );
        // Fiel ao byte: 1 ≠ 1.0, escape ≠ caractere.
        assert_eq!(dif("{\"n\":1}", "{\"n\":1.0}"), ["alterado    .n  1 → 1.0"]);
        assert_eq!(
            dif("{\"s\":\"é\"}", "{\"s\":\"\\u00e9\"}"),
            ["alterado    .s  \"é\" → \"\\u00e9\""]
        );
        // Chave que não é identificador: `["…"]`, reescapada.
        assert_eq!(
            dif("{\"a b\":1,\"q\\\"\":1}", "{\"a b\":2,\"q\\\"\":2}"),
            [
                "alterado    .[\"a b\"]  1 → 2",
                "alterado    .[\"q\\\"\"]  1 → 2"
            ]
        );
        // Chave repetida: vale a primeira.
        assert!(dif("{\"a\":1,\"a\":2}", "{\"a\":1}").is_empty());
        assert_eq!(dif("{\"a\":1}", "{"), ["inválido"]);
        // Bytes iguais: iguais sem ler o JSON; espaço nas pontas não conta, nem inválido.
        assert!(dif("{", "{").is_empty());
        assert!(dif(" {\t", "{").is_empty());
    }

    /// Lista de 60 contra outra: 60 diferenças, só as 50 primeiras guardadas.
    fn listas_60() -> (String, String) {
        let l = |x| format!("[{}]", [x; 60].join(","));
        (l("1"), l("2"))
    }

    #[test]
    fn teto_por_registro_conta_tudo() {
        let (a, b) = listas_60();
        let Par::Diferentes { diferencas, total } = par(a.as_bytes(), b.as_bytes()) else {
            panic!()
        };
        assert_eq!((diferencas.len(), total), (MAX_POR_REGISTRO, 60));
        assert_eq!(diferencas[49].caminho, ".[49]");
    }

    /// O teto vale nos dois caminhos públicos (arquivo inteiro e par escolhido).
    #[test]
    fn teto_por_registro_nos_dois_caminhos() {
        let (a, b) = listas_60();
        let nada = AtomicBool::new(false);
        let r = arquivos(
            a.as_bytes(),
            Entrada::Jsonl,
            b.as_bytes(),
            Entrada::Jsonl,
            &nada,
            |_| {},
        );
        let s = registros(
            a.as_bytes(),
            Entrada::Jsonl,
            0,
            b.as_bytes(),
            Entrada::Jsonl,
            0,
            &nada,
        );
        for r in [r, s] {
            let g = &r.guardados[0];
            assert_eq!(
                (g.diferencas.len(), g.total, r.diferencas),
                (MAX_POR_REGISTRO, 60, 60)
            );
        }
    }

    /// Registro acima do teto de bytes não é analisado: conta em `grandes`, nos dois caminhos.
    /// Igual em bytes continua igual, de qualquer tamanho.
    #[test]
    fn registro_grande_nao_comparado() {
        let g = format!("[\"{}\"]", "x".repeat(MAX_BYTES_REGISTRO));
        let nada = AtomicBool::new(false);
        assert_eq!(par(g.as_bytes(), b"[1]"), Par::Grande);
        assert_eq!(par(b"[1]", g.as_bytes()), Par::Grande);
        assert_eq!(par(g.as_bytes(), g.as_bytes()), Par::Iguais);
        let (a, b) = (format!("[1]\n{g}\n"), "[1]\n[2]\n");
        let r = arquivos(
            a.as_bytes(),
            Entrada::Jsonl,
            b.as_bytes(),
            Entrada::Jsonl,
            &nada,
            |_| {},
        );
        assert_eq!(
            (r.iguais, r.grandes, r.diferentes, r.guardados.len()),
            (1, 1, 0, 0)
        );
        let r = registros(
            a.as_bytes(),
            Entrada::Jsonl,
            1,
            b.as_bytes(),
            Entrada::Jsonl,
            1,
            &nada,
        );
        assert_eq!((r.grandes, r.diferentes, r.guardados.len()), (1, 0, 0));
    }

    #[test]
    fn previa_cortada() {
        let longo = format!("\"{}\"", "x".repeat(500));
        let Par::Diferentes { diferencas, .. } =
            par(format!("{{\"s\":{longo}}}").as_bytes(), b"{\"s\":1}")
        else {
            panic!()
        };
        let a = diferencas[0].antes.as_deref().unwrap();
        assert!(a.len() < 210 && a.ends_with('…'), "{a}");
    }

    fn dif_arquivos(a: &str, b: &str) -> DiffArquivos {
        let nada = AtomicBool::new(false);
        let e = |d: &str| crate::estrutura::entrada_provavel(d.as_bytes());
        arquivos(a.as_bytes(), e(a), b.as_bytes(), e(b), &nada, |_| {})
    }

    #[test]
    fn arquivos_por_posicao() {
        let r = dif_arquivos(
            "{\"a\":1}\n{\"a\":2}\n\n{\"a\":3}\n{\"a\":4}\n",
            "{\"a\":1}\n{\"a\":9}\n{\"a\":3}\n{\n",
        );
        // A linha vazia de antes não é registro: o 3º de cada lado é `{"a":3}`.
        assert_eq!(
            (
                r.registros_antes,
                r.registros_depois,
                r.iguais,
                r.diferentes
            ),
            (4, 4, 2, 1)
        );
        assert_eq!(
            (r.invalidos, r.so_antes, r.so_depois, r.diferencas),
            (1, 0, 0, 1)
        );
        assert_eq!(r.guardados.len(), 1);
        let g = &r.guardados[0];
        assert_eq!(
            (g.registro, g.linha_antes, g.linha_depois),
            (1, Some(1), Some(1))
        );
        assert_eq!(g.diferencas[0].to_string(), "alterado    .a  2 → 9");

        let r = dif_arquivos("{\"a\":1}\n", "{\"a\":1}\n{\"b\":2}\n{\"c\":3}");
        assert_eq!((r.so_antes, r.so_depois, r.iguais), (0, 2, 1));
        let g = &r.guardados[1];
        assert_eq!(
            (g.registro, g.linha_antes, g.linha_depois),
            (2, None, Some(2))
        );
        assert_eq!(g.diferencas[0].to_string(), "adicionado  .  {\"c\":3}");
        let r = dif_arquivos("{\"a\":1}\n{\"b\":2}\n", "{\"a\":1}\n");
        assert_eq!(
            (r.so_antes, r.guardados[0].diferencas[0].acao),
            (1, Acao::Removido)
        );
    }

    /// JSON único que é lista: cada elemento é um registro, mesmo em várias linhas.
    #[test]
    fn json_unico_por_elemento() {
        let r = dif_arquivos(
            "[\n  {\"a\": 1},\n  {\"a\": 2}\n]\n",
            "[{\"a\":1},{\"a\":3},{\"a\":4}]",
        );
        assert_eq!((r.iguais, r.diferentes, r.so_depois), (1, 1, 1));
        assert_eq!(r.guardados[0].linha_antes, Some(2));
        assert_eq!(
            r.guardados[0].diferencas[0].to_string(),
            "alterado    .a  2 → 3"
        );
    }

    #[test]
    fn teto_de_registros_guardados_e_lotes() {
        // Mais registros que um lote e mais diferentes que o teto: contagem inteira.
        let n = LOTE * 3 + 7;
        let a: String = (0..n).map(|i| format!("{{\"i\":{i}}}\n")).collect();
        let b: String = (0..n).map(|i| format!("{{\"i\":{}}}\n", i + 1)).collect();
        let r = dif_arquivos(&a, &b);
        assert_eq!((r.diferentes, r.registros_antes), (n as u64, n as u64));
        assert_eq!(r.guardados.len(), MAX_REGISTROS);
        assert_eq!(
            r.guardados.last().unwrap().registro,
            MAX_REGISTROS as u64 - 1
        );
        let r = dif_arquivos(&a, &a);
        assert_eq!((r.iguais, r.diferentes), (n as u64, 0));
    }

    /// Dois registros quaisquer: o 3º de antes contra o 1º de depois, e um que não existe.
    #[test]
    fn dois_registros() {
        let nada = AtomicBool::new(false);
        let (a, b) = ("{\"a\":1}\n\n{\"a\":2}\n", "[{\"a\":3}]");
        let r = registros(
            a.as_bytes(),
            Entrada::Jsonl,
            1,
            b.as_bytes(),
            Entrada::JsonUnico,
            0,
            &nada,
        );
        assert_eq!((r.diferentes, r.diferencas), (1, 1));
        let g = &r.guardados[0];
        assert_eq!(
            (g.registro, g.linha_antes, g.linha_depois),
            (1, Some(2), Some(0))
        );
        assert_eq!(g.diferencas[0].to_string(), "alterado    .a  2 → 3");
        let r = registros(
            a.as_bytes(),
            Entrada::Jsonl,
            0,
            a.as_bytes(),
            Entrada::Jsonl,
            0,
            &nada,
        );
        assert_eq!((r.iguais, r.guardados.len()), (1, 0));
        let r = registros(
            a.as_bytes(),
            Entrada::Jsonl,
            0,
            b.as_bytes(),
            Entrada::JsonUnico,
            5,
            &nada,
        );
        assert_eq!((r.so_antes, r.registros_depois), (1, 0));
        assert_eq!(
            r.guardados[0].diferencas[0].to_string(),
            "removido    .  {\"a\":1}"
        );
    }

    #[test]
    fn cancelado_para() {
        let a: String = (0..20_000).map(|i| format!("{{\"i\":{i}}}\n")).collect();
        let sim = AtomicBool::new(true);
        let r = arquivos(
            a.as_bytes(),
            Entrada::Jsonl,
            a.as_bytes(),
            Entrada::Jsonl,
            &sim,
            |_| {},
        );
        assert!(r.cancelado);
        assert_eq!(r.iguais, 0);
    }
}
