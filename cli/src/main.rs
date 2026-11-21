//! Comandos (o arquivo pode ser JSONL cru, gzip ou zstd):
//!   linhas <arquivo>                      conta as linhas e mostra o tempo do índice
//!   janela <arquivo> <inicio> <quantidade> imprime as linhas cruas pedidas
//!   buscar <arquivo> <padrão> [--regex] [--ignorar-caixa] [--max N]
//!                                         imprime linha:coluna (em bytes, a partir de 1) e trecho
//!   inferir <arquivo> [--json]            estrutura por caminho (linhas a partir de 1)

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Instant;

use std::sync::atomic::AtomicBool;

use nucleo::{ArquivoLinhas, Busca, Opcoes};

const USO: &str = "uso:\n  linhas <arquivo>\n  janela <arquivo> <inicio> <quantidade>\n  \
                   buscar <arquivo> <padrão> [--regex] [--ignorar-caixa] [--max N]\n  \
                   inferir <arquivo> [--json]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let resultado = match args.as_slice() {
        ["linhas", arquivo] => linhas(arquivo),
        ["janela", arquivo, inicio, quantidade] => match (inicio.parse(), quantidade.parse()) {
            (Ok(i), Ok(q)) => janela(arquivo, i, q),
            _ => Err("início e quantidade precisam ser números inteiros".into()),
        },
        ["buscar", resto @ ..] => match opcoes_busca(resto) {
            Some(op) => buscar(op),
            None => {
                eprintln!("{USO}");
                return ExitCode::from(2);
            }
        },
        ["inferir", arquivo] => inferir(arquivo, false),
        ["inferir", arquivo, "--json"] | ["inferir", "--json", arquivo] => inferir(arquivo, true),
        _ => {
            eprintln!("{USO}");
            return ExitCode::from(2);
        }
    };
    match resultado {
        Ok(()) => ExitCode::SUCCESS,
        // Pipe fechado (ex.: `| head`) não é erro.
        Err(e)
            if e.downcast_ref::<io::Error>().map(io::Error::kind)
                == Some(io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

type Resultado = Result<(), Box<dyn std::error::Error>>;

fn abrir(arquivo: &str) -> Result<ArquivoLinhas, Box<dyn std::error::Error>> {
    // Progresso só aparece para compactado (o núcleo só chama ao descompactar), em stderr.
    let mostrou = std::cell::Cell::new(false);
    let progresso = |lidos: u64, total: u64| {
        mostrou.set(true);
        let pct = lidos * 100 / total.max(1);
        eprint!("\rdescompactando… {pct}%");
    };
    let op = Opcoes {
        progresso: Some(&progresso),
        ..Opcoes::default()
    };
    let r = ArquivoLinhas::abrir_com(arquivo, &op);
    if mostrou.get() {
        eprintln!();
    }
    r.map_err(|e| format!("{arquivo}: {e}").into())
}

fn linhas(arquivo: &str) -> Resultado {
    let t = Instant::now();
    let a = abrir(arquivo)?;
    let dt = t.elapsed();
    let mb = a.bytes().len() as f64 / 1e6;
    println!("{} linhas · {mb:.1} MB · índice em {dt:.2?}", a.linhas());
    Ok(())
}

fn janela(arquivo: &str, inicio: usize, quantidade: usize) -> Resultado {
    let a = abrir(arquivo)?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    for linha in a.janela(inicio, quantidade) {
        out.write_all(linha)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    Ok(())
}

struct OpcoesBusca<'a> {
    arquivo: &'a str,
    padrao: &'a str,
    regex: bool,
    ignorar_caixa: bool,
    max: usize,
}

fn opcoes_busca<'a>(args: &[&'a str]) -> Option<OpcoesBusca<'a>> {
    let (mut posicionais, mut regex, mut ignorar_caixa, mut max) = (Vec::new(), false, false, 100);
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--regex" => regex = true,
            "--ignorar-caixa" => ignorar_caixa = true,
            "--max" => max = it.next()?.parse().ok()?,
            _ => posicionais.push(a),
        }
    }
    let [arquivo, padrao] = posicionais[..] else {
        return None;
    };
    Some(OpcoesBusca {
        arquivo,
        padrao,
        regex,
        ignorar_caixa,
        max,
    })
}

