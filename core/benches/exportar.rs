//! Exportar (Schema e Zod a partir do resumo) e validar o arquivo inteiro contra o Schema
//! exportado, sobre JSONL gerado em memória. Tamanho: `NUCLEO_BENCH_MB` (padrão 256).

use std::hint::black_box;
use std::sync::atomic::AtomicBool;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nucleo::{estrutura::inferir, exportar, validar::validar};

fn exportar_validar(c: &mut Criterion) {
    let mb: u64 = std::env::var("NUCLEO_BENCH_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256);
    let mut dados = Vec::with_capacity((mb << 20) as usize + 1024);
    nucleo::gerador::gerar(&mut dados, mb << 20, 1).expect("gerar em memória");
    let nada = AtomicBool::new(false);
    let inf = inferir(&dados, &nada, |_| {});
    let op = exportar::Opcoes::default();
    let schema = exportar::schema(&inf.resumo, op);

    let mut g = c.benchmark_group(format!("exportar_{mb}MB"));
    g.bench_function("schema", |b| {
        b.iter(|| exportar::schema(black_box(&inf.resumo), op).len())
    });
    g.bench_function("zod", |b| {
        b.iter(|| exportar::zod(black_box(&inf.resumo), op).len())
    });
    g.sample_size(10);
    g.throughput(Throughput::Bytes(dados.len() as u64));
    g.bench_function("validar", |b| {
        b.iter(|| {
            let v = validar(black_box(&dados), inf.entrada, &schema, &nada, |_| {}).unwrap();
            assert_eq!(v.falham, 0, "o arquivo passa no próprio Schema");
            v.passam
        })
    });
    g.finish();
}

criterion_group!(benches, exportar_validar);
criterion_main!(benches);
