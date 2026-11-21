//! Núcleo do visualizador: leitura e índices de JSON/JSONL, sem dependência de interface.

pub mod busca;
pub mod compactado;
pub mod estrutura;
pub mod gerador;
pub mod linhas;

pub use busca::{Achado, Busca, ErroBusca, Lote, Resumo};
pub use compactado::{Formato, Opcoes};
pub use linhas::{ArquivoLinhas, IndiceLinhas};
