//! `jasao diff` pelo binário: os códigos de saída que um gate de CI lê.

use std::path::PathBuf;
use std::process::Command;

struct Arquivo(PathBuf);

impl Arquivo {
    fn novo(nome: &str, conteudo: &str) -> Self {
        let p = std::env::temp_dir().join(format!("jasao-diff-{}-{nome}", std::process::id()));
        std::fs::write(&p, conteudo).unwrap();
        Arquivo(p)
    }
}

impl Drop for Arquivo {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// (código, stdout, stderr)
fn jasao(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_jasao"))
        .arg("diff")
        .args(args)
        .output()
        .unwrap();
    let s = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (o.status.code().unwrap(), s(o.stdout), s(o.stderr))
}

#[test]
fn codigos_de_saida() {
    let base = Arquivo::novo("base", "{\"a\":1}\n{\"a\":2}\n");
    let quebra = Arquivo::novo("quebra", "{\"a\":1}\n{\"a\":null}\n");
    let compat = Arquivo::novo("compat", "{\"a\":1,\"n\":true}\n{\"a\":2}\n");
    let invalida = Arquivo::novo("invalida", "{\"a\":1}\n{\"a\":\n{\"a\":2}\n");
    let p = |a: &Arquivo| a.0.to_str().unwrap().to_owned();
    let (b, q, c, i) = (p(&base), p(&quebra), p(&compat), p(&invalida));

    // Alargar (nulo novo) é quebra.
    let (codigo, out, _) = jasao(&[&b, &q, "--falha-em", "quebra"]);
    assert_eq!(codigo, 1, "{out}");
    assert!(out.starts_with("quebra      .a  tipo alargado"), "{out}");
    assert_eq!(jasao(&[&b, &q]).0, 0, "sem a flag nunca falha");
    assert_eq!(jasao(&["--falha-em", "qualquer", &b, &q]).0, 1);

    // Só compatível: `quebra` passa, `qualquer` pega.
    assert_eq!(jasao(&[&b, &c, "--falha-em", "quebra"]).0, 0);
    assert_eq!(jasao(&[&b, &c, "--falha-em", "qualquer"]).0, 1);
    assert_eq!(jasao(&[&b, &b, "--falha-em", "qualquer"]).0, 0);

    // Uso errado.
    for args in [
        &[b.as_str()][..],
        &[&b, &c, &b],
        &[&b, &c, "--falha-em"],
        &[&b, &c, "--falha-em", "talvez"],
        &[&b, &c, "--outra"],
    ] {
        assert_eq!(jasao(args).0, 2, "{args:?}");
    }

    // Arquivo que não abre: nem 0, nem 1 (não é "quebra").
    let (codigo, _, err) = jasao(&[&b, "/nao/existe.jsonl", "--falha-em", "quebra"]);
    assert_eq!(codigo, 3, "{err}");
    assert!(err.starts_with("erro: "), "{err}");

    // Registro inválido: com `--falha-em` não dá verde; sem a flag, aviso no stdout.
    let (codigo, _, err) = jasao(&[&b, &i, "--falha-em", "quebra"]);
    assert_eq!(codigo, 3, "{err}");
    assert!(err.contains("registros inválidos"), "{err}");
    assert_eq!(jasao(&[&i, &b, "--falha-em", "qualquer"]).0, 3);
    let (codigo, out, _) = jasao(&[&b, &i]);
    assert_eq!(codigo, 0);
    assert!(
        out.ends_with(
            "aviso: registros inválidos ignorados (antes: 0, depois: 1); o diff não os vê\n"
        ),
        "{out}"
    );
    let (_, json, _) = jasao(&[&b, &i, "--json"]);
    assert!(
        json.contains(r#""invalidas":{"antes":0,"depois":1}"#),
        "{json}"
    );
}
