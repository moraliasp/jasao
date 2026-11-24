//! `jasao jq` de ponta a ponta: o processo pai sobrevive ao filtro que estoura pilha, heap ou
//! relógio, e sai com 3 e mensagem legível.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Roda `jasao jq` com teto do próprio teste (o mutante sem supervisor não pode travar a suíte).
fn jq(nome: &str, args: &[&str]) -> (Option<i32>, String, Duration) {
    let arquivo =
        std::env::temp_dir().join(format!("jasao-cli-{nome}-{}.jsonl", std::process::id()));
    std::fs::write(&arquivo, "1\n").unwrap();
    let mut filho = Command::new(env!("CARGO_BIN_EXE_jasao"))
        .arg("jq")
        .args(&args[..1])
        .arg(&arquivo)
        .args(&args[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let t = Instant::now();
    while filho.try_wait().unwrap().is_none() {
        if t.elapsed() > Duration::from_secs(30) {
            filho.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let saida = filho.wait_with_output().unwrap();
    let _ = std::fs::remove_file(&arquivo);
    (
        saida.status.code(),
        String::from_utf8_lossy(&saida.stderr).into_owned(),
        t.elapsed(),
    )
}

#[test]
fn recursao_sem_fundo_nao_aborta() {
    let (codigo, err, _) = jq("pilha", &["def f: 1 + f; f"]);
    assert_eq!(codigo, Some(3), "{err}");
    assert!(err.contains("recursão sem fundo"), "{err}");
}

#[test]
fn teto_de_tempo_vale_sem_resultado() {
    let (codigo, err, t) = jq("tempo", &["until(false; .)", "--tempo-max", "1"]);
    assert_eq!(codigo, Some(3), "{err}");
    assert!(err.contains("foi parada"), "{err}");
    assert!(t < Duration::from_secs(10), "{t:?}");
}

#[test]
fn teto_de_memoria_com_expressao_unica() {
    // `--tempo-max`: sem o teto de heap, o relógio para o filho com outra mensagem.
    let args = [
        "[range(1e9)] | length",
        "--max-memoria-mb",
        "64",
        "--tempo-max",
        "8",
    ];
    let (codigo, err, _) = jq("memoria", &args);
    assert_eq!(codigo, Some(3), "{err}");
    assert!(err.contains(nucleo::memoria::MARCA), "{err}");
    assert!(err.contains("teto de memória (64 MiB)"), "{err}");
}
