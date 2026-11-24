//! Núcleo do visualizador: leitura e índices de JSON/JSONL, sem dependência de interface.

pub mod arvore;
pub mod busca;
pub mod caminho;
pub mod compactado;
pub mod consulta;
pub mod diff;
pub mod erros;
pub mod estrutura;
pub mod exportar;
pub mod gerador;
pub mod linhas;
pub mod regua;
pub mod resumo;
pub mod tabela;
pub mod texto;
pub mod validar;
pub mod vivo;

pub use arvore::Arvore;
pub use busca::{Achado, Busca, ErroBusca, Lote, Resumo};
pub use compactado::{Formato, Opcoes};
pub use linhas::{ArquivoLinhas, IndiceLinhas};
pub use vivo::{Mudanca, Vivo};
