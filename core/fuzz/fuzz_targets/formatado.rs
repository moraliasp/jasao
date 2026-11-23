#![no_main]
//! Arquivo qualquer: formatar cada registro não entra em pânico; registro inválido dá erro
//! (o mesmo da linha na árvore), válido dá texto dentro de `MAX_FORMATADO` (UTF-8 pelo tipo).
//! Sem corte, o formatado é JSON válido de novo (numa linha só) e formatar outra vez não muda
//! nada (idempotente).

use libfuzzer_sys::fuzz_target;
use nucleo::arvore::MAX_FORMATADO;
use nucleo::{ArquivoLinhas, Arvore};

fuzz_target!(|dados: &[u8]| {
    // SAFETY: a árvore (dona do arquivo) morre no fim desta closure, enquanto `dados` vive.
    let estatico = unsafe { std::mem::transmute::<&[u8], &'static [u8]>(dados) };
    let a = Arvore::new(ArquivoLinhas::estatico(estatico));
    for r in 0..a.registros() {
        let linha = &a.janela(a.linha_do_registro(r).unwrap(), 1)[0];
        match a.formatado(r) {
            Err(e) => assert_eq!(linha.erro, Some(e)),
            Ok(f) => {
                assert!(linha.erro.is_none());
                assert!(f.texto.len() <= MAX_FORMATADO);
                if f.truncado {
                    continue;
                }
                // Texto JSON não tem `\n` cru: trocar a quebra por espaço junta numa linha só.
                let uma = f.texto.replace('\n', " ");
                // SAFETY: `b` morre antes de `uma`.
                let uma_estatico =
                    unsafe { std::mem::transmute::<&[u8], &'static [u8]>(uma.as_bytes()) };
                let b = Arvore::new(ArquivoLinhas::estatico(uma_estatico));
                let de_novo = b.formatado(0).expect("formatado é JSON válido");
                assert_eq!(de_novo, f);
            }
        }
    }
});
