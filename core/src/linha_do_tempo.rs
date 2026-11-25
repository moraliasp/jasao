//! Linha do tempo de uma pasta de arquivos datados: as versões em ordem de data (no nome ou, se
//! algum nome não tem data, pelo `mtime`) e o diff de estrutura de cada uma contra a anterior.
//!
//! A pasta é entrada não confiável. Só a própria pasta (sem descer), só arquivo comum com
//! extensão que o app abre, e link simbólico só entra se o alvo mora dentro da pasta. Tetos
//! (`Limites`): entradas lidas da pasta, arquivos aceitos, prazo do percurso e mudanças guardadas
//! por passo; em memória ficam só dois `Resumo` (o anterior e o atual). Decisão em
//! `docs/decisoes.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::diff::{Classe, Mudanca, diff};
use crate::estrutura::{self, Resumo};
use crate::{ArquivoLinhas, Opcoes};

#[derive(Debug, Clone, Copy)]
pub struct Limites {
    /// Entradas lidas da pasta (de qualquer tipo): passar disso é erro, não lista parcial.
    pub max_entradas: usize,
    /// Arquivos aceitos (versões): passar disso é erro.
    pub max_arquivos: usize,
    /// Tempo do percurso; estourou, o resultado sai parcial (`Parada::Prazo`).
    pub prazo: Duration,
    /// Mudanças guardadas por passo; o resto só conta (`Passo::mais`).
    pub max_mudancas: usize,
}

pub const LIMITES: Limites = Limites {
    max_entradas: 100_000,
    max_arquivos: 500,
    prazo: Duration::from_secs(600),
    max_mudancas: 200,
};

#[derive(Debug)]
pub enum Erro {
    MuitasEntradas(usize),
    MuitosArquivos(usize),
    Io(io::Error),
}

