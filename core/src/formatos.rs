//! MessagePack, CBOR, BSON, YAML e TOML como entrada, somente leitura: cada formato é convertido
//! para JSONL (um documento por linha) no mesmo cache dos compactados e aberto pelo caminho normal.
//!
//! Detecção: pela extensão (`.msgpack`/`.mpk`, `.cbor`, `.bson`, `.yaml`/`.yml`, `.toml`) e,
//! sem extensão conhecida, pela assinatura: CBOR com a marca de autodescrição (`D9 D9 F7`), BSON
//! com o tamanho do 1º documento coerente e terminado em zero, YAML começando em `%YAML` ou `---`.
//! MessagePack e TOML não têm assinatura: só pela extensão. `.json`, `.jsonl` e `.ndjson` nunca
//! passam pela conversão.
//!
//! Representação do que não tem par exato em JSON (decisão em `docs/decisoes.md`):
//! - inteiro fora de ±2^53 sai com todos os dígitos (JSON aceita; o app mostra o número cru);
//! - NaN e ±infinito viram os textos `"NaN"`, `"Infinity"` e `"-Infinity"`;
//! - binário (MessagePack `bin`, CBOR byte string) vira texto base64 padrão, com `=`;
//! - chave que não é texto vira o JSON compacto dela como texto (`1` → `"1"`, `[1,2]` → `"[1,2]"`);
//!   chave repetida fica repetida;
//! - MessagePack ext vira `{"$ext": tipo, "$dados": base64}`; o timestamp (ext −1) vira texto
//!   ISO 8601 em UTC;
//! - CBOR: marcas (tags) são transparentes, exceto bignum (2 e 3, até 1 KiB) que vira inteiro;
//!   `undefined` vira `null`; valor simples não atribuído vira o texto `"simple(N)"`;
//! - BSON segue o Extended JSON relaxado do MongoDB (o que o `mongoexport` grava):
//!   `{"$oid": …}`, `{"$date": …}`, `{"$binary": …}`, `{"$numberDecimal": …}` etc.;
//! - YAML: esquema core do YAML 1.2 (`yes`/`no` são texto); datas ficam texto; âncoras e aliases
//!   se expandem dentro de um teto; tags fora do esquema (`!python/object`, `!Ref`…) são recusadas;
//!   `<<` (merge do YAML 1.1) fica como chave comum;
//! - TOML: datas e horas viram texto RFC 3339.
//!
//! Arquivo grande: MessagePack, CBOR, BSON e YAML são lidos por `mmap` e convertidos em fluxo
//! (memória proporcional à profundidade, não ao tamanho); TOML é carregado inteiro e tem teto
//! próprio (`Limites::max_toml`). A saída tem os mesmos tetos dos compactados
//! (`Opcoes::max_saida` e `max_razao` × a entrada).
//!
//! Entrada é não confiável: comprimento declarado nunca vira alocação (é conferido contra o que
//! resta do arquivo), e profundidade, nós, chaves, âncoras e aliases têm teto (`Limites`).
//! Malformado ou acima de um teto vira `InvalidData` com mensagem, nunca pânico.

use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::fs::File;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use memmap2::Mmap;

use crate::compactado::{self, Opcoes};

const MIB: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrada {
    MsgPack,
    Cbor,
    Bson,
    Yaml,
    Toml,
}

impl Entrada {
    fn nome(self) -> &'static str {
        match self {
            Entrada::MsgPack => "MessagePack",
            Entrada::Cbor => "CBOR",
            Entrada::Bson => "BSON",
            Entrada::Yaml => "YAML",
            Entrada::Toml => "TOML",
        }
    }
}

/// Tetos da conversão. Cada um tem teste próprio que falha se ele for desligado.
#[derive(Debug, Clone)]
pub struct Limites {
    /// Aninhamento máximo; o padrão é o mesmo que a árvore abre (`arvore::MAX_PROFUNDIDADE`).
    pub profundidade: usize,
    /// Nós (valores, contando contêineres e chaves) no arquivo todo, aliases expandidos inclusive.
    pub nos: u64,
    /// Nós que os aliases do YAML podem acrescentar (contra a bomba de aliases, "billion laughs").
    pub nos_alias: u64,
    /// Bytes de JSON que os aliases do YAML podem acrescentar (a mesma bomba com poucos nós e
    /// âncoras gordas: um texto de 16 MiB repetido 600 vezes).
    pub bytes_alias: u64,
    /// Bytes de JSON guardados por âncora do YAML para repetir no alias; acima disso a âncora
    /// não é guardada e um alias para ela é recusado.
    pub max_ancora: usize,
    /// Bytes de uma chave que não é texto, depois de virar JSON.
    pub max_chave: usize,
    /// Cada texto/binário CBOR em pedaços (indefinido), que precisa ser juntado na memória.
    pub max_texto: u64,
    /// Arquivo TOML: é carregado inteiro como árvore, que ocupa até ~180× o arquivo na memória
    /// (medido: tabelas pequenas); 1 MiB dá no máximo ~200 MB.
    pub max_toml: u64,
}

impl Default for Limites {
    fn default() -> Self {
        Self {
            profundidade: crate::arvore::MAX_PROFUNDIDADE,
            nos: 1 << 31,
            nos_alias: 1_000_000,
            bytes_alias: 256 * MIB,
            max_ancora: 16 << 20,
            max_chave: 64 << 10,
            max_texto: 64 * MIB,
            max_toml: MIB,
        }
    }
}

/// Formato de entrada de `caminho`, ou `None` para JSON/JSONL (o caminho de sempre).
/// Só lê o começo do arquivo (e um byte do BSON) quando a extensão não decide.
pub fn detectar(caminho: &Path, arquivo: &mut File) -> io::Result<Option<Entrada>> {
    let ext = caminho
        .extension()
        .and_then(|x| x.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("msgpack" | "mpk") => return Ok(Some(Entrada::MsgPack)),
        Some("cbor") => return Ok(Some(Entrada::Cbor)),
        Some("bson") => return Ok(Some(Entrada::Bson)),
        Some("yaml" | "yml") => return Ok(Some(Entrada::Yaml)),
        Some("toml") => return Ok(Some(Entrada::Toml)),
        Some("json" | "jsonl" | "ndjson") => return Ok(None),
        _ => {}
    }
    let total = arquivo.metadata()?.len();
    let mut inicio = [0u8; 8];
    arquivo.seek(SeekFrom::Start(0))?;
    let n = arquivo.read(&mut inicio)?;
    let inicio = &inicio[..n];
    let sem_bom = inicio.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(inicio);
    let achado = if inicio.starts_with(b"\xD9\xD9\xF7") {
        Some(Entrada::Cbor)
    } else if sem_bom.starts_with(b"%YAML") || sem_bom.starts_with(b"---") {
        Some(Entrada::Yaml)
    } else if parece_bson(arquivo, inicio, total)? {
        Some(Entrada::Bson)
    } else {
        None
    };
    arquivo.seek(SeekFrom::Start(0))?;
    Ok(achado)
}

