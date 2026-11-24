//! Consulta jq em processo filho. O `jaq` não tem ponto de interrupção: filtro que gira sem
//! entregar (`last(range(1e12))`, `def f: f; f`) não vê o cancelamento, e recursão sem fundo
//! (`def f: 1 + f; f`) estoura a pilha e aborta o processo. Por isso o app reexecuta o próprio
//! binário com `VAR` no ambiente: o filho roda a consulta, grava o resultado em `destino` e manda
//! andamento e fim em JSON, um por linha, no stderr. O pai mata o filho ao cancelar; aborto do
//! filho vira falha legível. Se o app morrer, o stdin do filho fecha e ele sai.

use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use nucleo::consulta::{Consulta, LoteConsulta};
use nucleo::resumo::{self, Conteudo};

use super::{AndamentoConsulta, INTERVALO, abrir_linhas, exemplos};

/// Presente no ambiente: o processo é o filho de uma consulta; o valor é o arquivo consultado.
const VAR: &str = "JASAO_CONSULTA_FILHO";
const VAR_FILTRO: &str = "JASAO_CONSULTA_FILTRO";
const VAR_DESTINO: &str = "JASAO_CONSULTA_DESTINO";
/// Presente: a consulta roda sobre o arquivo anonimizado com este sal (hex), ver
/// `nucleo::mascara::anonimizar`.
const VAR_SAL: &str = "JASAO_CONSULTA_SAL";
/// Teto do resultado em disco: filtro que multiplica a entrada (`range(1e12)`) para aqui.
pub(crate) const TETO_SAIDA: u64 = if cfg!(test) { 1 << 20 } else { 1 << 30 };

/// Recusa o que não é arquivo comum antes de criar o filho, para o erro voltar na hora.
pub(crate) fn arquivo_comum(caminho: &str) -> Result<(), String> {
    if exemplos::embutido(caminho).is_some() {
        return Ok(());
    }
    nucleo::linhas::arquivo_comum(Path::new(caminho)).map_err(|e| format!("{caminho}: {e}"))
}

/// O executável do filho: o próprio app. Nos testes, o binário de teste rodando só `modo_filho`.
fn comando() -> io::Result<Command> {
    #[allow(unused_mut)]
    let mut c = Command::new(std::env::current_exe()?);
    #[cfg(test)]
    c.args([
        "filho::testes::modo_filho",
        "--exact",
        "--nocapture",
        "--quiet",
    ]);
    Ok(c)
}

/// Roda `filtro` sobre `caminho` num processo filho que grava em `destino` (já criado).
/// `avisar` recebe o andamento; devolve o aviso de fim (sem geração nem arquivo). Com `sal`, os
/// segredos entram anonimizados em vez de mascarados.
pub(crate) fn rodar(
    filtro: &str,
    caminho: &str,
    destino: &Path,
    sal: Option<u64>,
    cancelar: &AtomicBool,
    mut avisar: impl FnMut(AndamentoConsulta),
) -> AndamentoConsulta {
    let falhou = |falha: String| AndamentoConsulta {
        fim: true,
        falha: Some(falha),
        ..Default::default()
    };
    let filho = comando().and_then(|mut c| {
        c.env(VAR, caminho)
            .env(VAR_FILTRO, filtro)
            .env(VAR_DESTINO, destino);
        let c = match sal {
            Some(s) => c.env(VAR_SAL, format!("{s:x}")),
            None => c.env_remove(VAR_SAL),
        };
        c.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
    });
    let mut filho = match filho {
        Ok(f) => f,
        Err(e) => return falhou(format!("processo da consulta: {e}")),
    };
    // Fica aberto até o fim: o filho sai quando ele fecha (app encerrado ou morto).
    let _vivo = filho.stdin.take();
    let stderr = filho.stderr.take().expect("stderr do filho");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || ler_linhas(stderr, &tx));
    let (mut fim, mut ultima, mut aberto) = (None, String::new(), true);
    // O cancelar vale até o filho sair, mesmo depois de ele fechar o stderr.
    let saida = loop {
        if cancelar.load(Relaxed) {
            let _ = filho.kill();
            let _ = filho.wait();
            return AndamentoConsulta {
                fim: true,
                cancelado: true,
                ..Default::default()
            };
        }
        if !aberto {
            match filho.try_wait() {
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Ok(Some(s)) => break Ok(s),
                Err(e) => break Err(e),
            }
            continue;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(linha) => match serde_json::from_str::<AndamentoConsulta>(&linha) {
                Ok(a) if a.fim => fim = Some(a),
                Ok(a) => avisar(a),
                Err(_) => ultima = linha,
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => aberto = false,
        }
    };
    match (saida, fim) {
        (Ok(s), Some(a)) if s.success() => a,
        (s, _) => falhou(format!(
            "a consulta parou sem terminar (recursão sem fundo ou falta de memória?): {}",
            if ultima.is_empty() {
                s.map_or_else(|e| e.to_string(), |s| s.to_string())
            } else {
                ultima
            }
        )),
    }
}

