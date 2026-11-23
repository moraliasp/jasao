#![no_main]
//! Arquivo qualquer: a régua (inválidas marcadas como na árvore, variantes pintadas pelo
//! discriminador escolhido) não entra em pânico; inválida continua inválida, as cores ficam em
//! `NEUTRO..=OUTROS`, a contagem de cada cor da legenda bate, e baldes e janela batem com as
//! marcas. Entrada: [nº de baldes, início da janela, tamanho da janela, arquivo…].

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use libfuzzer_sys::fuzz_target;
use nucleo::arvore::Tipo;
use nucleo::regua::{INVALIDA, OUTROS, baldes, colorir, escolher, janela, legenda, marcas};
use nucleo::{ArquivoLinhas, Arvore, estrutura};

fuzz_target!(|entrada: &[u8]| {
    let [k, ini, qtd, dados @ ..] = entrada else {
        return;
    };
    // SAFETY: a árvore (dona do arquivo) morre no fim desta closure, enquanto `dados` vive.
    let estatico = unsafe { std::mem::transmute::<&[u8], &'static [u8]>(dados) };
    let a = Arvore::new(ArquivoLinhas::estatico(estatico));
    let n = a.registros();
    let m = marcas(n);
    // A 1ª passada (`resumo::varrer`) dorme 50 ms por chamada; a regra é a mesma da árvore.
    let invalidas: Vec<bool> = a
        .janela(0, n)
        .iter()
        .map(|l| l.tipo == Tipo::Erro)
        .collect();
    for (r, &ruim) in invalidas.iter().enumerate() {
        if ruim {
            m[r].store(INVALIDA, Relaxed);
        }
    }

    let nao = AtomicBool::new(false);
    let inf = estrutura::inferir(dados, &nao, |_| {});
    let ds = inf.resumo.discriminadores();
    if let Some(d) = escolher(&ds) {
        assert!(colorir(a.arquivo(), d, &m, &nao));
        let leg = legenda(d);
        assert!(leg.variantes.len() <= OUTROS as usize - 1);
        if inf.entrada == estrutura::Entrada::Jsonl {
            for (i, (_, total)) in leg.variantes.iter().enumerate() {
                let cor = i as u8 + 1;
                let pintadas = m.iter().filter(|x| x.load(Relaxed) == cor).count() as u64;
                assert!(pintadas <= *total, "cor {cor}: {pintadas} > {total}");
            }
        }
    }
    let todas = janela(&m, 0, n);
    assert_eq!(todas.len(), n);
    for (r, &c) in todas.iter().enumerate() {
        assert_eq!(c == INVALIDA, invalidas[r]);
        assert!(c == INVALIDA || c <= OUTROS);
    }

    let b = baldes(&m, *k as usize);
    assert_eq!(b.cores.len(), (*k as usize).min(n));
    assert!(b.cores.iter().all(|&c| c <= OUTROS));
    let ruins: usize = b.invalidas.iter().map(|x| x.1).sum();
    if !b.cores.is_empty() {
        assert_eq!(ruins, invalidas.iter().filter(|&&x| x).count());
    }
    for &(_, _, primeira) in &b.invalidas {
        assert!(invalidas[primeira]);
    }
    let j = janela(&m, *ini as usize, *qtd as usize);
    assert_eq!(
        j,
        todas
            .get(*ini as usize..)
            .map_or(&[][..], |t| &t[..t.len().min(*qtd as usize)])
    );
});