/// Bytes de contexto de cada lado do achado no trecho impresso.
const CONTEXTO: usize = 40;

fn buscar(op: OpcoesBusca) -> Resultado {
    let busca = Busca::nova(op.padrao, op.regex, op.ignorar_caixa)?;
    let a = abrir(op.arquivo)?;
    let t = Instant::now();
    let mut out = io::BufWriter::new(io::stdout().lock());
    let (mut impressos, mut erro) = (0, None);
    let resumo = busca.executar(a.bytes(), &AtomicBool::new(false), |lote| {
        for achado in lote.achados {
            if impressos == op.max || erro.is_some() {
                // Segue varrendo para contar o total; só para de imprimir.
                return;
            }
            impressos += 1;
            let linha = a.janela(achado.linha, 1)[0];
            let ini = achado.coluna.saturating_sub(CONTEXTO);
            let fim_achado = achado.coluna + achado.comprimento;
            let fim = (fim_achado + CONTEXTO).min(linha.len());
            let r = writeln!(
                out,
                "{}:{}: {}{}[{}]{}{}",
                achado.linha + 1,
                achado.coluna + 1,
                if ini > 0 { "…" } else { "" },
                String::from_utf8_lossy(&linha[ini..achado.coluna]),
                String::from_utf8_lossy(&linha[achado.coluna..fim_achado]),
                String::from_utf8_lossy(&linha[fim_achado..fim]),
                if fim < linha.len() { "…" } else { "" },
            );
            if let Err(e) = r {
                erro = Some(e);
            }
        }
    });
    if let Some(e) = erro {
        return Err(e.into());
    }
    out.flush()?;
    eprintln!(
        "{} achados (mostrando {impressos}) · {:.1} MB em {:.2?}",
        resumo.achados,
        resumo.total as f64 / 1e6,
        t.elapsed()
    );
    Ok(())
}

