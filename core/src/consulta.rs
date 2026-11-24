//! Consulta jq (pelo `jaq`) sobre JSONL ou sobre um documento JSON único.
//!
//! - O filtro é compilado uma vez (`Consulta::nova`); erro de compilação vem com linha, coluna
//!   e trecho, nunca pânico.
//! - JSONL: o filtro roda em cada linha, em blocos paralelos que terminam em `\n` (mesmo molde
//!   de `busca.rs`). Os resultados chegam por callback em lotes, em ordem de arquivo, com o
//!   número da linha de origem (a partir de 0) e o progresso em linhas.
//! - Linha vazia (só espaço) é pulada; linha que não é JSON, aninhada demais ou em que o filtro
//!   falha conta como erro, guarda os primeiros `MAX_ERROS_GUARDADOS` e a varredura segue.
//! - Cancelamento por `AtomicBool` e teto de tempo opcional, conferidos entre linhas e entre
//!   resultados: `range(1e12)` para. Limite deste módulo: filtro que não entrega nada enquanto
//!   gira (`last(range(1e12))`, `def f: f; f`) só é visto parar no fim, e recursão sem fundo
//!   estoura a pilha e aborta o processo. Quem chama isola: o app roda a consulta num processo
//!   filho e o mata ao cancelar (`app/src-tauri/src/filho.rs`); a CLI tem um vigia que encerra
//!   o processo depois do `--tempo-max`, e em recursão sem fundo ela aborta (código de sinal).
//!   Ver `docs/decisoes.md`.
//! - Números: inteiro de qualquer tamanho é exato (também em soma, subtração e produto);
//!   decimal sai como veio enquanto não entra em conta, e numa conta vira `f64`.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use jaq_core::load::{Arena, File, Loader, lex::Expect};
use jaq_core::{Compiler, Ctx, Vars, compile, data::JustLut, load, unwrap_valr};
use jaq_json::{Num, Val};
use memchr::{memchr, memchr_iter};

use crate::linhas::BOM;

type Filtro = jaq_core::Filter<JustLut<Val>>;

/// Tamanho-alvo de um bloco de varredura.
pub const BLOCO_PADRAO: usize = 256 << 10;
/// Resultados que um trabalhador junta antes de entregar; também o teto de um lote.
pub const MAX_LOTE_PADRAO: usize = 10_000;
/// Quantos erros de linha o resumo guarda com linha e motivo (os demais só contam).
pub const MAX_ERROS_GUARDADOS: usize = 100;
/// Aninhamento máximo de uma linha (o mesmo do `jq`); acima disso o parser estoura a pilha.
pub const MAX_ANINHAMENTO: usize = 10_000;
/// Pilha das threads que compilam e avaliam: o parser desce no filtro, e `..`, `tostring` e
/// cia. descem no valor. É reserva virtual; só ocupa memória o que a recursão usar.
const PILHA: usize = 256 << 20;
/// Tamanho máximo do filtro: limita o pior caso (um nível por byte, `-----1`, `((((.`). Medido:
/// com `PILHA` o filtro de 16 KiB compila e roda até em debug (com 128 MiB, não).
pub const MAX_FILTRO: usize = 16 << 10;
const ADIANTE_MAX: usize = 64;
/// Bytes que um trabalhador junta antes de entregar (com `MAX_LOTE_PADRAO`, o teto de memória
/// por trabalhador: um bloco adiantado espera a sua vez com no máximo isso em mãos).
pub const MAX_BYTES_LOTE_PADRAO: usize = 8 << 20;

/// Um valor produzido pelo filtro: linha de origem (a partir de 0; 0 no documento único) e o
/// valor em JSON compacto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resultado {
    pub linha: usize,
    pub json: String,
}

/// Linha em que a consulta falhou, com o motivo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErroLinha {
    pub linha: usize,
    pub motivo: String,
}

/// O que o chamador recebe durante a varredura.
#[derive(Debug, Default)]
pub struct LoteConsulta {
    pub resultados: Vec<Resultado>,
    /// Linhas já varridas (todas antes desta já foram entregues).
    pub linhas: usize,
    pub total_linhas: usize,
}

/// Fim da varredura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumoConsulta {
    pub resultados: u64,
    pub linhas: usize,
    pub total_linhas: usize,
    pub erros: u64,
    /// Os primeiros `MAX_ERROS_GUARDADOS` erros, em ordem de arquivo.
    pub primeiros_erros: Vec<ErroLinha>,
    pub cancelado: bool,
    pub tempo_esgotado: bool,
    /// A saída passou do teto de `com_teto_saida`: entregue até o teto, parou.
    pub cortado: bool,
}

/// Um problema no texto do filtro: faixa em bytes, linha e coluna (a partir de 1, coluna em
/// caracteres) e mensagem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Falha {
    pub faixa: Range<usize>,
    pub linha: usize,
    pub coluna: usize,
    pub mensagem: String,
}

/// O filtro não compila.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErroFiltro {
    pub falhas: Vec<Falha>,
}

impl fmt::Display for ErroFiltro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "filtro inválido")?;
        for x in &self.falhas {
            write!(f, "\n  {}:{}: {}", x.linha, x.coluna, x.mensagem)?;
        }
        Ok(())
    }
}

impl std::error::Error for ErroFiltro {}

