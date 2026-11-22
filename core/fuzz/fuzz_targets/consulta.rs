#![no_main]
//! Filtro qualquer: compilar não entra em pânico (vira filtro ou erro com posição válida).
//! Arquivo qualquer com filtros fixos e seguros: a varredura cobre todas as linhas, os
//! resultados vêm em ordem de linha e cada um é JSON que a própria consulta relê.
//! Filtro arbitrário não roda: `def f: f; f` gira ou estoura a pilha (limite documentado).
//! Entrada: [modo, tamanho do bloco, tamanho do filtro, filtro…, arquivo…].

use std::sync::atomic::AtomicBool;

use libfuzzer_sys::fuzz_target;
use nucleo::IndiceLinhas;
use nucleo::consulta::Consulta;

const FIXOS: &[&str] = &[".", "..", "keys?", "tostring", ".[]?", "[paths]", "tojson | fromjson"];

fuzz_target!(|entrada: &[u8]| {
    let [modo, bloco, n, resto @ ..] = entrada else { return };
    let (filtro, dados) = resto.split_at((*n as usize).min(resto.len()));

    if let Ok(filtro) = std::str::from_utf8(filtro) {
        if let Err(e) = Consulta::nova(filtro) {
            assert!(!e.falhas.is_empty());
            for f in &e.falhas {
                assert!(f.faixa.end <= filtro.len() && f.linha >= 1 && f.coluna >= 1);
            }
        }
    }

    let consulta = Consulta::nova(FIXOS[*modo as usize % FIXOS.len()])
        .unwrap()
        .com_bloco(*bloco as usize)
        .com_max_lote(7);
    let mut saida = Vec::new();
    let r = consulta.executar_jsonl(dados, &AtomicBool::new(false), |l| {
        assert!(l.resultados.len() <= 7);
        saida.extend(l.resultados);
    });
    assert!(!r.cancelado);
    assert_eq!(r.total_linhas, IndiceLinhas::construir(dados).len());
    assert_eq!(r.linhas, r.total_linhas);
    assert_eq!(r.resultados, saida.len() as u64);
    assert!(saida.windows(2).all(|w| w[0].linha <= w[1].linha));

    let mut jsonl = String::new();
    for s in &saida {
        jsonl.push_str(&s.json);
        jsonl.push('\n');
    }
    let eco = Consulta::nova(".").unwrap();
    let r = eco.executar_jsonl(jsonl.as_bytes(), &AtomicBool::new(false), |_| {});
    assert_eq!(r.erros, 0, "{:?}", r.primeiros_erros.first());
});
