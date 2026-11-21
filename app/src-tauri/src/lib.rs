//! Casca do app: guarda o documento aberto e entrega janelas da árvore à interface.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use nucleo::Arvore;
use nucleo::arvore::Linha;
use nucleo::resumo::{self, Conteudo, Progresso};
use serde::Serialize;
use tauri::{AppHandle, Builder, Emitter, Manager, Runtime, State};

/// Teto de linhas por pedido: a interface pede janelas, nunca o arquivo.
const MAX_JANELA: usize = 1_000;

#[derive(Default)]
struct Estado {
    arvore: Mutex<Option<Arvore>>,
    varredura: Mutex<Varredura>,
}

/// Varredura de linhas inválidas do arquivo aberto (roda em 2º plano, ver `abrir`).
#[derive(Default)]
struct Varredura {
    /// Sobe a cada `abrir`: progresso de um arquivo anterior é descartado.
    geracao: u64,
    cancelar: Arc<AtomicBool>,
    conteudo: Option<Conteudo>,
    progresso: Option<Progresso>,
}

/// Evento `varredura` e parte do `resumo`.
#[derive(Clone, Serialize)]
struct Andamento {
    geracao: u64,
    #[serde(flatten)]
    progresso: Progresso,
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
    let cancelar = Arc::new(AtomicBool::new(false));
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
        v.geracao
    };
    *estado.arvore.lock().map_err(|e| e.to_string())? = Some(arvore);
    if !pular {
        std::thread::spawn(move || {
            resumo::varrer(&arquivo, &cancelar, |progresso| {
                let estado = app.state::<Estado>();
                let Ok(mut v) = estado.varredura.lock() else {
                    return;
                };
                if v.geracao != geracao {
                    return;
                }
                v.progresso = Some(progresso);
                drop(v);
                let _ = app.emit("varredura", Andamento { geracao, progresso });
            });
        });
    }
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
    let (Some(conteudo), Some(progresso)) = (v.conteudo, v.progresso) else {
        return Err("nenhum arquivo aberto".into());
    };
    Ok(Resumo {
        conteudo,
        bytes,
        registros,
        andamento: Andamento {
            geracao: v.geracao,
            progresso,
        },
    })
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
            formatado
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
            .find(|r| r["fim"] == true)
            .expect("varredura não terminou");
        assert_eq!(
            resumo,
            json!({"conteudo": "objetos", "bytes": 22, "registros": 2, "geracao": 1,
                   "varridas": 2, "total": 2, "invalidas": 1, "fim": true, "cancelado": false})
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
    }
}
