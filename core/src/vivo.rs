//! Arquivo vivo (como `tail -f`): segue um JSONL que cresce, sem nunca mapear o arquivo do usuário.
//!
//! Por que uma cópia: `mmap` do original dá SIGBUS se outro processo truncar o arquivo (achado
//! 14), e nenhuma checagem antes do acesso fecha essa corrida. Aqui o original só é lido com
//! `read` (truncou no meio: a leitura volta curta, nunca derruba o processo) e o que se mapeia é
//! uma cópia que só o app conhece e que só cresce. A cópia guarda só linhas completas: a última
//! linha sem `\n` espera a próxima leitura. O índice de linhas cresce junto, sem revarrer o início.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use memmap2::Mmap;

use crate::compactado::{self, Formato, Opcoes};
use crate::linhas::{ArquivoLinhas, IndiceLinhas, arquivo_comum};

/// Últimos bytes copiados que cada leitura confere no original: reescrita no mesmo arquivo
/// (truncar e escrever de novo, maior) muda esses bytes mesmo quando o tamanho só cresce.
const RABO: usize = 64;
const BLOCO: usize = 1 << 20;

/// O que mudou no original desde a última leitura.
pub enum Mudanca {
    Nada,
    /// Chegaram linhas completas: o arquivo novo tem as antigas (mesmos bytes) e as novas.
    Cresceu(ArquivoLinhas),
    /// Truncado, rotacionado ou reescrito: o que estava na tela não é mais o arquivo. Quem
    /// segue reabre com `Vivo::abrir`.
    Reescrito,
}

/// Um JSONL seguido: de onde lê, quanto já copiou e a cópia mapeável.
pub struct Vivo {
    origem: PathBuf,
    /// (dispositivo, inode) do original; rotação troca o arquivo por trás do mesmo caminho.
    identidade: Option<(u64, u64)>,
    /// Bytes do original já copiados; sempre logo depois de um `\n`.
    lidos: u64,
    rabo: Vec<u8>,
    copia: File,
    max: u64,
    indice: IndiceLinhas,
    /// Bytes da cópia que o índice já cobre.
    indexados: usize,
}

