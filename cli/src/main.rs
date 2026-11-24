//! Comandos (o arquivo pode ser JSONL cru, gzip ou zstd):
//!   linhas <arquivo>                      conta as linhas e mostra o tempo do índice
//!   janela <arquivo> <inicio> <quantidade> imprime as linhas cruas pedidas
//!   buscar <arquivo> <padrão> [--regex] [--ignorar-caixa] [--max N]
//!                                         imprime linha:coluna (em bytes, a partir de 1) e trecho
//!   jq|consultar <filtro> <arquivo> [--documento] [--tempo-max SEG] [--saida jsonl|csv]
//!              [--max-resultados N] [--max-mb N] [--max-memoria-mb N]
//!                                         roda o filtro jq em cada linha (ou no documento inteiro);
//!                                         resultados em JSONL (ou CSV achatado, ver `nucleo::csv`)
//!                                         no stdout, erros por linha no stderr. Passar de um teto
//!                                         de saída imprime o parcial e sai com 1. O CSV junta o
//!                                         resultado em memória: sem `--max-mb`, o teto é 256 MiB.
//!                                         Roda num processo filho com teto de heap (4096 MiB sem
//!                                         `--max-memoria-mb`) e de relógio (300 s sem `--tempo-max`);
//!                                         passar de um deles, ou recursão sem fundo, sai com 3
//!   erros <arquivo> [--json] [--max N]    lista as linhas que não são JSON; sai com 1 se houver
//!   inferir <arquivo> [--json]            estrutura por caminho (linhas a partir de 1)
//!   exportar --zod|--schema|--ts [--literal] [--numero-texto manter|numero] <arquivo>
//!                                         Zod 4, JSON Schema 2020-12 ou tipos TypeScript (.d.ts)
//!                                         no stdout; valida o arquivo contra ele e sai com 1 se
//!                                         algum registro falhar
//!   validar <arquivo> --schema <schema.json> | --como <referência> [--literal]
//!                                         confere cada registro contra um JSON Schema (o salvo
//!                                         pelo `exportar --schema` ou um de fora) ou contra a
//!                                         estrutura inferida de outro arquivo; imprime as falhas
//!                                         (linha: /caminho: motivo, até 20). Sai com 0 se todos
//!                                         passam, 1 se algum falha (ou não é JSON), 2 em uso
//!                                         errado, 3 se não deu para validar (arquivo não abre,
//!                                         schema recusado: >1 MiB, >64 níveis, `$ref` não local)
//!   diff <antes> <depois> [--json] [--falha-em quebra|qualquer]
//!                                         mudanças de estrutura, quebra × compatível; sai com 1
//!                                         se `--falha-em` pegar quebra (ou qualquer), 2 em uso
//!                                         errado, 3 se não deu para comparar (arquivo não abre,
//!                                         ou registro inválido com `--falha-em`)
//!   diff --valores <antes> <depois> [--json] [--falha-em qualquer]
//!                                         valores adicionados, removidos e alterados, registro N
//!                                         contra registro N; mesmos códigos de saída
//!   linha-do-tempo <pasta> [--json]       versões da pasta em ordem de data (no nome ou, se algum
//!                                         nome não tem, mtime) e o diff de estrutura de cada uma
//!                                         contra a anterior; até 500 arquivos, 10 min, 200
//!                                         mudanças por versão; link para fora da pasta não entra
//!
//! Com o stdout num terminal, `janela`, `buscar`, `consultar`, `inferir`, `diff` e `linha-do-tempo` trocam controles (C0
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
                   jq|consultar <filtro> <arquivo> [--documento] [--tempo-max SEG] [--saida jsonl|csv]\n\
                   \x20     [--max-resultados N] [--max-mb N] [--max-memoria-mb N]\n  \
                   erros <arquivo> [--json] [--max N]\n  \
                   inferir <arquivo> [--json]\n  \
                   exportar --zod|--schema|--ts [--literal] [--numero-texto manter|numero] <arquivo>\n  \
                   validar <arquivo> --schema <schema.json> | --como <referência> [--literal]\n\
                   \x20   saída do validar: 0 todos passam · 1 algum falha · 2 uso errado ·\n\
                   \x20   3 não deu para validar (arquivo não abre, schema recusado)\n  \
                   diff <antes> <depois> [--json] [--falha-em quebra|qualquer]\n  \
                   diff --valores <antes> <depois> [--json] [--falha-em qualquer]\n\
                   \x20   saída do diff: 0 ok · 1 `--falha-em` pegou · 2 uso errado ·\n\
                   \x20   3 não deu para comparar (arquivo não abre, ou registro inválido com `--falha-em`)\n  \
                   linha-do-tempo <pasta> [--json]";

