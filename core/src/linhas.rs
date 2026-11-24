//! Índice de linhas de JSONL: offsets de início de cada linha, sem copiar o arquivo.
//!
//! Regras (iguais às de `str::lines`, mais BOM):
//! - quebra é `\n`; um `\r` logo antes dela sai da linha (CRLF); `\r` solto é dado;
//! - BOM UTF-8 no início do arquivo não faz parte da primeira linha;
//! - quebra final não cria linha vazia; arquivo vazio (ou só BOM) tem zero linhas.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use memmap2::Mmap;
use rayon::prelude::*;

use crate::compactado::{self, Formato, Opcoes};
use crate::formatos;
use crate::mascara;

pub(crate) const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Onde começa e termina cada linha de um buffer. Não guarda os bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndiceLinhas {
    inicios: Vec<usize>,
    /// Fim (exclusivo) da última linha, já sem a quebra final.
    fim: usize,
}

impl IndiceLinhas {
    pub fn construir(dados: &[u8]) -> Self {
        let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
        let corpo = &dados[base..];
        if corpo.is_empty() {
            return Self::default();
        }
        // shortcut: varredura de uma thread (memchr, vários GB/s); paralelizar com rayon se o
        // benchmark de 1 GB passar da meta de 2 s.
        let mut inicios = Vec::with_capacity(corpo.len() / 256 + 1);
        inicios.push(base);
        inicios.extend(memchr::memchr_iter(b'\n', corpo).map(|p| base + p + 1));
        let termina_em_quebra = inicios.last() == Some(&dados.len());
        if termina_em_quebra {
            inicios.pop();
        }
        let fim = dados.len() - usize::from(termina_em_quebra);
        Self { inicios, fim }
    }

    /// Acrescenta as linhas de `dados[de..]` a um índice feito sobre `dados[..de]`, sem varrer de
    /// novo o que já estava indexado. `de` tem de ser 0 ou cair logo depois de um `\n` (o arquivo
    /// vivo só confirma linhas completas); dá o mesmo índice que `construir(dados)`.
    pub fn estender(&mut self, dados: &[u8], de: usize) {
        if de == 0 {
            *self = Self::construir(dados);
            return;
        }
        debug_assert_eq!(dados[de - 1], b'\n', "estender fora de fim de linha");
        if de >= dados.len() {
            return;
        }
        self.inicios.push(de);
        self.inicios
            .extend(memchr::memchr_iter(b'\n', &dados[de..]).map(|p| de + p + 1));
        let termina_em_quebra = self.inicios.last() == Some(&dados.len());
        if termina_em_quebra {
            self.inicios.pop();
        }
        self.fim = dados.len() - usize::from(termina_em_quebra);
    }

    pub fn len(&self) -> usize {
        self.inicios.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inicios.is_empty()
    }

    /// Faixa de bytes da linha `i` (sem `\n` nem `\r` final), ou `None` fora do índice.
    pub fn faixa(&self, i: usize, dados: &[u8]) -> Option<Range<usize>> {
        let inicio = *self.inicios.get(i)?;
        let mut fim = match self.inicios.get(i + 1) {
            Some(prox) => prox - 1,
            None => self.fim,
        };
        // `\r` só sai se vier antes de um `\n` (CRLF); no fim sem quebra ele é dado.
        if fim > inicio && fim < dados.len() && dados[fim - 1] == b'\r' {
            fim -= 1;
        }
        Some(inicio..fim)
    }

    /// Linha que contém o byte `b` (`None` com o índice vazio).
    pub fn linha_do_byte(&self, b: usize) -> Option<usize> {
        self.inicios.partition_point(|&i| i <= b).checked_sub(1)
    }
}

/// Arquivo JSONL aberto por `mmap`, com o índice de linhas pronto.
pub struct ArquivoLinhas {
    // Arquivo vazio não é mapeado: mmap de tamanho zero falha em algumas plataformas.
    // shortcut: se outro processo truncar o arquivo enquanto ele está mapeado, o acesso pode
    // gerar SIGBUS (achado 14). Quem espera mudança usa `vivo::Vivo`, que mapeia uma cópia do
    // app; abrir sem seguir continua mapeando o original (mais rápido na 1ª tela).
    mapa: Option<Mmap>,
    /// Bytes embutidos no binário (`estatico`); vazio quando há `mapa`.
    estatico: &'static [u8],
    indice: IndiceLinhas,
    /// Cópia com os segredos mascarados (`mascara`), mesmo tamanho e mesmas posições; `None` com
    /// `sem_mascara` ou arquivo vazio.
    mascara: Option<Mascara>,
}