/// 1º documento BSON: tamanho entre 5 e o arquivo, tipo do 1º elemento válido e último byte zero.
/// Texto JSON não tem byte zero, então não passa.
fn parece_bson(arquivo: &mut File, inicio: &[u8], total: u64) -> io::Result<bool> {
    let Some(cab) = inicio.get(..5) else {
        return Ok(false);
    };
    let tam = i32::from_le_bytes([cab[0], cab[1], cab[2], cab[3]]);
    let tipo_ok = matches!(cab[4], 0x01..=0x13 | 0x7F | 0xFF) || (cab[4] == 0 && tam == 5);
    if tam < 5 || tam as u64 > total || !tipo_ok {
        return Ok(false);
    }
    let mut fim = [1u8];
    arquivo.seek(SeekFrom::Start(tam as u64 - 1))?;
    arquivo.read_exact(&mut fim)?;
    Ok(fim[0] == 0)
}

/// Converte `caminho` para JSONL no cache (ou reaproveita o que já está lá).
pub(crate) fn para_cache(caminho: &Path, entrada: Entrada, op: &Opcoes) -> io::Result<PathBuf> {
    // Versão da conversão na chave: mudar a representação invalida o cache antigo.
    let etiqueta = format!("formatos/1/{entrada:?}");
    compactado::em_cache(
        caminho,
        op,
        etiqueta.as_bytes(),
        |_, total| Ok((total.saturating_mul(2).min(op.max_saida), op.max_saida)),
        |arquivo, _, saida| {
            let mapa = if arquivo.metadata()?.len() == 0 {
                None
            } else {
                // SAFETY: só leitura; truncar o original durante a conversão é o mesmo caso do
                // `mmap` de `ArquivoLinhas`.
                Some(unsafe { Mmap::map(&*arquivo)? })
            };
            converter(mapa.as_deref().unwrap_or_default(), entrada, saida, op)
        },
    )
}

/// Converte `dados` para JSONL em `saida`, com os tetos de `op` (`formatos`, `max_saida`),
/// progresso e cancelamento. Devolve os bytes escritos.
pub fn converter(
    dados: &[u8],
    entrada: Entrada,
    saida: &mut dyn Write,
    op: &Opcoes,
) -> io::Result<u64> {
    let mut e = Emissor::new(saida, op, entrada.nome(), dados.len() as u64);
    match entrada {
        Entrada::MsgPack => msgpack(dados, &mut e)?,
        Entrada::Cbor => cbor(dados, &mut e)?,
        Entrada::Bson => bson(dados, &mut e)?,
        Entrada::Yaml => yaml(dados, &mut e)?,
        Entrada::Toml => toml(dados, &mut e)?,
    }
    e.out.flush()?;
    if let Some(p) = op.progresso {
        p(e.total, e.total);
    }
    Ok(e.escritos)
}

// ---------------------------------------------------------------------------------------------
// Emissor: escreve JSONL a partir de eventos (abrir, folha, fechar), com todos os tetos.

#[derive(Clone, Copy)]
enum Quadro {
    Lista { n: u64 },
    Mapa { n: u64, chave: bool },
}

/// Âncora do YAML sendo gravada: os bytes do nó, para repetir no alias.
struct Gravacao {
    id: usize,
    /// `chaves.len()` quando começou: só grava o que é escrito nesse nível.
    nivel: usize,
    /// `pilha.len()` quando começou: o nó termina quando a pilha volta a esse tamanho.
    base: usize,
    buf: Vec<u8>,
    nos0: u64,
    prof: usize,
    grande: bool,
}

struct Ancora {
    texto: Vec<u8>,
    nos: u64,
    prof: usize,
}

struct Emissor<'a, 'o> {
    out: &'a mut dyn Write,
    op: &'a Opcoes<'o>,
    lim: &'a Limites,
    nome: &'static str,
    escritos: u64,
    /// Saída máxima: `max_saida`, e no máximo `max_razao` × a entrada (como nos compactados).
    limite: u64,
    nos: u64,
    nos_alias: u64,
    bytes_alias: u64,
    pilha: Vec<Quadro>,
    /// Chaves em construção (a chave vira texto só quando termina).
    chaves: Vec<Vec<u8>>,
    gravando: Vec<Gravacao>,
    /// `None`: âncora grande demais para guardar.
    ancoras: HashMap<usize, Option<Ancora>>,
    /// Posição na entrada (byte; no YAML, caractere), para mensagens e progresso.
    pos: u64,
    total: u64,
    avisado: u64,
    prox_checagem: u64,
}

impl<'a, 'o> Emissor<'a, 'o> {
    fn new(out: &'a mut dyn Write, op: &'a Opcoes<'o>, nome: &'static str, total: u64) -> Self {
        Self {
            out,
            op,
            lim: &op.formatos,
            nome,
            escritos: 0,
            limite: compactado::limite(op, total),
            nos: 0,
            nos_alias: 0,
            bytes_alias: 0,
            pilha: Vec::new(),
            chaves: Vec::new(),
            gravando: Vec::new(),
            ancoras: HashMap::new(),
            pos: 0,
            total,
            avisado: 0,
            prox_checagem: 0,
        }
    }

