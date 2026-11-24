//! MessagePack, CBOR, BSON, YAML e TOML: abrem pela mesma função do JSONL (`abrir_com`), viram
//! JSONL no cache e respeitam cada teto. Cada teto tem um teste que falha se ele for desligado.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use nucleo::formatos::Limites;
use nucleo::{ArquivoLinhas, Opcoes};
use tempfile::TempDir;

struct Caso {
    pasta: TempDir,
}

impl Caso {
    fn novo() -> Self {
        Self {
            pasta: tempfile::tempdir().unwrap(),
        }
    }
    fn op(&self) -> Opcoes<'static> {
        Opcoes {
            dir_cache: Some(self.pasta.path().join("cache")),
            ..Opcoes::default()
        }
    }
    fn escrever(&self, nome: &str, dados: &[u8]) -> PathBuf {
        let p = self.pasta.path().join(nome);
        fs::write(&p, dados).unwrap();
        p
    }
    /// Linhas do JSONL convertido.
    fn linhas(&self, nome: &str, dados: &[u8]) -> Vec<String> {
        self.linhas_com(nome, dados, &self.op())
    }
    fn linhas_com(&self, nome: &str, dados: &[u8], op: &Opcoes) -> Vec<String> {
        let a = ArquivoLinhas::abrir_com(self.escrever(nome, dados), op).unwrap();
        a.janela(0, a.linhas())
            .iter()
            .map(|l| String::from_utf8(l.to_vec()).unwrap())
            .collect()
    }
    /// Mensagem do erro ao abrir; falha se abrir.
    fn erro(&self, nome: &str, dados: &[u8], op: &Opcoes) -> String {
        match ArquivoLinhas::abrir_com(self.escrever(nome, dados), op) {
            Ok(a) => panic!("abriu {} linhas; esperava erro", a.linhas()),
            Err(e) => {
                assert_eq!(e.kind(), ErrorKind::InvalidData, "{e}");
                e.to_string()
            }
        }
    }
    fn com(&self, f: impl FnOnce(&mut Limites)) -> Opcoes<'static> {
        let mut op = self.op();
        f(&mut op.formatos);
        op
    }
}

/// Todas as linhas são JSON válido.
fn validas(linhas: &[String]) {
    for l in linhas {
        serde_json::from_str::<serde_json::Value>(l).unwrap_or_else(|e| panic!("{l}: {e}"));
    }
}

fn bson_doc(d: &bson::Document) -> Vec<u8> {
    bson::RawDocumentBuf::try_from(d).unwrap().into_bytes()
}

// ---------------------------------------------------------------------------------------------
// Detecção