/// Linhas por bloco da máscara preguiçosa.
const BLOCO: usize = 1 << 14;

/// Cópia mascarada feita por blocos de linhas, sob demanda: a 1ª tela (`janela`) só mascara os
/// blocos que mostra; `bytes()` mascara o resto antes de devolver o arquivo inteiro. JSONL tem
/// blocos independentes (em paralelo); documento em várias linhas é lido em ordem, cada bloco
/// continuando de onde o anterior parou (`mascara::Estado`): a 1ª tela mascara só o 1º bloco.
/// Do mapa é cópia privada (copy-on-write): só as páginas com segredo ocupam memória.
struct Mascara {
    /// Dono da cópia; só é lida e escrita por `ptr` (memória estável: mapa ou heap).
    _dono: Box<dyn Send + Sync>,
    ptr: *mut u8,
    len: usize,
    /// Documento em várias linhas (não `mascara::por_linha`): o próximo bloco a ler e onde a
    /// leitura parou.
    documento: Option<Box<Mutex<(usize, mascara::Estado)>>>,
    /// Por bloco: já mascarado, e se algo mudou.
    blocos: Vec<OnceLock<bool>>,
    /// Todos os blocos mascarados, e se algo mudou.
    tudo: OnceLock<bool>,
}

// SAFETY: cada bloco da cópia é escrito uma vez só, antes do seu `OnceLock` (um escritor: o
// `get_or_init` dele no JSONL, o `Mutex` no documento), e só é lido depois dele (o `OnceLock`
// ordena a escrita antes da leitura); blocos não se sobrepõem.
unsafe impl Send for Mascara {}
unsafe impl Sync for Mascara {}

impl Mascara {
    fn nova<C>(mut copia: C, dados: &[u8], indice: &IndiceLinhas) -> Self
    where
        C: std::ops::DerefMut<Target = [u8]> + Send + Sync + 'static,
    {
        let documento = (!mascara::por_linha(dados, indice)).then(Box::default);
        Self {
            ptr: copia.as_mut_ptr(),
            len: copia.len(),
            _dono: Box::new(copia),
            documento,
            blocos: (0..indice.len().div_ceil(BLOCO))
                .map(|_| OnceLock::new())
                .collect(),
            tudo: OnceLock::new(),
        }
    }

    fn bloco_de(&self, linha: usize) -> usize {
        linha / BLOCO
    }

    /// Mascara o bloco `b` (uma vez só); `true` se algo mudou nele.
    fn bloco(&self, b: usize, dados: &[u8], indice: &IndiceLinhas) -> bool {
        if let Some(&feito) = self.blocos[b].get() {
            return feito;
        }
        let Some(doc) = &self.documento else {
            return *self.blocos[b].get_or_init(|| self.escrever(b, dados, indice, None));
        };
        // Pânico no meio de um bloco deixa o estado pela metade: envenenado não se reaproveita.
        let mut g = doc.lock().expect("máscara do documento interrompida");
        while g.0 <= b {
            let k = g.0;
            let algo = self.escrever(k, dados, indice, Some(&mut g.1));
            let _ = self.blocos[k].set(algo);
            g.0 += 1;
        }
        self.blocos[b].get().copied().unwrap_or(true)
    }

    /// Escreve na cópia a máscara das linhas do bloco `b`; `true` se algo mudou.
    fn escrever(
        &self,
        b: usize,
        dados: &[u8],
        indice: &IndiceLinhas,
        estado: Option<&mut mascara::Estado>,
    ) -> bool {
        let a = b * BLOCO;
        let e = (a + BLOCO).min(indice.len());
        let inicio = |i| indice.faixa(i, dados).map_or(self.len, |f| f.start);
        let (de, ate) = (inicio(a), inicio(e));
        // SAFETY: bytes das linhas `a..e`, que só este bloco escreve; ver `unsafe impl Sync`.
        let saida = unsafe { std::slice::from_raw_parts_mut(self.ptr.add(de), ate - de) };
        mascara::mascarar_bloco(dados, indice, a..e, estado, saida, de)
    }

