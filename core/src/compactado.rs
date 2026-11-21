//! Entrada compactada (gzip e zstd): detecção pelos bytes mágicos e descompactação em fluxo para
//! um arquivo de cache em disco, que depois é aberto pelo caminho normal de `mmap` e índice.
//!
//! - gzip com vários membros concatenados e zstd com vários quadros saem inteiros;
//! - progresso em bytes lidos do compactado, cancelamento por `AtomicBool`;
//! - erro, cancelamento ou limite estourado apagam o cache parcial;
//! - o cache vale enquanto caminho, tamanho e data de modificação do original não mudam.

use std::fs::{self, File};
use std::io::{self, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GZIP: &[u8] = b"\x1f\x8b";
const ZSTD: &[u8] = b"\x28\xb5\x2f\xfd";
const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
/// Saída que sempre é aceita, qualquer que seja a razão (arquivo minúsculo com razão alta é normal).
const SAIDA_LIVRE: u64 = 16 * MIB;
const EXT_CACHE: &str = "dados";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formato {
    Cru,
    Gzip,
    Zstd,
}

/// Formato pelos primeiros bytes; a extensão do arquivo não conta.
pub fn detectar(inicio: &[u8]) -> Formato {
    if inicio.starts_with(GZIP) {
        Formato::Gzip
    } else if inicio.starts_with(ZSTD) {
        Formato::Zstd
    } else {
        Formato::Cru
    }
}

/// Como abrir um arquivo compactado. `Opcoes::default()` usa o cache do sistema e limites padrão.
pub struct Opcoes<'a> {
    /// Pasta do cache; `None` = cache do sistema para o app (`.../jasao/descompactados`).
    pub dir_cache: Option<PathBuf>,
    /// Soma máxima dos arquivos no cache; os mais antigos saem para caber o novo.
    pub teto_cache: u64,
    /// Tamanho máximo de um arquivo descompactado.
    pub max_saida: u64,
    /// Razão máxima saída/compactado (acima de `SAIDA_LIVRE`), contra bomba de descompactação.
    pub max_razao: u64,
    pub cancelar: Option<&'a AtomicBool>,
    /// Chamado com (bytes lidos do compactado, total do compactado), no máximo a cada 1 MiB.
    pub progresso: Option<&'a dyn Fn(u64, u64)>,
}

impl Default for Opcoes<'_> {
    fn default() -> Self {
        Self {
            dir_cache: None,
            teto_cache: 20 * GIB,
            // Plano, seção 3: o teto de tamanho é 10 GB.
            max_saida: 10 * GIB,
            max_razao: 1000,
            cancelar: None,
            progresso: None,
        }
    }
}

fn cancelado() -> io::Error {
    io::Error::new(ErrorKind::Interrupted, "abertura cancelada")
}

fn corrompido(formato: Formato, e: &io::Error) -> io::Error {
    io::Error::new(
        ErrorKind::InvalidData,
        format!(
            "{formato:?} truncado ou corrompido ({e}); confira o arquivo ou baixe de novo (ex.: `gzip -t`, `zstd -t`)"
        ),
    )
}

