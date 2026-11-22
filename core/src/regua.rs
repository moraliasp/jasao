//! Régua do micronavegador: uma marca por registro (variante do discriminador ou inválida),
//! reduzida a K baldes para desenhar.
//!
//! As marcas chegam em duas passadas sobre o mesmo vetor: `resumo::varrer` marca as inválidas
//! e `colorir` pinta as variantes depois da inferência (`estrutura`).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};

use rayon::prelude::*;
use serde::Serialize;

use crate::ArquivoLinhas;
use crate::estrutura::Discriminador;

/// Válido sem cor (ainda sem a 2ª passada, sem discriminador ou registro que não é objeto).
pub const NEUTRO: u8 = 0;
/// Variantes com cor própria: 1..=CORES, da mais frequente para a menos.
pub const CORES: u8 = 4;
/// As variantes além das `CORES` primeiras.
pub const OUTROS: u8 = CORES + 1;
pub const INVALIDA: u8 = u8::MAX;

/// Linhas por tarefa da 2ª passada.
const BLOCO: usize = 4_096;

/// Uma marca por registro, todas `NEUTRO`.
pub fn marcas(n: usize) -> Box<[AtomicU8]> {
    (0..n).map(|_| AtomicU8::new(NEUTRO)).collect()
}

/// O discriminador que cobre mais registros; no empate, o primeiro.
pub fn escolher(ds: &[Discriminador]) -> Option<&Discriminador> {
    let cobre = |d: &Discriminador| d.variantes.iter().map(|v| v.registros.n).sum::<u64>();
    ds.iter().rev().max_by_key(|d| cobre(d))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Legenda {
    /// Como o `estrutura` escreve (`.tipo`, `.["a b"]`).
    pub campo: String,
    /// As `CORES` mais frequentes, na ordem das cores: (valor como está no arquivo, registros).
    pub variantes: Vec<(String, u64)>,
    /// Registros das demais variantes.
    pub outros: u64,
}

/// Cor por frequência e depois por valor: o mesmo arquivo abre sempre igual.
pub fn legenda(d: &Discriminador) -> Legenda {
    let mut vs: Vec<(String, u64)> = d
        .variantes
        .iter()
        .map(|v| (v.valor.clone(), v.registros.n))
        .collect();
    vs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let outros = vs.iter().skip(CORES as usize).map(|v| v.1).sum();
    vs.truncate(CORES as usize);
    Legenda {
        campo: d.campo.clone(),
        variantes: vs,
        outros,
    }
}

/// Chave crua do topo a partir do `campo` do `estrutura` (`.x` ou `.["…"]` com escapes).
fn chave(campo: &str) -> Option<String> {
    match campo
        .strip_prefix(".[\"")
        .and_then(|c| c.strip_suffix("\"]"))
    {
        Some(cru) => sonic_rs::from_str(&format!("\"{cru}\"")).ok(),
        None => campo.strip_prefix('.').map(str::to_owned),
    }
}

/// 2ª passada: pinta cada registro válido com a cor da sua variante (as que ficam fora da
/// `legenda` viram `OUTROS`). Devolve `false` se cancelada.
pub fn colorir(
    arquivo: &ArquivoLinhas,
    d: &Discriminador,
    marcas: &[AtomicU8],
    cancelar: &AtomicBool,
) -> bool {
    let legenda = legenda(d);
    let Some(chave) = chave(&d.campo) else {
        return true;
    };
    let cor = |valor: &[u8]| -> u8 {
        match legenda
            .variantes
            .iter()
            .position(|v| v.0.as_bytes() == valor)
        {
            Some(i) => i as u8 + 1,
            None if d.variantes.iter().any(|v| v.valor.as_bytes() == valor) => OUTROS,
            None => NEUTRO,
        }
    };
    let blocos = arquivo.linhas().div_ceil(BLOCO);
    (0..blocos).into_par_iter().for_each(|b| {
        if cancelar.load(Relaxed) {
            return;
        }
        for (i, linha) in arquivo.janela(b * BLOCO, BLOCO).into_iter().enumerate() {
            let m = &marcas[b * BLOCO + i];
            if m.load(Relaxed) == INVALIDA {
                continue;
            }
            // `as_raw_str` vem com as aspas; a legenda guarda o texto sem elas.
            if let Ok(v) = sonic_rs::get(linha, [chave.as_str()])
                && let Some(t) = v
                    .as_raw_str()
                    .strip_prefix('"')
                    .and_then(|t| t.strip_suffix('"'))
            {
                m.store(cor(t.as_bytes()), Relaxed);
            }
        }
    });
    !cancelar.load(Relaxed)
}

/// A régua em `k` baldes iguais (menos, se houver menos registros).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Baldes {
    /// Cor mais frequente entre as válidas do balde (`NEUTRO` se nenhuma tiver cor).
    pub cores: Vec<u8>,
    /// Baldes com inválidas: (balde, quantas, primeiro registro inválido).
    pub invalidas: Vec<(usize, usize, usize)>,
}