    /// Mascara todos os blocos (JSONL em paralelo, documento em ordem); `true` se algo mudou.
    fn tudo(&self, dados: &[u8], indice: &IndiceLinhas) -> bool {
        *self.tudo.get_or_init(|| {
            let n = self.blocos.len();
            match self.documento {
                // `|` e não `||`: todo bloco tem de ser escrito.
                Some(_) => (0..n).fold(false, |x, b| x | self.bloco(b, dados, indice)),
                None => (0..n)
                    .into_par_iter()
                    .map(|b| self.bloco(b, dados, indice))
                    .reduce(|| false, |x, y| x || y),
            }
        })
    }

    /// As linhas do bloco `b` saem da cópia? Sim se algo mudou nele; depois de `tudo`, se algo
    /// mudou no arquivo (a mesma base de `bytes()`, para quem mede posição entre os dois).
    fn da_copia(&self, b: usize, dados: &[u8], indice: &IndiceLinhas) -> bool {
        let bloco = self.bloco(b, dados, indice);
        self.tudo.get().copied().unwrap_or(bloco)
    }
}

impl ArquivoLinhas {
    /// Abre JSONL cru, gzip ou zstd (pelos bytes mágicos), ou MessagePack, CBOR, BSON, YAML e TOML
    /// convertidos para JSONL (ver `formatos`), com as opções padrão.
    pub fn abrir(caminho: impl AsRef<Path>) -> io::Result<Self> {
        Self::abrir_com(caminho, &Opcoes::default())
    }

    /// Como `abrir`, com cache, limites, progresso e cancelamento escolhidos por quem chama.
    /// Compactado é descompactado para o cache e o cache é que é mapeado.
    pub fn abrir_com(caminho: impl AsRef<Path>, op: &Opcoes) -> io::Result<Self> {
        Self::abrir_(caminho.as_ref(), op, false)
    }

    /// Como `abrir_com`, mas nunca mapeia o arquivo do usuário: JSONL cru é copiado com `read`
    /// para uma cópia só do app (a de `vivo`) e é a cópia que se mapeia; MessagePack, CBOR, BSON,
    /// YAML e TOML são convertidos a partir dessa cópia. Arquivo que outro
    /// processo trunca no meio volta curto, sem SIGBUS. Para varrer pasta, onde muitos arquivos
    /// passam pela leitura e qualquer um pode mudar (achado 14). Custa uma cópia em disco.
    pub fn abrir_copia_com(caminho: impl AsRef<Path>, op: &Opcoes) -> io::Result<Self> {
        Self::abrir_(caminho.as_ref(), op, true)
    }

    fn abrir_(caminho: &Path, op: &Opcoes, copiar: bool) -> io::Result<Self> {
        arquivo_comum(caminho)?;
        let mut arquivo = File::open(caminho)?;
        let mut magico = [0u8; 4];
        let n = arquivo.by_ref().take(4).read(&mut magico)?;
        let arquivo = match compactado::detectar(&magico[..n]) {
            Formato::Cru => match formatos::detectar(caminho, &mut arquivo)? {
                None if copiar => copia(arquivo, op)?,
                None => arquivo,
                Some(f) if copiar => convertido_da_copia(arquivo, f, op)?,
                Some(f) => File::open(formatos::para_cache(caminho, f, op)?)?,
            },
            f => File::open(compactado::para_cache(caminho, f, op)?)?,
        };
        let mapa = if arquivo.metadata()?.len() == 0 {
            None
        } else {
            // SAFETY: mapeamento só leitura; ver a nota sobre truncamento no campo `mapa`.
            Some(unsafe { Mmap::map(&arquivo)? })
        };
        let indice = IndiceLinhas::construir(mapa.as_deref().unwrap_or_default());
        let mascara = match &mapa {
            Some(m) if !op.sem_mascara => Some(mascarar_mapa(&arquivo, m, &indice)?),
            _ => None,
        };
        Ok(Self {
            mapa,
            estatico: &[],
            indice,
            mascara,
        })
    }

    /// Bytes que já estão na memória do programa (os exemplos embutidos no app): sem disco nem
    /// `mmap`, mesmo índice e mesmas janelas de um arquivo aberto.
    pub fn estatico(dados: &'static [u8]) -> Self {
        let indice = IndiceLinhas::construir(dados);
        let mascara = Mascara::nova(dados.to_vec(), dados, &indice);
        Self {
            mapa: None,
            estatico: dados,
            indice,
            mascara: Some(mascara),
        }
    }

