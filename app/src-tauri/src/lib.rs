//! Casca do app: guarda o documento aberto e entrega janelas da árvore à interface.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod abertura;
mod exemplos;
mod recentes;

use nucleo::arvore::Linha;
use nucleo::consulta::{Consulta, LoteConsulta};
use nucleo::regua::{Baldes, Legenda};
use nucleo::resumo::{self, Conteudo, Progresso};
use nucleo::{Achado, ArquivoLinhas, Arvore, Busca};
use serde::Serialize;
use tauri::{AppHandle, Builder, Emitter, Manager, Runtime, State};

/// Teto de linhas por pedido: a interface pede janelas, nunca o arquivo.
const MAX_JANELA: usize = 1_000;
/// Teto de baldes da régua (mais que isso não cabe em pixels).
const MAX_BALDES: usize = 4_096;
/// Achados guardados para navegar; acima disso a busca só conta (~24 MB de achados).
// shortcut: navegação para no milionésimo achado; guardar em disco se pedirem mais.
const MAX_ACHADOS: usize = 1_000_000;
/// Teto de achados por pedido de `achados` (uma janela da tela).
const MAX_ACHADOS_JANELA: usize = 10_000;
/// Intervalo mínimo entre avisos de progresso de busca e consulta.
const INTERVALO: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Estado {
    arvore: Mutex<Option<Arvore>>,
    varredura: Mutex<Varredura>,
    busca: Mutex<EstadoBusca>,
    consulta: Mutex<EstadoConsulta>,
}

/// Consulta jq em curso ou mostrada (ver `consultar`).
#[derive(Default)]
struct EstadoConsulta {
    /// Sobe a cada `consultar` e `fechar_consulta`: evento de consulta anterior é descartado.
    geracao: u64,
    cancelar: Arc<AtomicBool>,
    /// JSONL temporário com o resultado da última consulta.
    arquivo: Option<PathBuf>,
}

impl EstadoConsulta {
    /// Cancela a consulta em curso e apaga o resultado anterior. Arquivo apagado continua legível
    /// enquanto a árvore o tiver mapeado (Unix).
    // shortcut: no Windows o arquivo mapeado não apaga e fica no temp; limpar ao abrir outro.
    fn reiniciar(&mut self) -> (u64, Arc<AtomicBool>) {
        self.cancelar.store(true, Relaxed);
        self.cancelar = Arc::new(AtomicBool::new(false));
        self.geracao += 1;
        if let Some(a) = self.arquivo.take() {
            let _ = std::fs::remove_file(a);
        }
        (self.geracao, Arc::clone(&self.cancelar))
    }
}

/// Evento `consulta`: progresso e, no fim, o arquivo com o resultado da consulta `geracao`.
#[derive(Clone, Serialize)]
struct AndamentoConsulta {
    geracao: u64,
    resultados: u64,
    linhas: usize,
    total: usize,
    erros: u64,
    /// Primeiro erro de linha (linha a partir de 0, motivo), no fim.
    primeiro_erro: Option<(usize, String)>,
    fim: bool,
    cancelado: bool,
    /// JSONL com o resultado, no fim de uma consulta que não foi cancelada nem falhou.
    arquivo: Option<String>,
    /// Falha ao gravar o resultado.
    falha: Option<String>,
}

/// Erro de `consultar`: filtro que não compila (com as falhas) ou arquivo que não abre.
#[derive(Debug, Serialize)]
struct ErroConsulta {
    mensagem: String,
    falhas: Vec<FalhaFiltro>,
}

#[derive(Debug, Serialize)]
struct FalhaFiltro {
    inicio: usize,
    fim: usize,
    linha: usize,
    coluna: usize,
    mensagem: String,
}

impl From<String> for ErroConsulta {
    fn from(mensagem: String) -> Self {
        Self {
            mensagem,
            falhas: Vec::new(),
        }
    }
}

/// Busca no arquivo aberto (roda em 2º plano, ver `buscar`).
#[derive(Default)]
struct EstadoBusca {
    /// Sobe a cada `buscar` e a cada `abrir`: achado de busca anterior é descartado.
    geracao: u64,
    cancelar: Arc<AtomicBool>,
    /// Os primeiros `MAX_ACHADOS`, em ordem de arquivo.
    achados: Vec<Achado>,
}

impl EstadoBusca {
    /// Cancela a busca em curso e começa outra geração, sem achados.
    fn reiniciar(&mut self) -> (u64, Arc<AtomicBool>) {
        self.cancelar.store(true, Relaxed);
        self.cancelar = Arc::new(AtomicBool::new(false));
        self.geracao += 1;
        self.achados = Vec::new();
        (self.geracao, Arc::clone(&self.cancelar))
    }
}

/// Evento `busca`: progresso e contagem da busca `geracao`.
#[derive(Clone, Serialize)]
struct AndamentoBusca {
    geracao: u64,
    /// Achados até aqui (todos, inclusive os acima de `MAX_ACHADOS`).
    achados: u64,
    /// Quantos dá para navegar (`achado(i)` com `i < guardados`).
    guardados: usize,
    varridos: usize,
    total: usize,
    fim: bool,
}