impl std::fmt::Display for Erro {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Erro::MuitasEntradas(n) => write!(f, "a pasta tem mais de {n} entradas"),
            Erro::MuitosArquivos(n) => write!(f, "a pasta tem mais de {n} arquivos para comparar"),
            Erro::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Erro {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origem {
    /// Data no nome, lida como UTC.
    Nome,
    Mtime,
}

#[derive(Debug, Clone)]
pub struct Versao {
    /// Canônico, dentro da pasta canônica.
    pub caminho: PathBuf,
    pub nome: String,
    /// Milissegundos desde 1970 (do nome, como UTC; ou o `mtime`).
    pub ms: i64,
}

#[derive(Debug)]
pub struct Lista {
    pub origem: Origem,
    pub versoes: Vec<Versao>,
    /// Entradas recusadas: extensão, pasta, oculto, link para fora, não comum.
    pub ignorados: usize,
}

/// Mesmo conjunto do `aceito` do app (`app/src/abas.ts`).
fn aceito(nome: &str) -> bool {
    let n = nome.to_ascii_lowercase();
    let base = n
        .strip_suffix(".gz")
        .or_else(|| n.strip_suffix(".zst"))
        .unwrap_or(&n);
    [".json", ".jsonl", ".ndjson"]
        .iter()
        .any(|e| base.ends_with(e))
        || [
            ".yaml", ".yml", ".toml", ".msgpack", ".mpk", ".cbor", ".bson",
        ]
        .iter()
        .any(|e| n.ends_with(e))
}

/// Lê as versões de `pasta`, em ordem. Ver os critérios no topo do arquivo.
pub fn listar(pasta: &Path, lim: &Limites) -> Result<Lista, Erro> {
    let base = fs::canonicalize(pasta).map_err(Erro::Io)?;
    let mut achados = Vec::new();
    let mut ignorados = 0;
    for (i, e) in fs::read_dir(&base).map_err(Erro::Io)?.enumerate() {
        if i == lim.max_entradas {
            return Err(Erro::MuitasEntradas(lim.max_entradas));
        }
        let Ok(e) = e else {
            ignorados += 1;
            continue;
        };
        let nome = e.file_name().to_string_lossy().into_owned();
        match versao(&base, &e, &nome) {
            Some(_) if achados.len() == lim.max_arquivos => {
                return Err(Erro::MuitosArquivos(lim.max_arquivos));
            }
            Some(v) => achados.push(v),
            None => ignorados += 1,
        }
    }
    let datas: Option<Vec<i64>> = achados.iter().map(|(_, _, d)| *d).collect();
    let origem = if datas.is_some() && !achados.is_empty() {
        Origem::Nome
    } else {
        Origem::Mtime
    };
    let mut versoes: Vec<Versao> = achados
        .into_iter()
        .map(|((caminho, nome), mtime, data)| Versao {
            caminho,
            nome,
            ms: if origem == Origem::Nome {
                data.unwrap_or(0)
            } else {
                mtime
            },
        })
        .collect();
    ordenar(&mut versoes);
    Ok(Lista {
        origem,
        versoes,
        ignorados,
    })
}

/// Por data; data igual (ou mtime igual), por nome.
fn ordenar(versoes: &mut [Versao]) {
    versoes.sort_by(|a, b| a.ms.cmp(&b.ms).then_with(|| a.nome.cmp(&b.nome)));
}

type Achado = ((PathBuf, String), i64, Option<i64>);

/// A versão de uma entrada, ou `None` se ela não entra.
fn versao(base: &Path, e: &fs::DirEntry, nome: &str) -> Option<Achado> {
    if nome.starts_with('.') || !aceito(nome) {
        return None;
    }
    // `file_type` não segue link: o link só entra se o alvo canônico mora dentro da pasta.
    let tipo = e.file_type().ok()?;
    let caminho = if tipo.is_symlink() {
        let alvo = fs::canonicalize(e.path()).ok()?;
        alvo.starts_with(base).then_some(alvo)?
    } else {
        e.path()
    };
    let meta = fs::metadata(&caminho).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    Some(((caminho, nome.to_owned()), mtime, data_no_nome(nome)))
}

/// A primeira data do nome, em ms desde 1970 (UTC): `AAAA-MM-DD`, `AAAA_MM_DD`, `AAAA.MM.DD` ou
/// `AAAAMMDD`, entre 1970 e 2100, com hora opcional (`T`, `_`, `-` ou espaço, e então `HH`,
/// `HHMM`, `HHMMSS` ou `HH:MM[:SS]`/`HH-MM[-SS]`, e fuso `Z`/`±HH:MM`/`±HHMM` opcional depois
/// da hora). Componentes conferidos: `2024-02-30` não vale.
/// Dígito colado antes ou depois não vale (id ou EAN não vira data); sufixo numérico depois de
/// `_` ou `-` vale, e então a data fica sem hora (`2024-01-01_001`).
pub fn data_no_nome(nome: &str) -> Option<i64> {
    let b = nome.as_bytes();
    (0..b.len()).find_map(|i| {
        if i > 0 && b[i - 1].is_ascii_digit() {
            return None;
        }
        data_em(&b[i..])
    })
}

fn num(b: &[u8], n: usize) -> Option<u32> {
    let d = b.get(..n)?;
    d.iter()
        .all(u8::is_ascii_digit)
        .then(|| d.iter().fold(0, |a, c| a * 10 + u32::from(c - b'0')))
}

fn data_em(b: &[u8]) -> Option<i64> {
    let ano = num(b, 4)?;
    let sep = *b.get(4)?;
    let s = if matches!(sep, b'-' | b'_' | b'.') {
        1
    } else {
        0
    };
    let mes = num(&b[4 + s..], 2)?;
    if s == 1 && b.get(7) != Some(&sep) {
        return None;
    }
    let dia = num(&b[6 + 2 * s..], 2)?;
    let fim = 8 + 2 * s;
    if !(1970..=2100).contains(&ano) || !(1..=12).contains(&mes) {
        return None;
    }
    let bissexto = ano % 4 == 0 && (ano % 100 != 0 || ano % 400 == 0);
    let dias = [
        31,
        if bissexto { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if dia == 0 || dia > dias[mes as usize - 1] {
        return None;
    }
    let (seg, depois) = hora(&b[fim..]);
    // Fuso só depois de uma hora lida.
    let (seg, depois) = if depois.len() < b.len() - fim {
        fuso(seg, depois)
    } else {
        (seg, depois)
    };
    let seg = if !depois.first().is_some_and(u8::is_ascii_digit) {
        seg
    } else if matches!(b.get(fim), Some(b'_' | b'-')) {
        // Sufixo numérico separado (`export-2024-01-01_001`): a data vale, sem hora.
        0
    } else {
        return None;
    };
    Some((dias_desde_1970(ano, mes, dia) * 86_400 + seg) * 1000)
}

/// Segundos da hora logo depois da data (0 se não há) e o que sobra.
fn hora(b: &[u8]) -> (i64, &[u8]) {
    let nada = (0, b);
    if !matches!(b.first(), Some(b'T' | b't' | b'_' | b'-' | b' ')) {
        return nada;
    }
    let r = &b[1..];
    let Some(h) = num(r, 2).filter(|h| *h < 24) else {
        return nada;
    };
    let mut p = 2;
    let mut partes = [h, 0, 0];
    // O separador dos minutos vale para os segundos: em `10:00-03:00` o `-03:00` é fuso, não
    // segundos (`10-00-03` continua 10:00:03).
    let mut sep = None;
    for parte in partes.iter_mut().skip(1) {
        let c = match r.get(p) {
            Some(&c @ (b':' | b'-')) => c,
            _ => 0,
        };
        if *sep.get_or_insert(c) != c {
            break;
        }
        let s = usize::from(c != 0);
        match num(&r[(p + s).min(r.len())..], 2).filter(|m| *m < 60) {
            Some(m) => {
                *parte = m;
                p += s + 2;
            }
            None => break,
        }
    }
    let [h, m, s] = partes.map(i64::from);
    (h * 3600 + m * 60 + s, &r[p..])
}

/// `Z` ou `±HH:MM`/`±HHMM` logo depois da hora: leva os segundos para UTC. Sem fuso, a hora
/// fica como está (tratada como UTC).
fn fuso(seg: i64, b: &[u8]) -> (i64, &[u8]) {
    match b.first() {
        Some(b'Z' | b'z') => (seg, &b[1..]),
        Some(&c @ (b'+' | b'-')) => {
            let r = &b[1..];
            let s = usize::from(r.get(2) == Some(&b':'));
            let h = num(r, 2).filter(|h| *h < 24);
            let m = num(&r[(2 + s).min(r.len())..], 2).filter(|m| *m < 60);
            match (h, m) {
                (Some(h), Some(m)) => {
                    let o = i64::from(h * 3600 + m * 60);
                    (if c == b'-' { seg + o } else { seg - o }, &r[4 + s..])
                }
                _ => (seg, b),
            }
        }
        _ => (seg, b),
    }
}

/// Dias de 1970-01-01 até a data (algoritmo de Howard Hinnant, `days_from_civil`).
fn dias_desde_1970(ano: u32, mes: u32, dia: u32) -> i64 {
    let y = i64::from(ano) - i64::from(mes <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(mes);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(dia) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `AAAA-MM-DD HH:MM:SS` em UTC (algoritmo `civil_from_days`, de Howard Hinnant).
pub fn data_texto(ms: i64) -> String {
    let (dias, seg) = (
        ms.div_euclid(86_400_000),
        ms.div_euclid(1000).rem_euclid(86_400),
    );
    let z = dias + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dia = doy - (153 * mp + 2) / 5 + 1;
    let mes = if mp < 10 { mp + 3 } else { mp - 9 };
    let ano = yoe + era * 400 + i64::from(mes <= 2);
    format!(
        "{ano:04}-{mes:02}-{dia:02} {:02}:{:02}:{:02}",
        seg / 3600,
        seg / 60 % 60,
        seg % 60
    )
}

/// Uma versão e o diff dela contra a anterior que abriu.
#[derive(Debug)]
pub struct Passo {
    pub versao: Versao,
    /// Por que não deu para ler; o passo seguinte compara com a última que abriu.
    pub erro: Option<String>,
    pub registros: u64,
    pub invalidas: u64,
    /// Índice (em `passos`) da versão contra a qual comparou; `None` na primeira que abriu.
    pub contra: Option<usize>,
    pub quebras: usize,
    pub compativeis: usize,
    /// As primeiras `Limites::max_mudancas`.
    pub mudancas: Vec<Mudanca>,
    /// Mudanças além das guardadas.
    pub mais: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parada {
    Prazo,
    Cancelado,
}

#[derive(Debug)]
pub struct LinhaDoTempo {
    pub origem: Origem,
    pub passos: Vec<Passo>,
    pub ignorados: usize,
    /// `Some`: parou antes do fim e `passos` tem só as versões percorridas inteiras.
    pub parada: Option<Parada>,
}

/// Percorre as versões em ordem: abre (sempre por cópia, com o cache, o progresso e o
/// cancelamento de `op`), infere e
/// compara com a anterior; versão que muda entre abrir e inferir vira passo com erro. Abre com
/// `ArquivoLinhas::abrir_copia_com`, nunca o `mmap` do original (SIGBUS se outro processo truncar).
/// `progresso(feitas, total)` antes de cada versão. O prazo e `cancelar`
/// valem também no meio de uma inferência (a cada ~1% dela); a abertura em si não é interrompida.
pub fn percorrer(
    lista: Lista,
    lim: &Limites,
    op: &Opcoes,
    cancelar: &AtomicBool,
    progresso: impl FnMut(usize, usize),
) -> LinhaDoTempo {
    percorrer_com(
        lista,
        lim,
        |c| ArquivoLinhas::abrir_copia_com(c, op),
        cancelar,
        progresso,
    )
}

/// `percorrer` com a abertura trocável (os testes simulam o arquivo mudando no meio).
fn percorrer_com(
    lista: Lista,
    lim: &Limites,
    mut abrir: impl FnMut(&Path) -> io::Result<ArquivoLinhas>,
    cancelar: &AtomicBool,
    mut progresso: impl FnMut(usize, usize),
) -> LinhaDoTempo {
    let t0 = Instant::now();
    let total = lista.versoes.len();
    let parar = AtomicBool::new(false);
    let checar = || {
        if cancelar.load(Relaxed) {
            Some(Parada::Cancelado)
        } else if t0.elapsed() >= lim.prazo {
            Some(Parada::Prazo)
        } else {
            None
        }
    };
    let mut r = LinhaDoTempo {
        origem: lista.origem,
        passos: Vec::with_capacity(total),
        ignorados: lista.ignorados,
        parada: None,
    };
    // Só o resumo anterior fica vivo: memória de dois resumos, não de `total`.
    let mut anterior: Option<(usize, Resumo)> = None;
    for (i, versao) in lista.versoes.into_iter().enumerate() {
        if let Some(p) = checar() {
            r.parada = Some(p);
            break;
        }
        progresso(i, total);
        let mut passo = Passo {
            versao,
            erro: None,
            registros: 0,
            invalidas: 0,
            contra: None,
            quebras: 0,
            compativeis: 0,
            mudancas: Vec::new(),
            mais: 0,
        };
        let antes = carimbo(&passo.versao.caminho);
        let inf = match abrir(&passo.versao.caminho) {
            Ok(a) => estrutura::inferir(a.bytes(), &parar, |_| {
                if checar().is_some() {
                    parar.store(true, Relaxed);
                }
            }),
            Err(e) => {
                passo.erro = Some(e.to_string());
                r.passos.push(passo);
                continue;
            }
        };
        if inf.cancelado {
            r.parada = checar().or(Some(Parada::Cancelado));
            break;
        }
        if carimbo(&passo.versao.caminho) != antes {
            passo.erro = Some("o arquivo mudou durante a leitura".into());
            r.passos.push(passo);
            continue;
        }
        passo.registros = inf.resumo.registros();
        passo.invalidas = inf.resumo.invalidas.n;
        if let Some((j, antes)) = &anterior {
            let mut ms = diff(antes, &inf.resumo);
            passo.contra = Some(*j);
            passo.quebras = ms
                .iter()
                .filter(|m| m.regra.classe() == Classe::Quebra)
                .count();
            passo.compativeis = ms.len() - passo.quebras;
            passo.mais = ms.len().saturating_sub(lim.max_mudancas);
            ms.truncate(lim.max_mudancas);
            passo.mudancas = ms;
        }
        anterior = Some((r.passos.len(), inf.resumo));
        r.passos.push(passo);
    }
    if r.parada.is_none() {
        progresso(total, total);
    }
    r
}

/// Tamanho e `mtime`: mudou entre abrir e terminar a inferência, o passo sai como erro.
fn carimbo(c: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    fs::metadata(c).ok().map(|m| (m.len(), m.modified().ok()))
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::io::Write;

    fn escrever(p: &Path, nome: &str, conteudo: &str) {
        fs::write(p.join(nome), conteudo).unwrap();
    }

    fn nomes(l: &Lista) -> Vec<&str> {
        l.versoes.iter().map(|v| v.nome.as_str()).collect()
    }

    const ABRIR: fn(&Path) -> io::Result<ArquivoLinhas> = |c| ArquivoLinhas::abrir(c);

    #[test]
    fn datas_no_nome() {
        let d = |s| data_no_nome(s).map(|ms| ms / 1000);
        assert_eq!(d("log-1970-01-01.jsonl"), Some(0));
        assert_eq!(d("2024-02-29.json"), Some(1_709_164_800));
        assert_eq!(d("x_20240229.json"), Some(1_709_164_800));
        assert_eq!(d("2024_02_29.json"), Some(1_709_164_800));
        assert_eq!(d("2024.02.29.json"), Some(1_709_164_800));
        assert_eq!(d("2024-02-29T01:02:03.json"), Some(1_709_164_800 + 3723));
        assert_eq!(d("20240229_010203.json"), Some(1_709_164_800 + 3723));
        assert_eq!(d("2024-02-29_01.json"), Some(1_709_164_800 + 3600));
        assert_eq!(d("2100-12-31.json"), Some(4_133_894_400));
        // Data que não existe, separador misto, fora da faixa, dígito colado.
        for ruim in [
            "2023-02-29.json",
            "2024-02-30.json",
            "2024-13-01.json",
            "2024-00-10.json",
            "2024-01_10.json",
            "1969-12-31.json",
            "2101-01-01.json",
            "120240101.json",
            "202401011.json",
            "dados.json",
            "2024-1-1.json",
        ] {
            assert_eq!(d(ruim), None, "{ruim}");
        }
        // A primeira data que vale, mesmo depois de um número que não é data.
        assert_eq!(d("v9999-2024-01-01.json"), Some(1_704_067_200));
        assert_eq!(data_texto(0), "1970-01-01 00:00:00");
        for n in [
            "2024-02-29T01:02:03",
            "2100-12-31T23:59:59",
            "2000-03-01T00:00:00",
        ] {
            assert_eq!(data_texto(data_no_nome(n).unwrap()), n.replace('T', " "));
        }
        assert_eq!(d("2024-01-01.v2.json"), Some(1_704_067_200));
        // Sufixo numérico depois de `_` ou `-`: data sem hora; colado continua não valendo.
        assert_eq!(d("export-2024-01-01_001.jsonl"), Some(1_704_067_200));
        assert_eq!(d("export-20240101-7.jsonl"), Some(1_704_067_200));
        assert_eq!(d("2024-01-01-12345.json"), Some(1_704_067_200));
        // Fuso depois da hora vai para UTC: 10:00 em -03:00 é 13:00Z, depois de 12:00Z.
        let t = 1_704_067_200;
        assert_eq!(
            d("dump-2024-01-01T10:00:00-03:00.json"),
            Some(t + 13 * 3600)
        );
        assert_eq!(d("dump-2024-01-01T12:00:00Z.json"), Some(t + 12 * 3600));
        assert_eq!(d("dump-2024-01-01T10:00-03:00.json"), Some(t + 13 * 3600));
        assert_eq!(d("dump-20240101T100000-0300.json"), Some(t + 13 * 3600));
        assert_eq!(
            d("dump-2024-01-01T10:00:00+05:30.json"),
            Some(t + 4 * 3600 + 1800)
        );
        assert_eq!(d("dump-2024-01-01T10-00-03.json"), Some(t + 10 * 3600 + 3));
        assert_eq!(d("2024-01-01001.json"), None);
    }

    #[test]
    fn sufixo_apos_a_data_mantem_a_ordem_pelo_nome() {
        let p = tempfile::tempdir().unwrap();
        escrever(p.path(), "export-2024-01-02_001.jsonl", "{}");
        escrever(p.path(), "export-2024-01-01_002.jsonl", "{}");
        escrever(p.path(), "export-2024-01-01_001.jsonl", "{}");
        let l = listar(p.path(), &LIMITES).unwrap();
        assert_eq!(l.origem, Origem::Nome);
        assert_eq!(
            nomes(&l),
            [
                "export-2024-01-01_001.jsonl",
                "export-2024-01-01_002.jsonl",
                "export-2024-01-02_001.jsonl"
            ]
        );
    }

    #[test]
    fn data_igual_desempata_pelo_nome() {
        // `ordenar` direto: a ordem do `read_dir` (às vezes já alfabética) não mascara o desempate.
        let v = |nome: &str, ms| Versao {
            caminho: PathBuf::from(nome),
            nome: nome.into(),
            ms,
        };
        let mut vs = vec![v("c", 5), v("b", 1), v("a", 5), v("z", 0)];
        ordenar(&mut vs);
        let ns: Vec<&str> = vs.iter().map(|v| v.nome.as_str()).collect();
        assert_eq!(ns, ["z", "b", "a", "c"]);
        // Pela pasta: criados em ordem inversa à do nome, mesma data.
        let p = tempfile::tempdir().unwrap();
        for n in [
            "z-2024-01-01.json",
            "m-2024-01-01.json",
            "a-2024-01-01.json",
        ] {
            escrever(p.path(), n, "{}");
        }
        assert_eq!(
            nomes(&listar(p.path(), &LIMITES).unwrap()),
            [
                "a-2024-01-01.json",
                "m-2024-01-01.json",
                "z-2024-01-01.json"
            ]
        );
    }

    fn op_em(p: &Path) -> Opcoes<'static> {
        Opcoes {
            dir_cache: Some(p.join("cache")),
            ..Opcoes::default()
        }
    }

    #[test]
    fn msgpack_truncado_na_conversao_vira_erro_sem_derrubar() {
        // Pela `percorrer` pública: a abertura que ela escolhe é a que está em teste.
        let p = tempfile::tempdir().unwrap();
        let alvo = p.path().join("2024-01-02.msgpack");
        // 2^20 mapas `{"a":1}` (4 bytes cada): a conversão avisa progresso a cada MiB lido.
        fs::write(&alvo, [0x81, 0xA1, b'a', 0x01].repeat(1 << 20)).unwrap();
        escrever(p.path(), "2024-01-01.jsonl", "{\"a\":1}\n");
        let truncou = std::cell::Cell::new(false);
        let truncar = |_: u64, _: u64| {
            if !truncou.replace(true) {
                fs::File::create(&alvo).unwrap();
            }
        };
        let op = Opcoes {
            progresso: Some(&truncar),
            ..op_em(p.path())
        };
        let l = listar(p.path(), &LIMITES).unwrap();
        let r = percorrer(l, &LIMITES, &op, &AtomicBool::new(false), |_, _| {});
        assert!(truncou.get(), "a conversão não chegou a avisar progresso");
        assert_eq!(r.passos.len(), 2);
        assert!(r.passos[0].erro.is_none());
        assert_eq!(
            r.passos[1].erro.as_deref(),
            Some("o arquivo mudou durante a leitura")
        );
    }

    #[test]
    fn reescrito_com_o_mesmo_tamanho_vira_erro() {
        let p = tempfile::tempdir().unwrap();
        escrever(p.path(), "2024-01-01.jsonl", "{\"a\":1}\n");
        escrever(p.path(), "2024-01-02.jsonl", "{\"a\":2}\n");
        let op = op_em(p.path());
        let lista = listar(p.path(), &LIMITES).unwrap();
        let abrir = |c: &Path| {
            let a = ArquivoLinhas::abrir_copia_com(c, &op)?;
            if c.ends_with("2024-01-02.jsonl") {
                let f = fs::File::options().write(true).open(c)?;
                (&f).write_all(b"{\"b\":3}\n")?;
                f.set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000))?;
            }
            Ok(a)
        };
        let r = percorrer_com(lista, &LIMITES, abrir, &AtomicBool::new(false), |_, _| {});
        assert_eq!(
            fs::metadata(p.path().join("2024-01-02.jsonl"))
                .unwrap()
                .len(),
            8
        );
        assert_eq!(
            r.passos[1].erro.as_deref(),
            Some("o arquivo mudou durante a leitura")
        );
    }

    #[test]
    fn teto_da_copia() {
        let p = tempfile::tempdir().unwrap();
        escrever(p.path(), "2024-01-01.jsonl", "{\"a\":1}\n");
        escrever(p.path(), "2024-01-02.jsonl", "{\"a\":22}\n");
        let op = |n| Opcoes {
            max_saida: n,
            ..op_em(p.path())
        };
        let erros = |n| {
            let l = listar(p.path(), &LIMITES).unwrap();
            percorrer(l, &LIMITES, &op(n), &AtomicBool::new(false), |_, _| {})
                .passos
                .into_iter()
                .map(|s| s.erro)
                .collect::<Vec<_>>()
        };
        assert_eq!(erros(9), [None, None]);
        let e = erros(8);
        assert!(e[0].is_none());
        assert!(
            e[1].as_deref().unwrap().contains("maior que 8 bytes"),
            "{e:?}"
        );
    }

    #[test]
    fn arquivo_truncado_no_percurso_vira_erro_sem_derrubar() {
        let p = tempfile::tempdir().unwrap();
        let linhas = "{\"a\":1,\"b\":\"x\"}\n".repeat(50_000);
        escrever(p.path(), "2024-01-01.jsonl", "{\"a\":1}\n");
        escrever(p.path(), "2024-01-02.jsonl", &linhas);
        escrever(p.path(), "2024-01-03.jsonl", "{\"a\":2}\n");
        let op = op_em(p.path());
        let lista = listar(p.path(), &LIMITES).unwrap();
        // Trunca o original logo depois de abrir, antes da inferência: com `mmap` do original
        // isto seria SIGBUS e o processo de teste morreria.
        let abrir = |c: &Path| {
            let a = ArquivoLinhas::abrir_copia_com(c, &op)?;
            if c.ends_with("2024-01-02.jsonl") {
                fs::File::create(c)?;
            }
            Ok(a)
        };
        let r = percorrer_com(lista, &LIMITES, abrir, &AtomicBool::new(false), |_, _| {});
        assert_eq!(r.passos.len(), 3);
        assert!(r.passos[0].erro.is_none());
        assert_eq!(
            r.passos[1].erro.as_deref(),
            Some("o arquivo mudou durante a leitura")
        );
        // O seguinte compara com a última que abriu inteira.
        assert_eq!(r.passos[2].contra, Some(0));
        assert!(r.passos[2].erro.is_none());
    }

    #[test]
    fn ordena_pelo_nome_e_cai_para_mtime() {
        let p = tempfile::tempdir().unwrap();
        escrever(p.path(), "b-2024-03-01.json", "{}");
        escrever(p.path(), "a-2024-03-01.json", "{}");
        escrever(p.path(), "2023-12-31.jsonl", "{}");
        escrever(p.path(), "c-20240102.ndjson", "{}");
        escrever(p.path(), "leia.txt", "x");
        escrever(p.path(), ".2020-01-01.json", "{}");
        fs::create_dir(p.path().join("2019-01-01.json")).unwrap();
        let l = listar(p.path(), &LIMITES).unwrap();
        assert_eq!(l.origem, Origem::Nome);
        assert_eq!(
            nomes(&l),
            [
                "2023-12-31.jsonl",
                "c-20240102.ndjson",
                "a-2024-03-01.json",
                "b-2024-03-01.json"
            ]
        );
        assert_eq!(l.ignorados, 3);
        // Um nome sem data: a lista inteira passa ao mtime.
        escrever(p.path(), "sem-data.json", "{}");
        let l = listar(p.path(), &LIMITES).unwrap();
        assert_eq!(l.origem, Origem::Mtime);
        assert_eq!(l.versoes.len(), 5);
        let ms: Vec<i64> = l.versoes.iter().map(|v| v.ms).collect();
        assert!(ms.is_sorted());
        assert!(ms[0] > 1_600_000_000_000, "{ms:?}");
    }

    #[test]
    fn teto_de_arquivos() {
        let p = tempfile::tempdir().unwrap();
        for i in 0..4 {
            escrever(p.path(), &format!("{i}.json"), "{}");
        }
        let lim = |n| Limites {
            max_arquivos: n,
            ..LIMITES
        };
        assert_eq!(listar(p.path(), &lim(4)).unwrap().versoes.len(), 4);
        assert!(matches!(
            listar(p.path(), &lim(3)),
            Err(Erro::MuitosArquivos(3))
        ));
    }

    #[test]
    fn teto_de_entradas_conta_o_que_nao_entra() {
        let p = tempfile::tempdir().unwrap();
        for i in 0..4 {
            escrever(p.path(), &format!("{i}.txt"), "");
        }
        let lim = |n| Limites {
            max_entradas: n,
            ..LIMITES
        };
        assert_eq!(listar(p.path(), &lim(4)).unwrap().ignorados, 4);
        assert!(matches!(
            listar(p.path(), &lim(3)),
            Err(Erro::MuitasEntradas(3))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn link_so_dentro_da_pasta() {
        use std::os::unix::fs::symlink;
        let fora = tempfile::tempdir().unwrap();
        escrever(fora.path(), "segredo.json", "{}");
        let p = tempfile::tempdir().unwrap();
        escrever(p.path(), "2024-01-01.json", "{}");
        fs::create_dir(p.path().join("sub")).unwrap();
        escrever(&p.path().join("sub"), "x.json", "{}");
        symlink(
            fora.path().join("segredo.json"),
            p.path().join("2024-01-02.json"),
        )
        .unwrap();
        symlink(
            p.path().join("sub/x.json"),
            p.path().join("2024-01-03.json"),
        )
        .unwrap();
        symlink(fora.path(), p.path().join("2024-01-04.json")).unwrap();
        symlink(p.path().join("laço.json"), p.path().join("laço.json")).unwrap();
        let l = listar(p.path(), &LIMITES).unwrap();
        assert_eq!(nomes(&l), ["2024-01-01.json", "2024-01-03.json"]);
        let base = fs::canonicalize(p.path()).unwrap();
        assert!(l.versoes.iter().all(|v| v.caminho.starts_with(&base)));
        assert_eq!(l.versoes[1].caminho, base.join("sub/x.json"));
        assert_eq!(l.ignorados, 4); // sub, link para fora, link para pasta de fora, laço
        // A pasta pedida por um link também vale: a base é o canônico.
        let elo = fora.path().join("elo");
        symlink(p.path(), &elo).unwrap();
        assert_eq!(nomes(&listar(&elo, &LIMITES).unwrap()).len(), 2);
    }

    fn pasta_com(versoes: &[&str]) -> tempfile::TempDir {
        let p = tempfile::tempdir().unwrap();
        for (i, c) in versoes.iter().enumerate() {
            escrever(p.path(), &format!("2024-01-{:02}.jsonl", i + 1), c);
        }
        p
    }

    #[test]
    fn percorre_comparando_com_a_anterior() {
        let p = pasta_com(&[
            "{\"a\":1}\n",
            "{\"a\":1,\"b\":2}\n",
            "nada de json\n",
            "{\"b\":2}\n",
        ]);
        // Arquivo que não abre (removido depois de listar) vira passo com erro e é pulado.
        let l = listar(p.path(), &LIMITES).unwrap();
        fs::remove_file(p.path().join("2024-01-04.jsonl")).unwrap();
        let mut visto = Vec::new();
        let r = percorrer_com(l, &LIMITES, ABRIR, &AtomicBool::new(false), |f, t| {
            visto.push((f, t))
        });
        assert_eq!(r.parada, None);
        assert_eq!(visto, [(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
        let resumo: Vec<_> = r
            .passos
            .iter()
            .map(|s| {
                (
                    s.contra,
                    s.quebras,
                    s.compativeis,
                    s.invalidas,
                    s.erro.is_some(),
                )
            })
            .collect();
        assert_eq!(
            resumo,
            [
                (None, 0, 0, 0, false),
                (Some(0), 1, 0, 0, false), // b novo em todo registro: obrigatório, quebra
                (Some(1), 1, 0, 1, false), // nenhum registro válido: a raiz deixa de ser objeto
                (None, 0, 0, 0, true),
            ]
        );
        assert_eq!(r.passos[1].mudancas[0].caminho, ".b");
    }

    #[test]
    fn erro_no_meio_compara_com_a_ultima_que_abriu() {
        let p = pasta_com(&["{\"a\":1}\n", "{\"a\":1}\n", "{\"a\":\"x\"}\n"]);
        let l = listar(p.path(), &LIMITES).unwrap();
        fs::remove_file(p.path().join("2024-01-02.jsonl")).unwrap();
        let r = percorrer_com(l, &LIMITES, ABRIR, &AtomicBool::new(false), |_, _| {});
        assert!(r.passos[1].erro.is_some());
        assert_eq!(r.passos[2].contra, Some(0));
        assert_eq!(r.passos[2].quebras, 1);
    }

    #[test]
    fn teto_de_mudancas_por_passo() {
        let campos = |n: usize| {
            let c: Vec<String> = (0..n).map(|i| format!("\"c{i}\":1")).collect();
            format!("{{{}}}\n", c.join(","))
        };
        let p = pasta_com(&[&campos(0), &campos(30)]);
        let lim = Limites {
            max_mudancas: 20,
            ..LIMITES
        };
        let l = listar(p.path(), &lim).unwrap();
        let r = percorrer_com(l, &lim, ABRIR, &AtomicBool::new(false), |_, _| {});
        let s = &r.passos[1];
        assert_eq!((s.mudancas.len(), s.mais, s.quebras), (20, 10, 30));
        // O padrão guarda 200.
        assert_eq!(LIMITES.max_mudancas, 200);
    }

    #[test]
    fn prazo_e_cancelamento() {
        let p = pasta_com(&["{\"a\":1}\n", "{\"a\":2}\n", "{\"a\":3}\n"]);
        let lim = Limites {
            prazo: Duration::ZERO,
            ..LIMITES
        };
        let r = percorrer_com(
            listar(p.path(), &lim).unwrap(),
            &lim,
            ABRIR,
            &AtomicBool::new(false),
            |_, _| {},
        );
        assert_eq!((r.parada, r.passos.len()), (Some(Parada::Prazo), 0));
        // Cancelado durante a abertura de um arquivo grande (64 blocos da inferência): para no
        // meio dela, e a versão não entra.
        escrever(p.path(), "2024-01-02.jsonl", &"{\"a\":2}\n".repeat(2 << 20));
        let cancelar = AtomicBool::new(false);
        let mut abertos = 0;
        let r = percorrer_com(
            listar(p.path(), &LIMITES).unwrap(),
            &LIMITES,
            |c| {
                abertos += 1;
                if abertos == 2 {
                    cancelar.store(true, Relaxed);
                }
                ArquivoLinhas::abrir(c)
            },
            &cancelar,
            |_, _| {},
        );
        assert_eq!((r.parada, r.passos.len()), (Some(Parada::Cancelado), 1));
        let r = percorrer_com(
            listar(p.path(), &LIMITES).unwrap(),
            &LIMITES,
            ABRIR,
            &AtomicBool::new(true),
            |_, _| {},
        );
        assert_eq!((r.parada, r.passos.len()), (Some(Parada::Cancelado), 0));
    }
}
