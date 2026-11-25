//! Linhas inválidas de um JSONL: varredura do arquivo inteiro que diz onde e por que cada linha
//! não é JSON.
//!
//! - Linhas nas regras de `linhas.rs` (BOM fora, `\r` de CRLF fora). Linha vazia ou só com
//!   espaço em branco do JSON (espaço, tab, `\r`) não é erro: conta à parte.
//! - Validador próprio, sem montar o valor e sem recursão (pilha explícita): passar de
//!   `estrutura::PROFUNDIDADE_MAX` níveis é erro (o mesmo corte da Estrutura, do card e do
//!   Exportar) e a memória é a da pilha de `{`/`[` abertos. Segue a gramática da RFC 8259;
//!   chave duplicada e `\ud800` solto passam (são sintaxe válida; o app mostra como estão).
//! - Primeiro problema de cada linha, com a coluna em bytes. UTF-8 inválido conta se vier antes
//!   do primeiro erro de sintaxe (ou se a sintaxe estiver boa).
//! - Mesmo motor paralelo da busca (`busca::em_blocos`): blocos por trabalhador, saída em ordem
//!   de arquivo, progresso e cancelamento por `AtomicBool`.

use std::fmt;
use std::sync::atomic::AtomicBool;

use memchr::memchr;

use crate::busca::{BLOCO_PADRAO, em_blocos};

/// Por que a linha não é JSON. `fecho` é o `}` ou `]` em jogo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motivo {
    VirgulaSobrando { fecho: char },
    FaltaVirgula { fecho: char },
    FaltaValor { antes: char },
    FaltaDoisPontos,
    ChaveSemAspas,
    AspasSimples,
    ValorInvalido,
    NumeroInvalido,
    TextoSemFechamento,
    EscapeInvalido,
    ControleEmTexto,
    ConteudoDepois,
    Utf8Invalido,
    Truncada,
    Aninhamento,
}

impl Motivo {
    /// Código estável para máquina (saída `--json`, filtros).
    pub fn codigo(self) -> &'static str {
        match self {
            Self::VirgulaSobrando { .. } => "virgula_sobrando",
            Self::FaltaVirgula { .. } => "falta_virgula",
            Self::FaltaValor { .. } => "falta_valor",
            Self::FaltaDoisPontos => "falta_dois_pontos",
            Self::ChaveSemAspas => "chave_sem_aspas",
            Self::AspasSimples => "aspas_simples",
            Self::ValorInvalido => "valor_invalido",
            Self::NumeroInvalido => "numero_invalido",
            Self::TextoSemFechamento => "texto_sem_fechamento",
            Self::EscapeInvalido => "escape_invalido",
            Self::ControleEmTexto => "controle_em_texto",
            Self::ConteudoDepois => "conteudo_depois",
            Self::Utf8Invalido => "utf8_invalido",
            Self::Truncada => "linha_truncada",
            Self::Aninhamento => "aninhamento",
        }
    }
}

impl fmt::Display for Motivo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VirgulaSobrando { fecho } => write!(f, "vírgula sobrando antes de `{fecho}`"),
            Self::FaltaVirgula { fecho } => write!(f, "falta `,` ou `{fecho}` depois do valor"),
            Self::FaltaValor { antes } => write!(f, "falta um valor antes de `{antes}`"),
            Self::FaltaDoisPontos => write!(f, "falta `:` depois da chave"),
            Self::ChaveSemAspas => write!(f, "chave sem aspas (em JSON a chave vai entre \"…\")"),
            Self::AspasSimples => write!(f, "aspas simples: em JSON texto usa aspas duplas"),
            Self::ValorInvalido => write!(
                f,
                "valor inválido: esperava objeto, lista, texto, número, true, false ou null"
            ),
            Self::NumeroInvalido => write!(f, "número malformado"),
            Self::TextoSemFechamento => write!(f, "texto sem aspas de fechamento"),
            Self::EscapeInvalido => write!(f, "escape inválido dentro de texto"),
            Self::ControleEmTexto => {
                write!(f, "caractere de controle sem escape dentro de texto")
            }
            Self::ConteudoDepois => write!(
                f,
                "conteúdo depois do fim do valor (dois valores na mesma linha?)"
            ),
            Self::Utf8Invalido => write!(f, "UTF-8 inválido"),
            Self::Truncada => write!(f, "linha truncada: termina antes de fechar o valor"),
            Self::Aninhamento => write!(
                f,
                "aninhamento além de {} níveis",
                crate::estrutura::PROFUNDIDADE_MAX
            ),
        }
    }
}

