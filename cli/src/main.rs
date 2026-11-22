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
//!
//! Com o stdout num terminal, `janela`, `buscar` e `inferir` trocam controles (C0 menos
//! `\n`/`\t`, DEL, C1) e invisíveis bidi/de largura zero por `\u{..}` visível: um JSON hostil
//! não manda sequência ESC/CSI/OSC ao terminal nem inverte o texto. Redirecionada (arquivo, pipe),
//! a saída é fiel byte a byte.

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
                   inferir <arquivo> [--json]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut saida = ExitCode::SUCCESS;
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
        ["consultar", resto @ ..] => match opcoes_consulta(resto) {
            Some(op) => consultar(op),
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
        ["inferir", arquivo] => inferir(arquivo, false),
        ["inferir", arquivo, "--json"] | ["inferir", "--json", arquivo] => inferir(arquivo, true),
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

/// O stdout, neutralizado se for terminal.
fn saida() -> Terminal<io::BufWriter<io::StdoutLock<'static>>> {
    let stdout = io::stdout();
    Terminal {
        neutralizar: stdout.is_terminal(),
        out: io::BufWriter::new(stdout.lock()),
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

fn janela(arquivo: &str, inicio: usize, quantidade: usize) -> Resultado {
    let a = abrir(arquivo)?;
    let mut out = saida();
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
    let mut out = saida();
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

fn consultar(op: OpcoesConsulta) -> Resultado {
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
    let mut out = io::BufWriter::new(io::stdout().lock());
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
        eprintln!("linha {}: {}", e.linha + 1, e.motivo);
    }
    if resumo.erros > resumo.primeiros_erros.len() as u64 {
        eprintln!(
            "… e mais {} erros",
            resumo.erros - resumo.primeiros_erros.len() as u64
        );
    }
    eprintln!(
        "{} resultados · {} erros · {} de {} linhas · {:.1} MB em {:.2?}",
        resumo.resultados,
        resumo.erros,
        resumo.linhas,
        resumo.total_linhas,
        a.bytes().len() as f64 / 1e6,
        t.elapsed()
    );
    if resumo.tempo_esgotado {
        return Err("tempo máximo esgotado; resultado parcial".into());
    }
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
    let mut out = saida();
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
