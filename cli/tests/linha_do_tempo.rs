//! `jasao linha-do-tempo` pelo binário: os tetos e a saída valem no call site real.

use std::fs;
use std::process::Command;

/// (código, stdout, stderr)
fn jasao(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_jasao"))
        .arg("linha-do-tempo")
        .args(args)
        .output()
        .unwrap();
    let s = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (o.status.code().unwrap(), s(o.stdout), s(o.stderr))
}

fn pasta(nome: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("jasao-ldt-{}-{nome}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir(&p).unwrap();
    p
}

#[test]
fn versoes_em_ordem_com_o_diff() {
    let p = pasta("ok");
    fs::write(p.join("2024-01-02.jsonl"), "{\"a\":1,\"b\":true}\n").unwrap();
    fs::write(p.join("2024-01-01.jsonl"), "{\"a\":1}\n").unwrap();
    fs::write(p.join("notas.txt"), "x").unwrap();
    let (c, out, err) = jasao(&[p.to_str().unwrap()]);
    assert_eq!(c, 0, "{err}");
    let linhas: Vec<&str> = out.lines().collect();
    assert_eq!(
        linhas[0],
        "2024-01-01 00:00:00  2024-01-01.jsonl  1 registros"
    );
    assert_eq!(
        linhas[1],
        "2024-01-02 00:00:00  2024-01-02.jsonl  1 quebras · 0 compatíveis contra 2024-01-01.jsonl"
    );
    assert!(linhas[2].contains(".b  campo novo obrigatório"), "{out}");
    assert!(
        err.contains("2 versões · ordem por nome · 1 entradas ignoradas"),
        "{err}"
    );
    let (c, json, _) = jasao(&["--json", p.to_str().unwrap()]);
    assert_eq!(c, 0);
    assert!(json.starts_with(r#"{"origem":"nome","ignorados":1,"parada":null,"versoes":[{"nome":"2024-01-01.jsonl","data":"2024-01-01 00:00:00","registros":1,"invalidas":0,"erro":null,"contra":null,"#), "{json}");
    assert!(json.contains(r#""contra":0,"quebras":1,"compativeis":0,"mais":0,"mudancas":[{"caminho":".b","regra":"campo_novo_obrigatorio""#), "{json}");
    fs::remove_dir_all(p).unwrap();
}

#[test]
fn teto_de_500_arquivos() {
    let p = pasta("teto");
    for i in 0..500 {
        fs::write(p.join(format!("{i:03}.json")), "").unwrap();
    }
    let (c, _, err) = jasao(&[p.to_str().unwrap()]);
    assert_eq!(c, 0, "{err}");
    assert!(err.contains("500 versões"), "{err}");
    fs::write(p.join("500.json"), "").unwrap();
    let (c, out, err) = jasao(&[p.to_str().unwrap()]);
    assert_eq!((c, out.as_str()), (1, ""));
    assert!(err.contains("mais de 500 arquivos"), "{err}");
    fs::remove_dir_all(p).unwrap();
}

#[cfg(unix)]
#[test]
fn link_para_fora_nao_entra() {
    let fora = pasta("fora");
    fs::write(fora.join("segredo.json"), "{\"senha\":1}\n").unwrap();
    let p = pasta("links");
    fs::write(p.join("2024-01-01.json"), "{}\n").unwrap();
    std::os::unix::fs::symlink(fora.join("segredo.json"), p.join("2024-01-02.json")).unwrap();
    let (c, out, err) = jasao(&["--json", p.to_str().unwrap()]);
    assert_eq!(c, 0, "{err}");
    assert!(
        !out.contains("2024-01-02") && !out.contains("senha"),
        "{out}"
    );
    assert!(out.contains(r#""ignorados":1"#), "{out}");
    fs::remove_dir_all(p).unwrap();
    fs::remove_dir_all(fora).unwrap();
}

#[test]
fn uso_errado() {
    assert_eq!(jasao(&[]).0, 2);
}
