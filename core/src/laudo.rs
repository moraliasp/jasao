//! Laudo do arquivo: o que ele é, a forma dele e o que merece atenção, em poucas linhas.
//!
//! Não lê o arquivo de novo: sai da inferência (`estrutura::inferir`, com os avisos) e do
//! `resumo::Conteudo` que a abertura já calcula. O custo é proporcional ao número de caminhos.
//! O tamanho do laudo é fixo, venha o arquivo que vier: até `MAX_VARIANTES` variantes,
//! `MAX_OPCIONAIS` campos opcionais e `MAX_ATENCAO` avisos, e cada texto vindo do arquivo é
//! cortado em `MAX_TEXTO` caracteres. Valor de caminho com aviso de segredo nunca entra: o laudo
//! não guarda valor nenhum de campo, só caminhos, valores de discriminador e contagens.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde::Serialize;

use crate::ArquivoLinhas;
use crate::estrutura::{self, Aviso, Inferencia, Relacao, Seg};
use crate::resumo::{self, Conteudo};

pub const MAX_VARIANTES: usize = 3;
pub const MAX_OPCIONAIS: usize = 5;
pub const MAX_ATENCAO: usize = 8;
/// Caracteres de caminho ou valor do arquivo; acima disso, cortado com `…`.
pub const MAX_TEXTO: usize = 120;

