//! Escolha do parser da árvore (docs/decisoes.md): mesma tarefa nos três candidatos.
//! Tarefa: validar um registro JSONL e percorrer todos os nós com chave e texto cru do valor
//! (o que a árvore precisa para desenhar), sobre 20 mil linhas do gerador.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nucleo::IndiceLinhas;
use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::hint::black_box;

/// sonic-rs: iteradores preguiçosos sobre o texto; número fica cru.
fn sonic(raw: &str, n: &mut usize) {
    match raw.as_bytes()[0] {
        b'{' => {
            for par in sonic_rs::to_object_iter(raw) {
                let (k, v) = par.unwrap();
                *n += k.len();
                sonic(v.as_raw_str(), n);
            }
        }
        b'[' => {
            for v in sonic_rs::to_array_iter(raw) {
                sonic(v.unwrap().as_raw_str(), n);
            }
        }
        _ => *n += raw.len(),
    }
}

/// serde_json com RawValue: um nível por vez, número fica cru.
struct Filhos<'a>(Vec<(Option<Cow<'a, str>>, &'a RawValue)>);

impl<'de> serde::Deserialize<'de> for Filhos<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Filhos<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("objeto ou lista")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut v = Vec::new();
                while let Some(k) = m.next_key::<Cow<'de, str>>()? {
                    v.push((Some(k), m.next_value()?));
                }
                Ok(Filhos(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Self::Value, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = s.next_element()? {
                    v.push((None, x));
                }
                Ok(Filhos(v))
            }
        }
        d.deserialize_any(V)
    }
}

fn serde(raw: &str, n: &mut usize) {
    if !matches!(raw.as_bytes()[0], b'{' | b'[') {
        *n += raw.len();
        return;
    }
    let filhos: Filhos = serde_json::from_str(raw).unwrap();
    for (k, v) in filhos.0 {
        *n += k.map_or(0, |k| k.len());
        serde(v.get(), n);
    }
}

/// simd-json: fita (tape) num passo só, mas número vira u64/i64/f64 (perde o texto).
fn simd(buf: &mut [u8], n: &mut usize) {
    let fita = simd_json::to_tape(buf).unwrap();
    for no in fita.0 {
        if let simd_json::Node::String(s) = no {
            *n += s.len();
        } else {
            *n += 1;
        }
    }
}

fn parsers(c: &mut Criterion) {
    let mut dados = Vec::new();
    nucleo::gerador::gerar(&mut dados, 4 << 20, 1).unwrap();
    let ind = IndiceLinhas::construir(&dados);
    let linhas: Vec<&str> = (0..20_000)
        .map(|i| std::str::from_utf8(&dados[ind.faixa(i, &dados).unwrap()]).unwrap())
        .collect();
    let bytes: u64 = linhas.iter().map(|l| l.len() as u64).sum();

    let mut g = c.benchmark_group("arvore_registro");
    g.throughput(Throughput::Bytes(bytes));
    g.bench_function("sonic-rs", |b| {
        b.iter(|| {
            let mut n = 0;
            for l in &linhas {
                sonic(black_box(l), &mut n);
            }
            n
        })
    });
    g.bench_function("serde_json RawValue", |b| {
        b.iter(|| {
            let mut n = 0;
            for l in &linhas {
                serde(black_box(l), &mut n);
            }
            n
        })
    });
    let mut copias: Vec<Vec<u8>> = linhas.iter().map(|l| l.as_bytes().to_vec()).collect();
    g.bench_function("simd-json tape (com cópia)", |b| {
        b.iter(|| {
            let mut n = 0;
            for (l, c) in linhas.iter().zip(copias.iter_mut()) {
                c.copy_from_slice(l.as_bytes()); // a fita reescreve o buffer
                simd(black_box(c), &mut n);
            }
            n
        })
    });
    g.finish();
}

criterion_group!(benches, parsers);
criterion_main!(benches);
