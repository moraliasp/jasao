//! Comandos (o arquivo pode ser JSONL cru, gzip ou zstd):
//!   linhas <arquivo>                      conta as linhas e mostra o tempo do índice
//!   janela <arquivo> <inicio> <quantidade> imprime as linhas cruas pedidas

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Instant;

use nucleo::{ArquivoLinhas, Opcoes};

const USO: &str = "uso:\n  linhas <arquivo>\n  janela <arquivo> <inicio> <quantidade>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let resultado = match args.as_slice() {
        ["linhas", arquivo] => linhas(arquivo),
        ["janela", arquivo, inicio, quantidade] => match (inicio.parse(), quantidade.parse()) {
            (Ok(i), Ok(q)) => janela(arquivo, i, q),
            _ => Err("início e quantidade precisam ser números inteiros".into()),
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
