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

use memmap2::MmapOptions;

use crate::compactado::{self, Formato, Opcoes};
use crate::formatos;
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

/// Como estava o arquivo que o documento mostra ao ser aberto sem seguir: o 1º `seguir`
/// confere se ainda é o mesmo antes de só acrescentar (lido com `read`, nunca pelo mapa).
pub struct Marca {
    identidade: Option<(u64, u64)>,
    tamanho: u64,
    rabo: Vec<u8>,
}

impl Marca {
    /// Marca de `caminho` com os `tamanho` bytes que o documento tem dele.
    pub fn ler(caminho: impl AsRef<Path>, tamanho: u64) -> io::Result<Self> {
        let mut f = File::open(caminho)?;
        let identidade = identidade(&f.metadata()?);
        let mut rabo = vec![0u8; tamanho.min(RABO as u64) as usize];
        f.seek(SeekFrom::Start(tamanho - rabo.len() as u64))?;
        f.read_exact(&mut rabo)?;
        Ok(Self {
            identidade,
            tamanho,
            rabo,
        })
    }

    /// `f` ainda é o arquivo marcado: mesma identidade e os mesmos últimos bytes no lugar.
    fn confere(&self, f: &mut File) -> io::Result<bool> {
        let meta = f.metadata()?;
        if identidade(&meta) != self.identidade || meta.len() < self.tamanho {
            return Ok(false);
        }
        bytes_em(f, self.tamanho, &self.rabo)
    }
}

