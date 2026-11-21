//! Núcleo do visualizador: leitura e índices de JSON/JSONL, sem dependência de interface.

pub mod compactado;
pub mod gerador;
pub mod linhas;

pub use compactado::{Formato, Opcoes};
pub use linhas::{ArquivoLinhas, IndiceLinhas};