impl Vivo {
    /// Copia as linhas completas de `caminho` e devolve o arquivo pronto para as visões.
    /// Recusa compactado (o cache dele já é outra cópia, que não cresce).
    pub fn abrir(caminho: impl AsRef<Path>, op: &Opcoes) -> io::Result<(Self, ArquivoLinhas)> {
        let origem = caminho.as_ref().to_path_buf();
        arquivo_comum(&origem)?;
        let mut f = File::open(&origem)?;
        let mut magico = [0u8; 4];
        let n = Read::by_ref(&mut f).take(4).read(&mut magico)?;
        if compactado::detectar(&magico[..n]) != Formato::Cru {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "arquivo compactado não dá para seguir; descompacte e abra o JSONL",
            ));
        }
        let mut v = Self {
            identidade: identidade(&f.metadata()?),
            origem,
            lidos: 0,
            rabo: Vec::new(),
            copia: criar_copia(&compactado::pasta_cache(op)?)?,
            max: op.max_saida,
            indice: IndiceLinhas::default(),
            indexados: 0,
        };
        v.copiar(&mut f)?;
        let arquivo = v.mapear()?;
        Ok((v, arquivo))
    }

    /// Lê o que o original ganhou desde a última vez. Original sumido é erro (pode voltar numa
    /// rotação: quem segue tenta de novo).
    pub fn atualizar(&mut self) -> io::Result<Mudanca> {
        let mut f = File::open(&self.origem)?;
        let meta = f.metadata()?;
        if identidade(&meta) != self.identidade || !self.rabo_confere(&mut f)? {
            return Ok(Mudanca::Reescrito);
        }
        let antes = self.lidos;
        self.copiar(&mut f)?;
        if self.lidos == antes {
            return Ok(Mudanca::Nada);
        }
        Ok(Mudanca::Cresceu(self.mapear()?))
    }

    /// Os últimos bytes copiados ainda estão no mesmo lugar do original.
    fn rabo_confere(&self, f: &mut File) -> io::Result<bool> {
        let mut agora = vec![0u8; self.rabo.len()];
        f.seek(SeekFrom::Start(self.lidos - self.rabo.len() as u64))?;
        match f.read_exact(&mut agora) {
            Ok(()) => Ok(agora == self.rabo),
            // Encolheu entre o `metadata` e a leitura.
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Copia do original, a partir de `lidos`, tudo até o último `\n` que já chegou.
    fn copiar(&mut self, f: &mut File) -> io::Result<()> {
        f.seek(SeekFrom::Start(self.lidos))?;
        // shortcut: a linha parcial fica toda na memória até o `\n` chegar; ler em blocos com
        // reinício se aparecer log com linha parcial de centenas de MB.
        let mut pendente = Vec::new();
        let mut bloco = vec![0u8; BLOCO];
        loop {
            let n = match f.read(&mut bloco) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            pendente.extend_from_slice(&bloco[..n]);
            let Some(p) = memchr::memrchr(b'\n', &pendente) else {
                continue;
            };
            let completo = &pendente[..=p];
            if self.lidos + completo.len() as u64 > self.max {
                return Err(io::Error::new(
                    ErrorKind::FileTooLarge,
                    "o arquivo seguido passou do teto de tamanho; pause e reabra",
                ));
            }
            self.copia.write_all(completo)?;
            self.lidos += completo.len() as u64;
            if completo.len() >= RABO {
                self.rabo = completo[completo.len() - RABO..].to_vec();
            } else {
                self.rabo.extend_from_slice(completo);
                let sobra = self.rabo.len().saturating_sub(RABO);
                self.rabo.drain(..sobra);
            }
            pendente.drain(..=p);
        }
        Ok(())
    }

    /// Mapeia a cópia inteira e estende o índice com o que ainda não estava nele.
    fn mapear(&mut self) -> io::Result<ArquivoLinhas> {
        if self.lidos == 0 {
            return Ok(ArquivoLinhas::de_mapa(None, IndiceLinhas::default()));
        }
        // SAFETY: a cópia é só do app (criada com `create_new`, apagada do diretório no Unix) e
        // só cresce; nenhum outro processo a encolhe por baixo do mapa.
        let mapa = unsafe { Mmap::map(&self.copia)? };
        self.indice.estender(&mapa, self.indexados);
        self.indexados = mapa.len();
        // shortcut: cada crescimento clona o vetor de inícios (8 bytes por linha); dividir em
        // blocos compartilhados se seguir arquivo de dezenas de milhões de linhas ficar lento.
        Ok(ArquivoLinhas::de_mapa(Some(mapa), self.indice.clone()))
    }
}

#[cfg(unix)]
fn identidade(m: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((m.dev(), m.ino()))
}

// shortcut: fora do Unix a rotação só é notada pelo tamanho e pelo rabo; usar o file index do
// Windows se o app for distribuído lá.
#[cfg(not(unix))]
fn identidade(_: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// Arquivo novo e exclusivo no cache. No Unix sai do diretório na hora: vive só enquanto o app
/// o tem aberto ou mapeado, e nada sobra em disco se o app cair.
fn criar_copia(dir: &Path) -> io::Result<File> {
    let caminho = dir.join(format!(
        "vivo.{}.{}.jsonl",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let f = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&caminho)?;
    compactado::privado(&caminho, 0o600)?;
    // shortcut: no Windows o arquivo aberto não sai do diretório e fica no cache até a limpeza.
    #[cfg(unix)]
    fs::remove_file(&caminho)?;
    Ok(f)
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::fs::OpenOptions;

    fn op(dir: &Path) -> Opcoes<'static> {
        Opcoes {
            dir_cache: Some(dir.join("cache")),
            ..Opcoes::default()
        }
    }

    fn acrescentar(caminho: &Path, dados: &[u8]) {
        OpenOptions::new()
            .append(true)
            .open(caminho)
            .unwrap()
            .write_all(dados)
            .unwrap();
    }

    fn linhas(a: &ArquivoLinhas) -> Vec<String> {
        a.janela(0, usize::MAX)
            .iter()
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect()
    }

    fn cresceu(m: Mudanca) -> ArquivoLinhas {
        match m {
            Mudanca::Cresceu(a) => a,
            Mudanca::Nada => panic!("esperava Cresceu, veio Nada"),
            Mudanca::Reescrito => panic!("esperava Cresceu, veio Reescrito"),
        }
    }

    /// Estender a partir de qualquer fim de linha dá o mesmo índice que varrer tudo (CRLF, BOM,
    /// linha vazia). Derruba: esquecer de empurrar `de`, deslocar os inícios, não tirar a quebra
    /// final.
    #[test]
    fn estender_igual_a_construir() {
        let dados: &[u8] = b"\xEF\xBB\xBF{\"a\":1}\r\n\n{\"b\":2}\n[3]\r\n{}\n";
        for de in (1..=dados.len()).filter(|&k| dados[k - 1] == b'\n') {
            let mut ind = IndiceLinhas::construir(&dados[..de]);
            ind.estender(dados, de);
            assert_eq!(ind, IndiceLinhas::construir(dados), "de = {de}");
        }
        let mut vazio = IndiceLinhas::default();
        vazio.estender(dados, 0);
        assert_eq!(vazio, IndiceLinhas::construir(dados));
    }

    /// Crescimento: as linhas novas entram no índice e as antigas ficam como estavam.
    #[test]
    fn crescimento() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        fs::write(&log, "{\"i\":0}\n{\"i\":1}\n").unwrap();
        let (mut v, a) = Vivo::abrir(&log, &op(pasta.path())).unwrap();
        assert_eq!(linhas(&a), ["{\"i\":0}", "{\"i\":1}"]);
        assert!(matches!(v.atualizar().unwrap(), Mudanca::Nada));
        acrescentar(&log, b"{\"i\":2}\r\n{\"i\":3}\n");
        let b = cresceu(v.atualizar().unwrap());
        assert_eq!(
            linhas(&b),
            ["{\"i\":0}", "{\"i\":1}", "{\"i\":2}", "{\"i\":3}"]
        );
        // O arquivo de antes continua válido e igual (as visões em 2º plano ainda o leem).
        assert_eq!(linhas(&a), ["{\"i\":0}", "{\"i\":1}"]);
        acrescentar(&log, b"{\"i\":4}\n");
        assert_eq!(cresceu(v.atualizar().unwrap()).linhas(), 5);
    }

    /// A última linha sem `\n` espera: não aparece pela metade e entra inteira quando termina.
    #[test]
    fn linha_parcial() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        fs::write(&log, "{\"i\":0}\n{\"i\":").unwrap();
        let (mut v, a) = Vivo::abrir(&log, &op(pasta.path())).unwrap();
        assert_eq!(linhas(&a), ["{\"i\":0}"]);
        acrescentar(&log, b"1");
        assert!(matches!(v.atualizar().unwrap(), Mudanca::Nada));
        acrescentar(&log, b"}\n{\"i\"");
        let b = cresceu(v.atualizar().unwrap());
        assert_eq!(linhas(&b), ["{\"i\":0}", "{\"i\":1}"]);
    }

    /// Truncar o original com o arquivo aberto e lido: com o original mapeado, ler `a` depois do
    /// `set_len(0)` dá SIGBUS e derruba o processo de teste. Aqui lê os bytes de antes e avisa.
    #[test]
    fn truncamento_sem_sigbus() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        let linha = format!("{{\"texto\":\"{}\"}}\n", "x".repeat(200));
        // Várias páginas: o SIGBUS só vem em página além do novo fim.
        fs::write(&log, linha.repeat(2_000)).unwrap();
        let (mut v, a) = Vivo::abrir(&log, &op(pasta.path())).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_len(0)
            .unwrap();
        let soma: u64 = a.bytes().iter().map(|&b| u64::from(b)).sum();
        assert!(soma > 0);
        assert_eq!(a.linhas(), 2_000);
        assert!(matches!(v.atualizar().unwrap(), Mudanca::Reescrito));
        // Truncado e reescrito maior antes da leitura seguinte (mesmo inode): o rabo denuncia.
        fs::write(&log, "{\"a\":1}\n").unwrap();
        let (mut w, _) = Vivo::abrir(&log, &op(pasta.path())).unwrap();
        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&log)
            .unwrap()
            .write_all(b"{\"b\":22}\n{\"b\":3}\n")
            .unwrap();
        assert!(matches!(w.atualizar().unwrap(), Mudanca::Reescrito));
    }

    /// Rotação (`mv log log.1` e um `log` novo): o novo começa com os mesmos bytes e é maior,
    /// então só a identidade do arquivo denuncia a troca.
    #[cfg(unix)]
    #[test]
    fn rotacao() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        fs::write(&log, "{\"i\":0}\n").unwrap();
        let (mut v, _) = Vivo::abrir(&log, &op(pasta.path())).unwrap();
        fs::rename(&log, pasta.path().join("log.jsonl.1")).unwrap();
        assert!(
            v.atualizar().is_err(),
            "sem o arquivo no caminho é erro, não mudança"
        );
        fs::write(&log, "{\"i\":0}\n{\"i\":1}\n").unwrap();
        assert!(matches!(v.atualizar().unwrap(), Mudanca::Reescrito));
        let (_, a) = Vivo::abrir(&log, &op(pasta.path())).unwrap();
        assert_eq!(a.linhas(), 2);
    }

    #[test]
    fn recusa_compactado() {
        let pasta = tempfile::tempdir().unwrap();
        let gz = pasta.path().join("log.jsonl.gz");
        fs::write(&gz, b"\x1f\x8b\x08\x00").unwrap();
        let erro = Vivo::abrir(&gz, &op(pasta.path())).err().unwrap();
        assert!(erro.to_string().contains("compactado"), "{erro}");
    }

    /// A cópia não fica no cache (Unix): nada sobra em disco se o app cair.
    #[cfg(unix)]
    #[test]
    fn copia_nao_fica_no_disco() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        fs::write(&log, "{}\n").unwrap();
        let (_v, _a) = Vivo::abrir(&log, &op(pasta.path())).unwrap();
        assert_eq!(fs::read_dir(pasta.path().join("cache")).unwrap().count(), 0);
    }
}