/// Uma linha inválida. `linha` a partir de 0 (como em `ArquivoLinhas::janela`); `deslocamento`
/// é o byte do início da linha no arquivo; `coluna` é o byte do erro dentro da linha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinhaInvalida {
    pub linha: usize,
    pub deslocamento: usize,
    pub coluna: usize,
    pub motivo: Motivo,
}

/// Entregue durante a varredura: as linhas inválidas novas (dentro do teto), em ordem, e o
/// progresso.
#[derive(Debug, Default)]
pub struct LoteErros {
    pub erros: Vec<LinhaInvalida>,
    pub varridos: usize,
    pub total: usize,
}

/// Fim da varredura. As contagens são exatas mesmo quando o teto cortou o que foi guardado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumoErros {
    pub linhas: u64,
    pub invalidas: u64,
    pub em_branco: u64,
    pub varridos: usize,
    pub total: usize,
    pub cancelado: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct OpcoesErros {
    /// Quantas linhas inválidas entregar, no máximo; a contagem segue até o fim.
    pub max_guardados: usize,
    /// Tamanho-alvo do bloco (testes e fuzz usam blocos minúsculos).
    pub bloco: usize,
}

impl Default for OpcoesErros {
    fn default() -> Self {
        Self {
            max_guardados: 10_000,
            bloco: BLOCO_PADRAO,
        }
    }
}

#[derive(Default)]
struct Bloco {
    erros: Vec<LinhaInvalida>,
    linhas: u64,
    invalidas: u64,
    em_branco: u64,
}

/// Varre `dados` inteiro. `ao_lote` roda na thread do chamador, em ordem de arquivo: a cada lote
/// com erros novos e, sem erros, a cada ~1% varrido e no fim. Ligar `cancelar` para antes do
/// próximo bloco; depois disso nenhum lote sai.
pub fn varrer(
    dados: &[u8],
    op: &OpcoesErros,
    cancelar: &AtomicBool,
    mut ao_lote: impl FnMut(LoteErros),
) -> ResumoErros {
    let total = dados.len();
    let max = op.max_guardados;
    let mut r = ResumoErros {
        linhas: 0,
        invalidas: 0,
        em_branco: 0,
        varridos: 0,
        total,
        cancelado: false,
    };
    let (mut ultimo_aviso, mut guardados) = (0, 0);
    let (varridos, cancelado) = em_blocos(
        dados,
        op.bloco.max(1),
        cancelar,
        |b| {
            // Endereço do bloco → deslocamento no arquivo, sem passar a faixa ao trabalhador.
            let ini = b.as_ptr() as usize - dados.as_ptr() as usize;
            varrer_bloco(b, ini, max)
        },
        |mut b, faixa, fim| {
            for e in &mut b.erros {
                e.linha += r.linhas as usize;
            }
            b.erros.truncate(max - guardados);
            guardados += b.erros.len();
            r.linhas += b.linhas;
            r.invalidas += b.invalidas;
            r.em_branco += b.em_branco;
            if !b.erros.is_empty() || fim || faixa.end - ultimo_aviso >= total / 100 {
                ultimo_aviso = faixa.end;
                ao_lote(LoteErros {
                    erros: b.erros,
                    varridos: faixa.end,
                    total,
                });
            }
        },
    );
    r.varridos = varridos;
    r.cancelado = cancelado;
    r
}

/// Um bloco que começa no início de uma linha e termina logo depois de um `\n` (ou no fim do
/// arquivo). Guarda até `max` erros; conta todos. Linha relativa ao bloco.
fn varrer_bloco(b: &[u8], ini: usize, max: usize) -> Bloco {
    let mut out = Bloco::default();
    let mut pilha = Vec::new();
    let mut pos = 0;
    while pos < b.len() {
        let (mut fim, prox) = match memchr(b'\n', &b[pos..]) {
            Some(p) => (pos + p, pos + p + 1),
            None => (b.len(), b.len()),
        };
        // `\r` de CRLF não é da linha; `\r` final sem `\n` é dado (aqui, espaço em branco).
        if prox > fim && fim > pos && b[fim - 1] == b'\r' {
            fim -= 1;
        }
        match validar_linha(&b[pos..fim], &mut pilha) {
            Ok(true) => {}
            Ok(false) => out.em_branco += 1,
            Err((coluna, motivo)) => {
                out.invalidas += 1;
                if out.erros.len() < max {
                    out.erros.push(LinhaInvalida {
                        linha: out.linhas as usize,
                        deslocamento: ini + pos,
                        coluna,
                        motivo,
                    });
                }
            }
        }
        out.linhas += 1;
        pos = prox;
    }
    out
}