pub fn baldes(marcas: &[AtomicU8], k: usize) -> Baldes {
    let n = marcas.len();
    let k = k.min(n);
    let mut saida = Baldes {
        cores: Vec::with_capacity(k),
        invalidas: Vec::new(),
    };
    for b in 0..k {
        let (ini, fim) = (b * n / k, (b + 1) * n / k);
        let mut conta = [0usize; OUTROS as usize + 1];
        let (mut ruins, mut primeira) = (0, 0);
        for (r, m) in marcas[ini..fim].iter().enumerate() {
            match m.load(Relaxed) {
                INVALIDA => {
                    if ruins == 0 {
                        primeira = ini + r;
                    }
                    ruins += 1;
                }
                c => conta[c as usize] += 1,
            }
        }
        let cor = (1..=OUTROS).max_by_key(|&c| (conta[c as usize], u8::MAX - c));
        saida
            .cores
            .push(cor.filter(|&c| conta[c as usize] > 0).unwrap_or(NEUTRO));
        if ruins > 0 {
            saida.invalidas.push((b, ruins, primeira));
        }
    }
    saida
}

/// Marcas de `quantidade` registros a partir de `inicio` (o ponto de cor ao lado de cada um).
pub fn janela(marcas: &[AtomicU8], inicio: usize, quantidade: usize) -> Vec<u8> {
    let fim = inicio.saturating_add(quantidade).min(marcas.len());
    marcas
        .get(inicio..fim)
        .unwrap_or_default()
        .iter()
        .map(|m| m.load(Relaxed))
        .collect()
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::estrutura;
    use crate::resumo;

    fn arquivo(conteudo: &[u8]) -> ArquivoLinhas {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, conteudo).unwrap();
        ArquivoLinhas::abrir(f.path()).unwrap()
    }

    /// As duas passadas como o app faz: inválidas, inferência, cor.
    fn regua(dados: &[u8]) -> (Option<Legenda>, Box<[AtomicU8]>) {
        let a = arquivo(dados);
        let nao = AtomicBool::new(false);
        let m = marcas(a.linhas());
        resumo::varrer(&a, &nao, &m, |_| {});
        let inf = estrutura::inferir(a.bytes(), &nao, |_| {});
        let ds = inf.resumo.discriminadores();
        let Some(d) = escolher(&ds) else {
            return (None, m);
        };
        assert!(colorir(&a, d, &m, &nao));
        (Some(legenda(d)), m)
    }

    fn dados() -> Vec<u8> {
        // 6 tipos (frequências 6, 5, 4, 3, 2, 1) e inválidas juntas no meio.
        let mut d = Vec::new();
        for (tipo, n) in [("f", 1), ("e", 2), ("d", 3), ("c", 4), ("b", 5), ("a", 6)] {
            for i in 0..n {
                d.extend_from_slice(format!("{{\"tipo\":\"{tipo}\",\"{tipo}\":{i}}}\n").as_bytes());
            }
        }
        d.extend_from_slice(b"{\"tipo\":tru}\n\n");
        d
    }

    #[test]
    fn legenda_por_frequencia_e_marcas_por_registro() {
        let (l, m) = regua(&dados());
        let l = l.expect("discriminador");
        assert_eq!(l.campo, ".tipo");
        let nomes: Vec<&str> = l.variantes.iter().map(|v| v.0.as_str()).collect();
        assert_eq!(nomes, ["a", "b", "c", "d"]);
        assert_eq!(l.outros, 3);
        let todas = janela(&m, 0, usize::MAX);
        assert_eq!(todas.len(), 23);
        assert_eq!(&todas[..3], &[OUTROS, OUTROS, OUTROS]); // f, e, e
        assert_eq!(todas[3], 4); // primeiro d
        assert_eq!(todas[20], 1); // último a
        assert_eq!(&todas[21..], &[INVALIDA, INVALIDA]);
    }

    #[test]
    fn baldes_com_cor_dominante_e_inválidas() {
        let (_, m) = regua(&dados());
        let b = baldes(&m, 2);
        assert_eq!(b.cores, [3, 1]); // 0..11: c ganha de d (4 x 3); 11..23: a
        assert_eq!(b.invalidas, [(1, 2, 21)]);
        assert_eq!(baldes(&m, 1000).cores.len(), 23);
    }

    #[test]
    fn sem_discriminador_fica_neutra() {
        let (l, m) = regua(b"{\"a\":1}\n{\"a\":2}\n{\"a\":}\n");
        assert_eq!(l, None);
        assert_eq!(janela(&m, 0, 9), [NEUTRO, NEUTRO, INVALIDA]);
    }

    #[test]
    fn chave_do_campo() {
        assert_eq!(chave(".tipo").as_deref(), Some("tipo"));
        assert_eq!(chave(".[\"a b\"]").as_deref(), Some("a b"));
        assert_eq!(chave(".[\"a\\\"b\"]").as_deref(), Some("a\"b"));
    }
}
