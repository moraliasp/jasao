//! Casca do app: guarda o arquivo aberto e entrega janelas de linhas à interface.

use std::sync::Mutex;

use nucleo::ArquivoLinhas;
use tauri::ipc::Response;
use tauri::{Manager, State};

/// Teto de linhas por pedido: a interface pede janelas, nunca o arquivo.
const MAX_JANELA: usize = 10_000;

#[derive(Default)]
struct Estado(Mutex<Option<ArquivoLinhas>>);

/// Caminho passado na linha de comando (`app <arquivo>`), se houver.
#[tauri::command]
fn arquivo_inicial() -> Option<String> {
    std::env::args().nth(1)
}

/// Abre o arquivo, constrói o índice e devolve o número de linhas.
/// `async` para rodar fora da thread principal: o índice de 1 GB leva ~150 ms.
// shortcut: aceita qualquer caminho vindo da interface (só leitura); passar a concessões por
// diálogo/arraste como no Leve quando a interface carregar conteúdo de fora.
#[tauri::command]
async fn abrir(caminho: String, estado: State<'_, Estado>) -> Result<usize, String> {
    let arquivo = ArquivoLinhas::abrir(&caminho).map_err(|e| format!("{caminho}: {e}"))?;
    let linhas = arquivo.linhas();
    *estado.0.lock().map_err(|e| e.to_string())? = Some(arquivo);
    Ok(linhas)
}

/// Linhas cruas `[inicio, inicio + quantidade)` separadas por `\n`, como ArrayBuffer
/// (`ipc::Response`, sem JSON). Ver docs/decisoes.md.
#[tauri::command]
fn janela(inicio: usize, quantidade: usize, estado: State<'_, Estado>) -> Result<Response, String> {
    let guarda = estado.0.lock().map_err(|e| e.to_string())?;
    let arquivo = guarda.as_ref().ok_or("nenhum arquivo aberto")?;
    let linhas = arquivo.janela(inicio, quantidade.min(MAX_JANELA));
    Ok(Response::new(linhas.join(&b'\n')))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Estado::default())
        .setup(|app| {
            // Título vem do productName (tauri.conf.json), onde mora o nome do produto.
            let nome = app.package_info().name.clone();
            if let Some(janela) = app.get_webview_window("main") {
                janela.set_title(&nome)?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![arquivo_inicial, abrir, janela])
        .run(tauri::generate_context!())
        .expect("falha ao iniciar o app");
}