/// Descompacta `entrada` em `saida` respeitando limites, progresso e cancelamento.
/// `total` é o tamanho do compactado (base da razão e do progresso). Devolve os bytes escritos.
pub fn descompactar(
    entrada: impl Read,
    formato: Formato,
    total: u64,
    mut saida: impl Write,
    op: &Opcoes,
) -> io::Result<u64> {
    let lidos = AtomicU64::new(0);
    let contador = Contador {
        dentro: entrada,
        lidos: &lidos,
        avisado: 0,
        total,
        op,
    };
    let mut leitor: Box<dyn Read + '_> = match formato {
        Formato::Gzip => Box::new(flate2::read::MultiGzDecoder::new(contador)),
        Formato::Zstd => Box::new(zstd::stream::read::Decoder::new(contador)?),
        Formato::Cru => Box::new(contador),
    };
    let limite = op
        .max_saida
        .min(total.saturating_mul(op.max_razao).max(SAIDA_LIVRE));
    let mut buf = vec![0u8; 256 * 1024];
    let mut escritos = 0u64;
    loop {
        if op.cancelar.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(cancelado());
        }
        let n = match leitor.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) if op.cancelar.is_some_and(|c| c.load(Ordering::Relaxed)) => {
                return Err(cancelado());
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(corrompido(formato, &e)),
        };
        escritos += n as u64;
        if escritos > limite {
            let mb = |b: u64| b / MIB;
            return Err(io::Error::new(
                ErrorKind::FileTooLarge,
                format!(
                    "descompactado passou de {} MiB (compactado tem {} MiB; limites: {} MiB e razão {}×); \
                     parece bomba de descompactação ou é grande demais para abrir; se for confiável, \
                     descompacte fora do app e abra o resultado",
                    mb(limite),
                    mb(total),
                    mb(op.max_saida),
                    op.max_razao
                ),
            ));
        }
        saida.write_all(&buf[..n])?;
    }
    saida.flush()?;
    if let Some(p) = op.progresso {
        p(lidos.load(Ordering::Relaxed), total);
    }
    Ok(escritos)
}

/// Conta os bytes lidos do compactado, avisa o progresso e para no cancelamento.
struct Contador<'a, 'o, R> {
    dentro: R,
    lidos: &'a AtomicU64,
    avisado: u64,
    total: u64,
    op: &'a Opcoes<'o>,
}

impl<R: Read> Read for Contador<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.op.cancelar.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(cancelado());
        }
        let n = self.dentro.read(buf)?;
        let lidos = self.lidos.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        if let Some(p) = self.op.progresso
            && lidos - self.avisado >= MIB
        {
            self.avisado = lidos;
            p(lidos, self.total);
        }
        Ok(n)
    }
}

/// Pasta de cache do sistema para o app.
fn dir_cache_sistema() -> io::Result<PathBuf> {
    let env = |k| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches"))
    } else if cfg!(windows) {
        env("LOCALAPPDATA")
    } else {
        env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache")))
    };
    base.map(|b| b.join("jasao").join("descompactados"))
        .ok_or_else(|| {
            io::Error::new(
                ErrorKind::NotFound,
                "não achei a pasta de cache do sistema (HOME/LOCALAPPDATA vazio); defina HOME",
            )
        })
}

