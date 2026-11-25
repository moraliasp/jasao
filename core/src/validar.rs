//! Autovalidação: o arquivo inteiro contra um JSON Schema 2020-12, com um validador de
//! terceiros (`jsonschema`), independente do gerador (`crate::exportar`). Cada registro é lido
//! pelo `serde_json` (não pelo leitor da inferência) e conferido em paralelo.

use std::collections::HashMap;
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
/// Maior schema aceito (o de fora é entrada não confiável; o gerado fica muito abaixo).
pub const MAX_SCHEMA: usize = 1 << 20;
/// Maior aninhamento de objeto/lista no schema.
pub const MAX_PROFUNDIDADE: usize = 64;
/// Teto do schema expandido: valores visitados seguindo cada `$ref` como se o alvo estivesse
/// copiado no lugar (a volta de uma recursão é cortada). Limita o custo de validar um nó do
/// dado; sem isto, `allOf` com dois `$ref` ao nível seguinte dobra o custo a cada nível.
pub const MAX_EXPANDIDO: usize = 100_000;
/// Mais objetos/listas encadeados que isto (aninhamento mais `$ref` seguidos) é recusado: o
/// `jsonschema` segue a cadeia por recursão, e numa thread de 2 MiB a cadeia de 3 mil `$ref`
/// estoura a pilha em debug (2 mil passa; em release, 4 mil). Margem de 2× sobre o debug.
pub const MAX_ENCADEADO: usize = 1024;
/// Voltas de recursão abertas ao mesmo tempo na expansão.
const MAX_VOLTAS: usize = 64;

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
    /// Linhas que não são JSON (a regra da árvore, `arvore::validar`) ou aninhadas acima de 128
    /// (`estrutura::PROFUNDIDADE_MAX`, o limite da inferência): não contam como registro.
    pub invalidas: u64,
    /// Registros (JSON válido para a árvore e a inferência) que o `serde_json` não lê: número
    /// além do `f64`, o alcance do JavaScript. Nem passam nem falham.
    pub recusadas: u64,
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
        self.recusadas += o.recusadas;
        self.falhas.extend(o.falhas);
        self.falhas.sort_by_key(|f| f.linha);
        self.falhas.truncate(MAX_FALHAS);
        self
    }

    fn conferir(&mut self, v: &jsonschema::Validator, linha: u64, b: &[u8]) {
        let cheio = self.falhas.len() >= MAX_FALHAS;
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
        // O motivo é um código `jasao:…` (a interface traduz), não uma frase.
        let invalida = || match crate::arvore::validar(b) {
            Err(l) => {
                let c = serde_json::to_value(l.motivo).unwrap_or_default();
                format!("jasao:{}", c.as_str().unwrap_or("outro"))
            }
            Ok(_) => "jasao:aninhamento".into(),
        };
        // Mesmo corte da inferência: até `PROFUNDIDADE_MAX` níveis é registro. Conferido antes,
        // porque a leitura abaixo não tem limite de recursão.
        if crate::arvore::passa_da_profundidade(b, crate::estrutura::PROFUNDIDADE_MAX).is_some() {
            falha(invalida());
            self.invalidas += 1;
            return;
        }
        match ler(b) {
            Err(e) => match crate::arvore::validar(b) {
                Err(_) => {
                    falha(invalida());
                    self.invalidas += 1;
                }
                Ok(_) => {
                    falha(recusa(&e).into());
                    self.recusadas += 1;
                }
            },
            // Com as falhas já guardadas, `is_valid` evita montar o erro (13× mais rápido em 1 GB que reprova).
            Ok(j) if cheio => match v.is_valid(&j) {
                true => self.passam += 1,
                false => self.falham += 1,
            },
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

/// Lê o registro sem o limite de 127 níveis do `serde_json`: quem chama já cortou acima de
/// `estrutura::PROFUNDIDADE_MAX`, que é o que protege a pilha.
fn ler(b: &[u8]) -> serde_json::Result<serde_json::Value> {
    let mut d = serde_json::Deserializer::from_slice(b);
    d.disable_recursion_limit();
    let v = serde::Deserialize::deserialize(&mut d)?;
    d.end()?;
    Ok(v)
}

/// Por que o `serde_json` recusou um JSON que a árvore aceita (mensagens estáveis dele).
fn recusa(e: &serde_json::Error) -> &'static str {
    if e.to_string().starts_with("number out of range") {
        "jasao:numero_fora_do_alcance"
    } else {
        "jasao:recusado_pelo_validador"
    }
}

/// Motivo do registro que o schema reprovou. Em `anyOf` (união), o erro do ramo que chegou
/// mais perto: o do discriminador que bateu (sem erro de `const`), depois o de menos erros.
///
/// Os tipos comuns saem como código para a interface traduzir:
/// `jasao:schema_<tipo>|<caminho>|<parâmetro>…`, campos separados por `|`, com `%` e `|` dentro
/// de cada campo escritos `%25` e `%7C` (o caminho é a chave do arquivo e pode ter qualquer
/// caractere). O caminho vem primeiro e é vazio na raiz. Os parâmetros vêm só do schema (tipo,
/// limite, formato, `pattern`, valores de `enum`/`const`, propriedade exigida), nunca do valor
/// do registro. Exceção: o nome da propriedade a mais (`additionalProperties`), que é chave do
/// arquivo e que o `masked()` já mostrava. Tipo sem código: `"{caminho}: {mensagem}"` mascarada.
fn motivo(e: &jsonschema::ValidationError) -> String {
    use jsonschema::error::ValidationErrorKind::{
        AdditionalProperties, AnyOf, Constant, Enum, ExclusiveMaximum, ExclusiveMinimum, Format,
        MaxLength, Maximum, MinLength, Minimum, Pattern, Required, Type,
    };
    if let AnyOf { context } = e.kind()
        && let Some(ramo) = context.iter().filter(|r| !r.is_empty()).min_by_key(|r| {
            (
                r.iter().any(|e| matches!(e.kind(), Constant { .. })),
                r.len(),
            )
        })
    {
        return motivo(&ramo[0]);
    }
    let onde = e.instance_path().to_string();
    let (cod, ps): (&str, Vec<String>) = match e.kind() {
        Type { kind } => (
            "tipo",
            vec![match kind {
                jsonschema::error::TypeKind::Single(t) => t.to_string(),
                jsonschema::error::TypeKind::Multiple(ts) => ts
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            }],
        ),
        Required { property } => (
            "obrigatoria",
            vec![
                property
                    .as_str()
                    .map_or_else(|| property.to_string(), str::to_owned),
            ],
        ),
        Enum { options } => ("enum", vec![options.to_string()]),
        Constant { expected_value } => ("const", vec![expected_value.to_string()]),
        Minimum { limit } => ("minimo", vec![limit.to_string()]),
        Maximum { limit } => ("maximo", vec![limit.to_string()]),
        ExclusiveMinimum { limit } => ("minimo_exclusivo", vec![limit.to_string()]),
        ExclusiveMaximum { limit } => ("maximo_exclusivo", vec![limit.to_string()]),
        MinLength { limit } => ("tamanho_minimo", vec![limit.to_string()]),
        MaxLength { limit } => ("tamanho_maximo", vec![limit.to_string()]),
        Pattern { pattern } => ("pattern", vec![pattern.clone()]),
        Format { format } => ("formato", vec![format.clone()]),
        AdditionalProperties { unexpected } => ("propriedade_extra", vec![unexpected.join(",")]),
        _ => {
            let msg = e.masked().to_string();
            return if onde.is_empty() {
                msg
            } else {
                format!("{onde}: {msg}")
            };
        }
    };
    let campo = |s: &str| s.replace('%', "%25").replace('|', "%7C");
    let mut m = format!("jasao:schema_{cod}|{}", campo(&onde));
    for p in &ps {
        m.push('|');
        m.push_str(&campo(p));
    }
    m
}

/// O motivo em frase PT para o terminal (as mesmas de `motivosSchema` em `app/src/i18n.ts`):
/// `jasao:schema_*` é decodificado (desescape de passada única, o inverso do de `motivo`); os
/// demais `jasao:<cod>` perdem só o prefixo; o que não é código passa como veio.
pub fn motivo_legivel(m: &str) -> String {
    let Some(r) = m.strip_prefix("jasao:schema_") else {
        return m.strip_prefix("jasao:").unwrap_or(m).to_string();
    };
    let mut campos = r.split('|').map(desescapar);
    let cod = campos.next().unwrap_or_default();
    let onde = campos.next().unwrap_or_default();
    let ps: Vec<String> = campos.collect();
    let Some(p) = ps.first() else {
        return m.strip_prefix("jasao:").unwrap_or(m).to_string();
    };
    let frase = match cod.as_str() {
        "tipo" => {
            let tipos: Vec<&str> = p
                .split(',')
                .map(|t| match t {
                    "string" => "texto",
                    "integer" => "inteiro",
                    "number" => "número",
                    "boolean" => "booleano",
                    "array" => "lista",
                    "object" => "objeto",
                    "null" => "nulo",
                    o => o,
                })
                .collect();
            format!("deve ser do tipo {}", tipos.join(" ou "))
        }
        "obrigatoria" => format!("falta a propriedade obrigatória \"{p}\""),
        "enum" => format!("deve ser um destes valores: {p}"),
        "const" => format!("deve ser exatamente {p}"),
        "minimo" => format!("deve ser maior ou igual a {p}"),
        "maximo" => format!("deve ser menor ou igual a {p}"),
        "minimo_exclusivo" => format!("deve ser maior que {p}"),
        "maximo_exclusivo" => format!("deve ser menor que {p}"),
        "tamanho_minimo" => format!("deve ter no mínimo {p} caracteres"),
        "tamanho_maximo" => format!("deve ter no máximo {p} caracteres"),
        "pattern" => format!("não bate com o padrão {p}"),
        "formato" => format!("não está no formato \"{p}\""),
        "propriedade_extra" => format!("propriedade não permitida: {p}"),
        _ => return m.strip_prefix("jasao:").unwrap_or(m).to_string(),
    };
    if onde.is_empty() {
        frase
    } else {
        format!("{onde}: {frase}")
    }
}

/// Inverso de `campo` em `motivo`: `%25` -> `%`, `%7C` -> `|`, numa passada (`%257C` -> `%7C`).
fn desescapar(c: &str) -> String {
    let mut o = String::with_capacity(c.len());
    let mut r = c;
    while let Some(i) = r.find('%') {
        o.push_str(&r[..i]);
        r = &r[i..];
        if let Some(t) = r.strip_prefix("%25") {
            o.push('%');
            r = t;
        } else if let Some(t) = r.strip_prefix("%7C") {
            o.push('|');
            r = t;
        } else {
            o.push('%');
            r = &r[1..];
        }
    }
    o.push_str(r);
    o
}

/// Lê o schema de um arquivo sem trazer mais que `MAX_SCHEMA + 1` bytes para a memória: o
/// excesso fica para `compilar` recusar. O erro cita o caminho.
pub fn ler_schema(caminho: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let erro = |e: &dyn std::fmt::Display| format!("{}: {e}", caminho.display());
    let mut b = Vec::new();
    std::fs::File::open(caminho)
        .map_err(|e| erro(&e))?
        .take(MAX_SCHEMA as u64 + 1)
        .read_to_end(&mut b)
        .map_err(|e| erro(&e))?;
    String::from_utf8(b).map_err(|_| erro(&"não é UTF-8"))
}

/// Compila um schema tratado como não confiável: no máximo `MAX_SCHEMA` bytes e
/// `MAX_PROFUNDIDADE` níveis; `$ref` só local (`#...`), nada buscado na rede nem no disco;
/// `pattern` no motor `regex` (tempo linear, sem retrocesso). Ver `docs/decisoes.md`,
/// "Validar contra schema de fora".
pub fn compilar(schema: &str) -> Result<jsonschema::Validator, String> {
    if schema.len() > MAX_SCHEMA {
        return Err(format!(
            "schema com {} bytes passa do limite de {MAX_SCHEMA}",
            schema.len()
        ));
    }
    let s: serde_json::Value = serde_json::from_str(schema).map_err(|e| e.to_string())?;
    conferir_schema(&s, 1, false)?;
    // Pilha própria: a expansão desce até `MAX_ENCADEADO` níveis com alguns quadros por nível,
    // e quem chama pode estar numa thread de 2 MiB.
    thread::scope(|t| {
        thread::Builder::new()
            .stack_size(32 << 20)
            .spawn_scoped(t, || {
                Custo {
                    raiz: &s,
                    nos: 0,
                    pilha: 0,
                    caminho: Vec::new(),
                    ativos: HashMap::new(),
                    voltas: Vec::new(),
                }
                .visitar(&s)
            })
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "análise do schema falhou".to_string())?
    })?;
    jsonschema::options()
        .offline()
        .with_pattern_options(jsonschema::PatternOptions::regex())
        .build(&s)
        .map_err(|e| e.to_string())
}