/// Varredura de linhas inválidas do arquivo aberto (roda em 2º plano, ver `abrir`).
#[derive(Default)]
struct Varredura {
    /// Sobe a cada `abrir`: progresso de um arquivo anterior é descartado.
    geracao: u64,
    cancelar: Arc<AtomicBool>,
    conteudo: Option<Conteudo>,
    progresso: Option<Progresso>,
    /// Chega depois do fim da varredura (ver `resumo::campos`).
    campos: Option<usize>,
    /// Uma marca por registro (ver `nucleo::regua`): inválidas na 1ª passada, cor na 2ª.
    marcas: Arc<[AtomicU8]>,
    legenda: Option<Legenda>,
    /// A 2ª passada acabou (com ou sem discriminador): a régua já tem as cores finais.
    coloriu: bool,
}

/// Evento `varredura` e parte do `resumo`.
#[derive(Clone, Serialize)]
struct Andamento {
    geracao: u64,
    #[serde(flatten)]
    progresso: Progresso,
    campos: Option<usize>,
    legenda: Option<Legenda>,
    coloriu: bool,
}

impl Varredura {
    fn andamento(&self) -> Option<Andamento> {
        Some(Andamento {
            geracao: self.geracao,
            progresso: self.progresso?,
            campos: self.campos,
            legenda: self.legenda.clone(),
            coloriu: self.coloriu,
        })
    }
}

impl Estado {
    fn com<T>(&self, f: impl FnOnce(&mut Arvore) -> T) -> Result<T, String> {
        let mut guarda = self.arvore.lock().map_err(|e| e.to_string())?;
        Ok(f(guarda.as_mut().ok_or("nenhum arquivo aberto")?))
    }
}

/// Onde mora `recentes.json`. Só o `run` gerencia: nos testes de IPC não há recentes em disco.
struct Recentes(PathBuf);

fn com_recentes<R: Runtime, T>(app: &AppHandle<R>, f: impl FnOnce(&Path) -> T) -> Option<T> {
    app.try_state::<Recentes>().map(|r| f(&r.0))
}

/// Põe um arquivo aberto no topo dos recentes. Falha ao gravar não impede a abertura.
fn lembrar<R: Runtime>(app: &AppHandle<R>, caminho: &str) {
    if let Some(Err(e)) = com_recentes(app, |a| recentes::adicionar(a, caminho)) {
        eprintln!("recentes: {e}");
    }
}

/// Lista dos recentes, mais novo primeiro, com nome e pasta.
#[tauri::command]
fn recentes<R: Runtime>(app: AppHandle<R>) -> Vec<recentes::Recente> {
    recentes::descrever(com_recentes(&app, recentes::ler).unwrap_or_default())
}

#[tauri::command]
fn limpar_recentes<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    com_recentes(&app, recentes::limpar)
        .unwrap_or(Ok(()))
        .map_err(|e| e.to_string())
}

