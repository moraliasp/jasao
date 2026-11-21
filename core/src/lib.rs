//! Núcleo do visualizador: leitura e índices de JSON/JSONL, sem dependência de interface.

pub mod gerador;
pub mod linhas;

pub use linhas::{ArquivoLinhas, IndiceLinhas};
