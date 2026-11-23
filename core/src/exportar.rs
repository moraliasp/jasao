//! Export da estrutura inferida como JSON Schema 2020-12 e Zod 4.
//!
//! Um modelo só (`Forma`, montado do `Resumo`) e dois impressores. O JSON Schema é a fonte da
//! autovalidação (`crate::validar`, com um validador de terceiros); o Zod sai da mesma `Forma`.
//! Decisão e limites: `docs/decisoes.md`, "Exportar Zod e JSON Schema".
//!
//! Segurança: nomes de campo e literais entram no código gerado só por `aspas` (JSON com
//! escape de `<`, `>`, `&`, controles, bidi e U+2028/2029). Nenhum outro valor do arquivo entra.

use crate::estrutura::{Discriminador, No, Resumo, Tipo, Unico};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Opcoes {
    /// Campo com um valor só: travar no valor (`const` / `z.literal`) em vez do tipo genérico.
    pub literal: bool,
    /// Texto que é sempre número: tratar como número (Schema `number`; Zod confere o padrão e
    /// converte com `.transform(Number)`) em vez de texto com padrão numérico.
    pub numero: bool,
}

/// Padrão (ECMA, só ASCII) de um número JSON dentro de um texto; o mesmo de `eh_numero`.
pub const PADRAO_NUMERO: &str = r"^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$";

#[derive(Debug, Clone, PartialEq)]
struct Forma {
    nulo: bool,
    /// Vazio e `!nulo` = qualquer valor (caminho nunca visto, como itens de lista sempre vazia).
    alts: Vec<Alt>,
}

#[derive(Debug, Clone, PartialEq)]
enum Alt {
    Booleano,
    /// `seguro = false`: algum inteiro passa de 2^53 e o Zod usa `z.number()`.
    Inteiro {
        seguro: bool,
    },
    Numero,
    Texto,
    /// Texto que é sempre número.
    TextoNumero,
    /// Literal já em JSON (texto entre aspas escapado, `true`/`false` ou inteiro seguro).
    Literal(String),
    Objeto(Vec<Campo>),
    Mapa(Box<Forma>),
    Lista(Box<Forma>),
    /// União discriminada no topo: campo e as variantes (cada uma um objeto).
    Uniao(String, Vec<Vec<Campo>>),
}

#[derive(Debug, Clone, PartialEq)]
struct Campo {
    nome: String,
    opcional: bool,
    forma: Forma,
}

/// Decodifica um texto JSON cru (sem aspas, escapes intactos). `None` se não for UTF-8 válido.
fn decodificar(cru: &[u8]) -> Option<String> {
    let mut v = Vec::with_capacity(cru.len() + 2);
    v.push(b'"');
    v.extend_from_slice(cru);
    v.push(b'"');
    sonic_rs::from_slice::<String>(&v).ok()
}

fn forma(no: &No, op: Opcoes) -> Forma {
    let n = |t: Tipo| no.tipo(t).n > 0;
    let mut alts = Vec::new();
    let escalares = [Tipo::Booleano, Tipo::Inteiro, Tipo::Decimal, Tipo::Texto];
    let nao_nulos: Vec<Tipo> = Tipo::TODOS
        .into_iter()
        .filter(|&t| t != Tipo::Nulo && n(t))
        .collect();
    if op.literal
        && let [t] = nao_nulos[..]
        && escalares.contains(&t)
        && let Unico::Um(_, v) = &no.valores.unico
        && let Some(lit) = literal(t, v)
    {
        return Forma {
            nulo: n(Tipo::Nulo),
            alts: vec![Alt::Literal(lit)],
        };
    }
    if n(Tipo::Booleano) {
        alts.push(Alt::Booleano);
    }
    match (n(Tipo::Inteiro), n(Tipo::Decimal)) {
        (_, true) => alts.push(Alt::Numero),
        (true, false) => alts.push(Alt::Inteiro {
            seguro: !no.valores.inteiro_grande,
        }),
        _ => {}
    }
    if n(Tipo::Texto) {
        alts.push(if !no.valores.texto_nao_numerico {
            Alt::TextoNumero
        } else {
            Alt::Texto
        });
    }
    if n(Tipo::Objeto) {
        alts.push(match &no.mapa {
            Some(m) => Alt::Mapa(Box::new(forma(m, op))),
            None => Alt::Objeto(campos(no, op)),
        });
    }
    if n(Tipo::Lista) {
        alts.push(Alt::Lista(Box::new(no.elementos.as_deref().map_or(
            Forma {
                nulo: false,
                alts: vec![],
            },
            |e| forma(e, op),
        ))));
    }
    Forma {
        nulo: n(Tipo::Nulo),
        alts,
    }
}

