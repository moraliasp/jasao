#![no_main]
//! Qualquer entrada: detectar e descompactar não entram em pânico e respeitam o teto de saída.
//! E ida e volta: o que sai de gzip/zstd de `dados` volta igual a `dados`.

use std::io::Write;

use libfuzzer_sys::fuzz_target;
use nucleo::compactado::{Formato, Opcoes, descompactar, detectar};

const TETO: u64 = 4 << 20;

fuzz_target!(|dados: &[u8]| {
    let op = Opcoes { max_saida: TETO, ..Opcoes::default() };
    let formato = detectar(dados);
    let mut saida = Vec::new();
    if let Ok(n) = descompactar(dados, formato, dados.len() as u64, &mut saida, &op) {
        assert_eq!(n, saida.len() as u64);
        assert!(n <= TETO);
        if formato == Formato::Cru {
            assert_eq!(saida, dados);
        }
    }

    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(dados).unwrap();
    let gz = gz.finish().unwrap();
    let zst = zstd::encode_all(dados, 1).unwrap();
    for (f, c) in [(Formato::Gzip, gz), (Formato::Zstd, zst)] {
        assert_eq!(detectar(&c), f);
        let mut volta = Vec::new();
        descompactar(&c[..], f, c.len() as u64, &mut volta, &op).unwrap();
        assert_eq!(volta, dados);
    }
});
