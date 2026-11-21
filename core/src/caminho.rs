//! Caminho até um valor dentro do JSON, e seus quatro formatos de texto.
//!
//! - jq: `.a.b[0]`, `.["chave com espaço"]` (texto entre aspas no escape do JSON).
//! - JSONPath (RFC 9535): `$.a.b[0]` quando a chave cabe em `member-name-shorthand`; senão
//!   `$['chave']` com o escape do Normalized Path (§2.7): só `'`, `\` e controles, de um jeito só.
//! - JSON Pointer (RFC 6901): `/a/b/0`, com `~` → `~0` e `/` → `~1`; a raiz é o texto vazio.
//! - JavaScript: `dado.a.b[0]`; ponto só quando a chave é identificador ASCII que não é palavra
//!   reservada, senão `dado["chave"]`.
//!
//! Ler de volta: jq e JSON Pointer. O JSON Pointer não distingue índice de chave numérica
//! (`/0` serve para os dois); na leitura, token que é `array-index` vira `Indice`.

use std::fmt::{self, Write};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Segmento {
    /// Chave de objeto.
    // shortcut: `String` não guarda chave com UTF-8 inválido nem `\ud800` solto; trocar por
    // bytes crus quando a árvore precisar mostrar essas chaves fiéis ao byte.
    Chave(String),
    /// Posição numa lista, a partir de 0.
    Indice(usize),
}

/// Sequência de segmentos a partir da raiz; vazio é a própria raiz.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Caminho(pub Vec<Segmento>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErroCaminho {
    /// Byte do texto onde a leitura parou.
    pub posicao: usize,
    pub motivo: &'static str,
}

impl fmt::Display for ErroCaminho {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "caminho inválido no byte {}: {}",
            self.posicao, self.motivo
        )
    }
}

impl std::error::Error for ErroCaminho {}

const PALAVRAS_JQ: &[&str] = &[
    "__loc__", "and", "as", "catch", "def", "elif", "else", "end", "foreach", "if", "import",
    "include", "label", "module", "not", "or", "reduce", "then", "try",
];

const PALAVRAS_JS: &[&str] = &[
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

fn identificador_ascii(k: &str, extra: char, reservadas: &[&str]) -> bool {
    let mut c = k.chars();
    c.next()
        .is_some_and(|p| p.is_ascii_alphabetic() || p == '_' || p == extra)
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_' || x == extra)
        && !reservadas.contains(&k)
}

/// `member-name-shorthand` da RFC 9535: `name-first = ALPHA / "_" / %x80-D7FF / %xE000-10FFFF`.
fn atalho_jsonpath(k: &str) -> bool {
    let primeiro = |c: char| c.is_ascii_alphabetic() || c == '_' || c >= '\u{80}';
    let mut c = k.chars();
    c.next().is_some_and(primeiro) && c.all(|x| primeiro(x) || x.is_ascii_digit())
}

/// Texto entre `aspas`, escapando `\`, as próprias aspas e controles (`\b \f \n \r \t`, o resto
/// `\u00xx` minúsculo). Com `'` é o escape do Normalized Path; com `"` é JSON, que jq e JS aceitam.
fn citar(out: &mut String, s: &str, aspas: char) {
    out.push(aspas);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == aspas => {
                out.push('\\');
                out.push(c);
            }
            // U+2028/2029 quebravam string de JS antes do ES2019; escapar não muda o valor.
            c if c < ' ' || (aspas == '"' && matches!(c, '\u{2028}' | '\u{2029}')) => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(aspas);
}

/// Texto JSON (com aspas) de `s`.
pub fn texto_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    citar(&mut out, s, '"');
    out
}

impl Caminho {
    pub fn raiz() -> Self {
        Self::default()
    }

    pub fn jq(&self) -> String {
        let mut out = String::from(".");
        for s in &self.0 {
            match s {
                Segmento::Chave(k) if identificador_ascii(k, '_', PALAVRAS_JQ) => {
                    if out.len() > 1 {
                        out.push('.');
                    }
                    out.push_str(k);
                }
                Segmento::Chave(k) => {
                    out.push('[');
                    citar(&mut out, k, '"');
                    out.push(']');
                }
                Segmento::Indice(i) => {
                    let _ = write!(out, "[{i}]");
                }
            }
        }
        out
    }