    /// Mapa e índice já prontos (arquivo vivo: o índice cresce fora, ver `vivo`); `arquivo` é o
    /// que foi mapeado, para a cópia mascarada.
    // shortcut: remascara o arquivo inteiro a cada crescimento; mascarar só o pedaço novo se
    // seguir arquivo grande ficar lento.
    pub(crate) fn de_mapa(
        arquivo: &File,
        mapa: Option<Mmap>,
        indice: IndiceLinhas,
    ) -> io::Result<Self> {
        let mascara = match &mapa {
            Some(m) => Some(mascarar_mapa(arquivo, m, &indice)?),
            None => None,
        };
        Ok(Self {
            mapa,
            estatico: &[],
            indice,
            mascara,
        })
    }

    /// O arquivo com os segredos mascarados: é o que sai para tela, busca, consulta e exportação.
    /// Na 1ª chamada mascara o que a `janela` ainda não mascarou (o arquivo inteiro).
    pub fn bytes(&self) -> &[u8] {
        // Sem segredo nenhum, o arquivo mesmo: ler a cópia mapearia as mesmas páginas de novo.
        match &self.mascara {
            // SAFETY: todos os blocos já foram escritos (`tudo`); ninguém mais escreve neles.
            Some(m) if m.tudo(self.cru(), &self.indice) => unsafe {
                std::slice::from_raw_parts(m.ptr, m.len)
            },
            _ => self.cru(),
        }
    }

    /// O arquivo como está no disco, sem máscara. Só para o que não devolve valor (validar) ou
    /// o que o usuário pediu campo a campo (`Arvore::valor_cru`, anonimizar).
    pub fn cru(&self) -> &[u8] {
        self.mapa.as_deref().unwrap_or(self.estatico)
    }

    /// O mesmo trecho de `bytes()` (um pedaço dele) no arquivo cru. Para diagnóstico de linha
    /// inválida (byte e motivo, nunca o valor): a máscara troca o pedaço estranho e mudaria o erro.
    pub fn cru_de<'a>(&'a self, trecho: &'a [u8]) -> &'a [u8] {
        let p = trecho.as_ptr() as usize;
        let base = match &self.mascara {
            Some(m) if (m.ptr as usize..m.ptr as usize + m.len).contains(&p) => m.ptr as usize,
            _ => self.cru().as_ptr() as usize,
        };
        let inicio = p.wrapping_sub(base);
        self.cru()
            .get(inicio..inicio.saturating_add(trecho.len()))
            .unwrap_or(trecho)
    }

    /// Índice de linhas (para `mascara::anonimizar`).
    pub fn indice(&self) -> &IndiceLinhas {
        &self.indice
    }

    pub fn linhas(&self) -> usize {
        self.indice.len()
    }

    /// Linha que contém o byte `b` do arquivo e o byte onde ela começa.
    pub fn linha_do_byte(&self, b: usize) -> Option<(usize, usize)> {
        let l = self.indice.linha_do_byte(b)?;
        Some((l, self.indice.faixa(l, self.cru())?.start))
    }

    /// Linha `i` como está no disco, sem máscara (ver `cru`).
    pub fn linha_crua(&self, i: usize) -> &[u8] {
        let d = self.cru();
        self.indice.faixa(i, d).map_or(&[][..], |r| &d[r])
    }

    /// Até `quantidade` linhas cruas a partir de `inicio`; vazio se `inicio` passar do fim.
    pub fn janela(&self, inicio: usize, quantidade: usize) -> Vec<&[u8]> {
        let cru = self.cru();
        let fim = inicio.saturating_add(quantidade).min(self.indice.len());
        let Some(m) = self.mascara.as_ref().filter(|_| inicio < fim) else {
            return (inicio..fim)
                .filter_map(|i| self.indice.faixa(i, cru).map(|r| &cru[r]))
                .collect();
        };
        let copia: Vec<bool> = (m.bloco_de(inicio)..=m.bloco_de(fim - 1))
            .map(|b| m.da_copia(b, cru, &self.indice))
            .collect();
        (inicio..fim)
            .filter_map(|i| Some((i, self.indice.faixa(i, cru)?)))
            .map(|(i, r)| match copia[m.bloco_de(i) - m.bloco_de(inicio)] {
                // SAFETY: a linha está num bloco já escrito (`da_copia` acima).
                true => unsafe { std::slice::from_raw_parts(m.ptr.add(r.start), r.len()) },
                false => &cru[r],
            })
            .collect()
    }
}

