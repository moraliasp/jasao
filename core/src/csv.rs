//! Resultado da consulta jq (JSONL, um valor por linha) em CSV.
//!
//! Regra de achatamento (ver `docs/decisoes.md`):
//! - Cada folha vira uma coluna com o caminho jq relativo ao resultado (`.a.b`, `.itens[0]`,
//!   `.["com espaço"]`); um resultado que não é objeto nem lista vai na coluna `.`.
//! - Objeto ou lista vazios são folhas: `{}` e `[]`.
//! - Colunas na ordem em que aparecem pela primeira vez (ordem de chave do próprio resultado).
//! - Célula: `null` vazia; `true`/`false`; número exato como veio (inteiro grande não perde
//!   dígito); texto cru. Caminho ausente no resultado: vazia.
//! - Injeção de fórmula: texto (e nome de coluna) que começa com `=`, `+`, `-`, `@`, tab, CR ou
//!   as formas de largura total `＝ ＋ － ＠` ganha um `'` na frente, também quando antes dele só
//!   há espaço, NUL ou BOM (U+FEFF), que a planilha pode ignorar. Número não (`-5` é número).
//! - UTF-8 sem BOM; resultado vazio é arquivo vazio (sem linha de cabeçalho).
//! - Aspas RFC 4180 quando a célula tem `,`, `"`, CR ou LF; linhas terminam em CRLF.
//!
//! São duas passadas: `ver` em todos os resultados junta as colunas, `escrever` grava. Memória:
//! só os nomes das colunas, com teto `MAX_COLUNAS`.

use std::collections::HashMap;
use std::io::{self, Write};

use jaq_json::Val;

use crate::caminho::{Caminho, Segmento};

/// Aninhamento máximo de um resultado: o achatamento é recursivo e roda na pilha de quem chama.
pub const MAX_ANINHAMENTO: usize = 256;
/// Teto de colunas: um resultado hostil com um milhão de chaves não vira cabeçalho de 1 milhão.
pub const MAX_COLUNAS: usize = 10_000;

#[derive(Debug, Default)]
pub struct Csv {
    colunas: Vec<String>,
    indice: HashMap<String, usize>,
}

impl Csv {
    /// 1ª passada: junta as colunas de um resultado. Erro se não for JSON ou passar do teto.
    pub fn ver(&mut self, json: &[u8]) -> Result<(), String> {
        let v = ler(json)?;
        let mut erro = None;
        folhas(&v, &mut Caminho::raiz(), &mut |c, _| {
            if erro.is_some() || self.indice.contains_key(&c) {
                return;
            }
            if self.colunas.len() >= MAX_COLUNAS {
                erro = Some(format!("o resultado passa de {MAX_COLUNAS} colunas"));
                return;
            }
            self.indice.insert(c.clone(), self.colunas.len());
            self.colunas.push(c);
        });
        erro.map_or(Ok(()), Err)
    }

    pub fn colunas(&self) -> &[String] {
        &self.colunas
    }

    /// Sem colunas (nenhum resultado), não escreve nada: arquivo vazio, não uma linha vazia.
    pub fn cabecalho(&self, out: &mut impl Write) -> io::Result<()> {
        if self.colunas.is_empty() {
            return Ok(());
        }
        linha(out, self.colunas.iter().map(|c| celula_texto(c)))
    }

    /// 2ª passada: uma linha do CSV. Coluna que não foi vista em `ver` é erro.
    pub fn escrever(&self, json: &[u8], out: &mut impl Write) -> io::Result<()> {
        let v = ler(json).map_err(io::Error::other)?;
        let mut celulas = vec![String::new(); self.colunas.len()];
        let mut faltou = false;
        folhas(
            &v,
            &mut Caminho::raiz(),
            &mut |c, folha| match self.indice.get(&c) {
                Some(&i) => celulas[i] = celula(folha),
                None => faltou = true,
            },
        );
        if faltou {
            return Err(io::Error::other("coluna que não estava na 1ª passada"));
        }
        linha(out, celulas.into_iter())
    }
}

fn ler(json: &[u8]) -> Result<Val, String> {
    if crate::consulta::aninhamento_excede(json, MAX_ANINHAMENTO) {
        return Err(format!(
            "resultado com mais de {MAX_ANINHAMENTO} níveis não vira CSV"
        ));
    }
    jaq_json::read::parse_single(json).map_err(|e| format!("JSON inválido: {e}"))
}

/// Chama `f` com o caminho jq de cada folha.
fn folhas(v: &Val, c: &mut Caminho, f: &mut dyn FnMut(String, &Val)) {
    match v {
        Val::Arr(a) if !a.is_empty() => {
            for (i, x) in a.iter().enumerate() {
                c.0.push(Segmento::Indice(i));
                folhas(x, c, f);
                c.0.pop();
            }
        }
        Val::Obj(o) if !o.is_empty() => {
            for (k, x) in o.iter() {
                c.0.push(Segmento::Chave(texto(k)));
                folhas(x, c, f);
                c.0.pop();
            }
        }
        _ => f(c.jq(), v),
    }
}

fn texto(v: &Val) -> String {
    match v {
        Val::TStr(b) | Val::BStr(b) => String::from_utf8_lossy(b).into_owned(),
        v => v.to_string(),
    }
}

fn celula(v: &Val) -> String {
    match v {
        Val::Null => String::new(),
        Val::TStr(_) | Val::BStr(_) => celula_texto(&texto(v)),
        v => v.to_string(), // número, booleano, `{}` e `[]`
    }
}

