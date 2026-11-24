//! `-` lê stdin pelo binário: JSON, JSONL, vazio, inválido, teto, `./-`, e o temporário que some
//! ao sair, em erro, em SIGTERM e, se ficou órfão, na próxima leitura de stdin.

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

    fn comando(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_jasao"));
        c.args(args)
            .env("HOME", &self.0)
            .env("XDG_CACHE_HOME", self.0.join(".cache"));
        c
    }

    /// (código, stdout, stderr), com `entrada` no stdin.
    fn jasao(&self, args: &[&str], entrada: &[u8]) -> (i32, String, String) {
        self.jasao_com(self.comando(args), entrada)
    }

    fn jasao_com(&self, mut c: Command, entrada: &[u8]) -> (i32, String, String) {
        let mut filho = c
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

    /// Temporários de stdin na casa.
    fn temporarios(&self) -> Vec<PathBuf> {
        self.sobras()
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("entrada.")
            })
            .collect()
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
fn pipe_de_jsonl_com_traco() {
    let casa = Casa::nova("jsonl");
    let jsonl = b"{\"a\":1}\n{\"a\":2,\"b\":\"x\"}\n";
    let (c, md, _) = casa.jasao(&["laudo", "-"], jsonl);
    assert_eq!(c, 0);
    assert!(md.starts_with("# Laudo de `-`"), "{md}");
    assert!(md.contains("2 registros"), "{md}");
    // O nome do temporário não vaza: os próximos passos falam em `-`.
    assert!(!md.contains(".parcial"), "{md}");

    let (c, out, _) = casa.jasao(&["jq", ".a", "-"], jsonl);
    assert_eq!((c, out.as_str()), (0, "1\n2\n"));
    let (c, out, _) = casa.jasao(&["linhas", "-"], jsonl);
    assert_eq!(c, 0);
    assert!(out.starts_with("2 linhas"), "{out}");
    // Sem `-`, stdin não é lido: pipe sem arquivo é uso errado, não espera o EOF.
    for args in [&["linhas"][..], &["jq", ".a"], &["laudo"]] {
        let (c, _, err) = casa.jasao(args, jsonl);
        assert_eq!(c, 2, "{args:?}: {err}");
        assert!(err.starts_with("uso"), "{args:?}: {err}");
    }
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
    // stdin só se lê uma vez: dois `-` é erro claro, antes de ler.
    let (c, _, err) = casa.jasao(&["diff", "-", "-"], json);
    assert_eq!(c, 3, "{err}");
    assert!(
        err.contains("stdin só se lê uma vez") && err.contains("./-"),
        "{err}"
    );
    assert!(casa.sobras().is_empty(), "{:?}", casa.sobras());
}

#[test]
fn stdin_vazio_sai_com_erro() {
    let casa = Casa::nova("vazio");
    for args in [&["laudo", "-"][..], &["jq", ".", "-"], &["linhas", "-"]] {
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

#[test]
fn teto_de_stdin() {
    let casa = Casa::nova("teto");
    let mut c = casa.comando(&["linhas", "-"]);
    c.env("JASAO_TETO_STDIN_TESTE", "16");
    let (codigo, _, err) = casa.jasao_com(c, &[b'{'; 17]);
    assert_eq!(codigo, 1, "{err}");
    assert!(err.contains("passou do teto de 16 bytes"), "{err}");
    // No teto exato, passa.
    let mut c = casa.comando(&["linhas", "-"]);
    c.env("JASAO_TETO_STDIN_TESTE", "16");
    let (codigo, out, err) = casa.jasao_com(c, b"{\"a\":1}\n{\"b\":2}\n");
    assert_eq!(codigo, 0, "{err}");
    assert!(out.starts_with("2 linhas"), "{out}");
    // A variável só baixa o teto: valor acima do padrão fica no padrão.
    let mut c = casa.comando(&["linhas", "-"]);
    c.env("JASAO_TETO_STDIN_TESTE", "99999999999999999");
    assert_eq!(casa.jasao_com(c, b"{}\n").0, 0);
    assert!(casa.sobras().is_empty(), "{:?}", casa.sobras());
}

#[test]
fn arquivo_chamado_traco_abre_com_ponto_barra() {
    let casa = Casa::nova("traco");
    std::fs::write(casa.0.join("-"), b"{\"a\":1}\n").unwrap();
    let mut c = casa.comando(&["linhas", "./-"]);
    c.current_dir(&casa.0);
    let (codigo, out, err) = casa.jasao_com(c, b"");
    assert_eq!(codigo, 0, "{err}");
    assert!(out.starts_with("1 linha"), "{out}");
}

/// O verificador deixou 262 MB em claro com `yes | jasao laudo -` + SIGTERM: agora o tratador
/// apaga o temporário antes de sair.
#[cfg(unix)]
#[test]
fn sigterm_no_meio_apaga_o_temporario() {
    let casa = Casa::nova("sigterm");
    let mut filho = casa
        .comando(&["laudo", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut entrada = filho.stdin.take().unwrap();
    // stdin sem fim, até o pipe quebrar.
    let escritor = std::thread::spawn(move || {
        let bloco = b"{\"a\":1}\n".repeat(4096);
        while entrada.write_all(&bloco).is_ok() {}
    });
    let inicio = std::time::Instant::now();
    while casa.temporarios().is_empty() {
        assert!(inicio.elapsed().as_secs() < 10, "o temporário não apareceu");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let ok = Command::new("kill")
        .args(["-TERM", &filho.id().to_string()])
        .status()
        .unwrap();
    assert!(ok.success());
    let status = filho.wait().unwrap();
    escritor.join().unwrap();
    assert_eq!(status.code(), Some(130), "{status:?}");
    assert!(casa.temporarios().is_empty(), "{:?}", casa.temporarios());
}

/// Órfão (dono morto sem limpar, ex.: SIGKILL) sai na próxima leitura de stdin; o de um processo
/// vivo (travado) e o recém-criado ficam.
#[test]
fn orfao_sai_na_proxima_leitura() {
    use std::fs::File;
    let casa = Casa::nova("orfao");
    // Cria a pasta do cache do jeito do binário.
    assert_eq!(casa.jasao(&["linhas", "-"], b"{}\n").0, 0);
    let dir = casa.0.join(if cfg!(target_os = "macos") {
        "Library/Caches/jasao/descompactados"
    } else {
        ".cache/jasao/descompactados"
    });
    let antigo = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    let criar = |nome: &str, velho: bool| {
        let c = dir.join(nome);
        let f = File::create(&c).unwrap();
        if velho {
            f.set_modified(antigo).unwrap();
        }
        (c, f)
    };
    let (orfao, _) = criar("entrada.1.2.parcial", true);
    let (novo, _) = criar("entrada.3.4.parcial", false);
    let (vivo, trava) = criar("entrada.5.6.parcial", true);
    trava.lock().unwrap();
    assert_eq!(casa.jasao(&["linhas", "-"], b"{}\n").0, 0);
    assert!(!orfao.exists());
    assert!(novo.exists() && vivo.exists());
    drop(trava);
}