/// Profundidade e formato das referências. O `serde_json` já para em 128 níveis, então a
/// recursão aqui é limitada. `nomes`: objeto de nome → schema (`properties` e afins), onde a
/// chave é dado (um campo pode se chamar `$ref`).
fn conferir_schema(v: &serde_json::Value, nivel: usize, nomes: bool) -> Result<(), String> {
    use serde_json::Value::{Array, Object};
    if nivel > MAX_PROFUNDIDADE {
        return Err(format!(
            "schema com mais de {MAX_PROFUNDIDADE} níveis de aninhamento"
        ));
    }
    match v {
        Object(m) => {
            for (k, f) in m {
                // shortcut: `$ref` relativo a um `$id` embutido também é recusado; liberar se
                // aparecer schema de fora legítimo que dependa disso.
                if !nomes
                    && matches!(k.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef")
                    && !f.as_str().is_some_and(|r| r.starts_with('#'))
                {
                    return Err(format!(
                        "{k} fora do próprio schema é proibido (nada é buscado): {}",
                        // Texto vai cru para `aspas` (que põe as aspas); outro valor, como JSON.
                        crate::exportar::aspas(
                            &f.as_str()
                                .map_or_else(|| f.to_string(), str::to_owned)
                                .chars()
                                .take(120)
                                .collect::<String>()
                        )
                    ));
                }
                // Um `$id` abaixo da raiz muda a base dos `$ref` de baixo, e a análise de custo
                // resolve tudo a partir da raiz.
                if !nomes && nivel > 1 && matches!(k.as_str(), "$id" | "id") && f.is_string() {
                    return Err(format!("{k} abaixo da raiz não é aceito"));
                }
                // Valor de dado, não schema: um `{"$ref": ...}` ali não é referência.
                if !nomes && matches!(k.as_str(), "const" | "enum" | "default" | "examples") {
                    continue;
                }
                let filhos_sao_nomes = !nomes
                    && matches!(
                        k.as_str(),
                        "properties"
                            | "patternProperties"
                            | "$defs"
                            | "definitions"
                            | "dependentSchemas"
                    );
                conferir_schema(f, nivel + 1, filhos_sao_nomes)?;
            }
            Ok(())
        }
        Array(a) => a
            .iter()
            .try_for_each(|f| conferir_schema(f, nivel + 1, false)),
        _ => Ok(()),
    }
}

/// Um passo no dado a partir da instância onde o schema se aplica.
#[derive(Debug, Clone, PartialEq)]
enum Passo {
    Campo(String),
    QualquerCampo,
    Item,
}

/// Um caminho pode ser começo do outro (com `QualquerCampo` casando qualquer campo)? Se nenhum
/// par de voltas é, o conjunto é livre de prefixo e cada dado é alcançado por uma só sequência
/// de voltas; se algum é (`[x]` e `[x, x]`), o mesmo dado é alcançado de vários jeitos.
fn cruzam(a: &[Passo], b: &[Passo]) -> bool {
    use Passo::*;
    a.iter().zip(b).all(|p| match p {
        (Campo(x), Campo(y)) => x == y,
        (Item, Item) | (Campo(_) | QualquerCampo, Campo(_) | QualquerCampo) => true,
        _ => false,
    })
}

/// Custo do schema: expande os `$ref` (até `MAX_EXPANDIDO` valores e `MAX_ENCADEADO` níveis)
/// e confere as recursões. Recursão aceita é a que anda no dado a cada volta e cujas voltas
/// abertas não são começo uma da outra (`cruzam`): com duas que são, o número de aplicações
/// num mesmo dado cresce exponencialmente com a profundidade do registro.
struct Custo<'a> {
    raiz: &'a serde_json::Value,
    nos: usize,
    pilha: usize,
    /// Caminho no dado desde a raiz do registro.
    caminho: Vec<Passo>,
    /// Objeto/lista do schema em expansão → tamanho do `caminho` ao entrar nele.
    ativos: HashMap<*const serde_json::Value, usize>,
    /// Voltas de recursão abertas: alvo e caminho no dado percorrido desde o alvo.
    voltas: Vec<(*const serde_json::Value, Vec<Passo>)>,
}

