//! Vazão do índice de linhas sobre JSONL gerado (em memória, sem custo de disco).
//! Tamanho: `NUCLEO_BENCH_MB` (padrão 256).

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nucleo::IndiceLinhas;
use std::hint::black_box;

fn indice(c: &mut Criterion) {
    let mb: u64 = std::env::var("NUCLEO_BENCH_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256);
    let mut dados = Vec::with_capacity((mb << 20) as usize + 1024);
    nucleo::gerador::gerar(&mut dados, mb << 20, 1).expect("gerar em memória");

    let mut g = c.benchmark_group("indice_linhas");
    g.throughput(Throughput::Bytes(dados.len() as u64));
    g.sample_size(20);
    g.bench_function(format!("{mb}MB"), |b| {
        b.iter(|| IndiceLinhas::construir(black_box(&dados)).len())
    });
    g.finish();
}

criterion_group!(benches, indice);
criterion_main!(benches);
