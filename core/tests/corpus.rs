//! Corpus hostil: nada aqui pode derrubar o processo, e as linhas saem como estão no arquivo.

use std::io::Write;
use std::path::PathBuf;

use nucleo::ArquivoLinhas;

fn abrir(nome: &str) -> ArquivoLinhas {
    let caminho: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "corpus", nome]
        .iter()
        .collect();
    ArquivoLinhas::abrir(&caminho).unwrap_or_else(|e| panic!("abrir {nome}: {e}"))
}

#[test]
fn vazio() {
    let a = abrir("vazio.jsonl");
    assert_eq!(a.linhas(), 0);
    assert!(a.janela(0, 10).is_empty());
    assert!(a.janela(usize::MAX, usize::MAX).is_empty());
}

#[test]
fn truncado_mantem_a_linha_incompleta() {
    let a = abrir("truncado.jsonl");
    assert_eq!(a.linhas(), 2);
    assert_eq!(
        a.janela(1, 1),
        [&br#"{"tipo":"pedido","id":2,"cliente":"Ana Exem"#[..]]
    );
}

#[test]
fn utf8_invalido_passa_cru() {
    let a = abrir("utf8-invalido.jsonl");
    assert_eq!(a.linhas(), 3);
    let meio = a.janela(1, 1)[0];
    assert!(std::str::from_utf8(meio).is_err());
    assert!(meio.starts_with(br#"{"tipo":"evento","id":2"#));
    assert_eq!(
        a.janela(2, 5),
        [&br#"{"tipo":"evento","id":3,"mensagem":"fim"}"#[..]]
    );
}

#[test]
fn bom_fica_fora_da_primeira_linha() {
    let a = abrir("bom.jsonl");
    assert_eq!(
        a.janela(0, 10),
        [
            &br#"{"tipo":"sensor","id":1}"#[..],
            br#"{"tipo":"sensor","id":2}"#
        ]
    );
}

#[test]
fn crlf_sem_quebra_final() {
    let a = abrir("crlf.jsonl");
    assert_eq!(
        a.janela(0, 10),
        [
            &br#"{"tipo":"pedido","id":1}"#[..],
            br#"{"tipo":"pedido","id":2}"#,
            b"",
            br#"{"tipo":"pedido","id":3}"#,
        ]
    );
}

#[test]
fn linha_muito_longa() {
    // 64 MB numa linha só, entre duas linhas curtas.
    let mut f = tempfile::NamedTempFile::new().unwrap();
    let longa = vec![b'x'; 64 << 20];
    f.write_all(b"{}\n\"").unwrap();
    f.write_all(&longa).unwrap();
    f.write_all(b"\"\n{}").unwrap();
    f.flush().unwrap();
    let a = ArquivoLinhas::abrir(f.path()).unwrap();
    assert_eq!(a.linhas(), 3);
    let j = a.janela(0, 3);
    assert_eq!(j[1].len(), longa.len() + 2);
    assert_eq!((j[0], j[2]), (&b"{}"[..], &b"{}"[..]));
}

#[test]
fn caminho_inexistente_e_pasta_viram_erro() {
    assert!(ArquivoLinhas::abrir("/caminho/que/nao/existe.jsonl").is_err());
    let pasta = env!("CARGO_MANIFEST_DIR");
    // Abrir uma pasta não pode entrar em pânico: erro ou zero linhas, conforme o sistema.
    let _ = ArquivoLinhas::abrir(pasta);
}