/// Literal em JSON para `Unico::Um`. Número só se for inteiro seguro no JS (decimal e inteiro
/// grande mudariam de valor ao passar por `f64`).
fn literal(t: Tipo, v: &[u8]) -> Option<String> {
    match t {
        Tipo::Texto => decodificar(v).map(|s| aspas(&s)),
        Tipo::Booleano => Some(String::from_utf8_lossy(v).into_owned()),
        Tipo::Inteiro => {
            let s = std::str::from_utf8(v).ok()?;
            let n: i64 = s.parse().ok()?;
            (n.unsigned_abs() < 1 << 53).then(|| n.to_string())
        }
        _ => None,
    }
}

/// Campos de um objeto, em ordem de primeira aparição. Chaves que decodificam igual
/// (`"a"` e `"a"`) viram um campo só.
fn campos(no: &No, op: Opcoes) -> Vec<Campo> {
    let objetos = no.tipo(Tipo::Objeto).n;
    let mut v: Vec<(&[u8], &No)> = no.filhos.iter().map(|(k, f)| (&**k, f)).collect();
    v.sort_by(|a, b| (a.1.ordem(), a.0).cmp(&(b.1.ordem(), b.0)));
    let mut saida: Vec<(String, No)> = Vec::new();
    for (k, f) in v {
        let nome = decodificar(k).unwrap_or_else(|| String::from_utf8_lossy(k).into_owned());
        match saida.iter_mut().find(|(n, _)| *n == nome) {
            Some((_, g)) => g.combinar(f.clone()),
            None => saida.push((nome, f.clone())),
        }
    }
    saida
        .into_iter()
        .map(|(nome, f)| Campo {
            opcional: f.presenca < objetos,
            forma: forma(&f, op),
            nome,
        })
        .collect()
}

fn modelo(r: &Resumo, op: Opcoes) -> Forma {
    let raiz = &r.raiz;
    let so_objetos = raiz.tipo(Tipo::Objeto).n == r.registros() && raiz.mapa.is_none();
    // Mais de um candidato: o de menos variantes (um id ou data única também separa).
    let ds = r.discriminadores();
    match ds.iter().min_by_key(|d| d.variantes.len()) {
        Some(d) if so_objetos && r.registros() > 0 => Forma {
            nulo: false,
            alts: vec![uniao(raiz, d, op)],
        },
        _ => forma(raiz, op),
    }
}

/// Variante = campos do topo que aparecem nos registros dela (obrigatório se em todos), com a
/// forma geral do campo; o discriminador vira o literal da variante.
fn uniao(raiz: &No, d: &Discriminador, op: Opcoes) -> Alt {
    let todos = campos(raiz, op);
    let campo = decodificar(&d.cru).unwrap_or_else(|| String::from_utf8_lossy(&d.cru).into());
    let nome = |cru: &str| decodificar(cru.as_bytes()).unwrap_or_else(|| cru.to_string());
    let variantes = d
        .variantes
        .iter()
        .map(|v| {
            let obrig: Vec<String> = v.obrigatorias.iter().map(|k| nome(k)).collect();
            let opc: Vec<String> = v.opcionais.iter().map(|(k, _)| nome(k)).collect();
            todos
                .iter()
                .filter_map(|c| {
                    let opcional = if obrig.contains(&c.nome) {
                        false
                    } else if opc.contains(&c.nome) {
                        true
                    } else {
                        return None;
                    };
                    let forma = if c.nome == campo {
                        Forma {
                            nulo: false,
                            alts: vec![Alt::Literal(aspas(
                                &decodificar(v.valor.as_bytes()).unwrap_or_default(),
                            ))],
                        }
                    } else {
                        c.forma.clone()
                    };
                    Some(Campo {
                        nome: c.nome.clone(),
                        opcional,
                        forma,
                    })
                })
                .collect()
        })
        .collect();
    Alt::Uniao(campo, variantes)
}

