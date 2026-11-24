//! Casca do app: guarda os documentos abertos (um por aba) e entrega janelas da árvore à
//! interface. Todo comando recebe o `id` do documento; os eventos o devolvem.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Conta o heap: o processo filho da consulta liga o teto (ver `filho`).
#[global_allocator]
static ALOCADOR: nucleo::memoria::Contador = nucleo::memoria::Contador;

mod abertura;
mod exemplos;
mod filho;
mod recentes;

pub use filho::filho as consulta_filho;

use nucleo::arvore::{Formatado, Linha, MAX_FORMATADO};
use nucleo::consulta::Consulta;
use nucleo::filtro::{self, Criterio, FalhaFiltro as FalhaFiltragem};
use nucleo::laudo::Laudo;
use nucleo::regua::{Baldes, Legenda};
use nucleo::resumo::{self, Conteudo, Progresso};
use nucleo::tabela::{self, Coluna, FalhaOrdem, Indisponivel, Ordem, Tabela};
use nucleo::transformar::{self, Fonte, Mascara, Saida, Transformacao};
use nucleo::{Achado, ArquivoLinhas, Arvore, Busca, Opcoes};
use serde::{Deserialize, Serialize};
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

/// Documentos abertos, por id. Cada aba da interface tem o seu.
#[derive(Default)]
struct Estado {
    docs: Mutex<HashMap<u32, Arc<Doc>>>,
    proximo: AtomicU32,
    /// Descompactações em curso (ver `cancelar_abertura`).
    abrindo: Aberturas,
    /// Diff de valores em curso (envolve dois documentos, então fica fora de `Doc`).
    diff: Mutex<EstadoExportar>,
    /// Linha do tempo em curso (uma pasta, nenhum documento aberto).
    linha: Mutex<EstadoExportar>,
}

/// Flags de cancelamento por abertura. A chave é um id por abertura, não o caminho: o mesmo
/// arquivo pode estar abrindo em duas abas. O id vem do backend (`nova_abertura`), que já deixa
/// a flag registrada: o Cancelar que chega antes de o `abrir` começar não se perde, e o id não se
/// repete nesta execução (nem depois de recarregar a página).
#[derive(Default)]
struct Aberturas(Mutex<HashMap<u32, Arc<AtomicBool>>>, AtomicU32);

impl Aberturas {
    fn nova(&self) -> u32 {
        let id = self.1.fetch_add(1, Relaxed) + 1;
        self.registrar(id);
        id
    }
    /// A flag de `id`, a já registrada (talvez já cancelada) ou uma nova.
    fn registrar(&self, id: u32) -> Arc<AtomicBool> {
        match self.0.lock() {
            Ok(mut m) => Arc::clone(m.entry(id).or_default()),
            Err(_) => Arc::default(),
        }
    }
    /// Tira `id` se ainda for a flag `c` (o id pode ter sido reusado).
    fn soltar(&self, id: u32, c: &Arc<AtomicBool>) {
        if let Ok(mut m) = self.0.lock()
            && m.get(&id).is_some_and(|x| Arc::ptr_eq(x, c))
        {
            m.remove(&id);
        }
    }
    fn cancelar(&self, id: u32) {
        if let Some(c) = self.0.lock().ok().as_ref().and_then(|m| m.get(&id)) {
            c.store(true, Relaxed);
        }
    }
}

/// Um arquivo aberto e o que roda sobre ele. As threads de 2º plano guardam só um `Weak`:
/// fechar o documento solta a árvore na hora, e elas param no próximo aviso.
struct Doc {
    arvore: Mutex<Arvore>,
    varredura: Mutex<Varredura>,
    busca: Mutex<EstadoBusca>,
    consulta: Mutex<EstadoConsulta>,
    /// "Salvar como" em curso (ver `salvar_como`).
    salvar: Mutex<EstadoSalvar>,
    exportar: Mutex<EstadoExportar>,
    ordem: Mutex<EstadoOrdem>,
    filtro: Mutex<EstadoFiltro>,
    /// Cancela o perfil em curso quando outro caminho é pedido.
    perfil: Mutex<Arc<AtomicBool>>,
    /// Modo seguir (arquivo vivo): a cópia que cresce; `None` antes do 1º `seguir`. Pausar
    /// não a solta: ao retomar, ela confere o original contra a última leitura.
    vivo: Mutex<Option<nucleo::Vivo>>,
    /// Como estava o arquivo aberto (sem seguir): o 1º `seguir` só acrescenta se ainda é ele.
    marca: Mutex<Option<nucleo::Marca>>,
}

/// Ordenação da Tabela: a pronta (com a direção) e a que roda em 2º plano.
#[derive(Default)]
struct EstadoOrdem {
    /// Sobe a cada pedido: progresso de uma ordenação anterior é descartado.
    geracao: u64,
    cancelar: Arc<AtomicBool>,
    pronta: Option<(Arc<Ordem>, bool)>,
    /// `EstadoFiltro::versao` sobre a qual `pronta` foi feita: com outro filtro ela não vale.
    filtro: u64,
}

impl EstadoOrdem {
    fn reiniciar(&mut self) {
        self.cancelar();
        self.pronta = None;
    }

    /// Para a ordenação em curso. Subir a geração descarta o resultado mesmo que o núcleo já
    /// tenha passado do último ponto em que olha o `cancelar`.
    fn cancelar(&mut self) {
        self.cancelar.store(true, Relaxed);
        self.geracao += 1;
    }

    /// Ordem pronta da ordenação `geracao`; se ela foi cancelada ou substituída, descarta.
    fn publicar(
        &mut self,
        geracao: u64,
        ordem: Ordem,
        desc: bool,
        filtro: u64,
    ) -> Option<FalhaOrdem> {
        if self.geracao != geracao {
            return Some(FalhaOrdem::Cancelada);
        }
        self.pronta = Some((Arc::new(ordem), desc));
        self.filtro = filtro;
        None
    }
}

/// Filtro da Tabela (e da navegação ⌘↓/⌘↑ na Árvore): o pronto e o que roda em 2º plano.
struct EstadoFiltro {
    /// Sobe a cada pedido: progresso e resultado de um filtro anterior são descartados.
    geracao: u64,
    cancelar: Arc<AtomicBool>,
    pronto: Option<Arc<FiltroPronto>>,
    /// Sobe a cada troca de `pronto`: a ordem feita sobre outro filtro não vale (`vista`).
    versao: u64,
    /// `filtro::MAX_FILTRADOS`; o teste de IPC baixa para provar o teto no call site.
    teto: usize,
}

impl Default for EstadoFiltro {
    fn default() -> Self {
        EstadoFiltro {
            geracao: 0,
            cancelar: Arc::default(),
            pronto: None,
            versao: 0,
            teto: filtro::MAX_FILTRADOS,
        }
    }
}

impl EstadoFiltro {
    fn cancelar(&mut self) {
        self.cancelar.store(true, Relaxed);
        self.geracao += 1;
    }

    fn reiniciar(&mut self) {
        self.cancelar();
        self.pronto = None;
        self.versao += 1;
    }
}

struct FiltroPronto {
    registros: Vec<u32>,
    ui: FiltroUi,
}

/// O filtro pronto como a interface o mostra (ela formata e escapa).
#[derive(Clone, Serialize)]
struct FiltroUi {
    achados: usize,
    /// Caminho (`.x.y`) ou campo do discriminador.
    caminho: String,
    /// Filtro por caminho: com (`true`) ou sem.
    tem: Option<bool>,
    /// Filtro por variante: o valor, cru.
    valor: Option<String>,
    /// Filtro por valor dos mais comuns: o token cru (`"a"`, `1.5`, `null`).
    comum: Option<String>,
    /// Filtro por faixa de número: sinal e ordem de grandeza.
    faixa: Option<(bool, Option<i32>)>,
}

/// Evento com o id do documento de onde veio.
#[derive(Clone, Serialize)]
struct DoDoc<T> {
    id: u32,
    #[serde(flatten)]
    dado: T,
}

fn emitir<R: Runtime, T: Serialize + Clone>(app: &AppHandle<R>, evento: &str, id: u32, dado: T) {
    let _ = app.emit(evento, DoDoc { id, dado });
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

/// "Salvar como" em curso: um por documento; outro pedido cancela o anterior.
#[derive(Default)]
struct EstadoSalvar {
    geracao: u64,
    cancelar: Arc<AtomicBool>,
}

impl EstadoSalvar {
    fn reiniciar(&mut self) -> (u64, Arc<AtomicBool>) {
        self.cancelar.store(true, Relaxed);
        self.cancelar = Arc::new(AtomicBool::new(false));
        self.geracao += 1;
        (self.geracao, Arc::clone(&self.cancelar))
    }
}

/// Evento `consulta`: progresso e, no fim, o arquivo com o resultado da consulta `geracao`.
/// Também é o que o processo filho manda ao pai (ver `filho`).
#[derive(Clone, Default, Serialize, Deserialize)]
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
    /// Falha ao gravar o resultado, ou a consulta abortou.
    falha: Option<String>,
    /// A saída passou de `filho::TETO_SAIDA`: o resultado é parcial e a consulta parou.
    cortado: bool,
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
    /// Estrutura inferida (modo Estrutura); chega junto com `campos`.
    estrutura: Option<Arc<Estrutura>>,
    /// A inferência crua e o arquivo dela, para Exportar (gerar e validar); chega com `estrutura`.
    inferencia: Option<InfArquivo>,
    /// Laudo do painel de entrada (`nucleo::laudo`); chega junto com `estrutura`.
    laudo: Option<Arc<Laudo>>,
    /// Modo Tabela (ou por que não há); chega junto com `estrutura`.
    tabela: Option<Result<Arc<Tabela>, Indisponivel>>,
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
    /// A inferência acabou: os comandos `estrutura` e `laudo` já têm resposta.
    inferiu: bool,
    /// A Tabela foi montada (ou recusada): o comando `tabela` já tem resposta. Vem depois de
    /// `inferiu`, porque JSON único lista os elementos numa passada a mais.
    tabelou: bool,
}

impl Varredura {
    fn andamento(&self) -> Option<Andamento> {
        Some(Andamento {
            geracao: self.geracao,
            progresso: self.progresso?,
            campos: self.campos,
            legenda: self.legenda.clone(),
            coloriu: self.coloriu,
            inferiu: self.estrutura.is_some(),
            tabelou: self.tabela.is_some(),
        })
    }
}

impl Estado {
    fn doc(&self, id: u32) -> Result<Arc<Doc>, String> {
        let docs = self.docs.lock().map_err(|e| e.to_string())?;
        docs.get(&id)
            .cloned()
            .ok_or_else(|| format!("documento {id} não está aberto (a aba já foi fechada?)"))
    }

    fn com<T>(&self, id: u32, f: impl FnOnce(&mut Arvore) -> T) -> Result<T, String> {
        self.doc(id)?.com(f)
    }
}

impl Doc {
    fn com<T>(&self, f: impl FnOnce(&mut Arvore) -> T) -> Result<T, String> {
        Ok(f(&mut *self.arvore.lock().map_err(|e| e.to_string())?))
    }

    /// Para tudo o que roda sobre o documento e apaga o resultado de consulta.
    fn encerrar(&self) {
        if let Ok(v) = self.varredura.lock() {
            v.cancelar.store(true, Relaxed);
        }
        if let Ok(mut b) = self.busca.lock() {
            b.reiniciar();
        }
        if let Ok(mut c) = self.consulta.lock() {
            c.reiniciar();
        }
        if let Ok(mut o) = self.ordem.lock() {
            o.reiniciar();
        }
        if let Ok(mut f) = self.filtro.lock() {
            f.reiniciar();
        }
        if let Ok(p) = self.perfil.lock() {
            p.store(true, Relaxed);
        }
    }
}

/// Abre `caminho` (ou o exemplo embutido `exemplo:<nome>`), com erro legível para pasta.
fn abrir_linhas(caminho: &str) -> Result<ArquivoLinhas, String> {
    abrir_linhas_com(caminho, &Opcoes::default())
}

/// Erros da descompactação que a interface traduz (e, na razão, pergunta se abre assim mesmo).
const ERRO_CANCELADA: &str = "jasao:cancelada";
const ERRO_RAZAO: &str = "jasao:razao";
const ERRO_TETO: &str = "jasao:teto";

fn abrir_linhas_com(caminho: &str, op: &Opcoes) -> Result<ArquivoLinhas, String> {
    if let Some(bytes) = exemplos::embutido(caminho) {
        return Ok(ArquivoLinhas::estatico(bytes));
    }
    if Path::new(caminho).is_dir() {
        return Err(format!("{caminho}: é uma pasta, não um arquivo"));
    }
    ArquivoLinhas::abrir_com(caminho, op).map_err(|e| match e.kind() {
        _ if nucleo::compactado::cancelada(&e) => ERRO_CANCELADA.into(),
        _ if nucleo::compactado::razao_estourada(&e) => ERRO_RAZAO.into(),
        std::io::ErrorKind::FileTooLarge => ERRO_TETO.into(),
        _ => format!("{caminho}: {e}"),
    })
}

/// Evento `descompactando`: progresso da abertura `abertura`, em bytes do compactado.
#[derive(Clone, Serialize)]
struct Descompactando {
    abertura: u32,
    lidos: u64,
    total: u64,
}

/// Abre `caminho` descompactando com progresso e cancelável por `cancelar_abertura(abertura)`.
/// `forcar`: sem teto de razão (a interface perguntou); o teto absoluto continua.
fn abrir_cancelavel<R: Runtime>(
    caminho: &str,
    abertura: u32,
    forcar: bool,
    app: &AppHandle<R>,
    estado: &Estado,
) -> Result<ArquivoLinhas, String> {
    let cancelar = estado.abrindo.registrar(abertura);
    let ultimo = std::cell::Cell::new(None::<Instant>);
    let progresso = |lidos: u64, total: u64| {
        if lidos < total && ultimo.get().is_some_and(|t| t.elapsed() < INTERVALO) {
            return;
        }
        ultimo.set(Some(Instant::now()));
        let _ = app.emit(
            "descompactando",
            Descompactando {
                abertura,
                lidos,
                total,
            },
        );
    };
    let mut op = Opcoes {
        cancelar: Some(&cancelar),
        progresso: Some(&progresso),
        ..Opcoes::default()
    };
    if forcar {
        op.max_razao = u64::MAX;
    }
    let r = abrir_linhas_com(caminho, &op);
    estado.abrindo.soltar(abertura, &cancelar);
    r
}

/// Id para a próxima abertura (ver `Aberturas`); a interface o passa ao `abrir`.
#[tauri::command]
fn nova_abertura(estado: State<'_, Estado>) -> u32 {
    estado.abrindo.nova()
}

/// Cancela a abertura `abertura` em curso (o `abrir` dela falha com `jasao:cancelada`).
#[tauri::command]
fn cancelar_abertura(abertura: u32, estado: State<'_, Estado>) {
    estado.abrindo.cancelar(abertura);
}

/// A interface pergunta antes de baixar por `http://` e repete com `http: true`.
const ERRO_HTTP: &str = "jasao:http";

/// Evento `baixando`: bytes recebidos (e o total declarado, ou 0) da abertura `abertura`.
#[derive(Clone, Serialize)]
struct Baixando {
    abertura: u32,
    lidos: u64,
    total: u64,
}

/// Resultado do `baixar_url`: onde ficou e o nome para a aba (sem consulta nem credencial).
#[derive(Serialize)]
struct Baixado {
    caminho: String,
    nome: String,
}

/// Baixa uma URL `https://` (ou `http://`, com `http`) ou um comando cURL colado para o cache e
/// devolve o caminho, já concedido, para o `abrir`. Só por ação explícita do usuário (campo
/// "Abrir URL"); cancelável por `cancelar_abertura(abertura)`. Erros nunca trazem a entrada
/// crua: cabeçalho de autorização, `-u` e consulta da URL saem mascarados pelo núcleo.
#[tauri::command]
async fn baixar_url<R: Runtime>(
    entrada: String,
    http: Option<bool>,
    abertura: Option<u32>,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<Baixado, String> {
    let pedido = nucleo::url::Pedido::interpretar(&entrada)?;
    let abertura = abertura.unwrap_or(0);
    let cancelar = estado.abrindo.registrar(abertura);
    let progresso = |lidos: u64, total: u64| {
        let _ = app.emit(
            "baixando",
            Baixando {
                abertura,
                lidos,
                total,
            },
        );
    };
    let op = nucleo::url::Opcoes {
        permitir_http: http.unwrap_or(false),
        cancelar: Some(&cancelar),
        progresso: Some(&progresso),
        ..nucleo::url::Opcoes::default()
    };
    let r = nucleo::url::baixar(&pedido, &op);
    estado.abrindo.soltar(abertura, &cancelar);
    let caminho = r.map_err(|e| match e.kind() {
        _ if nucleo::compactado::cancelada(&e) => ERRO_CANCELADA.into(),
        _ if e.to_string() == nucleo::url::ERRO_HTTP => ERRO_HTTP.into(),
        _ => e.to_string(),
    })?;
    app.state::<abertura::Concedidos>().conceder(&caminho);
    let caminho = caminho
        .into_os_string()
        .into_string()
        .map_err(|_| NEGADO.to_owned())?;
    Ok(Baixado {
        caminho,
        nome: pedido.nome(),
    })
}

/// Se cada caminho é uma pasta (o arraste avisa em vez de tentar abrir).
#[tauri::command]
fn pastas<R: Runtime>(caminhos: Vec<String>, app: AppHandle<R>) -> Vec<bool> {
    // Só responde pelo concedido. Síncrono de propósito: roda na thread principal, depois do
    // handler do soltar que concede o arraste (ver `run`); a interface chama este antes de abrir.
    caminhos
        .iter()
        .map(|c| autorizado(&app, c).is_ok_and(|c| Path::new(&c).is_dir()))
        .collect()
}

/// Onde mora `recentes.json`. Só o `run` gerencia: nos testes de IPC não há recentes em disco.
struct Recentes(PathBuf);

fn com_recentes<R: Runtime, T>(app: &AppHandle<R>, f: impl FnOnce(&Path) -> T) -> Option<T> {
    app.try_state::<Recentes>().map(|r| f(&r.0))
}

/// Erro de todo caminho recusado: o mesmo para não concedido, inexistente ou ilegível, para a
/// interface não servir de oráculo de existência de arquivo.
const NEGADO: &str = "jasao:negado";

/// O caminho a abrir de fato, se o usuário o concedeu: escolhido no diálogo, solto na janela,
/// pedido pelo sistema (argv, associação, 2ª instância), resultado de consulta ou recente. O
/// recente conta porque `recentes.json` só recebe caminho que já passou por aqui e mora no
/// diretório de dados, fora do alcance da interface. Compara e devolve o canônico (link e `..`
/// resolvidos), e é ele que se abre. Exemplo embutido passa como veio (não é arquivo).
fn autorizado<R: Runtime>(app: &AppHandle<R>, caminho: &str) -> Result<String, String> {
    if exemplos::embutido(caminho).is_some() {
        return Ok(caminho.to_owned());
    }
    let negado = || NEGADO.to_owned();
    let c = std::fs::canonicalize(caminho).map_err(|_| negado())?;
    let recente = || {
        com_recentes(app, recentes::ler)
            .unwrap_or_default()
            .iter()
            .any(|r| Path::new(r) == c)
    };
    if !app.state::<abertura::Concedidos>().tem(&c) && !recente() {
        return Err(negado());
    }
    c.into_os_string().into_string().map_err(|_| negado())
}

/// Põe um arquivo aberto no topo dos recentes. Falha ao gravar não impede a abertura.
fn lembrar<R: Runtime>(app: &AppHandle<R>, caminho: &str) {
    // O temporário de stdin some ao fechar: não vira recente.
    if app.try_state::<Stdin>().is_some_and(|s| {
        s.0.lock().is_ok_and(|t| {
            t.as_ref()
                .is_some_and(|t| t.caminho() == Path::new(caminho))
        })
    }) {
        return;
    }
    if let Some(Err(e)) = com_recentes(app, |a| recentes::adicionar(a, caminho)) {
        eprintln!("recentes: {e}");
    }
    recentes_mudaram(app);
}

/// Refaz o menu Arquivo › Abrir recente e avisa a interface (`recentes`) para redesenhar a lista.
fn recentes_mudaram<R: Runtime>(app: &AppHandle<R>) {
    // O AppKit só aceita menu na thread principal (`abrir` roda num worker do tokio). Fora dos
    // testes: o runtime de teste roda a closure ali mesmo, fora da thread principal.
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let h = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(e) = menu(&h).and_then(|m| h.set_menu(m)) {
                eprintln!("menu: {e}");
            }
        });
    }
    let _ = app.emit("recentes", ());
}

/// Lista dos recentes, mais novo primeiro, com nome e pasta.
#[tauri::command]
fn recentes<R: Runtime>(app: AppHandle<R>) -> Vec<recentes::Recente> {
    recentes::descrever(com_recentes(&app, recentes::ler).unwrap_or_default())
}

#[tauri::command]
fn limpar_recentes<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let r = com_recentes(&app, recentes::limpar)
        .unwrap_or(Ok(()))
        .map_err(|e| e.to_string());
    recentes_mudaram(&app);
    r
}

/// Se o recente não existe mais: tira da lista e devolve `true` (a interface avisa). Caminho fora
/// da lista é `false` sem olhar o disco (não é oráculo de existência).
#[tauri::command]
fn recente_ausente<R: Runtime>(caminho: String, app: AppHandle<R>) -> Result<bool, String> {
    let lista = com_recentes(&app, recentes::ler).unwrap_or_default();
    if !lista.contains(&caminho) || Path::new(&caminho).exists() {
        return Ok(false);
    }
    com_recentes(&app, |a| recentes::remover(a, &caminho))
        .transpose()
        .map_err(|e| e.to_string())?;
    recentes_mudaram(&app);
    Ok(true)
}

fn soltos<R: Runtime>(app: &AppHandle<R>, caminhos: &[PathBuf]) {
    let concedidos = app.state::<abertura::Concedidos>();
    caminhos.iter().for_each(|c| concedidos.conceder(c));
}

/// Diálogo de abrir no backend: o que o usuário escolher fica concedido. `extensoes` é só o
/// filtro do diálogo (vem da interface, que tem a lista); escolher é sempre do usuário.
#[tauri::command]
async fn escolher<R: Runtime>(
    extensoes: Vec<String>,
    janela: tauri::WebviewWindow<R>,
) -> Vec<String> {
    use tauri_plugin_dialog::DialogExt;
    let ext: Vec<&str> = extensoes.iter().map(String::as_str).collect();
    let escolhidos = janela
        .dialog()
        .file()
        .set_parent(&janela)
        .add_filter("JSON, JSON Lines", &ext)
        .blocking_pick_files()
        .unwrap_or_default();
    let caminhos = escolhidos.into_iter().filter_map(|f| f.into_path().ok());
    conceder_escolhidos(janela.app_handle(), caminhos)
}

