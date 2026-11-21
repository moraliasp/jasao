//! Índice de linhas de JSONL: offsets de início de cada linha, sem copiar o arquivo.
//!
//! Regras (iguais às de `str::lines`, mais BOM):
//! - quebra é `\n`; um `\r` logo antes dela sai da linha (CRLF); `\r` solto é dado;
//! - BOM UTF-8 no início do arquivo não faz parte da primeira linha;
//! - quebra final não cria linha vazia; arquivo vazio (ou só BOM) tem zero linhas.

use std::fs::File;
use std::io::{self, Read};
use std::ops::Range;
use std::path::Path;

use memmap2::Mmap;

use crate::compactado::{self, Formato, Opcoes};

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Onde começa e termina cada linha de um buffer. Não guarda os bytes.
#[derive(Debug, Clone, Default)]
pub struct IndiceLinhas {
    inicios: Vec<usize>,
    /// Fim (exclusivo) da última linha, já sem a quebra final.
    fim: usize,
}

impl IndiceLinhas {
    pub fn construir(dados: &[u8]) -> Self {
        let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
        let corpo = &dados[base..];
        if corpo.is_empty() {
            return Self::default();
        }
        // shortcut: varredura de uma thread (memchr, vários GB/s); paralelizar com rayon se o
        // benchmark de 1 GB passar da meta de 2 s.
        let mut inicios = Vec::with_capacity(corpo.len() / 256 + 1);
        inicios.push(base);
        inicios.extend(memchr::memchr_iter(b'\n', corpo).map(|p| base + p + 1));
        let termina_em_quebra = inicios.last() == Some(&dados.len());
        if termina_em_quebra {
            inicios.pop();
        }
        let fim = dados.len() - usize::from(termina_em_quebra);
        Self { inicios, fim }
    }

    pub fn len(&self) -> usize {
        self.inicios.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inicios.is_empty()
    }

    /// Faixa de bytes da linha `i` (sem `\n` nem `\r` final), ou `None` fora do índice.
    pub fn faixa(&self, i: usize, dados: &[u8]) -> Option<Range<usize>> {
        let inicio = *self.inicios.get(i)?;
        let mut fim = match self.inicios.get(i + 1) {
            Some(prox) => prox - 1,
            None => self.fim,
        };
        // `\r` só sai se vier antes de um `\n` (CRLF); no fim sem quebra ele é dado.
        if fim > inicio && fim < dados.len() && dados[fim - 1] == b'\r' {
            fim -= 1;
        }
        Some(inicio..fim)
    }
}

/// Arquivo JSONL aberto por `mmap`, com o índice de linhas pronto.
pub struct ArquivoLinhas {
    // Arquivo vazio não é mapeado: mmap de tamanho zero falha em algumas plataformas.
    // shortcut: se outro processo truncar o arquivo enquanto ele está mapeado, o acesso pode
    // gerar SIGBUS; tratar junto com "arquivo que muda por baixo" (plano, seção 4).
    mapa: Option<Mmap>,
    indice: IndiceLinhas,
}

impl ArquivoLinhas {
    /// Abre JSONL cru, gzip ou zstd (pelos bytes mágicos), com as opções padrão.
    pub fn abrir(caminho: impl AsRef<Path>) -> io::Result<Self> {
        Self::abrir_com(caminho, &Opcoes::default())
    }

    /// Como `abrir`, com cache, limites, progresso e cancelamento escolhidos por quem chama.
    /// Compactado é descompactado para o cache e o cache é que é mapeado.
    pub fn abrir_com(caminho: impl AsRef<Path>, op: &Opcoes) -> io::Result<Self> {
        let caminho = caminho.as_ref();
        let mut arquivo = File::open(caminho)?;
        let mut magico = [0u8; 4];
        let n = arquivo.by_ref().take(4).read(&mut magico)?;
        let arquivo = match compactado::detectar(&magico[..n]) {
            Formato::Cru => arquivo,
            f => File::open(compactado::para_cache(caminho, f, op)?)?,
        };
        let mapa = if arquivo.metadata()?.len() == 0 {
            None
        } else {
            // SAFETY: mapeamento só leitura; ver a nota sobre truncamento no campo `mapa`.
            Some(unsafe { Mmap::map(&arquivo)? })
        };
        let indice = IndiceLinhas::construir(mapa.as_deref().unwrap_or_default());
        Ok(Self { mapa, indice })
    }

    pub fn bytes(&self) -> &[u8] {
        self.mapa.as_deref().unwrap_or_default()
    }

    pub fn linhas(&self) -> usize {
        self.indice.len()
    }

    /// Até `quantidade` linhas cruas a partir de `inicio`; vazio se `inicio` passar do fim.
    pub fn janela(&self, inicio: usize, quantidade: usize) -> Vec<&[u8]> {
        let dados = self.bytes();
        let fim = inicio.saturating_add(quantidade).min(self.indice.len());
        (inicio..fim)
            .filter_map(|i| self.indice.faixa(i, dados).map(|r| &dados[r]))
            .collect()
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn linhas(dados: &[u8]) -> Vec<&[u8]> {
        let ind = IndiceLinhas::construir(dados);
        (0..ind.len())
            .map(|i| &dados[ind.faixa(i, dados).unwrap()])
            .collect()
    }

    #[test]
    fn casos_de_borda() {
        let casos: &[(&[u8], &[&[u8]])] = &[
            (b"", &[]),
            (b"\xEF\xBB\xBF", &[]),
            (b"\n", &[b""]),
            (b"a", &[b"a"]),
            (b"a\n", &[b"a"]),
            (b"a\nb", &[b"a", b"b"]),
            (b"a\nb\n", &[b"a", b"b"]),
            (b"a\n\nb", &[b"a", b"", b"b"]),
            (b"a\r\nb\r\n", &[b"a", b"b"]),
            (b"a\r\nb", &[b"a", b"b"]),
            (b"\r\n", &[b""]),
            (b"\r", &[b"\r"]),
            (b"a\r", &[b"a\r"]),
            (b"a\rb\n", &[b"a\rb"]),
            (b"\xEF\xBB\xBF{}\n{}", &[b"{}", b"{}"]),
            (b"\xEF\xBB\xBF\n", &[b""]),
            // BOM só conta no início do arquivo.
            (b"a\n\xEF\xBB\xBFb", &[b"a", b"\xEF\xBB\xBFb"]),
        ];
        for (entrada, esperado) in casos {
            assert_eq!(&linhas(entrada), esperado, "entrada {entrada:?}");
        }
    }

    #[test]
    fn faixa_fora_do_indice() {
        let ind = IndiceLinhas::construir(b"a\n");
        assert_eq!(ind.faixa(1, b"a\n"), None);
    }
}
