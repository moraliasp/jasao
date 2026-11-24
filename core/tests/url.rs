//! Abertura por URL contra servidores locais (sem rede externa): HTTP puro e HTTPS com
//! certificado gerado na hora.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use nucleo::url::{self, Opcoes, Pedido};
use nucleo::{ArquivoLinhas, compactado};

/// Lê o pedido até o fim dos cabeçalhos (e o corpo, pelo Content-Length).
fn ler_pedido(s: &mut impl Read) -> String {
    let mut r = BufReader::new(s);
    let mut cab = String::new();
    let mut tamanho = 0;
    loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
            break;
        }
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
            tamanho = v.trim().parse().unwrap();
        }
        cab.push_str(&l);
    }
    let mut corpo = vec![0; tamanho];
    let _ = r.read_exact(&mut corpo);
    cab + &String::from_utf8_lossy(&corpo)
}

/// Servidor HTTP de uma conexão por resposta; devolve a porta e os pedidos recebidos.
fn servir(respostas: Vec<Vec<u8>>) -> (u16, Receiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let porta = l.local_addr().unwrap().port();
    let (tx, rx) = channel();
    thread::spawn(move || {
        for r in respostas {
            let (mut s, _) = l.accept().unwrap();
            let _ = tx.send(ler_pedido(&mut s));
            let _ = s.write_all(&r);
        }
    });
    (porta, rx)
}

fn ok(corpo: &[u8], extra: &str) -> Vec<u8> {
    let mut r = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        corpo.len()
    )
    .into_bytes();
    r.extend_from_slice(corpo);
    r
}

fn op(dir: &Path) -> Opcoes<'static> {
    Opcoes {
        dir_cache: Some(dir.to_path_buf()),
        permitir_http: true,
        tempo_max: Duration::from_secs(5),
        ..Opcoes::default()
    }
}

/// Nada de `.parcial` nem `url-*.dados` sobrando no cache.
fn cache_vazio(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .unwrap()
        .all(|e| !e.unwrap().file_name().to_string_lossy().starts_with("url-"))
}

#[test]
fn baixa_json_e_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = b"{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n";
    let (porta, rx) = servir(vec![ok(br#"{"x":[1,2]}"#, ""), ok(jsonl, "")]);
    let base = format!("http://127.0.0.1:{porta}");

    let p = url::baixar(
        &Pedido::de_url(&format!("{base}/a.json")).unwrap(),
        &op(dir.path()),
    )
    .unwrap();
    assert!(url::baixado(&p));
    assert_eq!(std::fs::read(&p).unwrap(), br#"{"x":[1,2]}"#);
    let pedido = rx.recv().unwrap();
    assert!(pedido.starts_with("GET /a.json "), "{pedido}");
    assert!(
        pedido.to_ascii_lowercase().contains("user-agent: jasao/"),
        "{pedido}"
    );
    assert!(!pedido.to_ascii_lowercase().contains("cookie"), "{pedido}");

    let p = url::baixar(
        &Pedido::de_url(&format!("{base}/b.jsonl")).unwrap(),
        &op(dir.path()),
    )
    .unwrap();
    assert_eq!(ArquivoLinhas::abrir(&p).unwrap().linhas(), 3);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn gzip_abre_transparente() {
    let dir = tempfile::tempdir().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(b"{\"a\":1}\n{\"a\":2}\n").unwrap();
    let (porta, rx) = servir(vec![ok(
        &gz.finish().unwrap(),
        "Content-Encoding: gzip\r\n",
    )]);
    let curl = format!("curl --compressed http://127.0.0.1:{porta}/x");
    let p = url::baixar(&Pedido::interpretar(&curl).unwrap(), &op(dir.path())).unwrap();
    assert!(
        rx.recv()
            .unwrap()
            .to_ascii_lowercase()
            .contains("accept-encoding: gzip")
    );
    let cop = compactado::Opcoes {
        dir_cache: Some(dir.path().to_path_buf()),
        ..compactado::Opcoes::default()
    };
    assert_eq!(ArquivoLinhas::abrir_com(&p, &cop).unwrap().linhas(), 2);
}

#[test]
fn erro_404_sem_segredo() {
    let dir = tempfile::tempdir().unwrap();
    let segredo = "tok-9f8e7d6c5b4a";
    let corpo = format!("nao achei; voce mandou Bearer {segredo}");
    let (porta, rx) = servir(vec![
        format!(
            "HTTP/1.1 404 {segredo}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
            corpo.len()
        )
        .into_bytes(),
    ]);
    let curl = format!(
        "curl -H 'Authorization: Bearer {segredo}' 'http://127.0.0.1:{porta}/x?api_key={segredo}'"
    );
    let pedido = Pedido::interpretar(&curl).unwrap();
    let e = url::baixar(&pedido, &op(dir.path()))
        .unwrap_err()
        .to_string();
    // O cabeçalho foi enviado, mas não volta em erro nem em Debug (o que iria para log).
    assert!(rx.recv().unwrap().contains(segredo));
    assert!(e.contains("404"), "{e}");
    assert!(!e.contains(segredo), "{e}");
    assert!(!format!("{pedido:?}").contains(segredo));
    assert!(cache_vazio(dir.path()));
}

#[test]
fn tempo_esgotado() {
    let dir = tempfile::tempdir().unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let porta = l.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        ler_pedido(&mut s);
        thread::sleep(Duration::from_secs(3));
    });
    let o = Opcoes {
        tempo_max: Duration::from_millis(300),
        ..op(dir.path())
    };
    let e = url::baixar(
        &Pedido::de_url(&format!("http://127.0.0.1:{porta}/")).unwrap(),
        &o,
    )
    .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::TimedOut, "{e}");
    assert!(cache_vazio(dir.path()));
}

#[test]
fn acima_do_teto() {
    let dir = tempfile::tempdir().unwrap();
    let grande = vec![b'1'; 5000];
    // Um declara o tamanho; o outro não (só para no que chega).
    let mut sem_tamanho = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    sem_tamanho.extend_from_slice(&grande);
    let (porta, _rx) = servir(vec![ok(&grande, ""), sem_tamanho]);
    let o = Opcoes {
        max_bytes: 1000,
        ..op(dir.path())
    };
    for _ in 0..2 {
        let p = Pedido::de_url(&format!("http://127.0.0.1:{porta}/")).unwrap();
        let e = url::baixar(&p, &o).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::FileTooLarge, "{e}");
    }
    assert!(cache_vazio(dir.path()));
}

