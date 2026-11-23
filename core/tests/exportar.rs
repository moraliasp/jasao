//! Export Zod / JSON Schema: dourados, autovalidação com o validador independente
//! (`jsonschema`), caso que deve reprovar e chaves hostis. Os dourados ficam em
//! `app/tests/dourados`, onde o `app/tests/zod.test.ts` roda o Zod gerado com o `zod` de verdade.
//! `ATUALIZAR_DOURADOS=1 cargo test -p nucleo --test exportar` regrava os dourados.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use nucleo::estrutura::{self, Entrada, Inferencia};
use nucleo::exportar::{self, Opcoes};
use nucleo::validar::{self, Validacao};
use proptest::prelude::*;

fn dourados() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "..", "app", "tests", "dourados"]
        .iter()
        .collect()
}

fn inferir(b: &[u8]) -> Inferencia {
    estrutura::inferir(b, &AtomicBool::new(false), |_| {})
}

fn validar(b: &[u8], entrada: Entrada, schema: &str) -> Validacao {
    validar::validar(b, entrada, schema, &AtomicBool::new(false), |_| {}).expect("schema compila")
}

fn conferir_dourado(nome: &str, gerado: &str) {
    let p = dourados().join(nome);
    if std::env::var_os("ATUALIZAR_DOURADOS").is_some() {
        std::fs::write(&p, gerado).unwrap();
    }
    let esperado = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{nome}: {e}"));
    assert_eq!(
        gerado, esperado,
        "dourado {nome} mudou (ATUALIZAR_DOURADOS=1 regrava)"
    );
}

const VARIANTES: [(&str, Opcoes); 2] = [
    (
        "",
        Opcoes {
            literal: false,
            numero: false,
        },
    ),
    (
        ".literal-numero",
        Opcoes {
            literal: true,
            numero: true,
        },
    ),
];

#[test]
fn dourados_de_cada_formato() {
    for base in ["exportar", "hostil"] {
        let dados = std::fs::read(dourados().join(format!("{base}.jsonl"))).unwrap();
        let inf = inferir(&dados);
        for (sufixo, op) in VARIANTES {
            conferir_dourado(
                &format!("{base}{sufixo}.zod.ts"),
                &exportar::zod(&inf.resumo, op),
            );
            conferir_dourado(
                &format!("{base}{sufixo}.schema.json"),
                &exportar::schema(&inf.resumo, op),
            );
        }
    }
}

#[test]
fn todo_registro_passa_nas_opcoes_que_nao_convertem() {
    for base in ["exportar", "hostil"] {
        let dados = std::fs::read(dourados().join(format!("{base}.jsonl"))).unwrap();
        let inf = inferir(&dados);
        let n = inf.resumo.registros();
        for op in [
            Opcoes::default(),
            Opcoes {
                literal: true,
                numero: false,
            },
        ] {
            let v = validar(&dados, inf.entrada, &exportar::schema(&inf.resumo, op));
            assert_eq!(
                (v.passam, v.falham, v.invalidas),
                (n, 0, 0),
                "{base} {op:?}: {:?}",
                v.falhas
            );
        }
        // Zod com "tratar como número" usa z.coerce: a entrada que ele aceita é a do texto.
        let op = Opcoes {
            literal: true,
            numero: true,
        };
        let v = validar(
            &dados,
            inf.entrada,
            &exportar::schema_da_entrada_zod(&inf.resumo, op),
        );
        assert_eq!((v.passam, v.falham), (n, 0));
    }
}

#[test]
fn numero_como_texto_tratado_como_numero_reprova_no_schema() {
    let dados = std::fs::read(dourados().join("exportar.jsonl")).unwrap();
    let inf = inferir(&dados);
    let op = Opcoes {
        literal: false,
        numero: true,
    };
    let v = validar(&dados, inf.entrada, &exportar::schema(&inf.resumo, op));
    // 4 pagamentos têm `valor` em texto e 4 pedidos têm `itens[].preco` em texto: os 8 caem.
    assert_eq!((v.passam, v.falham, v.invalidas), (4, 8, 0));
    assert_eq!(v.falhas.len(), 8);
    assert!(v.falhas.windows(2).all(|w| w[0].linha < w[1].linha));
    let pagamento = v
        .falhas
        .iter()
        .find(|f| f.linha == 1)
        .expect("linha 2 é pagamento");
    assert_eq!(pagamento.motivo, "/valor: value is not of type \"number\"");
    let pedido = v
        .falhas
        .iter()
        .find(|f| f.linha == 2)
        .expect("linha 3 é pedido");
    assert_eq!(
        pedido.motivo,
        "/itens/0/preco: value is not of type \"number\""
    );
}