fn espaco(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// `Ok(true)` se a linha é um valor JSON, `Ok(false)` se está em branco, `Err((coluna, motivo))`
/// no primeiro problema. `pilha` é só memória reaproveitada entre linhas.
pub fn validar_linha(l: &[u8], pilha: &mut Vec<u8>) -> Result<bool, (usize, Motivo)> {
    let sintaxe = sintaxe(l, pilha);
    match (std::str::from_utf8(l), sintaxe) {
        (Err(u), Err((p, _))) if u.valid_up_to() <= p => {
            Err((u.valid_up_to(), Motivo::Utf8Invalido))
        }
        (Err(u), Ok(_)) => Err((u.valid_up_to(), Motivo::Utf8Invalido)),
        (_, s) => s,
    }
}

fn sintaxe(l: &[u8], pilha: &mut Vec<u8>) -> Result<bool, (usize, Motivo)> {
    use Motivo::*;
    pilha.clear();
    let n = l.len();
    let pular = |i: &mut usize| {
        while *i < n && espaco(l[*i]) {
            *i += 1;
        }
    };
    let mut i = 0;
    pular(&mut i);
    if i == n {
        return Ok(false);
    }
    // Chave de objeto em `i` (já sem espaço), até depois do `:` e do espaço seguinte.
    let chave = |i: &mut usize| -> Result<(), (usize, Motivo)> {
        match l.get(*i) {
            None => return Err((*i, Truncada)),
            Some(b'"') => *i = texto(l, *i)?,
            Some(b'\'') => return Err((*i, AspasSimples)),
            Some(b',') => return Err((*i, FaltaValor { antes: ',' })),
            Some(_) => return Err((*i, ChaveSemAspas)),
        }
        while *i < n && espaco(l[*i]) {
            *i += 1;
        }
        match l.get(*i) {
            None => Err((*i, Truncada)),
            Some(b':') => {
                *i += 1;
                Ok(())
            }
            Some(_) => Err((*i, FaltaDoisPontos)),
        }
    };
    'valor: loop {
        pular(&mut i);
        let Some(&c) = l.get(i) else {
            return Err((i, Truncada));
        };
        match c {
            b'{' | b'[' => {
                if pilha.len() >= crate::estrutura::PROFUNDIDADE_MAX {
                    return Err((i, Aninhamento));
                }
                let fecho = if c == b'{' { b'}' } else { b']' };
                i += 1;
                pular(&mut i);
                if l.get(i) == Some(&fecho) {
                    i += 1;
                } else {
                    pilha.push(c);
                    if c == b'{' {
                        chave(&mut i)?;
                    }
                    continue 'valor;
                }
            }
            b'"' => i = texto(l, i)?,
            b'-' | b'0'..=b'9' => i = numero(l, i)?,
            b't' | b'f' | b'n' => {
                let palavra: &[u8] = match c {
                    b't' => b"true",
                    b'f' => b"false",
                    _ => b"null",
                };
                let resto = &l[i..];
                if resto.starts_with(palavra) {
                    i += palavra.len();
                } else if palavra.starts_with(resto) {
                    return Err((n, Truncada));
                } else {
                    return Err((i, ValorInvalido));
                }
            }
            b'\'' => return Err((i, AspasSimples)),
            b'}' | b']' | b',' => return Err((i, FaltaValor { antes: c as char })),
            _ => return Err((i, ValorInvalido)),
        }
        // Depois de um valor: fecha o que puder e decide o próximo passo.
        loop {
            pular(&mut i);
            let Some(&aberto) = pilha.last() else {
                return if i == n {
                    Ok(true)
                } else {
                    Err((i, ConteudoDepois))
                };
            };
            let fecho = if aberto == b'{' { b'}' } else { b']' };
            match l.get(i) {
                None => return Err((i, Truncada)),
                Some(&b',') => {
                    i += 1;
                    pular(&mut i);
                    match l.get(i) {
                        Some(&x) if x == fecho => {
                            return Err((i, VirgulaSobrando { fecho: x as char }));
                        }
                        _ if aberto == b'{' => chave(&mut i)?,
                        _ => {}
                    }
                    continue 'valor;
                }
                Some(&x) if x == fecho => {
                    pilha.pop();
                    i += 1;
                }
                Some(_) => {
                    return Err((
                        i,
                        FaltaVirgula {
                            fecho: fecho as char,
                        },
                    ));
                }
            }
        }
    }
}

