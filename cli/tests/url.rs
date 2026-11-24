//! URL e cURL no lugar do arquivo, contra um servidor local (sem rede externa).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

/// Responde cada conexão com a próxima resposta.
fn servir(respostas: Vec<String>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let porta = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for r in respostas {
            let (mut s, _) = l.accept().unwrap();
            let mut ler = BufReader::new(&mut s);
            let mut linha = String::new();
            while ler.read_line(&mut linha).unwrap_or(0) > 0 && linha != "\r\n" {
                linha.clear();
            }
            let _ = s.write_all(r.as_bytes());
        }
    });
    porta
}

fn resposta(status: &str, corpo: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
        corpo.len()
    )
}

/// (código, stdout, stderr), com o cache numa pasta temporária.
fn jasao(args: &[&str], cache: &std::path::Path) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_jasao"))
        .args(args)
        .env("HOME", cache)
        .env("XDG_CACHE_HOME", cache)
        .output()
        .unwrap();
    let s = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
    (o.status.code().unwrap(), s(o.stdout), s(o.stderr))
}

#[test]
fn linhas_de_url_com_http_explicito() {
    let cache = tempfile::tempdir().unwrap();
    let porta = servir(vec![resposta("200 OK", "{\"a\":1}\n{\"a\":2}\n")]);
    let url = format!("http://127.0.0.1:{porta}/x.jsonl");
    // Sem --http, recusa sem nem conectar.
    let (c, _, err) = jasao(&["linhas", &url], cache.path());
    assert_eq!(c, 1);
    assert!(err.contains("--http"), "{err}");
    let (c, out, err) = jasao(&["linhas", &url, "--http"], cache.path());
    assert_eq!(c, 0, "{err}");
    assert!(out.starts_with("2 linhas"), "{out}");
    assert!(err.contains("aviso: http://"), "{err}");
}

#[test]
fn curl_com_segredo_nao_vaza_no_erro() {
    let cache = tempfile::tempdir().unwrap();
    let porta = servir(vec![resposta("404 Not Found", "nada")]);
    let curl = format!(
        "curl -H 'Authorization: Bearer seg-123abc' http://127.0.0.1:{porta}/x?token=seg-123abc"
    );
    let (c, _, err) = jasao(&["linhas", &curl, "--http"], cache.path());
    assert_eq!(c, 1);
    assert!(err.contains("404"), "{err}");
    assert!(!err.contains("seg-123abc"), "{err}");
    let (c, _, err) = jasao(&["linhas", "curl -L https://127.0.0.1/"], cache.path());
    assert_eq!(c, 1);
    assert!(err.contains("-L"), "{err}");
}

