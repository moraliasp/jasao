//! Entrada que não é arquivo local (stdin, URL): grava o fluxo num temporário e o resto do
//! núcleo abre esse caminho como qualquer arquivo (ver docs/decisoes.md, "stdin por
//! temporário"). `criar` é a peça genérica (stdin usa `gravar`, que chama `criar("entrada")`;
//! download usa `criar("download")`): arquivo `<prefixo>.<pid>.<nanos>.parcial` na pasta do
//! cache (só do dono), com permissão 0600, que sai:
//! - no `Drop` de `Temporario` (fim normal e erro);
//! - em SIGINT/SIGTERM, se quem chama ligou `apagar_no_sinal` (o CLI liga);
//! - como órfão (processo morto sem chance de limpar, ex.: SIGKILL): o dono segura uma trava
//!   exclusiva no arquivo enquanto vive, e `limpar_orfaos` (chamado por todo `criar` e na
//!   abertura do app) apaga o que ninguém trava, de qualquer prefixo de `PREFIXOS`.

use std::fs::{self, File};
use std::io::{self, BufWriter, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Opcoes;
use crate::compactado::{pasta_cache, privado};

const MIB: u64 = 1 << 20;
/// Espaço que precisa sobrar no disco enquanto stdin é gravado, conferido a cada `PASSO`.
const MARGEM: u64 = 64 * MIB;
const PASSO: u64 = 64 * MIB;
/// Órfão mais novo que isso fica: o dono pode ter criado o arquivo e ainda não ter travado.
const CARENCIA: Duration = Duration::from_secs(60);

/// Prefixos de `criar`. Só eles: o `.parcial` do descompactado não trava e não pode ser
/// confundido com órfão.
pub const PREFIXOS: &[&str] = &["entrada", "download"];

/// Temporários vivos deste processo, para o tratador de sinal apagar.
static VIVOS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Arquivo temporário com o conteúdo de um fluxo; apagado quando sai de escopo. Segura a trava
/// exclusiva do arquivo enquanto vive (ver `limpar_orfaos`).
#[derive(Debug)]
pub struct Temporario {
    caminho: PathBuf,
    _trava: File,
}

impl Temporario {
    pub fn caminho(&self) -> &Path {
        &self.caminho
    }
}

impl Drop for Temporario {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.caminho);
        if let Ok(mut v) = VIVOS.lock() {
            v.retain(|c| *c != self.caminho);
        }
    }
}

/// Liga (uma vez por processo) o tratador de SIGINT/SIGTERM que apaga os temporários vivos e
/// sai com 130: em sinal, o `Drop` não roda. Para binário de linha de comando; o app não liga
/// (a janela trata o próprio fim, e órfão de crash sai em `limpar_orfaos`).
pub fn apagar_no_sinal() {
    static LIGADO: std::sync::Once = std::sync::Once::new();
    LIGADO.call_once(|| {
        let _ = ctrlc::set_handler(|| {
            apagar_vivos();
            std::process::exit(130);
        });
    });
}

/// Apaga os temporários vivos deste processo (o tratador de `apagar_no_sinal` usa).
pub fn apagar_vivos() {
    if let Ok(v) = VIVOS.lock() {
        for c in v.iter() {
            let _ = fs::remove_file(c);
        }
    }
}

/// Se `caminho` tem nome de temporário de `criar` (`<prefixo>.*.parcial`, prefixo de `PREFIXOS`).
pub fn temporario(caminho: &Path) -> bool {
    caminho
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".parcial"))
        .and_then(|n| n.split_once('.'))
        .is_some_and(|(p, _)| PREFIXOS.contains(&p))
}

/// Apaga da pasta do cache os temporários órfãos (de qualquer prefixo de `PREFIXOS`): os que nenhum processo trava (o
/// dono morreu) e que têm mais de `CARENCIA`. Devolve quantos apagou.
pub fn limpar_orfaos(op: &Opcoes) -> io::Result<usize> {
    let dir = pasta_cache(op)?;
    let agora = SystemTime::now();
    let mut n = 0;
    for e in fs::read_dir(&dir)?.flatten() {
        let c = e.path();
        let velho = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|m| agora.duration_since(m).unwrap_or_default() > CARENCIA);
        // A trava cai com o processo, até em SIGKILL: conseguir travar = ninguém vivo usa.
        if temporario(&c)
            && velho
            && File::open(&c).is_ok_and(|f| f.try_lock().is_ok())
            && fs::remove_file(&c).is_ok()
        {
            n += 1;
        }
    }
    Ok(n)
}