/// Filtro compilado, pronto para rodar em um ou mais arquivos.
pub struct Consulta {
    filtro: Filtro,
    bloco: usize,
    max_lote: usize,
    max_bytes_lote: usize,
    tempo_max: Option<Duration>,
    max_resultados: u64,
    max_bytes: u64,
    max_por_registro: u64,
    tempo_por_registro: Option<Duration>,
}

impl Consulta {
    pub fn nova(codigo: &str) -> Result<Self, ErroFiltro> {
        // O parser e o compilador do jaq descem recursivamente no filtro: roda numa pilha
        // grande para `((((…` fundo virar erro ou filtro, não aborto do processo.
        thread::scope(|s| {
            thread::Builder::new()
                .stack_size(PILHA)
                .spawn_scoped(s, || Self::compilar(codigo))
                .expect("criar thread de compilação")
                .join()
                .expect("compilação do filtro entrou em pânico")
        })
    }

    fn compilar(codigo: &str) -> Result<Self, ErroFiltro> {
        if codigo.len() > MAX_FILTRO {
            let corte = (0..=MAX_FILTRO)
                .rev()
                .find(|&i| codigo.is_char_boundary(i))
                .unwrap_or(0);
            let mensagem = format!("filtro acima de {} KiB", MAX_FILTRO >> 10);
            return Err(ErroFiltro {
                falhas: vec![falha(codigo, &codigo[corte..], mensagem)],
            });
        }
        let defs = jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs());
        let funs = jaq_core::funs()
            .chain(jaq_std::funs())
            .chain(jaq_json::funs());
        let arena = Arena::default();
        let arquivo = File {
            code: codigo,
            path: (),
        };
        let modulos = Loader::new(defs)
            .load(&arena, arquivo)
            .map_err(|e| erro_carga(codigo, e))?;
        let filtro = Compiler::default()
            .with_funs(funs)
            .compile(modulos)
            .map_err(|e| erro_compilacao(codigo, e))?;
        Ok(Self {
            filtro,
            bloco: BLOCO_PADRAO,
            max_lote: MAX_LOTE_PADRAO,
            max_bytes_lote: MAX_BYTES_LOTE_PADRAO,
            tempo_max: None,
            max_resultados: u64::MAX,
            max_bytes: u64::MAX,
            max_por_registro: u64::MAX,
            tempo_por_registro: None,
        })
    }

    /// Troca o tamanho-alvo do bloco (testes e fuzz usam blocos minúsculos).
    pub fn com_bloco(mut self, bytes: usize) -> Self {
        self.bloco = bytes.max(1);
        self
    }

    /// Teto de resultados por lote (e em memória por trabalhador).
    pub fn com_max_lote(mut self, n: usize) -> Self {
        self.max_lote = n.max(1);
        self
    }

    /// Teto de bytes por lote (e em memória por trabalhador), além de `com_max_lote`.
    pub fn com_max_bytes_lote(mut self, n: usize) -> Self {
        self.max_bytes_lote = n.max(1);
        self
    }

    /// Teto de saída: entrega no máximo `resultados` valores e `bytes` de JSON (cada um conta
    /// com o `\n`), depois para com `ResumoConsulta::cortado`.
    pub fn com_teto_saida(mut self, resultados: Option<u64>, bytes: Option<u64>) -> Self {
        self.max_resultados = resultados.unwrap_or(u64::MAX);
        self.max_bytes = bytes.unwrap_or(u64::MAX);
        self
    }

    /// Teto por registro: passar de `resultados` valores ou de `tempo` numa mesma linha (ou no
    /// documento) vira erro daquela linha e a varredura segue. Conferido entre resultados.
    pub fn com_teto_registro(mut self, resultados: Option<u64>, tempo: Option<Duration>) -> Self {
        self.max_por_registro = resultados.unwrap_or(u64::MAX);
        self.tempo_por_registro = tempo;
        self
    }

    /// Para a varredura depois de `tempo` (conferido como o cancelamento).
    pub fn com_tempo_max(mut self, tempo: Option<Duration>) -> Self {
        self.tempo_max = tempo;
        self
    }

    /// Roda o filtro em cada linha de `dados` (JSONL). `ao_lote` roda na thread do chamador,
    /// em ordem de arquivo; recebe um lote quando há resultados e, sem eles, a cada ~1% das
    /// linhas e no fim. Visto `cancelar` (ou o teto de tempo), nenhum lote é entregue e
    /// `ResumoConsulta::linhas` para na última linha entregue.
    pub fn executar_jsonl(
        &self,
        dados: &[u8],
        cancelar: &AtomicBool,
        ao_lote: impl FnMut(LoteConsulta),
    ) -> ResumoConsulta {
        self.executar(dados, false, cancelar, ao_lote)
    }

    /// Roda o filtro no documento inteiro (um valor JSON; BOM fora). Os resultados vêm com
    /// linha 0; o total de linhas é 1 (0 se o documento for vazio).
    pub fn executar_documento(
        &self,
        dados: &[u8],
        cancelar: &AtomicBool,
        ao_lote: impl FnMut(LoteConsulta),
    ) -> ResumoConsulta {
        self.executar(dados, true, cancelar, ao_lote)
    }

    fn executar(
        &self,
        dados: &[u8],
        documento: bool,
        cancelar: &AtomicBool,
        mut ao_lote: impl FnMut(LoteConsulta),
    ) -> ResumoConsulta {
        let prazo = self.tempo_max.map(|t| Instant::now() + t);
        let esgotou = || prazo.is_some_and(|p| Instant::now() >= p);
        let cortar = AtomicBool::new(false);
        let parar = || cancelar.load(Relaxed) || cortar.load(Relaxed) || esgotou();

        let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
        let blocos = if documento {
            Some(base..dados.len())
                .filter(|b| !b.is_empty())
                .into_iter()
                .collect()
        } else {
            crate::busca::fatiar(dados, base, self.bloco)
        };
        // Linha inicial de cada bloco; uma passada de `memchr` (vários GB/s).
        let mut inicio_linha = Vec::with_capacity(blocos.len() + 1);
        let mut total_linhas = 0;
        for b in &blocos {
            inicio_linha.push(total_linhas);
            total_linhas += if documento {
                1
            } else {
                contar_linhas(&dados[b.clone()])
            };
        }

        let mut resumo = ResumoConsulta {
            resultados: 0,
            linhas: 0,
            total_linhas,
            erros: 0,
            primeiros_erros: Vec::new(),
            cancelado: false,
            tempo_esgotado: false,
            cortado: false,
        };
        let mut bytes_saida = 0u64;
        let trabalhadores = thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(blocos.len())
            .max(1);
        let proximo = AtomicUsize::new(0);
        let entregues = AtomicUsize::new(0);

        thread::scope(|s| {
            let (tx, rx) = mpsc::sync_channel::<(usize, Parte)>(trabalhadores * 2);
            for _ in 0..trabalhadores {
                let tx = tx.clone();
                let (blocos, inicio_linha) = (&blocos, &inicio_linha);
                let (proximo, entregues, parar) = (&proximo, &entregues, &parar);
                let trabalho = move || {
                    loop {
                        if parar() {
                            return;
                        }
                        let i = proximo.fetch_add(1, Relaxed);
                        let Some(faixa) = blocos.get(i) else { return };
                        // Espera a sua vez de entregar; devolve falso se for para parar.
                        let esperar_vez = |ate: usize| {
                            while i >= entregues.load(Relaxed) + ate {
                                if parar() {
                                    return false;
                                }
                                thread::sleep(Duration::from_micros(100));
                            }
                            true
                        };
                        if !esperar_vez(ADIANTE_MAX) {
                            return;
                        }
                        let mut saida = Saida {
                            linhas: inicio_linha[i],
                            ..Saida::default()
                        };
                        let mut enviar = |parte: Parte, fim: bool| {
                            // Parte no meio do bloco só sai quando o bloco é o da vez: assim
                            // o que espera na fila é limitado a um lote por trabalhador.
                            (fim || esperar_vez(1)) && tx.send((i, parte)).is_ok()
                        };
                        let bloco = &dados[faixa.clone()];
                        let ok = if documento {
                            self.avaliar(bloco, 0, &mut saida, &parar, &mut enviar)
                        } else {
                            self.varrer_bloco(
                                bloco,
                                inicio_linha[i],
                                &mut saida,
                                &parar,
                                &mut enviar,
                            )
                        };
                        if !ok || !enviar(saida.tirar(true), true) {
                            return;
                        }
                    }
                };
                thread::Builder::new()
                    .stack_size(PILHA)
                    .spawn_scoped(s, trabalho)
                    .expect("criar thread da consulta");
            }
            drop(tx);

            let mut pendentes: BTreeMap<usize, Parte> = BTreeMap::new();
            let (mut prox, mut ultimo_aviso) = (0, 0);
            let passo = (total_linhas / 100).max(1);
            'receber: for (i, parte) in &rx {
                // Parte no meio de um bloco só chega quando ele é o da vez; bloco adiantado
                // chega inteiro e espera aqui.
                let mut chegou = if i == prox {
                    Some(parte)
                } else {
                    pendentes.insert(i, parte);
                    None
                };
                while let Some(parte) = chegou.take().or_else(|| pendentes.remove(&prox)) {
                    // Conferido a cada parte: nenhum lote sai depois de parar.
                    if parar() {
                        break 'receber;
                    }
                    let mut resultados = parte.resultados;
                    let livre = self.max_resultados - resumo.resultados;
                    if resultados.len() as u64 > livre {
                        resultados.truncate(livre as usize);
                        resumo.cortado = true;
                    }
                    if let Some(i) = resultados.iter().position(|r| {
                        bytes_saida += r.json.len() as u64 + 1;
                        bytes_saida > self.max_bytes
                    }) {
                        resultados.truncate(i);
                        resumo.cortado = true;
                    }
                    resumo.resultados += resultados.len() as u64;
                    resumo.erros += parte.erros;
                    let vaga = MAX_ERROS_GUARDADOS - resumo.primeiros_erros.len();
                    resumo
                        .primeiros_erros
                        .extend(parte.guardados.into_iter().take(vaga));
                    resumo.linhas = parte.linhas;
                    if parte.fim {
                        resumo.linhas = inicio_linha.get(prox + 1).copied().unwrap_or(total_linhas);
                        prox += 1;
                        entregues.store(prox, Relaxed);
                    }
                    let fim = prox == blocos.len();
                    if !resultados.is_empty()
                        || fim
                        || resumo.cortado
                        || resumo.linhas - ultimo_aviso >= passo
                    {
                        ultimo_aviso = resumo.linhas;
                        ao_lote(LoteConsulta {
                            resultados,
                            linhas: resumo.linhas,
                            total_linhas,
                        });
                    }
                    if resumo.cortado {
                        cortar.store(true, Relaxed);
                        break 'receber;
                    }
                }
            }
            drop(rx);
            resumo.cancelado = prox < blocos.len() && !resumo.cortado;
            resumo.tempo_esgotado = resumo.cancelado && esgotou();
        });
        resumo
    }

    /// Avalia cada linha de um bloco que começa no início de uma linha.
    fn varrer_bloco(
        &self,
        b: &[u8],
        linha0: usize,
        saida: &mut Saida,
        parar: &dyn Fn() -> bool,
        enviar: &mut dyn FnMut(Parte, bool) -> bool,
    ) -> bool {
        let (mut ini, mut linha) = (0, linha0);
        while ini < b.len() {
            let fim = memchr(b'\n', &b[ini..]).map_or(b.len(), |p| ini + p);
            if !self.avaliar(&b[ini..fim], linha, saida, parar, enviar) {
                return false;
            }
            linha += 1;
            saida.linhas = linha;
            ini = fim + 1;
        }
        true
    }

    /// Avalia um valor JSON (uma linha ou o documento). Devolve falso se for para parar.
    fn avaliar(
        &self,
        texto: &[u8],
        linha: usize,
        saida: &mut Saida,
        parar: &dyn Fn() -> bool,
        enviar: &mut dyn FnMut(Parte, bool) -> bool,
    ) -> bool {
        if texto.iter().all(u8::is_ascii_whitespace) {
            return true;
        }
        if aninhamento_excede(texto, MAX_ANINHAMENTO) {
            saida.erro(
                linha,
                format!("aninhamento acima de {MAX_ANINHAMENTO} níveis"),
            );
            return true;
        }
        let valor = match jaq_json::read::parse_single(texto) {
            Ok(v) => v,
            Err(e) => {
                saida.erro(linha, format!("JSON inválido: {e}"));
                return true;
            }
        };
        let ctx = Ctx::<JustLut<Val>>::new(&self.filtro.lut, Vars::new([]));
        let prazo = self.tempo_por_registro.map(|t| Instant::now() + t);
        let mut n = 0u64;
        for r in self.filtro.id.run((ctx, valor)).map(unwrap_valr) {
            if parar() {
                return false;
            }
            n += 1;
            if n > self.max_por_registro {
                let max = self.max_por_registro;
                saida.erro(linha, format!("o registro passou de {max} resultados"));
                break;
            }
            if prazo.is_some_and(|p| Instant::now() >= p) {
                let t = self.tempo_por_registro.unwrap_or_default();
                saida.erro(linha, format!("o registro passou de {t:.1?} de consulta"));
                break;
            }
            match r {
                Ok(v) => {
                    let json = para_json(&v);
                    saida.bytes += json.len();
                    saida.resultados.push(Resultado { linha, json });
                }
                Err(e) => {
                    // Como no jq: o erro encerra os resultados desta entrada.
                    saida.erro(linha, e.to_string());
                    break;
                }
            }
            let cheio =
                saida.resultados.len() >= self.max_lote || saida.bytes >= self.max_bytes_lote;
            if cheio && !enviar(saida.tirar(false), false) {
                return false;
            }
        }
        true
    }
}

