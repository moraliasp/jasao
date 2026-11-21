//! Comandos (o arquivo pode ser JSONL cru, gzip ou zstd):
//!   linhas <arquivo>                      conta as linhas e mostra o tempo do índice
//!   janela <arquivo> <inicio> <quantidade> imprime as linhas cruas pedidas
//!   buscar <arquivo> <padrão> [--regex] [--ignorar-caixa] [--max N]
//!                                         imprime linha:coluna (em bytes, a partir de 1) e trecho

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Instant;

use std::sync::atomic::AtomicBool;

use nucleo::{ArquivoLinhas, Busca, Opcoes};

const USO: &str = "uso:\n  linhas <arquivo>\n  janela <arquivo> <inicio> <quantidade>\n  \
                   buscar <arquivo> <padrão> [--regex] [--ignorar-caixa] [--max N]";

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
