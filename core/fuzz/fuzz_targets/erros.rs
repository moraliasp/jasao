#![no_main]
//! Qualquer arquivo: a varredura de linhas inválidas não entra em pânico, conta as linhas como o
//! índice, os erros vêm em ordem, dentro da linha, e batem com a validação linha a linha.
//! Entrada: [tamanho do bloco, teto, arquivo…].

use std::sync::atomic::AtomicBool;

use libfuzzer_sys::fuzz_target;
use nucleo::IndiceLinhas;
use nucleo::erros::{OpcoesErros, validar_linha, varrer};

fuzz_target!(|entrada: &[u8]| {
    let [bloco, max, dados @ ..] = entrada else { return };
    let op = OpcoesErros { bloco: *bloco as usize, max_guardados: *max as usize };
    let mut erros = Vec::new();
    let r = varrer(dados, &op, &AtomicBool::new(false), |l| erros.extend(l.erros));
    assert!(!r.cancelado && r.varridos == dados.len());

    let ind = IndiceLinhas::construir(dados);
    assert_eq!(r.linhas as usize, ind.len());
    let mut pilha = Vec::new();
    let mut esperado = Vec::new();
    let mut em_branco = 0;
    for l in 0..ind.len() {
        let faixa = ind.faixa(l, dados).unwrap();
        match validar_linha(&dados[faixa.clone()], &mut pilha) {
            Ok(false) => em_branco += 1,
            Ok(true) => {}
            Err((c, m)) => {
                assert!(c <= faixa.len());
                esperado.push((l, faixa.start, c, m));
            }
        }
    }
    assert_eq!(r.invalidas as usize, esperado.len());
    assert_eq!(r.em_branco, em_branco);
    let obtido: Vec<_> = erros.iter().map(|e| (e.linha, e.deslocamento, e.coluna, e.motivo)).collect();
    assert_eq!(obtido, esperado[..esperado.len().min(*max as usize)]);
});