#[test]
fn http_so_com_permissao() {
    let dir = tempfile::tempdir().unwrap();
    let o = Opcoes {
        permitir_http: false,
        ..op(dir.path())
    };
    let e = url::baixar(&Pedido::de_url("http://127.0.0.1:9/").unwrap(), &o).unwrap_err();
    assert_eq!(e.to_string(), url::ERRO_HTTP);
}

#[test]
fn curl_com_flag_nao_suportada() {
    let e = Pedido::interpretar("curl --insecure https://127.0.0.1/").unwrap_err();
    assert!(e.contains("--insecure"), "{e}");
}

/// Servidor HTTPS (certificado gerado agora, para 127.0.0.1) que responde `resposta`.
fn servir_tls(resposta: Vec<u8>) -> (u16, Vec<u8>) {
    let ck = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
    let der = ck.cert.der().to_vec();
    let chave = rustls::pki_types::PrivateKeyDer::Pkcs8(ck.signing_key.serialize_der().into());
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![der.clone().into()], chave)
    .unwrap();
    let cfg = Arc::new(cfg);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let porta = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(tcp): Result<TcpStream, _> = s else {
                break;
            };
            let conn = rustls::ServerConnection::new(Arc::clone(&cfg)).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            ler_pedido(&mut tls);
            let _ = tls.write_all(&resposta);
            let _ = tls.flush();
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    (porta, der)
}

#[test]
fn https_para_http_recusado() {
    let dir = tempfile::tempdir().unwrap();
    let (http, rx) = servir(vec![ok(b"{}", "")]);
    let (porta, raiz) = servir_tls(
        format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{http}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .into_bytes(),
    );
    let o = Opcoes {
        raizes: vec![raiz],
        ..op(dir.path())
    };
    let p = Pedido::de_url(&format!("https://127.0.0.1:{porta}/")).unwrap();
    let e = url::baixar(&p, &o).unwrap_err();
    assert!(e.to_string().contains("https para http"), "{e}");
    assert!(
        rx.try_recv().is_err(),
        "o salto para http não pode acontecer"
    );
    assert!(cache_vazio(dir.path()));
}

