//! Gera JSONL sintético: `cargo run --release -p nucleo --example gerar -- <saida> [MB] [semente]`.

use std::fs::File;
use std::io::BufWriter;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(saida) = args.first() else {
        eprintln!("uso: gerar <saida.jsonl> [megabytes=100] [semente=1]");
        return ExitCode::from(2);
    };
    let mb: u64 = args
        .get(1)
        .map_or(Ok(100), |s| s.parse())
        .unwrap_or_else(|_| {
            eprintln!("megabytes inválido");
            std::process::exit(2)
        });
    let semente: u64 = args
        .get(2)
        .map_or(Ok(1), |s| s.parse())
        .unwrap_or_else(|_| {
            eprintln!("semente inválida");
            std::process::exit(2)
        });
    let resultado = File::create(saida).and_then(|f| {
        let mut w = BufWriter::with_capacity(1 << 20, f);
        let n = nucleo::gerador::gerar(&mut w, mb << 20, semente)?;
        std::io::Write::flush(&mut w)?;
        Ok(n)
    });
    match resultado {
        Ok(n) => {
            println!("{n} linhas em {saida}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("erro ao gerar {saida}: {e}");
            ExitCode::FAILURE
        }
    }
}
