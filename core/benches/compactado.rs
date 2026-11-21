//! Vazão da descompactação (MB/s de saída) sobre JSONL gerado, em memória, saída descartada.
//! Mede só o decodificador mais os limites; o custo de gravar o cache em disco fica de fora.
//! Tamanho descompactado: `NUCLEO_BENCH_MB` (padrão 256). gzip nível 6, zstd nível 3.

use std::io::{self, Write};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nucleo::compactado::{Formato, Opcoes, descompactar};

fn compactado(c: &mut Criterion) {
    let mb: u64 = std::env::var("NUCLEO_BENCH_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256);
    let mut dados = Vec::with_capacity((mb << 20) as usize + 1024);
    nucleo::gerador::gerar(&mut dados, mb << 20, 1).expect("gerar em memória");

    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&dados).unwrap();
    let gz = gz.finish().unwrap();
    let zst = zstd::encode_all(&dados[..], 3).unwrap();
    let op = Opcoes::default();

    let mut g = c.benchmark_group("descompactar");
    g.throughput(Throughput::Bytes(dados.len() as u64));
    g.sample_size(10);
    for (nome, f, comp) in [("gzip", Formato::Gzip, gz), ("zstd", Formato::Zstd, zst)] {
        g.bench_function(format!("{nome}_{mb}MB"), |b| {
            b.iter(|| descompactar(&comp[..], f, comp.len() as u64, io::sink(), &op).unwrap())
        });
    }
    g.finish();
}

criterion_group!(benches, compactado);
criterion_main!(benches);