/// Se o recente não existe mais: tira da lista e devolve `true` (a interface avisa).
#[tauri::command]
fn recente_ausente<R: Runtime>(caminho: String, app: AppHandle<R>) -> Result<bool, String> {
    if Path::new(&caminho).exists() {
        return Ok(false);
    }
    com_recentes(&app, |a| recentes::remover(a, &caminho))
        .transpose()
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Abre o arquivo, constrói o índice e devolve o número de registros (= linhas visíveis).
/// `async` para rodar fora da thread principal: o índice de 1 GB leva ~150 ms.
/// Dispara a varredura de inválidas numa thread própria, que emite `varredura` com o progresso;
/// a do arquivo anterior é cancelada.
// shortcut: aceita qualquer caminho vindo da interface (só leitura); passar a concessões por
// diálogo/arraste como no Leve quando a interface carregar conteúdo de fora.
/// Arquivo do disco ou exemplo embutido (`exemplo:…`), pelo mesmo caminho para `abrir` e `consultar`.
fn abrir_linhas(caminho: &str) -> Result<ArquivoLinhas, String> {
    match exemplos::embutido(caminho) {
        Some(bytes) => Ok(ArquivoLinhas::estatico(bytes)),
        None => ArquivoLinhas::abrir(caminho).map_err(|e| format!("{caminho}: {e}")),
    }
}

#[tauri::command]
async fn abrir<R: Runtime>(
    caminho: String,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<usize, String> {
    let arvore = Arvore::new(abrir_linhas(&caminho)?);
    let total = arvore.total();
    let arquivo = arvore.compartilhado();
    let conteudo = resumo::conteudo(&arquivo);
    let linhas = arquivo.linhas();
    // JSON em várias linhas: "linhas inválidas" não faz sentido (ver `Conteudo`).
    let pular = conteudo == Conteudo::JsonVariasLinhas;
    // Sem régua para um JSON só (decisão 2 do micronavegador): nem 2ª passada.
    let colorir = !pular && conteudo != Conteudo::JsonUnico;
    let cancelar = Arc::new(AtomicBool::new(false));
    let marcas: Arc<[AtomicU8]> = nucleo::regua::marcas(linhas).into();
    let geracao = {
        let mut v = estado.varredura.lock().map_err(|e| e.to_string())?;
        v.cancelar.store(true, Relaxed);
        v.geracao += 1;
        v.cancelar = Arc::clone(&cancelar);
        v.conteudo = Some(conteudo);
        v.progresso = Some(Progresso {
            varridas: if pular { linhas } else { 0 },
            total: linhas,
            invalidas: 0,
            fim: pular,
            cancelado: false,
        });
        v.campos = None;
        v.marcas = Arc::clone(&marcas);
        v.legenda = None;
        v.coloriu = false;
        v.geracao
    };
    estado.busca.lock().map_err(|e| e.to_string())?.reiniciar();
    *estado.arvore.lock().map_err(|e| e.to_string())? = Some(arvore);
    // Recentes: só arquivo do usuário, nem exemplo embutido nem resultado de consulta jq.
    let consulta = estado
        .consulta
        .lock()
        .map_err(|e| e.to_string())?
        .arquivo
        .clone();
    if exemplos::embutido(&caminho).is_none() && consulta.is_none_or(|a| a != Path::new(&caminho)) {
        lembrar(&app, &caminho);
    }
    std::thread::spawn(move || {
        // Grava no estado e emite, se o arquivo ainda for este.
        let avisar = |f: &dyn Fn(&mut Varredura)| {
            let estado = app.state::<Estado>();
            let Ok(mut v) = estado.varredura.lock() else {
                return;
            };
            if v.geracao != geracao {
                return;
            }
            f(&mut v);
            let Some(andamento) = v.andamento() else {
                return;
            };
            drop(v);
            let _ = app.emit("varredura", andamento);
        };
        if !pular {
            resumo::varrer(&arquivo, &cancelar, &marcas, |p| {
                avisar(&|v| v.progresso = Some(p))
            });
        }
        let inf = nucleo::estrutura::inferir(arquivo.bytes(), &cancelar, |_| {});
        if inf.cancelado || cancelar.load(Relaxed) {
            return;
        }
        let campos = resumo::campos(&inf.resumo);
        avisar(&|v| v.campos = campos);
        let ds = inf.resumo.discriminadores();
        let escolhido = nucleo::regua::escolher(&ds).filter(|_| colorir);
        if let Some(d) = escolhido
            && !nucleo::regua::colorir(&arquivo, d, &marcas, &cancelar)
        {
            return;
        }
        let legenda = escolhido.map(nucleo::regua::legenda);
        avisar(&|v| {
            v.legenda = legenda.clone();
            v.coloriu = true;
        });
    });
    Ok(total)
}

#[derive(Serialize)]
struct Resumo {
    conteudo: Conteudo,
    bytes: usize,
    registros: usize,
    #[serde(flatten)]
    andamento: Andamento,
}

/// Números do painel de entrada; o progresso depois chega pelo evento `varredura`.
#[tauri::command]
fn resumo(estado: State<'_, Estado>) -> Result<Resumo, String> {
    let (bytes, registros) = estado.com(|a| (a.arquivo().bytes().len(), a.arquivo().linhas()))?;
    let v = estado.varredura.lock().map_err(|e| e.to_string())?;
    let (Some(conteudo), Some(andamento)) = (v.conteudo, v.andamento()) else {
        return Err("nenhum arquivo aberto".into());
    };
    Ok(Resumo {
        conteudo,
        bytes,
        registros,
        andamento,
    })
}

fn marcas_atuais(estado: &Estado) -> Result<Arc<[AtomicU8]>, String> {
    Ok(Arc::clone(
        &estado.varredura.lock().map_err(|e| e.to_string())?.marcas,
    ))
}

/// A régua do micronavegador em até `k` baldes (ver `nucleo::regua::baldes`).
#[tauri::command]
fn regua(k: usize, estado: State<'_, Estado>) -> Result<Baldes, String> {
    Ok(nucleo::regua::baldes(
        &marcas_atuais(&estado)?,
        k.min(MAX_BALDES),
    ))
}

/// Marca de cada registro `[inicio, inicio + quantidade)`: o ponto de cor ao lado dele.
#[tauri::command]
fn marcas(inicio: usize, quantidade: usize, estado: State<'_, Estado>) -> Result<Vec<u8>, String> {
    Ok(nucleo::regua::janela(
        &marcas_atuais(&estado)?,
        inicio,
        quantidade.min(MAX_JANELA),
    ))
}

/// Linhas visíveis `[inicio, inicio + quantidade)`, prontas para desenhar. Vão como JSON:
/// são poucas e estruturadas (ver docs/decisoes.md).
#[tauri::command]
fn linhas(
    inicio: usize,
    quantidade: usize,
    estado: State<'_, Estado>,
) -> Result<Vec<Linha>, String> {
    estado.com(|a| a.janela(inicio, quantidade.min(MAX_JANELA)))
}

/// Expande ou recolhe a linha visível `v`; devolve o novo total de linhas visíveis.
#[tauri::command]
fn alternar(v: usize, estado: State<'_, Estado>) -> Result<usize, String> {
    estado.com(|a| {
        a.alternar(v);
        a.total()
    })
}

/// Linha visível do pai de `v`.
#[tauri::command]
fn pai(v: usize, estado: State<'_, Estado>) -> Result<Option<usize>, String> {
    estado.com(|a| a.pai(v))
}

/// Linha visível do registro `registro` (base 0).
#[tauri::command]
fn ir_para(registro: usize, estado: State<'_, Estado>) -> Result<Option<usize>, String> {
    estado.com(|a| a.linha_do_registro(registro))
}

/// Linhas cruas para a visão Texto, como bytes (ver `nucleo::texto::janela` e docs/decisoes.md).
#[tauri::command]
fn texto(
    inicio: usize,
    quantidade: usize,
    estado: State<'_, Estado>,
) -> Result<tauri::ipc::Response, String> {
    estado.com(|a| {
        let bytes = nucleo::texto::janela(
            a.arquivo(),
            inicio,
            quantidade.min(MAX_JANELA),
            nucleo::texto::MAX_COLUNAS,
        );
        tauri::ipc::Response::new(bytes)
    })
}

/// Clique no Texto: abre o caminho até o valor no `byte` do registro e devolve a linha visível
/// dele e o novo total, como `[v, total]`.
#[tauri::command]
fn revelar(
    registro: usize,
    byte: usize,
    estado: State<'_, Estado>,
) -> Result<Option<[usize; 2]>, String> {
    estado.com(|a| a.revelar(registro, byte).map(|v| [v, a.total()]))
}

/// Teto do registro formatado: acima disso a resposta travaria a interface.
const MAX_FORMATADO: usize = 16 << 20;

/// O registro com um campo por linha, para o painel "Ver formatado".
#[tauri::command]
fn formatado(registro: usize, estado: State<'_, Estado>) -> Result<String, String> {
    estado.com(|a| {
        let tamanho = a
            .arquivo()
            .janela(registro, 1)
            .first()
            .map_or(0, |l| l.len());
        if tamanho > MAX_FORMATADO {
            return Err(format!(
                "registro de {tamanho} bytes passa do teto de {MAX_FORMATADO}"
            ));
        }
        a.formatado(registro)
            .map_err(|e| format!("registro inválido no byte {}", e.byte))
    })?
}

/// Caminho do nó selecionado nos três formatos que se copiam.
#[derive(Serialize)]
struct Caminhos {
    jq: String,
    jsonpath: String,
    pointer: String,
}

/// Caminho do nó na linha visível `v`, a partir do registro (`None` fora da lista).
#[tauri::command]
fn caminhos(v: usize, estado: State<'_, Estado>) -> Result<Option<Caminhos>, String> {
    estado.com(|a| {
        a.caminho(v).map(|c| Caminhos {
            jq: c.jq(),
            jsonpath: c.jsonpath(),
            pointer: c.json_pointer(),
        })
    })
}

/// Valor da linha visível `v` para copiar (texto JSON decodificado; ver `Arvore::valor`).
#[tauri::command]
fn valor(v: usize, estado: State<'_, Estado>) -> Result<Option<String>, String> {
    let valor = estado.com(|a| a.valor(v))?;
    match valor {
        Some(s) if s.len() > MAX_FORMATADO => Err(format!(
            "valor de {} MB passa do limite de {} MB para copiar",
            s.len() >> 20,
            MAX_FORMATADO >> 20
        )),
        v => Ok(v),
    }
}

/// Roda o filtro jq `filtro` sobre `caminho` em outra thread e grava os resultados (JSON
/// compacto, um por linha) num JSONL temporário, avisado no evento `consulta`. JSON único ou em
/// várias linhas vira um documento; o resto, JSONL linha a linha. Devolve a geração.
#[tauri::command]
fn consultar<R: Runtime>(
    filtro: String,
    caminho: String,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<u64, ErroConsulta> {
    let (geracao, cancelar) = estado
        .consulta
        .lock()
        .map_err(|e| e.to_string())?
        .reiniciar();
    let consulta = Consulta::nova(&filtro).map_err(|e| ErroConsulta {
        mensagem: e.to_string(),
        falhas: e
            .falhas
            .into_iter()
            .map(|f| FalhaFiltro {
                inicio: f.faixa.start,
                fim: f.faixa.end,
                linha: f.linha,
                coluna: f.coluna,
                mensagem: f.mensagem,
            })
            .collect(),
    })?;
    let arquivo = abrir_linhas(&caminho)?;
    let documento = matches!(
        resumo::conteudo(&arquivo),
        Conteudo::JsonUnico | Conteudo::JsonVariasLinhas
    );
    // Nome com os nanossegundos do relógio para não ser previsível; `create_new` (O_EXCL) recusa
    // arquivo ou link que alguém tenha posto ali antes, e 0600 deixa o resultado só para o dono.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let destino = std::env::temp_dir().join(format!(
        "jasao-consulta-{}-{geracao}-{nanos:x}.jsonl",
        std::process::id()
    ));
    let mut abrir = std::fs::OpenOptions::new();
    abrir.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut abrir, 0o600);
    let saida = abrir
        .open(&destino)
        .map_err(|e| format!("{}: {e}", destino.display()))?;
    estado.consulta.lock().map_err(|e| e.to_string())?.arquivo = Some(destino.clone());
    std::thread::spawn(move || {
        let avisar = |a: AndamentoConsulta| {
            let _ = app.emit("consulta", a);
        };
        let mut gravado = 0u64;
        let mut falha: Option<std::io::Error> = None;
        let mut ultimo = Instant::now();
        let mut out = BufWriter::new(saida);
        let ao_lote = |lote: LoteConsulta| {
            for r in &lote.resultados {
                if let Err(e) = writeln!(out, "{}", r.json) {
                    falha.get_or_insert(e);
                    cancelar.store(true, Relaxed);
                    return;
                }
            }
            gravado += lote.resultados.len() as u64;
            if ultimo.elapsed() >= INTERVALO {
                ultimo = Instant::now();
                avisar(AndamentoConsulta {
                    geracao,
                    resultados: gravado,
                    linhas: lote.linhas,
                    total: lote.total_linhas,
                    erros: 0,
                    primeiro_erro: None,
                    fim: false,
                    cancelado: false,
                    arquivo: None,
                    falha: None,
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
        // Consulta mais nova ou fechada: o resultado desta não serve a ninguém.
        let estado = app.state::<Estado>();
        let atual = estado.consulta.lock().is_ok_and(|c| c.geracao == geracao);
        let cancelado = resumo.cancelado;
        if !atual || cancelado || falha.is_some() {
            let _ = std::fs::remove_file(&destino);
        }
        if !atual {
            return;
        }
        avisar(AndamentoConsulta {
            geracao,
            resultados: resumo.resultados,
            linhas: resumo.linhas,
            total: resumo.total_linhas,
            erros: resumo.erros,
            primeiro_erro: resumo
                .primeiros_erros
                .first()
                .map(|e| (e.linha, e.motivo.clone())),
            fim: true,
            cancelado: falha.is_none() && cancelado,
            arquivo: (falha.is_none() && !cancelado)
                .then(|| destino.to_string_lossy().into_owned()),
            falha: falha.map(|e| format!("{}: {e}", destino.display())),
        });
    });
    Ok(geracao)
}

/// Cancela a consulta em curso e apaga o resultado (o arquivo original já foi reaberto).
#[tauri::command]
fn fechar_consulta(estado: State<'_, Estado>) -> Result<(), String> {
    estado
        .consulta
        .lock()
        .map_err(|e| e.to_string())?
        .reiniciar();
    Ok(())
}

/// Busca `padrao` no arquivo inteiro numa thread própria, que emite `busca` com o progresso
/// (no máximo a cada `INTERVALO`, e no fim); a busca anterior é cancelada e seus achados somem.
/// Padrão vazio só limpa. Devolve a geração da busca, que vem em cada evento.
#[tauri::command]
fn buscar<R: Runtime>(
    padrao: String,
    regex: bool,
    ignorar_caixa: bool,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<u64, String> {
    let arquivo = estado.com(|a| a.compartilhado())?;
    let (geracao, cancelar) = estado.busca.lock().map_err(|e| e.to_string())?.reiniciar();
    if padrao.is_empty() {
        return Ok(geracao);
    }
    let busca = Busca::nova(&padrao, regex, ignorar_caixa).map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let mut aviso = Instant::now();
        let avisar = |achados, varridos, total, fim| {
            let estado = app.state::<Estado>();
            let Ok(b) = estado.busca.lock() else { return };
            if b.geracao != geracao {
                return;
            }
            let guardados = b.achados.len();
            drop(b);
            let _ = app.emit(
                "busca",
                AndamentoBusca {
                    geracao,
                    achados,
                    guardados,
                    varridos,
                    total,
                    fim,
                },
            );
        };
        let mut contados = 0;
        let r = busca.executar(arquivo.bytes(), &cancelar, |lote| {
            contados += lote.achados.len() as u64;
            if let Ok(mut b) = app.state::<Estado>().busca.lock()
                && b.geracao == geracao
            {
                let cabem = MAX_ACHADOS - b.achados.len();
                b.achados.extend(lote.achados.into_iter().take(cabem));
            }
            if aviso.elapsed() >= INTERVALO {
                aviso = Instant::now();
                avisar(contados, lote.varridos, lote.total, false);
            }
        });
        if !r.cancelado {
            avisar(r.achados, r.varridos, r.total, true);
        }
    });
    Ok(geracao)
}

/// O achado `i` da busca atual como `[linha, coluna, comprimento]` (bytes na linha).
#[tauri::command]
fn achado(i: usize, estado: State<'_, Estado>) -> Result<Option<[usize; 3]>, String> {
    let b = estado.busca.lock().map_err(|e| e.to_string())?;
    Ok(b.achados.get(i).map(|a| [a.linha, a.coluna, a.comprimento]))
}

/// Achados dos registros `[inicio, inicio + quantidade)`, como em `achado`, mais o índice do
/// primeiro deles: `[indice, [[linha, coluna, comprimento], …]]`. É o realce da tela.
#[tauri::command]
fn achados(
    inicio: usize,
    quantidade: usize,
    estado: State<'_, Estado>,
) -> Result<(usize, Vec<[usize; 3]>), String> {
    let b = estado.busca.lock().map_err(|e| e.to_string())?;
    let fim = inicio.saturating_add(quantidade.min(MAX_JANELA));
    let i = b.achados.partition_point(|a| a.linha < inicio);
    let lista = b.achados[i..]
        .iter()
        .take_while(|a| a.linha < fim)
        .take(MAX_ACHADOS_JANELA)
        .map(|a| [a.linha, a.coluna, a.comprimento])
        .collect();
    Ok((i, lista))
}

/// Baldes da régua (de `k`) que têm achado da busca atual, em ordem.
#[tauri::command]
fn regua_busca(k: usize, estado: State<'_, Estado>) -> Result<Vec<usize>, String> {
    let registros = estado.com(|a| a.registros())?.max(1);
    let k = k.clamp(1, MAX_BALDES);
    let b = estado.busca.lock().map_err(|e| e.to_string())?;
    let mut baldes: Vec<usize> = b.achados.iter().map(|a| a.linha * k / registros).collect();
    baldes.dedup();
    Ok(baldes)
}

/// Estado e comandos; separado de `run` para o teste de IPC usar o mesmo registro.
fn comandos<R: Runtime>(b: Builder<R>) -> Builder<R> {
    b.manage(Estado::default())
        .manage(abertura::Abertura::default())
        .invoke_handler(tauri::generate_handler![
            abertura::arquivos_iniciais,
            abertura::arquivos_pedidos,
            recentes,
            limpar_recentes,
            recente_ausente,
            abrir,
            resumo,
            linhas,
            alternar,
            pai,
            ir_para,
            texto,
            revelar,
            formatado,
            regua,
            marcas,
            buscar,
            achado,
            achados,
            regua_busca,
            caminhos,
            valor,
            consultar,
            fechar_consulta
        ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    // Primeiro plugin, como pede o single-instance: a 2ª instância repassa o argv e sai.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            abertura::pedir(app, abertura::caminhos_argv(&argv, Path::new(&cwd)));
        }));
    }
    comandos(builder)
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Título vem do productName (tauri.conf.json), onde mora o nome do produto.
            let nome = app.package_info().name.clone();
            if let Some(janela) = app.get_webview_window("main") {
                janela.set_title(&nome)?;
            }
            if let Ok(dir) = app.path().app_data_dir() {
                app.manage(Recentes(dir.join("recentes.json")));
            }
            let cwd = std::env::current_dir().unwrap_or_default();
            let argv: Vec<String> = std::env::args().collect();
            let abertura = app.state::<abertura::Abertura>();
            for c in abertura::caminhos_argv(&argv, &cwd) {
                abertura.enfileirar(c);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("falha ao iniciar o app")
        .run(|app, evento| match evento {
            tauri::RunEvent::Ready => app.state::<abertura::Abertura>().pronto(),
            // macOS: Finder, "Abrir com" e `open -a` chegam como Apple Event, um por pedido.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Opened { urls } => {
                let caminhos = urls
                    .iter()
                    .filter_map(|u| u.to_file_path().ok())
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                abertura::pedir(app, caminhos);
            }
            // Não deixa o resultado da última consulta no diretório temporário.
            tauri::RunEvent::Exit => {
                if let Ok(mut c) = app.state::<Estado>().consulta.lock() {
                    c.reiniciar();
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod testes {
    use super::*;
    use serde_json::{Value, json};
    use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::{InvokeRequest, WebviewWindowBuilder};

    /// Os nomes de comando e de argumento são os que `src/main.ts` manda.
    #[test]
    fn comandos_pelo_ipc() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, b"{\"a\":[1,2]}\n{\"b\":tru}\n").unwrap();
        let mut tipos = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(
            &mut tipos,
            b"{\"tipo\":\"x\",\"x\":1}\n{\"tipo\":\"y\",\"y\":1}\n{\"tipo\":\"x\",\"x\":2}\n{\"tipo\":}\n",
        )
        .unwrap();
        let app = comandos(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let janela = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let chamar = |cmd: &str, args: Value| -> Result<Value, Value> {
            let pedido = InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            };
            get_ipc_response(&janela, pedido).map(|b| b.deserialize::<Value>().unwrap())
        };

        assert!(chamar("linhas", json!({"inicio": 0, "quantidade": 10})).is_err());
        let caminho = f.path().to_str().unwrap();
        assert_eq!(chamar("abrir", json!({ "caminho": caminho })), Ok(json!(2)));
        // A varredura roda em outra thread: espera o fim pelo próprio comando.
        let resumo = (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                chamar("resumo", json!({})).unwrap()
            })
            .find(|r| r["coloriu"] == true)
            .expect("varredura não terminou");
        assert_eq!(
            resumo,
            json!({"conteudo": "objetos", "bytes": 22, "registros": 2, "geracao": 1,
                   "varridas": 2, "total": 2, "invalidas": 1, "fim": true, "cancelado": false,
                   "campos": 1, "legenda": null, "coloriu": true})
        );
        assert_eq!(
            chamar("marcas", json!({"inicio": 0, "quantidade": 9})),
            Ok(json!([0, 255]))
        );
        assert_eq!(chamar("alternar", json!({"v": 0})), Ok(json!(3)));
        let linhas = chamar("linhas", json!({"inicio": 0, "quantidade": 10})).unwrap();
        assert_eq!(linhas[1]["caminho"], ".a");
        assert_eq!(linhas[2]["erro"], json!({"byte": 8, "motivo": "literal"}));
        assert_eq!(chamar("pai", json!({"v": 1})), Ok(json!(0)));
        assert_eq!(chamar("ir_para", json!({"registro": 1})), Ok(json!(2)));
        assert_eq!(chamar("ir_para", json!({"registro": 9})), Ok(Value::Null));

        // Texto volta cru (ArrayBuffer na interface), não JSON.
        let pedido = |cmd: &str, args: Value| InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        let texto = get_ipc_response(
            &janela,
            pedido("texto", json!({"inicio": 0, "quantidade": 9})),
        );
        match texto {
            Ok(InvokeResponseBody::Raw(b)) => {
                assert_eq!(b, b"11 9\n{\"a\":[1,2]}\n{\"b\":tru}")
            }
            outro => panic!("esperava bytes, veio {outro:?}"),
        }
        assert_eq!(chamar("alternar", json!({"v": 0})), Ok(json!(2)));
        assert_eq!(
            chamar("revelar", json!({"registro": 0, "byte": 7})),
            Ok(json!([3, 5]))
        );
        assert_eq!(
            chamar("revelar", json!({"registro": 9, "byte": 0})),
            Ok(Value::Null)
        );
        assert_eq!(
            chamar("formatado", json!({"registro": 0})),
            Ok(json!("{\n  \"a\": [\n    1,\n    2\n  ]\n}"))
        );
        assert!(chamar("formatado", json!({"registro": 1})).is_err());

        // Régua: discriminador `.tipo`, cor por frequência, inválida marcada.
        let caminho = tipos.path().to_str().unwrap();
        assert_eq!(chamar("abrir", json!({ "caminho": caminho })), Ok(json!(4)));
        let resumo = (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                chamar("resumo", json!({})).unwrap()
            })
            .find(|r| r["coloriu"] == true)
            .expect("2ª passada não terminou");
        assert_eq!(
            resumo["legenda"],
            json!({"campo": ".tipo", "variantes": [["x", 2], ["y", 1]], "outros": 0})
        );
        assert_eq!(
            chamar("marcas", json!({"inicio": 0, "quantidade": 9})),
            Ok(json!([1, 2, 1, 255]))
        );
        assert_eq!(
            chamar("regua", json!({"k": 2})),
            Ok(json!({"cores": [1, 1], "invalidas": [[1, 1, 3]]}))
        );

        // Copiar: caminho nos três formatos e valor (texto decodificado).
        assert_eq!(chamar("alternar", json!({"v": 0})), Ok(json!(6)));
        assert_eq!(
            chamar("caminhos", json!({"v": 2})),
            Ok(json!({"jq": ".x", "jsonpath": "$.x", "pointer": "/x"}))
        );
        assert_eq!(chamar("valor", json!({"v": 1})), Ok(json!("x")));
        assert_eq!(chamar("valor", json!({"v": 2})), Ok(json!("1")));
        assert_eq!(chamar("caminhos", json!({"v": 9})), Ok(Value::Null));

        // Busca: roda em outra thread; os achados aparecem no estado.
        let buscar = |padrao: &str, regex: bool| {
            chamar(
                "buscar",
                json!({"padrao": padrao, "regex": regex, "ignorarCaixa": true}),
            )
        };
        assert!(buscar("(", true).is_err());
        assert_eq!(buscar("X", false), Ok(json!(4)));
        let achados = (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                chamar("achados", json!({"inicio": 0, "quantidade": 9})).unwrap()
            })
            .find(|a| a[1].as_array().unwrap().len() == 4)
            .expect("busca não terminou");
        assert_eq!(
            achados,
            json!([0, [[0, 9, 1], [0, 13, 1], [2, 9, 1], [2, 13, 1]]])
        );
        assert_eq!(chamar("achado", json!({"i": 2})), Ok(json!([2, 9, 1])));
        assert_eq!(chamar("achado", json!({"i": 4})), Ok(Value::Null));
        assert_eq!(
            chamar("achados", json!({"inicio": 1, "quantidade": 5})),
            Ok(json!([2, [[2, 9, 1], [2, 13, 1]]]))
        );
        assert_eq!(chamar("regua_busca", json!({"k": 2})), Ok(json!([0, 1])));
        // Termo vazio limpa; abrir outro arquivo também.
        assert_eq!(buscar("", false), Ok(json!(5)));
        assert_eq!(chamar("achado", json!({"i": 0})), Ok(Value::Null));

        // Consulta jq: erro de sintaxe com posição; resultado num JSONL que abre como arquivo.
        use tauri::Listener;
        let (tx, rx) = std::sync::mpsc::channel::<Value>();
        app.listen("consulta", move |e| {
            let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
        });
        let fim = || loop {
            let a = rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("consulta não terminou");
            if a["fim"] == true {
                break a;
            }
        };
        let erro =
            chamar("consultar", json!({"filtro": ".x | (", "caminho": caminho})).unwrap_err();
        assert_eq!(erro["falhas"][0]["linha"], json!(1));
        assert!(erro["falhas"][0]["coluna"].as_u64().unwrap() >= 5, "{erro}");
        assert_eq!(
            chamar("consultar", json!({"filtro": ".x", "caminho": caminho})),
            Ok(json!(2))
        );
        let a = fim();
        assert_eq!(
            (
                &a["geracao"],
                &a["resultados"],
                &a["erros"],
                &a["primeiro_erro"][0],
                &a["cancelado"]
            ),
            (&json!(2), &json!(3), &json!(1), &json!(3), &json!(false))
        );
        let resultado = a["arquivo"].as_str().unwrap().to_owned();
        assert_eq!(std::fs::read_to_string(&resultado).unwrap(), "1\nnull\n2\n");
        assert_eq!(
            chamar("abrir", json!({ "caminho": resultado })),
            Ok(json!(3))
        );
        // Voltar ao original apaga o resultado.
        assert_eq!(chamar("abrir", json!({ "caminho": caminho })), Ok(json!(4)));
        assert_eq!(chamar("fechar_consulta", json!({})), Ok(Value::Null));
        assert!(!std::path::Path::new(&resultado).exists());
        // Exemplo embutido também é consultável.
        assert_eq!(
            chamar(
                "consultar",
                json!({"filtro": ".tipo?", "caminho": "exemplo:vitrine"})
            ),
            Ok(json!(4))
        );
        let a = fim();
        assert!(a["resultados"].as_u64().unwrap() > 1000, "{a}");
        assert_eq!(chamar("fechar_consulta", json!({})), Ok(Value::Null));
        assert!(!std::path::Path::new(a["arquivo"].as_str().unwrap()).exists());
    }

    /// Abrir põe nos recentes; recente apagado sai da lista com `recente_ausente`.
    #[test]
    fn recentes_pelo_ipc() {
        let pasta = tempfile::tempdir().unwrap();
        // Canônica: os recentes guardam o caminho canônico (no macOS, /var é /private/var).
        let raiz = std::fs::canonicalize(pasta.path()).unwrap();
        let arq = raiz.join("a.jsonl");
        std::fs::write(&arq, b"{\"a\":1}\n").unwrap();
        let lista = pasta.path().join("dados").join("recentes.json");
        let app = comandos(mock_builder())
            .manage(Recentes(lista.clone()))
            .build(mock_context(noop_assets()))
            .unwrap();
        let janela = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let chamar = |cmd: &str, args: Value| -> Value {
            let pedido = InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            };
            get_ipc_response(&janela, pedido)
                .unwrap()
                .deserialize::<Value>()
                .unwrap()
        };
        let caminho = arq.to_str().unwrap();
        assert_eq!(chamar("recentes", json!({})), json!([]));
        chamar("abrir", json!({ "caminho": caminho }));
        let pasta_txt = raiz.to_str().unwrap();
        assert_eq!(
            chamar("recentes", json!({})),
            json!([{"caminho": caminho, "nome": "a.jsonl", "pasta": pasta_txt}])
        );
        assert_eq!(
            chamar("recente_ausente", json!({ "caminho": caminho })),
            json!(false)
        );
        std::fs::remove_file(&arq).unwrap();
        assert_eq!(
            chamar("recente_ausente", json!({ "caminho": caminho })),
            json!(true)
        );
        assert_eq!(chamar("recentes", json!({})), json!([]));
        chamar("limpar_recentes", json!({}));
        assert!(!lista.exists());
    }
}
