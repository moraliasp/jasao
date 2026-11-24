//! `jasao laudo` pelo binário: códigos de saída e segredo que nunca aparece.

use std::path::PathBuf;
use std::process::Command;

struct Arquivo(PathBuf);

impl Arquivo {
    fn novo(nome: &str, conteudo: &[u8]) -> Self {
        let p = std::env::temp_dir().join(format!("jasao-laudo-{}-{nome}", std::process::id()));
        std::fs::write(&p, conteudo).unwrap();
        Arquivo(p)
    }
    fn p(&self) -> &str {
        self.0.to_str().unwrap()
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
        .arg("laudo")
        .args(args)
        .output()
        .unwrap();
    let s = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
    (o.status.code().unwrap(), s(o.stdout), s(o.stderr))
}

#[test]
fn codigos_de_saida() {
    // O laudo é relatório, não gate: parcial, linhas inválidas e binário saem com 0 e dizem no texto.
    let invalida = Arquivo::novo("invalida", b"{\"a\":1}\nruim\n");
    let (c, md, _) = jasao(&[invalida.p()]);
    assert_eq!(c, 0);
    assert!(md.contains("1 linha inválida"), "{md}");

    let parcial = Arquivo::novo("parcial", b"{\"a\":1}\n{\"b\":2}\n");
    let (c, md, _) = jasao(&[parcial.p(), "--teto", "0"]);
    assert_eq!(c, 0);
    // Com teto zero a leitura pode ou não começar; se parou, diz que é parcial e não afirma ausência.
    if md.contains("**Parcial:**") {
        assert!(!md.contains("Nenhum aviso"), "{md}");
    }

    let binario = Arquivo::novo("binario", &[0, 159, 146, 150, 0, 1, 2, 255, 254, 0]);
    let (c, md, _) = jasao(&[binario.p()]);
    assert_eq!(c, 0, "{md}");

    // Arquivo que não abre: 1, com o motivo no stderr.
    let (c, md, err) = jasao(&["/nao/existe/jasao-laudo"]);
    assert_eq!((c, md.as_str()), (1, ""));
    assert!(err.contains("/nao/existe/jasao-laudo"), "{err}");

    // Uso errado: 2.
    assert_eq!(jasao(&[]).0, 2);
    assert_eq!(jasao(&[parcial.p(), "--teto", "x"]).0, 2);
}

#[test]
fn segredo_com_valores_distintos_nao_aparece() {
    let f = Arquivo::novo(
        "segredo",
        b"{\"token\":\"hunter2xyz\",\"a\":1}\n{\"token\":\"s3cr3tvalue\",\"b\":2}\n",
    );
    for args in [&[f.p()][..], &[f.p(), "--json"]] {
        let (c, out, _) = jasao(args);
        assert_eq!(c, 0);
        assert!(out.contains(".token"), "{out}");
        assert!(
            !out.contains("hunter2xyz") && !out.contains("s3cr3tvalue"),
            "{out}"
        );
    }
}