    pub fn jsonpath(&self) -> String {
        let mut out = String::from("$");
        for s in &self.0 {
            match s {
                Segmento::Chave(k) if atalho_jsonpath(k) => {
                    out.push('.');
                    out.push_str(k);
                }
                Segmento::Chave(k) => {
                    out.push('[');
                    citar(&mut out, k, '\'');
                    out.push(']');
                }
                Segmento::Indice(i) => {
                    let _ = write!(out, "[{i}]");
                }
            }
        }
        out
    }

    pub fn json_pointer(&self) -> String {
        let mut out = String::new();
        for s in &self.0 {
            out.push('/');
            match s {
                Segmento::Chave(k) => out.push_str(&k.replace('~', "~0").replace('/', "~1")),
                Segmento::Indice(i) => {
                    let _ = write!(out, "{i}");
                }
            }
        }
        out
    }

    /// Acesso em JavaScript a partir da variável `dado`.
    pub fn javascript(&self) -> String {
        let mut out = String::from("dado");
        for s in &self.0 {
            match s {
                // shortcut: identificador só ASCII (sem tabela Unicode ID_Start); chave com
                // acento sai entre colchetes, que vale igual. Trocar se pedirem `dado.ação`.
                Segmento::Chave(k) if identificador_ascii(k, '$', PALAVRAS_JS) => {
                    out.push('.');
                    out.push_str(k);
                }
                Segmento::Chave(k) => {
                    out.push('[');
                    citar(&mut out, k, '"');
                    out.push(']');
                }
                Segmento::Indice(i) => {
                    let _ = write!(out, "[{i}]");
                }
            }
        }
        out
    }

    /// Lê JSON Pointer (RFC 6901). Token no formato `array-index` (`0` ou sem zero à esquerda)
    /// vira `Indice`; o resto, inclusive `-`, vira `Chave`.
    pub fn ler_json_pointer(texto: &str) -> Result<Self, ErroCaminho> {
        if texto.is_empty() {
            return Ok(Self::raiz());
        }
        if !texto.starts_with('/') {
            return Err(ErroCaminho {
                posicao: 0,
                motivo: "JSON Pointer começa com `/` (ou é vazio, para a raiz)",
            });
        }
        let mut segs = Vec::new();
        let mut pos = 1;
        for token in texto[1..].split('/') {
            let mut k = String::with_capacity(token.len());
            let mut c = token.char_indices();
            while let Some((i, ch)) = c.next() {
                if ch != '~' {
                    k.push(ch);
                    continue;
                }
                // `~1` → `/` e `~0` → `~` numa passada só: `~01` vira `~1`, nunca `/`.
                match c.next() {
                    Some((_, '0')) => k.push('~'),
                    Some((_, '1')) => k.push('/'),
                    _ => {
                        return Err(ErroCaminho {
                            posicao: pos + i,
                            motivo: "`~` só vale como `~0` (til) ou `~1` (barra)",
                        });
                    }
                }
            }
            let indice = (k == "0" || (!k.starts_with('0') && !k.is_empty()))
                && k.bytes().all(|b| b.is_ascii_digit());
            segs.push(match k.parse() {
                Ok(i) if indice => Segmento::Indice(i),
                _ => Segmento::Chave(k),
            });
            pos += token.len() + 1;
        }
        Ok(Self(segs))
    }

    /// Lê o dialeto jq que `jq()` escreve: `.`, `.chave`, `["texto"]`, `[n]`, em sequência.
    pub fn ler_jq(texto: &str) -> Result<Self, ErroCaminho> {
        let erro = |posicao, motivo| Err(ErroCaminho { posicao, motivo });
        let b = texto.as_bytes();
        if b.first() != Some(&b'.') {
            return erro(0, "caminho jq começa com `.`");
        }
        let (mut i, mut ponto, mut segs) = (1, true, Vec::new());
        while i < b.len() {
            match b[i] {
                b'[' => {
                    i += 1;
                    if b.get(i) == Some(&b'"') {
                        let Some((k, n)) = ler_texto_json(&texto[i..]) else {
                            return erro(i, "texto entre aspas malformado");
                        };
                        segs.push(Segmento::Chave(k));
                        i += n;
                    } else {
                        let n = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
                        let Ok(n_) = texto[i..i + n].parse() else {
                            return erro(i, "esperava índice (número inteiro ≥ 0) ou texto");
                        };
                        segs.push(Segmento::Indice(n_));
                        i += n;
                    }
                    if b.get(i) != Some(&b']') {
                        return erro(i, "falta `]`");
                    }
                    i += 1;
                    ponto = false;
                }
                b'.' if !ponto => {
                    i += 1;
                    ponto = true;
                }
                c if ponto && (c.is_ascii_alphabetic() || c == b'_') => {
                    let n = b[i..]
                        .iter()
                        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
                        .count();
                    segs.push(Segmento::Chave(texto[i..i + n].to_owned()));
                    i += n;
                    ponto = false;
                }
                _ => return erro(i, "esperava `.chave`, `[\"texto\"]` ou `[n]`"),
            }
        }
        if ponto && !segs.is_empty() {
            return erro(i, "ponto sobrando no fim");
        }
        Ok(Self(segs))
    }
}

