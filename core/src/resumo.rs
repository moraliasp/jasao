//! Resumo do arquivo para o painel de entrada: o que o arquivo é e quantas linhas inválidas tem.
//!
//! - `conteudo` olha só o começo do arquivo (rápido, roda ao abrir).
//! - `varrer` valida todas as linhas em paralelo, fora da thread da interface, com progresso e
//!   cancelamento. "Inválida" é o que a Estrutura e o laudo contam: não é JSON para a árvore
//!   (`arvore::validar`) ou passa de `estrutura::PROFUNDIDADE_MAX` níveis (a árvore abre até
//!   1000). Linha vazia (só espaço) conta à parte, como no laudo, na Estrutura e em `erros`.
//! - `campos` conta as chaves do topo dos registros pela inferência (`estrutura::inferir`).

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering::Relaxed};
use std::thread;
use std::time::Duration;

use serde::Serialize;

use crate::ArquivoLinhas;
use crate::arvore::{passa_da_profundidade, validar};
use crate::{estrutura, regua};

/// Linhas olhadas por `conteudo` para dizer de que é feito um JSONL.
const AMOSTRA: usize = 1_000;
/// Linhas por tarefa de um trabalhador da varredura.
const BLOCO: usize = 4_096;
/// Intervalo entre avisos de progresso.
const AVISO: Duration = Duration::from_millis(50);

/// De que o arquivo é feito, pelo começo dele.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Conteudo {
    Vazio,
    /// Uma linha só.
    JsonUnico,
    /// Um JSON quebrado em várias linhas: a árvore o mostra linha a linha (lacuna conhecida),
    /// então contar "linhas inválidas" aqui enganaria.
    JsonVariasLinhas,
    /// Amostra válida só com objetos.
    Objetos,
    /// Amostra válida sem objetos.
    Valores,
    /// Objetos e outros valores, ou nenhuma linha válida na amostra.
    Misto,
}

pub fn conteudo(arquivo: &ArquivoLinhas) -> Conteudo {
    let linhas = arquivo.janela(0, AMOSTRA);
    match linhas.len() {
        0 => return Conteudo::Vazio,
        1 => return Conteudo::JsonUnico,
        _ => {}
    }
    let abre = |l: &[u8]| l.trim_ascii_start().first().copied();
    if validar(linhas[0]).is_err()
        && matches!(abre(linhas[0]), Some(b'{' | b'['))
        && validar(
            arquivo
                .bytes()
                .strip_prefix(crate::linhas::BOM)
                .unwrap_or(arquivo.bytes()),
        )
        .is_ok()
    {
        return Conteudo::JsonVariasLinhas;
    }
    let (mut objetos, mut outros) = (0, 0);
    for l in linhas.iter().filter(|l| validar(l).is_ok()) {
        if abre(l) == Some(b'{') {
            objetos += 1;
        } else {
            outros += 1;
        }
    }
    match (objetos, outros) {
        (1.., 0) => Conteudo::Objetos,
        (0, 1..) => Conteudo::Valores,
        _ => Conteudo::Misto,
    }
}

/// Estado da varredura. `varridas` conta linhas já validadas (não necessariamente as primeiras).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progresso {
    pub varridas: usize,
    pub total: usize,
    pub invalidas: usize,
    /// Linhas só com espaço em branco: nem registro nem inválida.
    pub vazias: usize,
    pub fim: bool,
    pub cancelado: bool,
}

/// Valida todas as linhas e marca as inválidas em `marcas` (uma por linha, ver `regua`).
/// `ao_progresso` roda na thread de quem chama, a cada ~50 ms e no fim. Ligar `cancelar` para
/// a varredura antes do próximo bloco.
pub fn varrer(
    arquivo: &ArquivoLinhas,
    cancelar: &AtomicBool,
    marcas: &[AtomicU8],
    mut ao_progresso: impl FnMut(Progresso),
) -> Progresso {
    let total = arquivo.linhas();
    let blocos = total.div_ceil(BLOCO);
    let trabalhadores = thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(blocos)
        .max(1);
    let (proximo, varridas, invalidas, vazias, ativos) = (
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(trabalhadores),
    );
    let agora = |fim: bool| Progresso {
        varridas: varridas.load(Relaxed),
        total,
        invalidas: invalidas.load(Relaxed),
        vazias: vazias.load(Relaxed),
        fim,
        cancelado: fim && varridas.load(Relaxed) < total,
    };
    thread::scope(|s| {
        for _ in 0..trabalhadores {
            s.spawn(|| {
                while !cancelar.load(Relaxed) {
                    let b = proximo.fetch_add(1, Relaxed);
                    if b >= blocos {
                        break;
                    }
                    let linhas = arquivo.janela(b * BLOCO, BLOCO);
                    let (mut ruins, mut brancas) = (0, 0);
                    for (i, l) in linhas.iter().enumerate() {
                        if l.iter().all(|c| matches!(c, b' ' | b'\t' | b'\r')) {
                            brancas += 1;
                        } else if validar(l).is_err()
                            || passa_da_profundidade(l, estrutura::PROFUNDIDADE_MAX).is_some()
                        {
                            marcas[b * BLOCO + i].store(regua::INVALIDA, Relaxed);
                            ruins += 1;
                        }
                    }
                    invalidas.fetch_add(ruins, Relaxed);
                    vazias.fetch_add(brancas, Relaxed);
                    varridas.fetch_add(linhas.len(), Relaxed);
                }
                ativos.fetch_sub(1, Relaxed);
            });
        }
        while ativos.load(Relaxed) > 0 {
            thread::sleep(AVISO);
            ao_progresso(agora(false));
        }
    });
    let fim = agora(true);
    ao_progresso(fim);
    fim
}