impl<'a> Custo<'a> {
    fn visitar(&mut self, v: &'a serde_json::Value) -> Result<(), String> {
        use serde_json::Value::{Array, Object};
        self.nos += 1;
        if self.nos > MAX_EXPANDIDO {
            return Err(format!(
                "schema expandido (cada $ref no lugar do alvo) passa de {MAX_EXPANDIDO} valores"
            ));
        }
        if !matches!(v, Object(_) | Array(_)) {
            return Ok(());
        }
        self.pilha += 1;
        if self.pilha > MAX_ENCADEADO {
            return Err(format!(
                "schema com mais de {MAX_ENCADEADO} níveis encadeados (aninhamento e $ref seguidos)"
            ));
        }
        let id: *const serde_json::Value = v;
        self.ativos.insert(id, self.caminho.len());
        let r = self.filhos(v);
        self.ativos.remove(&id);
        self.voltas.retain(|(alvo, _)| *alvo != id);
        self.pilha -= 1;
        r
    }

    fn descer(&mut self, p: Passo, v: &'a serde_json::Value) -> Result<(), String> {
        self.caminho.push(p);
        let r = self.visitar(v);
        self.caminho.pop();
        r
    }

    /// Só as palavras que aplicam schema; as desconhecidas o validador não aplica.
    fn filhos(&mut self, v: &'a serde_json::Value) -> Result<(), String> {
        use serde_json::Value::{Array, Object};
        let m = match v {
            Array(a) => return a.iter().try_for_each(|f| self.visitar(f)),
            Object(m) => m,
            _ => return Ok(()),
        };
        for (k, f) in m {
            match (k.as_str(), f) {
                ("$ref" | "$dynamicRef" | "$recursiveRef", serde_json::Value::String(r)) => {
                    self.seguir(r)?
                }
                ("properties", Object(p)) => {
                    for (nome, s) in p {
                        self.descer(Passo::Campo(nome.clone()), s)?;
                    }
                }
                ("patternProperties", Object(p)) => {
                    for s in p.values() {
                        self.descer(Passo::QualquerCampo, s)?;
                    }
                }
                ("dependentSchemas" | "dependencies", Object(p)) => {
                    for s in p.values() {
                        self.visitar(s)?;
                    }
                }
                ("additionalProperties" | "unevaluatedProperties" | "propertyNames", _) => {
                    self.descer(Passo::QualquerCampo, f)?
                }
                (
                    "items" | "prefixItems" | "additionalItems" | "unevaluatedItems" | "contains",
                    _,
                ) => self.descer(Passo::Item, f)?,
                ("allOf" | "anyOf" | "oneOf" | "not" | "if" | "then" | "else", _) => {
                    self.visitar(f)?
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn seguir(&mut self, r: &str) -> Result<(), String> {
        let alvo = resolver(self.raiz, r)?;
        let id: *const serde_json::Value = alvo;
        let Some(&entrada) = self.ativos.get(&id) else {
            return self.visitar(alvo);
        };
        let volta = self.caminho[entrada..].to_vec();
        let mostrar = || crate::exportar::aspas(&r.chars().take(120).collect::<String>());
        if volta.is_empty() {
            return Err(format!(
                "recursão de $ref sem avançar no dado: {}",
                mostrar()
            ));
        }
        if self.voltas.iter().any(|(_, o)| cruzam(o, &volta)) {
            return Err(format!(
                "recursão com mais de um caminho pelo mesmo dado em {} (custo exponencial)",
                mostrar()
            ));
        }
        if self.voltas.len() >= MAX_VOLTAS {
            return Err(format!("recursão com mais de {MAX_VOLTAS} voltas abertas"));
        }
        self.voltas.push((id, volta));
        Ok(())
    }
}

/// Resolve `#` ou `#/ponteiro` (RFC 6901, com escape `%XX` de fragmento de URI) na raiz.
fn resolver<'a>(raiz: &'a serde_json::Value, r: &str) -> Result<&'a serde_json::Value, String> {
    let nada = || {
        format!(
            "$ref não encontrado: {}",
            crate::exportar::aspas(&r.chars().take(120).collect::<String>())
        )
    };
    let frag = r.strip_prefix('#').unwrap_or(r);
    if frag.is_empty() {
        return Ok(raiz);
    }
    if !frag.starts_with('/') {
        return Err(format!(
            "$ref para âncora não é aceito: {}",
            crate::exportar::aspas(&r.chars().take(120).collect::<String>())
        ));
    }
    let b = frag.as_bytes();
    let mut bytes = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = b
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok());
            bytes.push(
                h.and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or_else(nada)?,
            );
            i += 3;
        } else {
            bytes.push(b[i]);
            i += 1;
        }
    }
    let ponteiro = String::from_utf8(bytes).map_err(|_| nada())?;
    let mut v = raiz;
    for seg in ponteiro.split('/').skip(1) {
        let seg = seg.replace("~1", "/").replace("~0", "~");
        v = match v {
            serde_json::Value::Object(m) => m.get(&seg),
            serde_json::Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
            _ => None,
        }
        .ok_or_else(nada)?;
    }
    Ok(v)
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
    let v = compilar(schema)?;
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