/// Ordem em que os avisos aparecem: o que expõe dado primeiro.
const PRIORIDADE: [Aviso; 5] = [
    Aviso::Segredo,
    Aviso::Opaco,
    Aviso::NumeroComoTexto,
    Aviso::DataForaIso,
    Aviso::ValorUnico,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Laudo {
    pub conteudo: Conteudo,
    pub bytes: u64,
    pub registros: u64,
    /// Linhas lidas de `total`: menos que `total` é laudo parcial (teto de tempo, cancelado ou
    /// JSON único interrompido por erro).
    pub varridas: u64,
    pub total: u64,
    pub teto_estourado: bool,
    pub invalidas: u64,
    /// Base 0.
    pub primeira_invalida: Option<u64>,
    pub vazias: u64,
    pub discriminador: Option<Forma>,
    /// Campos do topo que faltam em algum registro-objeto, na ordem do arquivo.
    pub opcionais: Vec<Opcional>,
    pub opcionais_mais: usize,
    pub atencao: Vec<Atencao>,
    pub atencao_mais: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Forma {
    pub campo: String,
    pub variantes: Vec<Variante>,
    pub mais: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Variante {
    /// Valor como está no arquivo (escapes intactos), cortado.
    pub valor: String,
    pub registros: u64,
    /// Registro da 1ª ocorrência, base 0.
    pub linha: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Opcional {
    pub caminho: String,
    /// Posição em `Resumo::caminhos()` (a mesma do modo Estrutura e do `perfil`).
    pub indice: usize,
    pub presenca: u64,
    pub de: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Atencao {
    /// `Aviso::nome`.
    pub aviso: &'static str,
    pub caminho: String,
    pub indice: usize,
}

impl Laudo {
    /// Laudo de uma inferência já feita (é o que o app usa depois da varredura de `abrir`).
    pub fn de(conteudo: Conteudo, bytes: u64, inf: &Inferencia) -> Laudo {
        let r = &inf.resumo;
        let caminhos = r.caminhos();
        let discriminador = r.discriminadores().into_iter().next().map(|d| {
            let mut vs = d.variantes;
            // Mais registros primeiro; no empate, a que aparece antes.
            vs.sort_by_key(|v| (std::cmp::Reverse(v.registros.n), v.registros.primeira()));
            Forma {
                campo: cortar(&d.campo),
                mais: vs.len().saturating_sub(MAX_VARIANTES),
                variantes: vs
                    .iter()
                    .take(MAX_VARIANTES)
                    .map(|v| Variante {
                        valor: cortar(&v.valor),
                        registros: v.registros.n,
                        linha: v.registros.primeira(),
                    })
                    .collect(),
            }
        });
        let mut opcionais = caminhos.iter().enumerate().filter(|(_, c)| {
            c.relacao == Relacao::Campo
                && matches!(c.segs.as_slice(), [Seg::Campo(_)])
                && c.presenca < c.de
        });
        let lista: Vec<Opcional> = opcionais
            .by_ref()
            .take(MAX_OPCIONAIS)
            .map(|(indice, c)| Opcional {
                caminho: cortar(&c.caminho),
                indice,
                presenca: c.presenca,
                de: c.de,
            })
            .collect();
        let opcionais_mais = opcionais.count();
        // Só (aviso, índice) até cortar: 10 mil caminhos com aviso não viram 10 mil `String`.
        let mut avisos: Vec<(usize, usize)> = caminhos
            .iter()
            .enumerate()
            .flat_map(|(i, c)| {
                c.avisos
                    .iter()
                    .filter_map(move |a| PRIORIDADE.iter().position(|p| p == a).map(|p| (p, i)))
            })
            .collect();
        avisos.sort_unstable();
        let atencao_mais = avisos.len().saturating_sub(MAX_ATENCAO);
        let atencao = avisos
            .into_iter()
            .take(MAX_ATENCAO)
            .map(|(p, i)| Atencao {
                aviso: PRIORIDADE[p].nome(),
                caminho: cortar(&caminhos[i].caminho),
                indice: i,
            })
            .collect();
        Laudo {
            conteudo,
            bytes,
            registros: r.registros(),
            varridas: inf.varridas,
            total: inf.total,
            teto_estourado: false,
            invalidas: r.invalidas.n,
            primeira_invalida: r.invalidas.primeira(),
            vazias: r.vazias,
            discriminador,
            opcionais: lista,
            opcionais_mais,
            atencao,
            atencao_mais,
        }
    }

    pub fn parcial(&self) -> bool {
        self.varridas < self.total
    }
}

/// Infere e faz o laudo, parando a inferência quando passa de `teto` (o laudo sai parcial e diz
/// quantas linhas leu). É o caminho da CLI.
pub fn gerar(arquivo: &ArquivoLinhas, teto: Duration) -> Laudo {
    let (parar, estourou) = (&AtomicBool::new(false), &AtomicBool::new(false));
    let (fim, espera) = mpsc::channel::<()>();
    let inf = thread::scope(|s| {
        s.spawn(move || {
            // Fim antes do teto chega como `Disconnected`, sem esperar.
            if let Err(mpsc::RecvTimeoutError::Timeout) = espera.recv_timeout(teto) {
                estourou.store(true, Relaxed);
                parar.store(true, Relaxed);
            }
        });
        let inf = estrutura::inferir(arquivo.bytes(), parar, |_| {});
        drop(fim);
        inf
    });
    let mut l = Laudo::de(
        resumo::conteudo(arquivo),
        arquivo.bytes().len() as u64,
        &inf,
    );
    l.teto_estourado = estourou.load(Relaxed) && l.parcial();
    l
}

fn cortar(s: &str) -> String {
    match s.char_indices().nth(MAX_TEXTO) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// 1234567 → "1.234.567".
fn milhar(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

fn tamanho(b: u64) -> String {
    const UNIDADES: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let mut x = b as f64 / 1024.0;
    let mut u = 0;
    while x >= 1024.0 && u < UNIDADES.len() - 1 {
        x /= 1024.0;
        u += 1;
    }
    format!("{x:.1} {}", UNIDADES[u]).replace('.', ",")
}

/// Trecho de código Markdown que aguenta crase no texto.
fn codigo(s: &str) -> String {
    let mut maior = 0;
    let mut atual = 0;
    for c in s.chars() {
        atual = if c == '`' { atual + 1 } else { 0 };
        maior = maior.max(atual);
    }
    let cerca = "`".repeat(maior + 1);
    let folga = if maior > 0 { " " } else { "" };
    format!("{cerca}{folga}{s}{folga}{cerca}")
}

fn conteudo_pt(c: Conteudo) -> &'static str {
    match c {
        Conteudo::Vazio => "arquivo vazio",
        Conteudo::JsonUnico => "JSON único",
        Conteudo::JsonVariasLinhas => "JSON em várias linhas",
        Conteudo::Objetos => "JSON Lines de objetos",
        Conteudo::Valores => "JSON Lines de valores",
        Conteudo::Misto => "JSON Lines misto",
    }
}

pub fn aviso_pt(nome: &str) -> &'static str {
    match nome {
        "segredo" => "parece segredo (o valor não é mostrado)",
        "opaco" => "valor opaco (JSON dentro de texto ou base64)",
        "numero_como_texto" => "número guardado como texto",
        "data_fora_iso" => "data fora do ISO 8601",
        "valor_unico" => "sempre o mesmo valor",
        _ => "aviso",
    }
}

fn plural(n: u64, um: &str, varios: &str) -> String {
    format!("{} {}", milhar(n), if n == 1 { um } else { varios })
}

/// O laudo em Markdown (pt-BR). `nome` é como o arquivo aparece no título e nos comandos.
pub fn markdown(l: &Laudo, nome: &str) -> String {
    let mut m = String::new();
    let _ = writeln!(m, "# Laudo de {}\n", codigo(nome));
    let _ = writeln!(
        m,
        "**O que é:** {} · {} · {}.",
        conteudo_pt(l.conteudo),
        tamanho(l.bytes),
        plural(l.registros, "registro", "registros"),
    );
    if l.parcial() {
        let por = if l.teto_estourado {
            " (o teto de tempo parou a leitura)"
        } else {
            ""
        };
        let _ = writeln!(
            m,
            "\n**Parcial:** {} de {} linhas lidas{por}; os números abaixo são só dessas linhas.",
            milhar(l.varridas),
            milhar(l.total),
        );
    }

    let _ = writeln!(m, "\n## Forma\n");
    // Sem registro lido não há base para afirmar ausência (de formatos, opcionais ou avisos).
    let sem_base = l.registros == 0;
    match &l.discriminador {
        Some(f) => {
            let n = (f.variantes.len() + f.mais) as u64;
            let vs: Vec<String> = f
                .variantes
                .iter()
                .map(|v| format!("{} ({})", codigo(&v.valor), milhar(v.registros)))
                .collect();
            let mais = if f.mais > 0 {
                format!(" e mais {}", f.mais)
            } else {
                String::new()
            };
            let _ = writeln!(
                m,
                "- {} por {}: {}{mais}.",
                plural(n, "formato", "formatos"),
                codigo(&f.campo),
                vs.join(", "),
            );
        }
        None if sem_base => {
            let _ = writeln!(m, "- Nenhum registro lido: sem base para dizer a forma.");
        }
        None => {
            let _ = writeln!(
                m,
                "- Um formato só (nenhum campo separa os registros em tipos)."
            );
        }
    }
    if l.opcionais.is_empty() && sem_base {
    } else if l.opcionais.is_empty() {
        let _ = writeln!(m, "- Nenhum campo do topo falta em algum registro.");
    } else {
        for o in &l.opcionais {
            let _ = writeln!(
                m,
                "- {} falta em {} de {} registros.",
                codigo(&o.caminho),
                milhar(o.de - o.presenca),
                milhar(o.de),
            );
        }
        if l.opcionais_mais > 0 {
            let _ = writeln!(m, "- E mais {} campos opcionais.", l.opcionais_mais);
        }
    }

    let _ = writeln!(m, "\n## Atenção\n");
    let vazio = l.invalidas == 0 && l.vazias == 0 && l.atencao.is_empty();
    if l.invalidas > 0 {
        let primeira = l
            .primeira_invalida
            .map(|p| format!(" (a 1ª na linha {})", milhar(p + 1)))
            .unwrap_or_default();
        let _ = writeln!(
            m,
            "- {}{primeira}.",
            plural(l.invalidas, "linha inválida", "linhas inválidas")
        );
    }
    if l.vazias > 0 {
        let _ = writeln!(
            m,
            "- {}.",
            plural(l.vazias, "linha em branco", "linhas em branco")
        );
    }
    for a in &l.atencao {
        let _ = writeln!(m, "- {}: {}.", codigo(&a.caminho), aviso_pt(a.aviso));
    }
    if l.atencao_mais > 0 {
        let _ = writeln!(
            m,
            "- E mais {} avisos (`jasao inferir` lista todos).",
            l.atencao_mais
        );
    }
    if sem_base && l.atencao.is_empty() {
        let _ = writeln!(m, "- Nenhum registro lido: sem base para avisos de campo.");
    } else if vazio {
        let _ = writeln!(m, "- Nenhum aviso.");
    }

    let _ = writeln!(m, "\n## Próximos passos\n");
    let arg = codigo_arg(nome);
    let _ = writeln!(
        m,
        "- `jasao inferir {arg}`: estrutura por caminho, com presença e avisos."
    );
    let _ = writeln!(
        m,
        "- `jasao exportar --schema {arg}`: JSON Schema conferido contra o arquivo."
    );
    if l.invalidas > 0 {
        let _ = writeln!(
            m,
            "- `jasao erros {arg}`: posição e motivo de cada linha inválida."
        );
    }
    m
}

/// Nome do arquivo como argumento de shell dentro de um trecho de código: entre aspas simples
/// se tiver algo além de letras e números ASCII e `._-/+`. Crase vira `'` para não fechar o trecho.
fn codigo_arg(nome: &str) -> String {
    let simples = nome
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._-/+_".contains(c));
    let nome = nome.replace('`', "'");
    if simples {
        nome
    } else {
        format!("'{}'", nome.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn laudo(dados: &str) -> Laudo {
        let inf = estrutura::inferir(dados.as_bytes(), &AtomicBool::new(false), |_| {});
        Laudo::de(Conteudo::Objetos, dados.len() as u64, &inf)
    }

    fn linhas(n: usize, f: impl Fn(usize) -> String) -> String {
        (0..n).map(|i| f(i) + "\n").collect()
    }

    #[test]
    fn o_que_e_e_invalidas() {
        let l = laudo("{\"a\":1}\n\n{quebrado\n{\"a\":2}\nnem\n");
        assert_eq!(l.registros, 2);
        assert_eq!(l.invalidas, 2);
        assert_eq!(l.primeira_invalida, Some(2));
        assert_eq!(l.vazias, 1);
        assert!(!l.parcial());
        let m = markdown(&l, "x.jsonl");
        assert!(m.contains("JSON Lines de objetos · "), "{m}");
        assert!(m.contains("2 registros"), "{m}");
        assert!(m.contains("2 linhas inválidas (a 1ª na linha 3)."), "{m}");
        assert!(m.contains("1 linha em branco."), "{m}");
        assert!(m.contains("`jasao erros x.jsonl`"), "{m}");
        assert!(!m.contains("Nenhum aviso"), "{m}");
    }

    #[test]
    fn sem_aviso_diz_nenhum() {
        let l = laudo("{\"a\":1}\n{\"a\":2}\n");
        let m = markdown(&l, "x");
        assert!(m.contains("- Nenhum aviso."), "{m}");
        assert!(m.contains("Nenhum campo do topo falta"), "{m}");
        assert!(m.contains("Um formato só"), "{m}");
        assert!(!m.contains("jasao erros"), "{m}");
    }

    #[test]
    fn variantes_por_registros_com_teto() {
        // 5 tipos com contagens 1..5 e uma chave exclusiva por tipo (vira discriminador).
        let mut d = String::new();
        for t in 0..5 {
            for _ in 0..=t {
                d += &format!("{{\"tipo\":\"t{t}\",\"k{t}\":1}}\n");
            }
        }
        let l = laudo(&d);
        let f = l.discriminador.as_ref().expect("discriminador");
        assert_eq!(f.campo, ".tipo");
        let vs: Vec<(&str, u64)> = f
            .variantes
            .iter()
            .map(|v| (v.valor.as_str(), v.registros))
            .collect();
        assert_eq!(vs, [("t4", 5), ("t3", 4), ("t2", 3)]);
        assert_eq!(f.mais, 2);
        assert_eq!(f.variantes[0].linha, Some(10));
        let m = markdown(&l, "x");
        assert!(
            m.contains("- 5 formatos por `.tipo`: `t4` (5), `t3` (4), `t2` (3) e mais 2."),
            "{m}"
        );
    }

    #[test]
    fn opcionais_so_do_topo_com_teto() {
        // 7 campos do topo opcionais (k0..k6) e um aninhado opcional, que não entra.
        let d = linhas(4, |i| {
            if i == 0 {
                "{\"o\":{\"x\":1},\"id\":0}".into()
            } else {
                let ks: String = (0..7).map(|k| format!(",\"k{k}\":1")).collect();
                format!("{{\"o\":{{}},\"id\":{i}{ks}}}")
            }
        });
        let l = laudo(&d);
        let cs: Vec<&str> = l.opcionais.iter().map(|o| o.caminho.as_str()).collect();
        assert_eq!(cs, [".k0", ".k1", ".k2", ".k3", ".k4"]);
        assert_eq!(l.opcionais_mais, 2);
        assert_eq!((l.opcionais[0].presenca, l.opcionais[0].de), (3, 4));
        let caminhos = estrutura::inferir(d.as_bytes(), &AtomicBool::new(false), |_| {})
            .resumo
            .caminhos();
        assert_eq!(caminhos[l.opcionais[0].indice].caminho, ".k0");
        let m = markdown(&l, "x");
        assert!(m.contains("- `.k0` falta em 1 de 4 registros."), "{m}");
        assert!(m.contains("- E mais 2 campos opcionais."), "{m}");
    }

    #[test]
    fn atencao_em_ordem_de_prioridade_com_teto() {
        let d = linhas(3, |i| {
            format!(
                "{{\"fixo\":\"igual\",\"valor\":\"1{i}.5\",\"quando\":\"2{i}/11/2026\",\"api_key\":\"k{i}\"}}"
            )
        });
        let l = laudo(&d);
        let a: Vec<(&str, &str)> = l
            .atencao
            .iter()
            .map(|a| (a.aviso, a.caminho.as_str()))
            .collect();
        assert_eq!(
            a,
            [
                ("segredo", ".api_key"),
                ("numero_como_texto", ".valor"),
                ("data_fora_iso", ".quando"),
                ("valor_unico", ".fixo"),
            ]
        );
        assert_eq!(l.atencao_mais, 0);
    }

    #[test]
    fn segredo_so_o_caminho_nunca_o_valor() {
        let token = "ghp_0123456789abcdefghijABCDEFGHIJ0123";
        let d = linhas(2, |_| {
            format!("{{\"chave\":\"{token}\",\"senha\":\"hunter2\"}}")
        });
        let l = laudo(&d);
        assert!(
            l.atencao
                .iter()
                .any(|a| a.aviso == "segredo" && a.caminho == ".chave")
        );
        assert!(
            l.atencao
                .iter()
                .any(|a| a.aviso == "segredo" && a.caminho == ".senha")
        );
        let tudo = format!(
            "{}{l:?}{}",
            markdown(&l, "x"),
            serde_json::to_string(&l).unwrap()
        );
        assert!(!tudo.contains("ghp_0123"), "{tudo}");
        assert!(!tudo.contains("hunter2"), "{tudo}");
    }

    #[test]
    fn segredo_com_valores_distintos_nao_vira_discriminador() {
        // Dois valores distintos: antes o campo virava discriminador e os valores saíam.
        let d = "{\"token\":\"hunter2xyz\",\"a\":1}\n{\"token\":\"s3cr3tvalue\",\"b\":2}\n";
        let l = laudo(d);
        assert!(l.discriminador.is_none(), "{:?}", l.discriminador);
        assert!(
            l.atencao
                .iter()
                .any(|a| a.aviso == "segredo" && a.caminho == ".token")
        );
        let tudo = format!(
            "{}{l:?}{}",
            markdown(&l, "x"),
            serde_json::to_string(&l).unwrap()
        );
        assert!(
            !tudo.contains("hunter2xyz") && !tudo.contains("s3cr3tvalue"),
            "{tudo}"
        );
    }

    #[test]
    fn sem_registro_nao_afirma_ausencia() {
        let md = markdown(&laudo(""), "x");
        assert!(md.contains("sem base para dizer a forma"), "{md}");
        assert!(md.contains("sem base para avisos"), "{md}");
        for falso in ["Um formato só", "Nenhum campo do topo", "Nenhum aviso"] {
            assert!(!md.contains(falso), "{falso}: {md}");
        }
        // Cancelado antes de ler (o que o teto zero faz): 0 de 2 linhas, também sem afirmar ausência.
        let d = "{\"a\":1}\n{\"b\":2}\n";
        let inf = estrutura::inferir(d.as_bytes(), &AtomicBool::new(true), |_| {});
        let l = Laudo::de(Conteudo::Objetos, d.len() as u64, &inf);
        assert_eq!((l.registros, l.varridas, l.total), (0, 0, 2));
        let md = markdown(&l, "x");
        assert!(
            md.contains("**Parcial:**") && md.contains("sem base para dizer a forma"),
            "{md}"
        );
        assert!(
            !md.contains("Um formato só") && !md.contains("Nenhum aviso"),
            "{md}"
        );
    }

    #[test]
    fn novecentos_avisos_cabem_no_teto() {
        // Abaixo de `LIMITE_CHAVES`: acima dele o topo vira mapa e os campos somem num `.*`.
        let ks: String = (0..900).map(|k| format!("\"c{k}_token\":\"x\",")).collect();
        let d = linhas(2, |_| format!("{{{ks}\"fim\":1}}"));
        let l = laudo(&d);
        assert_eq!(l.atencao.len(), MAX_ATENCAO);
        // Cada `cK_token` dá segredo e valor único ("x" nos 2 registros); `.fim` dá valor único.
        assert_eq!(l.atencao_mais, 2 * 900 + 1 - MAX_ATENCAO);
        assert!(l.atencao.iter().all(|a| a.aviso == "segredo"));
        assert!(markdown(&l, "x").len() < 4_000);
    }

    #[test]
    fn texto_do_arquivo_cortado() {
        let longa = "a".repeat(500);
        let d = linhas(2, |i| format!("{{\"{longa}\":{i},\"t\":\"{longa}\"}}"));
        let d = d + "{\"t\":\"x\"}\n";
        let l = laudo(&d);
        let c = &l.opcionais[0].caminho;
        assert_eq!(c.chars().count(), MAX_TEXTO + 1);
        assert!(c.ends_with('…'));
    }

    #[test]
    fn crase_no_caminho_nao_quebra_o_markdown() {
        assert_eq!(codigo("a`b"), "`` a`b ``");
        assert_eq!(codigo("ab"), "`ab`");
        assert_eq!(codigo_arg("meu arquivo's.json"), r"'meu arquivo'\''s.json'");
        assert_eq!(codigo_arg("a`b"), r"'a'\''b'");
        assert_eq!(codigo_arg("dir/x-1.jsonl"), "dir/x-1.jsonl");
    }

    #[test]
    fn numeros_em_pt() {
        assert_eq!(milhar(0), "0");
        assert_eq!(milhar(999), "999");
        assert_eq!(milhar(1000), "1.000");
        assert_eq!(milhar(6_456_439), "6.456.439");
        assert_eq!(tamanho(500), "500 B");
        assert_eq!(tamanho(1536), "1,5 KB");
        assert_eq!(tamanho(1_073_741_850), "1,0 GB");
    }

    #[test]
    fn cancelado_sai_parcial_e_diz_quanto_leu() {
        let d = linhas(10_000, |i| format!("{{\"a\":{i}}}"));
        let inf = estrutura::inferir_em_blocos(d.as_bytes(), 64, &AtomicBool::new(true), |_| {});
        let l = Laudo::de(Conteudo::Objetos, d.len() as u64, &inf);
        assert!(l.parcial());
        let m = markdown(&l, "x");
        assert!(
            m.contains(&format!("de {} linhas lidas", milhar(l.total))),
            "{m}"
        );
        assert!(!m.contains("teto de tempo"), "{m}");
    }

    fn arquivo(d: &str) -> (tempfile::NamedTempFile, ArquivoLinhas) {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(d.as_bytes()).unwrap();
        let a = ArquivoLinhas::abrir(f.path()).unwrap();
        (f, a)
    }

    #[test]
    fn teto_de_tempo_para_a_leitura() {
        // ~40 MB: a inferência leva bem mais que 1 ms, mesmo em release.
        let d = linhas(400_000, |i| {
            format!(
                "{{\"id\":{i},\"nome\":\"registro número {i} com folga de texto\",\"v\":[1,2,3]}}"
            )
        });
        let (_f, a) = arquivo(&d);
        let l = gerar(&a, Duration::from_millis(1));
        assert!(
            l.teto_estourado && l.parcial(),
            "{} de {}",
            l.varridas,
            l.total
        );
        assert!(markdown(&l, "x").contains("o teto de tempo parou a leitura"));
    }

    #[test]
    fn dentro_do_teto_le_tudo() {
        let (_f, a) = arquivo("{\"a\":1}\n{\"a\":2}\n");
        let l = gerar(&a, Duration::from_secs(30));
        assert!(!l.teto_estourado && !l.parcial());
        assert_eq!((l.registros, l.conteudo), (2, Conteudo::Objetos));
    }
}
