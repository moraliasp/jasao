//! Comandos (o arquivo pode ser JSONL cru, gzip ou zstd):
//!   linhas <arquivo>                      conta as linhas e mostra o tempo do índice
//!   janela <arquivo> <inicio> <quantidade> imprime as linhas cruas pedidas
//!   buscar <arquivo> <padrão> [--regex] [--ignorar-caixa] [--max N]
//!                                         imprime linha:coluna (em bytes, a partir de 1) e trecho
//!   consultar <filtro> <arquivo> [--documento] [--tempo-max SEG]
//!                                         roda o filtro jq em cada linha (ou no documento inteiro);
//!                                         resultados em JSONL no stdout, erros por linha no stderr
//!   erros <arquivo> [--json] [--max N]    lista as linhas que não são JSON; sai com 1 se houver
//!   inferir <arquivo> [--json]            estrutura por caminho (linhas a partir de 1)
//!   exportar --zod|--schema [--literal] [--numero-texto manter|numero] <arquivo>
//!                                         Zod 4 ou JSON Schema 2020-12 no stdout; valida o
//!                                         arquivo contra ele e sai com 1 se algum registro falhar
//!
//! Com o stdout num terminal, `janela`, `buscar`, `consultar` e `inferir` trocam controles (C0
//! menos `\n`/`\t`, DEL, C1) e invisíveis bidi/de largura zero por `\u{..}` visível: um JSON
//! hostil não manda sequência ESC/CSI/OSC ao terminal nem inverte o texto. O mesmo vale para os
//! erros do jq no stderr, se ele for terminal. Redirecionada (arquivo, pipe), a saída é fiel byte
//! a byte. No terminal, o `\r` de um CRLF aparece como `\u{d}` e a saída `--json` deixa de ser
//! JSON válido onde houver escape (`\u{..}` não é escape JSON): para máquina, use pipe.

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use nucleo::caminho::texto_json;
use nucleo::consulta::Consulta;
use nucleo::erros::{self, OpcoesErros};
use nucleo::{ArquivoLinhas, Busca, Opcoes};

const USO: &str = "uso:\n  linhas <arquivo>\n  janela <arquivo> <inicio> <quantidade>\n  \
                   buscar <arquivo> <padrão> [--regex] [--ignorar-caixa] [--max N]\n  \
                   consultar <filtro> <arquivo> [--documento] [--tempo-max SEG]\n  \
                   erros <arquivo> [--json] [--max N]\n  \
                   inferir <arquivo> [--json]\n  \
                   exportar --zod|--schema [--literal] [--numero-texto manter|numero] <arquivo>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut saida = ExitCode::SUCCESS;
    let resultado = match args.as_slice() {
        ["linhas", arquivo] => linhas(arquivo),
        ["janela", arquivo, inicio, quantidade] => match (inicio.parse(), quantidade.parse()) {
            (Ok(i), Ok(q)) => janela(arquivo, i, q, &mut tela()),
            _ => Err("início e quantidade precisam ser números inteiros".into()),
        },
        ["buscar", resto @ ..] => match opcoes_busca(resto) {
            Some(op) => buscar(op, &mut tela()),
            None => {
                eprintln!("{USO}");
                return ExitCode::from(2);
            }
        },
        ["consultar", resto @ ..] => match opcoes_consulta(resto) {
            Some(op) => consultar(op, &mut tela(), &mut erro()),
            None => {
                eprintln!("{USO}");
                return ExitCode::from(2);
            }
        },
        ["erros", resto @ ..] => match opcoes_erros(resto) {
            Some((arquivo, json, max)) => erros(arquivo, json, max).map(|invalidas| {
                if invalidas {
                    saida = ExitCode::from(1);
                }
            }),
            None => {
                eprintln!("{USO}");
                return ExitCode::from(2);
            }
        },
        ["inferir", arquivo] => inferir(arquivo, false, &mut tela()),
        ["inferir", arquivo, "--json"] | ["inferir", "--json", arquivo] => {
            inferir(arquivo, true, &mut tela())
        }
        ["exportar", resto @ ..] => match opcoes_exportar(resto) {
            Some((arquivo, zod, op)) => exportar(arquivo, zod, op).map(|ok| {
                if !ok {
                    saida = ExitCode::from(1);
                }
            }),
            None => {
                eprintln!("{USO}");
                return ExitCode::from(2);
            }
        },
        _ => {
            eprintln!("{USO}");
            return ExitCode::from(2);
        }
    };
    match resultado {
        Ok(()) => saida,
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

/// Saída que, ligada, troca controles e invisíveis por `\u{..}` (ver o topo do arquivo).
struct Terminal<W: Write> {
    out: W,
    neutralizar: bool,
}

/// O stdout, neutralizado se for terminal. Os comandos que imprimem conteúdo do arquivo pedem um
/// `Terminal` no tipo: não há como passar o stdout cru por engano.
fn tela() -> Terminal<io::BufWriter<io::StdoutLock<'static>>> {
    let stdout = io::stdout();
    Terminal {
        neutralizar: stdout.is_terminal(),
        out: io::BufWriter::new(stdout.lock()),
    }
}