/// O que um trabalhador entrega de um bloco: inteiro (`fim`) ou em partes.
#[derive(Default)]
struct Parte {
    resultados: Vec<Resultado>,
    erros: u64,
    guardados: Vec<ErroLinha>,
    /// Linha absoluta até onde esta parte cobre.
    linhas: usize,
    fim: bool,
}

#[derive(Default)]
struct Saida {
    resultados: Vec<Resultado>,
    erros: u64,
    guardados: Vec<ErroLinha>,
    linhas: usize,
    /// Bytes de JSON em `resultados`.
    bytes: usize,
}

impl Saida {
    fn erro(&mut self, linha: usize, motivo: String) {
        self.erros += 1;
        if self.guardados.len() < MAX_ERROS_GUARDADOS {
            self.guardados.push(ErroLinha { linha, motivo });
        }
    }

    fn tirar(&mut self, fim: bool) -> Parte {
        self.bytes = 0;
        Parte {
            resultados: std::mem::take(&mut self.resultados),
            erros: std::mem::take(&mut self.erros),
            guardados: std::mem::take(&mut self.guardados),
            linhas: self.linhas,
            fim,
        }
    }
}

/// Linhas de um bloco pelas regras do índice: quebra final não cria linha.
fn contar_linhas(b: &[u8]) -> usize {
    memchr_iter(b'\n', b).count() + usize::from(!b.is_empty() && !b.ends_with(b"\n"))
}

