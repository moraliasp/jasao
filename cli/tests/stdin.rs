//! `-` (e pipe sem arquivo) lê stdin pelo binário: JSON, JSONL, vazio, inválido e o temporário
//! que some ao sair, mesmo em erro.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Pasta própria para o cache (HOME e XDG_CACHE_HOME apontam para ela): dá para conferir que o
/// temporário não ficou.
struct Casa(PathBuf);

impl Casa {
    fn nova(nome: &str) -> Self {
        let p = std::env::temp_dir().join(format!("jasao-stdin-{}-{nome}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Casa(p)
    }

    /// (código, stdout, stderr), com `entrada` no stdin.
    fn jasao(&self, args: &[&str], entrada: &[u8]) -> (i32, String, String) {
        let mut filho = Command::new(env!("CARGO_BIN_EXE_jasao"))
            .args(args)
            .env("HOME", &self.0)
            .env("XDG_CACHE_HOME", self.0.join(".cache"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Quem sai cedo (erro) fecha o pipe antes: escrever pode falhar, o que não é o teste.
        let _ = filho.stdin.take().unwrap().write_all(entrada);
        let o = filho.wait_with_output().unwrap();
        let s = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
        (o.status.code().unwrap(), s(o.stdout), s(o.stderr))
    }

    /// Arquivos que sobraram dentro da casa (o temporário de stdin não pode estar entre eles).
    fn sobras(&self) -> Vec<PathBuf> {
        fn andar(d: &Path, v: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                if e.file_type().unwrap().is_dir() {
                    andar(&e.path(), v);
                } else {
                    v.push(e.path());
                }
            }
        }
        let mut v = Vec::new();
        andar(&self.0, &mut v);
        v
    }
}

impl Drop for Casa {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn pipe_de_jsonl_com_traco_e_sem_arquivo() {
    let casa = Casa::nova("jsonl");
    let jsonl = b"{\"a\":1}\n{\"a\":2,\"b\":\"x\"}\n";
    let (c, md, _) = casa.jasao(&["laudo", "-"], jsonl);
    assert_eq!(c, 0);
    assert!(md.starts_with("# Laudo de `-`"), "{md}");
    assert!(md.contains("2 registros"), "{md}");
    // O nome do temporário não vaza: os próximos passos falam em `-`.
    assert!(!md.contains(".parcial"), "{md}");

    let (c, out, _) = casa.jasao(&["jq", ".a"], jsonl);
    assert_eq!((c, out.as_str()), (0, "1\n2\n"));
    let (c, out, _) = casa.jasao(&["linhas"], jsonl);
    assert_eq!(c, 0);
    assert!(out.starts_with("2 linhas"), "{out}");
    // Em `buscar`, `-` no lugar do padrão é o padrão.
    let (c, out, _) = casa.jasao(&["buscar", "-", "\"b\""], jsonl);
    assert_eq!(c, 0);
    assert!(out.starts_with("2:"), "{out}");
    assert!(casa.sobras().is_empty(), "{:?}", casa.sobras());
}

#[test]
fn pipe_de_json_em_varias_linhas() {
    let casa = Casa::nova("json");
    let json = b"{\n  \"a\": [1, 2],\n  \"b\": {\"c\": true}\n}\n";
    let (c, out, _) = casa.jasao(&["jq", "--documento", ".a[1]", "-"], json);
    assert_eq!((c, out.as_str()), (0, "2\n"));
    let (c, out, _) = casa.jasao(&["inferir", "-"], json);
    assert_eq!(c, 0);
    assert!(out.contains(".b.c"), "{out}");
    // Um lado do diff por stdin.
    let (c, _, _) = casa.jasao(&["diff", "-", "-"], json);
    assert_eq!(c, 3, "só um `-` lê stdin; o outro não abre");
    assert!(casa.sobras().is_empty(), "{:?}", casa.sobras());
}

#[test]
fn stdin_vazio_sai_com_erro() {
    let casa = Casa::nova("vazio");
    for args in [&["laudo", "-"][..], &["jq", ".", "-"], &["linhas"]] {
        let (c, _, err) = casa.jasao(args, b"");
        assert_eq!(c, 1, "{args:?}");
        assert!(err.contains("stdin vazio"), "{args:?}: {err}");
    }
    let (c, _, err) = casa.jasao(&["validar", "-", "--schema", "s.json"], b"");
    assert_eq!(c, 3, "{err}");
    assert!(casa.sobras().is_empty(), "{:?}", casa.sobras());
}

#[test]
fn conteudo_invalido_e_temporario_some() {
    let casa = Casa::nova("invalido");
    let (c, out, _) = casa.jasao(&["erros", "-"], b"{\"a\":1}\n{oops\n");
    assert_eq!(c, 1);
    assert!(out.starts_with("2:"), "{out}");
    // Binário: o laudo diz e sai com 0; o jq dá erro por linha. Nenhum deixa o temporário.
    let (c, md, _) = casa.jasao(&["laudo", "-"], b"\x00\x01\x02 nada de json");
    assert_eq!(c, 0, "{md}");
    let (c, _, _) = casa.jasao(&["jq", ".", "-", "--saida", "csv"], b"ruim\n");
    assert_ne!(c, 2);
    assert!(casa.sobras().is_empty(), "{:?}", casa.sobras());
}