/// O stderr, neutralizado se for terminal: os erros do jq citam valores do arquivo.
fn erro() -> Terminal<io::StderrLock<'static>> {
    let stderr = io::stderr();
    Terminal {
        neutralizar: stderr.is_terminal(),
        out: stderr.lock(),
    }
}

/// Controle que mexe no terminal, ou bidi/largura zero que engana quem lê.
fn perigoso(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(c, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

impl<W: Write> Write for Terminal<W> {
    // Cada chamada traz um pedaço inteiro (linha ou argumento do `write!`); UTF-8 inválido vira
    // U+FFFD, que o terminal mostraria do mesmo jeito.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.neutralizar {
            return self.out.write(buf);
        }
        let texto = String::from_utf8_lossy(buf);
        let mut resto = &texto[..];
        while let Some(i) = resto.find(perigoso) {
            let c = resto[i..].chars().next().unwrap_or_default();
            self.out.write_all(&resto.as_bytes()[..i])?;
            write!(self.out, "\\u{{{:x}}}", c as u32)?;
            resto = &resto[i + c.len_utf8()..];
        }
        self.out.write_all(resto.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

fn janela(
    arquivo: &str,
    inicio: usize,
    quantidade: usize,
    out: &mut Terminal<impl Write>,
) -> Resultado {
    let a = abrir(arquivo)?;
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

fn buscar(op: OpcoesBusca, out: &mut Terminal<impl Write>) -> Resultado {
    let busca = Busca::nova(op.padrao, op.regex, op.ignorar_caixa)?;
    let a = abrir(op.arquivo)?;
    let t = Instant::now();
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

struct OpcoesConsulta<'a> {
    filtro: &'a str,
    arquivo: &'a str,
    documento: bool,
    tempo_max: Option<Duration>,
}

fn opcoes_consulta<'a>(args: &[&'a str]) -> Option<OpcoesConsulta<'a>> {
    let (mut posicionais, mut documento, mut tempo_max) = (Vec::new(), false, None);
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--documento" => documento = true,
            "--tempo-max" => {
                tempo_max = Some(Duration::try_from_secs_f64(it.next()?.parse().ok()?).ok()?)
            }
            _ => posicionais.push(a),
        }
    }
    let [filtro, arquivo] = posicionais[..] else {
        return None;
    };
    Some(OpcoesConsulta {
        filtro,
        arquivo,
        documento,
        tempo_max,
    })
}

/// Folga depois do teto de tempo antes de derrubar o processo: o núcleo só vê o teto entre
/// resultados, e filtro que não produz nada (`last(range(1e12))`, `def f: f; f`) nunca devolve.
const FOLGA_TEMPO: Duration = Duration::from_secs(2);

