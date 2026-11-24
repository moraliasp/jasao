//! Entrada que não é arquivo local (stdin; depois, URL): grava o fluxo num temporário e o resto
//! do núcleo abre esse caminho como qualquer arquivo (ver docs/decisoes.md, "stdin por
//! temporário"). O temporário fica na pasta do cache (só do dono), com permissão 0600, e sai no
//! `Drop` de `Temporario`, inclusive em erro. Processo morto por sinal não roda o `Drop`: a
//! extensão `.parcial` faz a limpeza do cache apagá-lo depois de um dia.

use std::fs::{self, File};
use std::io::{self, BufWriter, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Opcoes;
use crate::compactado::{pasta_cache, privado};

/// Arquivo temporário com o conteúdo de um fluxo; apagado quando sai de escopo.
#[derive(Debug)]
pub struct Temporario(PathBuf);

impl Temporario {
    pub fn caminho(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temporario {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Grava `leitor` inteiro num temporário da pasta do cache de `op`. Teto: `op.max_saida` (o
/// mesmo do descompactado, 10 GiB no padrão). `origem` nomeia a entrada nas mensagens ("stdin").
/// Vazio é erro: não há o que abrir.
pub fn gravar(mut leitor: impl Read, origem: &str, op: &Opcoes) -> io::Result<Temporario> {
    let dir = pasta_cache(op)?;
    let nome = format!(
        "entrada.{}.{}.parcial",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut abrir = File::options();
    abrir.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut abrir, 0o600);
    let caminho = dir.join(nome);
    let arquivo = abrir.open(&caminho)?;
    let tmp = Temporario(caminho);
    privado(tmp.caminho(), 0o600)?;
    let mut saida = BufWriter::with_capacity(1 << 20, arquivo);
    let n = io::copy(&mut (&mut leitor).take(op.max_saida + 1), &mut saida)?;
    saida.flush()?;
    if n == 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("{origem} vazio: nada para abrir"),
        ));
    }
    if n > op.max_saida {
        return Err(io::Error::new(
            ErrorKind::FileTooLarge,
            format!(
                "{origem} passou do teto de {} MiB; grave num arquivo e abra o arquivo",
                op.max_saida >> 20
            ),
        ));
    }
    Ok(tmp)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn op(dir: &Path) -> Opcoes<'static> {
        Opcoes {
            dir_cache: Some(dir.to_path_buf()),
            ..Opcoes::default()
        }
    }

    #[test]
    fn grava_0600_e_apaga_ao_sair() {
        let d = tempfile::tempdir().unwrap();
        let t = gravar(&b"{\"a\":1}\n"[..], "stdin", &op(d.path())).unwrap();
        let c = t.caminho().to_path_buf();
        assert_eq!(fs::read(&c).unwrap(), b"{\"a\":1}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&c).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(t);
        assert!(!c.exists());
    }

    #[test]
    fn vazio_e_teto_dao_erro_sem_sobra() {
        let d = tempfile::tempdir().unwrap();
        let e = gravar(&b""[..], "stdin", &op(d.path())).unwrap_err();
        assert!(e.to_string().contains("stdin vazio"), "{e}");
        let pequeno = Opcoes {
            max_saida: 4,
            ..op(d.path())
        };
        let e = gravar(&b"12345"[..], "stdin", &pequeno).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::FileTooLarge);
        assert_eq!(
            gravar(&b"1234"[..], "stdin", &pequeno).map(|_| ()).ok(),
            Some(())
        );
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
    }
}
