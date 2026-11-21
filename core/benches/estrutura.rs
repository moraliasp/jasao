//! Inferência de estrutura sobre JSONL gerado em memória. Tamanho: `NUCLEO_BENCH_MB` (padrão 256).

use std::hint::black_box;
use std::sync::atomic::AtomicBool;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nucleo::estrutura::inferir;

fn estrutura(c: &mut Criterion) {
    let mb: u64 = std::env::var("NUCLEO_BENCH_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256);
    let mut dados = Vec::with_capacity((mb << 20) as usize + 1024);
    nucleo::gerador::gerar(&mut dados, mb << 20, 1).expect("gerar em memória");

    let mut g = c.benchmark_group(format!("estrutura_{mb}MB"));
    g.sample_size(10);
    g.throughput(Throughput::Bytes(dados.len() as u64));
    g.bench_function("inferir", |b| {
        b.iter(|| inferir(black_box(&dados), &AtomicBool::new(false), |_| {}))
    });
    g.finish();
}

criterion_group!(benches, estrutura);
criterion_main!(benches);
