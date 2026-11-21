//! Casca do app: guarda o documento aberto e entrega janelas da árvore à interface.

use std::sync::Mutex;

use nucleo::Arvore;
use nucleo::arvore::Linha;
use tauri::{Manager, State};

/// Teto de linhas por pedido: a interface pede janelas, nunca o arquivo.
const MAX_JANELA: usize = 1_000;

#[derive(Default)]
struct Estado(Mutex<Option<Arvore>>);

impl Estado {
    fn com<T>(&self, f: impl FnOnce(&mut Arvore) -> T) -> Result<T, String> {
        let mut guarda = self.0.lock().map_err(|e| e.to_string())?;
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
// shortcut: aceita qualquer caminho vindo da interface (só leitura); passar a concessões por
// diálogo/arraste como no Leve quando a interface carregar conteúdo de fora.
#[tauri::command]
async fn abrir(caminho: String, estado: State<'_, Estado>) -> Result<usize, String> {
    let arvore = Arvore::abrir(&caminho).map_err(|e| format!("{caminho}: {e}"))?;
    let total = arvore.total();
    *estado.0.lock().map_err(|e| e.to_string())? = Some(arvore);
    Ok(total)
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
        .invoke_handler(tauri::generate_handler![
            arquivo_inicial,
            abrir,
            linhas,
            alternar,
            pai,
            ir_para
        ])
        .run(tauri::generate_context!())
        .expect("falha ao iniciar o app");
}
