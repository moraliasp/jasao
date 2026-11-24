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
    for extra in [&[][..], &["--sobrescrever"]] {
        let mut v = vec!["transformar", o, "-o", o];
        v.extend_from_slice(extra);
        let r = jasao(&v);
        assert!(!r.status.success());
        assert!(String::from_utf8_lossy(&r.stderr).contains("aberto no jasão"));
    }
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
    // Destino que já existe: só com --sobrescrever.
    let r = jasao(&["transformar", o, "-o", n, "--anonimizar"]);
    assert_eq!(r.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&r.stderr).contains("--sobrescrever"));
    assert_eq!(ler(&novo), m);
    assert!(
        jasao(&["transformar", o, "-o", n, "--anonimizar", "--sobrescrever"])
            .status
            .success()
    );
    let a = ler(&novo);
    assert!(!a.contains("S3NT1N3LA") && a.contains("anon:"), "{a}");
    // Sem máscara: avisa e o arquivo fica só do dono.
    let r = jasao(&["--sem-mascara", "transformar", o, "-o", n, "--sobrescrever"]);
    assert!(r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("aviso: --sem-mascara"));
    assert_eq!(ler(&novo), DADOS);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let modo = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(modo(&novo), 0o600);
        assert!(
            jasao(&["transformar", o, "-o", n, "--sobrescrever"])
                .status
                .success()
        );
        assert_eq!(modo(&novo), 0o644);
    }

    // Transformações e formatos.
    let ok = |args: &[&str]| {
        let mut v = vec!["transformar", o, "-o", n, "--sobrescrever"];
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
    // Renomear ambíguo: recusado, nada gravado.
    let r = jasao(&[
        "transformar",
        o,
        "-o",
        n,
        "--sobrescrever",
        "--renomear",
        "id=x,nome=x",
    ]);
    assert_eq!(r.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&r.stderr).contains("dois campos viram"));
    // Uso errado: sem destino.
    assert_eq!(jasao(&["transformar", o]).status.code(), Some(2));
}

/// Linha inválida ou erro do jq: grava o resto, conta os descartados e sai com 3.
#[test]
fn transformar_conta_descartados_e_sai_com_3() {
    let d = tempfile::tempdir().unwrap();
    let orig = d.path().join("dados.jsonl");
    std::fs::write(&orig, "{\"n\":1}\nnão é json\n{\"n\":\"x\"}\n{\"n\":2}\n").unwrap();
    let novo = d.path().join("novo.jsonl");
    let (o, n) = (orig.to_str().unwrap(), novo.to_str().unwrap());
    let r = jasao(&["transformar", o, "-o", n, "--jq", "{m: (.n + 1)}"]);
    let err = String::from_utf8_lossy(&r.stderr);
    assert_eq!(r.status.code(), Some(3), "{err}");
    assert!(err.contains("2 resultados · 2 descartados"), "{err}");
    assert_eq!(ler(&novo), "{\"m\":2}\n{\"m\":3}\n");
    // Sem descarte, 0.
    let r = jasao(&[
        "transformar",
        o,
        "-o",
        n,
        "--sobrescrever",
        "--jq",
        "select(.n == 1)",
    ]);
    assert_eq!(
        r.status.code(),
        Some(3),
        "a linha inválida conta mesmo sem erro do jq"
    );
    std::fs::write(&orig, "{\"n\":1}\n").unwrap();
    let r = jasao(&["transformar", o, "-o", n, "--sobrescrever"]);
    assert_eq!(r.status.code(), Some(0));
}