/// Copia `arquivo` inteiro (até `op.max_saida`) para uma cópia anônima do app, só com `read`.
fn copia(mut arquivo: File, op: &Opcoes) -> io::Result<File> {
    let mut c = crate::vivo::criar_copia(&compactado::pasta_cache(op)?)?;
    arquivo.seek(SeekFrom::Start(0))?;
    let n = io::copy(&mut arquivo.take(op.max_saida + 1), &mut c)?;
    if n > op.max_saida {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("arquivo maior que {} bytes", op.max_saida),
        ));
    }
    Ok(c)
}

/// Converte a partir de uma cópia (o `para_cache` de `formatos` mapeia o original) para outra
/// cópia anônima. Não passa pelo cache de conversões: cada percurso converte de novo.
fn convertido_da_copia(arquivo: File, f: formatos::Entrada, op: &Opcoes) -> io::Result<File> {
    let c = copia(arquivo, op)?;
    let mut saida = crate::vivo::criar_copia(&compactado::pasta_cache(op)?)?;
    let mapa = if c.metadata()?.len() == 0 {
        None
    } else {
        // SAFETY: `c` é cópia só do app (apagada do diretório ao criar); ninguém a encolhe.
        Some(unsafe { Mmap::map(&c)? })
    };
    formatos::converter(
        mapa.as_deref().unwrap_or_default(),
        f,
        &mut io::BufWriter::new(&mut saida),
        op,
    )?;
    Ok(saida)
}

/// Cópia privada (copy-on-write) de `arquivo`, mascarada sob demanda (`Mascara`).
fn mascarar_mapa(arquivo: &File, mapa: &Mmap, indice: &IndiceLinhas) -> io::Result<Mascara> {
    // SAFETY: mapeamento privado do mesmo arquivo e do mesmo tamanho de `mapa`; escrever nele
    // não chega ao disco (MAP_PRIVATE). Sem escrita nenhuma página é copiada.
    let copia = unsafe {
        memmap2::MmapOptions::new()
            .len(mapa.len())
            .map_copy(arquivo)?
    };
    Ok(Mascara::nova(copia, mapa, indice))
}

/// Recusa o que não é arquivo comum, sem abrir: `open` de um FIFO bloqueia até alguém escrever,
/// e dispositivo (`/dev/zero`) tem tamanho 0 e não acaba. `metadata` segue link simbólico.
pub fn arquivo_comum(caminho: &Path) -> io::Result<()> {
    if std::fs::metadata(caminho)?.is_file() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "não é um arquivo comum (pasta, FIFO, socket ou dispositivo)",
    ))
}

#[cfg(test)]
mod testes {
    use super::*;