#[test]
fn motivo_vem_do_ramo_do_discriminador() {
    // O pedido tem mais erros (4 preços em texto) que o ramo `evento` (tipo e `origem`): o
    // motivo tem de falar do pedido, não do ramo com menos erros.
    let dados = br#"{"tipo":"evento","origem":"a"}
{"tipo":"pedido","itens":[{"preco":"1.5"},{"preco":"2.5"},{"preco":"3.5"},{"preco":"4.5"}]}
"#;
    let inf = inferir(dados);
    let op = Opcoes {
        literal: false,
        numero: true,
    };
    let v = validar(dados, inf.entrada, &exportar::schema(&inf.resumo, op));
    assert_eq!((v.passam, v.falham), (1, 1));
    assert!(
        v.falhas[0].motivo.starts_with("/itens/"),
        "{}",
        v.falhas[0].motivo
    );
}

#[test]
fn chaves_hostis_ficam_escapadas_e_voltam_iguais() {
    let dados = std::fs::read(dourados().join("hostil.jsonl")).unwrap();
    let inf = inferir(&dados);
    let primeiro: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(dados.split(|&c| c == b'\n').next().unwrap()).unwrap();
    for op in [
        Opcoes::default(),
        Opcoes {
            literal: true,
            numero: false,
        },
    ] {
        let zod = exportar::zod(&inf.resumo, op);
        let schema = exportar::schema(&inf.resumo, op);
        for codigo in [&zod, &schema] {
            for proibido in [
                "</script", "<script", "\u{7}", "\u{85}", "\u{202e}", "\u{2028}", "\u{200b}",
            ] {
                assert!(
                    !codigo.contains(proibido),
                    "{proibido:?} cru no código:\n{codigo}"
                );
            }
            // Fora de texto entre aspas, nada do arquivo: `${x}` e a crase só aparecem dentro de "…".
            assert!(!codigo.contains("`crase`,") && !codigo.contains(" ${x}"));
        }
        assert!(
            zod.contains("[\"__proto__\"]: "),
            "__proto__ precisa de chave computada"
        );
        // As chaves voltam iguais ao decodificar o Schema.
        let s: serde_json::Value = serde_json::from_str(&schema).unwrap();
        let props = s["properties"].as_object().unwrap();
        let mut a: Vec<&String> = props.keys().collect();
        let mut b: Vec<&String> = primeiro.keys().collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }
}

/// Bidi, invisíveis, separadores e parecidos: cada um como chave e como valor travado sai
/// escapado no Zod e no Schema, e o código inteiro é ASCII.
#[test]
fn todo_caractere_perigoso_sai_escapado() {
    let faixas = [
        '\u{200b}'..='\u{200f}',
        '\u{202a}'..='\u{202e}',
        '\u{2060}'..='\u{2069}',
        '\u{061c}'..='\u{061c}',
        '\u{2028}'..='\u{2029}',
        '\u{feff}'..='\u{feff}',
        '\u{80}'..='\u{9f}',
    ];
    let soltos = [
        '&',
        '<',
        '>',
        'é',
        '\u{ad}',
        '\u{180e}',
        '\u{34f}',
        '\u{e0041}',
        '\u{2800}',
    ];
    let todos: Vec<char> = faixas.into_iter().flatten().chain(soltos).collect();
    assert!(todos.len() > 60);
    for c in todos {
        let k = format!("k{c}k");
        let q = exportar::aspas(&k);
        assert!(q.is_ascii() && !q.contains(c), "{c:?}: {q}");
        assert_eq!(serde_json::from_str::<String>(&q).unwrap(), k);
        let linha = serde_json::to_string(&serde_json::json!({ &k: &k })).unwrap();
        let dados = format!("{linha}\n{linha}\n");
        let inf = inferir(dados.as_bytes());
        let op = Opcoes {
            literal: true,
            numero: false,
        };
        for codigo in [
            exportar::zod(&inf.resumo, op),
            exportar::schema(&inf.resumo, op),
        ] {
            assert!(codigo.is_ascii(), "{c:?} cru:\n{codigo}");
        }
        let v = validar(
            dados.as_bytes(),
            inf.entrada,
            &exportar::schema(&inf.resumo, op),
        );
        assert_eq!((v.passam, v.falham), (2, 0), "{c:?}");
    }
}

