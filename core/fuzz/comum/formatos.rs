//! Comum aos alvos de `formatos`: converter qualquer entrada não entra em pânico, respeita os
//! tetos e, quando dá certo, sai JSONL válido (uma linha JSON por documento).

use nucleo::compactado::Opcoes;
use nucleo::formatos::{Entrada, Limites, converter};

const TETO: u64 = 1 << 20;

pub fn verificar(dados: &[u8], entrada: Entrada) {
    let op = Opcoes {
        max_saida: TETO,
        formatos: Limites {
            profundidade: 64,
            nos: 100_000,
            nos_alias: 10_000,
            bytes_alias: TETO,
            max_ancora: 64 << 10,
            max_chave: 4 << 10,
            max_texto: TETO,
            max_toml: TETO,
        },
        ..Opcoes::default()
    };
    let mut saida = Vec::new();
    if let Ok(n) = converter(dados, entrada, &mut saida, &op) {
        assert_eq!(n, saida.len() as u64);
        assert!(n <= TETO);
        for linha in saida.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
            if let Err(e) = serde_json::from_slice::<serde_json::Value>(linha) {
                panic!("{entrada:?} gerou JSON inválido ({e}): {}", String::from_utf8_lossy(linha));
            }
        }
    }
}