/// Texto a partir das aspas em `i`; devolve a posição depois das aspas de fechamento.
fn texto(l: &[u8], mut i: usize) -> Result<usize, (usize, Motivo)> {
    i += 1;
    loop {
        // Atalho até o próximo byte que interessa: aspas, barra ou controle.
        while i < l.len() && l[i] >= 0x20 && l[i] != b'"' && l[i] != b'\\' {
            i += 1;
        }
        match l.get(i) {
            None => return Err((i, Motivo::TextoSemFechamento)),
            Some(b'"') => return Ok(i + 1),
            Some(b'\\') => {
                match l.get(i + 1) {
                    None => return Err((i + 1, Motivo::TextoSemFechamento)),
                    Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => i += 2,
                    Some(b'u') => {
                        let hex = l.get(i + 2..i + 6);
                        if hex.is_none_or(|h| !h.iter().all(u8::is_ascii_hexdigit)) {
                            // Fim da linha no meio do `\uXXXX` é texto sem fechamento.
                            let ok = l[(i + 2).min(l.len())..].iter().all(u8::is_ascii_hexdigit);
                            return Err(if hex.is_none() && ok {
                                (l.len(), Motivo::TextoSemFechamento)
                            } else {
                                (i, Motivo::EscapeInvalido)
                            });
                        }
                        i += 6;
                    }
                    Some(_) => return Err((i, Motivo::EscapeInvalido)),
                }
            }
            Some(_) => return Err((i, Motivo::ControleEmTexto)),
        }
    }
}

