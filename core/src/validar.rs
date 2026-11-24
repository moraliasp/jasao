//! Autovalidação: o arquivo inteiro contra um JSON Schema 2020-12, com um validador de
//! terceiros (`jsonschema`), independente do gerador (`crate::exportar`). Cada registro é lido
//! pelo `serde_json` (não pelo leitor da inferência) e conferido em paralelo.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc;
use std::thread;

use memchr::memchr_iter;
use rayon::prelude::*;

use crate::busca::{BLOCO_PADRAO, fatiar};
use crate::estrutura::{Entrada, Progresso, registros_json_unico};
use crate::linhas::BOM;

/// Quantas falhas guardar (as de linha menor).
pub const MAX_FALHAS: usize = 20;
/// Motivo maior que isto é cortado.
const MAX_MOTIVO: usize = 300;
/// Registros de JSON único conferidos por lote paralelo.
const LOTE_UNICO: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Falha {
    /// Linha (a partir de 0) onde o registro começa.
    pub linha: u64,
    pub motivo: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validacao {
    pub passam: u64,
    /// Registros que o `serde_json` leu e o schema reprovou.
    pub falham: u64,
    /// Linhas que o `serde_json` não leu como JSON (não contam como registro).
    pub invalidas: u64,
    /// As primeiras `MAX_FALHAS` (reprovadas e inválidas), por linha.
    pub falhas: Vec<Falha>,
    pub varridas: u64,
    pub total: u64,
    pub cancelado: bool,
}

impl Validacao {
    fn combinar(mut self, o: Validacao) -> Validacao {
        self.passam += o.passam;
        self.falham += o.falham;
        self.invalidas += o.invalidas;
        self.falhas.extend(o.falhas);
        self.falhas.sort_by_key(|f| f.linha);
        self.falhas.truncate(MAX_FALHAS);
        self
    }

    fn conferir(&mut self, v: &jsonschema::Validator, linha: u64, b: &[u8]) {
        let mut falha = |motivo: String| {
            if self.falhas.len() < MAX_FALHAS {
                let mut motivo = motivo;
                if motivo.len() > MAX_MOTIVO {
                    let mut c = MAX_MOTIVO;
                    while !motivo.is_char_boundary(c) {
                        c -= 1;
                    }
                    motivo.truncate(c);
                    motivo.push('…');
                }
                self.falhas.push(Falha { linha, motivo });
            }
        };
        match serde_json::from_slice::<serde_json::Value>(b) {
            Err(e) => {
                falha(format!("não é JSON válido para o validador: {e}"));
                self.invalidas += 1;
            }
            Ok(j) => match v.validate(&j) {
                Ok(()) => self.passam += 1,
                Err(e) => {
                    falha(motivo(&e));
                    self.falham += 1;
                }
            },
        }
    }
}

/// Caminho e mensagem (sem o valor do arquivo). Em `anyOf` (união), o erro do ramo que chegou
/// mais perto: o de menos erros.
fn motivo(e: &jsonschema::ValidationError) -> String {
    if let jsonschema::error::ValidationErrorKind::AnyOf { context } = e.kind()
        && let Some(ramo) = context
            .iter()
            .filter(|r| !r.is_empty())
            .min_by_key(|r| r.len())
    {
        return motivo(&ramo[0]);
    }
    let onde = e.instance_path().to_string();
    let msg = e.masked().to_string();
    if onde.is_empty() {
        msg
    } else {
        format!("{onde}: {msg}")
    }
}

/// Valida cada registro de `dados` (lido como `entrada`, a mesma da inferência) contra
/// `schema`. Progresso e cancelamento como em `estrutura::inferir`. `Err` se o schema não
/// compilar.
pub fn validar(
    dados: &[u8],
    entrada: Entrada,
    schema: &str,
    cancelar: &AtomicBool,
    mut ao_progresso: impl FnMut(Progresso),
) -> Result<Validacao, String> {
    let s: serde_json::Value = serde_json::from_str(schema).map_err(|e| e.to_string())?;
    let v = jsonschema::draft202012::new(&s).map_err(|e| e.to_string())?;
    let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
    let corpo = &dados[base..];
    let total = (memchr_iter(b'\n', corpo).count()
        + usize::from(!corpo.is_empty() && !corpo.ends_with(b"\n"))) as u64;
    let mut r = match entrada {
        Entrada::Jsonl => jsonl(dados, base, total, &v, cancelar, &mut ao_progresso),
        Entrada::JsonUnico => json_unico(dados, total, &v, cancelar, &mut ao_progresso),
    };
    r.total = total;
    Ok(r)
}

fn jsonl(
    dados: &[u8],
    base: usize,
    total: u64,
    v: &jsonschema::Validator,
    cancelar: &AtomicBool,
    ao_progresso: &mut impl FnMut(Progresso),
) -> Validacao {
    let blocos = fatiar(dados, base, BLOCO_PADRAO);
    let mut linha0 = Vec::with_capacity(blocos.len());
    let mut acc = 0u64;
    for n in blocos
        .par_iter()
        .map(|r| memchr_iter(b'\n', &dados[r.clone()]).count() as u64)
        .collect::<Vec<_>>()
    {
        linha0.push(acc);
        acc += n;
    }
    let mut varridas = 0;
    let mut r = thread::scope(|s| {
        let (tx, rx) = mpsc::channel::<u64>();
        let (blocos, linha0) = (&blocos, &linha0);
        let h = s.spawn(move || {
            blocos
                .par_iter()
                .enumerate()
                .map(|(i, faixa)| {
                    let mut r = Validacao::default();
                    if cancelar.load(Relaxed) {
                        return r;
                    }
                    let b = &dados[faixa.clone()];
                    let corpo = b.strip_suffix(b"\n").unwrap_or(b);
                    for (j, l) in corpo.split(|&c| c == b'\n').enumerate() {
                        if !l.iter().all(|c| matches!(c, b' ' | b'\t' | b'\r')) {
                            r.conferir(v, linha0[i] + j as u64, l);
                        }
                    }
                    let n = memchr_iter(b'\n', b).count() + usize::from(!b.ends_with(b"\n"));
                    let _ = tx.send(n as u64);
                    r
                })
                .reduce(Validacao::default, Validacao::combinar)
        });
        let mut ultimo = 0;
        for n in rx {
            varridas += n;
            if varridas == total || varridas - ultimo >= total / 100 {
                ultimo = varridas;
                ao_progresso(Progresso { varridas, total });
            }
        }
        h.join().expect("validação em pânico")
    });
    r.varridas = varridas;
    r.cancelado = varridas < total;
    r
}

fn json_unico(
    dados: &[u8],
    total: u64,
    v: &jsonschema::Validator,
    cancelar: &AtomicBool,
    ao_progresso: &mut impl FnMut(Progresso),
) -> Validacao {
    let conferir_lote = |lote: &[(u64, &[u8])]| {
        lote.par_iter()
            .fold(Validacao::default, |mut r, &(linha, b)| {
                r.conferir(v, linha, b);
                r
            })
            .reduce(Validacao::default, Validacao::combinar)
    };
    let mut r = Validacao::default();
    let mut lote = Vec::with_capacity(LOTE_UNICO);
    let mut ultimo = 0;
    let fim = registros_json_unico(dados, cancelar, |linha, b| {
        lote.push((linha, b));
        if lote.len() == LOTE_UNICO {
            r = std::mem::take(&mut r).combinar(conferir_lote(&lote));
            lote.clear();
            if linha - ultimo >= total / 100 {
                ultimo = linha;
                ao_progresso(Progresso {
                    varridas: linha,
                    total,
                });
            }
        }
        true
    });
    r = r.combinar(conferir_lote(&lote));
    r.cancelado = cancelar.load(Relaxed);
    r.varridas = if r.cancelado { fim } else { total };
    ao_progresso(Progresso {
        varridas: r.varridas,
        total,
    });
    r
}