    fn malformado(&self, msg: impl Display) -> io::Error {
        io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "{} malformado perto da posição {}: {msg}",
                self.nome, self.pos
            ),
        )
    }

    fn teto(&self, msg: impl Display) -> io::Error {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("{}: {msg}; o Jasão não abre este arquivo", self.nome),
        )
    }

    fn escrever(&mut self, b: &[u8]) -> io::Result<()> {
        let nivel = self.chaves.len();
        for g in &mut self.gravando {
            if g.nivel == nivel && !g.grande {
                if g.buf.len() + b.len() > self.lim.max_ancora {
                    g.grande = true;
                    g.buf = Vec::new();
                } else {
                    g.buf.extend_from_slice(b);
                }
            }
        }
        if let Some(k) = self.chaves.last_mut() {
            if k.len() + b.len() > self.lim.max_chave {
                let max = self.lim.max_chave;
                return Err(self.teto(format!("chave de mapa com mais de {max} bytes")));
            }
            k.extend_from_slice(b);
            return Ok(());
        }
        self.escritos += b.len() as u64;
        if self.escritos > self.limite {
            let mib = self.limite / MIB;
            return Err(self.teto(if self.limite < self.op.max_saida {
                format!(
                    "convertido para JSON passa de {mib} MiB, mais de {}× o arquivo",
                    self.op.max_razao
                )
            } else {
                format!("convertido para JSON passa de {mib} MiB, o teto do app")
            }));
        }
        self.out.write_all(b)
    }

    fn contar(&mut self, n: u64) -> io::Result<()> {
        self.nos += n;
        if self.nos > self.lim.nos {
            let max = self.lim.nos;
            return Err(self.teto(format!("mais de {max} valores")));
        }
        if self.nos >= self.prox_checagem {
            self.prox_checagem = self.nos + (1 << 14);
            if self.op.cancelar.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(compactado::cancelado());
            }
            if let Some(p) = self.op.progresso
                && self.pos - self.avisado >= MIB
            {
                self.avisado = self.pos;
                p(self.pos, self.total);
            }
        }
        Ok(())
    }

    /// Separador antes de um nó; em posição de chave, começa a juntar a chave.
    fn no_inicio(&mut self, ancora: usize) -> io::Result<()> {
        match self.pilha.last() {
            Some(Quadro::Lista { n }) if *n > 0 => self.escrever(b",")?,
            Some(Quadro::Mapa { n, chave: true }) => {
                if *n > 0 {
                    self.escrever(b",")?;
                }
                self.chaves.push(Vec::new());
            }
            _ => {}
        }
        if ancora > 0 {
            self.gravando.push(Gravacao {
                id: ancora,
                nivel: self.chaves.len(),
                base: self.pilha.len(),
                buf: Vec::new(),
                nos0: self.nos,
                prof: 0,
                grande: false,
            });
        }
        Ok(())
    }

    /// Fim de um nó: fecha a âncora dele, se houver, e avança o contêiner de cima.
    fn no_fim(&mut self) -> io::Result<()> {
        if let Some(g) = self.gravando.last()
            && g.base == self.pilha.len()
            && g.nivel == self.chaves.len()
        {
            let g = self.gravando.pop().expect("acabou de olhar");
            let ancora = (!g.grande).then(|| Ancora {
                texto: g.buf,
                nos: self.nos - g.nos0,
                prof: g.prof,
            });
            self.ancoras.insert(g.id, ancora);
        }
        match self.pilha.last_mut() {
            None => self.escrever(b"\n"),
            Some(Quadro::Lista { n }) => {
                *n += 1;
                Ok(())
            }
            Some(Quadro::Mapa { n, chave }) => {
                if *chave {
                    *chave = false;
                    let k = self.chaves.pop().expect("chave começada em no_inicio");
                    if k.first() == Some(&b'"') {
                        self.escrever(&k)?;
                    } else {
                        // Chave que não é texto: o JSON dela (sempre UTF-8 válido) como texto.
                        self.texto(&k)?;
                    }
                    self.escrever(b":")
                } else {
                    *chave = true;
                    *n += 1;
                    Ok(())
                }
            }
        }
    }

    fn crescer(&mut self, extra: usize) -> io::Result<()> {
        let prof = self.pilha.len() + extra;
        if prof > self.lim.profundidade {
            let max = self.lim.profundidade;
            return Err(self.teto(format!("aninhamento passa de {max} níveis")));
        }
        for g in &mut self.gravando {
            g.prof = g.prof.max(prof - g.base);
        }
        Ok(())
    }

    fn abrir(&mut self, mapa: bool, ancora: usize) -> io::Result<()> {
        self.no_inicio(ancora)?;
        self.contar(1)?;
        self.crescer(1)?;
        self.escrever(if mapa { b"{" } else { b"[" })?;
        self.pilha.push(if mapa {
            Quadro::Mapa { n: 0, chave: true }
        } else {
            Quadro::Lista { n: 0 }
        });
        Ok(())
    }

    fn fechar(&mut self) -> io::Result<()> {
        let fim: &[u8] = match self.pilha.pop() {
            Some(Quadro::Lista { .. }) => b"]",
            Some(Quadro::Mapa { chave: true, .. }) => b"}",
            Some(Quadro::Mapa { chave: false, .. }) => {
                return Err(self.malformado("mapa termina numa chave sem valor"));
            }
            None => return Err(self.malformado("fim de contêiner que não foi aberto")),
        };
        self.escrever(fim)?;
        self.no_fim()
    }

    /// Valor folha já pronto em JSON (`prof` = aninhamento dentro dele).
    fn folha(&mut self, ancora: usize, json: &[u8], prof: usize) -> io::Result<()> {
        self.no_inicio(ancora)?;
        self.contar(1)?;
        self.crescer(prof)?;
        self.escrever(json)?;
        self.no_fim()
    }

    fn nulo(&mut self) -> io::Result<()> {
        self.folha(0, b"null", 0)
    }

    fn booleano(&mut self, b: bool) -> io::Result<()> {
        self.folha(0, if b { b"true" } else { b"false" }, 0)
    }

    fn inteiro(&mut self, v: impl Display) -> io::Result<()> {
        self.folha(0, v.to_string().as_bytes(), 0)
    }

    fn real<F: Into<f64> + Debug + Copy>(&mut self, x: F) -> io::Result<()> {
        let v: f64 = x.into();
        if v.is_nan() {
            self.folha(0, b"\"NaN\"", 0)
        } else if v.is_infinite() {
            self.folha(
                0,
                if v > 0.0 {
                    b"\"Infinity\""
                } else {
                    b"\"-Infinity\""
                },
                0,
            )
        } else {
            // `Debug` dá a forma mais curta que volta ao mesmo valor (`0.1`, `1e300`): JSON válido.
            self.folha(0, format!("{x:?}").as_bytes(), 0)
        }
    }

    fn folha_texto(&mut self, ancora: usize, s: &[u8]) -> io::Result<()> {
        self.no_inicio(ancora)?;
        self.contar(1)?;
        self.texto(s)?;
        self.no_fim()
    }

    fn folha_binario(&mut self, b: &[u8]) -> io::Result<()> {
        self.no_inicio(0)?;
        self.contar(1)?;
        self.escrever(b"\"")?;
        self.base64(b)?;
        self.escrever(b"\"")?;
        self.no_fim()
    }

    /// Texto JSON com escape; bytes que não são UTF-8 viram U+FFFD.
    fn texto(&mut self, s: &[u8]) -> io::Result<()> {
        self.escrever(b"\"")?;
        for pedaco in s.utf8_chunks() {
            let v = pedaco.valid().as_bytes();
            let mut de = 0;
            for (i, &c) in v.iter().enumerate() {
                let esc: &[u8] = match c {
                    b'"' => b"\\\"",
                    b'\\' => b"\\\\",
                    b'\n' => b"\\n",
                    b'\r' => b"\\r",
                    b'\t' => b"\\t",
                    0..0x20 => b"",
                    _ => continue,
                };
                self.escrever(&v[de..i])?;
                if esc.is_empty() {
                    self.escrever(format!("\\u{c:04x}").as_bytes())?;
                } else {
                    self.escrever(esc)?;
                }
                de = i + 1;
            }
            self.escrever(&v[de..])?;
            if !pedaco.invalid().is_empty() {
                self.escrever("\u{FFFD}".as_bytes())?;
            }
        }
        self.escrever(b"\"")
    }

    fn base64(&mut self, b: &[u8]) -> io::Result<()> {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut buf = Vec::with_capacity(4096);
        for grupo in b.chunks(3) {
            let n =
                grupo.iter().fold(0u32, |n, &c| n << 8 | u32::from(c)) << (8 * (3 - grupo.len()));
            for i in 0..4 {
                buf.push(if i <= grupo.len() {
                    A[(n >> (18 - 6 * i) & 63) as usize]
                } else {
                    b'='
                });
            }
            if buf.len() >= 4092 {
                self.escrever(&buf)?;
                buf.clear();
            }
        }
        self.escrever(&buf)
    }

    /// Alias do YAML: repete os bytes guardados da âncora, contando nós e profundidade.
    fn alias(&mut self, id: usize) -> io::Result<()> {
        let a = match self.ancoras.get_mut(&id) {
            None => return Err(self.malformado("alias para âncora que ainda não existe")),
            Some(None) => {
                let max = self.lim.max_ancora;
                return Err(self.teto(format!("alias para uma âncora com mais de {max} bytes")));
            }
            Some(Some(a)) => std::mem::replace(
                a,
                Ancora {
                    texto: Vec::new(),
                    nos: 0,
                    prof: 0,
                },
            ),
        };
        let r = self.repetir(&a);
        self.ancoras.insert(id, Some(a));
        r
    }

    fn repetir(&mut self, a: &Ancora) -> io::Result<()> {
        self.nos_alias += a.nos;
        if self.nos_alias > self.lim.nos_alias {
            let max = self.lim.nos_alias;
            return Err(self.teto(format!(
                "aliases expandem mais de {max} valores (bomba de aliases?)"
            )));
        }
        self.bytes_alias += a.texto.len() as u64;
        if self.bytes_alias > self.lim.bytes_alias {
            let max = self.lim.bytes_alias / MIB;
            return Err(self.teto(format!(
                "aliases expandem mais de {max} MiB (bomba de aliases?)"
            )));
        }
        self.no_inicio(0)?;
        self.contar(a.nos)?;
        self.crescer(a.prof)?;
        self.escrever(&a.texto)?;
        self.no_fim()
    }
}