    /// Achado 9: FIFO e dispositivo são recusados na hora, sem abrir.
    #[cfg(unix)]
    #[test]
    fn recusa_o_que_nao_e_arquivo_comum() {
        let pasta = tempfile::tempdir().unwrap();
        let fifo = pasta.path().join("fifo");
        let ok = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(ok.success());
        // Sem a recusa, o `open` destrava quando este escritor abre (e o teste falha no Ok).
        let f = fifo.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let _ = std::fs::write(f, "1\n");
        });
        let t = std::time::Instant::now();
        let erro = ArquivoLinhas::abrir(&fifo)
            .err()
            .expect("FIFO devia ser recusado");
        assert!(
            erro.to_string().contains("não é um arquivo comum"),
            "{erro}"
        );
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
        assert!(ArquivoLinhas::abrir("/dev/zero").is_err());
        assert!(ArquivoLinhas::abrir(pasta.path()).is_err());
    }

    fn linhas(dados: &[u8]) -> Vec<&[u8]> {
        let ind = IndiceLinhas::construir(dados);
        (0..ind.len())
            .map(|i| &dados[ind.faixa(i, dados).unwrap()])
            .collect()
    }

    #[test]
    fn casos_de_borda() {
        let casos: &[(&[u8], &[&[u8]])] = &[
            (b"", &[]),
            (b"\xEF\xBB\xBF", &[]),
            (b"\n", &[b""]),
            (b"a", &[b"a"]),
            (b"a\n", &[b"a"]),
            (b"a\nb", &[b"a", b"b"]),
            (b"a\nb\n", &[b"a", b"b"]),
            (b"a\n\nb", &[b"a", b"", b"b"]),
            (b"a\r\nb\r\n", &[b"a", b"b"]),
            (b"a\r\nb", &[b"a", b"b"]),
            (b"\r\n", &[b""]),
            (b"\r", &[b"\r"]),
            (b"a\r", &[b"a\r"]),
            (b"a\rb\n", &[b"a\rb"]),
            (b"\xEF\xBB\xBF{}\n{}", &[b"{}", b"{}"]),
            (b"\xEF\xBB\xBF\n", &[b""]),
            // BOM só conta no início do arquivo.
            (b"a\n\xEF\xBB\xBFb", &[b"a", b"\xEF\xBB\xBFb"]),
        ];
        for (entrada, esperado) in casos {
            assert_eq!(&linhas(entrada), esperado, "entrada {entrada:?}");
        }
    }

    #[test]
    fn mascara_por_blocos_sob_demanda() {
        // Segredo em vários blocos; a janela mascara só o bloco que mostra, `bytes()` o resto.
        let n = 3 * BLOCO + 10;
        let mut d = String::new();
        for i in 0..n {
            d += &format!("{{\"i\":{i},\"senha\":\"S3NT1N3LA{i}\"}}\n");
        }
        let pasta = tempfile::tempdir().unwrap();
        let p = pasta.path().join("a.jsonl");
        std::fs::write(&p, &d).unwrap();
        let a = ArquivoLinhas::abrir(&p).unwrap();
        let alvo = 2 * BLOCO + 5;
        let l = a.janela(alvo, 1)[0];
        assert_eq!(
            l,
            format!("{{\"i\":{alvo},\"senha\":\"{}\"}}", "*".repeat(9 + 5)).as_bytes()
        );
        assert_eq!(a.cru_de(l), a.linha_crua(alvo));
        let m = a.mascara.as_ref().unwrap();
        assert!(m.documento.is_none());
        assert_eq!(m.blocos.len(), 4);
        assert_eq!(m.blocos.iter().filter(|b| b.get().is_some()).count(), 1);
        assert!(
            !a.janela(n - 2, 2)
                .concat()
                .windows(9)
                .any(|w| w == b"S3NT1N3LA")
        );
        assert!(!a.bytes().windows(9).any(|w| w == b"S3NT1N3LA"));
        assert!(m.blocos.iter().all(|b| b.get() == Some(&true)));
        assert_eq!(a.cru(), d.as_bytes());
    }

    #[test]
    fn documento_mascara_so_os_blocos_ate_a_janela() {
        // Documento formatado: a 1ª tela lê só o 1º bloco; o segredo aberto lá em cima
        // continua valendo nos blocos seguintes.
        let n = 3 * BLOCO;
        let mut d = String::from("{\"senha\": [\n");
        for i in 0..n {
            d += &format!("  \"S3NT1N3LA{i}\",\n");
        }
        d += "  0\n]}\n";
        let a = ArquivoLinhas::estatico(Box::leak(d.into_bytes().into_boxed_slice()));
        let m = a.mascara.as_ref().unwrap();
        assert!(m.documento.is_some());
        assert!(
            !a.janela(0, 50)
                .concat()
                .windows(9)
                .any(|w| w == b"S3NT1N3LA")
        );
        assert_eq!(m.blocos.iter().filter(|b| b.get().is_some()).count(), 1);
        let l = a.janela(2 * BLOCO + 7, 1)[0];
        assert!(
            !l.windows(9).any(|w| w == b"S3NT1N3LA"),
            "{}",
            String::from_utf8_lossy(l)
        );
        assert_eq!(m.blocos.iter().filter(|b| b.get().is_some()).count(), 3);
        assert!(!a.bytes().windows(9).any(|w| w == b"S3NT1N3LA"));
    }

    #[test]
    fn faixa_fora_do_indice() {
        let ind = IndiceLinhas::construir(b"a\n");
        assert_eq!(ind.faixa(1, b"a\n"), None);
    }
}
