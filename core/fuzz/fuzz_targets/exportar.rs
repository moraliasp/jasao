#![no_main]
//! Arquivo qualquer: o Schema gerado da inferência compila no validador independente
//! (`jsonschema`) e todo registro que o `serde_json` lê passa nele (opções padrão e literal).
//! O Zod sai da mesma `Forma`; aqui só conferimos que gera sem pânico.

use std::sync::atomic::AtomicBool;

use libfuzzer_sys::fuzz_target;
use nucleo::estrutura::inferir;
use nucleo::exportar::{Opcoes, schema, zod};
use nucleo::validar::validar;

fuzz_target!(|dados: &[u8]| {
    let nao = AtomicBool::new(false);
    let inf = inferir(dados, &nao, |_| {});
    for op in [
        Opcoes::default(),
        Opcoes { literal: true, numero: false },
        Opcoes { literal: true, numero: true },
    ] {
        let _ = zod(&inf.resumo, op);
        let s = schema(&inf.resumo, op);
        let v = validar(dados, inf.entrada, &s, &nao, |_| {}).expect("schema gerado compila");
        if !op.numero {
            assert_eq!(v.falham, 0, "{:?}\n{s}", v.falhas);
        }
    }
});