/// Texto JSON entre aspas, válido também como literal JS/TS, e só ASCII: escapa, além do que o
/// JSON exige, `<`, `>` e `&` (para `</script>` e HTML), DEL e todo caractere não-ASCII em
/// `\uXXXX` (pares substitutos fora do BMP). Assim nenhum controle bidi, invisível ou parecido
/// com outro chega cru ao código.
pub fn aspas(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{0}'..='\u{1f}' | '<' | '>' | '&' | '\u{7f}'.. => {
                for u in c.encode_utf16(&mut [0; 2]) {
                    o.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ---------------------------------------------------------------------------------------------
// JSON Schema 2020-12.

/// JSON mínimo, para imprimir com indentação e com `aspas` em todo texto.
enum J {
    Obj(Vec<(&'static str, J)>),
    /// Objeto com chaves vindas do arquivo (`properties`).
    Props(Vec<(String, J)>),
    Arr(Vec<J>),
    /// Já em JSON (número, booleano, literal escapado).
    Cru(String),
}

fn txt(s: &str) -> J {
    J::Cru(aspas(s))
}

fn imprimir(j: &J, nivel: usize, o: &mut String) {
    let pad = |n: usize| "  ".repeat(n);
    let lista = |itens: Vec<(Option<String>, &J)>, abre: char, fecha: char, o: &mut String| {
        if itens.is_empty() {
            o.push(abre);
            o.push(fecha);
            return;
        }
        o.push(abre);
        for (i, (k, v)) in itens.iter().enumerate() {
            o.push_str(if i == 0 { "\n" } else { ",\n" });
            o.push_str(&pad(nivel + 1));
            if let Some(k) = k {
                o.push_str(k);
                o.push_str(": ");
            }
            imprimir(v, nivel + 1, o);
        }
        o.push('\n');
        o.push_str(&pad(nivel));
        o.push(fecha);
    };
    match j {
        J::Cru(s) => o.push_str(s),
        J::Obj(v) => lista(
            v.iter().map(|(k, v)| (Some(aspas(k)), v)).collect(),
            '{',
            '}',
            o,
        ),
        J::Props(v) => lista(
            v.iter().map(|(k, v)| (Some(aspas(k)), v)).collect(),
            '{',
            '}',
            o,
        ),
        // Lista só de valores simples (`type`, `required`) numa linha.
        J::Arr(v) if v.iter().all(|j| matches!(j, J::Cru(_))) => {
            let itens: Vec<&str> = v
                .iter()
                .map(|j| match j {
                    J::Cru(s) => s.as_str(),
                    _ => unreachable!(),
                })
                .collect();
            o.push('[');
            o.push_str(&itens.join(", "));
            o.push(']');
        }
        J::Arr(v) => lista(v.iter().map(|v| (None, v)).collect(), '[', ']', o),
    }
}

fn schema_forma(f: &Forma, op: Opcoes) -> J {
    if f.alts.is_empty() {
        return if f.nulo {
            J::Obj(vec![("type", txt("null"))])
        } else {
            J::Obj(vec![])
        };
    }
    // Tipos simples juntos num `type: [...]`; o resto em `anyOf`.
    let mut tipos: Vec<&str> = Vec::new();
    let mut outros: Vec<J> = Vec::new();
    for a in &f.alts {
        match a {
            Alt::Booleano => tipos.push("boolean"),
            Alt::Inteiro { .. } => tipos.push("integer"),
            Alt::Numero => tipos.push("number"),
            Alt::Texto => tipos.push("string"),
            Alt::TextoNumero if op.numero => tipos.push("number"),
            Alt::TextoNumero => outros.push(J::Obj(vec![
                ("type", txt("string")),
                ("pattern", txt(PADRAO_NUMERO)),
            ])),
            Alt::Literal(l) => outros.push(J::Obj(vec![("const", J::Cru(l.clone()))])),
            Alt::Objeto(c) => outros.push(schema_objeto(c, op)),
            Alt::Mapa(m) => outros.push(J::Obj(vec![
                ("type", txt("object")),
                ("additionalProperties", schema_forma(m, op)),
            ])),
            Alt::Lista(e) => outros.push(J::Obj(vec![
                ("type", txt("array")),
                ("items", schema_forma(e, op)),
            ])),
            Alt::Uniao(_, vs) => outros.push(J::Obj(vec![(
                "anyOf",
                J::Arr(vs.iter().map(|c| schema_objeto(c, op)).collect()),
            )])),
        }
    }
    tipos.dedup();
    if f.nulo {
        if tipos.is_empty() {
            outros.push(J::Obj(vec![("type", txt("null"))]));
        } else {
            tipos.push("null");
        }
    }
    let tipo = match tipos[..] {
        [] => None,
        [t] => Some(txt(t)),
        _ => Some(J::Arr(tipos.iter().map(|t| txt(t)).collect())),
    };
    match (tipo, outros.len()) {
        (Some(t), 0) => J::Obj(vec![("type", t)]),
        (None, 1) => outros.pop().expect("um"),
        (t, _) => {
            if let Some(t) = t {
                outros.insert(0, J::Obj(vec![("type", t)]));
            }
            J::Obj(vec![("anyOf", J::Arr(outros))])
        }
    }
}

fn schema_objeto(c: &[Campo], op: Opcoes) -> J {
    let mut v = Vec::new();
    v.push(("type", txt("object")));
    v.push((
        "properties",
        J::Props(
            c.iter()
                .map(|c| (c.nome.clone(), schema_forma(&c.forma, op)))
                .collect(),
        ),
    ));
    let req: Vec<J> = c
        .iter()
        .filter(|c| !c.opcional)
        .map(|c| txt(&c.nome))
        .collect();
    if !req.is_empty() {
        v.push(("required", J::Arr(req)));
    }
    J::Obj(v)
}

/// JSON Schema 2020-12 de um registro.
pub fn schema(r: &Resumo, op: Opcoes) -> String {
    let f = modelo(r, op);
    let cabeca = vec![
        (
            "$schema",
            txt("https://json-schema.org/draft/2020-12/schema"),
        ),
        ("title", txt("Registro")),
    ];
    let j = match schema_forma(&f, op) {
        J::Obj(mut v) => {
            let mut c = cabeca;
            c.append(&mut v);
            J::Obj(c)
        }
        _ => unreachable!("schema_forma devolve objeto"),
    };
    let mut o = String::new();
    imprimir(&j, 0, &mut o);
    o.push('\n');
    o
}

// ---------------------------------------------------------------------------------------------
// Zod 4.

/// Nome de propriedade num objeto TS: identificador ASCII sem aspas; `__proto__` computado (num
/// literal de objeto, `__proto__:` e `"__proto__":` trocam o protótipo em vez de criar campo).
fn chave_ts(k: &str) -> String {
    let ident = |c: char, ini: bool| {
        c == '_' || c == '$' || c.is_ascii_alphabetic() || (!ini && c.is_ascii_digit())
    };
    let mut cs = k.chars();
    if k == "__proto__" {
        format!("[{}]", aspas(k))
    } else if cs.next().is_some_and(|c| ident(c, true)) && cs.all(|c| ident(c, false)) {
        k.to_string()
    } else {
        aspas(k)
    }
}

fn zod_forma(f: &Forma, nivel: usize, op: Opcoes) -> String {
    if f.alts.is_empty() {
        return if f.nulo { "z.null()" } else { "z.unknown()" }.into();
    }
    let alts: Vec<String> = f
        .alts
        .iter()
        .map(|a| match a {
            Alt::Booleano => "z.boolean()".into(),
            Alt::Inteiro { seguro: true } => "z.int()".into(),
            Alt::Inteiro { seguro: false } | Alt::Numero => "z.number()".into(),
            Alt::Texto => "z.string()".into(),
            // Mesmo padrão do Schema; "tratar como número" converte depois de conferir o texto.
            Alt::TextoNumero if op.numero => {
                format!("z.string().regex(/{PADRAO_NUMERO}/).transform(Number)")
            }
            Alt::TextoNumero => format!("z.string().regex(/{PADRAO_NUMERO}/)"),
            Alt::Literal(l) => format!("z.literal({l})"),
            Alt::Objeto(c) => zod_objeto(c, nivel, op),
            Alt::Mapa(m) => format!("z.record(z.string(), {})", zod_forma(m, nivel, op)),
            Alt::Lista(e) => format!("z.array({})", zod_forma(e, nivel, op)),
            Alt::Uniao(campo, vs) => {
                let pad = "  ".repeat(nivel + 1);
                let itens: Vec<String> = vs
                    .iter()
                    .map(|c| format!("{pad}{},\n", zod_objeto(c, nivel + 1, op)))
                    .collect();
                format!(
                    "z.discriminatedUnion({}, [\n{}{}])",
                    aspas(campo),
                    itens.concat(),
                    "  ".repeat(nivel)
                )
            }
        })
        .collect();
    let base = match &alts[..] {
        [a] => a.clone(),
        _ => format!("z.union([{}])", alts.join(", ")),
    };
    if f.nulo {
        format!("{base}.nullable()")
    } else {
        base
    }
}

fn zod_objeto(c: &[Campo], nivel: usize, op: Opcoes) -> String {
    if c.is_empty() {
        return "z.object({})".into();
    }
    let pad = "  ".repeat(nivel + 1);
    let corpo: String = c
        .iter()
        .map(|c| {
            format!(
                "{pad}{}: {}{},\n",
                chave_ts(&c.nome),
                zod_forma(&c.forma, nivel + 1, op),
                if c.opcional { ".optional()" } else { "" }
            )
        })
        .collect();
    format!("z.object({{\n{corpo}{}}})", "  ".repeat(nivel))
}

/// Módulo TypeScript com o Zod 4 de um registro.
pub fn zod(r: &Resumo, op: Opcoes) -> String {
    format!(
        "import {{ z }} from \"zod\";\n\nexport const Registro = {};\n\nexport type Registro = z.infer<typeof Registro>;\n",
        zod_forma(&modelo(r, op), 0, op)
    )
}

/// Para o Zod, o JSON Schema que diz o que ele aceita na entrada: com "tratar como número" o Zod
/// confere o mesmo padrão de texto antes de converter, então valida como "manter texto".
pub fn schema_da_entrada_zod(r: &Resumo, op: Opcoes) -> String {
    schema(
        r,
        Opcoes {
            numero: false,
            ..op
        },
    )
}
