#![no_main]
//! Conversão de Toml: ver `comum/formatos.rs`.

use libfuzzer_sys::fuzz_target;

#[path = "../comum/formatos.rs"]
mod comum;

fuzz_target!(|dados: &[u8]| comum::verificar(dados, nucleo::formatos::Entrada::Toml));