#[test]
fn detecta_pela_assinatura_sem_extensao() {
    let c = Caso::novo();
    // CBOR com a marca de autodescrição: 55799(1)
    assert_eq!(c.linhas("a", &[0xD9, 0xD9, 0xF7, 0x01]), ["1"]);
    assert_eq!(c.linhas("b", b"---\nx: 1\n"), [r#"{"x":1}"#]);
    let bson = bson_doc(&bson::doc! {"x": 1});
    assert_eq!(c.linhas("c", &bson), [r#"{"x":1}"#]);
    // Sem assinatura: o caminho de sempre (JSONL cru).
    assert_eq!(c.linhas("d", b"{\"x\":1}\n"), [r#"{"x":1}"#]);
    // `.json` nunca passa pela conversão, mesmo começando como YAML.
    assert_eq!(c.linhas("e.json", b"---\n"), ["---"]);
}

#[test]
fn reaproveita_o_cache_e_nao_mistura_com_compactado() {
    let c = Caso::novo();
    let p = c.escrever("x.yaml", b"a: 1\n");
    let op = c.op();
    let a = ArquivoLinhas::abrir_com(&p, &op).unwrap();
    let b = ArquivoLinhas::abrir_com(&p, &op).unwrap();
    assert_eq!(a.bytes(), b.bytes());
    assert_eq!(
        fs::read_dir(c.pasta.path().join("cache")).unwrap().count(),
        1
    );
}

// ---------------------------------------------------------------------------------------------
// Representação

#[test]
fn msgpack_representacao() {
    let c = Caso::novo();
    let mut d = vec![0x87]; // mapa de 7
    d.extend([0x01, 0xA1, b'a']); // chave inteira 1 → "1"
    d.extend([0xA1, b'b', 0xC4, 3, 1, 2, 3]); // bin → base64
    d.extend([0xA1, b'c', 0xCF]); // u64::MAX com todos os dígitos
    d.extend(u64::MAX.to_be_bytes());
    d.extend([0xA1, b'd', 0xCB]); // NaN
    d.extend(f64::NAN.to_be_bytes());
    d.extend([0xA1, b'e', 0xD6, 0xFF, 0, 0, 0, 0]); // timestamp 32 → 1970
    d.extend([0xA1, b'f', 0xD4, 5, 9]); // ext 5
    d.extend([0x92, 0x01, 0xC0, 0xC3]); // chave [1,nil] → "[1,null]", valor true
    d.extend([0x93, 0xCA]); // 2º valor do arquivo: [f32 0.1, -1, "é"]
    d.extend(0.1f32.to_be_bytes());
    d.extend([0xFF, 0xA2, 0xC3, 0xA9]);
    let l = c.linhas("x.msgpack", &d);
    validas(&l);
    assert_eq!(
        l,
        [
            r#"{"1":"a","b":"AQID","c":18446744073709551615,"d":"NaN","e":"1970-01-01T00:00:00Z","f":{"$ext":5,"$dados":"CQ=="},"[1,null]":true}"#,
            r#"[0.1,-1,"é"]"#
        ]
    );
}

#[test]
fn cbor_representacao() {
    let c = Caso::novo();
    let mut d = vec![0xA5]; // mapa de 5
    d.extend([0x61, b'a', 0xC2, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0]); // bignum 2^64
    d.extend([0x61, b'b', 0xC1, 0x1A, 0, 0, 0, 0]); // tag 1 (época) transparente
    d.extend([0x61, b'c', 0x9F, 0x01, 0xF9, 0x3C, 0x00, 0xFF]); // [1, f16 1.0] indefinido
    d.extend([0x61, b'd', 0x5F, 0x41, 1, 0x41, 2, 0xFF]); // bytes em pedaços
    d.extend([0xF5, 0xF7]); // chave true → "true", valor undefined → null
    d.extend([0xC3, 0x41, 0x00]); // 2º item: bignum negativo −1 − 0
    let l = c.linhas("x.cbor", &d);
    validas(&l);
    assert_eq!(
        l,
        [
            r#"{"a":18446744073709551616,"b":0,"c":[1,1.0],"d":"AQI=","true":null}"#,
            "-1"
        ]
    );
}

#[test]
fn bson_representacao_extended_json_relaxado() {
    let c = Caso::novo();
    let oid = bson::oid::ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").unwrap();
    let mut d = bson_doc(&bson::doc! {
        "_id": oid,
        "quando": bson::DateTime::from_millis(0),
        "n": 1i64 << 60,
        "lista": [1, {"x": "y"}],
    });
    d.extend(bson_doc(&bson::doc! {"dois": true}));
    let l = c.linhas("x.bson", &d);
    validas(&l);
    assert_eq!(
        l,
        [
            r#"{"_id":{"$oid":"65a1b2c3d4e5f60718293a4b"},"quando":{"$date":"1970-01-01T00:00:00Z"},"n":1152921504606846976,"lista":[1,{"x":"y"}]}"#,
            r#"{"dois":true}"#
        ]
    );
}

#[test]
fn yaml_representacao() {
    let c = Caso::novo();
    let y = "\
base: &b {x: 1, l: [a, b]}
copia: *b
sim: yes
hex: 0x10
zeros: 007
real: 1.5e3
inf: -.inf
nada: ~
data: 2024-01-02
texto: '1'
? [1, 2]
: chave lista
---
- segundo
";
    let l = c.linhas("x.yaml", y.as_bytes());
    validas(&l);
    assert_eq!(
        l,
        [
            r#"{"base":{"x":1,"l":["a","b"]},"copia":{"x":1,"l":["a","b"]},"sim":"yes","hex":16,"zeros":7,"real":1500.0,"inf":"-Infinity","nada":null,"data":"2024-01-02","texto":"1","[1,2]":"chave lista"}"#,
            r#"["segundo"]"#
        ]
    );
}

#[test]
fn toml_representacao() {
    let c = Caso::novo();
    let t = "a = 1\nquando = 1979-05-27T07:32:00Z\nn = nan\n[t]\nl = [1.5, \"x\"]\n";
    let l = c.linhas("x.toml", t.as_bytes());
    validas(&l);
    assert_eq!(
        l,
        [r#"{"a":1,"n":"NaN","quando":"1979-05-27T07:32:00Z","t":{"l":[1.5,"x"]}}"#]
    );
}

// ---------------------------------------------------------------------------------------------
// Malformado: erro com mensagem, nunca pânico

#[test]
fn malformado_vira_erro() {
    let c = Caso::novo();
    let op = c.op();
    for (nome, dados) in [
        ("a.msgpack", &[0x92, 0x01][..]),
        ("b.msgpack", &[0xC1]),
        ("c.cbor", &[0xA1, 0x01]),
        ("d.cbor", &[0xFF]),
        ("e.cbor", &[0x1C]),
        ("f.bson", &[9, 0, 0, 0, 0x10, b'a', 0, 1, 0]),
        ("g.yaml", b"a: [1\n"),
        ("h.toml", b"a = \n"),
        ("i.yaml", b"\xFF\xFE"),
    ] {
        let m = c.erro(nome, dados, &op);
        assert!(m.contains("malformado"), "{nome}: {m}");
    }
}

// ---------------------------------------------------------------------------------------------
// Tetos: um teste por teto, pela função que o app chama.

#[test]
fn teto_de_profundidade() {
    let c = Caso::novo();
    let op = c.com(|l| l.profundidade = 3);
    let casos: [(&str, &[u8]); 5] = [
        ("p.msgpack", &[0x91, 0x91, 0x91, 0x91, 0x01]),
        ("p.cbor", &[0x81, 0x81, 0x81, 0x81, 0x01]),
        ("p.yaml", b"[[[[1]]]]"),
        ("p.toml", b"a = [[[[1]]]]"),
        ("p.json", b""), // marcador: BSON vai à parte
    ];
    for (nome, dados) in &casos[..4] {
        let m = c.erro(nome, dados, &op);
        assert!(m.contains("aninhamento passa de 3"), "{nome}: {m}");
    }
    let b = bson_doc(&bson::doc! {"a": {"b": {"c": {"d": 1}}}});
    let m = c.erro("p.bson", &b, &op);
    assert!(m.contains("aninhamento passa de 3"), "{m}");
    // Folha BSON com Extended JSON aninhado conta a profundidade dela.
    let b = bson_doc(&bson::doc! {"a": {"b": bson::Binary {
    subtype: bson::spec::BinarySubtype::Generic, bytes: vec![1] }}});
    let m = c.erro("q.bson", &b, &op);
    assert!(m.contains("aninhamento passa de 3"), "{m}");
}

#[test]
fn profundidade_padrao_cabe_na_arvore_e_nao_estoura_a_pilha() {
    let c = Caso::novo();
    let op = c.op();
    let fundo = 100_000;
    let mut mp = vec![0x91; fundo];
    mp.push(0x01);
    let m = c.erro("f.msgpack", &mp, &op);
    assert!(m.contains("aninhamento passa de 1000"), "{m}");
    let y = format!("{}1{}", "[".repeat(fundo), "]".repeat(fundo));
    // O parser YAML pode recusar antes (teto próprio); o importante é erro, não estouro.
    c.erro("f.yaml", y.as_bytes(), &op);
    let t = format!("a = {}1{}", "[".repeat(fundo), "]".repeat(fundo));
    c.erro("f.toml", t.as_bytes(), &op);
}

#[test]
fn teto_de_nos() {
    let c = Caso::novo();
    let op = c.com(|l| l.nos = 5);
    for (nome, dados) in [
        ("n.msgpack", &[0x96, 1, 2, 3, 4, 5, 6][..]),
        ("n.cbor", &[0x86, 1, 2, 3, 4, 5, 6]),
        ("n.yaml", b"[1, 2, 3, 4, 5, 6]"),
        ("n.toml", b"a = [1, 2, 3, 4, 5, 6]"),
    ] {
        let m = c.erro(nome, dados, &op);
        assert!(m.contains("mais de 5 valores"), "{nome}: {m}");
    }
    let b = bson_doc(&bson::doc! {"a": [1, 2, 3, 4, 5, 6]});
    assert!(c.erro("n.bson", &b, &op).contains("mais de 5 valores"));
}

#[test]
fn teto_de_saida() {
    let c = Caso::novo();
    let op = Opcoes {
        max_saida: 64,
        ..c.op()
    };
    let mut d = vec![0xDA, 0, 100];
    d.extend([b'x'; 100]);
    let m = c.erro("s.msgpack", &d, &op);
    assert!(m.contains("convertido para JSON passa de"), "{m}");
    // Nada parcial fica no cache.
    let cache = c.pasta.path().join("cache");
    assert_eq!(fs::read_dir(cache).map(|i| i.count()).unwrap_or(0), 0);
}

#[test]
fn bomba_de_aliases_yaml() {
    let c = Caso::novo();
    let mut y = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for i in 1..10 {
        let p = i - 1;
        y +=
            &format!("a{i}: &a{i} [*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p}]\n");
    }
    // Padrão: 10^9 valores seriam expandidos; o teto de aliases para antes.
    let m = c.erro("bomba.yaml", y.as_bytes(), &c.op());
    assert!(m.contains("bomba de aliases"), "{m}");
    // Com teto de nós e de saída folgados, só o de aliases segura.
    let op = c.com(|l| {
        l.nos_alias = 100;
        l.nos = u64::MAX;
    });
    let m = c.erro("bomba.yaml", y.as_bytes(), &op);
    assert!(m.contains("mais de 100 valores"), "{m}");
    // Alias pequeno dentro do teto expande normalmente.
    assert_eq!(
        c.linhas_com("ok.yaml", b"a: &x [1]\nb: *x\n", &op),
        [r#"{"a":[1],"b":[1]}"#]
    );
}

#[test]
fn teto_de_ancora_yaml() {
    let c = Caso::novo();
    let op = c.com(|l| l.max_ancora = 8);
    let m = c.erro("a.yaml", b"a: &x [1, 2, 3, 4, 5]\nb: *x\n", &op);
    assert!(m.contains("âncora com mais de 8 bytes"), "{m}");
    // Âncora grande sem alias abre normal.
    assert_eq!(
        c.linhas_com("b.yaml", b"a: &x [1, 2, 3, 4, 5]\n", &op),
        [r#"{"a":[1,2,3,4,5]}"#]
    );
}

#[test]
fn tags_customizadas_yaml_sao_recusadas() {
    let c = Caso::novo();
    let op = c.op();
    for y in [
        "a: !!python/object:os.system x\n",
        "a: !Ref Bucket\n",
        "a: !custom [1]\n",
        "a: !<tag:exemplo.com,2024:x> {}\n",
    ] {
        let m = c.erro("t.yaml", y.as_bytes(), &op);
        assert!(m.contains("recusada"), "{y}: {m}");
    }
    // Tags do esquema core passam.
    assert_eq!(
        c.linhas("ok.yaml", b"a: !!str 1\nb: !!int \"2\"\nc: !!map {}\n"),
        [r#"{"a":"1","b":2,"c":{}}"#]
    );
}

#[test]
fn comprimento_declarado_mentiroso_nao_aloca() {
    let c = Caso::novo();
    let op = c.op();
    for (nome, dados) in [
        // array32 com 4 bilhões de itens em 5 bytes
        ("a.msgpack", &[0xDD, 0xFF, 0xFF, 0xFF, 0xFF][..]),
        // bin32 de 4 GiB
        ("b.msgpack", &[0xC6, 0xFF, 0xFF, 0xFF, 0xFF, 0]),
        // mapa32 de 2^32 − 1 pares
        ("c.msgpack", &[0xDF, 0xFF, 0xFF, 0xFF, 0xFF]),
        // CBOR: byte string de 2^64 − 1 bytes, e lista do mesmo tamanho
        (
            "d.cbor",
            &[0x5B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        ),
        (
            "e.cbor",
            &[0x9B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        ),
        (
            "f.cbor",
            &[0xBB, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        ),
        // BSON: documento declara 2 GiB
        ("g.bson", &[0xFF, 0xFF, 0xFF, 0x7F, 0x0A, b'a', 0, 0]),
    ] {
        let m = c.erro(nome, dados, &op);
        assert!(m.contains("declara"), "{nome}: {m}");
    }
    // BSON com tamanho negativo ou menor que o mínimo.
    let m = c.erro("h.bson", &[0xFF, 0xFF, 0xFF, 0xFF, 0], &op);
    assert!(m.contains("declara"), "{m}");
}

#[test]
fn teto_de_chave() {
    let c = Caso::novo();
    let op = c.com(|l| l.max_chave = 8);
    // Chave que é uma lista de 10 inteiros: "[1,1,1,…]" passa de 8 bytes.
    let m = c.erro(
        "k.msgpack",
        &[0x81, 0x9A, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0xC0],
        &op,
    );
    assert!(m.contains("chave de mapa com mais de 8 bytes"), "{m}");
    let m = c.erro(
        "k.cbor",
        &[0xA1, 0x8A, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0xF6],
        &op,
    );
    assert!(m.contains("chave de mapa com mais de 8 bytes"), "{m}");
}

#[test]
fn teto_de_texto_toml_e_cbor_em_pedacos() {
    let c = Caso::novo();
    let op = c.com(|l| l.max_texto = 16);
    let m = c.erro("x.toml", b"a = \"0123456789abcdef\"\n", &op);
    assert!(m.contains("TOML é carregado inteiro"), "{m}");
    let mut d = vec![0x7F];
    for _ in 0..3 {
        d.extend([0x68]);
        d.extend(b"12345678");
    }
    d.push(0xFF);
    let m = c.erro("x.cbor", &d, &op);
    assert!(m.contains("em pedaços com mais de"), "{m}");
}

#[test]
fn cancelamento_interrompe_a_conversao() {
    use std::sync::atomic::AtomicBool;
    static CANCELAR: AtomicBool = AtomicBool::new(true);
    let c = Caso::novo();
    let op = Opcoes {
        cancelar: Some(&CANCELAR),
        ..c.op()
    };
    let Err(e) = ArquivoLinhas::abrir_com(c.escrever("c.yaml", b"[1, 2]"), &op) else {
        panic!("abriu apesar do cancelamento");
    };
    assert!(nucleo::compactado::cancelada(&e), "{e}");
}
