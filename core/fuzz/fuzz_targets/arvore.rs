#![no_main]
//! Arquivo qualquer e uma sequência de cliques (expandir/recolher e revelar pela visão Texto):
//! nada entra em pânico e, depois de cada clique, a lista plana bate com `total`, cada linha
//! respeita seus cortes (prévia, chave, faixa dentro da linha do arquivo, `MAX_NOS` por
//! registro) e pai, caminho, migalhas e valor da linha clicada são coerentes.
//! Entrada: [nº de cliques, cliques de 4 bytes…, arquivo…].

use libfuzzer_sys::fuzz_target;
use nucleo::arvore::{MAX_NOS, MAX_PREVIA, Tipo};
use nucleo::{ArquivoLinhas, Arvore, IndiceLinhas};

const CORTE: usize = MAX_PREVIA + '…'.len_utf8();

fuzz_target!(|entrada: &[u8]| {
    let [n, resto @ ..] = entrada else { return };
    let (cliques, dados) = resto.split_at((*n as usize * 4).min(resto.len()));
    // SAFETY: a árvore (dona do arquivo) morre no fim desta closure, enquanto `dados` vive.
    let estatico = unsafe { std::mem::transmute::<&[u8], &'static [u8]>(dados) };
    let mut a = Arvore::new(ArquivoLinhas::estatico(estatico));
    let ind = IndiceLinhas::construir(dados);
    let tam = |r: usize| ind.faixa(r, dados).map_or(0, |f| f.len());
    assert_eq!(a.registros(), ind.len());
    conferir(&a, &tam);

    for c in cliques.chunks_exact(4) {
        if a.total() == 0 {
            break;
        }
        let x = u16::from_be_bytes([c[1], c[2]]) as usize;
        let v = if c[0] & 1 == 0 {
            let v = x % a.total();
            a.alternar(v);
            v.min(a.total() - 1)
        } else {
            let r = c[3] as usize % a.registros();
            let v = a.revelar(r, x % (tam(r) + 1)).expect("registro existe");
            assert!(v < a.total());
            assert_eq!(a.janela(v, 1)[0].registro, r);
            v
        };
        conferir(&a, &tam);

        let linha = &a.janela(v, 1)[0];
        let caminho = a.caminho(v).expect("linha visível");
        assert_eq!(caminho.0.len(), linha.prof);
        assert_eq!(a.migalhas(v).unwrap().len(), linha.prof);
        assert!(a.valor(v).is_some());
        match a.pai(v) {
            None => assert_eq!(linha.prof, 0),
            Some(p) => {
                assert!(p < v);
                let pai = &a.janela(p, 1)[0];
                assert_eq!((pai.registro, pai.prof + 1), (linha.registro, linha.prof));
                assert!(pai.aberto);
            }
        }
        let r = linha.registro;
        let lr = a.linha_do_registro(r).unwrap();
        assert!(lr <= v);
        assert_eq!(
            (a.janela(lr, 1)[0].registro, a.janela(lr, 1)[0].prof),
            (r, 0)
        );
    }
});

fn conferir(a: &Arvore, tam: &dyn Fn(usize) -> usize) {
    let linhas = a.janela(0, usize::MAX);
    assert_eq!(linhas.len(), a.total());
    assert!(a.janela(a.total(), 1).is_empty());
    let mut por_registro = 0;
    for (i, l) in linhas.iter().enumerate() {
        assert!(l.registro < a.registros());
        if l.prof == 0 {
            assert!(i == 0 || l.registro > linhas[i - 1].registro);
            por_registro = 0;
        } else {
            assert_eq!(l.registro, linhas[i - 1].registro);
            assert!(l.prof <= linhas[i - 1].prof + 1);
            por_registro += 1;
            assert!(por_registro <= MAX_NOS);
        }
        assert!(l.faixa[0] <= l.faixa[1] && l.faixa[1] <= tam(l.registro));
        assert!(l.previa.len() <= CORTE);
        assert!(l.chave.as_ref().is_none_or(|k| k.len() <= CORTE));
        assert_eq!(l.tipo == Tipo::Erro, l.erro.is_some());
        assert!(l.erro.is_none_or(|e| e.byte <= tam(l.registro)));
        assert_eq!(l.expansivel, l.filhos > 0);
        assert!(!l.aberto || l.expansivel);
        assert!(l.omitidos <= l.filhos && (l.aberto || l.omitidos == 0));
    }
}