fn inferir(arquivo: &str, json: bool) -> Resultado {
    use nucleo::estrutura::{self, Entrada, Relacao, Tipo};
    let a = abrir(arquivo)?;
    let t = Instant::now();
    let inf = estrutura::inferir(a.bytes(), &AtomicBool::new(false), |_| {});
    let dt = t.elapsed();
    let r = &inf.resumo;
    let caminhos = r.caminhos();
    let discriminadores = r.discriminadores();
    let linha = |c: nucleo::estrutura::Contagem| c.primeira().map(|l| l + 1);
    let mut out = io::BufWriter::new(io::stdout().lock());
    if json {
        let num = |x: Option<u64>| x.map_or("null".into(), |v| v.to_string());
        let lista = |v: Vec<String>| v.join(",");
        let entrada = match inf.entrada {
            Entrada::Jsonl => "jsonl",
            Entrada::JsonUnico => "json",
        };
        write!(
            out,
            r#"{{"entrada":"{entrada}","registros":{},"linhas":{{"varridas":{},"total":{}}},"cancelado":{},"vazias":{},"invalidas":{{"n":{},"primeira_linha":{}}},"caminhos":["#,
            r.registros(),
            inf.varridas,
            inf.total,
            inf.cancelado,
            r.vazias,
            r.invalidas.n,
            num(linha(r.invalidas)),
        )?;
        let itens = caminhos.iter().map(|c| {
            let relacao = match c.relacao {
                Relacao::Registro => "registro",
                Relacao::Campo => "campo",
                Relacao::Elemento => "elemento",
                Relacao::Entrada => "entrada",
            };
            let tipos = Tipo::TODOS
                .iter()
                .filter(|&&t| c.tipos[t as usize].n > 0)
                .map(|&t| {
                    let x = c.tipos[t as usize];
                    format!(
                        r#""{}":{{"n":{},"primeira_linha":{}}}"#,
                        t.nome(),
                        x.n,
                        num(linha(x))
                    )
                });
            format!(
                r#"{{"caminho":{},"relacao":"{relacao}","presenca":{},"de":{},"tipos":{{{}}}}}"#,
                texto_json(&c.caminho),
                c.presenca,
                c.de,
                lista(tipos.collect())
            )
        });
        write!(out, r#"{}],"discriminadores":["#, lista(itens.collect()))?;
        let ds = discriminadores.iter().map(|d| {
            let vs = d.variantes.iter().map(|v| {
                format!(
                    r#"{{"valor":{},"registros":{},"primeira_linha":{},"obrigatorias":[{}],"opcionais":[{}]}}"#,
                    texto_json(&v.valor),
                    v.registros.n,
                    num(linha(v.registros)),
                    lista(v.obrigatorias.iter().map(|k| texto_json(k)).collect()),
                    lista(
                        v.opcionais
                            .iter()
                            .map(|(k, n)| format!(r#"{{"chave":{},"presenca":{n}}}"#, texto_json(k)))
                            .collect()
                    ),
                )
            });
            format!(
                r#"{{"campo":{},"variantes":[{}]}}"#,
                texto_json(&d.campo),
                lista(vs.collect())
            )
        });
        writeln!(out, "{}]}}", lista(ds.collect()))?;
    } else {
        for c in &caminhos {
            let tipos: Vec<String> = Tipo::TODOS
                .iter()
                .filter(|&&t| c.tipos[t as usize].n > 0)
                .map(|&t| {
                    let x = c.tipos[t as usize];
                    format!("{} {} (l.{})", t.nome(), x.n, x.primeira().unwrap_or(0) + 1)
                })
                .collect();
            let pct = c.presenca as f64 * 100.0 / c.de.max(1) as f64;
            let presenca = match c.relacao {
                Relacao::Registro => String::new(),
                Relacao::Campo => format!("  · em {} de {} ({pct:.1}%)", c.presenca, c.de),
                Relacao::Elemento => format!("  · {} elementos em {} listas", c.presenca, c.de),
                Relacao::Entrada => {
                    format!("  · mapa: {} entradas em {} objetos", c.presenca, c.de)
                }
            };
            writeln!(
                out,
                "{:indent$}{}  {}{presenca}",
                "",
                c.caminho,
                tipos.join(", "),
                indent = 2 * c.profundidade
            )?;
        }
        for d in &discriminadores {
            writeln!(out, "\nunião discriminada por {}:", d.campo)?;
            for v in &d.variantes {
                let opc: Vec<String> = v
                    .opcionais
                    .iter()
                    .map(|(k, n)| format!("{k}?({n})"))
                    .collect();
                writeln!(
                    out,
                    "  \"{}\"  {} registros (l.{}): {}{}{}",
                    v.valor,
                    v.registros.n,
                    v.registros.primeira().unwrap_or(0) + 1,
                    v.obrigatorias.join(", "),
                    if opc.is_empty() { "" } else { "; opcionais: " },
                    opc.join(", ")
                )?;
            }
        }
    }
    out.flush()?;
    let entrada = match inf.entrada {
        Entrada::Jsonl => "JSONL",
        Entrada::JsonUnico => "JSON único",
    };
    let primeira = linha(r.invalidas).map_or(String::new(), |l| format!(" (1ª na linha {l})"));
    eprintln!(
        "{entrada} · {} registros · {} inválidas{primeira} · {} vazias · {} de {} linhas · {:.1} MB em {dt:.2?}",
        r.registros(),
        r.invalidas.n,
        r.vazias,
        inf.varridas,
        inf.total,
        a.bytes().len() as f64 / 1e6,
    );
    Ok(())
}

/// Texto como string JSON (aspas, barra e controles escapados).
fn texto_json(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