/// FNV-1a de 64 bits: estável entre versões do Rust (o `DefaultHasher` não promete isso).
fn fnv(partes: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in partes {
        for &b in *p {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Apaga o arquivo parcial ao sair por erro, cancelamento ou pânico.
struct Parcial(Option<PathBuf>);

impl Drop for Parcial {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(p);
        }
    }
}

/// Lê o tamanho que o próprio arquivo declara, quando declara: ISIZE do gzip (último membro,
/// módulo 4 GiB) ou o tamanho do primeiro quadro zstd. Só serve de estimativa.
fn tamanho_declarado(arquivo: &mut File, formato: Formato, total: u64) -> io::Result<u64> {
    let declarado = match formato {
        Formato::Gzip if total >= 18 => {
            let mut fim = [0u8; 4];
            arquivo.seek(SeekFrom::End(-4))?;
            arquivo.read_exact(&mut fim)?;
            u64::from(u32::from_le_bytes(fim))
        }
        Formato::Zstd => {
            let mut cab = [0u8; 18];
            arquivo.seek(SeekFrom::Start(0))?;
            let n = arquivo.read(&mut cab)?;
            zstd::zstd_safe::get_frame_content_size(&cab[..n])
                .ok()
                .flatten()
                .unwrap_or(0)
        }
        _ => 0,
    };
    arquivo.seek(SeekFrom::Start(0))?;
    Ok(declarado)
}

/// Descompacta `caminho` para o cache (ou reaproveita o que já está lá) e devolve o caminho do
/// arquivo descompactado.
pub fn para_cache(caminho: &Path, formato: Formato, op: &Opcoes) -> io::Result<PathBuf> {
    let mut arquivo = File::open(caminho)?;
    let meta = arquivo.metadata()?;
    let total = meta.len();
    let mtime = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = match &op.dir_cache {
        Some(d) => d.clone(),
        None => dir_cache_sistema()?,
    };
    fs::create_dir_all(&dir)?;
    let canonico = fs::canonicalize(caminho)?;
    let chave = fnv(&[
        canonico.as_os_str().as_encoded_bytes(),
        &total.to_le_bytes(),
        &mtime.to_le_bytes(),
    ]);
    let destino = dir.join(format!("{chave:016x}.{EXT_CACHE}"));

    if destino.is_file() {
        // Marca como usado agora: a limpeza tira primeiro o que foi usado há mais tempo.
        let _ = File::options()
            .write(true)
            .open(&destino)
            .and_then(|f| f.set_modified(SystemTime::now()));
        if let Some(p) = op.progresso {
            p(total, total);
        }
        return Ok(destino);
    }

    let estimativa = tamanho_declarado(&mut arquivo, formato, total)?
        .max(total)
        .min(op.max_saida);
    limpar(&dir, op.teto_cache.saturating_sub(estimativa), None)?;
    let livre = fs4::available_space(&dir)?;
    if livre < estimativa {
        return Err(io::Error::new(
            ErrorKind::StorageFull,
            format!(
                "sem espaço para descompactar: precisa de ~{} MiB em {}, há {} MiB livres; libere espaço",
                estimativa / MIB,
                dir.display(),
                livre / MIB
            ),
        ));
    }

    let tmp = dir.join(format!(
        "{chave:016x}.{}.{}.parcial",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut guarda = Parcial(Some(tmp.clone()));
    let saida = BufWriter::with_capacity(MIB as usize, File::create(&tmp)?);
    descompactar(&mut arquivo, formato, total, saida, op).map_err(|e| {
        if e.kind() == ErrorKind::StorageFull {
            io::Error::new(
                ErrorKind::StorageFull,
                format!(
                    "disco cheio ao descompactar em {}; libere espaço",
                    dir.display()
                ),
            )
        } else {
            e
        }
    })?;
    // Renomear é atômico: quem achar o destino acha o arquivo inteiro.
    fs::rename(&tmp, &destino)?;
    guarda.0 = None;
    limpar(&dir, op.teto_cache, Some(&destino))?;
    Ok(destino)
}

/// Tira do cache os arquivos usados há mais tempo até a soma caber em `teto`. Nunca tira
/// `manter`. Parciais com mais de um dia (sobra de processo que morreu) também saem.
fn limpar(dir: &Path, teto: u64, manter: Option<&Path>) -> io::Result<()> {
    let agora = SystemTime::now();
    let mut entradas = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        let caminho = e.path();
        let meta = e.metadata()?;
        let quando = meta.modified()?;
        match caminho.extension().and_then(|x| x.to_str()) {
            Some(EXT_CACHE) => entradas.push((quando, meta.len(), caminho)),
            Some("parcial")
                if agora.duration_since(quando).unwrap_or_default()
                    > Duration::from_secs(86_400) =>
            {
                let _ = fs::remove_file(&caminho);
            }
            _ => {}
        }
    }
    entradas.sort();
    let mut soma: u64 = entradas.iter().map(|e| e.1).sum();
    for (_, tamanho, caminho) in entradas {
        if soma <= teto {
            break;
        }
        if Some(caminho.as_path()) == manter {
            continue;
        }
        // Outro processo pode ter apagado antes; o que importa é a soma cair.
        let _ = fs::remove_file(&caminho);
        soma -= tamanho;
    }
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn detecta_pelos_bytes() {
        assert_eq!(detectar(b"\x1f\x8b\x08"), Formato::Gzip);
        assert_eq!(detectar(b"\x28\xb5\x2f\xfd"), Formato::Zstd);
        assert_eq!(detectar(b"\x28\xb5\x2f"), Formato::Cru);
        assert_eq!(detectar(b"\x1f"), Formato::Cru);
        assert_eq!(detectar(b"{}"), Formato::Cru);
        assert_eq!(detectar(b""), Formato::Cru);
    }
}