/// Lê um texto JSON entre aspas no início de `s`; devolve o valor e quantos bytes consumiu.
fn ler_texto_json(s: &str) -> Option<(String, usize)> {
    let mut c = s.char_indices();
    if c.next()?.1 != '"' {
        return None;
    }
    let mut out = String::new();
    let hex = |c: &mut std::str::CharIndices| -> Option<u32> {
        (0..4).try_fold(0, |n, _| Some(n * 16 + c.next()?.1.to_digit(16)?))
    };
    while let Some((i, ch)) = c.next() {
        match ch {
            '"' => return Some((out, i + 1)),
            '\\' => out.push(match c.next()?.1 {
                '"' => '"',
                '\\' => '\\',
                '/' => '/',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'u' => {
                    let alto = hex(&mut c)?;
                    if (0xD800..0xDC00).contains(&alto) {
                        let ('\\', 'u') = (c.next()?.1, c.next()?.1) else {
                            return None;
                        };
                        let baixo = hex(&mut c)?;
                        if !(0xDC00..0xE000).contains(&baixo) {
                            return None;
                        }
                        char::from_u32(0x10000 + ((alto - 0xD800) << 10) + (baixo - 0xDC00))?
                    } else {
                        char::from_u32(alto)?
                    }
                }
                _ => return None,
            }),
            c if c < ' ' => return None,
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod testes {
    use super::*;
    use Segmento::{Chave, Indice};
    use proptest::prelude::*;

    fn c(segs: &[Segmento]) -> Caminho {
        Caminho(segs.to_vec())
    }

    fn k(s: &str) -> Segmento {
        Chave(s.into())
    }

    /// (caminho, jq, JSONPath, JSON Pointer, JavaScript)
    fn casos() -> Vec<(
        Caminho,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    )> {
        vec![
            (c(&[]), ".", "$", "", "dado"),
            (
                c(&[k("a"), k("b"), Indice(0)]),
                ".a.b[0]",
                "$.a.b[0]",
                "/a/b/0",
                "dado.a.b[0]",
            ),
            (c(&[k("")]), r#".[""]"#, "$['']", "/", r#"dado[""]"#),
            (
                c(&[k("chave com espaço")]),
                r#".["chave com espaço"]"#,
                "$['chave com espaço']",
                "/chave com espaço",
                r#"dado["chave com espaço"]"#,
            ),
            (
                c(&[k("a.b")]),
                r#".["a.b"]"#,
                "$['a.b']",
                "/a.b",
                r#"dado["a.b"]"#,
            ),
            (
                c(&[k(r#"di"sse'"#)]),
                r#".["di\"sse'"]"#,
                r#"$['di"sse\'']"#,
                r#"/di"sse'"#,
                r#"dado["di\"sse'"]"#,
            ),
            (
                c(&[k("a/b"), k("c~d"), k("~1")]),
                r#".["a/b"]["c~d"]["~1"]"#,
                "$['a/b']['c~d']['~1']",
                "/a~1b/c~0d/~01",
                r#"dado["a/b"]["c~d"]["~1"]"#,
            ),
            (
                c(&[k("linha\nnova"), k("\u{1}\t\\")]),
                r#".["linha\nnova"]["\u0001\t\\"]"#,
                r"$['linha\nnova']['\u0001\t\\']",
                "/linha\nnova/\u{1}\t\\",
                r#"dado["linha\nnova"]["\u0001\t\\"]"#,
            ),
            (
                c(&[k("ação"), k("名前"), k("😀")]),
                r#".["ação"]["名前"]["😀"]"#,
                "$.ação.名前.😀",
                "/ação/名前/😀",
                r#"dado["ação"]["名前"]["😀"]"#,
            ),
            (
                c(&[k("lista"), Indice(3), k("0")]),
                r#".lista[3]["0"]"#,
                "$.lista[3]['0']",
                "/lista/3/0",
                r#"dado.lista[3]["0"]"#,
            ),
            (
                c(&[k("class"), k("if"), k("$x"), k("_y1")]),
                r#".class["if"]["$x"]._y1"#,
                "$.class.if['$x']._y1",
                "/class/if/$x/_y1",
                r#"dado["class"]["if"].$x._y1"#,
            ),
            (
                c(&[Indice(0), k("chave-com-hífen")]),
                r#".[0]["chave-com-hífen"]"#,
                "$[0]['chave-com-hífen']",
                "/0/chave-com-hífen",
                r#"dado[0]["chave-com-hífen"]"#,
            ),
            (
                c(&[k("1a"), k("\u{2028}")]),
                r#".["1a"]["\u2028"]"#,
                "$['1a'].\u{2028}",
                "/1a/\u{2028}",
                r#"dado["1a"]["\u2028"]"#,
            ),
        ]
    }

    #[test]
    fn quatro_formatos() {
        for (cam, jq, jp, ptr, js) in casos() {
            assert_eq!(cam.jq(), jq, "{cam:?}");
            assert_eq!(cam.jsonpath(), jp, "{cam:?}");
            assert_eq!(cam.json_pointer(), ptr, "{cam:?}");
            assert_eq!(cam.javascript(), js, "{cam:?}");
            assert_eq!(Caminho::ler_jq(jq).unwrap(), cam, "{jq}");
        }
    }

    #[test]
    fn pointer_le_de_volta_e_chave_numerica_vira_indice() {
        for (cam, _, _, ptr, _) in casos() {
            assert_eq!(Caminho::ler_json_pointer(ptr).unwrap(), normalizar(&cam));
        }
        assert_eq!(
            Caminho::ler_json_pointer("/0/00/01/-/").unwrap(),
            c(&[Indice(0), k("00"), k("01"), k("-"), k("")])
        );
        // `~01` é `~1`, nunca `/`.
        assert_eq!(Caminho::ler_json_pointer("/~01").unwrap(), c(&[k("~1")]));
    }

    #[test]
    fn leitura_recusa_texto_malformado() {
        for p in ["a", "/~", "/~2", "/a~"] {
            assert!(Caminho::ler_json_pointer(p).is_err(), "{p}");
        }
        for q in [
            "",
            "a",
            "..",
            ".a.",
            ".a..b",
            ".[",
            ".[-1]",
            ".[\"a]",
            ".[\"a\"",
            ".[\"\\x\"]",
            ".[\"\\ud800\"]",
            ".a b",
            ".1",
        ] {
            assert!(Caminho::ler_jq(q).is_err(), "{q}");
        }
        let e = Caminho::ler_jq(".a.").unwrap_err();
        assert_eq!(
            e.to_string(),
            "caminho inválido no byte 3: ponto sobrando no fim"
        );
    }

    #[test]
    fn jq_aceita_variantes_comuns() {
        assert_eq!(
            Caminho::ler_jq(r#".["a"].b[0].[1]["\u00e7\/\ud83d\ude00"]"#).unwrap(),
            c(&[k("a"), k("b"), Indice(0), Indice(1), k("ç/😀")])
        );
    }

    fn normalizar(cam: &Caminho) -> Caminho {
        Caminho(
            cam.0
                .iter()
                .map(|s| match s {
                    Chave(x) if (x == "0" || !x.starts_with('0')) && x.parse::<usize>().is_ok() => {
                        Indice(x.parse().unwrap())
                    }
                    s => s.clone(),
                })
                .collect(),
        )
    }

    fn segmento() -> impl Strategy<Value = Segmento> {
        prop_oneof![
            any::<String>().prop_map(Chave),
            "[a-z0-9~/._ $\"'\\\\-]{0,6}".prop_map(Chave),
            any::<usize>().prop_map(Indice),
        ]
    }

    proptest! {
        #[test]
        fn ida_e_volta_jq(segs in prop::collection::vec(segmento(), 0..8)) {
            let cam = Caminho(segs);
            prop_assert_eq!(Caminho::ler_jq(&cam.jq()).unwrap(), cam);
        }

        #[test]
        fn ida_e_volta_json_pointer(segs in prop::collection::vec(segmento(), 0..8)) {
            let cam = Caminho(segs);
            prop_assert_eq!(Caminho::ler_json_pointer(&cam.json_pointer()).unwrap(), normalizar(&cam));
        }
    }
}