/// Se o JSON passa de `max` níveis de `[`/`{` (fora de strings). Não valida nada mais.
pub(crate) fn aninhamento_excede(texto: &[u8], max: usize) -> bool {
    let (mut nivel, mut em_string, mut escape) = (0usize, false, false);
    for &c in texto {
        if em_string {
            match c {
                _ if escape => escape = false,
                b'\\' => escape = true,
                b'"' => em_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => em_string = true,
            b'[' | b'{' => {
                nivel += 1;
                if nivel > max {
                    return true;
                }
            }
            b']' | b'}' => nivel = nivel.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// JSON compacto. O `jaq` escreve `NaN` e `Infinity`, que não são JSON: como no `jq`, `NaN`
/// vira `null` e infinito vira o maior `f64` com sinal.
fn para_json(v: &Val) -> String {
    let s = v.to_string();
    if s.contains("NaN") || s.contains("Infinity") {
        trocar_nao_finitos(v).to_string()
    } else {
        s
    }
}

fn trocar_nao_finitos(v: &Val) -> Val {
    match v {
        Val::Num(Num::Float(f)) if f.is_nan() => Val::Null,
        Val::Num(Num::Float(f)) if f.is_infinite() => Val::Num(Num::Float(f64::MAX.copysign(*f))),
        Val::Arr(a) => Val::Arr(a.iter().map(trocar_nao_finitos).collect::<Vec<_>>().into()),
        Val::Obj(o) => Val::obj(
            o.iter()
                .map(|(k, v)| (k.clone(), trocar_nao_finitos(v)))
                .collect(),
        ),
        v => v.clone(),
    }
}

fn falha(codigo: &str, trecho: &str, mensagem: String) -> Falha {
    let faixa = load::span(codigo, trecho);
    let antes = &codigo[..faixa.start];
    let linha = antes.matches('\n').count() + 1;
    let coluna = antes[antes.rfind('\n').map_or(0, |p| p + 1)..]
        .chars()
        .count()
        + 1;
    Falha {
        faixa,
        linha,
        coluna,
        mensagem,
    }
}

fn achado(trecho: &str) -> String {
    match trecho.chars().next() {
        None => "o filtro terminou antes".into(),
        Some(c) => format!("encontrado `{c}`"),
    }
}

fn erro_carga(codigo: &str, erros: load::Errors<&str, ()>) -> ErroFiltro {
    let mut falhas = Vec::new();
    for (_, e) in erros {
        match e {
            load::Error::Io(v) => {
                for (caminho, e) in v {
                    falhas.push(falha(codigo, caminho, format!("módulo `{caminho}`: {e}")));
                }
            }
            load::Error::Lex(v) => {
                for (esperado, trecho) in v {
                    let o_que = match esperado {
                        // `Expect::as_str` entra em pânico em delimitador fora da lista.
                        Expect::Delim(abre) => format!("fechar `{abre}`"),
                        e => e.as_str().to_string(),
                    };
                    let msg = format!("esperado {o_que}; {}", achado(trecho));
                    falhas.push(falha(codigo, trecho, msg));
                }
            }
            load::Error::Parse(v) => {
                for (esperado, trecho) in v {
                    let msg = format!("esperado {}; {}", esperado.as_str(), achado(trecho));
                    falhas.push(falha(codigo, trecho, msg));
                }
            }
        }
    }
    ErroFiltro { falhas }
}

fn erro_compilacao(codigo: &str, erros: compile::Errors<&str, ()>) -> ErroFiltro {
    let mut falhas = Vec::new();
    for (_, v) in erros {
        for (trecho, indefinido) in v {
            let msg = match indefinido {
                compile::Undefined::Filter(aridade) => {
                    format!("filtro `{trecho}/{aridade}` não existe")
                }
                compile::Undefined::Var => format!("variável `{trecho}` não definida"),
                compile::Undefined::Label => format!("rótulo `{trecho}` não definido"),
                u => format!("{} `{trecho}` não definido", u.as_str()),
            };
            falhas.push(falha(codigo, trecho, msg));
        }
    }
    ErroFiltro { falhas }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn rodar(filtro: &str, dados: &[u8], bloco: usize) -> (Vec<(usize, String)>, ResumoConsulta) {
        rodar_com(Consulta::nova(filtro).unwrap(), dados, bloco)
    }

    fn rodar_com(
        c: Consulta,
        dados: &[u8],
        bloco: usize,
    ) -> (Vec<(usize, String)>, ResumoConsulta) {
        let c = c.com_bloco(bloco);
        let mut v = Vec::new();
        let r = c.executar_jsonl(dados, &AtomicBool::new(false), |l| {
            v.extend(l.resultados.into_iter().map(|x| (x.linha, x.json)))
        });
        assert!(!r.cancelado);
        assert_eq!(r.linhas, r.total_linhas);
        assert_eq!(r.resultados, v.len() as u64);
        (v, r)
    }

    fn jsons(filtro: &str, dados: &[u8]) -> Vec<String> {
        rodar(filtro, dados, 64)
            .0
            .into_iter()
            .map(|x| x.1)
            .collect()
    }

    const PESSOAS: &[u8] = b"{\"nome\":\"Ana Exemplo\",\"n\":1,\"tags\":[1,2]}\n\
        {\"nome\":\"Bruno Teste\",\"n\":2,\"tags\":[]}\n\
        {\"z\":0,\"nome\":\"Carla Ficticia\",\"n\":3,\"tags\":[5]}\n";

    #[test]
    fn filtros_basicos() {
        assert_eq!(
            jsons(".nome", PESSOAS),
            ["\"Ana Exemplo\"", "\"Bruno Teste\"", "\"Carla Ficticia\""]
        );
        assert_eq!(jsons("select(.n > 1) | .n", PESSOAS), ["2", "3"]);
        assert_eq!(
            jsons(".tags | map(. * 2)", PESSOAS),
            ["[2,4]", "[]", "[10]"]
        );
        assert_eq!(jsons(".tags | length", PESSOAS), ["2", "0", "1"]);
        // `keys` ordena; `keys_unsorted` e `.` mantêm a ordem do arquivo.
        assert_eq!(
            jsons("keys", PESSOAS).last().unwrap(),
            "[\"n\",\"nome\",\"tags\",\"z\"]"
        );
        assert_eq!(
            jsons("keys_unsorted", PESSOAS).last().unwrap(),
            "[\"z\",\"nome\",\"n\",\"tags\"]"
        );
        assert_eq!(jsons(".tags[]", PESSOAS), ["1", "2", "5"]);
        let (v, _) = rodar(".tags[]", PESSOAS, 1);
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), [0, 0, 2]);
        assert_eq!(jsons("empty", PESSOAS), Vec::<String>::new());
    }

    #[test]
    fn filtro_invalido_com_posicao() {
        let e = Consulta::nova(".a |").err().unwrap();
        assert_eq!(e.falhas.len(), 1);
        assert_eq!((e.falhas[0].linha, e.falhas[0].coluna), (1, 5));
        assert!(e.to_string().starts_with("filtro inválido"), "{e}");

        let e = Consulta::nova(".a |\n  foo(1)").err().unwrap();
        assert_eq!((e.falhas[0].linha, e.falhas[0].coluna), (2, 3));
        assert_eq!(e.falhas[0].mensagem, "filtro `foo/1` não existe");
        assert_eq!(e.falhas[0].faixa, 7..10);

        let e = Consulta::nova("[1, (2").err().unwrap();
        assert!(e.to_string().contains("fechar"), "{e}");
        assert!(Consulta::nova("$nada").is_err());
        assert!(Consulta::nova("\"\\(1").is_err());
        assert!(Consulta::nova("").is_err());
        // Fundo demais não aborta: ou compila (pilha grande) ou cai no teto de tamanho.
        let n = MAX_FILTRO / 2 - 1;
        let fundo = format!("{}.{}", "(".repeat(n), ")".repeat(n));
        assert!(Consulta::nova(&fundo).is_ok());
        let fundo = format!("{}1", "-".repeat(MAX_FILTRO - 1));
        let c = Consulta::nova(&fundo).unwrap();
        assert_eq!(rodar_com(c, b"null", 64).0, [(0, "-1".to_string())]);
        let e = Consulta::nova(&"-".repeat(100_000)).err().unwrap();
        assert_eq!(e.falhas[0].coluna, MAX_FILTRO + 1);
        assert!(e.falhas[0].mensagem.contains("KiB"));
    }

    #[test]
    fn linha_invalida_nao_derruba() {
        let dados = b"{\"a\":1}\nisso nao e json\n\n{\"a\":\"x\"}\n{\"a\":3}\n[1,\n";
        let (v, r) = rodar(".a + 1", dados, 4);
        assert_eq!(v, [(0, "2".into()), (4, "4".into())]);
        assert_eq!(r.total_linhas, 6);
        assert_eq!(r.erros, 3);
        let linhas: Vec<_> = r.primeiros_erros.iter().map(|e| e.linha).collect();
        assert_eq!(linhas, [1, 3, 5]);
        assert!(r.primeiros_erros[0].motivo.starts_with("JSON inválido"));
        assert!(
            r.primeiros_erros[1].motivo.contains("cannot"),
            "{:?}",
            r.primeiros_erros[1]
        );
    }

    #[test]
    fn erros_guardados_tem_teto() {
        let dados = "x\n".repeat(MAX_ERROS_GUARDADOS * 3);
        let (_, r) = rodar(".", dados.as_bytes(), 16);
        assert_eq!(r.erros, (MAX_ERROS_GUARDADOS * 3) as u64);
        assert_eq!(r.primeiros_erros.len(), MAX_ERROS_GUARDADOS);
        assert!(
            r.primeiros_erros
                .windows(2)
                .all(|w| w[0].linha < w[1].linha)
        );
        assert_eq!(r.primeiros_erros[0].linha, 0);
    }

    #[test]
    fn ordem_estavel_em_qualquer_bloco() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 300_000, 4).unwrap();
        let filtro = "select(.tipo == \"pedido\") | .id, (.itens? // [] | length)";
        let (esperado, r) = rodar(filtro, &dados, usize::MAX);
        assert!(esperado.len() > 100);
        assert_eq!(r.total_linhas, crate::IndiceLinhas::construir(&dados).len());
        for bloco in [1, 100, 4096] {
            for _ in 0..3 {
                assert_eq!(rodar(filtro, &dados, bloco).0, esperado);
            }
        }
    }

    #[test]
    fn lote_respeita_teto_e_parte_no_meio_da_linha() {
        let dados = b"5\n3\n";
        let c = Consulta::nova("range(.)").unwrap().com_max_lote(2);
        let mut lotes = Vec::new();
        c.executar_jsonl(dados, &AtomicBool::new(false), |l| {
            lotes.push((l.resultados.len(), l.linhas));
        });
        assert!(lotes.iter().all(|l| l.0 <= 2));
        assert_eq!(lotes.iter().map(|l| l.0).sum::<usize>(), 8);
        assert!(lotes.windows(2).all(|w| w[0].1 <= w[1].1));
        assert_eq!(lotes.last().unwrap().1, 2);
    }

    #[test]
    fn cancelar_filtro_que_explode() {
        // Uma linha, `range(1e12)`: só para se o cancelamento for visto entre resultados.
        let c = Consulta::nova("range(1e12)").unwrap().com_max_lote(1000);
        let cancelar = AtomicBool::new(false);
        let mut lotes = 0;
        let r = c.executar_jsonl(b"null\n", &cancelar, |l| {
            assert_eq!(l.resultados.len(), 1000);
            lotes += 1;
            if lotes == 3 {
                cancelar.store(true, Relaxed);
            }
        });
        assert!(r.cancelado && !r.tempo_esgotado);
        assert_eq!(lotes, 3);
        assert_eq!(r.resultados, 3000);
        assert_eq!(r.linhas, 0);
    }

    #[test]
    fn cancelar_em_arquivo_grande() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 2 << 20, 5).unwrap();
        let c = Consulta::nova(".id").unwrap().com_bloco(4096);
        let cancelar = AtomicBool::new(false);
        let (mut lotes, mut ultimo) = (0, 0);
        let r = c.executar_jsonl(&dados, &cancelar, |l| {
            assert!(l.linhas >= ultimo);
            ultimo = l.linhas;
            lotes += 1;
            if lotes == 3 {
                cancelar.store(true, Relaxed);
            }
        });
        assert!(r.cancelado);
        assert_eq!(lotes, 3);
        assert!(r.linhas < r.total_linhas);
        assert_eq!(r.linhas, ultimo);
    }

    #[test]
    fn teto_de_saida_em_resultados() {
        // 3 linhas × 100 resultados, teto 150: entrega exatamente 150 e para sem "cancelado".
        let c = Consulta::nova("range(100)")
            .unwrap()
            .com_bloco(4)
            .com_max_lote(7)
            .com_teto_saida(Some(150), None);
        let mut v = 0;
        let r = c.executar_jsonl(b"1\n2\n3\n", &AtomicBool::new(false), |l| {
            v += l.resultados.len()
        });
        assert!(r.cortado && !r.cancelado && !r.tempo_esgotado, "{r:?}");
        assert_eq!((v, r.resultados), (150, 150));
        // No teto exato não corta.
        let c = Consulta::nova("range(100)")
            .unwrap()
            .com_teto_saida(Some(300), None);
        let r = c.executar_jsonl(b"1\n2\n3\n", &AtomicBool::new(false), |_| {});
        assert!(!r.cortado && r.resultados == 300, "{r:?}");
    }

    #[test]
    fn teto_de_saida_em_bytes() {
        // Cada resultado é `"xxxxxxxx"` + `\n` = 11 bytes; teto 100 cabe 9.
        let c = Consulta::nova("range(50) | \"xxxxxxxx\"")
            .unwrap()
            .com_max_lote(4)
            .com_teto_saida(None, Some(100));
        let mut bytes = 0;
        let r = c.executar_documento(b"null", &AtomicBool::new(false), |l| {
            bytes += l.resultados.iter().map(|r| r.json.len() + 1).sum::<usize>()
        });
        assert!(r.cortado && !r.cancelado, "{r:?}");
        assert_eq!((r.resultados, bytes), (9, 99));
    }

    #[test]
    fn teto_de_saida_para_filtro_que_explode() {
        // Sem teto, `range(1e12)` numa linha só pararia por cancelamento.
        let c = Consulta::nova("range(1e12)")
            .unwrap()
            .com_teto_saida(Some(25_000), None);
        let r = c.executar_jsonl(b"null\n", &AtomicBool::new(false), |_| {});
        assert!(r.cortado && r.resultados == 25_000, "{r:?}");
    }

    #[test]
    fn lote_tem_teto_de_bytes() {
        // Um registro com 200 resultados de ~1 KB: sem o teto de bytes, um lote só (max_lote alto).
        let linha = format!("{{\"s\":\"{}\"}}\n", "a".repeat(1000));
        let c = Consulta::nova(".s as $s | range(200) | $s")
            .unwrap()
            .com_max_bytes_lote(4096);
        let (mut lotes, mut total) = (0, 0);
        c.executar_jsonl(linha.as_bytes(), &AtomicBool::new(false), |l| {
            let b: usize = l.resultados.iter().map(|r| r.json.len()).sum();
            assert!(b < 4096 + 1002, "lote de {b} bytes");
            lotes += usize::from(!l.resultados.is_empty());
            total += l.resultados.len();
        });
        assert_eq!(total, 200);
        assert!(lotes >= 40, "{lotes} lotes");
    }

    #[test]
    fn teto_por_registro_em_resultados() {
        // `repeat` e `range` enormes viram erro da linha; as outras linhas seguem. O teto de
        // saída só garante que, sem o teto por registro, o teste falhe em vez de girar para sempre.
        for filtro in ["range(1e12)", "repeat(1)", "1 | recurse(. + 1)"] {
            let c = Consulta::nova(filtro)
                .unwrap()
                .com_teto_saida(Some(1000), None)
                .com_teto_registro(Some(10), None);
            let (v, r) = rodar_com(c, b"1\n2\n", 64);
            assert_eq!(v.len(), 20, "{filtro}");
            assert_eq!(r.erros, 2, "{filtro}");
            assert!(
                r.primeiros_erros[0].motivo.contains("10 resultados"),
                "{r:?}"
            );
        }
        // No teto exato, sem erro.
        let c = Consulta::nova("range(10)")
            .unwrap()
            .com_teto_registro(Some(10), None);
        assert_eq!(rodar_com(c, b"1\n", 64).1.erros, 0);
    }

    #[test]
    fn teto_por_registro_em_tempo() {
        let c = Consulta::nova("range(1e12)")
            .unwrap()
            .com_teto_registro(None, Some(Duration::from_millis(30)))
            // Rede de segurança: sem o teto por registro, falha em vez de girar para sempre.
            .com_teto_saida(None, Some(256 << 20))
            .com_tempo_max(Some(Duration::from_secs(10)));
        let t = Instant::now();
        let (_, r) = rodar_com(c, b"1\n2\n", 64);
        assert!(t.elapsed() < Duration::from_secs(20));
        assert_eq!(r.erros, 2);
        assert!(r.primeiros_erros[0].motivo.contains("de consulta"), "{r:?}");
    }

    #[test]
    fn teto_de_tempo() {
        let c = Consulta::nova("range(1e12)")
            .unwrap()
            .com_tempo_max(Some(Duration::from_millis(50)));
        let r = c.executar_jsonl(b"null\n", &AtomicBool::new(false), |_| {});
        assert!(r.cancelado && r.tempo_esgotado);
    }

    #[test]
    fn fidelidade_de_numero() {
        let dados = b"{\"g\":12345678901234567890123,\"d\":0.1000000000000000000000001,\"m\":9007199254740993}\n";
        assert_eq!(jsons(".g", dados), ["12345678901234567890123"]);
        assert_eq!(jsons(".m + 1", dados), ["9007199254740994"]);
        assert_eq!(jsons(".g * 10", dados), ["123456789012345678901230"]);
        assert_eq!(jsons(".d", dados), ["0.1000000000000000000000001"]);
        // Limite conhecido: decimal em conta vira f64.
        assert_eq!(jsons(".d * 1", dados), ["0.1"]);
        assert_eq!(jsons(".m / 1", dados), ["9007199254740992.0"]);
        assert_eq!(jsons("1e400", b"null"), ["1e400"]);
        // NaN e infinito saem como no jq: JSON válido.
        assert_eq!(
            jsons("[nan, infinite, -infinite]", b"null"),
            ["[null,1.7976931348623157e308,-1.7976931348623157e308]"]
        );
        // Chave duplicada: fica a última, na posição da primeira.
        assert_eq!(
            jsons(".", b"{\"a\":1,\"b\":2,\"a\":3}"),
            ["{\"a\":3,\"b\":2}"]
        );
    }

    #[test]
    fn documento_unico() {
        let doc = b"\xEF\xBB\xBF{\n  \"itens\": [\n    {\"v\": 1},\n    {\"v\": 2}\n  ]\n}\n";
        let c = Consulta::nova(".itens[].v").unwrap();
        let mut v = Vec::new();
        let r = c.executar_documento(doc, &AtomicBool::new(false), |l| v.extend(l.resultados));
        assert_eq!(
            v,
            [
                Resultado {
                    linha: 0,
                    json: "1".into()
                },
                Resultado {
                    linha: 0,
                    json: "2".into()
                }
            ]
        );
        assert_eq!((r.linhas, r.total_linhas, r.erros), (1, 1, 0));

        let r = c.executar_documento(b"{\"a\":", &AtomicBool::new(false), |_| {});
        assert_eq!(r.erros, 1);
        let r = c.executar_documento(b"", &AtomicBool::new(false), |_| {});
        assert_eq!((r.total_linhas, r.resultados, r.erros), (0, 0, 0));
    }

    #[test]
    fn aninhamento_fundo_vira_erro_de_linha() {
        let fundo = format!("{}{}", "[".repeat(200_000), "]".repeat(200_000));
        let ok = format!(
            "{}{}",
            "[".repeat(MAX_ANINHAMENTO),
            "]".repeat(MAX_ANINHAMENTO)
        );
        let dados = format!("{fundo}\n{ok}\n\"[[[[\"\n");
        let (v, r) = rodar("[..] | length", dados.as_bytes(), 1 << 20);
        assert_eq!(v, [(1, MAX_ANINHAMENTO.to_string()), (2, "1".into())]);
        assert_eq!(r.erros, 1);
        assert!(r.primeiros_erros[0].motivo.starts_with("aninhamento"));
    }

    #[test]
    fn crlf_bom_e_linhas_vazias() {
        let dados = b"\xEF\xBB\xBF{\"a\":1}\r\n\r\n  \n{\"a\":2}\r\n{\"a\":3}";
        let (v, r) = rodar(".a", dados, 3);
        assert_eq!(v, [(0, "1".into()), (3, "2".into()), (4, "3".into())]);
        assert_eq!((r.total_linhas, r.erros), (5, 0));
        let (v, r) = rodar(".", b"", 3);
        assert!(v.is_empty() && r.total_linhas == 0);
    }
}