/// Número a partir de `i`: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
fn numero(l: &[u8], mut i: usize) -> Result<usize, (usize, Motivo)> {
    let digitos = |i: &mut usize| -> Result<(), (usize, Motivo)> {
        let ini = *i;
        while *i < l.len() && l[*i].is_ascii_digit() {
            *i += 1;
        }
        match *i == ini {
            false => Ok(()),
            true if *i == l.len() => Err((*i, Motivo::Truncada)),
            true => Err((*i, Motivo::NumeroInvalido)),
        }
    };
    if l[i] == b'-' {
        i += 1;
    }
    if l.get(i) == Some(&b'0') {
        i += 1;
        if l.get(i).is_some_and(u8::is_ascii_digit) {
            return Err((i, Motivo::NumeroInvalido));
        }
    } else {
        digitos(&mut i)?;
    }
    if l.get(i) == Some(&b'.') {
        i += 1;
        digitos(&mut i)?;
    }
    if matches!(l.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(l.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        digitos(&mut i)?;
    }
    Ok(i)
}

#[cfg(test)]
mod testes {
    use super::*;
    use Motivo::*;
    use std::sync::atomic::Ordering::Relaxed;

    fn v(l: &str) -> Result<bool, (usize, Motivo)> {
        validar_linha(l.as_bytes(), &mut Vec::new())
    }

    #[test]
    fn linhas_boas() {
        for l in [
            "{}",
            "[]",
            " { } ",
            r#"{"a":1,"b":[true,false,null],"c":{"d":"e"}}"#,
            r#"{"a":1,"a":2}"#,
            r#""só texto""#,
            "0",
            "-0.5e+10",
            "1E-2",
            "123456789012345678901234567890",
            r#"["\"\\\/\b\f\n\r\tç\ud800"]"#,
            "\t[1 , 2 ]\r",
            "{\"ação\":\"名前😀\"}",
        ] {
            assert_eq!(v(l), Ok(true), "{l}");
        }
    }

    #[test]
    fn corta_no_mesmo_nivel_da_inferencia() {
        let max = crate::estrutura::PROFUNDIDADE_MAX;
        for n in [max - 1, max, max + 1, max + 50, 200_000] {
            // Só `[` (vazio no fundo), e `{"a":` e `[` alternados (valor no fundo).
            let listas = format!("{}{}", "[".repeat(n), "]".repeat(n));
            let mut misto = String::new();
            for k in 0..n {
                misto.push_str(if k % 2 == 0 { "{\"a\":" } else { "[" });
            }
            misto.push('1');
            for k in (0..n).rev() {
                misto.push(if k % 2 == 0 { '}' } else { ']' });
            }
            for l in [listas, misto] {
                let corte = crate::arvore::passa_da_profundidade(l.as_bytes(), max);
                match v(&l) {
                    Err((c, Aninhamento)) => assert_eq!(Some(c), corte, "{n} níveis"),
                    r => assert!(r == Ok(true) && corte.is_none(), "{n} níveis: {r:?}"),
                }
            }
        }
    }

    #[test]
    fn em_branco() {
        for l in ["", " ", "\t \r"] {
            assert_eq!(v(l), Ok(false), "{l:?}");
        }
    }

    #[test]
    fn cada_motivo() {
        let casos: &[(&str, usize, Motivo)] = &[
            (r#"{"a":1,}"#, 7, VirgulaSobrando { fecho: '}' }),
            ("[1,2, ]", 6, VirgulaSobrando { fecho: ']' }),
            (r#"{"a":1 "b":2}"#, 7, FaltaVirgula { fecho: '}' }),
            ("[1 2]", 3, FaltaVirgula { fecho: ']' }),
            (r#"{"a":}"#, 5, FaltaValor { antes: '}' }),
            ("[1,,2]", 3, FaltaValor { antes: ',' }),
            ("{,}", 1, FaltaValor { antes: ',' }),
            (r#"{"a" 1}"#, 5, FaltaDoisPontos),
            ("{a:1}", 1, ChaveSemAspas),
            ("{'a':1}", 1, AspasSimples),
            ("['a']", 1, AspasSimples),
            ("[undefined]", 1, ValorInvalido),
            ("NaN", 0, ValorInvalido),
            ("True", 0, ValorInvalido),
            ("[nul]", 1, ValorInvalido),
            ("// comentário", 0, ValorInvalido),
            ("01", 1, NumeroInvalido),
            ("[1.]", 3, NumeroInvalido),
            ("[-x]", 2, NumeroInvalido),
            ("[.5]", 1, ValorInvalido),
            ("+1", 0, ValorInvalido),
            (r#"{"a":"sem fim}"#, 14, TextoSemFechamento),
            (r#"["a\"]"#, 6, TextoSemFechamento),
            (r#"["\x"]"#, 2, EscapeInvalido),
            (r#"["\u12G4"]"#, 2, EscapeInvalido),
            ("[\"a\tb\"]", 3, ControleEmTexto),
            ("[\"a\rb\"]", 3, ControleEmTexto),
            ("{} {}", 3, ConteudoDepois),
            ("1 2", 2, ConteudoDepois),
            ("{}x", 2, ConteudoDepois),
            (r#"{"a":[1,2"#, 9, Truncada),
            (r#"{"a":"#, 5, Truncada),
            (r#"{"a""#, 4, Truncada),
            (r#"{"a":1,"#, 7, Truncada),
            ("{", 1, Truncada),
            ("[tr", 3, Truncada),
            ("[1.", 3, Truncada),
            ("-", 1, Truncada),
            ("[1e", 3, Truncada),
            (r#"["\u12"#, 6, TextoSemFechamento),
            ("\u{feff}{}", 0, ValorInvalido),
        ];
        for &(l, col, m) in casos {
            assert_eq!(v(l), Err((col, m)), "{l}");
        }
    }

    #[test]
    fn utf8_invalido_conta_pela_posicao() {
        let mut p = Vec::new();
        // Dentro de texto: sintaxe boa, UTF-8 ruim.
        assert_eq!(
            validar_linha(b"[\"a\xff\"]", &mut p),
            Err((3, Utf8Invalido))
        );
        // UTF-8 ruim antes do erro de sintaxe ganha; depois, perde.
        assert_eq!(
            validar_linha(b"[\"\xc3\" 1]", &mut p),
            Err((2, Utf8Invalido))
        );
        assert_eq!(
            validar_linha(b"[1 2, \"\xff\"]", &mut p),
            Err((3, FaltaVirgula { fecho: ']' }))
        );
        // Fora de texto.
        assert_eq!(validar_linha(b"\xff", &mut p), Err((0, Utf8Invalido)));
    }

    #[test]
    fn mensagens_em_portugues() {
        assert_eq!(
            VirgulaSobrando { fecho: '}' }.to_string(),
            "vírgula sobrando antes de `}`"
        );
        assert_eq!(
            TextoSemFechamento.to_string(),
            "texto sem aspas de fechamento"
        );
        assert_eq!(Utf8Invalido.to_string(), "UTF-8 inválido");
        assert!(Truncada.to_string().starts_with("linha truncada"));
    }

    fn varrer_tudo(dados: &[u8], max: usize, bloco: usize) -> (Vec<LinhaInvalida>, ResumoErros) {
        let op = OpcoesErros {
            max_guardados: max,
            bloco,
        };
        let mut v = Vec::new();
        let r = varrer(dados, &op, &AtomicBool::new(false), |l| v.extend(l.erros));
        assert!(!r.cancelado);
        assert_eq!(r.varridos, r.total);
        (v, r)
    }

    fn e(linha: usize, deslocamento: usize, coluna: usize, motivo: Motivo) -> LinhaInvalida {
        LinhaInvalida {
            linha,
            deslocamento,
            coluna,
            motivo,
        }
    }

    #[test]
    fn arquivo_com_crlf_bom_branco_e_ultima_sem_quebra() {
        let dados = b"\xEF\xBB\xBF{\"a\":1}\r\n\r\n[1,]\r\n  \n{\"b\":\"x\"}\n{\"c\":";
        let esperado = vec![
            e(2, 14, 3, VirgulaSobrando { fecho: ']' }),
            e(5, 33, 5, Truncada),
        ];
        for bloco in 1..dados.len() + 2 {
            let (v, r) = varrer_tudo(dados, 100, bloco);
            assert_eq!(v, esperado, "bloco {bloco}");
            assert_eq!((r.linhas, r.invalidas, r.em_branco), (6, 2, 2));
        }
        // `\r` final sem `\n` é dado: aqui, espaço depois do valor.
        let (v, r) = varrer_tudo(b"{}\r", 100, 64);
        assert_eq!((v.len(), r.linhas), (0, 1));
    }

    #[test]
    fn so_linhas_boas_e_vazio() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 1 << 20, 4).unwrap();
        let (v, r) = varrer_tudo(&dados, 100, 4096);
        assert!(v.is_empty());
        assert_eq!(r.invalidas, 0);
        assert_eq!(
            r.linhas as usize,
            crate::IndiceLinhas::construir(&dados).len()
        );

        for vazio in [&b""[..], b"\xEF\xBB\xBF"] {
            let (v, r) = varrer_tudo(vazio, 100, 4096);
            assert!(v.is_empty());
            assert_eq!((r.linhas, r.invalidas, r.em_branco), (0, 0, 0));
        }
    }

    /// Arquivo gerado com uma linha quebrada a cada `passo`.
    fn com_injetados(passo: usize) -> (Vec<u8>, usize) {
        let mut base = Vec::new();
        crate::gerador::gerar(&mut base, 2 << 20, 7).unwrap();
        let mut dados = Vec::new();
        let mut n = 0;
        for (i, l) in base.split_inclusive(|&b| b == b'\n').enumerate() {
            if i % passo == 0 {
                dados.extend_from_slice(&l[..l.len() / 2]);
                dados.push(b'\n');
                n += 1;
            } else {
                dados.extend_from_slice(l);
            }
        }
        (dados, n)
    }

    #[test]
    fn teto_corta_o_guardado_mas_nao_a_contagem() {
        let (dados, injetados) = com_injetados(10);
        let (todos, r) = varrer_tudo(&dados, usize::MAX, 1000);
        assert_eq!(r.invalidas as usize, injetados);
        assert_eq!(todos.len(), injetados);
        // Em ordem e batendo com o índice.
        let ind = crate::IndiceLinhas::construir(&dados);
        for w in todos.windows(2) {
            assert!(w[0].linha < w[1].linha);
        }
        for x in &todos {
            assert_eq!(ind.faixa(x.linha, &dados).unwrap().start, x.deslocamento);
        }
        for max in [0, 1, 7, 500] {
            let (v, r2) = varrer_tudo(&dados, max, 1000);
            assert_eq!(v, todos[..max.min(todos.len())]);
            assert_eq!(r2, r);
        }
    }

    #[test]
    fn cancelar_no_meio() {
        let (dados, _) = com_injetados(3);
        let op = OpcoesErros {
            max_guardados: usize::MAX,
            bloco: 4096,
        };
        let cancelar = AtomicBool::new(false);
        let (mut lotes, mut ultimo) = (0, 0);
        let r = varrer(&dados, &op, &cancelar, |l| {
            assert!(l.varridos > ultimo);
            ultimo = l.varridos;
            lotes += 1;
            if lotes == 3 {
                cancelar.store(true, Relaxed);
            }
        });
        assert!(r.cancelado);
        assert_eq!(lotes, 3);
        assert_eq!(r.varridos, ultimo);
        assert!(r.varridos < r.total);
    }
}