/// Todo subcomando que aceita arquivo aceita URL e cURL, e nenhum devolve a entrada crua
/// (consulta, `usuario:senha@`, cabeçalho) em stdout ou stderr, nem no sucesso nem no erro.
#[test]
fn nenhum_subcomando_mostra_a_entrada_crua() {
    let corpo = "{\"a\":1,\"b\":\"x\"}\n{\"a\":2,\"b\":\"y\"}\n";
    let schema = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(schema.path(), r#"{"type":"object"}"#).unwrap();
    let schema = schema.path().to_str().unwrap().to_owned();
    let saida = tempfile::tempdir().unwrap();
    let novo = saida.path().join("novo.jsonl");
    let novo = novo.to_str().unwrap();
    // `U` é trocado pela entrada; quantas vezes ela aparece = quantos downloads.
    let casos: Vec<Vec<&str>> = vec![
        vec!["linhas", "U"],
        vec!["janela", "U", "0", "1"],
        vec!["buscar", "U", "x"],
        vec!["consultar", ".a", "U"],
        vec!["transformar", "U", "-o", novo, "--sobrescrever"],
        vec!["erros", "U"],
        vec!["inferir", "U"],
        vec!["exportar", "--schema", "U"],
        vec!["validar", "U", "--schema", &schema],
        vec!["validar", "U", "--como", "U"],
        vec!["diff", "U", "U"],
        vec!["laudo", "U"],
    ];
    for caso in &casos {
        let n = caso.iter().filter(|a| **a == "U").count();
        for status in ["200 OK", "404 Not Found"] {
            let cache = tempfile::tempdir().unwrap();
            let porta = servir(vec![resposta(status, corpo); n]);
            let entradas = [
                format!("http://fulano:seg-e1@127.0.0.1:{porta}/d/x.jsonl?token=seg-e2#seg-e3"),
                format!(
                    "curl -H 'X-Api-Key: seg-e1' 'http://127.0.0.1:{porta}/d/x.jsonl?token=seg-e2'"
                ),
            ];
            for entrada in entradas.iter().take(if status == "200 OK" { 1 } else { 2 }) {
                // O cURL usa outro servidor (cada um responde `n` vezes).
                let entrada = if entrada.starts_with("curl") {
                    let p = servir(vec![resposta(status, corpo); n]);
                    entrada.replace(&format!(":{porta}/"), &format!(":{p}/"))
                } else {
                    entrada.clone()
                };
                let args: Vec<String> = caso
                    .iter()
                    .map(|a| {
                        if *a == "U" {
                            entrada.clone()
                        } else {
                            (*a).to_owned()
                        }
                    })
                    .chain(["--http".to_owned()])
                    .collect();
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let (c, out, err) = jasao(&args, cache.path());
                let tudo = format!("{out}{err}");
                assert!(
                    !tudo.contains("seg-e") && !tudo.contains("fulano") && !tudo.contains("token"),
                    "{caso:?} {status}: {tudo}"
                );
                if status == "200 OK" {
                    assert_ne!(c, 1, "{caso:?}: {tudo}");
                } else {
                    assert_ne!(c, 0, "{caso:?}: {tudo}");
                    assert!(err.contains("404"), "{caso:?}: {err}");
                }
            }
        }
    }
}

/// O laudo usa a origem limpa no título e nos comandos sugeridos.
#[test]
fn laudo_de_url_mostra_a_origem_limpa() {
    let cache = tempfile::tempdir().unwrap();
    let porta = servir(vec![resposta("200 OK", "{\"a\":1}\n{\"a\":2}\n")]);
    let url = format!("http://fulano:seg-f1@127.0.0.1:{porta}/d/x.jsonl?token=seg-f2");
    let (c, out, err) = jasao(&["laudo", &url, "--http"], cache.path());
    assert_eq!(c, 0, "{err}");
    assert!(
        out.contains(&format!("http://127.0.0.1:{porta}/d/x.jsonl")),
        "{out}"
    );
    assert!(!out.contains("seg-f") && !out.contains("fulano"), "{out}");
}

/// Temporários de download (`download.*.parcial`) em qualquer lugar sob `dir`.
fn downloads(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut achados = Vec::new();
    let mut pilha = vec![dir.to_path_buf()];
    while let Some(d) = pilha.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let c = e.path();
            if c.is_dir() {
                pilha.push(c);
            } else if e.file_name().to_string_lossy().starts_with("download.") {
                achados.push(c);
            }
        }
    }
    achados
}

#[test]
fn baixado_sai_ao_terminar() {
    let cache = tempfile::tempdir().unwrap();
    let porta = servir(vec![resposta("200 OK", "{\"a\":1}\n")]);
    let url = format!("http://127.0.0.1:{porta}/x.jsonl");
    let (codigo, _, err) = jasao(&["linhas", &url, "--http"], cache.path());
    assert_eq!(codigo, 0, "{err}");
    assert!(
        downloads(cache.path()).is_empty(),
        "{:?}",
        downloads(cache.path())
    );
}

/// SIGTERM no meio do download: o tratador de `entrada::apagar_no_sinal` apaga o temporário.
#[cfg(unix)]
#[test]
fn sigterm_no_meio_do_download_apaga_o_temporario() {
    use std::process::Stdio;
    let cache = tempfile::tempdir().unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let porta = l.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut ler = BufReader::new(&mut s);
        let mut linha = String::new();
        while ler.read_line(&mut linha).unwrap_or(0) > 0 && linha != "\r\n" {
            linha.clear();
        }
        // Promete 1 GiB, manda um pedaço e para: o download fica no meio.
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1073741824\r\n\r\n");
        let _ = s.write_all(&b"{\"a\":1}\n".repeat(4096));
        thread::sleep(std::time::Duration::from_secs(30));
    });
    let url = format!("http://127.0.0.1:{porta}/x.jsonl");
    let mut filho = Command::new(env!("CARGO_BIN_EXE_jasao"))
        .args(["linhas", &url, "--http"])
        .env("HOME", cache.path())
        .env("XDG_CACHE_HOME", cache.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let inicio = std::time::Instant::now();
    while downloads(cache.path()).is_empty() {
        assert!(inicio.elapsed().as_secs() < 10, "o temporário não apareceu");
        thread::sleep(std::time::Duration::from_millis(20));
    }
    thread::sleep(std::time::Duration::from_millis(200));
    let ok = Command::new("kill")
        .args(["-TERM", &filho.id().to_string()])
        .status()
        .unwrap();
    assert!(ok.success());
    let status = filho.wait().unwrap();
    assert_eq!(status.code(), Some(130), "{status:?}");
    assert!(
        downloads(cache.path()).is_empty(),
        "{:?}",
        downloads(cache.path())
    );
}