/// Concede o que o usuário escolheu no diálogo e devolve os caminhos para a interface abrir.
fn conceder_escolhidos<R: Runtime>(
    app: &AppHandle<R>,
    escolhidos: impl Iterator<Item = PathBuf>,
) -> Vec<String> {
    let concedidos = app.state::<abertura::Concedidos>();
    escolhidos
        .inspect(|p| concedidos.conceder(p))
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Arquivos do argv da 1ª instância: o pedido é a concessão; entram na fila da abertura.
/// `-` abre stdin (ver `stdin_argv`).
fn conceder_argv<R: Runtime>(app: &AppHandle<R>, argv: &[String], cwd: &Path) {
    let abertura = app.state::<abertura::Abertura>();
    let concedidos = app.state::<abertura::Concedidos>();
    for c in abertura::caminhos_argv(argv, cwd) {
        concedidos.conceder(Path::new(&c));
        abertura.enfileirar(c);
    }
    stdin_argv(app, argv);
}

/// Temporário de stdin da 1ª instância; sai no `RunEvent::Exit`.
#[derive(Default)]
struct Stdin(Mutex<Option<nucleo::entrada::Temporario>>);

/// `app -`: grava stdin no temporário do núcleo (o mesmo da CLI) numa thread, para a janela
/// abrir sem esperar o EOF, e pede a abertura do caminho quando termina (concessão só desta
/// execução; não entra nos recentes, ver `lembrar`). Só na 1ª instância: com o app já aberto,
/// quem tem o stdin é a 2ª, que repassa o argv e sai. Erro (stdin terminal, vazio, acima do
/// teto, sem espaço) vai para o stderr e o app segue sem ele.
fn stdin_argv<R: Runtime>(app: &AppHandle<R>, argv: &[String]) {
    use std::io::IsTerminal;
    if !argv.iter().skip(1).any(|a| a == "-") {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let r = if std::io::stdin().is_terminal() {
            Err(std::io::Error::other(
                "`-` lê stdin, mas stdin é o terminal",
            ))
        } else {
            nucleo::entrada::gravar(std::io::stdin().lock(), "stdin", &nucleo::Opcoes::default())
        };
        match r {
            Ok(t) => {
                let c = t.caminho().to_string_lossy().into_owned();
                if let Ok(mut s) = app.state::<Stdin>().0.lock() {
                    *s = Some(t);
                }
                abertura::pedir(&app, vec![c]);
            }
            Err(e) => eprintln!("erro: {e}"),
        }
    });
}

/// Documento aberto: o id e o número de registros (= linhas visíveis).
#[derive(Serialize)]
struct Aberto {
    id: u32,
    total: usize,
}