/// Conta o heap: o filho da consulta liga o teto (ver `supervisionar`).
#[global_allocator]
static ALOCADOR: nucleo::memoria::Contador = nucleo::memoria::Contador;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if let ["consultar" | "jq", resto @ ..] = args.as_slice() {
        match opcoes_consulta(resto) {
            Some(op) if std::env::var_os(VAR_FILHO).is_none() => return supervisionar(&op),
            Some(op) => nucleo::memoria::limitar(op.max_memoria),
            None => {}
        }
    }
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
        ["consultar" | "jq", resto @ ..] => match opcoes_consulta(resto) {
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
        ["linha-do-tempo", pasta] => linha_do_tempo(pasta, false, &mut tela()),
        ["linha-do-tempo", pasta, "--json"] | ["linha-do-tempo", "--json", pasta] => {
            linha_do_tempo(pasta, true, &mut tela())
        }
        ["inferir", arquivo, "--json"] | ["inferir", "--json", arquivo] => {
            inferir(arquivo, true, &mut tela())
        }
        ["exportar", resto @ ..] => match opcoes_exportar(resto) {
            Some((arquivo, formato, op)) => exportar(arquivo, formato, op).map(|ok| {
                if !ok {
                    saida = ExitCode::from(1);
                }
            }),
            None => {
                eprintln!("{USO}");
                return ExitCode::from(2);
            }
        },
        ["validar", resto @ ..] => match opcoes_validar(resto) {
            Some(op) => match validar(op, &mut tela()) {
                Ok(falhou) => {
                    saida = ExitCode::from(u8::from(falhou));
                    Ok(())
                }
                // Schema recusado ou arquivo que não abre não pode sair como "falhou" (1) em CI.
                Err(e) if !pipe_fechado(&*e) => {
                    let _ = writeln!(erro(), "erro: {e}");
                    return ExitCode::from(VALIDAR_SEM_VALIDAR);
                }
                Err(e) => Err(e),
            },
            None => {
                eprintln!("{USO}");
                return ExitCode::from(2);
            }
        },
        ["diff", resto @ ..] => match opcoes_diff(resto) {
            Some(op) => match diff(op, &mut tela()) {
                Ok(c) => {
                    saida = ExitCode::from(c);
                    Ok(())
                }
                // Erro de execução não pode sair como "quebra" (1) num gate de CI.
                Err(e) if !pipe_fechado(&*e) => {
                    eprintln!("erro: {e}");
                    return ExitCode::from(DIFF_SEM_COMPARAR);
                }
                Err(e) => Err(e),
            },
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
        Err(e) if pipe_fechado(&*e) => ExitCode::SUCCESS,
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

#[derive(Default)]
struct OpcoesConsulta<'a> {
    filtro: &'a str,
    arquivo: &'a str,
    documento: bool,
    tempo_max: Option<Duration>,
    csv: bool,
    max_resultados: Option<u64>,
    max_bytes: Option<u64>,
    max_memoria: usize,
}

/// Teto de bytes do CSV sem `--max-mb`: o CSV precisa do resultado inteiro (as colunas vêm de
/// todos os registros) e o junta em memória.
const TETO_CSV: u64 = 256 << 20;

/// Teto de heap do filho sem `--max-memoria-mb`.
const TETO_MEMORIA: usize = 4096 << 20;
/// Relógio de parede do filho sem `--tempo-max`: `until(false; .)` não entrega nada e nunca
/// chega ao teto de tempo do núcleo, que só é visto entre resultados.
const TEMPO_PADRAO: Duration = Duration::from_secs(300);

fn opcoes_consulta<'a>(args: &[&'a str]) -> Option<OpcoesConsulta<'a>> {
    let mut op = OpcoesConsulta {
        max_memoria: TETO_MEMORIA,
        ..Default::default()
    };
    let mut posicionais = Vec::new();
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--documento" => op.documento = true,
            "--tempo-max" => {
                op.tempo_max = Some(Duration::try_from_secs_f64(it.next()?.parse().ok()?).ok()?)
            }
            "--saida" => match *it.next()? {
                "jsonl" => op.csv = false,
                "csv" => op.csv = true,
                _ => return None,
            },
            "--max-resultados" => op.max_resultados = Some(it.next()?.parse().ok()?),
            "--max-mb" => {
                op.max_bytes = Some(it.next()?.parse::<u64>().ok()?.checked_mul(1 << 20)?)
            }
            "--max-memoria-mb" => {
                op.max_memoria = it.next()?.parse::<usize>().ok()?.checked_mul(1 << 20)?
            }
            _ => posicionais.push(a),
        }
    }
    let [filtro, arquivo] = posicionais[..] else {
        return None;
    };
    if op.csv {
        op.max_bytes.get_or_insert(TETO_CSV);
    }
    Some(OpcoesConsulta {
        filtro,
        arquivo,
        ..op
    })
}

/// Folga depois do teto de tempo antes de matar o filho: o núcleo só vê o teto entre
/// resultados, e filtro que não produz nada (`last(range(1e12))`, `def f: f; f`) nunca devolve.
const FOLGA_TEMPO: Duration = Duration::from_secs(2);
/// Marca o processo filho da consulta (ver `supervisionar`).
const VAR_FILHO: &str = "JASAO_CLI_FILHO";

