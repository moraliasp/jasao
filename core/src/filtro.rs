//! Filtro de registros pela estrutura: os que têm (ou não têm) um caminho, ou os de uma variante
//! de união discriminada (`.tipo = "X"`). Roda sob pedido, numa varredura do arquivo inteiro em
//! 2º plano (a mesma do `perfil`), nunca no caminho da 1ª tela.
//!
//! O resultado é a lista crescente dos registros que passam, no mesmo espaço de índices da
//! Tabela (JSONL: linha do arquivo; JSON único: elemento da lista). A memória tem teto:
//! `MAX_FILTRADOS` índices de 4 bytes; ao passar dele a varredura para e devolve `Grande`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

use memchr::memchr_iter;
use rayon::prelude::*;
use serde::Serialize;

use crate::busca::{BLOCO_PADRAO, fatiar};
use crate::estrutura::{Entrada, Ev, Inferencia, Leitor, Seg, registros_json_unico};
use crate::linhas::BOM;
use crate::perfil::coletar;

/// Registros guardados no máximo (4 B cada: 200 MB), o mesmo teto de `tabela::MAX_REGISTROS`.
pub const MAX_FILTRADOS: usize = 50_000_000;

/// O que um registro precisa para passar.
#[derive(Debug, Clone, Copy)]
pub enum Criterio<'a> {
    /// Tem pelo menos um valor no caminho.
    Tem(&'a [Seg]),
    /// Não tem nenhum valor no caminho.
    NaoTem(&'a [Seg]),
    /// A 1ª ocorrência do caminho é o texto `valor` (cru, sem aspas, escapes intactos), como a
    /// variante de um discriminador conta.
    Igual(&'a [Seg], &'a [u8]),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filtro {
    /// Registros que passam, em ordem crescente.
    pub registros: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FalhaFiltro {
    Cancelada,
    /// Mais de `teto` registros passam (ou um índice não cabe em `u32`).
    Grande,
}

/// Registros que passam no `criterio`. `ao_progresso` recebe as linhas já varridas (só JSONL) e
/// roda nas threads de trabalho.
pub fn filtrar(
    inf: &Inferencia,
    dados: &[u8],
    criterio: Criterio,
    cancelar: &AtomicBool,
    ao_progresso: &(dyn Fn(u64) + Sync),
) -> Result<Filtro, FalhaFiltro> {
    filtrar_ate(
        inf,
        dados,
        criterio,
        BLOCO_PADRAO,
        MAX_FILTRADOS,
        cancelar,
        ao_progresso,
    )
}

/// `filtrar` com bloco e teto escolhidos (o app passa o teto do documento; testes, valores
/// pequenos).
pub fn filtrar_ate(
    inf: &Inferencia,
    dados: &[u8],
    criterio: Criterio,
    bloco: usize,
    teto: usize,
    cancelar: &AtomicBool,
    ao_progresso: &(dyn Fn(u64) + Sync),
) -> Result<Filtro, FalhaFiltro> {
    // Achados em todos os blocos: quem passaria do teto para ali, sem guardar.
    let achados = AtomicUsize::new(0);
    let estourou = AtomicBool::new(false);
    let guardar = |v: &mut Vec<u32>, r: u64| -> bool {
        match u32::try_from(r) {
            Ok(r) if achados.fetch_add(1, Relaxed) < teto => {
                v.push(r);
                true
            }
            _ => {
                estourou.store(true, Relaxed);
                false
            }
        }
    };
    let parar = || cancelar.load(Relaxed) || estourou.load(Relaxed);
    let registros = match inf.entrada {
        Entrada::Jsonl => {
            let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
            let blocos = fatiar(dados, base, bloco.max(1));
            let mut linha0 = Vec::with_capacity(blocos.len());
            let mut n = 0u64;
            for r in &blocos {
                linha0.push(n);
                n += memchr_iter(b'\n', &dados[r.clone()]).count() as u64;
            }
            let lidas = AtomicU64::new(0);
            let partes: Vec<Vec<u32>> = blocos
                .par_iter()
                .enumerate()
                .map(|(i, r)| {
                    let mut v = Vec::new();
                    let mut a = Avaliador::default();
                    let b = &dados[r.clone()];
                    let corpo = b.strip_suffix(b"\n").unwrap_or(b);
                    let mut j = 0u64;
                    for linha in corpo.split(|&c| c == b'\n') {
                        if parar() {
                            return v;
                        }
                        if a.passa(linha, criterio) && !guardar(&mut v, linha0[i] + j) {
                            return v;
                        }
                        j += 1;
                    }
                    ao_progresso(lidas.fetch_add(j, Relaxed) + j);
                    v
                })
                .collect();
            if parar() { Vec::new() } else { partes.concat() }
        }
        Entrada::JsonUnico => {
            let mut v = Vec::new();
            let mut a = Avaliador::default();
            let mut r = 0u64;
            registros_json_unico(dados, cancelar, |_, b| {
                let ok = !a.passa(b, criterio) || guardar(&mut v, r);
                r += 1;
                ok
            });
            v
        }
    };
    if cancelar.load(Relaxed) {
        return Err(FalhaFiltro::Cancelada);
    }
    if estourou.load(Relaxed) {
        return Err(FalhaFiltro::Grande);
    }
    Ok(Filtro { registros })
}

/// Lê um registro e confere o critério (reaproveita o buffer de eventos entre registros).
#[derive(Default)]
struct Avaliador {
    leitor: Leitor,
    evs: Vec<Ev>,
}

impl Avaliador {
    /// Linha em branco e registro inválido não passam em nenhum critério (como no `perfil`).
    fn passa(&mut self, b: &[u8], criterio: Criterio) -> bool {
        if b.iter().all(|c| matches!(c, b' ' | b'\t' | b'\r')) {
            return false;
        }
        self.evs.clear();
        self.leitor.p = 0;
        if !(self.leitor.valor(b, &mut self.evs) && self.leitor.resto_vazio(b)) {
            return false;
        }
        let segs = match criterio {
            Criterio::Tem(s) | Criterio::NaoTem(s) | Criterio::Igual(s, _) => s,
        };
        let mut primeiro: Option<Ev> = None;
        let mut visto = false;
        coletar(&self.evs, &mut 0, b, segs, &mut |ev| {
            if !visto {
                primeiro = Some(ev);
            }
            visto = true;
        });
        match criterio {
            Criterio::Tem(_) => visto,
            Criterio::NaoTem(_) => !visto,
            Criterio::Igual(_, valor) => {
                matches!(primeiro, Some(Ev::Texto(a, z)) if &b[a..z] == valor)
            }
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::estrutura::{Caminho, inferir_em_blocos};

    fn nao() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn caminho(i: &Inferencia, c: &str) -> Caminho {
        i.resumo
            .caminhos()
            .into_iter()
            .find(|x| x.caminho == c)
            .unwrap_or_else(|| panic!("sem {c}"))
    }

    fn rodar(dados: &[u8], f: impl Fn(&Caminho) -> Criterio, c: &str, bloco: usize) -> Vec<u32> {
        let i = inferir_em_blocos(dados, 7, &nao(), |_| {});
        let cam = caminho(&i, c);
        filtrar_ate(&i, dados, f(&cam), bloco, usize::MAX, &nao(), &|_| {})
            .unwrap()
            .registros
    }

    const JSONL: &str = "{\"tipo\":\"a\",\"x\":1}\n\n{\"tipo\":\"b\",\"y\":2}\n{oops\n{\"tipo\":\"a\",\"x\":{\"z\":3}}\n{\"tipo\":\"b\",\"tipo\":\"a\",\"y\":4}\n";

    #[test]
    fn tem_nao_tem_e_igual_no_jsonl() {
        for bloco in [1, 7, 100, 4096] {
            let d = JSONL.as_bytes();
            assert_eq!(rodar(d, |c| Criterio::Tem(&c.segs), ".x", bloco), [0, 4]);
            // Linha em branco (1) e inválida (3) não entram em nenhum dos dois.
            assert_eq!(rodar(d, |c| Criterio::NaoTem(&c.segs), ".x", bloco), [2, 5]);
            assert_eq!(rodar(d, |c| Criterio::Tem(&c.segs), ".x.z", bloco), [4]);
            // Chave repetida: vale a 1ª, como a variante conta.
            assert_eq!(
                rodar(d, |c| Criterio::Igual(&c.segs, b"a"), ".tipo", bloco),
                [0, 4]
            );
            assert_eq!(
                rodar(d, |c| Criterio::Igual(&c.segs, b"b"), ".tipo", bloco),
                [2, 5]
            );
        }
    }

    #[test]
    fn igual_bate_com_a_contagem_da_variante() {
        let d = JSONL.as_bytes();
        let i = inferir_em_blocos(d, 7, &nao(), |_| {});
        let disc = &i.resumo.discriminadores()[0];
        let cam = caminho(&i, &disc.campo);
        for v in &disc.variantes {
            let r = filtrar(
                &i,
                d,
                Criterio::Igual(&cam.segs, v.valor.as_bytes()),
                &nao(),
                &|_| {},
            )
            .unwrap();
            assert_eq!(r.registros.len() as u64, v.registros.n, "{}", v.valor);
        }
    }

    #[test]
    fn json_unico_conta_elementos() {
        let d = b"[{\"a\":1},\n {\"b\":2},\n {\"a\":3}]";
        assert_eq!(rodar(d, |c| Criterio::Tem(&c.segs), ".a", 4096), [0, 2]);
        assert_eq!(rodar(d, |c| Criterio::NaoTem(&c.segs), ".a", 4096), [1]);
    }

    #[test]
    fn escapes_comparam_crus() {
        let d = b"{\"t\":\"a\\\"b\"}\n{\"t\":\"c\"}\n";
        assert_eq!(
            rodar(d, |c| Criterio::Igual(&c.segs, b"a\\\"b"), ".t", 1),
            [0]
        );
    }

    #[test]
    fn teto_recusa_e_para_cedo() {
        let mut d = String::new();
        for i in 0..1000 {
            d.push_str(&format!("{{\"x\":{i}}}\n"));
        }
        let i = inferir_em_blocos(d.as_bytes(), 7, &nao(), |_| {});
        let cam = caminho(&i, ".x");
        let lidas = AtomicU64::new(0);
        let prog = |n| {
            lidas.fetch_max(n, Relaxed);
        };
        let r = filtrar_ate(
            &i,
            d.as_bytes(),
            Criterio::Tem(&cam.segs),
            1,
            3,
            &nao(),
            &prog,
        );
        assert_eq!(r, Err(FalhaFiltro::Grande));
        // Parou longe do fim: não juntou o arquivo inteiro para recusar depois.
        assert!(lidas.load(Relaxed) < 500, "{}", lidas.load(Relaxed));
        let r = filtrar_ate(
            &i,
            d.as_bytes(),
            Criterio::Tem(&cam.segs),
            1,
            1000,
            &nao(),
            &prog,
        );
        assert_eq!(r.unwrap().registros.len(), 1000);
        // JSON único também.
        let j = format!("[{}]", ["{\"x\":1}"; 10].join(","));
        let i = inferir_em_blocos(j.as_bytes(), 7, &nao(), |_| {});
        let cam = caminho(&i, ".x");
        let r = filtrar_ate(
            &i,
            j.as_bytes(),
            Criterio::Tem(&cam.segs),
            1,
            9,
            &nao(),
            &|_| {},
        );
        assert_eq!(r, Err(FalhaFiltro::Grande));
    }

    #[test]
    fn cancelado_nao_devolve_parcial() {
        let sim = AtomicBool::new(true);
        for d in [JSONL.as_bytes(), b"[{\"x\":1}]".as_slice()] {
            let i = inferir_em_blocos(d, 7, &nao(), |_| {});
            let cam = caminho(&i, ".x");
            let r = filtrar(&i, d, Criterio::Tem(&cam.segs), &sim, &|_| {});
            assert_eq!(r, Err(FalhaFiltro::Cancelada));
        }
    }
}
