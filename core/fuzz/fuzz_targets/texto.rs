#![no_main]
//! Arquivo e janela quaisquer: a visão Texto não entra em pânico; o cabeçalho traz o tamanho
//! de cada linha da janela, cada linha sai como prefixo da original com no máximo `max` bytes,
//! inteira se couber, e linha UTF-8 válida não é partida no meio de um caractere.
//! Entrada: [início, quantidade, max (2 bytes), arquivo…].

use libfuzzer_sys::fuzz_target;
use nucleo::{ArquivoLinhas, texto};

fuzz_target!(|entrada: &[u8]| {
    let [ini, qtd, m0, m1, dados @ ..] = entrada else {
        return;
    };
    let (ini, qtd, max) = (
        *ini as usize,
        *qtd as usize,
        u16::from_be_bytes([*m0, *m1]) as usize,
    );
    // SAFETY: o arquivo morre no fim desta closure, enquanto `dados` vive.
    let estatico = unsafe { std::mem::transmute::<&[u8], &'static [u8]>(dados) };
    let arquivo = ArquivoLinhas::estatico(estatico);
    let saida = texto::janela(&arquivo, ini, qtd, max);
    let originais = arquivo.janela(ini, qtd);
    assert_eq!(
        originais.len(),
        qtd.min(arquivo.linhas().saturating_sub(ini))
    );

    let mut partes = saida.split(|&b| b == b'\n');
    let cabecalho = partes.next().unwrap();
    let tamanhos: Vec<usize> = if cabecalho.is_empty() {
        Vec::new()
    } else {
        std::str::from_utf8(cabecalho)
            .unwrap()
            .split(' ')
            .map(|t| t.parse().unwrap())
            .collect()
    };
    let cortadas: Vec<&[u8]> = partes.collect();
    assert_eq!(tamanhos.len(), originais.len());
    assert_eq!(cortadas.len(), originais.len());
    for ((o, c), t) in originais.iter().zip(&cortadas).zip(&tamanhos) {
        assert_eq!(o.len(), *t);
        assert!(o.starts_with(c));
        assert!(c.len() <= max || o.len() <= max);
        if o.len() <= max {
            assert_eq!(c.len(), o.len());
        }
        if std::str::from_utf8(o).is_ok() {
            assert!(std::str::from_utf8(c).is_ok());
        }
    }
});
