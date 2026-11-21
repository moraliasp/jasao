//! Janela de linhas cruas para a visão Texto: bytes do arquivo, sem copiar nem reformatar.

use crate::ArquivoLinhas;

/// Corte por linha na tela; o resto vira "… +N bytes" (linha de 1 GB não vai inteira).
pub const MAX_COLUNAS: usize = 4096;

/// Linhas `inicio..inicio+quantidade` num buffer só: a primeira linha traz o tamanho completo
/// de cada linha em bytes, separados por espaço; depois vêm as linhas, cortadas em `max` bytes
/// sem partir caractere UTF-8, separadas por `\n`.
pub fn janela(arquivo: &ArquivoLinhas, inicio: usize, quantidade: usize, max: usize) -> Vec<u8> {
    let linhas = arquivo.janela(inicio, quantidade);
    let tamanhos: Vec<String> = linhas.iter().map(|l| l.len().to_string()).collect();
    let mut saida = tamanhos.join(" ").into_bytes();
    for l in linhas {
        saida.push(b'\n');
        saida.extend_from_slice(&l[..corte(l, max)]);
    }
    saida
}

/// Maior `n <= max` que não cai no meio de um caractere UTF-8.
fn corte(l: &[u8], max: usize) -> usize {
    if l.len() <= max {
        return l.len();
    }
    let mut n = max;
    while n > 0 && max - n < 3 && l[n] & 0xC0 == 0x80 {
        n -= 1;
    }
    n
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::io::Write;

    #[test]
    fn cabecalho_e_corte() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all("{\"a\":\"ção\"}\r\n[]\n\n".as_bytes()).unwrap();
        let arq = ArquivoLinhas::abrir(f.path()).unwrap();
        assert_eq!(
            janela(&arq, 0, 10, 100),
            "13 2 0\n{\"a\":\"ção\"}\n[]\n".as_bytes()
        );
        // Corte em 7 cai no meio do `ç` (bytes 6 e 7): recua para 6.
        assert_eq!(janela(&arq, 0, 1, 7), "13\n{\"a\":\"".as_bytes());
        assert_eq!(janela(&arq, 1, 1, 0), b"2\n");
        assert_eq!(janela(&arq, 9, 1, 10), b"");
    }
}