/// Manda cada linha de `r` (sem o fim de linha) por `tx` até o fim ou até ninguém ouvir. Em
/// bytes: uma linha que não é UTF-8 não pode encerrar a leitura.
fn ler_linhas(r: impl std::io::Read, tx: &mpsc::Sender<String>) {
    let (mut leitor, mut linha) = (BufReader::new(r), Vec::new());
    while leitor.read_until(b'\n', &mut linha).is_ok_and(|n| n > 0) {
        let texto = String::from_utf8_lossy(&linha).trim_end().to_owned();
        if tx.send(texto).is_err() {
            return;
        }
        linha.clear();
    }
}

/// Entrada do processo filho, chamada no `main` antes do app subir: `None` se não for o filho;
/// senão roda a consulta e devolve o código de saída.
pub fn filho() -> Option<i32> {
    let caminho = std::env::var(VAR).ok()?;
    let filtro = std::env::var(VAR_FILTRO).unwrap_or_default();
    let destino = PathBuf::from(std::env::var_os(VAR_DESTINO)?);
    let sal = std::env::var(VAR_SAL)
        .ok()
        .and_then(|s| u64::from_str_radix(&s, 16).ok());
    std::thread::spawn(|| {
        let _ = io::copy(&mut io::stdin(), &mut io::sink());
        std::process::exit(4);
    });
    let fim = consultar(&caminho, &filtro, &destino, sal, &mut enviar);
    enviar(fim);
    Some(0)
}

fn enviar(a: AndamentoConsulta) {
    let mut e = io::stderr().lock();
    let _ = serde_json::to_writer(&mut e, &a);
    let _ = writeln!(e);
}

