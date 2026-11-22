// Sem console extra no Windows em release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // O mesmo binário é o processo filho da consulta jq (ver `filho.rs`).
    if let Some(codigo) = app_lib::consulta_filho() {
        std::process::exit(codigo);
    }
    app_lib::run()
}
