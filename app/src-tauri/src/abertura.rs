//! Arquivos que o sistema pede para abrir: argv (1ª instância), `single-instance` (2ª instância,
//! Windows/Linux) e `RunEvent::Opened` (macOS: Finder, "Abrir com", `open -a`). Tudo entra numa
//! fila e cada caminho sai dela uma vez só: `arquivos_iniciais` entrega o que chegou na abertura
//! e `arquivos_pedidos` o que chegou depois, a cada aviso `abrir-arquivos`. Mesmo desenho do
//! `open.rs` do Leve (ver docs/decisoes.md, "Vários arquivos pedidos pelo SO", lá).

use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

/// Quanto esperar, depois do `RunEvent::Ready`, por um arquivo pedido pelo sistema. No macOS o
/// arquivo chega como Apple Event (`RunEvent::Opened`), que na prática vem antes do `setup`, mas o
/// AppKit não garante a ordem: sem a folga, a interface concluiria "aberto sem arquivo" e
/// mostraria os exemplos. Fora do macOS o arquivo vem por argv, lido no `setup`.
#[cfg(target_os = "macos")]
const FOLGA: Duration = Duration::from_millis(500);
#[cfg(not(target_os = "macos"))]
const FOLGA: Duration = Duration::ZERO;
/// Teto da espera, contado da chamada: sem `Ready`, a interface não trava.
const ESPERA_MAX: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Abertura {
    fila: Mutex<Vec<String>>,
    pronto_em: Mutex<Option<Instant>>,
    aviso: Condvar,
}

impl Abertura {
    /// Põe na fila e acorda quem espera a abertura.
    pub fn enfileirar(&self, caminho: String) {
        self.fila.lock().unwrap().push(caminho);
        self.aviso.notify_all();
    }

    pub fn pronto(&self) {
        self.pronto_em
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now);
        self.aviso.notify_all();
    }

    fn tirar(&self) -> Vec<String> {
        std::mem::take(&mut *self.fila.lock().unwrap())
    }

    /// Espera a abertura assentar (já chegou arquivo, ou o `Ready` passou há `folga`), até `max`.
    fn esperar(&self, folga: Duration, max: Duration) -> Vec<String> {
        let teto = Instant::now() + max;
        let mut pronto = self.pronto_em.lock().unwrap();
        loop {
            let agora = Instant::now();
            let tem = !self.fila.lock().unwrap().is_empty();
            let assentou = tem || pronto.is_some_and(|p| agora >= p + folga);
            if assentou || agora >= teto {
                break;
            }
            let prazo = pronto.map_or(teto, |p| (p + folga).min(teto));
            pronto = self.aviso.wait_timeout(pronto, prazo - agora).unwrap().0;
        }
        drop(pronto);
        self.tirar()
    }
}

/// Argumentos que são caminhos (não opções), resolvidos contra `cwd`: o de quem chamou, que na
/// 2ª instância vem do plugin, porque a 1ª tem outra pasta atual.
pub fn caminhos_argv(argv: &[String], cwd: &Path) -> Vec<String> {
    argv.iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(|a| cwd.join(a).to_string_lossy().into_owned())
        .collect()
}

/// Pedido do sistema com o app já aberto: enfileira, avisa a interface e traz a janela à frente.
pub fn pedir<R: Runtime>(app: &AppHandle<R>, caminhos: Vec<String>) {
    let abertura = app.state::<Abertura>();
    let vazio = caminhos.is_empty();
    for c in caminhos {
        abertura.enfileirar(c);
    }
    if !vazio {
        let _ = app.emit("abrir-arquivos", ());
    }
    if let Some(j) = app.get_webview_window("main") {
        let _ = j.unminimize();
        let _ = j.set_focus();
    }
}

/// Arquivos pedidos pelo sistema na abertura, em ordem, depois de ela assentar. Vazio quando
/// aberto pelo ícone ou Dock (ou na recarga da página): aí a interface mostra os exemplos.
#[tauri::command]
pub async fn arquivos_iniciais<R: Runtime>(app: AppHandle<R>) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<Abertura>().esperar(FOLGA, ESPERA_MAX))
        .await
        .unwrap_or_default()
}

/// Arquivos que chegaram desde a última entrega (o aviso `abrir-arquivos` não leva caminho).
#[tauri::command]
pub fn arquivos_pedidos(abertura: State<'_, Abertura>) -> Vec<String> {
    abertura.tirar()
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::sync::Arc;

    const FOLGA: Duration = Duration::from_millis(500);

    #[test]
    fn varios_na_abertura_chegam_todos_e_em_ordem() {
        // `open -a Jasão a.json b.jsonl c.ndjson` com o app fechado: um Opened por arquivo.
        let a = Abertura::default();
        for c in ["/a.json", "/b.jsonl", "/c.ndjson"] {
            a.enfileirar(c.into());
        }
        a.pronto();
        assert_eq!(
            a.esperar(FOLGA, ESPERA_MAX),
            ["/a.json", "/b.jsonl", "/c.ndjson"]
        );
        // Recarga da página pergunta de novo: nada volta (e não espera a folga de novo à toa).
        assert!(a.esperar(Duration::ZERO, ESPERA_MAX).is_empty());
    }

    #[test]
    fn sem_arquivo_espera_a_folga_e_devolve_vazio() {
        let a = Abertura::default();
        a.pronto();
        let t0 = Instant::now();
        assert!(a.esperar(Duration::from_millis(50), ESPERA_MAX).is_empty());
        assert!(t0.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn sem_ready_nao_trava_alem_do_teto() {
        let a = Abertura::default();
        let t0 = Instant::now();
        assert!(a.esperar(FOLGA, Duration::from_millis(80)).is_empty());
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn arquivo_tardio_acorda_a_espera() {
        // Opened depois do Ready: só o aviso de `enfileirar` acorda antes da folga de 5 s.
        let a = Arc::new(Abertura::default());
        a.pronto();
        let a2 = Arc::clone(&a);
        let tarde = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            a2.enfileirar("/tarde.json".into());
        });
        let t0 = Instant::now();
        let veio = a.esperar(Duration::from_secs(5), Duration::from_secs(10));
        tarde.join().unwrap();
        assert_eq!(veio, ["/tarde.json"]);
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    }

    #[test]
    fn depois_da_abertura_cada_um_sai_uma_vez() {
        let a = Abertura::default();
        a.enfileirar("/x.json".into());
        a.enfileirar("/y.json".into());
        assert_eq!(a.tirar(), ["/x.json", "/y.json"]);
        assert!(a.tirar().is_empty());
    }

    #[test]
    fn argv_pula_o_programa_e_opcoes_e_resolve_relativo() {
        let argv: Vec<String> = ["jasao", "a.json", "-psn_0_1", "/abs/b.jsonl"]
            .map(String::from)
            .into();
        assert_eq!(
            caminhos_argv(&argv, Path::new("/pasta")),
            ["/pasta/a.json", "/abs/b.jsonl"]
        );
        assert!(caminhos_argv(&argv[..1], Path::new("/")).is_empty());
    }
}