fn consultar(
    caminho: &str,
    filtro: &str,
    destino: &Path,
    sal: Option<u64>,
    avisar: &mut dyn FnMut(AndamentoConsulta),
) -> AndamentoConsulta {
    let falhou = |falha: String| AndamentoConsulta {
        fim: true,
        falha: Some(falha),
        ..Default::default()
    };
    let consulta = match Consulta::nova(filtro) {
        Ok(c) => c,
        Err(e) => return falhou(e.to_string()),
    };
    let arquivo = match abrir_linhas(caminho) {
        Ok(a) => a,
        Err(e) => return falhou(e),
    };
    let documento = matches!(
        resumo::conteudo(&arquivo),
        Conteudo::JsonUnico | Conteudo::JsonVariasLinhas
    );
    let saida = match std::fs::OpenOptions::new().write(true).open(destino) {
        Ok(s) => s,
        Err(e) => return falhou(format!("{}: {e}", destino.display())),
    };
    let cancelar = AtomicBool::new(false);
    let (mut gravado, mut bytes, mut cortado) = (0u64, 0u64, false);
    let mut falha: Option<io::Error> = None;
    let mut ultimo = Instant::now();
    let mut out = BufWriter::new(saida);
    let ao_lote = |lote: LoteConsulta| {
        for r in &lote.resultados {
            bytes += r.json.len() as u64 + 1;
            if bytes > TETO_SAIDA {
                cortado = true;
                cancelar.store(true, Relaxed);
                return;
            }
            if let Err(e) = writeln!(out, "{}", r.json) {
                falha.get_or_insert(e);
                cancelar.store(true, Relaxed);
                return;
            }
            gravado += 1;
        }
        if ultimo.elapsed() >= INTERVALO {
            ultimo = Instant::now();
            avisar(AndamentoConsulta {
                resultados: gravado,
                linhas: lote.linhas,
                total: lote.total_linhas,
                ..Default::default()
            });
        }
    };
    // Anonimizado é cópia na memória do filho (o comprimento muda; não dá para mapear).
    let anonimo = sal.map(|s| nucleo::mascara::anonimizar(arquivo.cru(), arquivo.indice(), s));
    let entrada = anonimo.as_deref().unwrap_or(arquivo.bytes());
    let resumo = if documento {
        consulta.executar_documento(entrada, &cancelar, ao_lote)
    } else {
        consulta.executar_jsonl(entrada, &cancelar, ao_lote)
    };
    if let Err(e) = out.flush() {
        falha.get_or_insert(e);
    }
    if let Some(e) = falha {
        return falhou(format!("{}: {e}", destino.display()));
    }
    AndamentoConsulta {
        resultados: gravado,
        linhas: resumo.linhas,
        total: resumo.total_linhas,
        erros: resumo.erros,
        primeiro_erro: resumo
            .primeiros_erros
            .first()
            .map(|e| (e.linha, e.motivo.clone())),
        fim: true,
        cortado,
        ..Default::default()
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    /// Linha do stderr que não é UTF-8 (pânico com bytes do arquivo) não esconde as seguintes.
    #[test]
    fn leitor_segue_depois_de_utf8_invalido() {
        let (tx, rx) = mpsc::channel();
        ler_linhas(
            &b"ruim \xff\xfe\n{\"fim\":true}\r\nsem fim de linha"[..],
            &tx,
        );
        drop(tx);
        assert_eq!(
            rx.iter().collect::<Vec<_>>(),
            [
                "ruim \u{fffd}\u{fffd}",
                "{\"fim\":true}",
                "sem fim de linha"
            ]
        );
    }

    /// Não é um teste: é a porta do processo filho quando `comando` reexecuta o binário de teste.
    #[test]
    fn modo_filho() {
        if let Some(c) = filho() {
            std::process::exit(c);
        }
    }

    fn jsonl(conteudo: &[u8]) -> (tempfile::NamedTempFile, tempfile::NamedTempFile) {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(conteudo).unwrap();
        (f, tempfile::NamedTempFile::new().unwrap())
    }

    /// Achado 6: filtro que multiplica a entrada para no teto, e o que foi gravado fica.
    /// A consulta nunca vê o valor mascarado; anonimizada, vê o código estável no lugar dele.
    #[test]
    fn consulta_mascarada_ou_anonimizada() {
        let (f, destino) =
            jsonl(b"{\"senha\":\"S3NT1N3LA-1\",\"n\":1}\n{\"senha\":\"S3NT1N3LA-1\",\"cpf\":11144477735}\n");
        let rodar_com = |sal| {
            let fim = rodar(
                ".",
                f.path().to_str().unwrap(),
                destino.path(),
                sal,
                &AtomicBool::new(false),
                |_| {},
            );
            assert_eq!(fim.falha, None);
            std::fs::read_to_string(destino.path()).unwrap()
        };
        let m = rodar_com(None);
        assert!(
            !m.contains("S3NT1N3LA") && !m.contains("11144477735"),
            "{m}"
        );
        assert!(
            m.contains("\"***********\"") && m.contains("99999999999"),
            "{m}"
        );
        let a = rodar_com(Some(7));
        assert!(
            !a.contains("S3NT1N3LA") && !a.contains("11144477735"),
            "{a}"
        );
        let l: Vec<serde_json::Value> = a
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(l[0]["senha"], l[1]["senha"]);
        assert!(l[1]["cpf"].as_str().unwrap().starts_with("anon:"));
        assert_ne!(rodar_com(Some(8)), a, "outro sal, outros códigos");
    }

    #[test]
    fn saida_tem_teto() {
        let (f, destino) = jsonl(b"1\n");
        let cancelar = AtomicBool::new(false);
        // ~2 MB de saída, o dobro do teto dos testes: sem o teto, o arquivo passa dele.
        let filtro = "range(300000)";
        let fim = rodar(
            filtro,
            f.path().to_str().unwrap(),
            destino.path(),
            None,
            &cancelar,
            |_| {},
        );
        assert!(fim.fim && fim.cortado && !fim.cancelado, "{:?}", fim.falha);
        assert_eq!(fim.falha, None);
        let tamanho = std::fs::metadata(destino.path()).unwrap().len();
        assert!(
            tamanho <= TETO_SAIDA && tamanho > TETO_SAIDA / 2,
            "{tamanho}"
        );
        assert!(fim.resultados > 0);
    }

    /// Achado 8: filtro que gira sem entregar para no cancelar; recursão sem fundo vira falha
    /// em vez de derrubar quem chamou.
    #[test]
    fn filtro_preso_cancela_e_recursao_nao_derruba() {
        let (f, destino) = jsonl(b"1\n");
        let caminho = f.path().to_str().unwrap().to_owned();
        let destino_ = destino.path().to_owned();
        let cancelar = std::sync::Arc::new(AtomicBool::new(false));
        let c = std::sync::Arc::clone(&cancelar);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(rodar("def f: f; f", &caminho, &destino_, None, &c, |_| {}));
        });
        std::thread::sleep(Duration::from_millis(300));
        cancelar.store(true, Relaxed);
        let fim = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("cancelar não parou o filtro preso");
        assert!(fim.cancelado);

        let fim = rodar(
            "def f: 1 + f; f",
            f.path().to_str().unwrap(),
            destino.path(),
            None,
            &AtomicBool::new(false),
            |_| {},
        );
        let falha = fim.falha.expect("recursão sem fundo devia falhar");
        assert!(falha.contains("parou sem terminar"), "{falha}");
    }

    /// Se o pai morre, o stdin do filho fecha e ele sai, mesmo com o filtro preso.
    #[test]
    fn filho_sai_quando_o_pai_some() {
        let (f, destino) = jsonl(b"1\n");
        let mut filho = comando()
            .unwrap()
            .env(VAR, f.path())
            .env(VAR_FILTRO, "def f: f; f")
            .env(VAR_DESTINO, destino.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        drop(filho.stdin.take());
        let ate = Instant::now() + Duration::from_secs(10);
        let saiu = loop {
            if let Some(s) = filho.try_wait().unwrap() {
                break Some(s);
            }
            if Instant::now() > ate {
                let _ = filho.kill();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(saiu.and_then(|s| s.code()), Some(4), "filho não saiu");
    }
}
