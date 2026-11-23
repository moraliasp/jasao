#![no_main]
//! Arquivo qualquer: o "de que é feito" do painel de entrada não entra em pânico e bate com a
//! árvore (vazio/JSON único pelo nº de linhas; objetos/valores pelas linhas válidas da amostra);
//! `campos` bate com a inferência. `varrer` fica de fora: dorme 50 ms por chamada.

use std::sync::atomic::AtomicBool;

use libfuzzer_sys::fuzz_target;
use nucleo::arvore::Tipo;
use nucleo::resumo::{Conteudo, campos, conteudo};
use nucleo::{ArquivoLinhas, Arvore, estrutura};

/// `resumo::AMOSTRA` (privada).
const AMOSTRA: usize = 1_000;

fuzz_target!(|dados: &[u8]| {
    // SAFETY: a árvore (dona do arquivo) morre no fim desta closure, enquanto `dados` vive.
    let estatico = unsafe { std::mem::transmute::<&[u8], &'static [u8]>(dados) };
    let a = Arvore::new(ArquivoLinhas::estatico(estatico));
    let c = conteudo(a.arquivo());
    assert_eq!(c == Conteudo::Vazio, a.registros() == 0);
    assert_eq!(c == Conteudo::JsonUnico, a.registros() == 1);

    let validas: Vec<Tipo> = a
        .janela(0, AMOSTRA)
        .iter()
        .map(|l| l.tipo)
        .filter(|&t| t != Tipo::Erro)
        .collect();
    let objetos = validas.iter().filter(|&&t| t == Tipo::Objeto).count();
    match c {
        Conteudo::Objetos => assert!(objetos > 0 && objetos == validas.len()),
        Conteudo::Valores => assert!(objetos == 0 && !validas.is_empty()),
        _ => {}
    }

    let inf = estrutura::inferir(dados, &AtomicBool::new(false), |_| {});
    if let Some(n) = campos(&inf.resumo) {
        assert!(n > 0 && inf.resumo.raiz.tipo(estrutura::Tipo::Objeto).n > 0);
    }
});