/// Cria, travado e registrado para o sinal, um temporário vazio `<prefixo>.<pid>.<nanos>.parcial`
/// na pasta do cache de `op` e devolve o arquivo aberto para escrita. `prefixo` precisa estar em
/// `PREFIXOS` (senão `limpar_orfaos` não o acharia). Antes, limpa os órfãos.
pub fn criar(prefixo: &str, op: &Opcoes) -> io::Result<(Temporario, File)> {
    if !PREFIXOS.contains(&prefixo) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("prefixo de temporário desconhecido: {prefixo}"),
        ));
    }
    let dir = pasta_cache(op)?;
    let _ = limpar_orfaos(op);
    let nome = format!(
        "{prefixo}.{}.{}.parcial",
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
    VIVOS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(caminho.clone());
    let tmp = Temporario {
        caminho,
        _trava: arquivo.try_clone()?,
    };
    arquivo.lock()?;
    privado(tmp.caminho(), 0o600)?;
    Ok((tmp, arquivo))
}

/// Grava `leitor` inteiro num temporário `entrada` (ver `criar`). Teto: `op.max_saida` (o
/// mesmo do descompactado, 10 GiB no padrão). `origem` nomeia a entrada nas mensagens ("stdin").
/// Antes e durante, para se o disco ficar com menos de `MARGEM` livre.
/// Vazio é erro: não há o que abrir.
pub fn gravar(mut leitor: impl Read, origem: &str, op: &Opcoes) -> io::Result<Temporario> {
    let dir = pasta_cache(op)?;
    espaco(&dir, origem)?;
    let (tmp, arquivo) = criar("entrada", op)?;
    let mut saida = BufWriter::with_capacity(MIB as usize, arquivo);
    let mut limitado = (&mut leitor).take(op.max_saida + 1);
    let mut n = 0;
    loop {
        let lidos = io::copy(&mut (&mut limitado).take(PASSO), &mut saida)?;
        n += lidos;
        if lidos < PASSO {
            break;
        }
        espaco(&dir, origem)?;
    }
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
                "{origem} passou do teto de {}; grave num arquivo e abra o arquivo",
                if op.max_saida >= MIB {
                    format!("{} MiB", op.max_saida / MIB)
                } else {
                    format!("{} bytes", op.max_saida)
                }
            ),
        ));
    }
    Ok(tmp)
}

/// Erro se o disco de `dir` tem menos de `MARGEM` livre.
fn espaco(dir: &Path, origem: &str) -> io::Result<()> {
    let livre = fs4::available_space(dir)?;
    if livre < MARGEM {
        return Err(io::Error::new(
            ErrorKind::StorageFull,
            format!(
                "sem espaço para gravar {origem}: {} MiB livres em {} (precisa de {} MiB); libere espaço",
                livre / MIB,
                dir.display(),
                MARGEM / MIB
            ),
        ));
    }
    Ok(())
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

    fn envelhecer(c: &Path) {
        File::options()
            .write(true)
            .open(c)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
    }

    #[test]
    fn grava_0600_e_apaga_ao_sair() {
        let d = tempfile::tempdir().unwrap();
        let t = gravar(&b"{\"a\":1}\n"[..], "stdin", &op(d.path())).unwrap();
        let c = t.caminho().to_path_buf();
        assert!(temporario(&c));
        assert_eq!(fs::read(&c).unwrap(), b"{\"a\":1}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&c).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(VIVOS.lock().unwrap().contains(&c));
        drop(t);
        assert!(!c.exists());
        assert!(!VIVOS.lock().unwrap().contains(&c));
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
        assert!(gravar(&b"1234"[..], "stdin", &pequeno).is_ok());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
    }

    #[test]
    fn orfao_sai_e_vivo_fica() {
        let d = tempfile::tempdir().unwrap();
        // Vivo: travado por um `Temporario` deste processo, envelhecido.
        let vivo = gravar(&b"{}"[..], "stdin", &op(d.path())).unwrap();
        envelhecer(vivo.caminho());
        let orfao = d.path().join("entrada.1.2.parcial");
        fs::write(&orfao, "x").unwrap();
        envelhecer(&orfao);
        // Novo demais: o dono pode não ter travado ainda.
        let novo = d.path().join("entrada.3.4.parcial");
        fs::write(&novo, "x").unwrap();
        // Órfão de download também sai; `.parcial` do descompactado (sem prefixo) fica.
        let download = d.path().join("download.7.8.parcial");
        fs::write(&download, "x").unwrap();
        envelhecer(&download);
        let descompactado = d.path().join("abc123.parcial");
        fs::write(&descompactado, "x").unwrap();
        envelhecer(&descompactado);
        assert!(criar("outro", &op(d.path())).is_err());
        // Outro arquivo do cache não é temporário.
        let outro = d.path().join("abc.dados");
        fs::write(&outro, "x").unwrap();
        envelhecer(&outro);
        assert_eq!(limpar_orfaos(&op(d.path())).unwrap(), 2);
        assert!(!orfao.exists() && !download.exists());
        assert!(novo.exists() && vivo.caminho().exists() && outro.exists());
        assert!(descompactado.exists());
    }
}