// ---------------------------------------------------------------------------------------------
// Leitura de bytes: todo comprimento é conferido contra o que resta antes de ser usado.

struct Leitor<'d> {
    d: &'d [u8],
    pos: usize,
}

impl<'d> Leitor<'d> {
    fn resta(&self) -> u64 {
        (self.d.len() - self.pos) as u64
    }

    fn tomar(&mut self, n: u64) -> Option<&'d [u8]> {
        if n > self.resta() {
            return None;
        }
        let s = &self.d[self.pos..self.pos + n as usize];
        self.pos += n as usize;
        Some(s)
    }

    fn be<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.tomar(N as u64).map(|s| s.try_into().expect("N bytes"))
    }

    fn u8(&mut self) -> Option<u8> {
        self.be::<1>().map(|[b]| b)
    }
}

fn truncado(e: &Emissor) -> io::Error {
    e.malformado("arquivo termina no meio de um valor (truncado?)")
}

/// Comprimento declarado que passa do fim do arquivo: erro antes de qualquer leitura ou alocação.
fn conferir(e: &Emissor, r: &Leitor, itens: u64, minimo: u64) -> io::Result<()> {
    if itens.saturating_mul(minimo) > r.resta() {
        return Err(e.malformado(format!(
            "declara {itens} itens/bytes, mas só restam {} bytes no arquivo",
            r.resta()
        )));
    }
    Ok(())
}