/// Texto que uma planilha leria como fórmula ganha `'` na frente.
pub fn celula_texto(s: &str) -> String {
    const PERIGO: [char; 10] = ['=', '+', '-', '@', '\t', '\r', '＝', '＋', '－', '＠'];
    let perigo = |t: &str| t.chars().next().is_some_and(|c| PERIGO.contains(&c));
    // Espaço, NUL e BOM na frente podem ser ignorados pela planilha antes de ler a fórmula.
    let util = s.trim_start_matches(|c: char| c.is_whitespace() || c == '\0' || c == '\u{feff}');
    if perigo(s) || perigo(util) {
        format!("'{s}")
    } else {
        s.to_owned()
    }
}

fn linha(out: &mut impl Write, celulas: impl Iterator<Item = String>) -> io::Result<()> {
    for (i, c) in celulas.enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        if c.contains([',', '"', '\r', '\n']) {
            write!(out, "\"{}\"", c.replace('"', "\"\""))?;
        } else {
            out.write_all(c.as_bytes())?;
        }
    }
    out.write_all(b"\r\n")
}

/// CSV inteiro de uma lista de resultados (testes e CLI com o resultado em memória).
pub fn de_jsonl<'a>(
    jsons: impl Iterator<Item = &'a [u8]> + Clone,
    out: &mut impl Write,
) -> Result<(), String> {
    let mut csv = Csv::default();
    for j in jsons.clone() {
        csv.ver(j)?;
    }
    let e = |e: io::Error| e.to_string();
    csv.cabecalho(out).map_err(e)?;
    for j in jsons {
        csv.escrever(j, out).map_err(e)?;
    }
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;

    fn csv(linhas: &[&str]) -> String {
        let mut out = Vec::new();
        de_jsonl(linhas.iter().map(|l| l.as_bytes()), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn achata_por_caminho_jq() {
        assert_eq!(
            csv(&[
                r#"{"id":1,"a":{"b":true,"c":[10,{"d":null}]},"x y":"s"}"#,
                r#"{"id":2,"novo":{},"l":[]}"#,
            ]),
            ".id,.a.b,.a.c[0],.a.c[1].d,\".[\"\"x y\"\"]\",.novo,.l\r\n\
             1,true,10,,s,,\r\n\
             2,,,,,{},[]\r\n"
        );
    }

    #[test]
    fn escalar_e_numero_exato() {
        assert_eq!(
            csv(&["123456789012345678901234567890", "1.50", "\"a,b\"", "null"]),
            ".\r\n123456789012345678901234567890\r\n1.50\r\n\"a,b\"\r\n\r\n"
        );
    }

    #[test]
    fn injecao_de_formula() {
        let s = csv(&[
            r#"{"=k":"=1+1","b":"+1","c":"-1","d":"@SUM(A1)","e":"＠x","f":"＝1","g":"\tx","h":-5,"i":"ok"}"#,
        ]);
        let mut l = s.split("\r\n");
        let cab = l.next().unwrap();
        assert!(cab.starts_with(r#"".[""=k""]""#), "{cab}");
        assert_eq!(
            l.next().unwrap(),
            "'=1+1,'+1,'-1,'@SUM(A1),'＠x,'＝1,'\tx,-5,ok"
        );
        assert_eq!(celula_texto("-"), "'-");
        assert_eq!(celula_texto("\rx"), "'\rx");
        for s in [" =1", "\0=1", "\u{feff}=1", " \u{a0}+1", "\n@x"] {
            assert_eq!(celula_texto(s), format!("'{s}"), "{s:?}");
        }
        assert_eq!(celula_texto(" ok"), " ok");
        assert_eq!(celula_texto("\u{feff}x"), "\u{feff}x");
    }

    #[test]
    fn vazio_e_arquivo_vazio() {
        assert_eq!(csv(&[]), "");
    }

    #[test]
    fn aspas_e_quebra() {
        assert_eq!(
            csv(&[r#"{"a":"diz \"oi\"\nfim"}"#]),
            ".a\r\n\"diz \"\"oi\"\"\nfim\"\r\n"
        );
    }

    #[test]
    fn teto_de_colunas() {
        let chaves: Vec<String> = (0..=MAX_COLUNAS).map(|i| format!("\"k{i}\":0")).collect();
        let linha = format!("{{{}}}", chaves.join(","));
        let mut c = Csv::default();
        let e = c.ver(linha.as_bytes()).unwrap_err();
        assert!(e.contains("colunas"), "{e}");
        assert_eq!(c.colunas().len(), MAX_COLUNAS);
        // No teto exato ainda passa.
        let linha = format!("{{{}}}", chaves[..MAX_COLUNAS].join(","));
        assert!(Csv::default().ver(linha.as_bytes()).is_ok());
    }

    #[test]
    fn teto_de_aninhamento() {
        let fundo = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(
            Csv::default()
                .ver(fundo(MAX_ANINHAMENTO).as_bytes())
                .is_ok()
        );
        let e = Csv::default()
            .ver(fundo(MAX_ANINHAMENTO + 1).as_bytes())
            .unwrap_err();
        assert!(e.contains("níveis"), "{e}");
    }

    #[test]
    fn json_invalido_e_erro() {
        assert!(Csv::default().ver(b"{").is_err());
    }
}
