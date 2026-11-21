#![no_main]
//! Qualquer padrão e arquivo: a busca não entra em pânico, os achados vêm em ordem e dentro da
//! linha (pelo índice), e o literal sensível à caixa bate com um oráculo linha a linha.
//! Entrada: [opções, tamanho do bloco, tamanho do padrão, padrão…, arquivo…].

use std::sync::atomic::AtomicBool;

use libfuzzer_sys::fuzz_target;
use nucleo::{Busca, IndiceLinhas};

fuzz_target!(|entrada: &[u8]| {
    let [opcoes, bloco, n, resto @ ..] = entrada else { return };
    let (padrao, dados) = resto.split_at((*n as usize).min(resto.len()));
    let Ok(padrao) = std::str::from_utf8(padrao) else { return };
    let (regex, caixa) = (opcoes & 1 != 0, opcoes & 2 != 0);
    let Ok(busca) = Busca::nova(padrao, regex, caixa) else { return };
    let busca = busca.com_bloco(*bloco as usize);

    let mut achados = Vec::new();
    let r = busca.executar(dados, &AtomicBool::new(false), |l| achados.extend(l.achados));
    assert!(!r.cancelado && r.varridos == dados.len());
    assert_eq!(r.achados, achados.len() as u64);

    let ind = IndiceLinhas::construir(dados);
    let mut anterior = None;
    for a in &achados {
        let faixa = ind.faixa(a.linha, dados).expect("linha existe");
        assert!(a.comprimento > 0 && a.coluna + a.comprimento <= faixa.len());
        assert!(anterior.is_none_or(|(l, c)| (a.linha, a.coluna) >= (l, c)));
        anterior = Some((a.linha, a.coluna + a.comprimento));
    }

    if !regex && !caixa {
        let agulha = padrao.as_bytes();
        let mut esperado = Vec::new();
        for l in 0..ind.len() {
            let linha = &dados[ind.faixa(l, dados).unwrap()];
            let mut p = 0;
            while let Some(i) = achar_ingenuo(&linha[p..], agulha) {
                esperado.push((l, p + i));
                p += i + agulha.len();
            }
        }
        let obtido: Vec<_> = achados.iter().map(|a| (a.linha, a.coluna)).collect();
        assert_eq!(obtido, esperado);
    }
});

fn achar_ingenuo(texto: &[u8], agulha: &[u8]) -> Option<usize> {
    texto.windows(agulha.len()).position(|w| w == agulha)
}
