//! Diff de estrutura entre duas inferências (`antes` e `depois`): cada mudança com caminho,
//! regra, classe (quebra × compatível) e evidência "N de M" nos dois lados.
//!
//! Quebra = um consumidor OU um validador escrito para o arquivo de antes pode falhar com o de
//! depois; na dúvida, quebra (é um gate de CI). Por isso só campo novo opcional é compatível:
//! tipo que muda em qualquer direção (inclusive alargar: `null` novo chega a quem lia inteiro),
//! presença que muda em qualquer direção e variante nova ou removida quebram. Critérios e limites:
//! `docs/decisoes.md`.

use std::fmt;

use crate::estrutura::{Discriminador, No, Resumo, Tipo, Unico, juntar, segmento};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classe {
    Quebra,
    Compativel,
}

impl Classe {
    pub fn nome(self) -> &'static str {
        match self {
            Classe::Quebra => "quebra",
            Classe::Compativel => "compativel",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regra {
    CampoRemovido,
    CampoNovoOpcional,
    CampoNovoObrigatorio,
    VirouObrigatorio,
    VirouOpcional,
    /// Os tipos de depois são um subconjunto estrito dos de antes.
    TipoEstreitado,
    /// Os tipos de depois contêm os de antes.
    TipoAlargado,
    /// Perdeu um tipo e ganhou outro (ex.: inteiro → texto).
    TipoTrocado,
    VarianteRemovida,
    VarianteNova,
}

impl Regra {
    pub fn classe(self) -> Classe {
        match self {
            Regra::CampoNovoOpcional => Classe::Compativel,
            _ => Classe::Quebra,
        }
    }

    /// Identificador estável (saída `--json`).
    pub fn nome(self) -> &'static str {
        match self {
            Regra::CampoRemovido => "campo_removido",
            Regra::CampoNovoOpcional => "campo_novo_opcional",
            Regra::CampoNovoObrigatorio => "campo_novo_obrigatorio",
            Regra::VirouObrigatorio => "virou_obrigatorio",
            Regra::VirouOpcional => "virou_opcional",
            Regra::TipoEstreitado => "tipo_estreitado",
            Regra::TipoAlargado => "tipo_alargado",
            Regra::TipoTrocado => "tipo_trocado",
            Regra::VarianteRemovida => "variante_removida",
            Regra::VarianteNova => "variante_nova",
        }
    }
}

/// `n` de `de`. Campo: objetos-pai em que aparece de quantos objetos-pai. Tipo: valores dos tipos
/// que mudaram de quantos valores no caminho. Variante: registros dela de quantos registros-objeto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evidencia {
    pub n: u64,
    pub de: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detalhe {
    Nenhum,
    /// Tipos vistos antes e depois.
    Tipos(Vec<Tipo>, Vec<Tipo>),
    /// Valor da variante, como está no arquivo (escapes intactos).
    Variante(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mudanca {
    pub caminho: String,
    pub regra: Regra,
    pub antes: Evidencia,
    pub depois: Evidencia,
    pub detalhe: Detalhe,
}

/// Uma linha: `quebra      .a.b  tipo estreitado: inteiro, texto → inteiro  (3 de 10 → 0 de 8)`.
impl fmt::Display for Mudanca {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let classe = match self.regra.classe() {
            Classe::Quebra => "quebra",
            Classe::Compativel => "compatível",
        };
        let regra = match self.regra {
            Regra::CampoRemovido => "campo removido",
            Regra::CampoNovoOpcional => "campo novo opcional",
            Regra::CampoNovoObrigatorio => "campo novo obrigatório",
            Regra::VirouObrigatorio => "virou obrigatório",
            Regra::VirouOpcional => "virou opcional",
            Regra::TipoEstreitado => "tipo estreitado",
            Regra::TipoAlargado => "tipo alargado",
            Regra::TipoTrocado => "tipo trocado",
            Regra::VarianteRemovida => "variante removida",
            Regra::VarianteNova => "variante nova",
        };
        let nomes = |ts: &[Tipo]| ts.iter().map(|t| t.nome()).collect::<Vec<_>>().join(", ");
        let detalhe = match &self.detalhe {
            Detalhe::Nenhum => String::new(),
            Detalhe::Tipos(a, b) => format!(": {} → {}", nomes(a), nomes(b)),
            Detalhe::Variante(v) => format!(" \"{v}\""),
        };
        let (a, b) = (self.antes, self.depois);
        write!(
            f,
            "{classe:<11} {}  {regra}{detalhe}  ({} de {} → {} de {})",
            self.caminho, a.n, a.de, b.n, b.de
        )
    }
}

/// Mudanças de `antes` para `depois` (resumos já finalizados), na ordem da árvore de `antes`
/// (caminhos só de `depois` em seguida, na ordem dele), e as de variante no fim.
pub fn diff(antes: &Resumo, depois: &Resumo) -> Vec<Mudanca> {
    let mut v = Vec::new();
    comparar(
        &antes.raiz,
        antes.registros(),
        &depois.raiz,
        depois.registros(),
        ".",
        false,
        &mut v,
    );
    variantes(antes, depois, &mut v);
    v
}

/// Caminhos dos dois lados sem nenhuma mudança, na ordem de `antes` (o grupo "igual" da tela).
pub fn iguais(antes: &Resumo, depois: &Resumo, mudancas: &[Mudanca]) -> Vec<String> {
    let mudou: std::collections::HashSet<&str> =
        mudancas.iter().map(|m| m.caminho.as_str()).collect();
    let nos_dois: std::collections::HashSet<String> =
        depois.caminhos().into_iter().map(|c| c.caminho).collect();
    antes
        .caminhos()
        .into_iter()
        .map(|c| c.caminho)
        .filter(|c| nos_dois.contains(c) && !mudou.contains(c.as_str()))
        .collect()
}

const INTEIRO: u8 = 1 << Tipo::Inteiro as u8;
const DECIMAL: u8 = 1 << Tipo::Decimal as u8;

/// O conjunto de tipos `m` aceita todo valor de `outro`. Decimal aceita inteiro (`number`).
fn aceita(m: u8, outro: u8) -> bool {
    let m = if m & DECIMAL != 0 { m | INTEIRO } else { m };
    outro & !m == 0
}

fn tipos(m: u8) -> Vec<Tipo> {
    Tipo::TODOS
        .into_iter()
        .filter(|&t| m >> t as u8 & 1 == 1)
        .collect()
}

fn soma(no: &No, m: u8) -> u64 {
    tipos(m).into_iter().map(|t| no.tipo(t).n).sum()
}

fn ordenados(no: &No) -> Vec<(&[u8], &No)> {
    let mut f: Vec<_> = no.filhos.iter().map(|(k, f)| (&**k, f)).collect();
    f.sort_by(|a, b| (a.1.ordem(), a.0).cmp(&(b.1.ordem(), b.0)));
    f
}

/// `de_a`/`de_b`: o `de` do caminho em cada lado; `campo`: aplica as regras de presença.
fn comparar(
    a: &No,
    de_a: u64,
    b: &No,
    de_b: u64,
    caminho: &str,
    campo: bool,
    v: &mut Vec<Mudanca>,
) {
    let mut por = |regra, antes: (u64, u64), depois: (u64, u64), detalhe| {
        v.push(Mudanca {
            caminho: caminho.to_string(),
            regra,
            antes: Evidencia {
                n: antes.0,
                de: antes.1,
            },
            depois: Evidencia {
                n: depois.0,
                de: depois.1,
            },
            detalhe,
        })
    };
    // Campo existe nos dois lados só se o pai foi objeto nos dois: `de_a`, `de_b` > 0.
    if campo {
        match (a.presenca == de_a, b.presenca == de_b) {
            (false, true) => por(
                Regra::VirouObrigatorio,
                (a.presenca, de_a),
                (b.presenca, de_b),
                Detalhe::Nenhum,
            ),
            (true, false) => por(
                Regra::VirouOpcional,
                (a.presenca, de_a),
                (b.presenca, de_b),
                Detalhe::Nenhum,
            ),
            _ => {}
        }
    }
    let (ma, mb) = (a.conjunto(), b.conjunto());
    let regra = match (aceita(mb, ma), aceita(ma, mb)) {
        (true, true) => None,
        (true, false) => Some(Regra::TipoAlargado),
        (false, true) => Some(Regra::TipoEstreitado),
        (false, false) => Some(Regra::TipoTrocado),
    };
    if let Some(r) = regra {
        let mudou = ma ^ mb;
        por(
            r,
            (soma(a, mudou), soma(a, 0x7f)),
            (soma(b, mudou), soma(b, 0x7f)),
            Detalhe::Tipos(tipos(ma), tipos(mb)),
        );
    }

    let (oa, ob) = (a.tipo(Tipo::Objeto).n, b.tipo(Tipo::Objeto).n);
    // Lado sem nenhum objeto: a troca de tipo acima já diz tudo; os filhos seriam ruído.
    for (k, fa) in ordenados(a) {
        let c = juntar(caminho, &segmento(k));
        match b.filhos.get(k) {
            Some(fb) => comparar(fa, oa, fb, ob, &c, true, v),
            None if ob > 0 => v.push(Mudanca {
                caminho: c,
                regra: Regra::CampoRemovido,
                antes: Evidencia {
                    n: fa.presenca,
                    de: oa,
                },
                depois: Evidencia { n: 0, de: ob },
                detalhe: Detalhe::Nenhum,
            }),
            None => {}
        }
    }
    for (k, fb) in ordenados(b) {
        if oa == 0 || a.filhos.contains_key(k) {
            continue;
        }
        v.push(Mudanca {
            caminho: juntar(caminho, &segmento(k)),
            regra: if fb.presenca == ob {
                Regra::CampoNovoObrigatorio
            } else {
                Regra::CampoNovoOpcional
            },
            antes: Evidencia { n: 0, de: oa },
            depois: Evidencia {
                n: fb.presenca,
                de: ob,
            },
            detalhe: Detalhe::Nenhum,
        });
    }
    // Elemento ou entrada de um lado só: listas/mapas vazios do outro, não é mudança de forma.
    if let (Some(x), Some(y)) = (&a.mapa, &b.mapa) {
        comparar(x, oa, y, ob, &juntar(caminho, ".*"), false, v);
    }
    if let (Some(x), Some(y)) = (&a.elementos, &b.elementos) {
        let (la, lb) = (a.tipo(Tipo::Lista).n, b.tipo(Tipo::Lista).n);
        comparar(x, la, y, lb, &juntar(caminho, "[]"), false, v);
    }
}

/// Variantes de um campo que é discriminador em pelo menos um lado. No outro lado, o campo vale
/// se também for discriminador ou se tiver um valor só (texto em todo registro-objeto).
fn variantes(antes: &Resumo, depois: &Resumo, v: &mut Vec<Mudanca>) {
    let (da, db) = (antes.discriminadores(), depois.discriminadores());
    let mut campos: Vec<&Discriminador> = da.iter().collect();
    campos.extend(db.iter().filter(|d| !da.iter().any(|x| x.cru == d.cru)));
    let (oa, ob) = (
        antes.raiz.tipo(Tipo::Objeto).n,
        depois.raiz.tipo(Tipo::Objeto).n,
    );
    for d in campos {
        let (Some(va), Some(vb)) = (valores(antes, &da, &d.cru), valores(depois, &db, &d.cru))
        else {
            continue;
        };
        for (valor, n) in &va {
            if !vb.iter().any(|x| x.0 == *valor) {
                v.push(Mudanca {
                    caminho: d.campo.clone(),
                    regra: Regra::VarianteRemovida,
                    antes: Evidencia { n: *n, de: oa },
                    depois: Evidencia { n: 0, de: ob },
                    detalhe: Detalhe::Variante(valor.clone()),
                });
            }
        }
        for (valor, n) in &vb {
            if !va.iter().any(|x| x.0 == *valor) {
                v.push(Mudanca {
                    caminho: d.campo.clone(),
                    regra: Regra::VarianteNova,
                    antes: Evidencia { n: 0, de: oa },
                    depois: Evidencia { n: *n, de: ob },
                    detalhe: Detalhe::Variante(valor.clone()),
                });
            }
        }
    }
}

/// Valores (e registros de cada um) do campo `cru` do topo, se ele separa variantes.
fn valores(r: &Resumo, ds: &[Discriminador], cru: &[u8]) -> Option<Vec<(String, u64)>> {
    if let Some(d) = ds.iter().find(|d| *d.cru == *cru) {
        return Some(
            d.variantes
                .iter()
                .map(|x| (x.valor.clone(), x.registros.n))
                .collect(),
        );
    }
    let no = r.raiz.filhos.get(cru)?;
    let todo_texto = no.conjunto() == 1 << Tipo::Texto as u8;
    match &no.valores.unico {
        Unico::Um(Tipo::Texto, x) if todo_texto && no.presenca == r.raiz.tipo(Tipo::Objeto).n => {
            Some(vec![(String::from_utf8_lossy(x).into_owned(), no.presenca)])
        }
        _ => None,
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::estrutura::inferir_em_blocos;
    use proptest::prelude::*;
    use serde_json::Value;
    use std::sync::atomic::AtomicBool;

    fn resumo(dados: &str) -> Resumo {
        inferir_em_blocos(dados.as_bytes(), 7, &AtomicBool::new(false), |_| {}).resumo
    }

    fn linhas(a: &str, b: &str) -> Vec<String> {
        diff(&resumo(a), &resumo(b))
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// Igual = nos dois lados e sem mudança: o removido, o novo e o que mudou ficam de fora.
    #[test]
    fn iguais_fora_das_mudancas() {
        let (a, b) = (
            resumo("{\"a\":1,\"b\":\"x\",\"c\":[1]}\n"),
            resumo("{\"a\":1,\"b\":2,\"c\":[1],\"d\":0}\n"),
        );
        let m = diff(&a, &b);
        assert_eq!(iguais(&a, &b, &m), [".", ".a", ".c", ".c[]"]);
        assert_eq!(iguais(&a, &a, &[]).len(), a.caminhos().len());
    }

    /// Dourados: uma regra da taxonomia por caso, com a linha exata que sai.
    #[test]
    fn dourados_por_regra() {
        let casos: &[(&str, &str, &[&str])] = &[
            (
                "{\"a\":1,\"b\":2}\n{\"a\":3,\"b\":4}\n",
                "{\"a\":1}\n{\"a\":3}\n{\"a\":5}\n",
                &["quebra      .b  campo removido  (2 de 2 → 0 de 3)"],
            ),
            (
                "{\"a\":1}\n{\"a\":2}\n",
                "{\"a\":1,\"n\":true}\n{\"a\":2}\n",
                &["compatível  .n  campo novo opcional  (0 de 2 → 1 de 2)"],
            ),
            (
                "{\"a\":1}\n{\"a\":2}\n",
                "{\"a\":1,\"n\":true}\n{\"a\":2,\"n\":false}\n",
                &["quebra      .n  campo novo obrigatório  (0 de 2 → 2 de 2)"],
            ),
            (
                "{\"a\":1}\n{\"b\":2}\n",
                "{\"a\":1,\"b\":2}\n",
                &[
                    "quebra      .a  virou obrigatório  (1 de 2 → 1 de 1)",
                    "quebra      .b  virou obrigatório  (1 de 2 → 1 de 1)",
                ],
            ),
            (
                "{\"a\":1}\n",
                "{\"a\":1}\n{}\n",
                &["quebra      .a  virou opcional  (1 de 1 → 1 de 2)"],
            ),
            (
                "{\"v\":1}\n{\"v\":\"x\"}\n{\"v\":null}\n",
                "{\"v\":2}\n{\"v\":3}\n",
                &[
                    "quebra      .v  tipo estreitado: nulo, inteiro, texto → inteiro  (2 de 3 → 0 de 2)",
                ],
            ),
            (
                "{\"v\":1}\n",
                "{\"v\":1}\n{\"v\":null}\n{\"v\":2.5}\n",
                &[
                    "quebra      .v  tipo alargado: inteiro → nulo, inteiro, decimal  (0 de 1 → 2 de 3)",
                ],
            ),
            (
                "{\"v\":1}\n",
                "{\"v\":\"1\"}\n",
                &["quebra      .v  tipo trocado: inteiro → texto  (1 de 1 → 1 de 1)"],
            ),
            (
                "{\"t\":\"a\",\"x\":1}\n{\"t\":\"b\",\"y\":2}\n{\"t\":\"c\",\"z\":3}\n",
                "{\"t\":\"a\",\"x\":1}\n{\"t\":\"b\",\"y\":2}\n",
                &[
                    "quebra      .z  campo removido  (1 de 3 → 0 de 2)",
                    "quebra      .t  variante removida \"c\"  (1 de 3 → 0 de 2)",
                ],
            ),
            (
                "{\"t\":\"a\",\"x\":1}\n{\"t\":\"a\",\"x\":2}\n",
                "{\"t\":\"a\",\"x\":1}\n{\"t\":\"b\",\"y\":2}\n",
                &[
                    "quebra      .x  virou opcional  (2 de 2 → 1 de 2)",
                    "compatível  .y  campo novo opcional  (0 de 2 → 1 de 2)",
                    "quebra      .t  variante nova \"b\"  (0 de 2 → 1 de 2)",
                ],
            ),
        ];
        for (a, b, esperado) in casos {
            assert_eq!(linhas(a, b), *esperado, "\nantes:\n{a}depois:\n{b}");
        }
    }

    #[test]
    fn aninhado_elemento_e_mapa() {
        // Só o pai muda de tipo: os filhos que somem com ele não viram ruído.
        assert_eq!(
            linhas("{\"o\":{\"a\":1}}\n", "{\"o\":3}\n"),
            ["quebra      .o  tipo trocado: objeto → inteiro  (1 de 1 → 1 de 1)"]
        );
        assert_eq!(
            linhas(
                "{\"l\":[{\"a\":1,\"b\":2}]}\n",
                "{\"l\":[{\"a\":1}]}\n{\"l\":[]}\n"
            ),
            ["quebra      .l[].b  campo removido  (1 de 1 → 0 de 1)"]
        );
        // Lista que ficou vazia não remove o `[]`.
        assert!(linhas("{\"l\":[1]}\n", "{\"l\":[]}\n").is_empty());
        let mapa = |v: &str| {
            (0..25)
                .map(|i| format!("{{\"m\":{{\"k{i}\":{v}}}}}\n"))
                .collect::<String>()
        };
        assert_eq!(
            linhas(&mapa("1"), &mapa("1.5")),
            ["quebra      .m.*  tipo alargado: inteiro → decimal  (25 de 25 → 25 de 25)"]
        );
        // Decimal aceita inteiro: {inteiro, decimal} → {decimal} não é mudança.
        assert!(linhas("{\"v\":1}\n{\"v\":1.5}\n", "{\"v\":2.5}\n").is_empty());
        assert_eq!(
            linhas("{\"v\":1.5}\n", "{\"v\":2}\n"),
            ["quebra      .v  tipo estreitado: decimal → inteiro  (1 de 1 → 1 de 1)"]
        );
    }

    /// Alargar também quebra: `null` ou decimal novo chega a quem lia só inteiro.
    #[test]
    fn alargar_quebra() {
        assert_eq!(
            linhas("{\"a\":1}\n", "{\"a\":1}\n{\"a\":null}\n"),
            ["quebra      .a  tipo alargado: inteiro → nulo, inteiro  (0 de 1 → 1 de 2)"]
        );
        assert_eq!(
            linhas("{\"a\":1}\n", "{\"a\":1}\n{\"a\":1.5}\n"),
            ["quebra      .a  tipo alargado: inteiro → inteiro, decimal  (0 de 1 → 1 de 2)"]
        );
        assert_eq!(
            linhas("{\"o\":{\"k\":1}}\n", "{\"o\":{\"k\":1}}\n{\"o\":null}\n"),
            ["quebra      .o  tipo alargado: objeto → nulo, objeto  (0 de 1 → 1 de 2)"]
        );
    }

    /// Guardas de `valores()` e da presença: cada caso cai se a guarda sair.
    #[test]
    fn guardas() {
        let ab = "{\"t\":\"a\",\"x\":1}\n{\"t\":\"b\",\"y\":2}\n";
        // Um valor só mas ausente em algum registro: não conta como variante única.
        assert_eq!(
            linhas(ab, "{\"t\":\"a\",\"x\":1}\n{\"y\":2}\n"),
            ["quebra      .t  virou opcional  (2 de 2 → 1 de 2)"]
        );
        // Um texto só, mas com `null` (fora do `unico`) em outro registro: idem.
        assert_eq!(
            linhas(ab, "{\"t\":\"a\",\"x\":1}\n{\"t\":null,\"y\":2}\n"),
            ["quebra      .t  tipo alargado: texto → nulo, texto  (0 de 2 → 1 de 2)"]
        );
        // Presença em `[]` conta elementos, não listas: sem regra de presença ali.
        assert!(linhas("{\"l\":[1,2]}\n", "{\"l\":[1]}\n").is_empty());
        assert!(linhas("{\"l\":[1]}\n", "{\"l\":[1,2]}\n").is_empty());
    }

    #[test]
    fn classe_e_nome_estaveis() {
        let compat = [Regra::CampoNovoOpcional];
        for r in [
            Regra::CampoRemovido,
            Regra::CampoNovoOpcional,
            Regra::CampoNovoObrigatorio,
            Regra::VirouObrigatorio,
            Regra::VirouOpcional,
            Regra::TipoEstreitado,
            Regra::TipoAlargado,
            Regra::TipoTrocado,
            Regra::VarianteRemovida,
            Regra::VarianteNova,
        ] {
            let esperado = if compat.contains(&r) {
                Classe::Compativel
            } else {
                Classe::Quebra
            };
            assert_eq!(r.classe(), esperado, "{}", r.nome());
        }
    }

    // ---- propriedades ----

    /// Objetos com poucas chaves (menos que `MAPA_MIN_CHAVES`: nada vira mapa).
    fn registro() -> impl Strategy<Value = String> {
        let folha = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i32>().prop_map(Value::from),
            (-100i32..100).prop_map(|x| Value::from(f64::from(x) / 4.0)),
            "[a-c]{0,2}".prop_map(Value::String),
        ];
        let valor = folha.prop_recursive(3, 24, 4, |dentro| {
            prop_oneof![
                prop::collection::vec(dentro.clone(), 0..3).prop_map(Value::Array),
                prop::collection::vec(("[a-d]|tipo", dentro), 0..4)
                    .prop_map(|kv| Value::Object(kv.into_iter().collect())),
            ]
        });
        prop::collection::vec(("[a-e]|tipo", valor), 0..5)
            .prop_map(|kv| Value::Object(kv.into_iter().collect()).to_string())
    }

    fn jsonl(v: &[String]) -> String {
        v.iter().map(|l| format!("{l}\n")).collect()
    }

    proptest! {
        #[test]
        fn a_contra_a_nao_tem_mudancas(a in prop::collection::vec(registro(), 0..30)) {
            let r = resumo(&jsonl(&a));
            prop_assert_eq!(diff(&r, &r), vec![]);
        }

        /// Acrescentar registros só alarga: nada some, nenhum tipo estreita, nada vira obrigatório.
        #[test]
        fn acrescentar_registros_so_alarga(
            a in prop::collection::vec(registro(), 1..20),
            b in prop::collection::vec(registro(), 0..20),
        ) {
            let ra = resumo(&jsonl(&a));
            let rab = resumo(&jsonl(&[a, b].concat()));
            for m in diff(&ra, &rab) {
                let alarga = [
                    Regra::CampoNovoOpcional,
                    Regra::VirouOpcional,
                    Regra::TipoAlargado,
                    Regra::VarianteNova,
                ];
                prop_assert!(alarga.contains(&m.regra), "{}", m);
            }
        }
    }
}
