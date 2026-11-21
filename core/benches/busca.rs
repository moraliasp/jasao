//! Busca sobre JSONL gerado em memória: tempo até o primeiro achado e até o fim.
//! Metas do plano (1 GB): primeiro < 200 ms, fim < 3 s. Tamanho: `NUCLEO_BENCH_MB` (padrão 1024).

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nucleo::Busca;

const CASOS: &[(&str, &str, bool, bool)] = &[
    ("literal", "tempo esgotado", false, false),
    ("literal_ausente", "agulha inexistente", false, false),
    ("ignorar_caixa", "TEMPO ESGOTADO", false, true),
    ("regex", r#""origem":"192\.0\.2\.25\d""#, true, false),
    ("regex_ausente", r#""qtd":\d{3}"#, true, false),
];

fn busca(c: &mut Criterion) {
    let mb: u64 = std::env::var("NUCLEO_BENCH_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024);
    let mut dados = Vec::with_capacity((mb << 20) as usize + 1024);
    nucleo::gerador::gerar(&mut dados, mb << 20, 1).expect("gerar em memória");

    let mut g = c.benchmark_group(format!("busca_{mb}MB"));
    g.sample_size(10);
    for &(nome, padrao, regex, caixa) in CASOS {
        let b = Busca::nova(padrao, regex, caixa).unwrap();
        g.throughput(Throughput::Bytes(dados.len() as u64));
        g.bench_function(format!("{nome}/fim"), |bch| {
            bch.iter(|| b.executar(black_box(&dados), &AtomicBool::new(false), |_| {}))
        });
        if nome.ends_with("ausente") {
            continue;
        }
        g.throughput(Throughput::Elements(1));
        g.bench_function(format!("{nome}/primeiro"), |bch| {
            bch.iter_custom(|iters| {
                let mut soma = Duration::ZERO;
                for _ in 0..iters {
                    let cancelar = AtomicBool::new(false);
                    let t = Instant::now();
                    let mut primeiro = None;
                    b.executar(&dados, &cancelar, |l| {
                        if primeiro.is_none() && !l.achados.is_empty() {
                            primeiro = Some(t.elapsed());
                            cancelar.store(true, Relaxed);
                        }
                    });
                    soma += primeiro.expect("o padrão aparece no arquivo");
                }
                soma
            })
        });
    }
    g.finish();
}

criterion_group!(benches, busca);
criterion_main!(benches);