/// Os bytes que terminam em `fim` no arquivo são `esperado` (o arquivo encolheu: não são).
fn bytes_em(f: &mut File, fim: u64, esperado: &[u8]) -> io::Result<bool> {
    let mut agora = vec![0u8; esperado.len()];
    f.seek(SeekFrom::Start(fim - esperado.len() as u64))?;
    match f.read_exact(&mut agora) {
        Ok(()) => Ok(agora == esperado),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

/// Um JSONL seguido: de onde lê, quanto já copiou e a cópia mapeável.
pub struct Vivo {
    origem: PathBuf,
    /// (dispositivo, inode) do original; rotação troca o arquivo por trás do mesmo caminho.
    identidade: Option<(u64, u64)>,
    /// Bytes do original já lidos (e gravados na cópia), com a linha parcial do fim.
    lidos: u64,
    /// Bytes da cópia até o último `\n`: só eles são mapeados e indexados.
    copiados: u64,
    /// Últimos `RABO` bytes lidos (os da linha parcial também).
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
        let (v, a, _) = Self::continuar(caminho, op, None)?;
        Ok((v, a))
    }

    /// Como `abrir`, e diz se o arquivo ainda é o de `marca` (o que o documento mostra): só então
    /// as linhas copiadas podem entrar como acréscimo ao que já está na tela. Sem marca: não é.
    pub fn continuar(
        caminho: impl AsRef<Path>,
        op: &Opcoes,
        marca: Option<&Marca>,
    ) -> io::Result<(Self, ArquivoLinhas, bool)> {
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
        // Convertido (YAML, BSON…): a cópia seria dos bytes crus, não do JSONL que a aba mostra.
        if let Some(e) = formatos::detectar(&origem, &mut File::open(&origem)?)? {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("arquivo {e:?} não dá para seguir; só JSONL"),
            ));
        }
        let mesmo = match marca {
            Some(m) => m.confere(&mut f)?,
            None => false,
        };
        let mut v = Self {
            identidade: identidade(&f.metadata()?),
            origem,
            lidos: 0,
            copiados: 0,
            rabo: Vec::new(),
            copia: criar_copia(&compactado::pasta_cache(op)?)?,
            max: op.max_saida,
            indice: IndiceLinhas::default(),
            indexados: 0,
        };
        v.copiar(&mut f)?;
        let arquivo = v.mapear()?;
        Ok((v, arquivo, mesmo))
    }

    /// Lê o que o original ganhou desde a última vez. Original sumido é erro (pode voltar numa
    /// rotação: quem segue tenta de novo).
    pub fn atualizar(&mut self) -> io::Result<Mudanca> {
        let mut f = File::open(&self.origem)?;
        let meta = f.metadata()?;
        if identidade(&meta) != self.identidade || !self.rabo_confere(&mut f)? {
            return Ok(Mudanca::Reescrito);
        }
        let antes = self.copiados;
        self.copiar(&mut f)?;
        if self.copiados == antes {
            return Ok(Mudanca::Nada);
        }
        Ok(Mudanca::Cresceu(self.mapear()?))
    }

    /// Os últimos bytes lidos ainda estão no mesmo lugar do original.
    fn rabo_confere(&self, f: &mut File) -> io::Result<bool> {
        bytes_em(f, self.lidos, &self.rabo)
    }

    /// Lê do original, a partir de `lidos`, tudo o que já chegou e grava na cópia; só a parte
    /// até o último `\n` passa a valer (`copiados`). A linha parcial fica no disco, não na
    /// memória, e não é relida: a próxima leitura continua de onde esta parou.
    fn copiar(&mut self, f: &mut File) -> io::Result<()> {
        f.seek(SeekFrom::Start(self.lidos))?;
        let mut bloco = vec![0u8; BLOCO];
        loop {
            let n = match f.read(&mut bloco) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if self.lidos + n as u64 > self.max {
                return Err(io::Error::new(
                    ErrorKind::FileTooLarge,
                    "o arquivo seguido passou do teto de tamanho; pause e reabra",
                ));
            }
            let lido = &bloco[..n];
            self.copia.write_all(lido)?;
            if let Some(p) = memchr::memrchr(b'\n', lido) {
                self.copiados = self.lidos + p as u64 + 1;
            }
            self.lidos += n as u64;
            self.rabo.extend_from_slice(&lido[n.saturating_sub(RABO)..]);
            let sobra = self.rabo.len().saturating_sub(RABO);
            self.rabo.drain(..sobra);
        }
        Ok(())
    }

    /// Mapeia a cópia inteira e estende o índice com o que ainda não estava nele.
    fn mapear(&mut self) -> io::Result<ArquivoLinhas> {
        if self.copiados == 0 {
            return ArquivoLinhas::de_mapa(&self.copia, None, IndiceLinhas::default());
        }
        // SAFETY: a cópia é só do app (criada com `create_new`, apagada do diretório no Unix) e
        // só cresce; nenhum outro processo a encolhe por baixo do mapa.
        // Só até o último `\n`: a linha parcial gravada depois dele não entra.
        let mapa = unsafe {
            MmapOptions::new()
                .len(self.copiados as usize)
                .map(&self.copia)?
        };
        self.indice.estender(&mapa, self.indexados);
        self.indexados = mapa.len();
        // shortcut: cada crescimento clona o vetor de inícios (8 bytes por linha); dividir em
        // blocos compartilhados se seguir arquivo de dezenas de milhões de linhas ficar lento.
        ArquivoLinhas::de_mapa(&self.copia, Some(mapa), self.indice.clone())
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

    /// Teto de tamanho: passar dele é erro, também quando é uma linha só sem `\n` (JSON
    /// minificado), que senão cresceria sem limite.
    #[test]
    fn teto() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        let op = Opcoes {
            max_saida: 20,
            ..op(pasta.path())
        };
        fs::write(&log, "{\"i\":0}\n").unwrap();
        let (mut v, _) = Vivo::abrir(&log, &op).unwrap();
        acrescentar(&log, b"{\"i\":1}\n");
        assert_eq!(cresceu(v.atualizar().unwrap()).linhas(), 2);
        acrescentar(&log, b"{\"i\":2}\n");
        let erro = v.atualizar().err().unwrap();
        assert_eq!(erro.kind(), ErrorKind::FileTooLarge, "{erro}");
        fs::write(&log, format!("[{}]", "1,".repeat(20))).unwrap();
        let erro = Vivo::abrir(&log, &op).err().unwrap();
        assert_eq!(erro.kind(), ErrorKind::FileTooLarge, "{erro}");
    }

    /// A linha parcial não é relida a cada leitura: o que já chegou dela foi lido uma vez. Se
    /// ela muda por baixo (reescrita no meio da linha), o rabo, que a inclui, denuncia.
    #[test]
    fn parcial_reescrita() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        fs::write(&log, "{\"i\":0}\n{\"i\":").unwrap();
        let (mut v, _) = Vivo::abrir(&log, &op(pasta.path())).unwrap();
        assert_eq!(v.lidos, 13);
        assert!(matches!(v.atualizar().unwrap(), Mudanca::Nada));
        assert_eq!(v.lidos, 13);
        let mut f = OpenOptions::new().write(true).open(&log).unwrap();
        f.seek(SeekFrom::Start(9)).unwrap();
        f.write_all(b"j").unwrap();
        assert!(matches!(v.atualizar().unwrap(), Mudanca::Reescrito));
    }

    /// 1º seguir de um arquivo aberto sem seguir: só continua se ainda é o marcado no `abrir`.
    #[test]
    fn continuar_confere_a_marca() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        fs::write(&log, "{\"a\":[1,2,3]}\n{\"i\":1}\n").unwrap();
        let marca = Marca::ler(&log, 22).unwrap();
        acrescentar(&log, b"{\"i\":2}\n");
        let (_, a, mesmo) = Vivo::continuar(&log, &op(pasta.path()), Some(&marca)).unwrap();
        assert!(mesmo);
        assert_eq!(a.linhas(), 3);
        // Trocado por outro com mais linhas (mesmo inode e nome): não é acréscimo.
        fs::write(&log, "{\"z\":1}\n{\"z\":2}\n{\"z\":3}\n{\"z\":4}\n").unwrap();
        let (_, a, mesmo) = Vivo::continuar(&log, &op(pasta.path()), Some(&marca)).unwrap();
        assert!(!mesmo);
        assert_eq!(a.linhas(), 4);
        assert!(!Vivo::continuar(&log, &op(pasta.path()), None).unwrap().2);
    }

    #[test]
    fn recusa_compactado() {
        let pasta = tempfile::tempdir().unwrap();
        let gz = pasta.path().join("log.jsonl.gz");
        fs::write(&gz, b"\x1f\x8b\x08\x00").unwrap();
        let erro = Vivo::abrir(&gz, &op(pasta.path())).err().unwrap();
        assert!(erro.to_string().contains("compactado"), "{erro}");
    }

    #[test]
    fn recusa_convertido() {
        let pasta = tempfile::tempdir().unwrap();
        let yaml = pasta.path().join("log.yaml");
        fs::write(&yaml, "a: 1\n").unwrap();
        let erro = Vivo::abrir(&yaml, &op(pasta.path())).err().unwrap();
        assert!(erro.to_string().contains("não dá para seguir"), "{erro}");
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
