#![no_main]
//! Qualquer entrada: o índice não entra em pânico, as faixas ficam dentro do buffer, em ordem,
//! sem `\n` dentro, e juntas cobrem o conteúdo depois do BOM.

use libfuzzer_sys::fuzz_target;
use nucleo::IndiceLinhas;

fuzz_target!(|dados: &[u8]| {
    let ind = IndiceLinhas::construir(dados);
    let mut anterior_fim = 0;
    for i in 0..ind.len() {
        let r = ind.faixa(i, dados).expect("linha dentro do índice");
        assert!(r.start <= r.end && r.end <= dados.len());
        assert!(r.start >= anterior_fim);
        assert!(!dados[r.clone()].contains(&b'\n'));
        anterior_fim = r.end;
    }
    assert!(ind.faixa(ind.len(), dados).is_none());
    let corpo = dados.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(dados);
    let esperado = if corpo.is_empty() { 0 } else { corpo.split(|&b| b == b'\n').count() - usize::from(corpo.ends_with(b"\n")) };
    assert_eq!(ind.len(), esperado);
});
