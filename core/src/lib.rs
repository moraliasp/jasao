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
pub mod formatos;
pub mod gerador;
pub mod linha_do_tempo;
pub mod linhas;
pub mod perfil;
pub mod regua;
pub mod resumo;
pub mod tabela;
pub mod texto;
pub mod validar;
pub mod valores;
pub mod vivo;

pub use arvore::Arvore;
pub use busca::{Achado, Busca, ErroBusca, Lote, Resumo};
pub use compactado::{Formato, Opcoes};
pub use linhas::{ArquivoLinhas, IndiceLinhas};
pub use vivo::{Marca, Mudanca, Vivo};
