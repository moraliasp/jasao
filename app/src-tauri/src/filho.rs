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
/// Teto do resultado em disco: filtro que multiplica a entrada (`range(1e12)`) para aqui.
pub(crate) const TETO_SAIDA: u64 = if cfg!(test) { 1 << 20 } else { 1 << 30 };

/// Recusa o que não é arquivo comum: abrir um FIFO ou dispositivo bloqueia ou nunca acaba.
pub(crate) fn arquivo_comum(caminho: &str) -> Result<(), String> {
    if exemplos::embutido(caminho).is_some() {
        return Ok(());
    }
    match std::fs::metadata(caminho) {
        Ok(m) if m.is_file() => Ok(()),
        Ok(m) if m.is_dir() => Err(format!("{caminho}: é uma pasta, não um arquivo")),
        Ok(_) => Err(format!(
            "{caminho}: não é um arquivo comum (FIFO, socket ou dispositivo); a consulta só lê arquivo comum"
        )),
        Err(e) => Err(format!("{caminho}: {e}")),
    }
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
/// `avisar` recebe o andamento; devolve o aviso de fim (sem geração nem arquivo).
pub(crate) fn rodar(
    filtro: &str,
    caminho: &str,
    destino: &Path,
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
            .env(VAR_DESTINO, destino)
            .stdin(Stdio::piped())
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
    std::thread::spawn(move || {
        for linha in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(linha).is_err() {
                return;
            }
        }
    });
    let (mut fim, mut ultima) = (None, String::new());
    loop {
        if cancelar.load(Relaxed) {
            let _ = filho.kill();
            let _ = filho.wait();
            return AndamentoConsulta {
                fim: true,
                cancelado: true,
                ..Default::default()
            };
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(linha) => match serde_json::from_str::<AndamentoConsulta>(&linha) {
                Ok(a) if a.fim => fim = Some(a),
                Ok(a) => avisar(a),
                Err(_) => ultima = linha,
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    match (filho.wait(), fim) {
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

/// Entrada do processo filho, chamada no `main` antes do app subir: `None` se não for o filho;
/// senão roda a consulta e devolve o código de saída.
pub fn filho() -> Option<i32> {
    let caminho = std::env::var(VAR).ok()?;
    let filtro = std::env::var(VAR_FILTRO).unwrap_or_default();
    let destino = PathBuf::from(std::env::var_os(VAR_DESTINO)?);
    std::thread::spawn(|| {
        let _ = io::copy(&mut io::stdin(), &mut io::sink());
        std::process::exit(4);
    });
    let fim = consultar(&caminho, &filtro, &destino, &mut enviar);
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
    let resumo = if documento {
        consulta.executar_documento(arquivo.bytes(), &cancelar, ao_lote)
    } else {
        consulta.executar_jsonl(arquivo.bytes(), &cancelar, ao_lote)
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
            let _ = tx.send(rodar("def f: f; f", &caminho, &destino_, &c, |_| {}));
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
            &AtomicBool::new(false),
            |_| {},
        );
        let falha = fim.falha.expect("recursão sem fundo devia falhar");
        assert!(falha.contains("parou sem terminar"), "{falha}");
    }
}