/// Abre o arquivo, constrói o índice e devolve o id do documento e o total. Sem `id`, cria um
/// documento novo (uma aba); com `id`, troca o arquivo daquele documento (a consulta mostra o
/// resultado no lugar do original) e mantém o estado da consulta.
/// `async` para rodar fora da thread principal: o índice de 1 GB leva ~150 ms.
/// Dispara a varredura de inválidas numa thread própria, que emite `varredura` com o progresso;
/// a do arquivo anterior do documento é cancelada.
/// Só abre caminho concedido (ver `autorizado`).
#[tauri::command]
async fn abrir<R: Runtime>(
    caminho: String,
    id: Option<u32>,
    abertura: Option<u32>,
    forcar: Option<bool>,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<Aberto, String> {
    let caminho = autorizado(&app, &caminho)?;
    let arvore = Arvore::new(abrir_cancelavel(
        &caminho,
        abertura.unwrap_or(0),
        forcar.unwrap_or(false),
        &app,
        &estado,
    )?);
    let novo = id.is_none();
    // Lida depois do mapa: se o arquivo mudar entre os dois, o 1º `seguir` reabre à toa, mas
    // nunca acrescenta a um arquivo trocado.
    let marca = nucleo::Marca::ler(&caminho, arvore.arquivo().cru().len() as u64).ok();
    let aberto = instalar(Some(arvore), id, &app, &estado)?;
    // Reabrir no mesmo documento (resultado de jq) sai do modo seguir.
    let doc = estado.doc(aberto.id)?;
    *doc.vivo.lock().map_err(|e| e.to_string())? = None;
    *doc.marca.lock().map_err(|e| e.to_string())? = marca;
    // Recentes: só arquivo novo do usuário. Exemplo embutido fica fora; o resultado de consulta
    // jq reabre num documento que já existe (`id` Some), então também fica.
    // Baixado de URL também: o cache é temporário e o recente apontaria para um arquivo que some.
    if novo && exemplos::embutido(&caminho).is_none() && !nucleo::url::baixado(Path::new(&caminho))
    {
        lembrar(&app, &caminho);
    }
    Ok(aberto)
}

/// Põe `arvore` no documento `id` (ou num novo, sem `id`) e dispara a varredura em 2º plano.
/// `arvore` `None`: a árvore do documento já foi trocada por quem chama (arquivo vivo).
fn instalar<R: Runtime>(
    arvore: Option<Arvore>,
    id: Option<u32>,
    app: &AppHandle<R>,
    estado: &Estado,
) -> Result<Aberto, String> {
    let app = app.clone();
    let (id, doc) = match (arvore, id) {
        (Some(arvore), Some(id)) => {
            let doc = estado.doc(id)?;
            *doc.arvore.lock().map_err(|e| e.to_string())? = arvore;
            (id, doc)
        }
        (Some(arvore), None) => {
            let id = estado.proximo.fetch_add(1, Relaxed) + 1;
            let doc = Arc::new(Doc {
                arvore: Mutex::new(arvore),
                varredura: Default::default(),
                busca: Default::default(),
                consulta: Default::default(),
                salvar: Default::default(),
                exportar: Default::default(),
                ordem: Default::default(),
                filtro: Default::default(),
                perfil: Default::default(),
                vivo: Default::default(),
                marca: Default::default(),
            });
            let mut docs = estado.docs.lock().map_err(|e| e.to_string())?;
            docs.insert(id, Arc::clone(&doc));
            (id, doc)
        }
        (None, Some(id)) => (id, estado.doc(id)?),
        (None, None) => return Err("instalar sem árvore nem documento".into()),
    };
    let (total, arquivo) = doc.com(|a| (a.total(), a.compartilhado()))?;
    let conteudo = resumo::conteudo(&arquivo);
    let linhas = arquivo.linhas();
    // JSON em várias linhas: "linhas inválidas" não faz sentido (ver `Conteudo`).
    let pular = conteudo == Conteudo::JsonVariasLinhas;
    // Sem régua para um JSON só (decisão 2 do micronavegador): nem 2ª passada.
    let colorir = !pular && conteudo != Conteudo::JsonUnico;
    let cancelar = Arc::new(AtomicBool::new(false));
    let marcas: Arc<[AtomicU8]> = nucleo::regua::marcas(linhas).into();
    let geracao = {
        let mut v = doc.varredura.lock().map_err(|e| e.to_string())?;
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
        v.estrutura = None;
        v.inferencia = None;
        v.laudo = None;
        v.tabela = None;
        v.geracao
    };
    doc.filtro.lock().map_err(|e| e.to_string())?.reiniciar();
    doc.ordem.lock().map_err(|e| e.to_string())?.reiniciar();
    doc.busca.lock().map_err(|e| e.to_string())?.reiniciar();
    let doc = Arc::downgrade(&doc);
    std::thread::spawn(move || {
        // Grava no estado e emite, se o documento ainda estiver aberto com este arquivo.
        let avisar = |f: &dyn Fn(&mut Varredura)| {
            let Some(doc) = doc.upgrade() else { return };
            let Ok(mut v) = doc.varredura.lock() else {
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
            emitir(&app, "varredura", id, andamento);
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
        let estrutura = Arc::new(Estrutura::de(&inf));
        let ds = inf.resumo.discriminadores();
        // O laudo sai da mesma inferência: custo de percorrer os caminhos, sem reler o arquivo.
        let laudo = Arc::new(Laudo::de(conteudo, arquivo.bytes().len() as u64, &inf));
        let inf = Arc::new(inf);
        avisar(&|v| {
            v.campos = campos;
            v.estrutura = Some(Arc::clone(&estrutura));
            v.laudo = Some(Arc::clone(&laudo));
            v.inferencia = Some((Arc::clone(&inf), Arc::clone(&arquivo)));
        });
        // Depois do aviso: a Árvore, o Texto, a Estrutura e Exportar não esperam a lista de elementos.
        let tabela = Tabela::montar(Arc::clone(&arquivo), &inf, &cancelar).map(Arc::new);
        if cancelar.load(Relaxed) {
            return;
        }
        avisar(&|v| v.tabela = Some(tabela.clone()));
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
    Ok(Aberto { id, total })
}

/// Resposta de `seguir`: o documento depois da leitura.
#[derive(Serialize)]
struct Seguido {
    total: usize,
    registros: usize,
    /// Registros que chegaram nesta leitura (0 se nada chegou ou se o arquivo foi reaberto).
    novas: usize,
    /// O arquivo foi truncado, rotacionado ou reescrito e o documento foi reaberto do zero.
    reescrito: bool,
}

/// Modo seguir (como `tail -f`): a 1ª chamada troca o documento por uma cópia que cresce
/// (`nucleo::vivo`, sem SIGBUS se o original encolher); as seguintes acrescentam as linhas
/// completas que chegaram e refazem a varredura. A interface chama a cada intervalo e para de
/// chamar na pausa. Só abre caminho concedido (ver `autorizado`).
#[tauri::command]
async fn seguir<R: Runtime>(
    caminho: String,
    id: u32,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<Seguido, String> {
    let caminho = autorizado(&app, &caminho)?;
    let doc = estado.doc(id)?;
    let mut vivo = doc.vivo.lock().map_err(|e| e.to_string())?;
    let erro = |e: std::io::Error| format!("{caminho}: {e}");
    let (arquivo, reescrito) = match vivo.as_mut().map(nucleo::Vivo::atualizar) {
        Some(Ok(nucleo::Mudanca::Nada)) => {
            let (total, registros) = doc.com(|a| (a.total(), a.registros()))?;
            return Ok(Seguido {
                total,
                registros,
                novas: 0,
                reescrito: false,
            });
        }
        Some(Ok(nucleo::Mudanca::Cresceu(a))) => (a, false),
        Some(Err(e)) => return Err(erro(e)),
        Some(Ok(nucleo::Mudanca::Reescrito)) => {
            let (v, a) = nucleo::Vivo::abrir(&caminho, &Opcoes::default()).map_err(erro)?;
            *vivo = Some(v);
            (a, true)
        }
        // 1º seguir: o documento mostra o arquivo do `abrir`; só acrescenta se ainda é ele.
        None => {
            let marca = doc.marca.lock().map_err(|e| e.to_string())?.take();
            let (v, a, mesmo) =
                nucleo::Vivo::continuar(&caminho, &Opcoes::default(), marca.as_ref())
                    .map_err(erro)?;
            *vivo = Some(v);
            (a, !mesmo)
        }
    };
    let novas = doc.com(|a| {
        let antes = a.registros();
        let novas = if reescrito {
            0
        } else {
            arquivo.linhas().saturating_sub(antes)
        };
        // A 1ª cópia pode ter uma linha a menos (a última, sem `\n`): árvore nova.
        if reescrito || arquivo.linhas() < antes {
            *a = Arvore::new(arquivo);
        } else {
            a.crescer(arquivo);
        }
        novas
    })?;
    drop(vivo);
    let Aberto { total, .. } = instalar(None, Some(id), &app, &estado)?;
    let registros = doc.com(|a| a.registros())?;
    Ok(Seguido {
        total,
        registros,
        novas,
        reescrito,
    })
}

/// Fecha o documento: some do mapa (a árvore e o mapeamento do arquivo saem com ele), as
/// threads dele param e o resultado de consulta é apagado.
#[tauri::command]
fn fechar(id: u32, estado: State<'_, Estado>) -> Result<(), String> {
    let doc = estado
        .docs
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&id)
        .ok_or_else(|| format!("documento {id} não está aberto"))?;
    doc.encerrar();
    Ok(())
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
fn resumo(id: u32, estado: State<'_, Estado>) -> Result<Resumo, String> {
    let doc = estado.doc(id)?;
    let (bytes, registros) = doc.com(|a| (a.arquivo().cru().len(), a.arquivo().linhas()))?;
    let v = doc.varredura.lock().map_err(|e| e.to_string())?;
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

/// Um tipo visto num caminho: nome, quantas vezes e a primeira linha (base 0).
type TipoVisto = (&'static str, u64, Option<u64>);

/// `nucleo::estrutura` pronto para a interface (o núcleo não depende de `serde` ali).
#[derive(Clone, Serialize)]
struct Estrutura {
    entrada: &'static str,
    registros: u64,
    invalidas: u64,
    caminhos: Vec<CaminhoUi>,
    discriminadores: Vec<DiscriminadorUi>,
}

#[derive(Clone, Serialize)]
struct CaminhoUi {
    caminho: String,
    relacao: &'static str,
    profundidade: usize,
    presenca: u64,
    de: u64,
    tipos: Vec<TipoVisto>,
    avisos: Vec<&'static str>,
}

#[derive(Clone, Serialize)]
struct DiscriminadorUi {
    campo: String,
    variantes: Vec<VarianteUi>,
}

#[derive(Clone, Serialize)]
struct VarianteUi {
    valor: String,
    registros: u64,
    linha: Option<u64>,
    obrigatorias: Vec<String>,
    opcionais: Vec<(String, u64)>,
}

impl Estrutura {
    fn de(inf: &nucleo::estrutura::Inferencia) -> Self {
        use nucleo::estrutura::{Entrada, Relacao, Tipo};
        let r = &inf.resumo;
        let caminhos = r
            .caminhos()
            .into_iter()
            .map(|c| CaminhoUi {
                relacao: match c.relacao {
                    Relacao::Registro => "registro",
                    Relacao::Campo => "campo",
                    Relacao::Elemento => "elemento",
                    Relacao::Entrada => "entrada",
                },
                tipos: Tipo::TODOS
                    .iter()
                    .map(|&t| (t.nome(), c.tipos[t as usize]))
                    .filter(|(_, x)| x.n > 0)
                    .map(|(nome, x)| (nome, x.n, x.primeira()))
                    .collect(),
                avisos: c.avisos.iter().map(|a| a.nome()).collect(),
                caminho: c.caminho,
                profundidade: c.profundidade,
                presenca: c.presenca,
                de: c.de,
            })
            .collect();
        let discriminadores = r
            .discriminadores()
            .into_iter()
            .map(|d| DiscriminadorUi {
                campo: d.campo,
                variantes: d
                    .variantes
                    .into_iter()
                    .map(|v| VarianteUi {
                        valor: v.valor,
                        registros: v.registros.n,
                        linha: v.registros.primeira(),
                        obrigatorias: v.obrigatorias,
                        opcionais: v.opcionais,
                    })
                    .collect(),
            })
            .collect();
        Estrutura {
            entrada: match inf.entrada {
                Entrada::Jsonl => "jsonl",
                Entrada::JsonUnico => "json",
            },
            registros: r.registros(),
            invalidas: r.invalidas.n,
            caminhos,
            discriminadores,
        }
    }
}

/// Estrutura do arquivo aberto, ou `null` enquanto a inferência roda (ela é parte da varredura
/// de `abrir`; o evento `varredura` avisa com `inferiu`). Só lê o que já foi calculado.
#[tauri::command]
fn estrutura(id: u32, estado: State<'_, Estado>) -> Result<Option<Estrutura>, String> {
    let doc = estado.doc(id)?;
    let e = doc
        .varredura
        .lock()
        .map_err(|e| e.to_string())?
        .estrutura
        .clone();
    // Clona fora da trava: são centenas de caminhos, não o arquivo.
    Ok(e.map(|e| (*e).clone()))
}

/// Laudo do arquivo (ver `nucleo::laudo`); `None` até a inferência acabar (`inferiu`).
#[tauri::command]
fn laudo(id: u32, estado: State<'_, Estado>) -> Result<Option<Laudo>, String> {
    let doc = estado.doc(id)?;
    let l = doc
        .varredura
        .lock()
        .map_err(|e| e.to_string())?
        .laudo
        .clone();
    Ok(l.map(|l| (*l).clone()))
}

/// Estatísticas e registros com e sem o caminho `indice` (a posição em `estrutura().caminhos`),
/// numa varredura do arquivo inteiro fora da thread da interface. Pedido novo cancela o
/// anterior; `None` se a inferência não acabou ou o pedido foi substituído.
#[tauri::command]
async fn perfil(
    id: u32,
    indice: usize,
    estado: State<'_, Estado>,
) -> Result<Option<nucleo::perfil::Perfil>, String> {
    let Some((inf, arquivo)) = inferencia(&estado, id)? else {
        return Ok(None);
    };
    let c = inf
        .resumo
        .caminhos()
        .into_iter()
        .nth(indice)
        .ok_or_else(|| format!("caminho {indice} não existe"))?;
    let cancelar = Arc::new(AtomicBool::new(false));
    {
        let doc = estado.doc(id)?;
        let mut atual = doc.perfil.lock().map_err(|e| e.to_string())?;
        atual.store(true, Relaxed);
        *atual = Arc::clone(&cancelar);
    }
    let p = tauri::async_runtime::spawn_blocking(move || {
        nucleo::perfil::perfil(&inf, arquivo.bytes(), &c, &cancelar)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok((!p.cancelado).then_some(p))
}

/// Inferência crua e o arquivo de onde veio (Exportar gera e valida a partir dela).
type InfArquivo = (Arc<nucleo::estrutura::Inferencia>, Arc<ArquivoLinhas>);

// ---- Exportar: Zod e JSON Schema da estrutura inferida, validados contra o arquivo inteiro. ----

/// Validação em curso (ver `exportar`): `geracao` descarta evento de pedido anterior.
#[derive(Default)]
struct EstadoExportar {
    geracao: u64,
    cancelar: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Formato {
    Zod,
    Schema,
    Ts,
}

#[derive(Clone, Copy, Deserialize)]
struct PedidoExportar {
    formato: Formato,
    literal: bool,
    numero: bool,
}

impl PedidoExportar {
    fn opcoes(self) -> nucleo::exportar::Opcoes {
        nucleo::exportar::Opcoes {
            literal: self.literal,
            // Tipos descrevem o JSON como está: "tratar como número" não se aplica.
            numero: self.numero && !matches!(self.formato, Formato::Ts),
        }
    }
    fn codigo(self, r: &nucleo::estrutura::Resumo) -> String {
        match self.formato {
            Formato::Zod => nucleo::exportar::zod(r, self.opcoes()),
            Formato::Schema => nucleo::exportar::schema(r, self.opcoes()),
            Formato::Ts => nucleo::exportar::tipos(r, self.opcoes()),
        }
    }
    /// O JSON Schema que valida o arquivo: o próprio, ou o da entrada que o Zod/tipo aceita.
    fn schema(self, r: &nucleo::estrutura::Resumo) -> String {
        match self.formato {
            Formato::Schema => nucleo::exportar::schema(r, self.opcoes()),
            Formato::Zod | Formato::Ts => nucleo::exportar::schema_da_entrada_zod(r, self.opcoes()),
        }
    }
}

#[derive(Serialize)]
struct Exportado {
    geracao: u64,
    codigo: String,
}

/// Evento `validacao`: progresso e, com `fim`, o resultado ("N de M registros passam").
#[derive(Clone, Serialize)]
struct AndamentoValidacao {
    geracao: u64,
    varridas: u64,
    total: u64,
    fim: bool,
    passam: u64,
    falham: u64,
    invalidas: u64,
    /// `[linha base 0, motivo]`, as primeiras por linha.
    falhas: Vec<(u64, String)>,
}

/// A inferência do documento, ou `None` se ainda não acabou.
fn inferencia(estado: &Estado, id: u32) -> Result<Option<InfArquivo>, String> {
    let doc = estado.doc(id)?;
    let v = doc.varredura.lock().map_err(|e| e.to_string())?;
    Ok(v.inferencia.clone())
}

/// Gera o código pedido e valida o arquivo inteiro contra ele numa thread própria, que emite
/// `validacao` (no máximo a cada `INTERVALO`, e no fim); a validação anterior é cancelada.
/// `None` enquanto a inferência não acabou (o evento `varredura` traz `inferiu`).
/// O Zod é validado pelo Schema da entrada que ele aceita (`schema_da_entrada_zod`).
#[tauri::command]
fn exportar<R: Runtime>(
    id: u32,
    pedido: PedidoExportar,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<Option<Exportado>, String> {
    let Some((inf, arquivo)) = inferencia(&estado, id)? else {
        return Ok(None);
    };
    let codigo = pedido.codigo(&inf.resumo);
    let schema = pedido.schema(&inf.resumo);
    let geracao = validar_em_fundo(
        app,
        &estado.doc(id)?,
        id,
        arquivo,
        Some(inf.entrada),
        schema,
    )?;
    Ok(Some(Exportado { geracao, codigo }))
}

/// Valida `arquivo` contra `schema` numa thread própria, que emite `validacao` (no máximo a
/// cada `INTERVALO`, e no fim) com a geração devolvida; a validação anterior do documento é
/// cancelada. Sem `entrada`, a thread infere antes (arquivo de fora, ainda não varrido).
fn validar_em_fundo<R: Runtime>(
    app: AppHandle<R>,
    doc: &Arc<Doc>,
    id: u32,
    arquivo: Arc<ArquivoLinhas>,
    entrada: Option<nucleo::estrutura::Entrada>,
    schema: String,
) -> Result<u64, String> {
    let (geracao, cancelar) = {
        let mut e = doc.exportar.lock().map_err(|e| e.to_string())?;
        e.cancelar.store(true, Relaxed);
        e.cancelar = Arc::new(AtomicBool::new(false));
        e.geracao += 1;
        (e.geracao, Arc::clone(&e.cancelar))
    };
    let doc = Arc::downgrade(doc);
    std::thread::spawn(move || {
        let avisar = |a: AndamentoValidacao| {
            let vale = doc
                .upgrade()
                .and_then(|d| d.exportar.lock().ok().map(|e| e.geracao == geracao));
            if vale == Some(true) {
                emitir(&app, "validacao", id, a);
            }
        };
        let vazio = |varridas, total, fim| AndamentoValidacao {
            geracao,
            varridas,
            total,
            fim,
            passam: 0,
            falham: 0,
            invalidas: 0,
            falhas: Vec::new(),
        };
        // shortcut: a inferência inteira só para saber se é JSONL ou JSON único; trocar por um
        // detector leve se validar arquivo grande de fora demorar a começar.
        let entrada = entrada.unwrap_or_else(|| {
            nucleo::estrutura::inferir(arquivo.bytes(), &cancelar, |_| {}).entrada
        });
        let mut aviso = Instant::now();
        let r = nucleo::validar::validar(arquivo.cru(), entrada, &schema, &cancelar, |p| {
            if aviso.elapsed() >= INTERVALO {
                aviso = Instant::now();
                avisar(vazio(p.varridas, p.total, false));
            }
        });
        match r {
            Ok(v) if !v.cancelado => avisar(AndamentoValidacao {
                passam: v.passam,
                falham: v.falham,
                invalidas: v.invalidas,
                falhas: v.falhas.into_iter().map(|f| (f.linha, f.motivo)).collect(),
                ..vazio(v.varridas, v.total, true)
            }),
            Ok(_) => {}
            // Schema gerado que não compila é bug do gerador: aparece como falha, não some.
            Err(e) => avisar(AndamentoValidacao {
                falhas: vec![(0, e)],
                ..vazio(0, 0, true)
            }),
        }
    });
    Ok(geracao)
}

/// Validação de outro arquivo (ou contra outro schema) em curso: a geração dos eventos
/// `validacao` e o nome do que foi escolhido, para a interface mostrar.
#[derive(Serialize)]
struct Validando {
    geracao: u64,
    nome: String,
}

fn nome_de(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Valida outro arquivo, escolhido no diálogo do backend, contra a estrutura do documento
/// (o mesmo schema do Exportar com `pedido`). `None` se o usuário cancelou.
#[tauri::command]
async fn validar_outro<R: Runtime>(
    id: u32,
    pedido: PedidoExportar,
    extensoes: Vec<String>,
    janela: tauri::WebviewWindow<R>,
    estado: State<'_, Estado>,
) -> Result<Option<Validando>, String> {
    use tauri_plugin_dialog::DialogExt;
    let ext: Vec<&str> = extensoes.iter().map(String::as_str).collect();
    let escolhido = janela
        .dialog()
        .file()
        .set_parent(&janela)
        .add_filter("JSON, JSON Lines", &ext)
        .blocking_pick_file();
    let Some(p) = escolhido.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    validar_outro_em(janela.app_handle().clone(), &estado, id, pedido, &p).map(Some)
}

fn validar_outro_em<R: Runtime>(
    app: AppHandle<R>,
    estado: &Estado,
    id: u32,
    pedido: PedidoExportar,
    caminho: &Path,
) -> Result<Validando, String> {
    let Some((inf, _)) = inferencia(estado, id)? else {
        return Err("a estrutura ainda está sendo inferida".into());
    };
    let schema = pedido.schema(&inf.resumo);
    let outro = Arc::new(abrir_linhas(&caminho.to_string_lossy())?);
    let geracao = validar_em_fundo(app, &estado.doc(id)?, id, outro, None, schema)?;
    Ok(Validando {
        geracao,
        nome: nome_de(caminho),
    })
}

/// Valida o documento contra um JSON Schema de fora, escolhido no diálogo do backend. O schema
/// é entrada não confiável: `nucleo::validar::compilar` limita tamanho, profundidade e `$ref`
/// (nada é buscado). Schema recusado volta como erro. `None` se o usuário cancelou.
#[tauri::command]
async fn validar_com_schema<R: Runtime>(
    id: u32,
    janela: tauri::WebviewWindow<R>,
    estado: State<'_, Estado>,
) -> Result<Option<Validando>, String> {
    use tauri_plugin_dialog::DialogExt;
    let escolhido = janela
        .dialog()
        .file()
        .set_parent(&janela)
        .add_filter("JSON Schema", &["json"])
        .blocking_pick_file();
    let Some(p) = escolhido.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    validar_com_schema_em(janela.app_handle().clone(), &estado, id, &p).map(Some)
}

fn validar_com_schema_em<R: Runtime>(
    app: AppHandle<R>,
    estado: &Estado,
    id: u32,
    caminho: &Path,
) -> Result<Validando, String> {
    let schema = nucleo::validar::ler_schema(caminho)?;
    nucleo::validar::compilar(&schema).map_err(|e| format!("schema recusado: {e}"))?;
    let Some((inf, arquivo)) = inferencia(estado, id)? else {
        return Err("a estrutura ainda está sendo inferida".into());
    };
    let geracao = validar_em_fundo(
        app,
        &estado.doc(id)?,
        id,
        arquivo,
        Some(inf.entrada),
        schema,
    )?;
    Ok(Validando {
        geracao,
        nome: nome_de(caminho),
    })
}

/// Para a validação em curso (Esc, trocar de vista ou de aba).
#[tauri::command]
fn cancelar_validacao(id: u32, estado: State<'_, Estado>) -> Result<(), String> {
    let doc = estado.doc(id)?;
    let mut e = doc.exportar.lock().map_err(|e| e.to_string())?;
    e.cancelar.store(true, Relaxed);
    e.geracao += 1;
    Ok(())
}

// ---- Diff entre dois documentos abertos (antes × depois). ----

#[derive(Clone, Serialize)]
struct MudancaUi {
    caminho: String,
    regra: &'static str,
    classe: &'static str,
    antes: [u64; 2],
    depois: [u64; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    tipos: Option<[Vec<&'static str>; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variante: Option<String>,
}

#[derive(Serialize)]
struct DiffEstrutura {
    mudancas: Vec<MudancaUi>,
    /// Caminhos nos dois lados sem mudança.
    iguais: Vec<String>,
    /// Registros inválidos ignorados `[antes, depois]`.
    invalidas: [u64; 2],
}

/// Diff de estrutura com a taxonomia do núcleo; `None` enquanto alguma inferência não acabou.
#[tauri::command]
fn diff_estrutura(
    antes: u32,
    depois: u32,
    estado: State<'_, Estado>,
) -> Result<Option<DiffEstrutura>, String> {
    use nucleo::diff::{diff, iguais};
    let (Some((a, _)), Some((b, _))) = (inferencia(&estado, antes)?, inferencia(&estado, depois)?)
    else {
        return Ok(None);
    };
    let ms = diff(&a.resumo, &b.resumo);
    Ok(Some(DiffEstrutura {
        iguais: iguais(&a.resumo, &b.resumo, &ms),
        invalidas: [a.resumo.invalidas.n, b.resumo.invalidas.n],
        mudancas: ms.into_iter().map(mudanca_ui).collect(),
    }))
}

fn mudanca_ui(m: nucleo::diff::Mudanca) -> MudancaUi {
    use nucleo::diff::Detalhe;
    let nomes = |ts: &[nucleo::estrutura::Tipo]| ts.iter().map(|t| t.nome()).collect();
    MudancaUi {
        regra: m.regra.nome(),
        classe: m.regra.classe().nome(),
        antes: [m.antes.n, m.antes.de],
        depois: [m.depois.n, m.depois.de],
        tipos: match &m.detalhe {
            Detalhe::Tipos(x, y) => Some([nomes(x), nomes(y)]),
            _ => None,
        },
        variante: match m.detalhe {
            Detalhe::Variante(v) => Some(v),
            _ => None,
        },
        caminho: m.caminho,
    }
}

/// Evento `diff_valores`: progresso e, no fim, o resultado.
#[derive(Clone, Serialize)]
struct AndamentoDiff {
    geracao: u64,
    varridas: u64,
    total: u64,
    resultado: Option<nucleo::valores::DiffArquivos>,
}

/// Diff de valores numa thread própria, que emite `diff_valores` (no máximo a cada `INTERVALO`,
/// e no fim); o anterior é cancelado. `registros` (base 0): compara só esses dois; sem ele, o
/// registro N de um contra o N do outro, no arquivo inteiro. Devolve a geração, ou `None`
/// enquanto alguma inferência não acabou (ela diz como ler os registros).
#[tauri::command]
fn diff_valores<R: Runtime>(
    antes: u32,
    depois: u32,
    registros: Option<[u64; 2]>,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<Option<u64>, String> {
    let (Some((ia, fa)), Some((ib, fb))) =
        (inferencia(&estado, antes)?, inferencia(&estado, depois)?)
    else {
        return Ok(None);
    };
    let (geracao, cancelar) = {
        let mut e = estado.diff.lock().map_err(|e| e.to_string())?;
        e.cancelar.store(true, Relaxed);
        e.cancelar = Arc::new(AtomicBool::new(false));
        e.geracao += 1;
        (e.geracao, Arc::clone(&e.cancelar))
    };
    std::thread::spawn(move || {
        let avisar = |a: AndamentoDiff| {
            let vale = app
                .state::<Estado>()
                .diff
                .lock()
                .ok()
                .map(|e| e.geracao == geracao);
            if vale == Some(true) {
                emitir(&app, "diff_valores", antes, a);
            }
        };
        let (a, b) = (fa.bytes(), fb.bytes());
        let r = match registros {
            Some([na, nb]) => {
                nucleo::valores::registros(a, ia.entrada, na, b, ib.entrada, nb, &cancelar)
            }
            None => {
                let mut aviso = Instant::now();
                nucleo::valores::arquivos(a, ia.entrada, b, ib.entrada, &cancelar, |p| {
                    if aviso.elapsed() >= INTERVALO {
                        aviso = Instant::now();
                        avisar(AndamentoDiff {
                            geracao,
                            varridas: p.varridas,
                            total: p.total,
                            resultado: None,
                        });
                    }
                })
            }
        };
        if !r.cancelado {
            avisar(AndamentoDiff {
                geracao,
                varridas: 1,
                total: 1,
                resultado: Some(r),
            });
        }
    });
    Ok(Some(geracao))
}

/// Para o diff de valores em curso (Esc, trocar de vista, fechar uma das abas).
#[tauri::command]
fn cancelar_diff_valores(estado: State<'_, Estado>) -> Result<(), String> {
    let mut e = estado.diff.lock().map_err(|e| e.to_string())?;
    e.cancelar.store(true, Relaxed);
    e.geracao += 1;
    Ok(())
}

// ---- Linha do tempo de uma pasta de versões datadas. ----

#[derive(Clone, Serialize)]
struct PassoUi {
    /// Canônico e já concedido: a interface abre direto para comparar dois pontos.
    caminho: String,
    nome: String,
    /// "AAAA-MM-DD HH:MM:SS" UTC (do nome ou do mtime).
    data: String,
    erro: Option<String>,
    registros: u64,
    invalidas: u64,
    contra: Option<usize>,
    quebras: usize,
    compativeis: usize,
    mudancas: Vec<MudancaUi>,
    mais: usize,
}

#[derive(Clone, Serialize)]
struct LinhaUi {
    /// "nome" ou "mtime".
    origem: &'static str,
    ignorados: usize,
    /// "prazo" quando parou no prazo (cancelada não chega à interface).
    parada: Option<&'static str>,
    passos: Vec<PassoUi>,
}

/// Evento `linha_do_tempo`: progresso em versões e, no fim, o resultado.
#[derive(Clone, Serialize)]
struct AndamentoLinha {
    geracao: u64,
    feitas: usize,
    total: usize,
    resultado: Option<LinhaUi>,
}

fn linha_ui(l: nucleo::linha_do_tempo::LinhaDoTempo) -> LinhaUi {
    use nucleo::linha_do_tempo::{Origem, Parada, data_texto};
    LinhaUi {
        origem: match l.origem {
            Origem::Nome => "nome",
            Origem::Mtime => "mtime",
        },
        ignorados: l.ignorados,
        parada: l.parada.map(|p| match p {
            Parada::Prazo => "prazo",
            Parada::Cancelado => "cancelado",
        }),
        passos: l
            .passos
            .into_iter()
            .map(|p| PassoUi {
                caminho: p.versao.caminho.to_string_lossy().into_owned(),
                data: data_texto(p.versao.ms),
                nome: p.versao.nome,
                erro: p.erro,
                registros: p.registros,
                invalidas: p.invalidas,
                contra: p.contra,
                quebras: p.quebras,
                compativeis: p.compativeis,
                mudancas: p.mudancas.into_iter().map(mudanca_ui).collect(),
                mais: p.mais,
            })
            .collect(),
    }
}

/// Diálogo de escolher pasta no backend: a escolhida fica concedida (não o que está dentro).
#[tauri::command]
async fn escolher_pasta<R: Runtime>(janela: tauri::WebviewWindow<R>) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let p = janela
        .dialog()
        .file()
        .set_parent(&janela)
        .blocking_pick_folder()?
        .into_path()
        .ok()?;
    conceder_escolhidos(janela.app_handle(), std::iter::once(p)).pop()
}

/// Linha do tempo da pasta concedida `pasta`, numa thread própria que emite `linha_do_tempo`
/// (no máximo a cada `INTERVALO`, e no fim); a anterior é cancelada. Os tetos do núcleo
/// (`LIMITES`) valem aqui; passar deles é erro antes de abrir qualquer arquivo. Cada versão
/// listada fica concedida, para a interface abrir os dois pontos que o usuário comparar: a
/// listagem só aceita arquivo regular dentro da pasta (link para fora não entra).
#[tauri::command]
fn linha_do_tempo<R: Runtime>(
    pasta: String,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<u64, String> {
    use nucleo::linha_do_tempo::{LIMITES, listar, percorrer};
    let base = autorizado(&app, &pasta)?;
    if !Path::new(&base).is_dir() {
        return Err(NEGADO.into());
    }
    let lista = listar(Path::new(&base), &LIMITES).map_err(|e| e.to_string())?;
    let concedidos = app.state::<abertura::Concedidos>();
    lista
        .versoes
        .iter()
        .for_each(|v| concedidos.conceder(&v.caminho));
    let (geracao, cancelar) = {
        let mut e = estado.linha.lock().map_err(|e| e.to_string())?;
        e.cancelar.store(true, Relaxed);
        e.cancelar = Arc::new(AtomicBool::new(false));
        e.geracao += 1;
        (e.geracao, Arc::clone(&e.cancelar))
    };
    std::thread::spawn(move || {
        let avisar = |a: AndamentoLinha| {
            let vale = app
                .state::<Estado>()
                .linha
                .lock()
                .ok()
                .map(|e| e.geracao == geracao);
            if vale == Some(true) {
                let _ = app.emit("linha_do_tempo", a);
            }
        };
        let op = Opcoes {
            cancelar: Some(&cancelar),
            ..Opcoes::default()
        };
        let mut aviso = None::<Instant>;
        let l = percorrer(lista, &LIMITES, &op, &cancelar, |feitas, total| {
            if feitas < total && aviso.is_some_and(|t| t.elapsed() < INTERVALO) {
                return;
            }
            aviso = Some(Instant::now());
            avisar(AndamentoLinha {
                geracao,
                feitas,
                total,
                resultado: None,
            });
        });
        let total = l.passos.len();
        avisar(AndamentoLinha {
            geracao,
            feitas: total,
            total,
            resultado: Some(linha_ui(l)),
        });
    });
    Ok(geracao)
}

/// Para a linha do tempo em curso (Esc, trocar de vista).
#[tauri::command]
fn cancelar_linha_do_tempo(estado: State<'_, Estado>) -> Result<(), String> {
    let mut e = estado.linha.lock().map_err(|e| e.to_string())?;
    e.cancelar.store(true, Relaxed);
    e.geracao += 1;
    Ok(())
}

/// Diálogo de salvar no backend (como `escolher`): a webview não escolhe caminho nem manda
/// conteúdo; o código é gerado de novo aqui. `nome` é só a sugestão do diálogo.
/// Devolve o caminho salvo, ou `None` se o usuário cancelou.
#[tauri::command]
async fn salvar_exportado<R: Runtime>(
    id: u32,
    pedido: PedidoExportar,
    nome: String,
    janela: tauri::WebviewWindow<R>,
    estado: State<'_, Estado>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some((inf, _)) = inferencia(&estado, id)? else {
        return Err("a estrutura ainda está sendo inferida".into());
    };
    let codigo = pedido.codigo(&inf.resumo);
    let (filtro, ext) = match pedido.formato {
        Formato::Zod | Formato::Ts => ("TypeScript", "ts"),
        Formato::Schema => ("JSON Schema", "json"),
    };
    // Só o nome do arquivo: a sugestão não escolhe pasta.
    let nome = Path::new(&nome)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let destino = janela
        .dialog()
        .file()
        .set_parent(&janela)
        .set_file_name(nome)
        .add_filter(filtro, &[ext])
        .blocking_save_file();
    let Some(p) = destino.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    std::fs::write(&p, codigo).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Some(p.to_string_lossy().into_owned()))
}

#[derive(Serialize)]
struct TabelaUi {
    registros: usize,
    objetos: u64,
    colunas: Vec<Coluna>,
    omitidas: usize,
    /// Por que o arquivo não vira tabela (`colunas` vazio).
    indisponivel: Option<Indisponivel>,
    ordem: Option<OrdemUi>,
    filtro: Option<FiltroUi>,
}

#[derive(Serialize)]
struct OrdemUi {
    coluna: usize,
    desc: bool,
}

/// Colunas da Tabela e a ordem pronta, ou `null` enquanto a inferência roda (igual a
/// `estrutura`).
#[tauri::command]
fn tabela(id: u32, estado: State<'_, Estado>) -> Result<Option<TabelaUi>, String> {
    let doc = estado.doc(id)?;
    let t = doc
        .varredura
        .lock()
        .map_err(|e| e.to_string())?
        .tabela
        .clone();
    let (filtro, _, ordem) = vista(&doc)?;
    let ordem = ordem.map(|(o, desc)| OrdemUi {
        coluna: o.coluna,
        desc,
    });
    let filtro = filtro.map(|f| f.ui.clone());
    Ok(t.map(|t| match t {
        Ok(t) => TabelaUi {
            registros: t.registros(),
            objetos: t.objetos,
            colunas: t.colunas.clone(),
            omitidas: t.omitidas,
            indisponivel: None,
            ordem,
            filtro,
        },
        Err(m) => TabelaUi {
            registros: 0,
            objetos: 0,
            colunas: Vec::new(),
            omitidas: 0,
            indisponivel: Some(m),
            ordem: None,
            filtro: None,
        },
    }))
}

/// O filtro pronto (e sua versão) e a ordem pronta, esta só se foi feita sobre esse filtro.
type Vista = (Option<Arc<FiltroPronto>>, u64, Option<(Arc<Ordem>, bool)>);

fn vista(doc: &Doc) -> Result<Vista, String> {
    let (filtro, versao) = {
        let f = doc.filtro.lock().map_err(|e| e.to_string())?;
        (f.pronto.clone(), f.versao)
    };
    let o = doc.ordem.lock().map_err(|e| e.to_string())?;
    let ordem = o.pronta.clone().filter(|_| o.filtro == versao);
    Ok((filtro, versao, ordem))
}

fn tabela_pronta(doc: &Doc) -> Result<Arc<Tabela>, String> {
    match doc
        .varredura
        .lock()
        .map_err(|e| e.to_string())?
        .tabela
        .clone()
    {
        Some(Ok(t)) => Ok(t),
        _ => Err("tabela indisponível neste documento".into()),
    }
}

/// Linhas `[inicio, inicio + quantidade)` da Tabela, só as do filtro pronto, na ordem pronta,
/// só com as `colunas` pedidas (as visíveis).
#[tauri::command]
fn tabela_linhas(
    id: u32,
    inicio: usize,
    quantidade: usize,
    mut colunas: Vec<usize>,
    estado: State<'_, Estado>,
) -> Result<Vec<tabela::Linha>, String> {
    let doc = estado.doc(id)?;
    let t = tabela_pronta(&doc)?;
    let (filtro, _, ordem) = vista(&doc)?;
    colunas.truncate(tabela::MAX_COLUNAS);
    Ok(t.janela(
        ordem.as_ref().map(|(o, desc)| (&**o, *desc)),
        filtro.as_ref().map(|f| &f.registros[..]),
        inicio,
        quantidade.min(MAX_JANELA),
        &colunas,
    ))
}

/// Evento `ordenacao`.
#[derive(Clone, Serialize)]
struct Ordenacao {
    geracao: u64,
    lidos: u64,
    total: u64,
    fim: bool,
    falha: Option<FalhaOrdem>,
}

#[derive(Serialize)]
struct Ordenado {
    geracao: u64,
    /// Já valeu (volta à ordem do arquivo ou só troca de direção): é só pedir as linhas.
    pronta: bool,
}

/// Ordena a Tabela pela `coluna`; sem coluna, volta à ordem do arquivo. A coluna já ordenada
/// só troca de direção. Coluna nova ordena o arquivo todo em 2º plano, com o evento
/// `ordenacao` (progresso e fim); até o fim, as linhas seguem na ordem anterior.
#[tauri::command]
fn ordenar<R: Runtime>(
    id: u32,
    coluna: Option<usize>,
    desc: bool,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<Ordenado, String> {
    let doc = estado.doc(id)?;
    let t = tabela_pronta(&doc)?;
    let (filtro, versao, _) = vista(&doc)?;
    let mut o = doc.ordem.lock().map_err(|e| e.to_string())?;
    o.cancelar();
    let geracao = o.geracao;
    let Some(coluna) = coluna else {
        o.pronta = None;
        return Ok(Ordenado {
            geracao,
            pronta: true,
        });
    };
    if let Some((p, _)) = &o.pronta
        && p.coluna == coluna
        && o.filtro == versao
    {
        o.pronta = Some((Arc::clone(p), desc));
        return Ok(Ordenado {
            geracao,
            pronta: true,
        });
    }
    if coluna >= t.colunas.len() {
        return Err(format!("coluna {coluna} não existe"));
    }
    let cancelar = Arc::new(AtomicBool::new(false));
    o.cancelar = Arc::clone(&cancelar);
    drop(o);
    let doc = Arc::downgrade(&doc);
    let total = filtro.as_ref().map_or(t.registros(), |f| f.registros.len()) as u64;
    std::thread::spawn(move || {
        let ultimo = Mutex::new(Instant::now());
        let aviso = |lidos| {
            if let Ok(mut u) = ultimo.lock()
                && u.elapsed() >= INTERVALO
            {
                *u = Instant::now();
                let fim = false;
                emitir(
                    &app,
                    "ordenacao",
                    id,
                    Ordenacao {
                        geracao,
                        lidos,
                        total,
                        fim,
                        falha: None,
                    },
                );
            }
        };
        let registros = filtro.as_ref().map(|f| &f.registros[..]);
        let falha = match t.ordenar(coluna, registros, &cancelar, &aviso) {
            Ok(ordem) => {
                let Some(doc) = doc.upgrade() else { return };
                let Ok(mut o) = doc.ordem.lock() else { return };
                o.publicar(geracao, ordem, desc, versao)
            }
            Err(f) => Some(f),
        };
        let fim = Ordenacao {
            geracao,
            lidos: total,
            total,
            fim: true,
            falha,
        };
        emitir(&app, "ordenacao", id, fim);
    });
    Ok(Ordenado {
        geracao,
        pronta: false,
    })
}

/// Para a ordenação em curso; a ordem anterior continua valendo. O evento de fim chega com
/// `falha: cancelada`.
#[tauri::command]
fn cancelar_ordenacao(id: u32, estado: State<'_, Estado>) -> Result<(), String> {
    let doc = estado.doc(id)?;
    doc.ordem.lock().map_err(|e| e.to_string())?.cancelar();
    Ok(())
}

/// O que filtrar: os registros com (ou sem) o caminho `caminho` (posição em
/// `estrutura().caminhos`), os da `variante` do `discriminador` (posições em
/// `estrutura().discriminadores`), ou, no caminho, os de um valor dos mais comuns do `perfil`
/// (`comum`) ou de uma faixa de número dele (`negativo`/`expoente`).
#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum Alvo {
    Comum {
        caminho: usize,
        comum: String,
    },
    Faixa {
        caminho: usize,
        negativo: bool,
        expoente: Option<i32>,
    },
    Caminho {
        caminho: usize,
        tem: bool,
    },
    Variante {
        discriminador: usize,
        variante: usize,
    },
}

/// Evento `filtragem`: progresso (só JSONL tem) e, com `fim`, os achados ou a falha.
#[derive(Clone, Serialize)]
struct Filtragem {
    geracao: u64,
    lidos: u64,
    total: u64,
    fim: bool,
    achados: usize,
    falha: Option<FalhaFiltragem>,
}

/// Caminho (segmentos), valor da variante (cru) e o que a interface mostra, do `alvo` do filtro.
fn alvo_segs(
    inf: &nucleo::estrutura::Inferencia,
    alvo: Alvo,
) -> Result<(Vec<nucleo::estrutura::Seg>, Option<String>, FiltroUi), String> {
    use nucleo::estrutura::Seg;
    let no_caminho = |caminho: usize| {
        inf.resumo
            .caminhos()
            .into_iter()
            .nth(caminho)
            .ok_or_else(|| format!("caminho {caminho} não existe"))
    };
    // Filtrar por valor é perguntar o valor: o perfil não mostra os de um segredo, nem isto.
    let com_valores = |caminho: usize| {
        no_caminho(caminho).and_then(|c| {
            if c.avisos.contains(&nucleo::estrutura::Aviso::Segredo) {
                Err(format!("caminho {caminho} tem segredo"))
            } else {
                Ok(c)
            }
        })
    };
    let vazio = |caminho| FiltroUi {
        achados: 0,
        caminho,
        tem: None,
        valor: None,
        comum: None,
        faixa: None,
    };
    Ok(match alvo {
        Alvo::Caminho { caminho, tem } => {
            let c = no_caminho(caminho)?;
            let ui = FiltroUi {
                tem: Some(tem),
                ..vazio(c.caminho)
            };
            (c.segs, None, ui)
        }
        Alvo::Comum { caminho, comum } => {
            let c = com_valores(caminho)?;
            let ui = FiltroUi {
                comum: Some(comum),
                ..vazio(c.caminho)
            };
            (c.segs, None, ui)
        }
        Alvo::Faixa {
            caminho,
            negativo,
            expoente,
        } => {
            let c = com_valores(caminho)?;
            let ui = FiltroUi {
                faixa: Some((negativo, expoente)),
                ..vazio(c.caminho)
            };
            (c.segs, None, ui)
        }
        Alvo::Variante {
            discriminador,
            variante,
        } => {
            let d = inf
                .resumo
                .discriminadores()
                .into_iter()
                .nth(discriminador)
                .ok_or_else(|| format!("discriminador {discriminador} não existe"))?;
            let v = d
                .variantes
                .into_iter()
                .nth(variante)
                .ok_or_else(|| format!("variante {variante} não existe"))?;
            let ui = FiltroUi {
                valor: Some(v.valor.clone()),
                ..vazio(d.campo)
            };
            // Discriminador é campo do topo do registro.
            (vec![Seg::Campo(d.cru)], Some(v.valor), ui)
        }
    })
}

/// O critério de `filtro` para os `segs` e `valor` de `alvo_segs`.
fn criterio<'a>(
    segs: &'a [nucleo::estrutura::Seg],
    valor: Option<&'a str>,
    ui: &'a FiltroUi,
) -> Criterio<'a> {
    match (valor, ui.tem, &ui.comum, ui.faixa) {
        (Some(v), ..) => Criterio::Igual(segs, v.as_bytes()),
        (_, _, Some(c), _) => Criterio::Comum(segs, c),
        (_, _, _, Some((neg, e))) => Criterio::Faixa(segs, neg, e),
        (None, Some(false), ..) => Criterio::NaoTem(segs),
        (None, ..) => Criterio::Tem(segs),
    }
}

/// Filtra os registros pelo `alvo` numa varredura do arquivo inteiro em 2º plano, com o evento
/// `filtragem`; o pedido anterior é cancelado. Até o fim, a Tabela segue com o filtro anterior.
/// Ao publicar, a ordem pronta deixa de valer (era de outro conjunto): a interface pede de novo.
/// `None` enquanto a inferência não acabou.
#[tauri::command]
fn filtrar<R: Runtime>(
    id: u32,
    alvo: Alvo,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<Option<u64>, String> {
    let Some((inf, arquivo)) = inferencia(&estado, id)? else {
        return Ok(None);
    };
    let (segs, valor, ui) = alvo_segs(&inf, alvo)?;
    let doc = estado.doc(id)?;
    let cancelar = Arc::new(AtomicBool::new(false));
    let (geracao, teto) = {
        let mut f = doc.filtro.lock().map_err(|e| e.to_string())?;
        f.cancelar();
        f.cancelar = Arc::clone(&cancelar);
        (f.geracao, f.teto)
    };
    let doc = Arc::downgrade(&doc);
    let total = arquivo.linhas() as u64;
    std::thread::spawn(move || {
        let ultimo = Mutex::new(Instant::now());
        let aviso = |lidos| {
            if let Ok(mut u) = ultimo.lock()
                && u.elapsed() >= INTERVALO
            {
                *u = Instant::now();
                let p = Filtragem {
                    geracao,
                    lidos,
                    total,
                    fim: false,
                    achados: 0,
                    falha: None,
                };
                emitir(&app, "filtragem", id, p);
            }
        };
        let criterio = criterio(&segs, valor.as_deref(), &ui);
        let r = filtro::filtrar_ate(
            &inf,
            arquivo.bytes(),
            criterio,
            nucleo::busca::BLOCO_PADRAO,
            teto,
            &cancelar,
            &aviso,
        );
        let (achados, falha) = match r {
            Ok(f) => {
                let Some(doc) = doc.upgrade() else { return };
                let Ok(mut e) = doc.filtro.lock() else { return };
                if e.geracao != geracao {
                    (0, Some(FalhaFiltragem::Cancelada))
                } else {
                    let achados = f.registros.len();
                    let ui = FiltroUi { achados, ..ui };
                    e.pronto = Some(Arc::new(FiltroPronto {
                        registros: f.registros,
                        ui,
                    }));
                    e.versao += 1;
                    drop(e);
                    // A ordem era de outro conjunto (`vista` já a ignora): para a que roda.
                    if let Ok(mut o) = doc.ordem.lock() {
                        o.reiniciar();
                    }
                    (achados, None)
                }
            }
            Err(f) => (0, Some(f)),
        };
        let fim = Filtragem {
            geracao,
            lidos: total,
            total,
            fim: true,
            achados,
            falha,
        };
        emitir(&app, "filtragem", id, fim);
    });
    Ok(Some(geracao))
}

/// Para o filtro em curso; o anterior continua valendo. O fim chega com `falha: cancelada`.
#[tauri::command]
fn cancelar_filtro(id: u32, estado: State<'_, Estado>) -> Result<(), String> {
    let doc = estado.doc(id)?;
    doc.filtro.lock().map_err(|e| e.to_string())?.cancelar();
    Ok(())
}

/// Tira o filtro (e o que estiver rodando); a ordem, que era sobre os filtrados, também sai.
#[tauri::command]
fn limpar_filtro(id: u32, estado: State<'_, Estado>) -> Result<(), String> {
    let doc = estado.doc(id)?;
    doc.filtro.lock().map_err(|e| e.to_string())?.reiniciar();
    doc.ordem.lock().map_err(|e| e.to_string())?.reiniciar();
    Ok(())
}

/// Onde está o filtrado de número `posicao` (na ordem do arquivo), para ⌘↓/⌘↑ na Árvore.
#[derive(Serialize)]
struct Filtrado {
    posicao: usize,
    total: usize,
    /// Linha e byte na linha, como `revelar` pede.
    linha: usize,
    byte: usize,
}

/// O `posicao`-ésimo registro do filtro pronto; `None` sem filtro ou fora dele. A Árvore filtra
/// só a navegação: ela segue mostrando o arquivo todo.
// shortcut: a interface guarda a posição da última ida; ⌘↓ não parte do cursor da Árvore.
#[tauri::command]
fn filtrado(
    id: u32,
    posicao: usize,
    estado: State<'_, Estado>,
) -> Result<Option<Filtrado>, String> {
    let doc = estado.doc(id)?;
    let (Some(f), _, _) = vista(&doc)? else {
        return Ok(None);
    };
    let Some(&r) = f.registros.get(posicao) else {
        return Ok(None);
    };
    let jsonl = doc
        .varredura
        .lock()
        .map_err(|e| e.to_string())?
        .inferencia
        .as_ref()
        .is_some_and(|(i, _)| i.entrada == nucleo::estrutura::Entrada::Jsonl);
    // JSONL: o registro é a linha. JSON único: o elemento, que a Tabela sabe onde começa.
    let onde = if jsonl {
        Some((r as usize, 0))
    } else {
        tabela_pronta(&doc)?
            .registro(r as usize)
            .map(|(_, linha, byte)| (linha, byte))
    };
    Ok(onde.map(|(linha, byte)| Filtrado {
        posicao,
        total: f.registros.len(),
        linha,
        byte,
    }))
}

fn marcas_atuais(estado: &Estado, id: u32) -> Result<Arc<[AtomicU8]>, String> {
    let doc = estado.doc(id)?;
    let v = doc.varredura.lock().map_err(|e| e.to_string())?;
    Ok(Arc::clone(&v.marcas))
}

/// A régua do micronavegador em até `k` baldes (ver `nucleo::regua::baldes`).
#[tauri::command]
fn regua(id: u32, k: usize, estado: State<'_, Estado>) -> Result<Baldes, String> {
    Ok(nucleo::regua::baldes(
        &marcas_atuais(&estado, id)?,
        k.min(MAX_BALDES),
    ))
}

/// Marca de cada registro `[inicio, inicio + quantidade)`: o ponto de cor ao lado dele.
#[tauri::command]
fn marcas(
    id: u32,
    inicio: usize,
    quantidade: usize,
    estado: State<'_, Estado>,
) -> Result<Vec<u8>, String> {
    Ok(nucleo::regua::janela(
        &marcas_atuais(&estado, id)?,
        inicio,
        quantidade.min(MAX_JANELA),
    ))
}

/// Linhas visíveis `[inicio, inicio + quantidade)`, prontas para desenhar. Vão como JSON:
/// são poucas e estruturadas (ver docs/decisoes.md).
#[tauri::command]
fn linhas(
    id: u32,
    inicio: usize,
    quantidade: usize,
    estado: State<'_, Estado>,
) -> Result<Vec<Linha>, String> {
    estado.com(id, |a| a.janela(inicio, quantidade.min(MAX_JANELA)))
}

/// Expande ou recolhe a linha visível `v`; devolve o novo total de linhas visíveis.
#[tauri::command]
fn alternar(id: u32, v: usize, estado: State<'_, Estado>) -> Result<usize, String> {
    estado.com(id, |a| {
        a.alternar(v);
        a.total()
    })
}

/// Linha visível do pai de `v`.
#[tauri::command]
fn pai(id: u32, v: usize, estado: State<'_, Estado>) -> Result<Option<usize>, String> {
    estado.com(id, |a| a.pai(v))
}

/// Linha visível do registro `registro` (base 0).
#[tauri::command]
fn ir_para(id: u32, registro: usize, estado: State<'_, Estado>) -> Result<Option<usize>, String> {
    estado.com(id, |a| a.linha_do_registro(registro))
}

/// Linhas cruas para a visão Texto, como bytes (ver `nucleo::texto::janela` e docs/decisoes.md).
#[tauri::command]
fn texto(
    id: u32,
    inicio: usize,
    quantidade: usize,
    estado: State<'_, Estado>,
) -> Result<tauri::ipc::Response, String> {
    estado.com(id, |a| {
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
    id: u32,
    registro: usize,
    byte: usize,
    estado: State<'_, Estado>,
) -> Result<Option<[usize; 2]>, String> {
    estado.com(id, |a| a.revelar(registro, byte).map(|v| [v, a.total()]))
}

/// O registro com um campo por linha, para o painel "Ver formatado" (com `truncado` se passou
/// de `MAX_FORMATADO`).
#[tauri::command]
fn formatado(id: u32, registro: usize, estado: State<'_, Estado>) -> Result<Formatado, String> {
    estado.com(id, |a| {
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
fn caminhos(id: u32, v: usize, estado: State<'_, Estado>) -> Result<Option<Caminhos>, String> {
    estado.com(id, |a| {
        a.caminho(v).map(|c| Caminhos {
            jq: c.jq(),
            jsonpath: c.jsonpath(),
            pointer: c.json_pointer(),
        })
    })
}

/// Rótulos das migalhas da linha visível `v` (a selecionada); ver `Arvore::migalhas`.
#[tauri::command]
fn migalhas(id: u32, v: usize, estado: State<'_, Estado>) -> Result<Option<Vec<String>>, String> {
    estado.com(id, |a| a.migalhas(v))
}

/// Valor da linha visível `v` para copiar (texto JSON decodificado; ver `Arvore::valor`).
#[tauri::command]
fn valor(id: u32, v: usize, estado: State<'_, Estado>) -> Result<Option<String>, String> {
    let valor = estado.com(id, |a| a.valor(v))?;
    match valor {
        Some(s) if s.len() > MAX_FORMATADO => Err(format!(
            "valor de {} MB passa do limite de {} MB para copiar",
            s.len() >> 20,
            MAX_FORMATADO >> 20
        )),
        v => Ok(v),
    }
}

/// Sal da anonimização (`nucleo::mascara::anonimizar`): um por execução do app, só na memória.
/// Mesmo valor, mesmo código enquanto o app está aberto; outro código depois de reabrir.
fn sal_da_sessao() -> u64 {
    static SAL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SAL.get_or_init(nucleo::mascara::sal_novo)
}

/// O valor da linha visível `v` sem a máscara de segredos: só no clique em "Mostrar valor", só
/// esse campo; a interface não guarda.
#[tauri::command]
fn valor_cru(id: u32, v: usize, estado: State<'_, Estado>) -> Result<Option<String>, String> {
    let valor = estado.com(id, |a| a.valor_cru(v))?;
    match valor {
        Some(s) if s.len() > MAX_FORMATADO => Err(format!(
            "valor de {} MB passa do limite de {} MB para mostrar",
            s.len() >> 20,
            MAX_FORMATADO >> 20
        )),
        v => Ok(v),
    }
}

/// Abre `url` no navegador do sistema, nunca dentro do app. A interface só chama no clique ou no
/// Enter do link e manda o `href` normalizado; aqui se recusa o que não for http(s) em ASCII
/// visível, então nada vira argumento de outro programa nem de outro esquema (`file:`, `javascript:`).
#[tauri::command]
fn abrir_link(url: String) -> Result<(), String> {
    if !link_valido(&url) {
        return Err("link recusado: só http:// ou https://".into());
    }
    #[cfg(target_os = "macos")]
    let mut c = std::process::Command::new("open");
    #[cfg(windows)]
    let mut c = std::process::Command::new("rundll32");
    #[cfg(windows)]
    c.arg("url.dll,FileProtocolHandler");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut c = std::process::Command::new("xdg-open");
    let mut filho = c.arg(&url).spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || filho.wait()); // recolhe o processo, sem zumbi
    Ok(())
}

fn link_valido(url: &str) -> bool {
    let resto = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    url.len() <= 2048
        && resto.is_some_and(|r| !r.is_empty() && !r.starts_with('/'))
        && url
            .bytes()
            .all(|b| (0x21..0x7f).contains(&b) && !b"\"<>\\^`{|}".contains(&b))
}

/// Roda o filtro jq `filtro` sobre `caminho` num processo filho (ver `filho`) e grava os
/// resultados (JSON compacto, um por linha) num JSONL temporário, avisado no evento `consulta`.
/// JSON único ou em várias linhas vira um documento; o resto, JSONL linha a linha. Assíncrono e
/// sem abrir o arquivo aqui: nada bloqueia a thread da interface. Devolve a geração.
#[tauri::command]
async fn consultar<R: Runtime>(
    id: u32,
    filtro: String,
    caminho: String,
    anonimizar: Option<bool>,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<u64, ErroConsulta> {
    let doc = estado.doc(id)?;
    // Caminho recusado não mexe na consulta em curso nem apaga o resultado mostrado.
    let caminho = autorizado(&app, &caminho)?;
    filho::arquivo_comum(&caminho)?;
    // Compila aqui só para devolver o erro de sintaxe com posição; quem roda é o filho. Antes de
    // `reiniciar`: filtro que não compila (digitando, ao vivo) não apaga o resultado mostrado.
    Consulta::nova(&filtro).map_err(|e| ErroConsulta {
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
    let (geracao, cancelar) = doc.consulta.lock().map_err(|e| e.to_string())?.reiniciar();
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
    abrir
        .open(&destino)
        .map_err(|e| format!("{}: {e}", destino.display()))?;
    // O resultado reabre pelo `abrir`: o backend que o criou o concede.
    app.state::<abertura::Concedidos>().conceder(&destino);
    doc.consulta.lock().map_err(|e| e.to_string())?.arquivo = Some(destino.clone());
    let doc = Arc::downgrade(&doc);
    std::thread::spawn(move || {
        let avisar = |a: AndamentoConsulta| emitir(&app, "consulta", id, a);
        let sal = anonimizar.unwrap_or(false).then(sal_da_sessao);
        let mut fim = filho::rodar(&filtro, &caminho, &destino, sal, &cancelar, |a| {
            avisar(AndamentoConsulta { geracao, ..a })
        });
        // Consulta mais nova, fechada ou documento fechado: o resultado não serve a ninguém.
        let atual = doc
            .upgrade()
            .is_some_and(|d| d.consulta.lock().is_ok_and(|c| c.geracao == geracao));
        let serve = fim.falha.is_none() && !fim.cancelado;
        if !atual || !serve {
            let _ = std::fs::remove_file(&destino);
        }
        if !atual {
            return;
        }
        fim.geracao = geracao;
        fim.arquivo = serve.then(|| destino.to_string_lossy().into_owned());
        avisar(fim);
    });
    Ok(geracao)
}

/// Cancela a consulta em curso e apaga o resultado (o arquivo original já foi reaberto).
#[tauri::command]
fn fechar_consulta(id: u32, estado: State<'_, Estado>) -> Result<(), String> {
    estado
        .doc(id)?
        .consulta
        .lock()
        .map_err(|e| e.to_string())?
        .reiniciar();
    Ok(())
}

/// Salva o resultado mostrado da consulta (`arquivo`, o temporário que o evento `consulta`
/// entregou) em JSONL ou CSV, pelo diálogo de salvar no backend. Só o resultado atual do
/// documento: caminho que não é o dele (outro arquivo, consulta nova já rodando) é recusado.
/// Devolve o caminho salvo, ou `None` se o usuário cancelou.
#[tauri::command]
async fn salvar_consulta<R: Runtime>(
    id: u32,
    arquivo: String,
    csv: bool,
    nome: String,
    janela: tauri::WebviewWindow<R>,
    estado: State<'_, Estado>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let atual = estado
        .doc(id)?
        .consulta
        .lock()
        .map_err(|e| e.to_string())?
        .arquivo
        .clone();
    let Some(origem) = atual.filter(|a| a.as_os_str() == arquivo.as_str()) else {
        return Err("o resultado da consulta não está mais aberto".into());
    };
    let (filtro, ext) = if csv {
        ("CSV", "csv")
    } else {
        ("JSONL", "jsonl")
    };
    let nome = Path::new(&nome)
        .file_stem()
        .map(|n| format!("{}.{ext}", n.to_string_lossy()))
        .unwrap_or_default();
    let destino = janela
        .dialog()
        .file()
        .set_parent(&janela)
        .set_file_name(nome)
        .add_filter(filtro, &[ext])
        .blocking_save_file();
    let Some(p) = destino.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    let mut protegidos = protegidos(janela.app_handle());
    protegidos.push(origem.clone());
    gravar_resultado(&origem, &p, csv, &protegidos)?;
    Ok(Some(p.to_string_lossy().into_owned()))
}

/// O que o salvar nunca sobrescreve: o que o usuário abriu nesta execução, os resultados de
/// consulta (todos concedidos) e os recentes (abrir por eles não concede).
fn protegidos<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
    let mut p = app.state::<abertura::Concedidos>().todos();
    p.extend(
        com_recentes(app, recentes::ler)
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from),
    );
    p
}

/// De onde vem o que o "salvar como" grava.
#[derive(Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
enum FonteSalvar {
    /// O arquivo inteiro.
    Tudo,
    /// O resultado do filtro jq (#47), rodado de novo sobre o original.
    Jq { filtro: String },
    /// Os registros do filtro da Estrutura/Tabela (#49).
    Filtro { alvo: Alvo },
}

#[derive(Deserialize)]
struct PedidoSalvar {
    fonte: FonteSalvar,
    #[serde(default)]
    transformacao: Transformacao,
    #[serde(default)]
    saida: Saida,
    /// Padrão: segredos mascarados. Sem máscara só se a interface mandar.
    #[serde(default)]
    mascara: Mascara,
}

/// Nos testes, o diálogo de salvar não abre: o destino vem daqui.
#[cfg(test)]
static DESTINO_TESTE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Pergunta onde salvar, no diálogo do sistema (o caminho nunca vem da interface).
fn escolher_destino<R: Runtime>(
    janela: &tauri::WebviewWindow<R>,
    nome: &str,
    saida: Saida,
) -> Option<PathBuf> {
    #[cfg(test)]
    {
        let _ = (janela, nome, saida);
        DESTINO_TESTE.lock().ok()?.clone()
    }
    #[cfg(not(test))]
    {
        use tauri_plugin_dialog::DialogExt;
        let ext = saida.extensao();
        let nome = Path::new(nome)
            .file_stem()
            .map(|n| format!("{}.{ext}", n.to_string_lossy()))
            .unwrap_or_default();
        janela
            .dialog()
            .file()
            .set_parent(janela)
            .set_file_name(nome)
            .add_filter(ext.to_uppercase(), &[ext])
            .blocking_save_file()
            .and_then(|f| f.into_path().ok())
    }
}

/// "Salvar como": grava num arquivo NOVO o que o `pedido` descreve (fonte, transformação,
/// formato, máscara), tudo compilado numa expressão jq (`nucleo::transformar::compor`) e rodado
/// no processo filho em fluxo, com o evento `salvando` (progresso; no fim, `arquivo` = o salvo).
/// Nunca sobrescreve o original nem outro arquivo aberto (`protegidos`); grava num temporário
/// na pasta do destino e só troca por `rename` no fim: cancelar ou falhar não deixa nada lá.
/// Devolve a geração, ou `None` se o usuário fechou o diálogo.
#[tauri::command]
async fn salvar_como<R: Runtime>(
    id: u32,
    caminho: String,
    pedido: PedidoSalvar,
    nome: String,
    janela: tauri::WebviewWindow<R>,
    estado: State<'_, Estado>,
) -> Result<Option<u64>, String> {
    let app = janela.app_handle().clone();
    let caminho = autorizado(&app, &caminho)?;
    filho::arquivo_comum(&caminho)?;
    let doc = estado.doc(id)?;
    pedido.transformacao.conferir()?;
    let fonte = match pedido.fonte {
        FonteSalvar::Tudo => Fonte::Tudo,
        FonteSalvar::Jq { filtro } => {
            // O erro do filtro do usuário, sem a transformação em volta.
            Consulta::nova(&filtro).map_err(|e| e.to_string())?;
            Fonte::Jq(filtro)
        }
        FonteSalvar::Filtro { alvo } => {
            let (inf, _) =
                inferencia(&estado, id)?.ok_or("a estrutura ainda está sendo inferida")?;
            let (segs, valor, ui) = alvo_segs(&inf, alvo)?;
            Fonte::filtro(&criterio(&segs, valor.as_deref(), &ui))?
        }
    };
    // O filho compõe com o `documento` do arquivo que abre; aqui só confere que compila.
    Consulta::nova(&transformar::compor(&fonte, &pedido.transformacao, false))
        .map_err(|e| e.to_string())?;
    let Some(destino) = escolher_destino(&janela, &nome, pedido.saida) else {
        return Ok(None);
    };
    let mut protegidos = protegidos(&app);
    protegidos.push(PathBuf::from(&caminho));
    let tmp = transformar::temporario(&destino, &protegidos).map_err(|e| e.to_string())?;
    let (geracao, cancelar) = doc.salvar.lock().map_err(|e| e.to_string())?.reiniciar();
    let pedido = filho::Salvar {
        fonte,
        transformacao: pedido.transformacao,
        saida: pedido.saida,
        mascara: pedido.mascara,
    };
    std::thread::spawn(move || {
        let avisar = |a: AndamentoConsulta| emitir(&app, "salvando", id, a);
        let sal = (pedido.mascara == Mascara::Anonimizar).then(sal_da_sessao);
        let mut fim = filho::rodar_com(
            "",
            &caminho,
            tmp.path(),
            sal,
            Some(&pedido),
            &cancelar,
            |a| avisar(AndamentoConsulta { geracao, ..a }),
        );
        // Cancelado ou falhou: o temporário some no `drop`, o destino fica como estava.
        if fim.falha.is_none() && !fim.cancelado {
            let cru = pedido.mascara == Mascara::SemMascara;
            match transformar::persistir(tmp, &destino, &protegidos, cru) {
                Ok(()) => fim.arquivo = Some(destino.to_string_lossy().into_owned()),
                Err(e) => fim.falha = Some(e.to_string()),
            }
        }
        fim.geracao = geracao;
        avisar(fim);
    });
    Ok(Some(geracao))
}

/// Cancela o "salvar como" em curso: o filho morre e o temporário some.
#[tauri::command]
fn cancelar_salvar(id: u32, estado: State<'_, Estado>) -> Result<(), String> {
    estado
        .doc(id)?
        .salvar
        .lock()
        .map_err(|e| e.to_string())?
        .cancelar
        .store(true, Relaxed);
    Ok(())
}

/// Copia o JSONL do resultado ou o converte em CSV (`nucleo::csv`, duas passadas no arquivo:
/// em memória só os nomes das colunas). Recusa sobrescrever um dos `protegidos` (o arquivo
/// aberto, o próprio resultado). Grava num temporário na pasta do destino e troca por `rename`:
/// falha no meio deixa o destino que já existia intacto.
fn gravar_resultado(
    origem: &Path,
    destino: &Path,
    csv: bool,
    protegidos: &[PathBuf],
) -> Result<(), String> {
    use std::io::{BufRead, BufReader, BufWriter, Write};
    let erro = |e: std::io::Error| format!("{}: {e}", destino.display());
    let mut tmp = transformar::temporario(destino, protegidos).map_err(|e| e.to_string())?;
    if csv {
        let linhas = |f: &mut dyn FnMut(&[u8]) -> Result<(), String>| -> Result<(), String> {
            let arquivo = std::fs::File::open(origem).map_err(|e| e.to_string())?;
            let (mut r, mut l) = (BufReader::new(arquivo), Vec::new());
            while r.read_until(b'\n', &mut l).map_err(|e| e.to_string())? > 0 {
                if !l.trim_ascii().is_empty() {
                    f(&l)?;
                }
                l.clear();
            }
            Ok(())
        };
        let mut c = nucleo::csv::Csv::default();
        linhas(&mut |l| c.ver(l))?;
        let mut out = BufWriter::new(tmp.as_file_mut());
        c.cabecalho(&mut out).map_err(erro)?;
        linhas(&mut |l| c.escrever(l, &mut out).map_err(erro))?;
        out.flush().map_err(erro)?;
    } else {
        let mut f = std::fs::File::open(origem).map_err(erro)?;
        std::io::copy(&mut f, tmp.as_file_mut()).map_err(erro)?;
    }
    // O resultado da consulta já sai mascarado ou anonimizado.
    transformar::persistir(tmp, destino, protegidos, false).map_err(|e| e.to_string())
}

/// Busca `padrao` no arquivo inteiro numa thread própria, que emite `busca` com o progresso
/// (no máximo a cada `INTERVALO`, e no fim); a busca anterior é cancelada e seus achados somem.
/// Padrão vazio só limpa. Devolve a geração da busca, que vem em cada evento.
#[tauri::command]
fn buscar<R: Runtime>(
    id: u32,
    padrao: String,
    regex: bool,
    ignorar_caixa: bool,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<u64, String> {
    let doc = estado.doc(id)?;
    let arquivo = doc.com(|a| a.compartilhado())?;
    let (geracao, cancelar) = doc.busca.lock().map_err(|e| e.to_string())?.reiniciar();
    if padrao.is_empty() {
        return Ok(geracao);
    }
    let busca = Busca::nova(&padrao, regex, ignorar_caixa).map_err(|e| e.to_string())?;
    let doc = Arc::downgrade(&doc);
    std::thread::spawn(move || {
        let mut aviso = Instant::now();
        let avisar = |achados, varridos, total, fim| {
            let Some(doc) = doc.upgrade() else { return };
            let Ok(b) = doc.busca.lock() else { return };
            if b.geracao != geracao {
                return;
            }
            let guardados = b.achados.len();
            drop(b);
            emitir(
                &app,
                "busca",
                id,
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
            let Some(doc) = doc.upgrade() else {
                cancelar.store(true, Relaxed);
                return;
            };
            if let Ok(mut b) = doc.busca.lock()
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
fn achado(id: u32, i: usize, estado: State<'_, Estado>) -> Result<Option<[usize; 3]>, String> {
    let doc = estado.doc(id)?;
    let b = doc.busca.lock().map_err(|e| e.to_string())?;
    Ok(b.achados.get(i).map(|a| [a.linha, a.coluna, a.comprimento]))
}

/// Achados dos registros `[inicio, inicio + quantidade)`, como em `achado`, mais o índice do
/// primeiro deles: `[indice, [[linha, coluna, comprimento], …]]`. É o realce da tela.
#[tauri::command]
fn achados(
    id: u32,
    inicio: usize,
    quantidade: usize,
    estado: State<'_, Estado>,
) -> Result<(usize, Vec<[usize; 3]>), String> {
    let doc = estado.doc(id)?;
    let b = doc.busca.lock().map_err(|e| e.to_string())?;
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
fn regua_busca(id: u32, k: usize, estado: State<'_, Estado>) -> Result<Vec<usize>, String> {
    let doc = estado.doc(id)?;
    let registros = doc.com(|a| a.registros())?.max(1);
    let k = k.clamp(1, MAX_BALDES);
    let b = doc.busca.lock().map_err(|e| e.to_string())?;
    let mut baldes: Vec<usize> = b.achados.iter().map(|a| a.linha * k / registros).collect();
    baldes.dedup();
    Ok(baldes)
}

/// Estado e comandos; separado de `run` para o teste de IPC usar o mesmo registro.
fn comandos<R: Runtime>(b: Builder<R>) -> Builder<R> {
    b.manage(Estado::default())
        .manage(abertura::Abertura::default())
        .manage(abertura::Concedidos::default())
        .manage(Stdin::default())
        .invoke_handler(tauri::generate_handler![
            escolher,
            salvar_como,
            cancelar_salvar,
            abertura::arquivos_iniciais,
            abertura::arquivos_pedidos,
            recentes,
            limpar_recentes,
            recente_ausente,
            abrir,
            seguir,
            nova_abertura,
            cancelar_abertura,
            baixar_url,
            fechar,
            pastas,
            resumo,
            linhas,
            alternar,
            pai,
            ir_para,
            texto,
            revelar,
            formatado,
            migalhas,
            regua,
            marcas,
            estrutura,
            laudo,
            perfil,
            tabela,
            tabela_linhas,
            ordenar,
            cancelar_ordenacao,
            filtrar,
            cancelar_filtro,
            limpar_filtro,
            filtrado,
            buscar,
            achado,
            achados,
            regua_busca,
            caminhos,
            valor,
            valor_cru,
            abrir_link,
            consultar,
            fechar_consulta,
            exportar,
            cancelar_validacao,
            validar_outro,
            validar_com_schema,
            salvar_exportado,
            salvar_consulta,
            diff_estrutura,
            diff_valores,
            cancelar_diff_valores,
            escolher_pasta,
            linha_do_tempo,
            cancelar_linha_do_tempo
        ])
}

/// Menu do macOS sem "Fechar janela": lá ⌘W fecharia a janela, e no Jasão ⌘W fecha a aba (no JS).
#[cfg(target_os = "macos")]
fn menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{Menu, PredefinedMenuItem as P, Submenu};
    let nome = app.package_info().name.clone();
    let app_menu = Submenu::with_items(
        app,
        &nome,
        true,
        &[
            &P::about(app, None, None)?,
            &P::separator(app)?,
            &P::services(app, None)?,
            &P::separator(app)?,
            &P::hide(app, None)?,
            &P::hide_others(app, None)?,
            &P::show_all(app, None)?,
            &P::separator(app)?,
            &P::quit(app, None)?,
        ],
    )?;
    let editar = Submenu::with_items(
        app,
        "Editar",
        true,
        &[
            &P::undo(app, None)?,
            &P::redo(app, None)?,
            &P::separator(app)?,
            &P::cut(app, None)?,
            &P::copy(app, None)?,
            &P::paste(app, None)?,
            &P::select_all(app, None)?,
        ],
    )?;
    let janela = Submenu::with_items(
        app,
        "Janela",
        true,
        &[
            &P::minimize(app, None)?,
            &P::maximize(app, None)?,
            &P::separator(app)?,
            &P::fullscreen(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&app_menu, &arquivo(app)?, &editar, &janela])
}

/// Arquivo › Abrir recente: um item por recente (id `recente:<caminho>`) e "Limpar recentes".
#[cfg(target_os = "macos")]
fn arquivo<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Submenu<R>> {
    use tauri::menu::{IsMenuItem, MenuItem, PredefinedMenuItem, Submenu};
    let lista = recentes::descrever(com_recentes(app, recentes::ler).unwrap_or_default());
    let mut itens: Vec<MenuItem<R>> = Vec::new();
    for r in &lista {
        let texto = format!("{} — {}", r.nome, r.pasta);
        let id = format!("recente:{}", r.caminho);
        itens.push(MenuItem::with_id(app, id, texto, true, None::<&str>)?);
    }
    if itens.is_empty() {
        itens.push(MenuItem::new(
            app,
            "Nenhum arquivo recente",
            false,
            None::<&str>,
        )?);
    }
    let separador = PredefinedMenuItem::separator(app)?;
    let limpar = MenuItem::with_id(
        app,
        "recentes:limpar",
        "Limpar recentes",
        !lista.is_empty(),
        None::<&str>,
    )?;
    let mut filhos: Vec<&dyn IsMenuItem<R>> = itens.iter().map(|i| i as _).collect();
    filhos.push(&separador);
    filhos.push(&limpar);
    let abrir = Submenu::with_items(app, "Abrir recente", true, &filhos)?;
    Submenu::with_items(app, "Arquivo", true, &[&abrir])
}

/// Escolha no menu Arquivo: recente vai pelo mesmo caminho de um pedido do sistema.
#[cfg(target_os = "macos")]
fn no_menu<R: Runtime>(app: &AppHandle<R>, evento: tauri::menu::MenuEvent) {
    let id = evento.id().as_ref();
    if let Some(caminho) = id.strip_prefix("recente:") {
        abertura::pedir(app, vec![caminho.to_owned()]);
    } else if id == "recentes:limpar" {
        if let Some(Err(e)) = com_recentes(app, recentes::limpar) {
            eprintln!("recentes: {e}");
        }
        recentes_mudaram(app);
    }
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
    let b = comandos(builder);
    #[cfg(target_os = "macos")]
    let b = b.menu(menu).on_menu_event(no_menu);
    b.plugin(tauri_plugin_dialog::init())
        // Arraste: soltar na janela é a concessão. No Tauri 2.12 o evento chega à interface
        // antes deste handler rodar; a ordem não importa porque a interface, ao receber o
        // soltar, chama primeiro `pastas`, comando síncrono que roda na thread principal, a
        // mesma deste handler: só começa depois que ele concedeu. Só então chama `abrir`.
        // Janela com webview manda como evento de janela.
        .on_window_event(|j, e| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = e {
                soltos(j.app_handle(), paths);
            }
        })
        .on_webview_event(|w, e| {
            if let tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = e {
                soltos(w.app_handle(), paths);
            }
        })
        .setup(|app| {
            // Título vem do productName (tauri.conf.json), onde mora o nome do produto.
            let nome = app.package_info().name.clone();
            if let Some(janela) = app.get_webview_window("main") {
                janela.set_title(&nome)?;
            }
            if let Ok(dir) = app.path().app_data_dir() {
                app.manage(Recentes(dir.join("recentes.json")));
                recentes_mudaram(app.handle());
            }
            // Temporário de stdin de execução morta sem limpar (crash, SIGKILL).
            let _ = nucleo::entrada::limpar_orfaos(&nucleo::Opcoes::default());
            let cwd = std::env::current_dir().unwrap_or_default();
            let argv: Vec<String> = std::env::args().collect();
            conceder_argv(app.handle(), &argv, &cwd);
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
            // Não deixa resultado de consulta no diretório temporário.
            tauri::RunEvent::Exit => {
                if let Ok(docs) = app.state::<Estado>().docs.lock() {
                    docs.values().for_each(|d| d.encerrar());
                }
                if let Ok(mut t) = app.state::<Stdin>().0.lock() {
                    t.take();
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod testes {
    #[test]
    fn gravar_resultado_jsonl_e_csv() {
        let d = tempfile::tempdir().unwrap();
        let origem = d.path().join("r.jsonl");
        std::fs::write(&origem, "{\"a\":1,\"b\":\"=x\"}\n\n{\"a\":2,\"c\":{}}\n").unwrap();
        let (jsonl, csv) = (d.path().join("s.jsonl"), d.path().join("s.csv"));
        super::gravar_resultado(&origem, &jsonl, false, &[]).unwrap();
        assert_eq!(
            std::fs::read(&jsonl).unwrap(),
            std::fs::read(&origem).unwrap()
        );
        super::gravar_resultado(&origem, &csv, true, &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&csv).unwrap(),
            ".a,.b,.c\r\n1,'=x,\r\n2,,{}\r\n"
        );
        // Colunas demais: erro sem criar o destino, e o que já existia fica intacto.
        let largo: Vec<String> = (0..=nucleo::csv::MAX_COLUNAS)
            .map(|i| format!("\"k{i}\":0"))
            .collect();
        std::fs::write(&origem, format!("{{{}}}\n", largo.join(","))).unwrap();
        let destino = d.path().join("largo.csv");
        let e = super::gravar_resultado(&origem, &destino, true, &[]).unwrap_err();
        assert!(e.contains("colunas"), "{e}");
        assert!(!destino.exists());
        let e = super::gravar_resultado(&origem, &csv, true, &[]).unwrap_err();
        assert!(e.contains("colunas"), "{e}");
        assert_eq!(
            std::fs::read_to_string(&csv).unwrap(),
            ".a,.b,.c\r\n1,'=x,\r\n2,,{}\r\n"
        );
        // Nenhum temporário sobra na pasta.
        let sobra: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".jasao-salvar-")
            })
            .collect();
        assert!(sobra.is_empty(), "{sobra:?}");
    }

    /// Salvar não sobrescreve o arquivo aberto nem o resultado: pelo caminho, por link
    /// simbólico, por link físico, e o resultado sobre ele mesmo.
    #[test]
    fn gravar_resultado_recusa_protegidos() {
        let d = tempfile::tempdir().unwrap();
        let aberto = d.path().join("aberto.jsonl");
        let resultado = d.path().join("resultado.jsonl");
        std::fs::write(&aberto, "{\"original\":1}\n").unwrap();
        std::fs::write(&resultado, "1\n2\n").unwrap();
        let protegidos = [aberto.clone(), resultado.clone()];
        let recusa = |destino: &std::path::Path, csv: bool| {
            let e = super::gravar_resultado(&resultado, destino, csv, &protegidos).unwrap_err();
            assert!(e.contains("aberto no jasão"), "{e}");
        };
        recusa(&aberto, false);
        recusa(&aberto, true);
        recusa(&resultado, false);
        #[cfg(unix)]
        {
            let link = d.path().join("link.jsonl");
            std::os::unix::fs::symlink(&aberto, &link).unwrap();
            recusa(&link, false);
            let fisico = d.path().join("fisico.jsonl");
            std::fs::hard_link(&aberto, &fisico).unwrap();
            recusa(&fisico, true);
        }
        assert_eq!(
            std::fs::read_to_string(&aberto).unwrap(),
            "{\"original\":1}\n"
        );
        assert_eq!(std::fs::read_to_string(&resultado).unwrap(), "1\n2\n");
        // Outro arquivo qualquer: sobrescreve.
        let outro = d.path().join("outro.jsonl");
        std::fs::write(&outro, "velho").unwrap();
        super::gravar_resultado(&resultado, &outro, false, &protegidos).unwrap();
        assert_eq!(std::fs::read_to_string(&outro).unwrap(), "1\n2\n");
    }

    #[test]
    fn link_so_http_em_ascii() {
        use super::link_valido;
        assert!(link_valido("https://exemplo.test/a?b=1#c"));
        assert!(link_valido("http://xn--exmplo-4of.test/%E2%80%AE"));
        for u in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
            "https:///x",
            "https://a b.test",
            "https://a.test/\"x",
            "https://á.test",
            "-a https://x.test",
            "https://x.test\n",
        ] {
            assert!(!link_valido(u), "{u}");
        }
        assert!(!link_valido(&format!(
            "https://x.test/{}",
            "a".repeat(2048)
        )));
    }

    use super::*;
    use serde_json::{Value, json};
    use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::{InvokeRequest, WebviewWindowBuilder};

    /// Cancelar depois que o núcleo terminou (sort já sem olhar o `cancelar`) descarta a ordem.
    #[test]
    fn cancelar_descarta_ordem_ja_calculada() {
        let arq = Arc::new(nucleo::linhas::ArquivoLinhas::estatico(
            b"{\"n\":2}\n{\"n\":1}\n",
        ));
        let nao = AtomicBool::new(false);
        let inf = nucleo::estrutura::inferir(arq.bytes(), &nao, |_| {});
        let t = Tabela::montar(arq, &inf, &nao).unwrap();
        let mut o = EstadoOrdem::default();
        o.cancelar(); // o pedido de ordenar
        let g = o.geracao;
        let ordem = t.ordenar(0, None, &nao, &|_| {}).unwrap();
        o.cancelar(); // o usuário cancela com a ordem pronta, antes de publicar
        assert_eq!(
            o.publicar(g, ordem.clone(), false, 0),
            Some(FalhaOrdem::Cancelada)
        );
        assert!(o.pronta.is_none());
        assert_eq!(o.publicar(o.geracao, ordem, false, 0), None);
        assert!(o.pronta.is_some());
    }

    /// Mesmo arquivo abrindo em duas abas: cancelar uma não cancela a outra.
    #[test]
    fn cancelar_abertura_e_por_abertura() {
        let a = Aberturas::default();
        let (um, dois) = (a.registrar(1), a.registrar(2));
        a.cancelar(1);
        assert!(um.load(Relaxed) && !dois.load(Relaxed));
        a.soltar(2, &dois);
        a.cancelar(3); // desconhecido: nada
    }

    /// Cancelar entre o `nova_abertura` e o `abrir` não se perde: o `abrir` já nasce cancelado.
    #[test]
    fn cancelar_antes_do_abrir_vale() {
        let a = Aberturas::default();
        let (x, y) = (a.nova(), a.nova());
        assert_ne!(x, y);
        a.cancelar(x);
        assert!(a.registrar(x).load(Relaxed));
        assert!(!a.registrar(y).load(Relaxed));
    }

    /// Abrir URL pelo IPC: `http://` só depois da confirmação, o baixado abre (concedido) e o
    /// erro não traz o segredo do cURL colado.
    #[test]
    fn baixar_url_pelo_ipc() {
        use std::io::{BufRead, BufReader, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let porta = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (status, corpo) in [("200 OK", "{\"a\":1}\n{\"a\":2}\n"), ("404 Not Found", "x")] {
                let (mut s, _) = l.accept().unwrap();
                let mut ler = BufReader::new(&mut s);
                let mut linha = String::new();
                while ler.read_line(&mut linha).unwrap_or(0) > 0 && linha != "\r\n" {
                    linha.clear();
                }
                let r = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
                    corpo.len()
                );
                let _ = s.write_all(r.as_bytes());
            }
        });
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
        let url = format!("http://127.0.0.1:{porta}/x.jsonl");
        assert_eq!(
            chamar("baixar_url", json!({ "entrada": url })),
            Err(json!(ERRO_HTTP))
        );
        let b = chamar("baixar_url", json!({ "entrada": url, "http": true })).unwrap();
        assert_eq!(b["nome"], "x.jsonl");
        let caminho = &b["caminho"];
        let aberto = chamar("abrir", json!({ "caminho": caminho })).unwrap();
        assert_eq!(aberto["total"], 2);
        let curl = format!("curl -H 'Authorization: Bearer seg-77aa' http://127.0.0.1:{porta}/y");
        let e = chamar("baixar_url", json!({ "entrada": curl, "http": true })).unwrap_err();
        assert!(
            e.as_str().unwrap().contains("404") && !e.to_string().contains("seg-77aa"),
            "{e}"
        );
        let e = chamar("baixar_url", json!({ "entrada": "curl -k https://x/" })).unwrap_err();
        assert!(e.to_string().contains("-k"), "{e}");
        let _ = std::fs::remove_file(caminho.as_str().unwrap());
    }

    /// Modo seguir pelo IPC: cresce sem reabrir, espera a linha parcial e reabre ao truncar.
    #[test]
    fn seguir_pelo_ipc() {
        let pasta = tempfile::tempdir().unwrap();
        let log = pasta.path().join("log.jsonl");
        std::fs::write(&log, "{\"i\":0}\n{\"i\":1}\n").unwrap();
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
        let caminho = log.to_str().unwrap();
        let seguir = || chamar("seguir", json!({ "caminho": caminho, "id": 1 }));
        // Sem concessão, nem abre nem segue.
        assert!(chamar("abrir", json!({ "caminho": caminho })).is_err());
        app.state::<abertura::Concedidos>().conceder(&log);
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho })),
            Ok(json!({"id": 1, "total": 2}))
        );
        let acrescentar = |b: &[u8]| {
            std::io::Write::write_all(
                &mut std::fs::OpenOptions::new().append(true).open(&log).unwrap(),
                b,
            )
            .unwrap()
        };
        // 1ª chamada: troca pela cópia (mesmas 2 linhas, nenhuma nova).
        assert_eq!(
            seguir(),
            Ok(json!({"total": 2, "registros": 2, "novas": 0, "reescrito": false}))
        );
        acrescentar(b"{\"i\":2}\n{\"i\":");
        assert_eq!(
            seguir(),
            Ok(json!({"total": 3, "registros": 3, "novas": 1, "reescrito": false}))
        );
        let ultima = chamar("linhas", json!({"id": 1, "inicio": 2, "quantidade": 5})).unwrap();
        assert_eq!(
            ultima.as_array().unwrap().len(),
            1,
            "a linha parcial não entra"
        );
        acrescentar(b"3}\n");
        assert_eq!(seguir().unwrap()["registros"], 4);
        std::fs::write(&log, "{\"novo\":true}\n").unwrap();
        assert_eq!(
            seguir(),
            Ok(json!({"total": 1, "registros": 1, "novas": 0, "reescrito": true}))
        );
        // Pausa (a interface só deixa de chamar) e o arquivo é trocado por outro maior:
        // retomar reabre, não acrescenta à árvore de antes.
        std::fs::write(&log, "{\"z\":1}\n{\"z\":2}\n{\"z\":3}\n").unwrap();
        assert_eq!(
            seguir(),
            Ok(json!({"total": 3, "registros": 3, "novas": 0, "reescrito": true}))
        );
        acrescentar(b"{}\n");
        assert_eq!(seguir().unwrap()["novas"], 1);
        // 1º seguir de arquivo trocado depois do `abrir` (o caso do verificador): reabre.
        std::fs::write(&log, "{\"a\":[1,2,3]}\n{\"i\":1}\n").unwrap();
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho, "id": 1 })),
            Ok(json!({"id": 1, "total": 2}))
        );
        std::fs::write(&log, "{\"z\":1}\n{\"z\":2}\n{\"z\":3}\n{\"z\":4}\n").unwrap();
        assert_eq!(
            seguir(),
            Ok(json!({"total": 4, "registros": 4, "novas": 0, "reescrito": true}))
        );
    }

    /// Os temporários de consulta levam pid e geração (por documento) no nome: dois testes que
    /// consultam no mesmo processo se confundiriam na conferência do temporário da velha.
    static CONSULTAS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Garantia da máscara: nenhum caminho de saída do IPC devolve o valor de um campo sensível
    /// (dados sintéticos). Só `valor_cru`, pedido campo a campo, mostra o original.
    #[test]
    fn segredo_nao_sai_pelo_ipc() {
        use tauri::Listener;
        const SEGREDOS: [&str; 5] = ["S3NT1N3LA", "111.444.777", "4821", "4111", "8T0K3N"];
        let vaza = |v: &Value| {
            let s = v.to_string();
            SEGREDOS
                .iter()
                .find(|x| s.contains(*x))
                .map(|x| format!("{x} em {s}"))
        };
        let mut f = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(
            &mut f,
            concat!(
                r#"{"id":1,"nome":"ana","senha":"S3NT1N3LA-1","cvv":4821,"obs":"111.444.777-35","#,
                r#""pagamento":{"cartao":{"numero":"4111 1111 1111 1111"}}}"#,
                "\n",
                r#"{"id":2,"nome":"bia","senha":"S3NT1N3LA-2","cvv":4821,"obs":"ok","#,
                r#""pagamento":{"cartao":{"numero":"4111 1111 1111 1111"}}}"#,
                "\n",
                r#"{"id":3,"token":"8T0K3N" oops}"#,
                "\n",
            )
            .as_bytes(),
        )
        .unwrap();
        let app = comandos(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let janela = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let pedido = |cmd: &str, args: Value| InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        let chamar = |cmd: &str, args: Value| -> Result<Value, Value> {
            get_ipc_response(&janela, pedido(cmd, args)).map(|b| b.deserialize::<Value>().unwrap())
        };
        // Toda resposta passa pelo crivo, inclusive as de erro.
        let em = |cmd: &str, mut args: Value| {
            args["id"] = json!(1);
            let r = chamar(cmd, args);
            let v = r.clone().unwrap_or_else(|e| e);
            assert_eq!(vaza(&v), None, "{cmd}");
            r
        };
        app.state::<abertura::Concedidos>().conceder(f.path());
        let caminho = f.path().to_str().unwrap();
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho })),
            Ok(json!({"id": 1, "total": 3}))
        );
        (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                em("resumo", json!({})).unwrap()
            })
            .find(|r| r["coloriu"] == true)
            .expect("varredura não terminou");

        // Árvore toda aberta: prévias, valor e copiar; a linha marcada revela pelo valor_cru.
        let mut linhas = em("linhas", json!({"inicio": 0, "quantidade": 99})).unwrap();
        while let Some(v) = linhas
            .as_array()
            .unwrap()
            .iter()
            .position(|l| l["expansivel"] == true && l["aberto"] == false)
        {
            em("alternar", json!({ "v": v })).unwrap();
            linhas = em("linhas", json!({"inicio": 0, "quantidade": 99})).unwrap();
        }
        let linhas = linhas.as_array().unwrap().clone();
        let mut revelados = Vec::new();
        for (v, l) in linhas.iter().enumerate() {
            em("valor", json!({ "v": v })).unwrap();
            em("formatado", json!({ "registro": l["registro"] })).ok();
            let cru = chamar("valor_cru", json!({"id": 1, "v": v})).unwrap();
            if l["expansivel"] == true || !l["erro"].is_null() {
                // Contêiner e linha inválida não revelam nada: só folha, campo a campo.
                assert_eq!(cru, Value::Null, "{l}");
            } else if l["mascarado"] == true {
                revelados.push(cru);
            } else {
                assert_eq!(
                    chamar("valor_cru", json!({"id": 1, "v": v})),
                    em("valor", json!({ "v": v }))
                );
            }
        }
        // O que a máscara escondeu é exatamente o que vazaria: prova que o crivo não é vazio.
        assert_eq!(
            revelados.iter().collect::<Vec<_>>(),
            [
                json!("S3NT1N3LA-1"),
                json!("4821"),
                json!("111.444.777-35"),
                json!("4111 1111 1111 1111"),
                json!("S3NT1N3LA-2"),
                json!("4821"),
                json!("4111 1111 1111 1111"),
            ]
            .iter()
            .collect::<Vec<_>>()
        );
        match get_ipc_response(
            &janela,
            pedido("texto", json!({"id": 1, "inicio": 0, "quantidade": 9})),
        ) {
            Ok(InvokeResponseBody::Raw(b)) => {
                let t = String::from_utf8(b).unwrap();
                assert_eq!(vaza(&json!(t)), None);
                assert!(t.contains("\"ana\""), "{t}");
            }
            outro => panic!("esperava bytes, veio {outro:?}"),
        }

        // Estrutura, perfil de cada caminho, tabela.
        let e = em("estrutura", json!({})).unwrap();
        for i in 0..e["caminhos"].as_array().unwrap().len() {
            em("perfil", json!({ "indice": i })).unwrap();
        }
        let t = em("tabela", json!({})).unwrap();
        let colunas: Vec<usize> = (0..t["colunas"].as_array().unwrap().len()).collect();
        em(
            "tabela_linhas",
            json!({"inicio": 0, "quantidade": 9, "colunas": colunas}),
        )
        .unwrap();

        // Exportar com literais: o valor sensível não vira exemplo nem discriminador.
        let (tx, rx) = std::sync::mpsc::channel::<Value>();
        let tx2 = tx.clone();
        app.listen_any("validacao", move |e| {
            let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
        });
        let fim = |geracao: &Value| {
            std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(5)).ok())
                .find(|v| v["fim"] == true && &v["geracao"] == geracao)
                .expect("não terminou")
        };
        for formato in ["schema", "zod", "ts"] {
            let r = em(
                "exportar",
                json!({"pedido": {"formato": formato, "literal": true, "numero": true}}),
            )
            .unwrap();
            assert_eq!(vaza(&fim(&r["geracao"])), None, "{formato}");
        }
        // Validar contra schema de fora: a falha diz onde, não o valor.
        let mut s = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(
            &mut s,
            br#"{"properties":{"senha":{"type":"number"},"obs":{"enum":["x"]}}}"#,
        )
        .unwrap();
        let v = validar_com_schema_em(app.handle().clone(), &app.state::<Estado>(), 1, s.path())
            .unwrap();
        let r = fim(&json!(v.geracao));
        assert_eq!(r["falham"], json!(2), "{r}");
        assert_eq!(vaza(&r), None);

        // Busca roda sobre o mascarado: o segredo não acha nada, o nome acha.
        app.listen_any("busca", move |e| {
            let _ = tx2.send(serde_json::from_str(e.payload()).unwrap());
        });
        for (termo, esperado) in [("S3NT1N3LA", 0), ("4111", 0), ("8T0K3N", 0), ("bia", 1)] {
            let g = em(
                "buscar",
                json!({"padrao": termo, "regex": false, "ignorarCaixa": true}),
            )
            .unwrap();
            assert_eq!(fim(&g)["achados"], json!(esperado), "{termo}");
        }

        // Consulta jq: resultado mascarado por padrão; anonimizado troca por código estável.
        let (tx, rx) = std::sync::mpsc::channel::<Value>();
        app.listen("consulta", move |e| {
            let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
        });
        let _so_um = CONSULTAS.lock().unwrap_or_else(|e| e.into_inner());
        let saidas = tempfile::tempdir().unwrap();
        let consulta_de = |filtro: &str, anonimizar: bool| {
            em(
                "consultar",
                json!({"filtro": filtro, "caminho": caminho, "anonimizar": anonimizar}),
            )
            .unwrap();
            let a = std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(5)).ok())
                .find(|a| a["fim"] == true)
                .expect("consulta não terminou");
            assert_eq!(vaza(&a), None);
            let origem = Path::new(a["arquivo"].as_str().unwrap());
            let saida = std::fs::read_to_string(origem).unwrap();
            assert_eq!(vaza(&json!(saida)), None);
            // Exportar o resultado (o que `salvar_consulta` grava depois do diálogo).
            for csv in [false, true].into_iter().filter(|_| filtro == ".") {
                let destino = saidas.path().join(if csv { "r.csv" } else { "r.jsonl" });
                gravar_resultado(origem, &destino, csv, &[]).unwrap();
                let gravado = std::fs::read_to_string(&destino).unwrap();
                assert!(gravado.contains("ana"), "exportou vazio: {gravado}");
                assert_eq!(vaza(&json!(gravado)), None);
            }
            saida
        };
        let consulta = |anonimizar: bool| {
            consulta_de(".", anonimizar);
            consulta_de(".senha", anonimizar)
        };
        assert_eq!(consulta(false), "\"***********\"\n\"***********\"\n");
        let anon = consulta(true);
        let codigos: Vec<&str> = anon.lines().collect();
        assert!(
            codigos[0].starts_with("\"anon:") && codigos[0] != codigos[1],
            "{anon}"
        );
        assert_eq!(
            consulta(true),
            anon,
            "o sal é da sessão: mesmo valor, mesmo código"
        );
        assert_eq!(em("fechar_consulta", json!({})), Ok(Value::Null));
    }

    /// "Salvar como" pelo IPC: recusa o original, máscara por padrão (anonimizar e sem máscara
    /// só pedidos), cada fonte e transformação, e cancelar sem resto na pasta.
    #[test]
    fn salvar_como_pelo_ipc() {
        use tauri::Listener;
        let _so_um = CONSULTAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let original = d.path().join("dados.jsonl");
        let conteudo = concat!(
            r#"{"tipo":"a","id":1,"senha":"S3NT1N3LA-1","nome":"ana"}"#,
            "\n",
            r#"{"tipo":"b","id":2,"senha":"S3NT1N3LA-2","nome":"bia","b":1}"#,
            "\n",
            r#"{"tipo":"a","id":3,"senha":"S3NT1N3LA-3","nome":"cid"}"#,
            "\n",
        );
        std::fs::write(&original, conteudo).unwrap();
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
        let (tx, rx) = std::sync::mpsc::channel::<Value>();
        app.listen("salvando", move |e| {
            let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
        });
        let fim = || loop {
            let a = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("salvar não terminou");
            if a["fim"] == true {
                break a;
            }
        };
        app.state::<abertura::Concedidos>().conceder(&original);
        let caminho = original.to_str().unwrap();
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho })),
            Ok(json!({"id": 1, "total": 3}))
        );
        let salvar = |destino: &std::path::Path, pedido: Value| {
            *DESTINO_TESTE.lock().unwrap() = Some(destino.to_owned());
            chamar(
                "salvar_como",
                json!({"id": 1, "caminho": caminho, "pedido": pedido, "nome": "dados"}),
            )
        };
        // Espera a inferência (a fonte "filtro" precisa dela).
        let alvo = json!({"tipo": "filtro", "alvo": {"discriminador": 0, "variante": 0}});
        let mut ultimo = None;
        (0..300)
            .find(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                *DESTINO_TESTE.lock().unwrap() = None; // diálogo fechado: não grava
                let r = chamar(
                    "salvar_como",
                    json!({"id": 1, "caminho": caminho, "pedido": {"fonte": alvo}, "nome": "x"}),
                );
                let ok = r == Ok(Value::Null);
                ultimo = Some(r);
                ok
            })
            .unwrap_or_else(|| panic!("inferência não terminou: {ultimo:?}"));
        let gravar = |nome: &str, pedido: Value| {
            let destino = d.path().join(nome);
            salvar(&destino, pedido).unwrap();
            let a = fim();
            assert_eq!(a["falha"], Value::Null, "{a}");
            assert_eq!(a["arquivo"], json!(destino.to_str().unwrap()));
            std::fs::read_to_string(&destino).unwrap()
        };

        // O original (e link para ele) é recusado antes de gravar qualquer coisa.
        let e = salvar(&original, json!({"fonte": {"tipo": "tudo"}})).unwrap_err();
        assert!(e.as_str().unwrap().contains("aberto no jasão"), "{e}");
        #[cfg(unix)]
        {
            let link = d.path().join("link.jsonl");
            std::os::unix::fs::symlink(&original, &link).unwrap();
            assert!(salvar(&link, json!({"fonte": {"tipo": "tudo"}})).is_err());
        }
        assert_eq!(std::fs::read_to_string(&original).unwrap(), conteudo);

        // Máscara por padrão; anonimizar e sem máscara só quando pedidos.
        let mascarado = gravar("m.jsonl", json!({"fonte": {"tipo": "tudo"}}));
        assert!(!mascarado.contains("S3NT1N3LA"), "{mascarado}");
        assert_eq!(mascarado.lines().count(), 3);
        assert!(mascarado.contains("\"nome\":\"ana\""), "{mascarado}");
        let anon = gravar(
            "a.jsonl",
            json!({"fonte": {"tipo": "tudo"}, "mascara": "anonimizar"}),
        );
        assert!(
            !anon.contains("S3NT1N3LA") && anon.contains("anon:"),
            "{anon}"
        );
        let cru = gravar(
            "c.jsonl",
            json!({"fonte": {"tipo": "tudo"}, "mascara": "sem_mascara"}),
        );
        assert_eq!(cru, conteudo);
        // Sem máscara fica só do dono; mascarado, 0644.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let modo = |n: &str| {
                std::fs::metadata(d.path().join(n))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            };
            assert_eq!((modo("c.jsonl"), modo("m.jsonl")), (0o600, 0o644));
        }

        // Erro do jq num registro: grava o resto e devolve a contagem e o primeiro.
        *DESTINO_TESTE.lock().unwrap() = Some(d.path().join("e.jsonl"));
        let filtro = "if .id == 2 then error(\"ruim\") else .id end";
        salvar(
            &d.path().join("e.jsonl"),
            json!({"fonte": {"tipo": "jq", "filtro": filtro}}),
        )
        .unwrap();
        let a = fim();
        assert_eq!((&a["falha"], &a["erros"]), (&Value::Null, &json!(1)), "{a}");
        assert_eq!(a["primeiro_erro"][0], json!(1), "{a}");
        assert_eq!(
            std::fs::read_to_string(d.path().join("e.jsonl")).unwrap(),
            "1\n3\n"
        );
        // Renomear ambíguo: recusado antes do diálogo.
        let e = salvar(
            &d.path().join("r.jsonl"),
            json!({"fonte": {"tipo": "tudo"},
                   "transformacao": {"renomear": [["id", "x"], ["nome", "x"]]}}),
        )
        .unwrap_err();
        assert!(e.as_str().unwrap().contains("dois campos viram"), "{e}");
        assert!(!d.path().join("r.jsonl").exists());

        // Fonte jq + transformações, em JSON formatado.
        let t = gravar(
            "t.json",
            json!({
                "fonte": {"tipo": "jq", "filtro": "select(.id > 1)"},
                "transformacao": {"tirar": ["senha"], "renomear": [["nome", "quem"]]},
                "saida": "json",
            }),
        );
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(
            v,
            json!([{"tipo":"b","id":2,"quem":"bia","b":1}, {"tipo":"a","id":3,"quem":"cid"}])
        );
        assert!(t.contains("\n  {"), "{t}");
        let manter = gravar(
            "k.jsonl",
            json!({"fonte": {"tipo": "tudo"}, "transformacao": {"manter": ["id"]}}),
        );
        assert_eq!(manter, "{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n");
        // Fonte filtro (#49): só a variante pedida, todos os registros dela.
        let f = gravar(
            "f.jsonl",
            json!({"fonte": alvo, "transformacao": {"manter": ["tipo"]}, "saida": "json_compacto"}),
        );
        let f: Value = serde_json::from_str(&f).unwrap();
        let tipo = &f[0]["tipo"];
        let n = conteudo.matches(&format!("\"tipo\":{tipo}")).count();
        assert_eq!(f.as_array().map(|l| l.len()), Some(n), "{f}");
        assert!(f.as_array().unwrap().iter().all(|r| &r["tipo"] == tipo));
        // Original JSON único com outro arquivo (JSONL) na aba, como o resultado de uma consulta:
        // a transformação vale por registro do original, não pelo formato do que está aberto.
        let unico = d.path().join("unico.json");
        std::fs::write(&unico, r#"[{"id":1,"senha":"x"},{"id":2,"senha":"y"}]"#).unwrap();
        app.state::<abertura::Concedidos>().conceder(&unico);
        *DESTINO_TESTE.lock().unwrap() = Some(d.path().join("u.jsonl"));
        chamar(
            "salvar_como",
            json!({"id": 1, "caminho": unico.to_str().unwrap(), "nome": "unico",
                   "pedido": {"fonte": {"tipo": "tudo"}, "transformacao": {"tirar": ["senha"]}}}),
        )
        .unwrap();
        assert_eq!(fim()["falha"], Value::Null);
        assert_eq!(
            std::fs::read_to_string(d.path().join("u.jsonl")).unwrap(),
            "[{\"id\":1},{\"id\":2}]\n"
        );

        // Cancelar: nada no destino nem temporário na pasta.
        let destino = d.path().join("cancelado.jsonl");
        let lento = "[limit(1e7; repeat(1))] | length";
        salvar(&destino, json!({"fonte": {"tipo": "jq", "filtro": lento}})).unwrap();
        chamar("cancelar_salvar", json!({"id": 1})).unwrap();
        let a = fim();
        assert_eq!(
            (&a["cancelado"], &a["arquivo"]),
            (&json!(true), &Value::Null),
            "{a}"
        );
        assert!(!destino.exists());
        let resto: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".jasao-salvar-"))
            .collect();
        assert_eq!(resto, Vec::<String>::new());
        *DESTINO_TESTE.lock().unwrap() = None;
    }

    /// Os nomes de comando e de argumento são os que `src/main.ts` manda.

    #[test]
    fn comandos_pelo_ipc() {
        let _so_um = CONSULTAS.lock().unwrap_or_else(|e| e.into_inner());
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

        // Cada comando leva o id do documento (a aba); sem documento aberto, erro legível.
        let em = |id: u32, cmd: &str, mut args: Value| {
            args["id"] = json!(id);
            chamar(cmd, args)
        };
        // Os arquivos do teste entram como se o usuário os tivesse escolhido no diálogo.
        let conceder = |p: &Path| app.state::<abertura::Concedidos>().conceder(p);
        conceder(f.path());
        conceder(tipos.path());
        let erro = em(1, "linhas", json!({"inicio": 0, "quantidade": 10})).unwrap_err();
        assert!(erro.as_str().unwrap().contains("não está aberto"), "{erro}");
        assert!(em(1, "estrutura", json!({})).is_err());
        assert!(em(1, "laudo", json!({})).is_err());
        let caminho = f.path().to_str().unwrap();
        let abertura = chamar("nova_abertura", json!({})).unwrap();
        assert_ne!(abertura, chamar("nova_abertura", json!({})).unwrap());
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho, "abertura": abertura })),
            Ok(json!({"id": 1, "total": 2}))
        );
        assert_eq!(
            chamar("cancelar_abertura", json!({ "abertura": abertura })),
            Ok(Value::Null)
        );
        // A varredura roda em outra thread: espera o fim pelo próprio comando.
        let resumo = (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                em(1, "resumo", json!({})).unwrap()
            })
            .find(|r| r["coloriu"] == true)
            .expect("varredura não terminou");
        assert_eq!(
            resumo,
            json!({"conteudo": "objetos", "bytes": 22, "registros": 2, "geracao": 1,
                   "varridas": 2, "total": 2, "invalidas": 1, "fim": true, "cancelado": false,
                   "campos": 1, "legenda": null, "coloriu": true, "inferiu": true,
                   "tabelou": true})
        );
        // Estrutura: a linha inválida não vira registro; `.a[]` com a 1ª linha de cada tipo.
        assert_eq!(
            em(1, "estrutura", json!({})),
            Ok(
                json!({"entrada": "jsonl", "registros": 1, "invalidas": 1, "caminhos": [
                {"caminho": ".", "relacao": "registro", "profundidade": 0, "presenca": 1,
                 "de": 1, "tipos": [["objeto", 1, 0]], "avisos": []},
                {"caminho": ".a", "relacao": "campo", "profundidade": 1, "presenca": 1,
                 "de": 1, "tipos": [["lista", 1, 0]], "avisos": []},
                {"caminho": ".a[]", "relacao": "elemento", "profundidade": 2, "presenca": 2,
                 "de": 1, "tipos": [["inteiro", 2, 0]], "avisos": []}
            ], "discriminadores": []})
            )
        );
        // Laudo: sai da mesma inferência que a Estrutura, junto com `inferiu`.
        let l = em(1, "laudo", json!({})).unwrap();
        assert_eq!(
            (
                &l["conteudo"],
                &l["bytes"],
                &l["registros"],
                &l["invalidas"],
                &l["primeira_invalida"]
            ),
            (
                &json!("objetos"),
                &json!(22),
                &json!(1),
                &json!(1),
                &json!(1)
            )
        );
        // Exportar: devolve o código e valida numa thread, que avisa o fim pelo evento `validacao`.
        {
            use tauri::Listener;
            let (tx, rx) = std::sync::mpsc::channel::<Value>();
            app.listen_any("validacao", move |e| {
                let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
            });
            let pedido = |f| json!({"pedido": {"formato": f, "literal": false, "numero": true}});
            let fim_de = |geracao: &Value| {
                std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(5)).ok())
                    .find(|v| v["fim"] == true && &v["geracao"] == geracao)
                    .expect("validação não terminou")
            };
            let contagem = |v: &Value| {
                (
                    v["id"].clone(),
                    v["passam"].clone(),
                    v["falham"].clone(),
                    v["invalidas"].clone(),
                )
            };
            for (formato, marca) in [
                ("schema", "2020-12"),
                ("zod", "z.object"),
                ("ts", "export type Registro = {"),
            ] {
                let r = em(1, "exportar", pedido(formato)).unwrap();
                let codigo = r["codigo"].as_str().unwrap();
                assert!(codigo.contains(marca), "{codigo}");
                let fim = fim_de(&r["geracao"]);
                assert_eq!(
                    contagem(&fim),
                    (json!(1), json!(1), json!(0), json!(1)),
                    "{fim}"
                );
            }
            // Outro arquivo contra a estrutura deste: nenhum registro de `tipos` tem `.a`.
            let ts = serde_json::from_value::<PedidoExportar>(pedido("ts")["pedido"].clone());
            let v = validar_outro_em(
                app.handle().clone(),
                &app.state::<Estado>(),
                1,
                ts.unwrap(),
                tipos.path(),
            )
            .unwrap();
            let fim = fim_de(&json!(v.geracao));
            assert_eq!(
                contagem(&fim),
                (json!(1), json!(0), json!(3), json!(1)),
                "{fim}"
            );
            assert!(
                fim["falhas"][0][1].as_str().unwrap().contains("\"a\""),
                "{fim}"
            );
            // Este arquivo contra um schema de fora: `.a` precisa ser texto, e é lista.
            let schema = |conteudo: &str| {
                let mut s = tempfile::NamedTempFile::new().unwrap();
                std::io::Write::write_all(&mut s, conteudo.as_bytes()).unwrap();
                s
            };
            let fora = schema(r#"{"properties":{"a":{"type":"string"}}}"#);
            let com_schema = |s: &tempfile::NamedTempFile| {
                validar_com_schema_em(app.handle().clone(), &app.state::<Estado>(), 1, s.path())
            };
            let v = com_schema(&fora).unwrap();
            let fim = fim_de(&json!(v.geracao));
            assert_eq!(
                contagem(&fim),
                (json!(1), json!(0), json!(1), json!(1)),
                "{fim}"
            );
            // Schema hostil volta como erro, antes de validar qualquer coisa.
            for hostil in [
                r#"{"$ref":"https://exemplo.invalid/s.json"}"#,
                "{\"items\":",
            ] {
                let e = com_schema(&schema(hostil)).err().unwrap();
                assert!(e.starts_with("schema recusado: "), "{e}");
            }
            assert_eq!(em(1, "cancelar_validacao", json!({})), Ok(Value::Null));
        }
        assert_eq!(
            em(1, "marcas", json!({"inicio": 0, "quantidade": 9})),
            Ok(json!([0, 255]))
        );
        // Tabela: colunas do topo, célula composta resumida, linha inválida marcada.
        let t = em(1, "tabela", json!({})).unwrap();
        assert_eq!(t["registros"], 2);
        assert_eq!(
            t["colunas"],
            json!([{"nome": "a", "caminho": ".a", "presenca": 1, "tipos": ["lista"]}])
        );
        assert_eq!(t["indisponivel"], Value::Null);
        let tl = em(
            1,
            "tabela_linhas",
            json!({"inicio": 0, "quantidade": 9, "colunas": [0]}),
        )
        .unwrap();
        assert_eq!(
            tl[0]["celulas"],
            json!([{"tipo": "lista", "texto": "", "filhos": 2, "byte": 5}])
        );
        assert_eq!(tl[1]["erro"], json!({"byte": 8, "motivo": "literal"}));
        assert_eq!(
            em(1, "ordenar", json!({"coluna": 0, "desc": true})).unwrap()["pronta"],
            false
        );
        let ordem = (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                em(1, "tabela", json!({})).unwrap()["ordem"].clone()
            })
            .find(|o| !o.is_null())
            .expect("ordenação não terminou");
        assert_eq!(ordem, json!({"coluna": 0, "desc": true}));
        assert_eq!(
            em(1, "ordenar", json!({"coluna": null, "desc": false})).unwrap()["pronta"],
            true
        );
        // Cancelar sobe a geração (descarta o resultado em voo) e liga a bandeira do núcleo.
        let ordem_de = || {
            let doc = app.state::<Estado>().doc(1).unwrap();
            let o = doc.ordem.lock().unwrap();
            (o.geracao, Arc::clone(&o.cancelar))
        };
        let (antes, bandeira) = ordem_de();
        bandeira.store(false, Relaxed);
        assert_eq!(em(1, "cancelar_ordenacao", json!({})), Ok(Value::Null));
        assert_eq!(ordem_de().0, antes + 1);
        assert!(bandeira.load(Relaxed));
        assert!(em(1, "ordenar", json!({"coluna": 5, "desc": false})).is_err());
        assert_eq!(em(1, "alternar", json!({"v": 0})), Ok(json!(3)));
        let linhas = em(1, "linhas", json!({"inicio": 0, "quantidade": 10})).unwrap();
        assert_eq!(linhas[1]["chave"], "a");
        assert_eq!(linhas[1].get("caminho"), None, "caminho sai sob demanda");
        assert_eq!(em(1, "migalhas", json!({"v": 1})), Ok(json!(["a"])));
        assert_eq!(linhas[2]["erro"], json!({"byte": 8, "motivo": "literal"}));
        assert_eq!(em(1, "pai", json!({"v": 1})), Ok(json!(0)));
        assert_eq!(em(1, "ir_para", json!({"registro": 1})), Ok(json!(2)));
        assert_eq!(em(1, "ir_para", json!({"registro": 9})), Ok(Value::Null));

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
            pedido("texto", json!({"id": 1, "inicio": 0, "quantidade": 9})),
        );
        match texto {
            Ok(InvokeResponseBody::Raw(b)) => {
                assert_eq!(b, b"11 9\n{\"a\":[1,2]}\n{\"b\":tru}")
            }
            outro => panic!("esperava bytes, veio {outro:?}"),
        }
        assert_eq!(em(1, "alternar", json!({"v": 0})), Ok(json!(2)));
        assert_eq!(
            em(1, "revelar", json!({"registro": 0, "byte": 7})),
            Ok(json!([3, 5]))
        );
        assert_eq!(
            em(1, "revelar", json!({"registro": 9, "byte": 0})),
            Ok(Value::Null)
        );
        assert_eq!(
            em(1, "formatado", json!({"registro": 0})),
            Ok(
                json!({"texto": "{\n  \"a\": [\n    1,\n    2\n  ]\n}", "truncado": false, "teto": MAX_FORMATADO})
            )
        );
        assert!(em(1, "formatado", json!({"registro": 1})).is_err());

        // Régua: discriminador `.tipo`, cor por frequência, inválida marcada. Segundo documento.
        let caminho = tipos.path().to_str().unwrap();
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho })),
            Ok(json!({"id": 2, "total": 4}))
        );
        let resumo = (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                em(2, "resumo", json!({})).unwrap()
            })
            .find(|r| r["coloriu"] == true)
            .expect("2ª passada não terminou");
        assert_eq!(
            resumo["legenda"],
            json!({"campo": ".tipo", "variantes": [["x", 2], ["y", 1]], "outros": 0})
        );
        assert_eq!(
            em(2, "marcas", json!({"inicio": 0, "quantidade": 9})),
            Ok(json!([1, 2, 1, 255]))
        );
        assert_eq!(
            em(2, "regua", json!({"k": 2})),
            Ok(json!({"cores": [1, 1], "invalidas": [[1, 1, 3]]}))
        );
        // Cada aba tem a sua: a do documento 1 não mudou com o 2.
        assert_eq!(em(1, "estrutura", json!({})).unwrap()["registros"], 1);
        let e = em(2, "estrutura", json!({})).unwrap();
        assert_eq!(
            e["discriminadores"],
            json!([{"campo": ".tipo", "variantes": [
                {"valor": "x", "registros": 2, "linha": 0, "obrigatorias": ["tipo", "x"], "opcionais": []},
                {"valor": "y", "registros": 1, "linha": 1, "obrigatorias": ["tipo", "y"], "opcionais": []}
            ]}])
        );
        let l = em(2, "laudo", json!({})).unwrap();
        assert_eq!(
            l["discriminador"],
            json!({"campo": ".tipo", "variantes": [
                {"valor": "x", "registros": 2, "linha": 0},
                {"valor": "y", "registros": 1, "linha": 1}
            ], "mais": 0})
        );
        // `.x` opcional: em 2 dos 3 objetos.
        assert_eq!(e["caminhos"][2]["caminho"], ".x");
        assert_eq!(
            (
                e["caminhos"][2]["presenca"].clone(),
                e["caminhos"][2]["de"].clone()
            ),
            (json!(2), json!(3))
        );
        assert_eq!(e["caminhos"][2]["avisos"], json!([]));
        // Perfil de `.x`: varre o arquivo; linhas com e sem o campo.
        let p = em(2, "perfil", json!({"indice": 2})).unwrap();
        assert_eq!(
            (
                p["com"].clone(),
                p["linhas_sem"].clone(),
                p["oculto"].clone()
            ),
            (json!(2), json!([1]), json!(false))
        );
        assert!(em(2, "perfil", json!({"indice": 99})).is_err());

        // Filtro: varre em 2º plano e avisa o fim por `filtragem`; a Tabela passa a só ter os
        // filtrados, e a navegação da Árvore acha cada um.
        {
            use tauri::Listener;
            let (tx, rx) = std::sync::mpsc::channel::<Value>();
            app.listen_any("filtragem", move |e| {
                let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
            });
            let filtrar = |alvo: Value| {
                let g = em(2, "filtrar", json!({ "alvo": alvo })).unwrap();
                std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(5)).ok())
                    .find(|v| v["fim"] == true && v["geracao"] == g)
                    .expect("filtro não terminou")
            };
            let registros = || {
                em(
                    2,
                    "tabela_linhas",
                    json!({"inicio": 0, "quantidade": 9, "colunas": [0]}),
                )
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["registro"].as_u64().unwrap())
                .collect::<Vec<_>>()
            };
            assert_eq!(registros(), [0, 1, 2, 3]);
            let fim = filtrar(json!({"discriminador": 0, "variante": 0}));
            assert_eq!(
                (fim["achados"].clone(), fim["falha"].clone()),
                (json!(2), Value::Null)
            );
            assert_eq!(registros(), [0, 2]);
            assert_eq!(
                em(2, "tabela", json!({})).unwrap()["filtro"],
                json!({"achados": 2, "caminho": ".tipo", "tem": null, "valor": "x", "comum": null, "faixa": null})
            );
            assert_eq!(
                em(2, "filtrado", json!({"posicao": 1})),
                Ok(json!({"posicao": 1, "total": 2, "linha": 2, "byte": 0}))
            );
            assert_eq!(em(2, "filtrado", json!({"posicao": 2})), Ok(Value::Null));
            // Ordenar com filtro ordena só os filtrados; filtro novo derruba essa ordem.
            em(2, "ordenar", json!({"coluna": 0, "desc": true})).unwrap();
            (0..200)
                .map(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    em(2, "tabela", json!({})).unwrap()["ordem"].clone()
                })
                .find(|o| !o.is_null())
                .expect("ordenação não terminou");
            assert_eq!(registros().len(), 2);
            let fim = filtrar(json!({"caminho": 2, "tem": false}));
            assert_eq!(fim["achados"], 1);
            assert_eq!(em(2, "tabela", json!({})).unwrap()["ordem"], Value::Null);
            assert_eq!(registros(), [1], "sem `.x`; a inválida não entra");
            // Valor dos mais comuns (token cru) e faixa de número do perfil.
            let fim = filtrar(json!({"caminho": 2, "comum": "2"}));
            assert_eq!(fim["achados"], 1);
            assert_eq!(registros(), [2]);
            assert_eq!(
                em(2, "tabela", json!({})).unwrap()["filtro"],
                json!({"achados": 1, "caminho": ".x", "tem": null, "valor": null, "comum": "2", "faixa": null})
            );
            let fim = filtrar(json!({"caminho": 2, "negativo": false, "expoente": 0}));
            assert_eq!(fim["achados"], 2);
            assert_eq!(registros(), [0, 2]);
            assert_eq!(
                em(2, "tabela", json!({})).unwrap()["filtro"]["faixa"],
                json!([false, 0])
            );
            assert!(em(2, "filtrar", json!({"alvo": {"caminho": 99, "comum": "2"}})).is_err());
            let fim = filtrar(json!({"caminho": 2, "tem": false}));
            assert_eq!(fim["achados"], 1);
            // Teto no call site: com o do documento baixado, recusa e não publica nada.
            app.state::<Estado>()
                .doc(2)
                .unwrap()
                .filtro
                .lock()
                .unwrap()
                .teto = 1;
            let fim = filtrar(json!({"caminho": 2, "tem": true}));
            assert_eq!(
                (fim["achados"].clone(), fim["falha"].clone()),
                (json!(0), json!("grande"))
            );
            assert_eq!(registros(), [1], "o filtro anterior segue valendo");
            app.state::<Estado>()
                .doc(2)
                .unwrap()
                .filtro
                .lock()
                .unwrap()
                .teto = filtro::MAX_FILTRADOS;
            assert!(em(2, "filtrar", json!({"alvo": {"caminho": 99, "tem": true}})).is_err());
            assert!(
                em(
                    2,
                    "filtrar",
                    json!({"alvo": {"discriminador": 0, "variante": 9}})
                )
                .is_err()
            );
            assert_eq!(em(2, "limpar_filtro", json!({})), Ok(Value::Null));
            assert_eq!(registros(), [0, 1, 2, 3]);
            assert_eq!(em(2, "tabela", json!({})).unwrap()["filtro"], Value::Null);
            assert_eq!(em(2, "filtrado", json!({"posicao": 0})), Ok(Value::Null));
        }

        // Copiar: caminho nos três formatos e valor (texto decodificado).
        assert_eq!(em(2, "alternar", json!({"v": 0})), Ok(json!(6)));
        assert_eq!(
            em(2, "caminhos", json!({"v": 2})),
            Ok(json!({"jq": ".x", "jsonpath": "$.x", "pointer": "/x"}))
        );
        assert_eq!(em(2, "valor", json!({"v": 1})), Ok(json!("x")));
        assert_eq!(em(2, "valor", json!({"v": 2})), Ok(json!("1")));
        assert_eq!(em(2, "caminhos", json!({"v": 9})), Ok(Value::Null));

        // Busca: roda em outra thread; os achados aparecem no estado.
        let buscar = |padrao: &str, regex: bool| {
            em(
                2,
                "buscar",
                json!({"padrao": padrao, "regex": regex, "ignorarCaixa": true}),
            )
        };
        assert!(buscar("(", true).is_err());
        assert_eq!(buscar("X", false), Ok(json!(3)));
        let achados = (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                em(2, "achados", json!({"inicio": 0, "quantidade": 9})).unwrap()
            })
            .find(|a| a[1].as_array().unwrap().len() == 4)
            .expect("busca não terminou");
        assert_eq!(
            achados,
            json!([0, [[0, 9, 1], [0, 13, 1], [2, 9, 1], [2, 13, 1]]])
        );
        assert_eq!(em(2, "achado", json!({"i": 2})), Ok(json!([2, 9, 1])));
        assert_eq!(em(2, "achado", json!({"i": 4})), Ok(Value::Null));
        assert_eq!(
            em(2, "achados", json!({"inicio": 1, "quantidade": 5})),
            Ok(json!([2, [[2, 9, 1], [2, 13, 1]]]))
        );
        assert_eq!(em(2, "regua_busca", json!({"k": 2})), Ok(json!([0, 1])));
        // Termo vazio limpa; abrir outro arquivo também.
        assert_eq!(buscar("", false), Ok(json!(4)));
        assert_eq!(em(2, "achado", json!({"i": 0})), Ok(Value::Null));

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
        let erro = em(
            2,
            "consultar",
            json!({"filtro": ".x | (", "caminho": caminho}),
        )
        .unwrap_err();
        assert_eq!(erro["falhas"][0]["linha"], json!(1));
        assert!(erro["falhas"][0]["coluna"].as_u64().unwrap() >= 5, "{erro}");
        assert_eq!(
            em(2, "consultar", json!({"filtro": ".x", "caminho": caminho})),
            Ok(json!(1))
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
            (&json!(1), &json!(3), &json!(1), &json!(3), &json!(false))
        );
        let resultado = a["arquivo"].as_str().unwrap().to_owned();
        assert_eq!(std::fs::read_to_string(&resultado).unwrap(), "1\nnull\n2\n");
        // Erro de sintaxe (digitando, ao vivo) não mexe na consulta mostrada.
        assert!(
            em(
                2,
                "consultar",
                json!({"filtro": ".x | (", "caminho": caminho})
            )
            .is_err()
        );
        assert!(
            std::path::Path::new(&resultado).exists(),
            "erro de sintaxe apagou o resultado"
        );
        // Salvar o resultado: só o temporário atual do documento (antes do diálogo).
        let salvar = |arquivo: &str| {
            em(
                2,
                "salvar_consulta",
                json!({"arquivo": arquivo, "csv": true, "nome": "x"}),
            )
        };
        assert!(
            salvar(caminho)
                .unwrap_err()
                .as_str()
                .unwrap()
                .contains("não está mais aberto")
        );
        assert!(salvar("/etc/passwd").is_err());
        // O resultado abre no lugar do original, no mesmo documento.
        assert_eq!(
            chamar("abrir", json!({ "caminho": resultado, "id": 2 })),
            Ok(json!({"id": 2, "total": 3}))
        );
        // O laudo é do arquivo novo, não o do anterior que ficou no documento.
        let l = (0..200)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let l = em(2, "laudo", json!({})).unwrap();
                (!l.is_null()).then_some(l)
            })
            .expect("laudo do resultado não chegou");
        assert_eq!(
            (&l["conteudo"], &l["registros"]),
            (&json!("valores"), &json!(3))
        );
        // Voltar ao original apaga o resultado.
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho, "id": 2 })),
            Ok(json!({"id": 2, "total": 4}))
        );
        assert_eq!(em(2, "fechar_consulta", json!({})), Ok(Value::Null));
        assert!(!std::path::Path::new(&resultado).exists());
        assert_eq!(a["id"], json!(2), "evento leva o id do documento");

        // Achado 8: recursão sem fundo roda no processo filho e volta como falha; este processo
        // (o app) segue de pé.
        assert!(
            em(
                2,
                "consultar",
                json!({"filtro": "def f: 1 + f; f", "caminho": caminho})
            )
            .is_ok()
        );
        let a = fim();
        assert!(
            a["falha"].as_str().unwrap().contains("parou sem terminar"),
            "{a}"
        );
        assert_eq!(a["arquivo"], Value::Null);
        // Filtro preso e `fechar_consulta`: a thread da consulta só termina (e solta o
        // `cancelar`) depois de matar e esperar o filho. `consultar` volta na hora, com o
        // filtro ainda girando: não espera a consulta (síncrono, nem voltaria).
        let t = Instant::now();
        assert!(
            em(
                2,
                "consultar",
                json!({"filtro": "def f: f; f", "caminho": caminho})
            )
            .is_ok()
        );
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        let preso = Arc::downgrade(
            &app.state::<Estado>()
                .doc(2)
                .unwrap()
                .consulta
                .lock()
                .unwrap()
                .cancelar,
        );
        assert_eq!(em(2, "fechar_consulta", json!({})), Ok(Value::Null));
        let t = Instant::now();
        while preso.upgrade().is_some() {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(10),
                "cancelar não parou o filtro preso"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Corrida de gerações: a consulta presa A é trocada por B; só o fim de B chega, e o
        // temporário de A some.
        let velha = em(
            2,
            "consultar",
            json!({"filtro": "def f: f; f", "caminho": caminho}),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let nova = em(2, "consultar", json!({"filtro": ".x", "caminho": caminho})).unwrap();
        let a = fim();
        assert_eq!(a["geracao"], nova, "{a}");
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(
            rx.try_iter()
                .all(|e| e["geracao"] != velha || e["fim"] != true),
            "fim da consulta velha chegou"
        );
        let prefixo = format!("jasao-consulta-{}-{velha}-", std::process::id());
        assert!(
            std::fs::read_dir(std::env::temp_dir())
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with(&prefixo)),
            "temporário da consulta velha ficou"
        );
        assert_eq!(em(2, "fechar_consulta", json!({})), Ok(Value::Null));

        // Achado 9: FIFO é recusado na hora, sem abrir (abrir bloquearia até alguém escrever).
        #[cfg(unix)]
        {
            let pasta = tempfile::tempdir().unwrap();
            let fifo = pasta.path().join("fifo");
            assert!(
                std::process::Command::new("mkfifo")
                    .arg(&fifo)
                    .status()
                    .unwrap()
                    .success()
            );
            conceder(&fifo);
            // Sem a recusa, a leitura destrava quando este escritor abre (e o teste falha).
            let f = fifo.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let _ = std::fs::write(f, "1\n");
            });
            let t = Instant::now();
            let erro = em(
                2,
                "consultar",
                json!({"filtro": ".", "caminho": fifo.to_str().unwrap()}),
            )
            .unwrap_err();
            assert!(
                erro["mensagem"]
                    .as_str()
                    .unwrap()
                    .contains("não é um arquivo comum"),
                "{erro}"
            );
            assert!(t.elapsed() < std::time::Duration::from_secs(2));
            // `abrir` recusa o mesmo FIFO sem ficar em "Abrindo…" (recusa no núcleo).
            let erro = chamar("abrir", json!({ "caminho": fifo.to_str().unwrap() })).unwrap_err();
            assert!(
                erro.as_str().unwrap().contains("não é um arquivo comum"),
                "{erro}"
            );
            assert!(t.elapsed() < std::time::Duration::from_secs(2));
        }

        // Isolamento: o documento 1 segue com o seu arquivo, expansão e busca (nenhuma).
        assert_eq!(
            em(1, "linhas", json!({"inicio": 0, "quantidade": 9}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            5,
            "o documento 1 mantém o que o `revelar` expandiu"
        );
        assert_eq!(em(1, "achado", json!({"i": 0})), Ok(Value::Null));
        assert_eq!(em(1, "regua_busca", json!({"k": 2})), Ok(json!([])));

        // Consulta sobre exemplo embutido (caminho sintético) também abre.
        let exemplo = chamar("abrir", json!({ "caminho": "exemplo:vitrine" })).unwrap();
        let ex = exemplo["id"].as_u64().unwrap() as u32;
        assert_eq!(ex, 3);
        assert!(
            em(
                ex,
                "consultar",
                json!({"filtro": ".", "caminho": "exemplo:vitrine"})
            )
            .is_ok()
        );
        let a = fim();
        assert_eq!(a["id"], json!(3));
        let resultado = a["arquivo"].as_str().unwrap().to_owned();

        // Fechar: o documento some na hora (as threads só guardam `Weak`), o resultado de
        // consulta é apagado e o id deixa de valer.
        let doc = Arc::downgrade(&app.state::<Estado>().doc(ex).unwrap());
        assert_eq!(em(ex, "fechar", json!({})), Ok(Value::Null));
        assert!(
            doc.upgrade().is_none(),
            "documento fechado continua na memória"
        );
        assert!(!std::path::Path::new(&resultado).exists());
        let erro = em(ex, "resumo", json!({})).unwrap_err();
        assert!(erro.as_str().unwrap().contains("documento 3"), "{erro}");
        assert!(em(ex, "fechar", json!({})).is_err());
        // Ids não se reaproveitam: evento atrasado de uma aba fechada não cai em outra.
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho })).unwrap()["id"],
            json!(4)
        );

        // Formatado com teto: registro pequeno que formatado passaria de 16 MB volta cortado e
        // avisa; registro acima de 16 MB nem é formatado.
        let mut grande = tempfile::NamedTempFile::new().unwrap();
        let cadeia = format!("{}0{}", "[".repeat(999), "]".repeat(999));
        let fundo = format!("[{}]\n", vec![cadeia; 100].join(","));
        let longo = format!("\"{}\"\n", "a".repeat(MAX_FORMATADO));
        std::io::Write::write_all(&mut grande, (fundo + &longo).as_bytes()).unwrap();
        conceder(grande.path());
        let caminho_grande = grande.path().to_str().unwrap();
        assert_eq!(
            chamar("abrir", json!({ "caminho": caminho_grande })).unwrap()["id"],
            json!(5)
        );
        let f = em(5, "formatado", json!({"registro": 0})).unwrap();
        assert_eq!(
            (&f["truncado"], &f["teto"]),
            (&json!(true), &json!(MAX_FORMATADO))
        );
        assert!(f["texto"].as_str().unwrap().len() <= MAX_FORMATADO);
        let erro = em(5, "formatado", json!({"registro": 1})).unwrap_err();
        assert!(erro.as_str().unwrap().contains("passa do teto"), "{erro}");

        // Segredo com 2 valores distintos: não vira discriminador nem sai em laudo, estrutura, régua.
        let mut sg = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(
            &mut sg,
            b"{\"token\":\"hunter2xyz\",\"a\":1}\n{\"token\":\"s3cr3tvalue\",\"b\":2}\n",
        )
        .unwrap();
        conceder(sg.path());
        let id = chamar("abrir", json!({ "caminho": sg.path().to_str().unwrap() })).unwrap()["id"]
            .as_u64()
            .unwrap() as u32;
        let resumo = (0..200)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                em(id, "resumo", json!({})).unwrap()
            })
            .find(|r| r["coloriu"] == true)
            .expect("2ª passada não terminou");
        let tudo = format!(
            "{resumo}{}{}",
            em(id, "laudo", json!({})).unwrap(),
            em(id, "estrutura", json!({})).unwrap()
        );
        assert!(tudo.contains(".token"), "{tudo}");
        assert!(
            !tudo.contains("hunter2xyz") && !tudo.contains("s3cr3tvalue"),
            "{tudo}"
        );
        // Nem filtrar pelo valor (seria perguntar se ele existe).
        let i = em(id, "estrutura", json!({})).unwrap()["caminhos"]
            .as_array()
            .unwrap()
            .iter()
            .position(|c| c["caminho"] == ".token")
            .unwrap();
        for alvo in [
            json!({"caminho": i, "comum": "\"hunter2xyz\""}),
            json!({"caminho": i, "negativo": false, "expoente": 0}),
        ] {
            let erro = em(id, "filtrar", json!({ "alvo": alvo })).unwrap_err();
            assert!(erro.as_str().unwrap().contains("segredo"), "{erro}");
        }

        // Pasta: `pastas` avisa e `abrir` recusa com mensagem clara.
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        conceder(dir.path());
        assert_eq!(
            chamar("pastas", json!({ "caminhos": [d, caminho] })),
            Ok(json!([true, false]))
        );
        let erro = chamar("abrir", json!({ "caminho": d })).unwrap_err();
        assert!(erro.as_str().unwrap().contains("pasta"), "{erro}");

        {
            use tauri::Listener;
            let (tx, rx) = std::sync::mpsc::channel::<Value>();
            app.listen_any("filtragem", move |e| {
                let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
            });
            // Dois discriminadores, contagens desiguais: {d, v} trocado acha outras linhas.
            let mut dois = tempfile::NamedTempFile::new().unwrap();
            std::io::Write::write_all(
                &mut dois,
                b"{\"tipo\":\"a\",\"k\":\"p\",\"ta\":1,\"kp\":1}\n\
                  {\"tipo\":\"b\",\"k\":\"q\",\"tb\":1,\"kq\":1}\n\
                  {\"tipo\":\"a\",\"k\":\"q\",\"ta\":1,\"kq\":1}\n\
                  {\"tipo\":\"a\",\"k\":\"p\",\"ta\":1,\"kp\":1}\n\
                  {\"tipo\":\"b\",\"k\":\"p\",\"tb\":1,\"kp\":1}\n",
            )
            .unwrap();
            conceder(dois.path());
            let id =
                chamar("abrir", json!({ "caminho": dois.path().to_str().unwrap() })).unwrap()["id"]
                    .as_u64()
                    .unwrap() as u32;
            let ds = (0..200)
                .map(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    em(id, "estrutura", json!({}))
                })
                .find_map(|e| e.ok().filter(|e| e["registros"] == 5))
                .expect("inferência não terminou")["discriminadores"]
                .clone();
            let resumo = |d: &Value| {
                (
                    d["campo"].clone(),
                    d["variantes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v["valor"].clone())
                        .collect::<Vec<_>>(),
                )
            };
            assert_eq!(
                ds.as_array()
                    .unwrap()
                    .iter()
                    .map(resumo)
                    .collect::<Vec<_>>(),
                [
                    (json!(".tipo"), vec![json!("a"), json!("b")]),
                    (json!(".k"), vec![json!("p"), json!("q")])
                ]
            );
            let filtrar_em = |alvo: Value| {
                let g = em(id, "filtrar", json!({ "alvo": alvo })).unwrap();
                std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(5)).ok())
                    .find(|v| v["fim"] == true && v["geracao"] == g)
                    .expect("filtro não terminou")
            };
            let linhas = || {
                em(
                    id,
                    "tabela_linhas",
                    json!({"inicio": 0, "quantidade": 9, "colunas": [0]}),
                )
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["registro"].as_u64().unwrap())
                .collect::<Vec<_>>()
            };
            assert_eq!(
                filtrar_em(json!({"discriminador": 1, "variante": 1}))["achados"],
                2
            );
            assert_eq!(linhas(), [1, 2], ".k = q");
            assert_eq!(
                filtrar_em(json!({"discriminador": 0, "variante": 1}))["achados"],
                2
            );
            assert_eq!(linhas(), [1, 4], ".tipo = b");
            assert_eq!(
                filtrar_em(json!({"discriminador": 1, "variante": 0}))["achados"],
                3
            );
            assert_eq!(linhas(), [0, 3, 4], ".k = p");
            assert_eq!(
                em(id, "tabela", json!({})).unwrap()["filtro"],
                json!({"achados": 3, "caminho": ".k", "tem": null, "valor": "p", "comum": null, "faixa": null})
            );
            assert_eq!(em(id, "fechar", json!({})), Ok(Value::Null));
        }
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
        app.state::<abertura::Concedidos>().conceder(&arq);
        assert_eq!(chamar("recentes", json!({})), json!([]));
        // O temporário de stdin abre, mas não vira recente.
        let op = nucleo::Opcoes {
            dir_cache: Some(raiz.join("cache")),
            ..nucleo::Opcoes::default()
        };
        let t = nucleo::entrada::gravar(&b"{\"s\":1}\n"[..], "stdin", &op).unwrap();
        let tmp = t.caminho().to_str().unwrap().to_owned();
        *app.state::<Stdin>().0.lock().unwrap() = Some(t);
        abertura::pedir(app.handle(), vec![tmp.clone()]);
        assert!(chamar("abrir", json!({ "caminho": tmp }))["total"] == json!(1));
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

    /// Só abre o que o usuário concedeu; o resto recebe o mesmo erro, exista ou não.
    #[test]
    fn caminhos_concedidos_pelo_ipc() {
        let pasta = tempfile::tempdir().unwrap();
        let raiz = std::fs::canonicalize(pasta.path()).unwrap();
        let dentro = raiz.join("dentro");
        std::fs::create_dir(&dentro).unwrap();
        let concedido = dentro.join("a.jsonl");
        let fora = raiz.join("fora.jsonl");
        let recente = raiz.join("recente.jsonl");
        for p in [&concedido, &fora, &recente] {
            std::fs::write(p, b"{\"a\":1}\n").unwrap();
        }
        // Recente de uma execução anterior: está na lista, mas não foi concedido nesta.
        let lista = raiz.join("recentes.json");
        recentes::adicionar(&lista, recente.to_str().unwrap()).unwrap();
        let app = comandos(mock_builder())
            .manage(Recentes(lista))
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
        let abrir = |c: &Path| chamar("abrir", json!({ "caminho": c.to_str().unwrap() }));
        let negado = Err(json!(NEGADO));
        // O usuário concede um arquivo e a pasta dele (arraste de pasta).
        let concedidos = app.state::<abertura::Concedidos>();
        concedidos.conceder(&concedido);
        concedidos.conceder(&dentro);

        // Não concedido: erro genérico, igual para arquivo que existe e que não existe.
        assert_eq!(abrir(&fora), negado);
        assert_eq!(abrir(&raiz.join("nao-existe.jsonl")), negado);
        assert_eq!(abrir(Path::new("/etc/passwd")), negado);
        // Pasta concedida não concede o que está dentro dela.
        assert_eq!(abrir(&dentro.join("outro.jsonl")), negado);
        // Nem oráculo pelos vizinhos: `pastas` e `recente_ausente` não olham o disco.
        let r = raiz.to_str().unwrap();
        assert_eq!(
            chamar(
                "pastas",
                json!({ "caminhos": [r, dentro.to_str().unwrap()] })
            ),
            Ok(json!([false, true]))
        );
        assert_eq!(
            chamar("recente_ausente", json!({ "caminho": "/nao/existe.json" })),
            Ok(json!(false))
        );
        // Consulta jq também só sobre o concedido (abre um documento para ter o id).
        assert_eq!(abrir(&concedido), Ok(json!({"id": 1, "total": 1})));
        let erro = chamar(
            "consultar",
            json!({"id": 1, "filtro": ".", "caminho": fora.to_str().unwrap()}),
        );
        assert_eq!(erro.unwrap_err()["mensagem"], json!(NEGADO));
        // Consulta recusada não mexe na que está em curso nem apaga o resultado dela.
        let caminho_c = concedido.to_str().unwrap();
        let consulta = || {
            let doc = app.state::<Estado>().doc(1).unwrap();
            let c = doc.consulta.lock().unwrap();
            (c.geracao, c.arquivo.clone(), c.cancelar.load(Relaxed))
        };
        assert_eq!(
            chamar(
                "consultar",
                json!({"id": 1, "filtro": "def f: f; f", "caminho": caminho_c})
            ),
            Ok(json!(1))
        );
        let antes = consulta();
        assert!(
            chamar(
                "consultar",
                json!({"id": 1, "filtro": ".", "caminho": fora.to_str().unwrap()})
            )
            .is_err()
        );
        assert_eq!(consulta(), antes);
        assert!(antes.1.unwrap().exists());
        assert_eq!(chamar("fechar_consulta", json!({"id": 1})), Ok(Value::Null));

        // Concessão vale para o alvo real, não para o texto do caminho: `..` concedido cru
        // concede o arquivo a que ele leva, e o mesmo nome apontando para outro lugar depois
        // não herda a concessão (comparar o texto deixaria passar).
        let outro = raiz.join("outro");
        std::fs::create_dir(&outro).unwrap();
        let ali = outro.join("ali.jsonl");
        std::fs::write(&ali, b"{\"b\":2}\n").unwrap();
        concedidos.conceder(&dentro.join("..").join("outro").join("ali.jsonl"));
        assert!(abrir(&ali).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            // Link concedido quando apontava para `ali`; depois aponta para `fora`.
            let link = raiz.join("link.jsonl");
            symlink(&ali, &link).unwrap();
            concedidos.conceder(&link);
            assert!(abrir(&link).is_ok());
            std::fs::remove_file(&link).unwrap();
            symlink(&fora, &link).unwrap();
            assert_eq!(abrir(&link), negado);
            // Pasta-link com `..`: `d/../ali.jsonl` concedido com `d` em `outro/sub`, depois `d`
            // repontado para `dentro`: o mesmo texto leva a `raiz/ali.jsonl`, nunca concedido.
            let sub = outro.join("sub");
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(raiz.join("ali.jsonl"), b"{}\n").unwrap();
            let d = raiz.join("d");
            symlink(&sub, &d).unwrap();
            let via = d.join("..").join("ali.jsonl");
            concedidos.conceder(&via);
            assert!(abrir(&via).is_ok());
            std::fs::remove_file(&d).unwrap();
            symlink(&dentro, &d).unwrap();
            assert_eq!(abrir(&via), negado);
            // Link para o concedido abre, e `autorizado` devolve o alvo canônico (é ele que se
            // abre, não o link).
            let bom = raiz.join("bom.jsonl");
            symlink(&concedido, &bom).unwrap();
            assert!(abrir(&bom).is_ok());
            assert_eq!(
                autorizado(app.handle(), bom.to_str().unwrap()),
                Ok(concedido.to_str().unwrap().to_owned())
            );
        }
        // `..` que volta ao concedido abre, e o que se abre é o canônico.
        let volta = dentro.join("..").join("dentro").join("a.jsonl");
        assert!(abrir(&volta).is_ok());
        assert_eq!(
            autorizado(app.handle(), volta.to_str().unwrap()),
            Ok(concedido.to_str().unwrap().to_owned())
        );

        // Recente reaberto (lista em disco, sem concessão nesta execução): abre.
        assert!(abrir(&recente).is_ok());
    }

    /// Linha do tempo: só de pasta concedida; concede as versões listadas (e só elas), e o teto
    /// de arquivos recusa antes de conceder qualquer uma.
    #[test]
    fn linha_do_tempo_concede_so_as_versoes() {
        let tmp = tempfile::tempdir().unwrap();
        let raiz = std::fs::canonicalize(tmp.path()).unwrap();
        let pasta = raiz.join("versoes");
        let grande = raiz.join("grande");
        std::fs::create_dir(&pasta).unwrap();
        std::fs::create_dir(&grande).unwrap();
        for n in ["2024-01-01.jsonl", "2024-01-02.jsonl"] {
            std::fs::write(pasta.join(n), b"{\"a\":1}\n").unwrap();
        }
        std::fs::write(pasta.join("notas.txt"), b"x").unwrap();
        let fora = raiz.join("fora.jsonl");
        std::fs::write(&fora, b"{}\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&fora, pasta.join("2024-01-03.jsonl")).unwrap();
        for i in 0..=nucleo::linha_do_tempo::LIMITES.max_arquivos {
            std::fs::write(grande.join(format!("{i}.jsonl")), b"{}\n").unwrap();
        }
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
        let linha = |p: &Path| chamar("linha_do_tempo", json!({ "pasta": p.to_str().unwrap() }));
        let abrir = |c: &Path| chamar("abrir", json!({ "caminho": c.to_str().unwrap() }));
        let negado = Err(json!(NEGADO));
        let a = pasta.join("2024-01-01.jsonl");

        // Pasta não concedida: negado, e nada dentro dela passa a abrir.
        assert_eq!(linha(&pasta), negado);
        assert_eq!(abrir(&a), negado);
        let concedidos = app.state::<abertura::Concedidos>();
        concedidos.conceder(&pasta);
        concedidos.conceder(&grande);
        // Arquivo concedido não é pasta.
        concedidos.conceder(&a.with_file_name("2024-01-02.jsonl"));
        assert_eq!(linha(&pasta.join("2024-01-02.jsonl")), negado);

        // Teto de arquivos: erro, e nenhum dos 501 fica concedido.
        let e = linha(&grande).unwrap_err();
        assert!(e.as_str().unwrap().contains("mais de 500 arquivos"), "{e}");
        assert_eq!(abrir(&grande.join("0.jsonl")), negado);

        // Pasta concedida: as versões listadas abrem; o que não é versão e o link para fora, não.
        assert_eq!(linha(&pasta), Ok(json!(1)));
        assert!(abrir(&a).is_ok());
        assert_eq!(abrir(&pasta.join("notas.txt")), negado);
        assert_eq!(abrir(&fora), negado);
        #[cfg(unix)]
        assert_eq!(abrir(&pasta.join("2024-01-03.jsonl")), negado);
    }

    /// Cada porta de concessão concede de fato: soltar, pedido do sistema, argv e diálogo.
    #[test]
    fn fontes_de_concessao() {
        let pasta = tempfile::tempdir().unwrap();
        let raiz = std::fs::canonicalize(pasta.path()).unwrap();
        let [solto, pedido, argv, escolhido] =
            ["solto", "pedido", "argv", "escolhido"].map(|n| raiz.join(format!("{n}.jsonl")));
        for p in [&solto, &pedido, &argv, &escolhido] {
            std::fs::write(p, b"{}\n").unwrap();
        }
        let app = comandos(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let h = app.handle();
        let negado = || Err(NEGADO.to_owned());
        let ok = |p: &Path| Ok(p.to_str().unwrap().to_owned());
        for p in [&solto, &pedido, &argv, &escolhido] {
            assert_eq!(autorizado(h, p.to_str().unwrap()), negado());
        }
        soltos(h, std::slice::from_ref(&solto));
        assert_eq!(autorizado(h, solto.to_str().unwrap()), ok(&solto));
        abertura::pedir(h, vec![pedido.to_str().unwrap().into()]);
        assert_eq!(autorizado(h, pedido.to_str().unwrap()), ok(&pedido));
        // argv relativo, resolvido contra a pasta de quem chamou.
        conceder_argv(h, &["jasao".into(), "argv.jsonl".into()], &raiz);
        assert_eq!(autorizado(h, argv.to_str().unwrap()), ok(&argv));
        assert_eq!(
            conceder_escolhidos(h, std::iter::once(escolhido.clone())),
            [escolhido.to_str().unwrap()]
        );
        assert_eq!(autorizado(h, escolhido.to_str().unwrap()), ok(&escolhido));
    }
}