fn consultar(
    op: OpcoesConsulta,
    out: &mut Terminal<impl Write>,
    err: &mut Terminal<impl Write>,
) -> Resultado {
    let consulta = Consulta::nova(op.filtro)?.com_tempo_max(op.tempo_max);
    let a = abrir(op.arquivo)?;
    if let Some(t) = op.tempo_max {
        std::thread::spawn(move || {
            std::thread::sleep(t + FOLGA_TEMPO);
            eprintln!("erro: tempo máximo de {t:.1?} esgotado (filtro não respondeu)");
            std::process::exit(3);
        });
    }
    let t = Instant::now();
    let cancelar = AtomicBool::new(false);
    let mut erro = None;
    let ao_lote = |lote: nucleo::consulta::LoteConsulta| {
        for r in lote.resultados {
            if let Err(e) = writeln!(out, "{}", r.json) {
                erro = Some(e);
                cancelar.store(true, Relaxed);
                return;
            }
        }
    };
    let resumo = if op.documento {
        consulta.executar_documento(a.bytes(), &cancelar, ao_lote)
    } else {
        consulta.executar_jsonl(a.bytes(), &cancelar, ao_lote)
    };
    if let Some(e) = erro {
        return Err(e.into());
    }
    out.flush()?;
    for e in &resumo.primeiros_erros {
        writeln!(err, "linha {}: {}", e.linha + 1, e.motivo)?;
    }
    if resumo.erros > resumo.primeiros_erros.len() as u64 {
        writeln!(
            err,
            "… e mais {} erros",
            resumo.erros - resumo.primeiros_erros.len() as u64
        )?;
    }
    writeln!(
        err,
        "{} resultados · {} erros · {} de {} linhas · {:.1} MB em {:.2?}",
        resumo.resultados,
        resumo.erros,
        resumo.linhas,
        resumo.total_linhas,
        a.bytes().len() as f64 / 1e6,
        t.elapsed()
    )?;
    err.flush()?;
    if resumo.tempo_esgotado {
        return Err("tempo máximo esgotado; resultado parcial".into());
    }
    Ok(())
}

fn inferir(arquivo: &str, json: bool, out: &mut Terminal<impl Write>) -> Resultado {
    use nucleo::estrutura::{self, Entrada, Relacao, Tipo};
    let a = abrir(arquivo)?;
    let t = Instant::now();
    let inf = estrutura::inferir(a.bytes(), &AtomicBool::new(false), |_| {});
    let dt = t.elapsed();
    let r = &inf.resumo;
    let caminhos = r.caminhos();
    let discriminadores = r.discriminadores();
    let linha = |c: nucleo::estrutura::Contagem| c.primeira().map(|l| l + 1);
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

fn opcoes_exportar<'a>(args: &[&'a str]) -> Option<(&'a str, bool, nucleo::exportar::Opcoes)> {
    let (mut arquivo, mut zod, mut op) = (None, None, nucleo::exportar::Opcoes::default());
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--zod" if zod.is_none() => zod = Some(true),
            "--schema" if zod.is_none() => zod = Some(false),
            "--literal" => op.literal = true,
            "--numero-texto" => {
                op.numero = match *it.next()? {
                    "manter" => false,
                    "numero" => true,
                    _ => return None,
                }
            }
            _ if arquivo.is_none() && !a.starts_with("--") => arquivo = Some(a),
            _ => return None,
        }
    }
    Some((arquivo?, zod?, op))
}

/// Imprime o código e valida o arquivo contra o JSON Schema equivalente. `Ok(false)` se algum
/// registro falhar.
fn exportar(
    arquivo: &str,
    zod: bool,
    op: nucleo::exportar::Opcoes,
) -> Result<bool, Box<dyn std::error::Error>> {
    use nucleo::{estrutura, exportar, validar};
    let a = abrir(arquivo)?;
    let nada = AtomicBool::new(false);
    let inf = estrutura::inferir(a.bytes(), &nada, |_| {});
    let r = &inf.resumo;
    let (codigo, schema) = if zod {
        (exportar::zod(r, op), exportar::schema_da_entrada_zod(r, op))
    } else {
        let s = exportar::schema(r, op);
        (s.clone(), s)
    };
    // O código já sai com controles e bidi escapados (`exportar::aspas`): fiel também no terminal.
    let mut out = io::stdout().lock();
    out.write_all(codigo.as_bytes())?;
    out.flush()?;
    let t = Instant::now();
    let v = validar::validar(a.bytes(), inf.entrada, &schema, &nada, |_| {})?;
    let m = v.passam + v.falham;
    eprintln!(
        "{} de {m} registros passam · validado em {:.2?}",
        v.passam,
        t.elapsed()
    );
    if v.invalidas > 0 {
        eprintln!("{} linhas não são JSON para o validador", v.invalidas);
    }
    for f in &v.falhas {
        eprintln!("linha {}: {}", f.linha + 1, neutro(&f.motivo));
    }
    Ok(v.falham == 0 && v.invalidas == 0)
}

fn opcoes_erros<'a>(args: &[&'a str]) -> Option<(&'a str, bool, usize)> {
    let (mut arquivo, mut json, mut max) = (None, false, 100);
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--json" => json = true,
            "--max" => max = it.next()?.parse().ok()?,
            _ if arquivo.is_none() => arquivo = Some(a),
            _ => return None,
        }
    }
    Some((arquivo?, json, max))
}

