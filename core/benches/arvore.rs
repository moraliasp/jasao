//! Árvore de valores: o que a interface pede a cada quadro de rolagem e a cada clique.
//! Arquivo: JSONL do gerador com `NUCLEO_BENCH_MB` (padrão 64) em disco temporário.

use criterion::{Criterion, criterion_group, criterion_main};
use nucleo::Arvore;
use std::hint::black_box;
use std::io::Write;

fn arvore(c: &mut Criterion) {
    let mb: u64 = std::env::var("NUCLEO_BENCH_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let mut f = tempfile::NamedTempFile::new().unwrap();
    let mut w = std::io::BufWriter::new(f.as_file_mut());
    nucleo::gerador::gerar(&mut w, mb << 20, 1).unwrap();
    w.flush().unwrap();
    drop(w);
    let mut a = Arvore::abrir(f.path()).unwrap();
    let meio = a.registros() / 2;

    let mut g = c.benchmark_group("arvore");
    g.bench_function("janela de 100 linhas (recolhidas)", |b| {
        b.iter(|| a.janela(black_box(meio), 100).len())
    });
    // Abre 100 registros perto do meio: a janela atravessa nós e registros abertos.
    for r in meio..meio + 100 {
        let v = a.linha_do_registro(r).unwrap();
        a.alternar(v);
    }
    let v = a.linha_do_registro(meio).unwrap();
    g.bench_function("janela de 100 linhas (100 registros abertos)", |b| {
        b.iter(|| a.janela(black_box(v), 100).len())
    });
    let v = a.linha_do_registro(meio + 200).unwrap();
    g.bench_function("expandir e recolher um registro", |b| {
        b.iter(|| {
            a.alternar(black_box(v));
            a.alternar(black_box(v))
        })
    });
    g.finish();
}

criterion_group!(benches, arvore);
criterion_main!(benches);