#[test]
fn https_baixa_e_certificado_e_validado() {
    let dir = tempfile::tempdir().unwrap();
    let (porta, raiz) = servir_tls(ok(b"{\"ok\":true}", ""));
    let p = Pedido::de_url(&format!("https://127.0.0.1:{porta}/")).unwrap();
    // Com a raiz certa, baixa.
    let o = Opcoes {
        raizes: vec![raiz],
        ..op(dir.path())
    };
    let c = url::baixar(&p, &o).unwrap();
    assert_eq!(std::fs::read(c).unwrap(), b"{\"ok\":true}");
    // Com as raízes do sistema, o autoassinado é recusado.
    let e = url::baixar(&p, &op(dir.path())).unwrap_err();
    assert!(e.to_string().contains("certificado"), "{e}");
}

#[test]
fn cancelado_apaga_o_parcial() {
    let dir = tempfile::tempdir().unwrap();
    let (porta, _rx) = servir(vec![ok(b"{\"a\":1}\n", "")]);
    let parar = std::sync::atomic::AtomicBool::new(true);
    let o = Opcoes {
        cancelar: Some(&parar),
        ..op(dir.path())
    };
    let e = url::baixar(
        &Pedido::de_url(&format!("http://127.0.0.1:{porta}/")).unwrap(),
        &o,
    )
    .unwrap_err();
    assert!(compactado::cancelada(&e), "{e}");
    assert!(cache_vazio(dir.path()));
}