/// Devolve se havia linha inválida. `--max` limita o que é guardado e impresso; as contagens
/// são sempre do arquivo inteiro.
fn erros(arquivo: &str, json: bool, max: usize) -> Result<bool, Box<dyn std::error::Error>> {
    let a = abrir(arquivo)?;
    let t = Instant::now();
    let op = OpcoesErros {
        max_guardados: max,
        ..OpcoesErros::default()
    };
    let mut lista = Vec::new();
    let r = erros::varrer(a.bytes(), &op, &AtomicBool::new(false), |l| {
        lista.extend(l.erros)
    });
    let dt = t.elapsed();
    let mut out = io::BufWriter::new(io::stdout().lock());
    if json {
        write!(
            out,
            "{{\"linhas\":{},\"invalidas\":{},\"em_branco\":{},\"erros\":[",
            r.linhas, r.invalidas, r.em_branco
        )?;
        for (i, e) in lista.iter().enumerate() {
            write!(
                out,
                "{}{{\"linha\":{},\"deslocamento\":{},\"coluna\":{},\"codigo\":\"{}\",\"motivo\":{}}}",
                if i > 0 { "," } else { "" },
                e.linha + 1,
                e.deslocamento,
                e.coluna + 1,
                e.motivo.codigo(),
                texto_json(&e.motivo.to_string()),
            )?;
        }
        writeln!(out, "]}}")?;
    } else {
        for e in &lista {
            writeln!(
                out,
                "{}:{} (byte {}): {}",
                e.linha + 1,
                e.coluna + 1,
                e.deslocamento + e.coluna,
                e.motivo
            )?;
        }
    }
    out.flush()?;
    eprintln!(
        "{} inválidas (mostrando {}) · {} em branco · {} linhas · {:.1} MB em {dt:.2?}",
        r.invalidas,
        lista.len(),
        r.em_branco,
        r.linhas,
        r.total as f64 / 1e6,
    );
    Ok(r.invalidas > 0)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn escrito(neutralizar: bool, partes: &[&[u8]]) -> Vec<u8> {
        let mut t = Terminal {
            out: Vec::new(),
            neutralizar,
        };
        for p in partes {
            t.write_all(p).unwrap();
        }
        t.out
    }

    #[test]
    fn terminal_neutraliza_escapes_controles_e_invisiveis() {
        // Título (OSC 0), limpar tela (CSI 2J), link OSC 8, CSI em C1, CR, DEL, RLO e ZWSP.
        let hostil = "{\"a\":\"\x1b]0;pwned\x07\x1b[2J\x1b]8;;http://x\x1b\\\\l\u{9b}31m\r\x7f\u{202e}fdp.exe\u{200b}\"}\tok";
        let s = String::from_utf8(escrito(true, &[hostil.as_bytes(), b"\n"])).unwrap();
        assert_eq!(
            s,
            "{\"a\":\"\\u{1b}]0;pwned\\u{7}\\u{1b}[2J\\u{1b}]8;;http://x\\u{1b}\\\\l\\u{9b}31m\\u{d}\\u{7f}\\u{202e}fdp.exe\\u{200b}\"}\tok\n"
        );
        assert!(!s.chars().any(perigoso));
    }

    /// Os 17 invisíveis bidi/de largura zero que a interface também marca (app/src/texto.ts).
    const INVISIVEIS: [char; 17] = [
        '\u{061c}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{200e}', '\u{200f}', '\u{202a}',
        '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2060}', '\u{2066}', '\u{2067}',
        '\u{2068}', '\u{2069}', '\u{feff}',
    ];

    #[test]
    fn perigoso_cobre_a_lista_inteira() {
        let controles = (0u32..0x20).chain(0x7f..0xa0).filter_map(char::from_u32);
        for c in controles.chain(INVISIVEIS) {
            let esperado = !matches!(c, '\n' | '\t');
            assert_eq!(perigoso(c), esperado, "U+{:04X}", c as u32);
            if esperado {
                assert_eq!(
                    escrito(true, &[format!("a{c}b").as_bytes()]),
                    format!("a\\u{{{:x}}}b", c as u32).as_bytes()
                );
            }
        }
        // Vizinhos que não entram: espaço, NBSP, U+200A, U+2010, U+202F, U+2065, U+206A.
        for c in [
            ' ', '\u{a0}', '\u{200a}', '\u{2010}', '\u{202f}', '\u{2065}', '\u{206a}',
        ] {
            assert!(!perigoso(c), "U+{:04X}", c as u32);
        }
    }

    /// Arquivo temporário com `conteudo`; apagado no fim do teste.
    struct Temporario(std::path::PathBuf);
    impl Temporario {
        fn novo(nome: &str, conteudo: &str) -> Self {
            let p = std::env::temp_dir().join(format!("jasao-{}-{nome}.jsonl", std::process::id()));
            std::fs::write(&p, conteudo).unwrap();
            Self(p)
        }
    }
    impl Drop for Temporario {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    // Linha 1: JSON válido com C1 e bidi crus. Linha 2: ESC/BEL crus (inválida; só a janela mostra).
    const HOSTIL: &str = "{\"k\":\"\u{9b}\u{202e} ok\"}\n{\"r\":\"\x1b]0;x\x07\"}\n";

    /// Roda o comando com um `Terminal` ligado e devolve o que ele escreveu.
    fn no_terminal(cmd: impl FnOnce(&mut Terminal<Vec<u8>>) -> Resultado) -> String {
        let mut t = Terminal {
            out: Vec::new(),
            neutralizar: true,
        };
        cmd(&mut t).unwrap();
        String::from_utf8(t.out).unwrap()
    }

    #[test]
    fn janela_e_consultar_passam_pelo_terminal() {
        let f = Temporario::novo("call-site", HOSTIL);
        let arquivo = f.0.to_str().unwrap();
        let jan = no_terminal(|t| janela(arquivo, 0, 2, t));
        assert_eq!(
            jan,
            "{\"k\":\"\\u{9b}\\u{202e} ok\"}\n{\"r\":\"\\u{1b}]0;x\\u{7}\"}\n"
        );
        let op = OpcoesConsulta {
            filtro: ".k",
            arquivo,
            documento: false,
            tempo_max: None,
        };
        let con = no_terminal(|t| {
            consultar(
                op,
                t,
                &mut Terminal {
                    out: io::sink(),
                    neutralizar: true,
                },
            )
        });
        // C1 e bidi são válidos crus em texto JSON e saíam assim do jq.
        assert!(con.contains("\\u{9b}\\u{202e}"), "{con:?}");
        assert!(!con.chars().any(perigoso), "{con:?}");
        let op = OpcoesBusca {
            arquivo,
            padrao: "ok",
            regex: false,
            ignorar_caixa: false,
            max: 10,
        };
        let bus = no_terminal(|t| buscar(op, t));
        assert!(
            bus.contains("\\u{9b}\\u{202e} [ok]") && !bus.chars().any(perigoso),
            "{bus:?}"
        );
        let inf = no_terminal(|t| inferir(arquivo, false, t));
        assert!(!inf.chars().any(perigoso), "{inf:?}");
    }

    /// `inferir` imprime chaves do arquivo: chave com bidi e C1 sai neutralizada.
    #[test]
    fn inferir_neutraliza_chave() {
        let f = Temporario::novo("inferir", "{\"a\u{202e}b\u{9b}\":1}\n");
        let inf = no_terminal(|t| inferir(f.0.to_str().unwrap(), false, t));
        assert!(inf.contains("a\\u{202e}b\\u{9b}"), "{inf:?}");
        assert!(!inf.chars().any(perigoso), "{inf:?}");
    }

    /// O erro do jq cita o valor do arquivo: no stderr de terminal, sai neutralizado.
    #[test]
    fn erro_da_consulta_neutralizado() {
        let f = Temporario::novo("erro-jq", HOSTIL);
        let op = OpcoesConsulta {
            filtro: "error(.k)",
            arquivo: f.0.to_str().unwrap(),
            documento: false,
            tempo_max: None,
        };
        let mut err = Terminal {
            out: Vec::new(),
            neutralizar: true,
        };
        no_terminal(|t| consultar(op, t, &mut err));
        let s = String::from_utf8(err.out).unwrap();
        assert!(s.contains("\\u{9b}\\u{202e} ok"), "{s:?}");
        assert!(!s.chars().any(perigoso), "{s:?}");
    }

    #[test]
    fn terminal_desligado_e_fiel_ao_byte() {
        let cru: &[u8] = b"\x1b[2J \xff \xe2\x80\xae";
        assert_eq!(escrito(false, &[cru]), cru);
        // Ligado, UTF-8 inválido vira U+FFFD (o terminal mostraria igual).
        assert_eq!(
            escrito(true, &[cru]),
            "\\u{1b}[2J \u{fffd} \\u{202e}".as_bytes()
        );
    }
}
