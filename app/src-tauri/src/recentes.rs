//! Arquivos recentes: a única coisa que o app grava em disco (ver docs/decisoes.md).
//!
//! Uma lista JSON de caminhos em `recentes.json`, no diretório de dados do app. Não guarda
//! conteúdo de arquivo e nunca sai da máquina. A gravação é atômica (temporário ao lado +
//! `rename`), e arquivo ausente ou corrompido vira lista vazia, sem travar a abertura do app.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Quantos recentes guardar (fixo, sem configuração).
pub const LIMITE: usize = 10;

/// Lê a lista; ausente, ilegível ou corrompida vira vazia.
pub fn ler(arquivo: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(arquivo) else {
        return Vec::new();
    };
    let mut lista: Vec<String> = serde_json::from_slice(&bytes).unwrap_or_default();
    lista.truncate(LIMITE);
    lista
}

/// Grava inteira, atômica: quem lê no meio vê a lista antiga ou a nova, nunca metade.
fn gravar(arquivo: &Path, lista: &[String]) -> std::io::Result<()> {
    let pasta = arquivo.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(pasta)?;
    let mut tmp = tempfile::NamedTempFile::new_in(pasta)?;
    tmp.write_all(&serde_json::to_vec(lista).map_err(std::io::Error::other)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(arquivo).map_err(|e| e.error)?;
    Ok(())
}

/// Põe `caminho` no topo (sobe se já estava) e corta no limite. Guarda o caminho canônico: o
/// mesmo arquivo pedido por `/tmp/a` e `/private/tmp/a` (ou por link) é um recente só.
pub fn adicionar(arquivo: &Path, caminho: &str) -> std::io::Result<Vec<String>> {
    let canonico = std::fs::canonicalize(caminho).map(|p| p.to_string_lossy().into_owned());
    let caminho = canonico.as_deref().unwrap_or(caminho);
    let mut lista = ler(arquivo);
    lista.retain(|c| c != caminho);
    lista.insert(0, caminho.to_owned());
    lista.truncate(LIMITE);
    gravar(arquivo, &lista)?;
    Ok(lista)
}

/// Tira `caminho` da lista.
pub fn remover(arquivo: &Path, caminho: &str) -> std::io::Result<Vec<String>> {
    let mut lista = ler(arquivo);
    lista.retain(|c| c != caminho);
    gravar(arquivo, &lista)?;
    Ok(lista)
}

/// Esvazia a lista (apaga o arquivo: nada fica em disco).
pub fn limpar(arquivo: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(arquivo) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Um recente como a interface mostra: caminho completo, nome e pasta.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recente {
    pub caminho: String,
    pub nome: String,
    pub pasta: String,
}

pub fn descrever(lista: Vec<String>) -> Vec<Recente> {
    lista
        .into_iter()
        .map(|caminho| {
            let p = PathBuf::from(&caminho);
            let nome = p
                .file_name()
                .map_or_else(|| caminho.clone(), |n| n.to_string_lossy().into_owned());
            let pasta = p
                .parent()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default();
            Recente {
                caminho,
                nome,
                pasta,
            }
        })
        .collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    fn arquivo() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("dados").join("recentes.json");
        (d, a)
    }

    #[test]
    fn ordem_mais_recente_primeiro() {
        let (_d, a) = arquivo();
        assert!(ler(&a).is_empty(), "ausente vira vazia");
        adicionar(&a, "/x/a.json").unwrap();
        adicionar(&a, "/x/b.jsonl").unwrap();
        assert_eq!(ler(&a), ["/x/b.jsonl", "/x/a.json"]);
    }

    #[test]
    fn duplicata_sobe_para_o_topo() {
        let (_d, a) = arquivo();
        for c in ["/a", "/b", "/c"] {
            adicionar(&a, c).unwrap();
        }
        assert_eq!(adicionar(&a, "/a").unwrap(), ["/a", "/c", "/b"]);
        assert_eq!(ler(&a), ["/a", "/c", "/b"]);
    }

    #[test]
    fn mesmo_arquivo_por_link_e_um_so() {
        let (d, a) = arquivo();
        let real = d.path().join("x.json");
        std::fs::write(&real, "{}").unwrap();
        let link = d.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        adicionar(&a, real.to_str().unwrap()).unwrap();
        let l = adicionar(&a, link.to_str().unwrap()).unwrap();
        assert_eq!(l.len(), 1, "{l:?}");
        assert_eq!(l[0], std::fs::canonicalize(&real).unwrap().to_string_lossy());
    }

    #[test]
    fn limite_corta_o_mais_antigo() {
        let (_d, a) = arquivo();
        for i in 0..LIMITE + 3 {
            adicionar(&a, &format!("/{i}")).unwrap();
        }
        let l = ler(&a);
        assert_eq!(l.len(), LIMITE);
        assert_eq!(l[0], format!("/{}", LIMITE + 2));
        assert_eq!(l[LIMITE - 1], "/3");
    }

    #[test]
    fn corrompido_vira_vazio_e_se_recupera() {
        let (_d, a) = arquivo();
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        for lixo in [&b"{nao e lista"[..], b"[1,2]", b"", b"\xff\xfe"] {
            std::fs::write(&a, lixo).unwrap();
            assert!(ler(&a).is_empty(), "{lixo:?}");
        }
        assert_eq!(adicionar(&a, "/a").unwrap(), ["/a"]);
        assert_eq!(ler(&a), ["/a"]);
    }

    #[test]
    fn remover_ausente_e_limpar() {
        let (_d, a) = arquivo();
        adicionar(&a, "/a").unwrap();
        adicionar(&a, "/b").unwrap();
        assert_eq!(remover(&a, "/a").unwrap(), ["/b"]);
        assert_eq!(remover(&a, "/nunca").unwrap(), ["/b"]);
        limpar(&a).unwrap();
        assert!(!a.exists(), "limpar não deixa arquivo");
        assert!(ler(&a).is_empty());
        limpar(&a).unwrap();
    }

    #[test]
    fn gravacao_nao_deixa_temporario() {
        let (_d, a) = arquivo();
        adicionar(&a, "/a").unwrap();
        adicionar(&a, "/b").unwrap();
        let nomes: Vec<_> = std::fs::read_dir(a.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(nomes, ["recentes.json"]);
    }

    #[test]
    fn descreve_nome_e_pasta() {
        assert_eq!(
            descrever(vec!["/dados/logs/a.jsonl".into()]),
            [Recente {
                caminho: "/dados/logs/a.jsonl".into(),
                nome: "a.jsonl".into(),
                pasta: "/dados/logs".into(),
            }]
        );
    }
}