fn redireciona(status: &str, para: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nLocation: {para}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

/// Cabeçalhos do pedido em minúsculas (o pedido inteiro, para `contains`).
fn baixo(pedido: String) -> String {
    pedido.to_ascii_lowercase()
}

#[test]
fn redirecionamento_para_outro_host_tira_credenciais() {
    let dir = tempfile::tempdir().unwrap();
    let (pb, rxb) = servir(vec![ok(b"{\"a\":1}\n", "")]);
    let (pa, rxa) = servir(vec![
        redireciona("302 Found", "/dois"),
        redireciona(
            "301 Moved Permanently",
            &format!("http://localhost:{pb}/fim"),
        ),
    ]);
    let curl = format!(
        "curl -H 'Authorization: Bearer seg-a1' -H 'Proxy-Authorization: Basic seg-a2' \
         -H 'X-Api-Key: seg-a3' -H 'X-Auth-Token: seg-a4' -H 'Cookie: s=seg-a5' \
         -H 'X-Comum: fica' http://127.0.0.1:{pa}/um"
    );
    url::baixar(&Pedido::interpretar(&curl).unwrap(), &op(dir.path())).unwrap();
    // Mesmo servidor: tudo vai.
    let um = baixo(rxa.recv().unwrap());
    let dois = baixo(rxa.recv().unwrap());
    assert!(
        um.contains("seg-a1") && dois.contains("seg-a1") && dois.contains("seg-a5"),
        "{dois}"
    );
    // Outro host: nenhum cabeçalho sensível, o resto sim.
    let fim = baixo(rxb.recv().unwrap());
    assert!(fim.starts_with("get /fim "), "{fim}");
    assert!(fim.contains("host: localhost:"), "{fim}");
    assert!(fim.contains("x-comum: fica"), "{fim}");
    for h in [
        "authorization",
        "proxy-authorization",
        "x-api-key",
        "x-auth-token",
        "cookie",
        "seg-a",
    ] {
        assert!(!fim.contains(h), "{h} foi para o outro host: {fim}");
    }
}

#[test]
fn redirecionamento_307_com_corpo_para_outro_host_recusado() {
    let dir = tempfile::tempdir().unwrap();
    let (pb, rxb) = servir(vec![ok(b"{}", "")]);
    let (pa, rxa) = servir(vec![
        redireciona("307 Temporary Redirect", "/de-novo"),
        redireciona("308 Permanent Redirect", &format!("http://localhost:{pb}/")),
    ]);
    let curl = format!("curl -d 'segredo=seg-b1' http://127.0.0.1:{pa}/");
    let e = url::baixar(&Pedido::interpretar(&curl).unwrap(), &op(dir.path())).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
    assert!(
        e.to_string().contains("308") && !e.to_string().contains("seg-b1"),
        "{e}"
    );
    // 307 no mesmo servidor reenvia o corpo; o outro host nunca recebe nada.
    assert!(rxa.recv().unwrap().ends_with("segredo=seg-b1"));
    assert!(rxa.recv().unwrap().ends_with("segredo=seg-b1"));
    assert!(rxb.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(cache_vazio(dir.path()));
}

#[test]
fn cinco_redirecionamentos_passam_e_o_sexto_falha() {
    let dir = tempfile::tempdir().unwrap();
    for (n, passa) in [(5, true), (6, false)] {
        let mut r: Vec<Vec<u8>> = (1..=n)
            .map(|i| redireciona("302 Found", &format!("/r{i}")))
            .collect();
        r.push(ok(b"{}", ""));
        let (porta, _rx) = servir(r);
        let p = Pedido::de_url(&format!("http://127.0.0.1:{porta}/")).unwrap();
        match url::baixar(&p, &op(dir.path())) {
            Ok(_) => assert!(passa, "{n} redirecionamentos passaram"),
            Err(e) => {
                assert!(!passa, "{n}: {e}");
                assert!(e.to_string().contains("mais de 5 redirecionamentos"), "{e}");
            }
        }
    }
}

#[test]
fn post_com_dados_e_usuario() {
    let dir = tempfile::tempdir().unwrap();
    let (porta, rx) = servir(vec![
        ok(b"{}", ""),
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec(),
    ]);
    let curl =
        format!("curl -u fulano:seg-c1 -d 'a=1' --data 'b=seg-c2' http://127.0.0.1:{porta}/x");
    let pedido = Pedido::interpretar(&curl).unwrap();
    url::baixar(&pedido, &op(dir.path())).unwrap();
    let r = rx.recv().unwrap();
    // base64("fulano:seg-c1")
    let basic = "ZnVsYW5vOnNlZy1jMQ==";
    assert!(r.starts_with("POST /x "), "{r}");
    assert!(
        r.contains(&format!("authorization: Basic {basic}"))
            || r.contains(&format!("Authorization: Basic {basic}")),
        "{r}"
    );
    assert!(
        baixo(r.clone()).contains("content-type: application/x-www-form-urlencoded"),
        "{r}"
    );
    assert!(r.ends_with("\r\na=1&b=seg-c2"), "{r}");
    // Nem o Debug (o que iria para log) nem o erro trazem corpo, senha ou base64.
    let e = url::baixar(&pedido, &op(dir.path()))
        .unwrap_err()
        .to_string();
    for s in [format!("{pedido:?}"), e] {
        for seg in ["seg-c1", "seg-c2", basic] {
            assert!(!s.contains(seg), "{seg} em {s}");
        }
    }
}

#[test]
fn usuario_e_senha_na_url() {
    let dir = tempfile::tempdir().unwrap();
    let (porta, rx) = servir(vec![
        ok(b"{}", ""),
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ]);
    let u = format!("http://fulano:seg%23d1@127.0.0.1:{porta}/x.jsonl?token=seg-d2");
    let pedido = Pedido::de_url(&u).unwrap();
    url::baixar(&pedido, &op(dir.path())).unwrap();
    let r = rx.recv().unwrap();
    // A credencial vai como Basic de "fulano:seg#d1", não na linha do pedido.
    assert!(r.starts_with("GET /x.jsonl?token=seg-d2 "), "{r}");
    assert!(
        baixo(r.clone()).contains("authorization: basic znvsyw5vonnlzynkmq=="),
        "{r}"
    );
    let e = url::baixar(&pedido, &op(dir.path()))
        .unwrap_err()
        .to_string();
    assert!(e.contains("404") && e.contains("/x.jsonl"), "{e}");
    for seg in ["fulano", "seg", "token"] {
        assert!(!e.contains(seg), "{seg} em {e}");
    }
    assert_eq!(pedido.exibir(), format!("http://127.0.0.1:{porta}/x.jsonl"));
}

#[test]
fn teto_vale_somando_varios_pedacos() {
    let dir = tempfile::tempdir().unwrap();
    // Sem Content-Length, em pedaços de 300 KiB: nenhum sozinho passa do teto de 1 MiB, a soma sim.
    let pedaco = vec![b'1'; 300 * 1024];
    let mut r =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    for _ in 0..5 {
        r.extend_from_slice(format!("{:x}\r\n", pedaco.len()).as_bytes());
        r.extend_from_slice(&pedaco);
        r.extend_from_slice(b"\r\n");
    }
    r.extend_from_slice(b"0\r\n\r\n");
    let (porta, _rx) = servir(vec![r]);
    let o = Opcoes {
        max_bytes: 1 << 20,
        ..op(dir.path())
    };
    let e = url::baixar(
        &Pedido::de_url(&format!("http://127.0.0.1:{porta}/")).unwrap(),
        &o,
    )
    .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::FileTooLarge, "{e}");
    assert!(cache_vazio(dir.path()));
}