/// Reexecuta o próprio binário como filho (mesmos argumentos, stdio herdado) e o vigia: mata no
/// relógio de parede e troca a morte por sinal (teto de heap, pilha estourada) por mensagem e
/// código 3, em vez de o processo abortar.
fn supervisionar(op: &OpcoesConsulta) -> ExitCode {
    let falhar = |m: String| {
        eprintln!("erro: {m}");
        ExitCode::from(3)
    };
    let mut filho = match std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(VAR_FILHO, "1")
            .spawn()
    }) {
        Ok(f) => f,
        Err(e) => return falhar(format!("não deu para iniciar a consulta: {e}")),
    };
    let limite = op.tempo_max.map_or(TEMPO_PADRAO, |t| t + FOLGA_TEMPO);
    let prazo = Instant::now() + limite;
    let status = loop {
        match filho.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= prazo => {
                let _ = filho.kill();
                let _ = filho.wait();
                return falhar(format!(
                    "a consulta passou de {limite:.0?} e foi parada (filtro não respondeu)"
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return falhar(format!("não deu para acompanhar a consulta: {e}")),
        }
    };
    match status.code() {
        Some(c) => ExitCode::from(u8::try_from(c).unwrap_or(3)),
        None => falhar(format!(
            "a consulta parou sem terminar: passou do teto de memória ({} MiB) ou recursão sem fundo",
            op.max_memoria >> 20
        )),
    }
}

fn consultar(
    op: OpcoesConsulta,
    out: &mut Terminal<impl Write>,
    err: &mut Terminal<impl Write>,
) -> Resultado {
    let consulta = Consulta::nova(op.filtro)?
        .com_tempo_max(op.tempo_max)
        .com_teto_saida(op.max_resultados, op.max_bytes);
    let a = abrir(op.arquivo)?;
    let t = Instant::now();
    let cancelar = AtomicBool::new(false);
    let mut erro = None;
    let mut para_csv = Vec::new();
    let ao_lote = |lote: nucleo::consulta::LoteConsulta| {
        if op.csv {
            para_csv.extend(lote.resultados.into_iter().map(|r| r.json));
            return;
        }
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
    if op.csv {
        nucleo::csv::de_jsonl(para_csv.iter().map(|j| j.as_bytes()), out)?;
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
    if resumo.cortado {
        return Err(format!(
            "a saída passou do teto ({}); resultado parcial",
            match (op.max_resultados, op.max_bytes) {
                (Some(n), Some(b)) => format!("{n} resultados ou {} MiB", b >> 20),
                (Some(n), None) => format!("{n} resultados"),
                (None, b) => format!("{} MiB", b.unwrap_or_default() >> 20),
            }
        )
        .into());
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Formato {
    Zod,
    Schema,
    Ts,
}

fn opcoes_exportar<'a>(args: &[&'a str]) -> Option<(&'a str, Formato, nucleo::exportar::Opcoes)> {
    let (mut arquivo, mut zod, mut op) = (None, None, nucleo::exportar::Opcoes::default());
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--zod" if zod.is_none() => zod = Some(Formato::Zod),
            "--schema" if zod.is_none() => zod = Some(Formato::Schema),
            "--ts" if zod.is_none() => zod = Some(Formato::Ts),
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
    formato: Formato,
    op: nucleo::exportar::Opcoes,
) -> Result<bool, Box<dyn std::error::Error>> {
    use nucleo::{estrutura, exportar, validar};
    let a = abrir(arquivo)?;
    let nada = AtomicBool::new(false);
    let inf = estrutura::inferir(a.bytes(), &nada, |_| {});
    let r = &inf.resumo;
    let (codigo, schema) = match formato {
        Formato::Zod => (exportar::zod(r, op), exportar::schema_da_entrada_zod(r, op)),
        Formato::Ts => (
            exportar::tipos(r, op),
            exportar::schema_da_entrada_zod(r, op),
        ),
        Formato::Schema => {
            let s = exportar::schema(r, op);
            (s.clone(), s)
        }
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
    // O motivo cita caminho de chave do arquivo: sai pelo stderr neutralizado.
    let mut err = erro();
    for f in &v.falhas {
        writeln!(err, "linha {}: {}", f.linha + 1, f.motivo)?;
    }
    Ok(v.falham == 0 && v.invalidas == 0)
}

/// Contra o que validar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Contra<'a> {
    Schema(&'a str),
    /// Estrutura inferida deste arquivo de referência; `bool` = `--literal`.
    Como(&'a str, bool),
}

fn opcoes_validar<'a>(args: &[&'a str]) -> Option<(&'a str, Contra<'a>)> {
    let (mut arquivo, mut schema, mut como, mut literal) = (None, None, None, false);
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--schema" if schema.is_none() => schema = Some(*it.next()?),
            "--como" if como.is_none() => como = Some(*it.next()?),
            "--literal" => literal = true,
            _ if arquivo.is_none() && !a.starts_with("--") => arquivo = Some(a),
            _ => return None,
        }
    }
    let contra = match (schema, como) {
        (Some(s), None) if !literal => Contra::Schema(s),
        (None, Some(c)) => Contra::Como(c, literal),
        _ => return None,
    };
    Some((arquivo?, contra))
}

/// Saída do `validar` quando não deu para validar.
const VALIDAR_SEM_VALIDAR: u8 = 3;

/// Imprime as falhas (as primeiras `MAX_FALHAS`, por linha) no `out`. `Ok(true)` se algum
/// registro falhou ou não é JSON.
fn validar(
    (arquivo, contra): (&str, Contra),
    out: &mut Terminal<impl Write>,
) -> Result<bool, Box<dyn std::error::Error>> {
    use nucleo::{estrutura, exportar, validar};
    let nada = AtomicBool::new(false);
    let schema = match contra {
        Contra::Schema(s) => validar::ler_schema(std::path::Path::new(s))?,
        Contra::Como(c, literal) => {
            let r = abrir(c)?;
            let inf = estrutura::inferir(r.bytes(), &nada, |_| {});
            let op = exportar::Opcoes {
                literal,
                numero: false,
            };
            exportar::schema(&inf.resumo, op)
        }
    };
    // Recusa o schema antes de abrir o arquivo (que pode ser grande).
    validar::compilar(&schema).map_err(|e| format!("schema recusado: {e}"))?;
    let a = abrir(arquivo)?;
    // shortcut: a inferência inteira só para saber se é JSONL ou JSON único; trocar por um
    // detector leve se o tempo do `validar` em arquivo grande incomodar.
    let entrada = estrutura::inferir(a.bytes(), &nada, |_| {}).entrada;
    let t = Instant::now();
    let v = validar::validar(a.bytes(), entrada, &schema, &nada, |_| {})?;
    // O motivo cita caminho de chave do arquivo: sai neutralizado no terminal.
    for f in &v.falhas {
        writeln!(out, "linha {}: {}", f.linha + 1, f.motivo)?;
    }
    out.flush()?;
    eprintln!(
        "{} de {} registros passam · {} falham · {} linhas não são JSON · validado em {:.2?}",
        v.passam,
        v.passam + v.falham,
        v.falham,
        v.invalidas,
        t.elapsed()
    );
    if v.falham + v.invalidas > v.falhas.len() as u64 {
        eprintln!("(só as {} primeiras falhas aparecem)", v.falhas.len());
    }
    Ok(v.falham + v.invalidas > 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FalhaEm {
    Nunca,
    Quebra,
    Qualquer,
}

struct OpcoesDiff<'a> {
    antes: &'a str,
    depois: &'a str,
    json: bool,
    falha_em: FalhaEm,
    valores: bool,
}

fn opcoes_diff<'a>(args: &[&'a str]) -> Option<OpcoesDiff<'a>> {
    let (mut arquivos, mut json, mut falha_em) = (Vec::new(), false, FalhaEm::Nunca);
    let mut valores = false;
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--json" => json = true,
            "--valores" => valores = true,
            "--falha-em" => {
                falha_em = match *it.next()? {
                    "quebra" => FalhaEm::Quebra,
                    "qualquer" => FalhaEm::Qualquer,
                    _ => return None,
                }
            }
            _ if !a.starts_with("--") => arquivos.push(a),
            _ => return None,
        }
    }
    let [antes, depois] = arquivos[..] else {
        return None;
    };
    // Valor não tem quebra × compatível.
    if valores && falha_em == FalhaEm::Quebra {
        return None;
    }
    Some(OpcoesDiff {
        antes,
        depois,
        json,
        falha_em,
        valores,
    })
}

