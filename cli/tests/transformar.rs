//! `jasao transformar` pelo binário: grava arquivo novo, recusa o original, segredo mascarado
//! por padrão e cada transformação.

use std::path::Path;
use std::process::{Command, Output};

const DADOS: &str = concat!(
    r#"{"id":1,"nome":"ana","senha":"S3NT1N3LA-1"}"#,
    "\n",
    r#"{"id":2,"nome":"bia","senha":"S3NT1N3LA-2"}"#,
    "\n",
);

fn jasao(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jasao"))
        .args(args)
        .output()
        .unwrap()
}

fn ler(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn transformar_grava_novo_mascarado_e_recusa_o_original() {
    let d = tempfile::tempdir().unwrap();
    let orig = d.path().join("dados.jsonl");
    std::fs::write(&orig, DADOS).unwrap();
    let o = orig.to_str().unwrap();
    let novo = d.path().join("novo.jsonl");
    let n = novo.to_str().unwrap();

    // Recusa o original, pelo caminho e por link; o original fica intacto.
    let r = jasao(&["transformar", o, "-o", o]);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("aberto no jasão"));
    #[cfg(unix)]
    {
        let link = d.path().join("link.jsonl");
        std::os::unix::fs::symlink(&orig, &link).unwrap();
        assert!(
            !jasao(&["transformar", o, "-o", link.to_str().unwrap()])
                .status
                .success()
        );
    }
    assert_eq!(ler(&orig), DADOS);

    // Máscara por padrão; --anonimizar dá código; --sem-mascara (global) deixa como está.
    assert!(jasao(&["transformar", o, "-o", n]).status.success());
    let m = ler(&novo);
    assert!(
        !m.contains("S3NT1N3LA") && m.contains("\"nome\":\"ana\""),
        "{m}"
    );
    assert!(
        jasao(&["transformar", o, "-o", n, "--anonimizar"])
            .status
            .success()
    );
    let a = ler(&novo);
    assert!(!a.contains("S3NT1N3LA") && a.contains("anon:"), "{a}");
    assert!(
        jasao(&["--sem-mascara", "transformar", o, "-o", n])
            .status
            .success()
    );
    assert_eq!(ler(&novo), DADOS);

    // Transformações e formatos.
    let ok = |args: &[&str]| {
        let mut v = vec!["transformar", o, "-o", n];
        v.extend_from_slice(args);
        let r = jasao(&v);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        ler(&novo)
    };
    assert_eq!(
        ok(&["--tirar", "senha", "--renomear", "nome=quem"]),
        "{\"id\":1,\"quem\":\"ana\"}\n{\"id\":2,\"quem\":\"bia\"}\n"
    );
    assert_eq!(ok(&["--manter", "id"]), "{\"id\":1}\n{\"id\":2}\n");
    assert_eq!(
        ok(&[
            "--jq",
            "select(.id == 2)",
            "--manter",
            "nome",
            "--saida",
            "json"
        ]),
        "{\n  \"nome\": \"bia\"\n}\n"
    );
    assert_eq!(
        ok(&["--manter", "id", "--saida", "json-compacto"]),
        "[{\"id\":1},{\"id\":2}]\n"
    );
    // Uso errado: sem destino.
    assert_eq!(jasao(&["transformar", o]).status.code(), Some(2));
}