/// Depois de um valor completo: desconta do contêiner de cima e fecha os que acabaram.
fn concluir(e: &mut Emissor, pilha: &mut Vec<u64>) -> io::Result<()> {
    while let Some(falta) = pilha.last_mut() {
        *falta -= 1;
        if *falta > 0 {
            break;
        }
        pilha.pop();
        e.fechar()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// MessagePack (https://github.com/msgpack/msgpack/blob/master/spec.md): valores concatenados.

fn msgpack(d: &[u8], e: &mut Emissor) -> io::Result<()> {
    let mut r = Leitor { d, pos: 0 };
    let mut pilha: Vec<u64> = Vec::new();
    while r.resta() > 0 || !pilha.is_empty() {
        e.pos = r.pos as u64;
        let m = r.u8().ok_or_else(|| truncado(e))?;
        let tam = |r: &mut Leitor, n: usize| -> Option<u64> {
            match n {
                1 => r.be::<1>().map(|b| u64::from(b[0])),
                2 => r.be::<2>().map(|b| u64::from(u16::from_be_bytes(b))),
                _ => r.be::<4>().map(|b| u64::from(u32::from_be_bytes(b))),
            }
        };
        let cont = |e: &mut Emissor, r: &Leitor, pilha: &mut Vec<u64>, mapa, n: u64| {
            let itens = if mapa { n * 2 } else { n };
            conferir(e, r, itens, 1)?;
            e.abrir(mapa, 0)?;
            if itens == 0 {
                e.fechar()?;
                concluir(e, pilha)
            } else {
                pilha.push(itens);
                Ok(())
            }
        };
        match m {
            0x00..=0x7f => e.inteiro(m)?,
            0xe0..=0xff => e.inteiro(m as i8)?,
            0x80..=0x8f => {
                cont(e, &r, &mut pilha, true, u64::from(m & 0x0f))?;
                continue;
            }
            0x90..=0x9f => {
                cont(e, &r, &mut pilha, false, u64::from(m & 0x0f))?;
                continue;
            }
            0xdc..=0xdf => {
                let n = tam(&mut r, if m & 1 == 0 { 2 } else { 4 }).ok_or_else(|| truncado(e))?;
                cont(e, &r, &mut pilha, m >= 0xde, n)?;
                continue;
            }
            0xc0 => e.nulo()?,
            0xc2 | 0xc3 => e.booleano(m == 0xc3)?,
            0xa0..=0xbf | 0xd9..=0xdb | 0xc4..=0xc6 => {
                let n = match m {
                    0xa0..=0xbf => u64::from(m & 0x1f),
                    0xd9 | 0xc4 => tam(&mut r, 1).ok_or_else(|| truncado(e))?,
                    0xda | 0xc5 => tam(&mut r, 2).ok_or_else(|| truncado(e))?,
                    _ => tam(&mut r, 4).ok_or_else(|| truncado(e))?,
                };
                conferir(e, &r, n, 1)?;
                let s = r.tomar(n).expect("conferido");
                if (0xc4..=0xc6).contains(&m) {
                    e.folha_binario(s)?;
                } else {
                    e.folha_texto(0, s)?;
                }
            }
            0xc7..=0xc9 | 0xd4..=0xd8 => {
                let n = match m {
                    0xc7 => tam(&mut r, 1).ok_or_else(|| truncado(e))?,
                    0xc8 => tam(&mut r, 2).ok_or_else(|| truncado(e))?,
                    0xc9 => tam(&mut r, 4).ok_or_else(|| truncado(e))?,
                    _ => 1 << (m - 0xd4),
                };
                let tipo = r.u8().ok_or_else(|| truncado(e))? as i8;
                conferir(e, &r, n, 1)?;
                let dados = r.tomar(n).expect("conferido");
                ext(e, tipo, dados)?;
            }
            0xca => e.real(f32::from_be_bytes(r.be().ok_or_else(|| truncado(e))?))?,
            0xcb => e.real(f64::from_be_bytes(r.be().ok_or_else(|| truncado(e))?))?,
            0xcc => e.inteiro(r.u8().ok_or_else(|| truncado(e))?)?,
            0xcd => e.inteiro(u16::from_be_bytes(r.be().ok_or_else(|| truncado(e))?))?,
            0xce => e.inteiro(u32::from_be_bytes(r.be().ok_or_else(|| truncado(e))?))?,
            0xcf => e.inteiro(u64::from_be_bytes(r.be().ok_or_else(|| truncado(e))?))?,
            0xd0 => e.inteiro(r.u8().ok_or_else(|| truncado(e))? as i8)?,
            0xd1 => e.inteiro(i16::from_be_bytes(r.be().ok_or_else(|| truncado(e))?))?,
            0xd2 => e.inteiro(i32::from_be_bytes(r.be().ok_or_else(|| truncado(e))?))?,
            0xd3 => e.inteiro(i64::from_be_bytes(r.be().ok_or_else(|| truncado(e))?))?,
            0xc1 => return Err(e.malformado("byte 0xc1 não é usado pelo MessagePack")),
        }
        concluir(e, &mut pilha)?;
    }
    Ok(())
}

/// Extensão MessagePack: timestamp (−1) vira ISO 8601; o resto, `{"$ext": tipo, "$dados": b64}`.
fn ext(e: &mut Emissor, tipo: i8, d: &[u8]) -> io::Result<()> {
    let ts = match (tipo, d.len()) {
        (-1, 4) => Some((i64::from(u32::from_be_bytes(d.try_into().expect("4"))), 0)),
        (-1, 8) => {
            let v = u64::from_be_bytes(d.try_into().expect("8"));
            Some(((v & 0x3_ffff_ffff) as i64, (v >> 34) as u32))
        }
        (-1, 12) => Some((
            i64::from_be_bytes(d[4..].try_into().expect("8")),
            u32::from_be_bytes(d[..4].try_into().expect("4")),
        )),
        _ => None,
    };
    if let Some(iso) = ts.and_then(|(s, ns)| iso8601(s, ns)) {
        return e.folha_texto(0, iso.as_bytes());
    }
    e.abrir(true, 0)?;
    e.folha_texto(0, b"$ext")?;
    e.inteiro(tipo)?;
    e.folha_texto(0, b"$dados")?;
    e.folha_binario(d)?;
    e.fechar()
}

/// Segundos e nanossegundos desde 1970 em UTC; `None` fora dos anos 0–9999 ou nanos inválidos.
fn iso8601(seg: i64, nanos: u32) -> Option<String> {
    if nanos >= 1_000_000_000 {
        return None;
    }
    let (dias, s) = (seg.div_euclid(86_400), seg.rem_euclid(86_400));
    // Howard Hinnant, `civil_from_days`.
    let z = dias.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dia = doy - (153 * mp + 2) / 5 + 1;
    let mes = if mp < 10 { mp + 3 } else { mp - 9 };
    let ano = yoe + era * 400 + i64::from(mes <= 2);
    if !(0..=9999).contains(&ano) {
        return None;
    }
    let frac = if nanos == 0 {
        String::new()
    } else {
        format!(".{nanos:09}").trim_end_matches('0').to_string()
    };
    Some(format!(
        "{ano:04}-{mes:02}-{dia:02}T{:02}:{:02}:{:02}{frac}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    ))
}

// ---------------------------------------------------------------------------------------------
// CBOR (RFC 8949): itens concatenados (sequência CBOR, RFC 8742).

/// Contêiner CBOR aberto: itens que faltam (`None` = indefinido, até o `break`) e se é mapa.
struct Aberto {
    falta: Option<u64>,
    mapa: bool,
    vistos: u64,
}

/// Bignum até este tamanho vira inteiro decimal; acima, fica o binário (base64).
const MAX_BIGNUM: usize = 1024;

fn cbor(d: &[u8], e: &mut Emissor) -> io::Result<()> {
    let mut r = Leitor { d, pos: 0 };
    let mut pilha: Vec<Aberto> = Vec::new();
    while r.resta() > 0 || !pilha.is_empty() {
        e.pos = r.pos as u64;
        let b = r.u8().ok_or_else(|| truncado(e))?;
        let (maior, info) = (b >> 5, b & 0x1f);
        let arg = argumento(e, &mut r, info)?;
        match (maior, arg) {
            (0, Some(n)) => e.inteiro(n)?,
            (1, Some(n)) => e.inteiro(-1 - i128::from(n))?,
            (2 | 3, _) => {
                let s = cbor_cadeia(e, &mut r, maior, arg)?;
                if maior == 2 {
                    e.folha_binario(&s)?;
                } else {
                    e.folha_texto(0, &s)?;
                }
            }
            (4 | 5, falta) => {
                let mapa = maior == 5;
                let falta = falta.map(|n| if mapa { n.saturating_mul(2) } else { n });
                if let Some(n) = falta {
                    conferir(e, &r, n, 1)?;
                }
                e.abrir(mapa, 0)?;
                if falta != Some(0) {
                    pilha.push(Aberto {
                        falta,
                        mapa,
                        vistos: 0,
                    });
                    continue;
                }
                e.fechar()?;
            }
            (6, Some(t)) => {
                // Bignum: inteiro a partir da cadeia de bytes que vem logo depois.
                let prox = r.d.get(r.pos).copied();
                if (t == 2 || t == 3) && prox.is_some_and(|p| p >> 5 == 2 && p & 0x1f != 31) {
                    r.pos += 1;
                    let n = argumento(e, &mut r, prox.expect("visto") & 0x1f)?;
                    let s = cbor_cadeia(e, &mut r, 2, n)?;
                    if s.len() <= MAX_BIGNUM {
                        e.inteiro(decimal(&s, t == 3))?;
                    } else {
                        e.folha_binario(&s)?;
                    }
                } else {
                    // As outras marcas são transparentes: o item marcado sai como está.
                    continue;
                }
            }
            (7, _) => match info {
                20 | 21 => e.booleano(info == 21)?,
                22 | 23 => e.nulo()?,
                24 => e.folha_texto(0, format!("simple({})", arg.expect("u8")).as_bytes())?,
                25 => e.real(meia(arg.expect("u16") as u16))?,
                26 => e.real(f32::from_bits(arg.expect("u32") as u32))?,
                27 => e.real(f64::from_bits(arg.expect("u64")))?,
                31 => {
                    match pilha.last() {
                        Some(a) if a.falta.is_none() && (!a.mapa || a.vistos % 2 == 0) => {}
                        _ => return Err(e.malformado("break fora de contêiner indefinido")),
                    }
                    pilha.pop();
                    e.fechar()?;
                }
                _ => e.folha_texto(0, format!("simple({info})").as_bytes())?,
            },
            _ => return Err(e.malformado(format!("item {maior} com tamanho indefinido"))),
        }
        // Valor completo: avança os contêineres de cima e fecha os definidos que acabaram.
        while let Some(a) = pilha.last_mut() {
            a.vistos += 1;
            if a.falta != Some(a.vistos) {
                break;
            }
            pilha.pop();
            e.fechar()?;
        }
    }
    Ok(())
}

/// Argumento do cabeçalho CBOR; `None` = tamanho indefinido (31).
fn argumento(e: &Emissor, r: &mut Leitor, info: u8) -> io::Result<Option<u64>> {
    let t = |o: Option<u64>| o.ok_or_else(|| truncado(e)).map(Some);
    match info {
        0..=23 => Ok(Some(u64::from(info))),
        24 => t(r.u8().map(u64::from)),
        25 => t(r.be::<2>().map(|b| u64::from(u16::from_be_bytes(b)))),
        26 => t(r.be::<4>().map(|b| u64::from(u32::from_be_bytes(b)))),
        27 => t(r.be::<8>().map(u64::from_be_bytes)),
        31 => Ok(None),
        _ => Err(e.malformado(format!("informação adicional {info} reservada"))),
    }
}

/// Texto ou bytes CBOR; o indefinido junta os pedaços (até `max_texto`).
fn cbor_cadeia<'d>(
    e: &Emissor,
    r: &mut Leitor<'d>,
    maior: u8,
    arg: Option<u64>,
) -> io::Result<std::borrow::Cow<'d, [u8]>> {
    if let Some(n) = arg {
        conferir(e, r, n, 1)?;
        return Ok(r.tomar(n).expect("conferido").into());
    }
    let mut junto = Vec::new();
    loop {
        let b = r.u8().ok_or_else(|| truncado(e))?;
        if b == 0xff {
            return Ok(junto.into());
        }
        if b >> 5 != maior || b & 0x1f == 31 {
            return Err(e.malformado("pedaço de cadeia indefinida com tipo errado"));
        }
        let n = argumento(e, r, b & 0x1f)?.expect("não é 31");
        conferir(e, r, n, 1)?;
        if junto.len() as u64 + n > e.lim.max_texto {
            let max = e.lim.max_texto / MIB;
            return Err(e.teto(format!("texto ou binário em pedaços com mais de {max} MiB")));
        }
        junto.extend_from_slice(r.tomar(n).expect("conferido"));
    }
}

/// Inteiro sem sinal big-endian em decimal; `negativo` dá −1 − n (bignum negativo do CBOR).
fn decimal(bytes: &[u8], negativo: bool) -> String {
    const BASE: u64 = 1_000_000_000;
    let mut membros: Vec<u64> = Vec::new(); // base 10^9, menos significativo primeiro
    let somar = |membros: &mut Vec<u64>, mul: u64, mut vai: u64| {
        for m in membros.iter_mut() {
            let v = *m * mul + vai;
            *m = v % BASE;
            vai = v / BASE;
        }
        while vai > 0 {
            membros.push(vai % BASE);
            vai /= BASE;
        }
    };
    for &b in bytes {
        somar(&mut membros, 256, u64::from(b));
    }
    if negativo {
        somar(&mut membros, 1, 1);
    }
    let mut s = String::from(if negativo { "-" } else { "" });
    match membros.split_last() {
        None => s.push('0'),
        Some((alto, resto)) => {
            s.push_str(&alto.to_string());
            for m in resto.iter().rev() {
                s.push_str(&format!("{m:09}"));
            }
        }
    }
    s
}

/// Meia precisão (IEEE 754 binary16) para f64.
fn meia(h: u16) -> f64 {
    let sinal = if h >> 15 == 1 { -1.0 } else { 1.0 };
    let (exp, mant) = (i32::from(h >> 10 & 0x1f), f64::from(h & 0x3ff));
    sinal
        * match exp {
            0 => mant * 2f64.powi(-24),
            31 if mant == 0.0 => f64::INFINITY,
            31 => f64::NAN,
            _ => (1024.0 + mant) * 2f64.powi(exp - 25),
        }
}

// ---------------------------------------------------------------------------------------------
// BSON (https://bsonspec.org): documentos concatenados, como o `mongodump` grava.

fn bson(d: &[u8], e: &mut Emissor) -> io::Result<()> {
    use bson::raw::{RawBsonRef, RawDocument};
    enum Iter<'a> {
        Doc(bson::raw::RawIter<'a>),
        Lista(bson::raw::RawArrayIter<'a>),
        /// Fecha o mapa de fora do `$scope` de um código com escopo.
        Fechar,
    }
    let mut r = Leitor { d, pos: 0 };
    while r.resta() > 0 {
        e.pos = r.pos as u64;
        let cab = r.d[r.pos..].get(..4).ok_or_else(|| truncado(e))?;
        let tam = i32::from_le_bytes(cab.try_into().expect("4"));
        if tam < 5 {
            return Err(e.malformado(format!("documento declara {tam} bytes (mínimo 5)")));
        }
        conferir(e, &r, tam as u64, 1)?;
        let doc = RawDocument::from_bytes(r.tomar(tam as u64).expect("conferido"))
            .map_err(|err| e.malformado(err))?;
        e.abrir(true, 0)?;
        let mut pilha = vec![Iter::Doc(doc.iter_elements())];
        while let Some(topo) = pilha.last_mut() {
            let prox = match topo {
                Iter::Doc(it) => it
                    .next()
                    .map(|x| x.and_then(|el| Ok((Some(el.key().as_str()), el.value()?)))),
                Iter::Lista(it) => it.next().map(|x| x.map(|v| (None, v))),
                Iter::Fechar => None,
            };
            let (chave, valor) = match prox {
                None => {
                    pilha.pop();
                    e.fechar()?;
                    continue;
                }
                Some(Err(err)) => return Err(e.malformado(err)),
                Some(Ok(kv)) => kv,
            };
            if let Some(k) = chave {
                e.folha_texto(0, k.as_bytes())?;
            }
            match valor {
                RawBsonRef::Document(d) => {
                    e.abrir(true, 0)?;
                    pilha.push(Iter::Doc(d.iter_elements()));
                }
                RawBsonRef::Array(a) => {
                    e.abrir(false, 0)?;
                    pilha.push(Iter::Lista(a.into_iter()));
                }
                RawBsonRef::JavaScriptCodeWithScope(c) => {
                    e.abrir(true, 0)?;
                    e.folha_texto(0, b"$code")?;
                    e.folha_texto(0, c.code.as_bytes())?;
                    e.folha_texto(0, b"$scope")?;
                    e.abrir(true, 0)?;
                    pilha.push(Iter::Fechar);
                    pilha.push(Iter::Doc(c.scope.iter_elements()));
                }
                folha => {
                    // Folhas não aninham: o Extended JSON relaxado delas tem no máximo 2 níveis.
                    let v = bson::Bson::try_from(folha)
                        .map_err(|err| e.malformado(err))?
                        .into_relaxed_extjson();
                    let prof = match &v {
                        serde_json::Value::Object(o) => {
                            1 + usize::from(o.values().any(serde_json::Value::is_object))
                        }
                        _ => 0,
                    };
                    let json = serde_json::to_vec(&v).map_err(io::Error::other)?;
                    e.folha(0, &json, prof)?;
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// YAML 1.2, esquema core, por eventos (sem montar a árvore).

const TAG_YAML: &str = "tag:yaml.org,2002:";

fn yaml(d: &[u8], e: &mut Emissor) -> io::Result<()> {
    use yaml_rust2::parser::{Event, Parser};
    let s = std::str::from_utf8(d).map_err(|err| e.malformado(format!("não é UTF-8 ({err})")))?;
    let s = s.strip_prefix('\u{FEFF}').unwrap_or(s);
    let mut p = Parser::new_from_str(s);
    loop {
        let (ev, marca) = p.next_token().map_err(|err| e.malformado(err))?;
        e.pos = marca.index() as u64;
        match ev {
            Event::StreamEnd => return Ok(()),
            Event::Alias(id) => e.alias(id)?,
            Event::Scalar(v, estilo, ancora, tag) => {
                let simples = estilo == yaml_rust2::scanner::TScalarStyle::Plain;
                let tipo = match &tag {
                    None if simples => None,
                    None => Some("str"),
                    // `!` sozinho: "não resolva", fica texto.
                    Some(t) if t.handle == "!" && t.suffix.is_empty() => Some("str"),
                    Some(t) if t.handle == TAG_YAML => Some(t.suffix.as_str()),
                    Some(t) => return Err(tag_recusada(e, &t.handle, &t.suffix)),
                };
                yaml_escalar(e, ancora, &v, tipo)?;
            }
            Event::SequenceStart(ancora, tag) => {
                yaml_tag_contem(e, tag.as_ref(), "seq")?;
                e.abrir(false, ancora)?;
            }
            Event::MappingStart(ancora, tag) => {
                yaml_tag_contem(e, tag.as_ref(), "map")?;
                e.abrir(true, ancora)?;
            }
            Event::SequenceEnd | Event::MappingEnd => e.fechar()?,
            _ => {}
        }
    }
}

fn tag_recusada(e: &Emissor, handle: &str, sufixo: &str) -> io::Error {
    let tag = if handle == TAG_YAML {
        format!("!!{sufixo}")
    } else {
        format!("{handle}{sufixo}")
    };
    e.teto(format!(
        "tag {tag} recusada: o Jasão só lê os tipos do YAML puro (texto, número, booleano, \
         nulo, lista, mapa)"
    ))
}

fn yaml_tag_contem(
    e: &Emissor,
    tag: Option<&yaml_rust2::parser::Tag>,
    tipo: &str,
) -> io::Result<()> {
    match tag {
        None => Ok(()),
        Some(t) if (t.handle == "!" && t.suffix.is_empty()) => Ok(()),
        Some(t) if t.handle == TAG_YAML && t.suffix == tipo => Ok(()),
        Some(t) => Err(tag_recusada(e, &t.handle, &t.suffix)),
    }
}

/// Escalar YAML com o tipo pedido pela tag (`None` = resolver pelo esquema core).
fn yaml_escalar(e: &mut Emissor, ancora: usize, v: &str, tipo: Option<&str>) -> io::Result<()> {
    let nulo = matches!(v, "" | "~" | "null" | "Null" | "NULL");
    let booleano = match v {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    };
    let inteiro = yaml_inteiro(v);
    let real = yaml_real(v);
    let json: Option<String> = match tipo {
        None => {
            if nulo {
                Some("null".into())
            } else if let Some(b) = booleano {
                Some(b.to_string())
            } else {
                inteiro.or(real)
            }
        }
        Some("str" | "binary" | "timestamp") => None,
        Some("null") if nulo => Some("null".into()),
        Some("bool") if booleano.is_some() => booleano.map(|b| b.to_string()),
        Some("int") if inteiro.is_some() => inteiro,
        Some("float") if real.is_some() || inteiro.is_some() => real.or(inteiro),
        Some(t @ ("null" | "bool" | "int" | "float")) => {
            return Err(e.malformado(format!("{v:?} não é !!{t}")));
        }
        Some(t) => return Err(tag_recusada(e, TAG_YAML, t)),
    };
    match json {
        Some(j) => e.folha(ancora, j.as_bytes(), 0),
        None => e.folha_texto(ancora, v.as_bytes()),
    }
}

/// Inteiro do esquema core (decimal, `0o`, `0x`) como JSON; decimal grande sai com todos os
/// dígitos.
fn yaml_inteiro(v: &str) -> Option<String> {
    if let Some(h) = v.strip_prefix("0x") {
        return u128::from_str_radix(h, 16).ok().map(|n| n.to_string());
    }
    if let Some(o) = v.strip_prefix("0o") {
        return u128::from_str_radix(o, 8).ok().map(|n| n.to_string());
    }
    let (sinal, digitos) = match v.as_bytes().first() {
        Some(b'-') => ("-", &v[1..]),
        Some(b'+') => ("", &v[1..]),
        _ => ("", v),
    };
    if digitos.is_empty() || !digitos.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // JSON não aceita zero à esquerda.
    let digitos = digitos.trim_start_matches('0');
    Some(format!(
        "{sinal}{}",
        if digitos.is_empty() { "0" } else { digitos }
    ))
}

/// Real do esquema core como JSON; `.nan` e `.inf` viram os textos de NaN e infinito.
fn yaml_real(v: &str) -> Option<String> {
    let sem_sinal = v.trim_start_matches(['-', '+']);
    if matches!(sem_sinal, ".inf" | ".Inf" | ".INF") && v.len() - sem_sinal.len() <= 1 {
        return Some(
            if v.starts_with('-') {
                "\"-Infinity\""
            } else {
                "\"Infinity\""
            }
            .into(),
        );
    }
    if matches!(v, ".nan" | ".NaN" | ".NAN") {
        return Some("\"NaN\"".into());
    }
    // Só o alfabeto do esquema core; o `parse` do Rust aceitaria `inf`, `nan`, `infinity`.
    if !v
        .bytes()
        .all(|c| c.is_ascii_digit() || b"+-.eE".contains(&c))
        || !v.bytes().any(|c| c.is_ascii_digit())
    {
        return None;
    }
    let x: f64 = v.parse().ok()?;
    Some(if x.is_infinite() {
        if x > 0.0 {
            "\"Infinity\""
        } else {
            "\"-Infinity\""
        }
        .into()
    } else {
        format!("{x:?}")
    })
}

// ---------------------------------------------------------------------------------------------
// TOML 1.1: carregado inteiro (o formato não tem leitura em fluxo), com teto de tamanho.

fn toml(d: &[u8], e: &mut Emissor) -> io::Result<()> {
    enum Iter<'a> {
        Tab(toml::map::Iter<'a, String, toml::Value>),
        Lista(std::slice::Iter<'a, toml::Value>),
    }
    if d.len() as u64 > e.lim.max_toml {
        let max = e.lim.max_toml >> 10;
        return Err(e.teto(format!(
            "arquivo com mais de {max} KiB; TOML é carregado inteiro na memória, que gasta até \
             ~180× o arquivo"
        )));
    }
    let s = std::str::from_utf8(d).map_err(|err| e.malformado(format!("não é UTF-8 ({err})")))?;
    let t: toml::Table = s.parse().map_err(|err: toml::de::Error| {
        // A mensagem do `toml` cita a linha inteira, que pode ser o arquivo todo.
        let mut m = err.to_string();
        if let Some((i, _)) = m.char_indices().nth(500) {
            m.truncate(i);
            m.push('…');
        }
        e.malformado(m)
    })?;
    e.abrir(true, 0)?;
    let mut pilha = vec![Iter::Tab(t.iter())];
    while let Some(topo) = pilha.last_mut() {
        let valor = match topo {
            Iter::Tab(it) => it.next().map(|(k, v)| (Some(k), v)),
            Iter::Lista(it) => it.next().map(|v| (None, v)),
        };
        let Some((chave, valor)) = valor else {
            pilha.pop();
            e.fechar()?;
            continue;
        };
        if let Some(k) = chave {
            e.folha_texto(0, k.as_bytes())?;
        }
        match valor {
            toml::Value::String(s) => e.folha_texto(0, s.as_bytes())?,
            toml::Value::Integer(i) => e.inteiro(i)?,
            toml::Value::Float(f) => e.real(*f)?,
            toml::Value::Boolean(b) => e.booleano(*b)?,
            toml::Value::Datetime(dt) => e.folha_texto(0, dt.to_string().as_bytes())?,
            toml::Value::Array(a) => {
                e.abrir(false, 0)?;
                pilha.push(Iter::Lista(a.iter()));
            }
            toml::Value::Table(t) => {
                e.abrir(true, 0)?;
                pilha.push(Iter::Tab(t.iter()));
            }
        }
    }
    Ok(())
}
