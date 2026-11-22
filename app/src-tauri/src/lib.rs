//! Casca do app: guarda o documento aberto e entrega janelas da árvore à interface.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nucleo::arvore::Linha;
use nucleo::regua::{Baldes, Legenda};
use nucleo::resumo::{self, Conteudo, Progresso};
use nucleo::{Achado, Arvore, Busca};
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

/// Caminho passado na linha de comando (`app <arquivo>`), se houver.
#[tauri::command]
fn arquivo_inicial() -> Option<String> {
    std::env::args().nth(1)
}

/// Abre o arquivo, constrói o índice e devolve o número de registros (= linhas visíveis).
/// `async` para rodar fora da thread principal: o índice de 1 GB leva ~150 ms.
/// Dispara a varredura de inválidas numa thread própria, que emite `varredura` com o progresso;
/// a do arquivo anterior é cancelada.
// shortcut: aceita qualquer caminho vindo da interface (só leitura); passar a concessões por
// diálogo/arraste como no Leve quando a interface carregar conteúdo de fora.
#[tauri::command]
async fn abrir<R: Runtime>(
    caminho: String,
    app: AppHandle<R>,
    estado: State<'_, Estado>,
) -> Result<usize, String> {
    let arvore = Arvore::abrir(&caminho).map_err(|e| format!("{caminho}: {e}"))?;
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
        .invoke_handler(tauri::generate_handler![
            arquivo_inicial,
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
            valor
        ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    comandos(tauri::Builder::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Título vem do productName (tauri.conf.json), onde mora o nome do produto.
            let nome = app.package_info().name.clone();
            if let Some(janela) = app.get_webview_window("main") {
                janela.set_title(&nome)?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("falha ao iniciar o app");
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
    }
}