/// Imprime as mudanças de estrutura. `Ok(true)` se `--falha-em` pegou alguma.
fn pipe_fechado(e: &(dyn std::error::Error + 'static)) -> bool {
    e.downcast_ref::<io::Error>().map(io::Error::kind) == Some(io::ErrorKind::BrokenPipe)
}

/// Saída do `diff` quando não deu para comparar com confiança.
const DIFF_SEM_COMPARAR: u8 = 3;

/// Código de saída: 0, 1 (`--falha-em` pegou) ou `DIFF_SEM_COMPARAR` (registro inválido com
/// `--falha-em`: linhas ignoradas podem esconder uma quebra).
/// Uma mudança do diff de estrutura no `--json`.
fn mudanca_json(m: &nucleo::diff::Mudanca) -> String {
    use nucleo::diff::Detalhe;
    let detalhe = match &m.detalhe {
        Detalhe::Nenhum => String::new(),
        Detalhe::Tipos(x, y) => {
            let l = |ts: &[nucleo::estrutura::Tipo]| {
                ts.iter()
                    .map(|t| format!("\"{}\"", t.nome()))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            format!(r#","tipos":{{"antes":[{}],"depois":[{}]}}"#, l(x), l(y))
        }
        Detalhe::Variante(v) => format!(r#","variante":{}"#, texto_json(v)),
    };
    format!(
        r#"{{"caminho":{},"regra":"{}","classe":"{}","antes":{{"n":{},"de":{}}},"depois":{{"n":{},"de":{}}}{detalhe}}}"#,
        texto_json(&m.caminho),
        m.regra.nome(),
        m.regra.classe().nome(),
        m.antes.n,
        m.antes.de,
        m.depois.n,
        m.depois.de,
    )
}

/// Linha do tempo da pasta: cada versão em ordem e o diff de estrutura contra a anterior.
fn linha_do_tempo(pasta: &str, json: bool, out: &mut Terminal<impl Write>) -> Resultado {
    use nucleo::linha_do_tempo::{LIMITES, Origem, Parada, data_texto, listar, percorrer};
    let lista =
        listar(std::path::Path::new(pasta), &LIMITES).map_err(|e| format!("{pasta}: {e}"))?;
    let r = percorrer(
        lista,
        &LIMITES,
        &Opcoes::default(),
        &AtomicBool::new(false),
        |_, _| {},
    );
    let origem = match r.origem {
        Origem::Nome => "nome",
        Origem::Mtime => "mtime",
    };
    let parada = match r.parada {
        None => "null",
        Some(Parada::Prazo) => r#""prazo""#,
        Some(Parada::Cancelado) => r#""cancelado""#,
    };
    if json {
        let itens: Vec<String> = r
            .passos
            .iter()
            .map(|s| {
                let ms: Vec<String> = s.mudancas.iter().map(mudanca_json).collect();
                format!(
                    r#"{{"nome":{},"data":"{}","registros":{},"invalidas":{},"erro":{},"contra":{},"quebras":{},"compativeis":{},"mais":{},"mudancas":[{}]}}"#,
                    texto_json(&s.versao.nome),
                    data_texto(s.versao.ms),
                    s.registros,
                    s.invalidas,
                    s.erro.as_deref().map_or("null".into(), texto_json),
                    s.contra.map_or("null".into(), |c| c.to_string()),
                    s.quebras,
                    s.compativeis,
                    s.mais,
                    ms.join(",")
                )
            })
            .collect();
        writeln!(
            out,
            r#"{{"origem":"{origem}","ignorados":{},"parada":{parada},"versoes":[{}]}}"#,
            r.ignorados,
            itens.join(",")
        )?;
    } else {
        for s in &r.passos {
            let data = data_texto(s.versao.ms);
            write!(out, "{data}  {}  ", s.versao.nome)?;
            match (&s.erro, s.contra) {
                (Some(e), _) => writeln!(out, "não abriu: {e}")?,
                (None, None) => writeln!(out, "{} registros", s.registros)?,
                (None, Some(c)) => writeln!(
                    out,
                    "{} quebras · {} compatíveis contra {}",
                    s.quebras, s.compativeis, r.passos[c].versao.nome
                )?,
            }
            for m in &s.mudancas {
                writeln!(out, "  {m}")?;
            }
            if s.mais > 0 {
                writeln!(out, "  … e mais {}", s.mais)?;
            }
        }
    }
    out.flush()?;
    eprintln!(
        "{} versões · ordem por {origem}{} · {} entradas ignoradas",
        r.passos.len(),
        if origem == "mtime" { " (UTC)" } else { "" },
        r.ignorados
    );
    if r.parada == Some(Parada::Prazo) {
        eprintln!(
            "aviso: parou no prazo de {} s; faltam versões",
            LIMITES.prazo.as_secs()
        );
    }
    Ok(())
}

fn diff(op: OpcoesDiff, out: &mut Terminal<impl Write>) -> Result<u8, Box<dyn std::error::Error>> {
    use nucleo::diff::{Classe, diff};
    use nucleo::estrutura;
    let nada = AtomicBool::new(false);
    let (a, b) = (abrir(op.antes)?, abrir(op.depois)?);
    if op.valores {
        return diff_valores(&op, &a, &b, out);
    }
    let (ia, ib) = (
        estrutura::inferir(a.bytes(), &nada, |_| {}),
        estrutura::inferir(b.bytes(), &nada, |_| {}),
    );
    let mudancas = diff(&ia.resumo, &ib.resumo);
    let quebras = mudancas
        .iter()
        .filter(|m| m.regra.classe() == Classe::Quebra)
        .count();
    let invalidas = ia.resumo.invalidas.n > 0 || ib.resumo.invalidas.n > 0;
    let aviso = format!(
        "aviso: registros inválidos ignorados (antes: {}, depois: {}); o diff não os vê",
        ia.resumo.invalidas.n, ib.resumo.invalidas.n
    );
    if op.json {
        let itens: Vec<String> = mudancas.iter().map(mudanca_json).collect();
        writeln!(
            out,
            r#"{{"quebras":{quebras},"compativeis":{},"invalidas":{{"antes":{},"depois":{}}},"mudancas":[{}]}}"#,
            mudancas.len() - quebras,
            ia.resumo.invalidas.n,
            ib.resumo.invalidas.n,
            itens.join(",")
        )?;
    } else {
        for m in &mudancas {
            writeln!(out, "{m}")?;
        }
        if invalidas {
            writeln!(out, "{aviso}")?;
        }
    }
    out.flush()?;
    let resumo = |i: &estrutura::Inferencia| {
        format!(
            "{} registros, {} inválidas",
            i.resumo.registros(),
            i.resumo.invalidas.n
        )
    };
    eprintln!(
        "{quebras} quebras · {} compatíveis · antes: {} · depois: {}",
        mudancas.len() - quebras,
        resumo(&ia),
        resumo(&ib)
    );
    if invalidas && op.falha_em != FalhaEm::Nunca {
        eprintln!("erro: {aviso}; `--falha-em` não confia no resultado (rode `jasao erros`)");
        return Ok(DIFF_SEM_COMPARAR);
    }
    let falhou = match op.falha_em {
        FalhaEm::Nunca => false,
        FalhaEm::Quebra => quebras > 0,
        FalhaEm::Qualquer => !mudancas.is_empty(),
    };
    Ok(u8::from(falhou))
}

/// `diff --valores`: registro N contra registro N, em fluxo; guarda os primeiros
/// `valores::MAX_REGISTROS` diferentes, conta todos.
fn diff_valores(
    op: &OpcoesDiff,
    a: &ArquivoLinhas,
    b: &ArquivoLinhas,
    out: &mut Terminal<impl Write>,
) -> Result<u8, Box<dyn std::error::Error>> {
    use nucleo::estrutura::entrada_provavel;
    use nucleo::valores::{self, MAX_REGISTROS};
    let nada = AtomicBool::new(false);
    let (da, db) = (a.bytes(), b.bytes());
    let r = valores::arquivos(
        da,
        entrada_provavel(da),
        db,
        entrada_provavel(db),
        &nada,
        |_| {},
    );
    let opt = |v: &Option<String>| v.as_deref().map_or("null".into(), texto_json);
    let linha = |l: Option<u64>| l.map_or("null".into(), |l| (l + 1).to_string());
    if op.json {
        let regs: Vec<String> = r
            .guardados
            .iter()
            .map(|g| {
                let difs: Vec<String> = g
                    .diferencas
                    .iter()
                    .map(|d| {
                        format!(
                            r#"{{"caminho":{},"acao":"{}","antes":{},"depois":{}}}"#,
                            texto_json(&d.caminho),
                            d.acao.nome(),
                            opt(&d.antes),
                            opt(&d.depois)
                        )
                    })
                    .collect();
                format!(
                    r#"{{"registro":{},"linha_antes":{},"linha_depois":{},"total":{},"diferencas":[{}]}}"#,
                    g.registro + 1,
                    linha(g.linha_antes),
                    linha(g.linha_depois),
                    g.total,
                    difs.join(",")
                )
            })
            .collect();
        writeln!(
            out,
            r#"{{"iguais":{},"diferentes":{},"so_antes":{},"so_depois":{},"invalidos":{},"grandes":{},"diferencas":{},"registros":[{}]}}"#,
            r.iguais,
            r.diferentes,
            r.so_antes,
            r.so_depois,
            r.invalidos,
            r.grandes,
            r.diferencas,
            regs.join(",")
        )?;
    } else {
        for g in &r.guardados {
            writeln!(
                out,
                "registro {} (linha {} → {})",
                g.registro + 1,
                linha(g.linha_antes),
                linha(g.linha_depois)
            )?;
            for d in &g.diferencas {
                writeln!(out, "  {d}")?;
            }
            if g.total > g.diferencas.len() as u64 {
                writeln!(out, "  … e mais {}", g.total - g.diferencas.len() as u64)?;
            }
        }
        let guardados = r.diferentes + r.so_antes + r.so_depois;
        if guardados > MAX_REGISTROS as u64 {
            writeln!(
                out,
                "… e mais {} registros diferentes",
                guardados - MAX_REGISTROS as u64
            )?;
        }
    }
    out.flush()?;
    eprintln!(
        "{} iguais · {} diferentes · {} só antes · {} só depois · {} inválidos",
        r.iguais, r.diferentes, r.so_antes, r.so_depois, r.invalidos
    );
    if r.grandes > 0 {
        eprintln!(
            "aviso: {} registros acima de {} MiB não comparados",
            r.grandes,
            valores::MAX_BYTES_REGISTRO >> 20
        );
    }
    if r.invalidos + r.grandes > 0 && op.falha_em != FalhaEm::Nunca {
        eprintln!(
            "erro: registros inválidos ou grandes demais não comparados; `--falha-em` não confia no resultado"
        );
        return Ok(DIFF_SEM_COMPARAR);
    }
    let algum = r.diferentes + r.so_antes + r.so_depois > 0;
    Ok(u8::from(op.falha_em == FalhaEm::Qualquer && algum))
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
            ..OpcoesConsulta::default()
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

    #[test]
    fn opcoes_do_diff() {
        fn ok<'a>(a: &[&'a str]) -> Option<(&'a str, &'a str, bool, FalhaEm)> {
            opcoes_diff(a).map(|o| (o.antes, o.depois, o.json, o.falha_em))
        }
        assert!(opcoes_diff(&["a", "b"]).is_some_and(|o| !o.valores));
        assert!(
            opcoes_diff(&["--valores", "a", "b", "--falha-em", "qualquer"])
                .is_some_and(|o| o.valores)
        );
        assert_eq!(ok(&["a", "b"]), Some(("a", "b", false, FalhaEm::Nunca)));
        assert_eq!(
            ok(&["--falha-em", "quebra", "a", "--json", "b"]),
            Some(("a", "b", true, FalhaEm::Quebra))
        );
        assert_eq!(
            ok(&["a", "b", "--falha-em", "qualquer"]).map(|o| o.3),
            Some(FalhaEm::Qualquer)
        );
        for ruim in [
            &["a"][..],
            &["a", "b", "c"],
            &["a", "b", "--falha-em"],
            &["a", "b", "--falha-em", "sempre"],
            &["a", "b", "--x"],
            &["--valores", "a", "b", "--falha-em", "quebra"],
        ] {
            assert!(opcoes_diff(ruim).is_none(), "{ruim:?}");
        }
    }

    #[test]
    fn diff_neutraliza_e_falha_em() {
        let a = Temporario::novo("diff-a", "{\"a\u{202e}b\u{9b}\":1,\"x\":1}\n");
        let b = Temporario::novo("diff-b", "{\"x\":1}\n{\"x\":2,\"n\":true}\n");
        let (pa, pb) = (a.0.to_str().unwrap(), b.0.to_str().unwrap());
        let rodar = |antes, depois, json, falha_em| {
            let mut t = Terminal {
                out: Vec::new(),
                neutralizar: true,
            };
            let op = OpcoesDiff {
                antes,
                depois,
                json,
                falha_em,
                valores: false,
            };
            let c = diff(op, &mut t).unwrap();
            (String::from_utf8(t.out).unwrap(), c == 1)
        };
        let (texto, falhou) = rodar(pa, pb, false, FalhaEm::Quebra);
        assert!(falhou);
        assert_eq!(
            texto,
            "quebra      .a\\u{202e}b\\u{9b}  campo removido  (1 de 1 → 0 de 2)\n\
             compatível  .n  campo novo opcional  (0 de 1 → 1 de 2)\n"
        );
        let (json, _) = rodar(pa, pb, true, FalhaEm::Nunca);
        assert!(json.starts_with(r#"{"quebras":1,"compativeis":1,"invalidas":{"antes":0,"depois":0},"mudancas":[{"caminho":".a"#));
        assert!(json.contains(r#""regra":"campo_novo_opcional","classe":"compativel","antes":{"n":0,"de":1},"depois":{"n":1,"de":2}}"#), "{json}");
        assert!(!json.chars().any(perigoso));
        // Só compatível: `quebra` passa, `qualquer` falha; sem `--falha-em` nunca falha.
        assert!(!rodar(pb, pb, false, FalhaEm::Qualquer).1);
        let c = Temporario::novo("diff-c", "{\"x\":1}\n{\"x\":2,\"n\":true,\"m\":null}\n");
        let pc = c.0.to_str().unwrap();
        assert!(!rodar(pb, pc, false, FalhaEm::Quebra).1);
        assert!(rodar(pb, pc, false, FalhaEm::Qualquer).1);
        assert!(!rodar(pa, pb, false, FalhaEm::Nunca).1);
    }

    #[test]
    fn diff_valores_neutraliza_e_falha_em() {
        let a = Temporario::novo("dv-a", "{\"a\":1}\n{\"s\":\"x\u{9b}\"}\n");
        let b = Temporario::novo("dv-b", "{\"a\":1}\n{\"s\":\"y\"}\n{\"n\":2}\n");
        let (pa, pb) = (a.0.to_str().unwrap(), b.0.to_str().unwrap());
        let rodar = |antes, depois, json, falha_em| {
            let mut t = Terminal {
                out: Vec::new(),
                neutralizar: true,
            };
            let op = OpcoesDiff {
                antes,
                depois,
                json,
                falha_em,
                valores: true,
            };
            let c = diff(op, &mut t).unwrap();
            (String::from_utf8(t.out).unwrap(), c)
        };
        let (texto, c) = rodar(pa, pb, false, FalhaEm::Qualquer);
        assert_eq!(c, 1);
        assert_eq!(
            texto,
            "registro 2 (linha 2 → 2)\n  alterado    .s  \"x\\u{9b}\" → \"y\"\n\
             registro 3 (linha null → 3)\n  adicionado  .  {\"n\":2}\n"
        );
        let (json, c) = rodar(pa, pb, true, FalhaEm::Nunca);
        assert_eq!(c, 0);
        assert!(json.starts_with(r#"{"iguais":1,"diferentes":1,"so_antes":0,"so_depois":1,"invalidos":0,"grandes":0,"diferencas":1,"registros":[{"registro":2,"linha_antes":2,"linha_depois":2,"total":1,"diferencas":[{"caminho":".s","acao":"alterado""#), "{json}");
        assert!(
            json.contains(r#"{"registro":3,"linha_antes":null,"linha_depois":3"#),
            "{json}"
        );
        assert_eq!(rodar(pa, pa, false, FalhaEm::Qualquer).1, 0);
        let c = Temporario::novo("dv-c", "{\"a\":1}\n{\n");
        assert_eq!(
            rodar(pa, c.0.to_str().unwrap(), false, FalhaEm::Qualquer).1,
            DIFF_SEM_COMPARAR
        );
        // Registro grande demais também não deixa o gate confiar no resultado.
        let g = format!(
            "{{\"a\":1}}\n[\"{}\"]\n",
            "x".repeat(nucleo::valores::MAX_BYTES_REGISTRO)
        );
        let g = Temporario::novo("dv-g", &g);
        let pg = g.0.to_str().unwrap();
        assert_eq!(rodar(pa, pg, false, FalhaEm::Qualquer).1, DIFF_SEM_COMPARAR);
        let (json, c) = rodar(pa, pg, true, FalhaEm::Nunca);
        assert!(json.contains(r#""invalidos":0,"grandes":1,"#), "{json}");
        assert_eq!(c, 0);
    }

    /// O erro do jq cita o valor do arquivo: no stderr de terminal, sai neutralizado.
    #[test]
    fn erro_da_consulta_neutralizado() {
        let f = Temporario::novo("erro-jq", HOSTIL);
        let op = OpcoesConsulta {
            filtro: "error(.k)",
            arquivo: f.0.to_str().unwrap(),
            ..OpcoesConsulta::default()
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

    fn jq(op: OpcoesConsulta) -> (String, Result<(), String>) {
        let mut t = Terminal {
            out: Vec::new(),
            neutralizar: false,
        };
        let mut err = Terminal {
            out: io::sink(),
            neutralizar: false,
        };
        let r = consultar(op, &mut t, &mut err).map_err(|e| e.to_string());
        (String::from_utf8(t.out).unwrap(), r)
    }

    #[test]
    fn jq_em_csv() {
        let f = Temporario::novo("jq-csv", "{\"a\":1,\"b\":\"=x\"}\n{\"a\":2,\"c\":[]}\n");
        let arquivo = f.0.to_str().unwrap();
        let op = opcoes_consulta(&[".", arquivo, "--saida", "csv"]).unwrap();
        assert_eq!(jq(op), (".a,.b,.c\r\n1,'=x,\r\n2,,[]\r\n".into(), Ok(())));
        assert!(opcoes_consulta(&[".", arquivo, "--saida", "xml"]).is_none());
    }

    #[test]
    fn jq_teto_de_saida() {
        let f = Temporario::novo("jq-teto", "1\n2\n3\n");
        let arquivo = f.0.to_str().unwrap();
        // `--tempo-max`: sem o teto de saída, o teste falha em vez de girar para sempre.
        let op = opcoes_consulta(&[
            "range(1e12)",
            arquivo,
            "--max-resultados",
            "4",
            "--tempo-max",
            "2",
        ])
        .unwrap();
        let (out, r) = jq(op);
        assert_eq!(out, "0\n1\n2\n3\n");
        assert!(r.unwrap_err().contains("teto (4 resultados)"));
        // CSV sem `--max-mb` tem teto (junta em memória); com `--max-mb`, vale o pedido.
        let op = opcoes_consulta(&[".", arquivo, "--saida", "csv"]).unwrap();
        assert_eq!(op.max_bytes, Some(TETO_CSV));
        let op = opcoes_consulta(&[".", arquivo, "--saida", "csv", "--max-mb", "1"]).unwrap();
        assert_eq!(op.max_bytes, Some(1 << 20));
        let op = OpcoesConsulta {
            max_bytes: Some(4),
            ..opcoes_consulta(&[".", arquivo, "--saida", "csv"]).unwrap()
        };
        let (out, r) = jq(op);
        assert_eq!(out, ".\r\n1\r\n2\r\n");
        assert!(r.unwrap_err().contains("resultado parcial"));
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

    #[test]
    fn opcoes_do_validar_e_do_exportar_ts() {
        assert_eq!(
            opcoes_validar(&["a", "--schema", "s"]),
            Some(("a", Contra::Schema("s")))
        );
        assert_eq!(
            opcoes_validar(&["--como", "r", "a", "--literal"]),
            Some(("a", Contra::Como("r", true)))
        );
        assert_eq!(
            opcoes_validar(&["a", "--como", "r"]),
            Some(("a", Contra::Como("r", false)))
        );
        for ruim in [
            &["a"][..],
            &["--schema", "s"],
            &["a", "--schema"],
            &["a", "--schema", "s", "--como", "r"],
            &["a", "--schema", "s", "--literal"],
            &["a", "--schema", "s", "--schema", "t"],
            &["a", "b", "--schema", "s"],
        ] {
            assert!(opcoes_validar(ruim).is_none(), "{ruim:?}");
        }
        let (_, f, op) = opcoes_exportar(&["a", "--ts", "--literal"]).unwrap();
        assert!(matches!(f, Formato::Ts) && op.literal);
        assert!(opcoes_exportar(&["a", "--ts", "--zod"]).is_none());
    }

    #[test]
    fn validar_lista_falhas_neutralizadas_e_recusa_ref_remoto() {
        let dados = Temporario::novo(
            "val-dados",
            "{\"n\":1}\n{\"n\":\"x\",\"k\u{202e}\":1}\n{\"n\":2}\n",
        );
        let schema = Temporario::novo(
            "val-schema",
            r#"{"type":"object","required":["n"],"properties":{"n":{"type":"integer"}},"additionalProperties":false}"#,
        );
        let remoto = Temporario::novo("val-remoto", r#"{"$ref":"https://exemplo.invalid/s.json"}"#);
        let ref_ok = Temporario::novo("val-ref", "{\"n\":5}\n");
        let [pd, ps, pr, pk] = [&dados, &schema, &remoto, &ref_ok].map(|t| t.0.to_str().unwrap());
        let rodar = |contra| {
            let mut t = Terminal {
                out: Vec::new(),
                neutralizar: true,
            };
            let r = validar((pd, contra), &mut t)
                .map(|falhou| (falhou, String::from_utf8(t.out).unwrap()));
            r.map_err(|e| e.to_string())
        };
        let (falhou, texto) = rodar(Contra::Schema(ps)).unwrap();
        assert!(falhou);
        assert_eq!(texto.lines().count(), 1, "{texto}");
        assert!(texto.starts_with("linha 2: "), "{texto}");
        assert!(
            texto.contains("\\u{202e}") && !texto.chars().any(perigoso),
            "{texto:?}"
        );
        // Pela estrutura de outro arquivo: `n` inteiro, então a linha 2 falha igual.
        let (falhou, texto) = rodar(Contra::Como(pk, false)).unwrap();
        assert!(falhou && texto.starts_with("linha 2: "), "{texto}");
        // `--literal` trava `n` em 5: as três linhas falham.
        assert_eq!(rodar(Contra::Como(pk, true)).unwrap().1.lines().count(), 3);
        // Tudo passa: `Ok(false)`, nada impresso.
        assert_eq!(
            rodar(Contra::Como(pd, false)).unwrap(),
            (false, String::new())
        );
        let e = rodar(Contra::Schema(pr)).unwrap_err();
        assert!(
            e.starts_with("schema recusado: ") && e.contains("proibido"),
            "{e}"
        );
        assert!(rodar(Contra::Schema("/nao/existe.json")).is_err());
    }
}
