#![no_main]
//! Arquivo qualquer: a inferência não entra em pânico, não depende do tamanho do bloco
//! (`combinar` associativa e comutativa), conta todas as linhas e os discriminadores respeitam
//! o critério documentado (2 a `DISCRIMINADOR_MAX_VALORES` variantes, cobrindo os objetos).
//! Entrada: [tamanho do bloco, arquivo…].

use std::sync::atomic::AtomicBool;

use libfuzzer_sys::fuzz_target;
use nucleo::estrutura::{DISCRIMINADOR_MAX_VALORES, Entrada, Tipo, inferir_em_blocos};

fuzz_target!(|entrada: &[u8]| {
    let [bloco, dados @ ..] = entrada else { return };
    let nao = AtomicBool::new(false);
    let inteiro = inferir_em_blocos(dados, usize::MAX, &nao, |_| {});
    let fatiado = inferir_em_blocos(dados, *bloco as usize, &nao, |_| {});
    assert_eq!(inteiro, fatiado);

    let inf = inteiro;
    assert!(!inf.cancelado && inf.varridas <= inf.total);
    let r = &inf.resumo;
    if inf.entrada == Entrada::Jsonl {
        assert_eq!(inf.varridas, inf.total);
        assert_eq!(r.registros() + r.vazias + r.invalidas.n, inf.total);
    }
    let soma: u64 = r.raiz.tipos.iter().map(|c| c.n).sum();
    assert_eq!(soma, r.registros());

    let caminhos = r.caminhos();
    if r.registros() > 0 {
        assert_eq!(caminhos[0].caminho, ".");
    }

    let objetos = r.raiz.tipo(Tipo::Objeto).n;
    for d in r.discriminadores() {
        assert!((2..=DISCRIMINADOR_MAX_VALORES).contains(&d.variantes.len()));
        assert_eq!(
            d.variantes.iter().map(|v| v.registros.n).sum::<u64>(),
            objetos
        );
        for v in &d.variantes {
            assert!(v.registros.primeira().is_some_and(|l| l < inf.total));
            assert!(v.opcionais.iter().all(|(_, n)| *n < v.registros.n));
        }
    }
});
