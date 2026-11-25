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
                &format!("{base}{sufixo}.d.ts"),
                &exportar::tipos(&inf.resumo, op),
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
fn conta_certo_depois_das_20_falhas_guardadas() {
    // 60 registros alternando texto e número: 30 reprovam com "tratar como número". Depois da
    // 20ª falha o validador só conta (`is_valid`); passam e falham têm de continuar certos.
    let dados: String = (0..60)
        .map(|i| {
            if i % 2 == 0 {
                format!("{{\"v\":\"{i}.5\"}}\n")
            } else {
                format!("{{\"v\":{i}.5}}\n")
            }
        })
        .collect();
    let inf = inferir(dados.as_bytes());
    let op = Opcoes {
        literal: false,
        numero: true,
    };
    let v = validar(
        dados.as_bytes(),
        inf.entrada,
        &exportar::schema(&inf.resumo, op),
    );
    assert_eq!((v.passam, v.falham, v.invalidas), (30, 30, 0));
    assert_eq!(v.falhas.len(), 20);
    let linhas: Vec<u64> = v.falhas.iter().map(|f| f.linha).collect();
    assert_eq!(linhas, (0..20).map(|i| i * 2).collect::<Vec<u64>>());
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
        '\u{7f}',
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

// ---- Validar contra schema de fora (entrada não confiável). ----

fn recusa(schema: &str) -> String {
    match validar::compilar(schema) {
        Err(e) => e,
        Ok(_) => panic!("devia recusar: {schema}"),
    }
}

#[test]
fn schema_de_fora_nao_busca_ref_remoto() {
    for r in [
        "https://example.com/x.json",
        "http://127.0.0.1:1/x",
        "file:///etc/passwd",
        "outro.json#/a",
        "//example.com/x",
    ] {
        for chave in ["$ref", "$dynamicRef"] {
            let s = format!(r#"{{"properties": {{"a": {{"{chave}": "{r}"}}}}}}"#);
            assert!(recusa(&s).contains("proibido"), "{s}");
        }
    }
    // `$ref` no topo e dentro de lista também.
    assert!(recusa(r#"{"anyOf": [{"$ref": "https://x/y"}]}"#).contains("proibido"));
    // Campo de dado chamado `$ref` e `$ref` dentro de `const` não são referência.
    let ok = r#"{"properties": {"$ref": {"const": {"$ref": "https://x"}}}, "required": ["$ref"]}"#;
    let v = validar::compilar(ok).expect("compila");
    assert!(v.is_valid(&serde_json::json!({"$ref": {"$ref": "https://x"}})));
    assert!(!v.is_valid(&serde_json::json!({})));
}

#[test]
fn schema_de_fora_recursivo_nao_derruba() {
    // Recursão que não anda no dado (ciclo de `$ref` puro): recusada na compilação.
    for s in [
        r##"{"$ref": "#"}"##,
        r##"{"$defs": {"a": {"$ref": "#/$defs/b"}, "b": {"$ref": "#/$defs/a"}}, "$ref": "#/$defs/a"}"##,
        r##"{"allOf": [{"$recursiveRef": "#"}]}"##,
    ] {
        assert!(recusa(s).contains("recursão"), "{s}");
    }
    // Recursão legítima (árvore): anda um nível no dado a cada volta; a 2ª linha reprova.
    let dados = b"{\"a\": {\"a\": {\"a\": 1}}}\n{\"a\": \"x\"}\n";
    let arvore = r##"{"type": "object", "properties": {"a": {"anyOf": [{"type": "integer"}, {"$ref": "#"}]}}}"##;
    let v = validar(dados, Entrada::Jsonl, arvore);
    assert_eq!((v.passam, v.falham), (1, 1));
    assert_eq!(v.falhas[0].linha, 1);
    assert!(v.falhas[0].motivo.starts_with("/a"), "{:?}", v.falhas);
    // "Qualquer valor JSON": dois `$ref` ao topo, mas um só vale para objeto e o outro para
    // lista; nunca os dois no mesmo dado.
    let json = r##"{"anyOf": [{"type": ["string", "number", "boolean", "null"]},
        {"type": "object", "additionalProperties": {"$ref": "#"}},
        {"type": "array", "items": {"$ref": "#"}}]}"##;
    let v = validar(
        b"{\"a\": [1, {\"b\": null}]}\n[[[]]]\n",
        Entrada::Jsonl,
        json,
    );
    assert_eq!((v.passam, v.falham), (2, 0));
}

/// `n` níveis de `allOf` com dois `$ref` ao nível seguinte: 2^n caminhos até o fim. Poucos
/// bytes e poucos `$ref`, custo exponencial para o validador.
fn bomba(n: usize) -> String {
    let defs: Vec<String> = (0..n)
        .map(|i| {
            let r = format!(r##"{{"$ref": "#/$defs/a{}"}}"##, i + 1);
            format!(r#""a{i}": {{"allOf": [{r}, {r}]}}"#)
        })
        .collect();
    format!(
        r##"{{"$defs": {{{}, "a{n}": {{"type": "integer"}}}}, "$ref": "#/$defs/a0"}}"##,
        defs.join(",")
    )
}

#[test]
fn schema_de_fora_com_custo_explosivo_recusado() {
    // Medido no #45 antes da correção: n=24 0,16 s; n=40 mais de 30 s (com 81 `$ref`).
    let t = std::time::Instant::now();
    for n in [15, 40, 300] {
        assert!(recusa(&bomba(n)).contains("expandido"), "n={n}");
    }
    // Funda demais: para no encadeamento antes de expandir.
    assert!(recusa(&bomba(5000)).contains("encadeado"));
    assert!(t.elapsed().as_secs() < 2, "{:?}", t.elapsed());
    // Abaixo do teto, o mesmo formato compila e valida.
    let v = validar(b"1\n\"x\"\n", Entrada::Jsonl, &bomba(8));
    assert_eq!((v.passam, v.falham), (1, 1));
    // Fan-out na recursão: cada volta dobra (2^profundidade do dado). Direto, por um `$def`
    // alcançado duas vezes, e com caminhos de tamanhos diferentes.
    for s in [
        r##"{"type": "object", "properties": {"x": {"allOf": [{"$ref": "#"}, {"$ref": "#"}]}}}"##,
        r##"{"$defs": {"b": {"$ref": "#"}}, "properties": {"x": {"allOf": [{"$ref": "#/$defs/b"}, {"$ref": "#/$defs/b"}]}}}"##,
        // Volta de 1 e de 2 níveis no mesmo caminho: Fibonacci.
        r##"{"properties": {"x": {"allOf": [{"$ref": "#"}, {"properties": {"x": {"$ref": "#"}}}]}}}"##,
        r##"{"items": {"$ref": "#"}, "additionalItems": {"$ref": "#"}}"##,
    ] {
        assert!(recusa(s).contains("recursão"), "{s}");
    }
    // Recursões em dados que não se cruzam (`x` e `y`; `x` e `y.z`, tamanhos diferentes) passam.
    for s in [
        r##"{"type": "object", "properties": {"x": {"$ref": "#"}, "y": {"$ref": "#"}}}"##,
        r##"{"type": "object", "properties": {"x": {"$ref": "#"}, "y": {"properties": {"z": {"$ref": "#"}}}}}"##,
    ] {
        let v = validar(b"{\"x\": {\"y\": {\"z\": {}}}}\n", Entrada::Jsonl, s);
        assert_eq!((v.passam, v.falham), (1, 0), "{s}");
    }
    // A mesma árvore recursiva usada em dois campos: cada uso recursa sozinho.
    let v = validar(
        b"{\"e\": {\"f\": [{\"f\": []}]}, \"d\": {\"f\": [1]}}\n",
        Entrada::Jsonl,
        r##"{"$defs": {"no": {"type": "object", "properties": {"f": {"items": {"$ref": "#/$defs/no"}}}}},
            "properties": {"e": {"$ref": "#/$defs/no"}, "d": {"$ref": "#/$defs/no"}}}"##,
    );
    assert_eq!((v.passam, v.falham), (0, 1));
}

#[test]
fn schema_de_fora_fundo_ou_grande_demais() {
    let fundo = |n: usize| "{\"items\":".repeat(n) + "{}" + &"}".repeat(n);
    // 63 `items` + o `{}` = 64 níveis: passa; um a mais, não.
    validar::compilar(&fundo(validar::MAX_PROFUNDIDADE - 1)).expect("no limite compila");
    assert!(recusa(&fundo(validar::MAX_PROFUNDIDADE)).contains("níveis"));
    // Fundo além do que o serde_json aceita: erro, não estouro de pilha.
    recusa(&fundo(100_000));
    let grande = format!(
        r#"{{"description": "{}"}}"#,
        "x".repeat(validar::MAX_SCHEMA)
    );
    assert!(recusa(&grande).contains("limite"));
    // Cadeia longa de `$ref` (não cíclica): até `MAX_ENCADEADO` compila e valida numa thread
    // com a pilha padrão (a das validações do app); acima, recusada antes de estourar a pilha.
    let cadeia = |n: usize| {
        let defs: Vec<String> = (0..n)
            .map(|i| format!(r##""a{i}": {{"$ref": "#/$defs/a{}"}}"##, i + 1))
            .collect();
        format!(
            r##"{{"$defs": {{{}, "a{n}": {{"type": "integer"}}}}, "$ref": "#/$defs/a0"}}"##,
            defs.join(",")
        )
    };
    let longa = cadeia(20_000);
    assert!(longa.len() < validar::MAX_SCHEMA);
    assert!(recusa(&longa).contains("encadeado"));
    // `cadeia(n)` encadeia o topo e n + 1 defs: n + 2 objetos.
    assert!(recusa(&cadeia(validar::MAX_ENCADEADO - 1)).contains("encadeado"));
    let s = cadeia(validar::MAX_ENCADEADO - 2);
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let v = validar(b"1\n\"x\"\n", Entrada::Jsonl, &s);
            assert_eq!((v.passam, v.falham), (1, 1));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn schema_de_fora_padrao_sem_retrocesso() {
    // Backreference/lookaround exigem o motor com retrocesso (ReDoS): recusados.
    assert!(validar::compilar(r#"{"pattern": "(a+)+\\1"}"#).is_err());
    assert!(validar::compilar(r#"{"pattern": "(?=a)a"}"#).is_err());
    // O padrão clássico de ReDoS em tempo linear.
    let s = r#"{"type": "string", "pattern": "^(a+)+$"}"#;
    let linha = format!("\"{}b\"\n", "a".repeat(5000));
    let t = std::time::Instant::now();
    let v = validar(linha.as_bytes(), Entrada::Jsonl, s);
    assert_eq!(v.falham, 1);
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
}

#[test]
fn schema_de_fora_com_meta_schema_desconhecido_nao_busca() {
    let e = recusa(r#"{"$schema": "https://example.com/meta", "type": "object"}"#);
    assert!(!e.is_empty());
    // Draft conhecido (07) funciona.
    let v = validar(
        b"{\"a\": 1}\n{}\n",
        Entrada::Jsonl,
        r#"{"$schema": "http://json-schema.org/draft-07/schema#", "required": ["a"]}"#,
    );
    assert_eq!((v.passam, v.falham), (1, 1));
}

#[test]
fn tipos_ts_uniao_e_opcional() {
    let dados = std::fs::read(dourados().join("exportar.jsonl")).unwrap();
    let t = exportar::tipos(&inferir(&dados).resumo, Opcoes::default());
    assert!(
        t.starts_with("export type Registro =\n  | {\n    tipo: \"evento\";\n"),
        "{t}"
    );
    assert!(t.contains("\n  | {\n    tipo: \"pagamento\";\n"), "{t}");
    // `obs` só aparece em parte dos pedidos.
    assert!(t.contains("    obs?: string;\n"), "{t}");
    assert!(!t.contains("moeda?"), "{t}");
    assert!(t.contains("cidade: string | null;"), "{t}");
    // Tipo não converte: "tratar como número" não muda o texto numérico.
    let numero = Opcoes {
        literal: false,
        numero: true,
    };
    assert_eq!(exportar::tipos(&inferir(&dados).resumo, numero), t);
    assert!(t.contains("valor: string;"), "{t}");
}

#[test]
fn invalida_vazia_e_recusada_contam_a_parte() {
    // Vazia não conta; `{"a":}` e 200 níveis são inválidas (a inferência para acima de 128); `1e400` é
    // registro que o `serde_json` não lê.
    let fundo = format!("{}{}", "[".repeat(200), "]".repeat(200));
    let dados = format!("{{\"a\":1}}\n\n{{\"a\":}}\n{{\"a\":1e400}}\n{{\"a\":{fundo}}}\n");
    let inf = inferir(dados.as_bytes());
    let r = &inf.resumo;
    assert_eq!((r.registros(), r.invalidas.n, r.vazias), (2, 2, 1));
    let v = validar(
        dados.as_bytes(),
        inf.entrada,
        &exportar::schema(r, Opcoes::default()),
    );
    assert_eq!((v.passam, v.falham, v.invalidas, v.recusadas), (1, 0, 2, 1));
    let motivos: Vec<_> = v
        .falhas
        .iter()
        .map(|f| (f.linha, f.motivo.as_str()))
        .collect();
    assert_eq!(
        motivos,
        [
            (2, "jasao:valor"),
            (3, "jasao:numero_fora_do_alcance"),
            (4, "jasao:aninhamento")
        ]
    );
}

#[test]
fn validador_e_inferencia_cortam_no_mesmo_nivel() {
    // Um nível a menos, o limite e um a mais: o que a Estrutura conta como registro o validador
    // lê (o `serde_json` sozinho para em 127), e o que ela conta como inválida ele também conta.
    let max = estrutura::PROFUNDIDADE_MAX;
    for (n, registros, invalidas) in [(max - 1, 2, 0), (max, 2, 0), (max + 1, 1, 1)] {
        let fundo = format!("{}{}", "[".repeat(n), "]".repeat(n));
        let dados = format!("1\n{fundo}\n");
        let inf = inferir(dados.as_bytes());
        let r = &inf.resumo;
        assert_eq!(
            (r.registros(), r.invalidas.n),
            (registros, invalidas),
            "{n} níveis"
        );
        let v = validar(dados.as_bytes(), inf.entrada, "{}");
        assert_eq!(
            (v.passam, v.invalidas, v.recusadas),
            (registros, invalidas, 0),
            "{n} níveis: {:?}",
            v.falhas
        );
    }
}