#[test]
fn aspas_escapa_o_perigoso_e_continua_json() {
    let s = "a\"b\\c</script>\u{0}\u{1b}\u{7f}\u{9b}\u{202e}\u{2066}\u{2028}\u{feff}ok";
    let q = exportar::aspas(s);
    assert!(q.is_ascii(), "{q}");
    assert!(!q.contains('<') && !q.contains('>'));
    assert_eq!(serde_json::from_str::<String>(&q).unwrap(), s);
}

#[test]
fn json_unico_tambem_valida() {
    let dados = br#"[{"tipo":"a","x":1},
{"tipo":"b","y":"2"},
{"tipo":"a","x":3}]"#;
    let inf = inferir(dados);
    assert_eq!(inf.entrada, Entrada::JsonUnico);
    let v = validar(
        dados,
        inf.entrada,
        &exportar::schema(&inf.resumo, Opcoes::default()),
    );
    assert_eq!((v.passam, v.falham), (3, 0));
    let op = Opcoes {
        literal: false,
        numero: true,
    };
    let v = validar(dados, inf.entrada, &exportar::schema(&inf.resumo, op));
    assert_eq!((v.passam, v.falham), (2, 1));
    assert_eq!(v.falhas[0].linha, 1);
}

#[test]
fn cancelar_para_a_validacao() {
    let dados = std::fs::read(dourados().join("exportar.jsonl")).unwrap();
    let inf = inferir(&dados);
    let v = validar::validar(
        &dados,
        inf.entrada,
        &exportar::schema(&inf.resumo, Opcoes::default()),
        &AtomicBool::new(true),
        |_| {},
    )
    .unwrap();
    assert!(v.cancelado);
    assert_eq!(v.passam, 0);
}

// ---------------------------------------------------------------------------------------------
// Propriedade: para JSONL aleatório, todo registro passa no Schema gerado (opções padrão e
// literal), conferido pelo `jsonschema`.

fn valor() -> impl Strategy<Value = serde_json::Value> {
    use serde_json::Value as V;
    let folha = prop_oneof![
        Just(V::Null),
        any::<bool>().prop_map(V::Bool),
        any::<i64>().prop_map(V::from),
        (-1e12f64..1e12).prop_map(V::from),
        "[a-c0-9.\\-]{0,4}".prop_map(V::String),
        "\\PC{0,6}".prop_map(V::String),
    ];
    folha.prop_recursive(4, 24, 5, |dentro| {
        prop_oneof![
            prop::collection::vec(dentro.clone(), 0..4).prop_map(V::Array),
            prop::collection::btree_map("[a-d\"\\\\ ]{0,3}", dentro, 0..5)
                .prop_map(|m| V::Object(m.into_iter().collect())),
        ]
    })
}

fn registro() -> impl Strategy<Value = serde_json::Value> {
    // Metade com um `tipo` de poucos valores, para exercitar a união discriminada.
    // Lista no topo de uma linha só seria um JSON único (outra entrada); embrulha num objeto.
    let topo = valor().prop_map(|v| match v {
        serde_json::Value::Array(_) => serde_json::json!({ "lista": v }),
        v => v,
    });
    prop_oneof![
        topo,
        (
            prop::sample::select(vec!["x", "y", "z"]),
            "[a-c]{1,2}",
            valor()
        )
            .prop_map(|(t, k, v)| {
                let mut m = serde_json::Map::new();
                m.insert("tipo".into(), t.into());
                m.insert(format!("{t}{k}"), v);
                serde_json::Value::Object(m)
            }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn todo_registro_passa_no_schema_gerado(regs in prop::collection::vec(registro(), 1..40)) {
        let mut dados = Vec::new();
        for r in &regs {
            dados.extend(serde_json::to_vec(r).unwrap());
            dados.push(b'\n');
        }
        let inf = inferir(&dados);
        prop_assert_eq!(inf.resumo.registros(), regs.len() as u64);
        for op in [Opcoes::default(), Opcoes { literal: true, numero: false }] {
            let schema = exportar::schema(&inf.resumo, op);
            let v = validar(&dados, inf.entrada, &schema);
            prop_assert_eq!(
                (v.passam, v.falham, v.invalidas),
                (regs.len() as u64, 0, 0),
                "{:?}\n{}", v.falhas, schema
            );
        }
    }
}