/// Chaves distintas do topo dos registros-objeto, pela inferência. `None` sem registro-objeto
/// ou com topo que é mapa (chaves variam demais para virar "campos").
pub fn campos(r: &estrutura::Resumo) -> Option<usize> {
    (r.raiz.mapa.is_none() && !r.raiz.filhos.is_empty()).then_some(r.raiz.filhos.len())
}

#[cfg(test)]
mod testes {
    use super::*;

    fn arquivo(conteudo: &[u8]) -> ArquivoLinhas {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, conteudo).unwrap();
        ArquivoLinhas::abrir(f.path()).unwrap()
    }

    #[test]
    fn conteudo_pelo_comeco() {
        let casos: &[(&[u8], Conteudo)] = &[
            (b"", Conteudo::Vazio),
            (b"{\"a\":1}", Conteudo::JsonUnico),
            (b"{\"a\":1}\n{\"b\":2}\n", Conteudo::Objetos),
            (b"{\"a\":1}\n{\"b\":tru}\n", Conteudo::Objetos),
            (b"1\n\"x\"\n[2]\n", Conteudo::Valores),
            (b"{\"a\":1}\n2\n", Conteudo::Misto),
            (b"{\n  \"a\": [1,\n 2]\n}\n", Conteudo::JsonVariasLinhas),
            (b"{\n  \"a\": \n", Conteudo::Misto),
        ];
        for (bytes, esperado) in casos {
            assert_eq!(
                conteudo(&arquivo(bytes)),
                *esperado,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn conta_invalidas_como_a_estrutura_e_vazias_a_parte() {
        let mut dados = Vec::new();
        for i in 0..10_000 {
            match i % 1000 {
                7 => dados.extend_from_slice(b"{\"a\":tru}\n"),
                // Aninhada acima do limite da inferência: inválida também aqui.
                9 if i == 9 => {
                    let n = estrutura::PROFUNDIDADE_MAX + 1;
                    dados.extend_from_slice(
                        format!("{}{}\n", "[".repeat(n), "]".repeat(n)).as_bytes(),
                    )
                }
                // No limite: registro.
                10 if i == 10 => {
                    let n = estrutura::PROFUNDIDADE_MAX;
                    dados.extend_from_slice(
                        format!("{}{}\n", "[".repeat(n), "]".repeat(n)).as_bytes(),
                    )
                }
                8 => dados.extend_from_slice(b"\n"),
                _ => dados.extend_from_slice(format!("{{\"i\":{i}}}\n").as_bytes()),
            }
        }
        let a = arquivo(&dados);
        let mut avisos = 0;
        let m = regua::marcas(a.linhas());
        let p = varrer(&a, &AtomicBool::new(false), &m, |_| avisos += 1);
        assert_eq!(
            p,
            Progresso {
                varridas: 10_000,
                total: 10_000,
                invalidas: 11,
                vazias: 10,
                fim: true,
                cancelado: false
            }
        );
        assert!(avisos >= 1);
    }

    #[test]
    fn cancelada_antes_de_comecar() {
        let a = arquivo(b"1\n2\n3\n");
        let p = varrer(&a, &AtomicBool::new(true), &regua::marcas(3), |_| {});
        assert_eq!((p.varridas, p.fim, p.cancelado), (0, true, true));
    }

    #[test]
    fn campos_do_topo() {
        let nao = AtomicBool::new(false);
        let casos: &[(&[u8], Option<usize>)] = &[
            (
                b"{\"a\":1,\"b\":{\"c\":2}}\n{\"a\":3,\"d\":4}\n{\"a\":tru}\n",
                Some(3),
            ),
            (b"[{\"a\":1},\n{\"b\":2}]\n", Some(2)),
            (b"1\n2\n", None),
            (b"", None),
        ];
        for (bytes, esperado) in casos {
            let inf = estrutura::inferir(bytes, &nao, |_| {});
            assert_eq!(
                campos(&inf.resumo),
                *esperado,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }
}
