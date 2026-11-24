#![no_main]
//! Dois arquivos quaisquer: o diff não entra em pânico, A contra A não tem mudanças, cada
//! evidência tem `n ≤ de`, e o diff de B contra A é o espelho do de A contra B (mesmos caminhos,
//! regra inversa, antes e depois trocados).
//! Entrada: [onde corta, bytes…] → A = bytes[..corte], B = bytes[corte..].

use std::sync::atomic::AtomicBool;

use libfuzzer_sys::fuzz_target;
use nucleo::diff::{Mudanca, Regra, diff};
use nucleo::estrutura::inferir;

/// Regra vista do outro lado (campo novo vira removido, independente de obrigatório).
fn espelho(r: Regra) -> Regra {
    match r {
        Regra::CampoRemovido => Regra::CampoNovoOpcional,
        Regra::CampoNovoOpcional | Regra::CampoNovoObrigatorio => Regra::CampoRemovido,
        Regra::VirouObrigatorio => Regra::VirouOpcional,
        Regra::VirouOpcional => Regra::VirouObrigatorio,
        Regra::TipoEstreitado => Regra::TipoAlargado,
        Regra::TipoAlargado => Regra::TipoEstreitado,
        Regra::TipoTrocado => Regra::TipoTrocado,
        Regra::VarianteRemovida => Regra::VarianteNova,
        Regra::VarianteNova => Regra::VarianteRemovida,
    }
}

fn chave(m: &Mudanca) -> (String, &'static str, (u64, u64), (u64, u64)) {
    let r = match m.regra {
        Regra::CampoNovoObrigatorio => Regra::CampoNovoOpcional,
        r => r,
    };
    (m.caminho.clone(), r.nome(), (m.antes.n, m.antes.de), (m.depois.n, m.depois.de))
}

fuzz_target!(|entrada: &[u8]| {
    let [corte, dados @ ..] = entrada else { return };
    let (a, b) = dados.split_at((*corte as usize).min(dados.len()));
    let nao = AtomicBool::new(false);
    let (ra, rb) = (inferir(a, &nao, |_| {}).resumo, inferir(b, &nao, |_| {}).resumo);

    assert!(diff(&ra, &ra).is_empty());
    let ida = diff(&ra, &rb);
    for m in &ida {
        assert!(m.antes.n <= m.antes.de && m.depois.n <= m.depois.de, "{m}");
    }
    let mut ida: Vec<_> = ida
        .iter()
        .map(|m| {
            let mut x = m.clone();
            x.regra = espelho(m.regra);
            (x.antes, x.depois) = (m.depois, m.antes);
            chave(&x)
        })
        .collect();
    let mut volta: Vec<_> = diff(&rb, &ra).iter().map(chave).collect();
    ida.sort();
    volta.sort();
    assert_eq!(ida, volta);
});