#[cfg(test)]
mod testes {
    use super::*;

    fn motivo_de(schema: &str, dado: &str) -> String {
        let v = compilar(schema).unwrap();
        let j: serde_json::Value = serde_json::from_str(dado).unwrap();
        motivo(&v.validate(&j).unwrap_err())
    }

    #[test]
    fn validar_motivo_vira_codigo_sem_valor_do_arquivo() {
        let casos = [
            (
                r#"{"type":"integer"}"#,
                r#""SEGREDO""#,
                "jasao:schema_tipo||integer",
            ),
            (
                r#"{"type":["integer","null"]}"#,
                r#""SEGREDO""#,
                "jasao:schema_tipo||null,integer",
            ),
            (
                r#"{"required":["a"]}"#,
                r#"{"b":1}"#,
                "jasao:schema_obrigatoria||a",
            ),
            (
                r#"{"enum":["x","y"]}"#,
                r#""SEGREDO""#,
                r#"jasao:schema_enum||["x","y"]"#,
            ),
            (
                r#"{"const":"x"}"#,
                r#""SEGREDO""#,
                r#"jasao:schema_const||"x""#,
            ),
            (r#"{"minimum":5}"#, "1", "jasao:schema_minimo||5"),
            (r#"{"maximum":5}"#, "99", "jasao:schema_maximo||5"),
            (
                r#"{"exclusiveMinimum":5}"#,
                "5",
                "jasao:schema_minimo_exclusivo||5",
            ),
            (
                r#"{"exclusiveMaximum":5}"#,
                "5",
                "jasao:schema_maximo_exclusivo||5",
            ),
            (
                r#"{"minLength":9}"#,
                r#""SEGREDO""#,
                "jasao:schema_tamanho_minimo||9",
            ),
            (
                r#"{"maxLength":2}"#,
                r#""SEGREDO""#,
                "jasao:schema_tamanho_maximo||2",
            ),
            (
                r#"{"pattern":"^a|b$"}"#,
                r#""SEGREDO""#,
                "jasao:schema_pattern||^a%7Cb$",
            ),
            (
                r#"{"properties":{"a%7C":{"type":"string"}}}"#,
                r#"{"a%7C":1}"#,
                "jasao:schema_tipo|/a%257C|string",
            ),
            (
                r#"{"properties":{"a|%":{"type":"string"}}}"#,
                r#"{"a|%":1}"#,
                "jasao:schema_tipo|/a%7C%25|string",
            ),
            (
                r#"{"properties":{"a":{}},"additionalProperties":false}"#,
                r#"{"x":"SEGREDO"}"#,
                "jasao:schema_propriedade_extra||x",
            ),
        ];
        for (s, d, esperado) in casos {
            let m = motivo_de(s, d);
            assert_eq!(m, esperado, "{s}");
            assert!(!m.contains("SEGREDO"), "{m}");
        }
    }

    #[test]
    fn validar_motivo_tipo_sem_codigo_fica_mascarado() {
        // `format` não é aplicado pelo validador (só anotação): o braço `Format` é defensivo.
        let m = motivo_de(r#"{"minItems":3}"#, r#"["SEGREDO"]"#);
        assert!(!m.starts_with("jasao:") && !m.contains("SEGREDO"), "{m}");
    }

    #[test]
    fn validar_motivo_legivel_em_pt() {
        let l = |s, d| motivo_legivel(&motivo_de(s, d));
        assert_eq!(
            l(
                r#"{"properties":{"a":{"type":"integer"}}}"#,
                r#"{"a":"SEGREDO"}"#
            ),
            "/a: deve ser do tipo inteiro"
        );
        assert_eq!(
            l(r#"{"type":["integer","null"]}"#, r#""x""#),
            "deve ser do tipo nulo ou inteiro"
        );
        assert_eq!(
            l(r#"{"required":["c"]}"#, "{}"),
            "falta a propriedade obrigatória \"c\""
        );
        assert_eq!(
            l(
                r#"{"properties":{"a%7C":{"type":"string"}}}"#,
                r#"{"a%7C":1}"#
            ),
            "/a%7C: deve ser do tipo texto"
        );
        assert_eq!(motivo_legivel("jasao:valor"), "valor");
        assert_eq!(motivo_legivel("jasao:schema_novo|/a|x"), "schema_novo|/a|x");
        assert_eq!(motivo_legivel("/a: texto"), "/a: texto");
    }
}
