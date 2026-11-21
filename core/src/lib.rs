//! Núcleo do visualizador: leitura e índices de JSON/JSONL, sem dependência de interface.

pub mod arvore;
pub mod busca;
pub mod caminho;
pub mod compactado;
pub mod erros;
pub mod gerador;
pub mod linhas;

pub use arvore::Arvore;
pub use busca::{Achado, Busca, ErroBusca, Lote, Resumo};
pub use compactado::{Formato, Opcoes};
pub use linhas::{ArquivoLinhas, IndiceLinhas};
