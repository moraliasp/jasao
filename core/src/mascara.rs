//! Máscara de segredos: acha, sem montar o valor, os escalares que não devem sair do núcleo, e a
//! anonimização (código estável por valor) para exportar. Ver `docs/decisoes.md`, "Máscara de
//! segredos".
//!
//! Mascarado = texto ou número que está abaixo de uma chave sensível (`estrutura::chave_de_segredo`:
//! senha, token, cvv, cpf…, em qualquer ancestral) ou cujo próprio texto parece credencial ou
//! documento (`sensivel`). Booleano e `null` não são mascarados: não carregam segredo. A chave
//! nunca é mascarada (é estrutura).
//!
//! A máscara tem o mesmo comprimento do original e deixa o JSON válido e do mesmo tipo: o conteúdo
//! do texto vira `*` (aspas ficam), os dígitos do número viram `9`. Assim as posições (Texto ×
//! Árvore, colunas da busca, faixas da tabela) não mudam. Linha que o léxico não entende fica
//! mascarada da primeira coisa estranha até o fim: letras, dígitos e bytes não ASCII viram `*`.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::ops::Range;

use crate::arvore::MAX_PROFUNDIDADE;
use crate::estrutura::{chave_de_segredo, parece_segredo, sem_escapes};
use crate::linhas::IndiceLinhas;

/// O que um trecho mascarado era.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tipo {
    /// Conteúdo de texto, sem as aspas.
    Texto,
    /// Número inteiro como escrito.
    Numero,
    /// Resto de linha que o léxico não entendeu.
    Bruto,
}

/// Valor de texto (cru, entre as aspas) que se mascara pelo formato: credencial (`parece_segredo`)
/// ou documento brasileiro / cartão com dígito verificador certo.
/// Escapes do JSON (`\u0034111…`) são resolvidos antes.
pub fn sensivel(v: &[u8]) -> bool {
    let v = sem_escapes(v);
    parece_segredo(&v) || doc_ou_cartao(&v)
}

/// CPF ou CNPJ (com ou sem pontuação) ou cartão de 13 a 19 dígitos (espaço ou hífen) que começa
/// com 3 a 6 (ou 2221 a 2720, Mastercard série 2) e passa no Luhn. Só o dígito verificador separa de um número qualquer: ~1% dos
/// textos de 11 dígitos passa no do CPF (falso positivo é o lado seguro).
fn doc_ou_cartao(v: &[u8]) -> bool {
    if v.len() > 23 || v.len() < 11 {
        return false;
    }
    let mut d = [0u8; 23];
    let mut n = 0;
    for &c in v {
        match c {
            b'0'..=b'9' => {
                d[n] = c - b'0';
                n += 1;
            }
            b'.' | b'-' | b'/' | b' ' => {}
            _ => return false,
        }
    }
    let d = &d[..n];
    if d.iter().all(|&x| x == d[0]) {
        return false;
    }
    let pontuacao_cartao = v
        .iter()
        .all(|c| c.is_ascii_digit() || matches!(c, b' ' | b'-'));
    match n {
        11 => cpf(d),
        14 => cnpj(d) || (pontuacao_cartao && cartao(d)),
        13..=19 => pontuacao_cartao && cartao(d),
        _ => false,
    }
}

fn dv_mod11(d: &[u8], pesos: impl Iterator<Item = u32>) -> u8 {
    let s: u32 = d.iter().zip(pesos).map(|(&x, p)| u32::from(x) * p).sum();
    let r = s % 11;
    if r < 2 { 0 } else { (11 - r) as u8 }
}

fn cpf(d: &[u8]) -> bool {
    dv_mod11(&d[..9], (2..=10).rev()) == d[9] && dv_mod11(&d[..10], (2..=11).rev()) == d[10]
}

fn cnpj(d: &[u8]) -> bool {
    let pesos = |n: usize| {
        (2..=9)
            .cycle()
            .take(n)
            .collect::<Vec<u32>>()
            .into_iter()
            .rev()
    };
    dv_mod11(&d[..12], pesos(12)) == d[12] && dv_mod11(&d[..13], pesos(13)) == d[13]
}

fn cartao(d: &[u8]) -> bool {
    let soma: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            let x = u32::from(x) * if i % 2 == 1 { 2 } else { 1 };
            if x > 9 { x - 9 } else { x }
        })
        .sum();
    let p4 = d[..4].iter().fold(0u32, |n, &x| n * 10 + u32::from(x));
    ((3..=6).contains(&d[0]) || (2221..=2720).contains(&p4)) && soma.is_multiple_of(10)
}

/// Chama `marcar` com cada trecho a mascarar de `dados`, em ordem. JSONL (`por_linha`) é lido
/// linha a linha, cada uma do zero; documento em várias linhas é lido de ponta a ponta
/// (`documento`). Depois de algo estranho, o resto da linha (e, no documento, o resto do arquivo)
/// é `Bruto` se houver sinal de segredo: chave sensível aberta ou palavra suspeita (`suspeito`).
/// Sem sinal fica como está: o erro da linha continua legível.
pub fn trechos(dados: &[u8], indice: &IndiceLinhas, mut marcar: impl FnMut(Range<usize>, Tipo)) {
    let todas = 0..indice.len();
    match por_linha(dados, indice) {
        true => jsonl(dados, indice, todas, &mut marcar),
        false => documento(dados, indice, todas, &mut Estado::default(), &mut marcar),
    }
}

/// JSONL (cada linha lida do zero, ver `trechos`): a 1ª linha não vazia fecha tudo o que abre, ou
/// é inválida. Senão é documento em várias linhas, que só se lê em ordem.
pub fn por_linha(dados: &[u8], indice: &IndiceLinhas) -> bool {
    (0..indice.len())
        .filter_map(|i| indice.faixa(i, dados))
        .find(|f| !dados[f.clone()].iter().all(|c| c.is_ascii_whitespace()))
        .is_none_or(|f| {
            let mut nada = |_, _| {};
            let mut l = Lexico::novo(dados, Estado::default(), &mut nada);
            !l.linha(f.start, f.end) || l.e.pilha.is_empty()
        })
}

fn jsonl(
    dados: &[u8],
    indice: &IndiceLinhas,
    linhas: Range<usize>,
    marcar: &mut impl FnMut(Range<usize>, Tipo),
) {
    let mut l = Lexico::novo(dados, Estado::default(), marcar);
    for f in linhas.filter_map(|i| indice.faixa(i, dados)) {
        l.e.pilha.clear();
        l.e.chave = false;
        l.linha(f.start, f.end);
    }
}

/// Onde a leitura de um documento em várias linhas parou: lido em blocos de linhas, em ordem,
/// cada bloco continua do `Estado` que o anterior deixou.
#[derive(Default)]
pub struct Estado {
    /// Contêineres abertos: (é objeto, está abaixo de chave sensível).
    pilha: Vec<(bool, bool)>,
    /// No objeto do topo: a chave do valor que vem é sensível.
    chave: bool,
    /// `chave_de_segredo` já respondida (aloca por chamada; as mesmas chaves se repetem).
    chaves: HashMap<Box<[u8]>, bool, foldhash::fast::RandomState>,
    /// Documento que se perdeu: `Some(true)` se a chave aberta era sensível (o resto todo sai).
    estragado: Option<bool>,
}

/// As `linhas` de um documento em várias linhas, continuando de `estado` (que fica onde parou).
pub fn documento(
    dados: &[u8],
    indice: &IndiceLinhas,
    linhas: Range<usize>,
    estado: &mut Estado,
    marcar: &mut impl FnMut(Range<usize>, Tipo),
) {
    let mut l = Lexico::novo(dados, std::mem::take(estado), marcar);
    for f in linhas.filter_map(|i| indice.faixa(i, dados)) {
        if let Some(tudo) = l.e.estragado {
            if tudo || suspeito(&dados[f.clone()]) {
                (l.marcar)(f, Tipo::Bruto);
            }
            continue;
        }
        if !l.linha(f.start, f.end) {
            l.e.estragado = Some(l.contexto_sensivel());
        }
    }
    *estado = l.e;
}

/// Mascara as `linhas` em `saida`, o pedaço de uma cópia dos dados que começa no byte `base` e
/// cobre essas linhas; `true` se algo foi mascarado. JSONL sem `estado`; documento com o
/// `Estado` do bloco anterior (ver `documento`).
pub fn mascarar_bloco(
    dados: &[u8],
    indice: &IndiceLinhas,
    linhas: Range<usize>,
    estado: Option<&mut Estado>,
    saida: &mut [u8],
    base: usize,
) -> bool {
    let mut algo = false;
    let mut marcar = |r: Range<usize>, t| {
        algo = true;
        aplicar(saida, r.start - base..r.end - base, t)
    };
    match estado {
        None => jsonl(dados, indice, linhas, &mut marcar),
        Some(e) => documento(dados, indice, linhas, e, &mut marcar),
    }
    algo
}

/// `trechos` aplicados em `saida` (cópia de `dados`, mesmo tamanho); `true` se algo foi mascarado.
pub fn mascarar_em(dados: &[u8], indice: &IndiceLinhas, saida: &mut [u8]) -> bool {
    let mut algo = false;
    trechos(dados, indice, |r, t| {
        algo = true;
        aplicar(saida, r, t)
    });
    algo
}

/// Linha estranha com cara de segredo: algum pedaço (entre aspas, ou palavra entre espaço e
/// pontuação do JSON) é nome de chave sensível ou valor sensível.
fn suspeito(r: &[u8]) -> bool {
    r.split(|&c| c == b'"').any(|seg| {
        sensivel(seg)
            || seg
                .split(|c| c.is_ascii_whitespace() || b",:{}[]".contains(c))
                .any(|p| !p.is_empty() && (chave_de_segredo(p) || sensivel(p)))
    })
}

struct Lexico<'a, F> {
    d: &'a [u8],
    e: Estado,
    marcar: &'a mut F,
}

impl<'a, F: FnMut(Range<usize>, Tipo)> Lexico<'a, F> {
    fn novo(d: &'a [u8], e: Estado, marcar: &'a mut F) -> Self {
        Lexico { d, e, marcar }
    }

    fn valor_sensivel(&self) -> bool {
        match self.e.pilha.last() {
            Some((true, _)) => self.e.chave,
            Some((false, s)) => *s,
            None => false,
        }
    }

    /// Lê `d[i..fim]`; `false` se achou algo estranho (ver `estranho`).
    fn linha(&mut self, mut i: usize, fim: usize) -> bool {
        let d = self.d;
        while i < fim {
            match d[i] {
                b' ' | b'\t' | b'\r' | b'\n' | b':' => i += 1,
                b',' => {
                    self.e.chave = false;
                    i += 1;
                }
                c @ (b'{' | b'[') => {
                    // Teto: abaixo dele a pilha (e a memória) não cresce; o resto é estranho.
                    if self.e.pilha.len() >= MAX_PROFUNDIDADE {
                        return self.estranho(i, fim);
                    }
                    let s = self.valor_sensivel();
                    self.e.pilha.push((c == b'{', s));
                    self.e.chave = false;
                    i += 1;
                }
                c @ (b'}' | b']') => {
                    if self.e.pilha.pop().map(|(obj, _)| obj) != Some(c == b'}') {
                        return self.estranho(i, fim);
                    }
                    i += 1;
                }
                b'"' => {
                    let Some(fecha) = fim_do_texto(&d[i + 1..fim]).map(|p| i + 1 + p) else {
                        return self.estranho(i, fim);
                    };
                    let conteudo = i + 1..fecha;
                    let mut k = fecha + 1;
                    while k < fim && matches!(d[k], b' ' | b'\t' | b'\r' | b'\n') {
                        k += 1;
                    }
                    if k < fim && d[k] == b':' {
                        let Some(&(true, s)) = self.e.pilha.last() else {
                            return self.estranho(i, fim);
                        };
                        self.e.chave = s || self.chave_sensivel(&d[conteudo]);
                    } else if self.valor_sensivel() || sensivel(&d[conteudo.clone()]) {
                        (self.marcar)(conteudo, Tipo::Texto);
                    }
                    i = fecha + 1;
                }
                b'-' | b'0'..=b'9' => {
                    let j = i + d[i..fim]
                        .iter()
                        .position(|c| !matches!(c, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'))
                        .unwrap_or(fim - i);
                    if self.valor_sensivel() {
                        (self.marcar)(i..j, Tipo::Numero);
                    }
                    i = j;
                }
                b't' | b'f' | b'n' => {
                    let j = i + d[i..fim]
                        .iter()
                        .position(|c| !c.is_ascii_alphanumeric())
                        .unwrap_or(fim - i);
                    if !matches!(&d[i..j], b"true" | b"false" | b"null") {
                        return self.estranho(i, fim);
                    }
                    i = j;
                }
                _ => return self.estranho(i, fim),
            }
        }
        true
    }

    fn chave_sensivel(&mut self, k: &[u8]) -> bool {
        if let Some(&s) = self.e.chaves.get(k) {
            return s;
        }
        let s = chave_de_segredo(k);
        if self.e.chaves.len() < 4096 {
            self.e.chaves.insert(k.into(), s);
        }
        s
    }

    /// Algo aberto (chave do topo ou contêiner) está abaixo de chave sensível.
    fn contexto_sensivel(&self) -> bool {
        self.e.chave || self.e.pilha.iter().any(|&(_, s)| s)
    }

    /// Algo estranho em `i`: o resto da linha sai `Bruto` se houver sinal de segredo.
    fn estranho(&mut self, i: usize, fim: usize) -> bool {
        if self.contexto_sensivel() || suspeito(&self.d[i..fim]) {
            (self.marcar)(i..fim, Tipo::Bruto);
        }
        false
    }
}

/// Posição da aspa que fecha o texto em `d` (que começa logo depois da que abre); `None` se a
/// linha acaba antes (ou há quebra de linha no meio).
fn fim_do_texto(d: &[u8]) -> Option<usize> {
    let mut j = 0;
    loop {
        let p = j + memchr::memchr3(b'"', b'\\', b'\n', d.get(j..)?)?;
        match d[p] {
            b'"' => return Some(p),
            b'\\' => j = p + 2,
            _ => return None,
        }
    }
}

/// Aplica a máscara em `saida` (cópia de mesmo tamanho dos dados) no trecho `r`.
pub fn aplicar(saida: &mut [u8], r: Range<usize>, t: Tipo) {
    for c in &mut saida[r] {
        match t {
            Tipo::Texto => *c = b'*',
            Tipo::Numero if c.is_ascii_digit() => *c = b'9',
            Tipo::Bruto if c.is_ascii_alphanumeric() || !c.is_ascii() => *c = b'*',
            _ => {}
        }
    }
}

/// Sal da sessão para `anonimizar`: aleatório por processo (as chaves do `RandomState` do std).
/// Não é segredo forte nem criptografia: ver `docs/decisoes.md`.
pub fn sal_novo() -> u64 {
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// Código estável de um valor dentro da sessão: `anon:` + 16 hex de SipHash(sal, valor cru).
/// Mesmo valor (como escrito no arquivo) dá o mesmo código com o mesmo sal.
pub fn codigo(sal: u64, valor: &[u8]) -> String {
    let mut h = std::hash::DefaultHasher::new();
    h.write_u64(sal);
    h.write(valor);
    format!("anon:{:016x}", h.finish())
}

/// Cópia de `cru` com cada texto ou número que seria mascarado trocado pelo `codigo` dele (o
/// número vira texto `"anon:…"`); linha estranha fica mascarada como na tela. As quebras de linha
/// são as mesmas, mas os comprimentos mudam: só serve para consulta e exportação.
pub fn anonimizar(cru: &[u8], indice: &IndiceLinhas, sal: u64) -> Vec<u8> {
    let mut saida = Vec::with_capacity(cru.len());
    let mut de = 0;
    trechos(cru, indice, |r, t| {
        saida.extend_from_slice(&cru[de..r.start]);
        match t {
            Tipo::Texto => saida.extend_from_slice(codigo(sal, &cru[r.clone()]).as_bytes()),
            Tipo::Numero => {
                saida.push(b'"');
                saida.extend_from_slice(codigo(sal, &cru[r.clone()]).as_bytes());
                saida.push(b'"');
            }
            Tipo::Bruto => {
                let i = saida.len();
                saida.extend_from_slice(&cru[r.clone()]);
                aplicar(&mut saida, i..i + r.len(), t);
            }
        }
        de = r.end;
    });
    saida.extend_from_slice(&cru[de..]);
    saida
}

/// `cru` mascarado, em cópia (para quem não tem `ArquivoLinhas`: testes e bytes soltos).
pub fn mascarar(cru: &[u8]) -> Vec<u8> {
    let mut saida = cru.to_vec();
    mascarar_em(cru, &IndiceLinhas::construir(cru), &mut saida);
    saida
}

#[cfg(test)]
mod testes {
    use super::*;

    fn m(s: &str) -> String {
        String::from_utf8(mascarar(s.as_bytes())).unwrap()
    }

    use proptest::prelude::*;
    use serde_json::Value;

    fn valor_json() -> impl Strategy<Value = Value> {
        let folha = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            (-1e9f64..1e9).prop_map(Value::from),
            "\\PC{0,12}".prop_map(Value::String),
            Just(Value::String("111.444.777-35".into())),
            "[a-z0-9\\\\\"\u{e3}\u{1f600}]{0,8}".prop_map(Value::String),
        ];
        let chave = prop_oneof![
            Just("senha".to_string()),
            Just("cart\u{e3}o".to_string()),
            Just("cpfCliente".to_string()),
            Just("cvv".to_string()),
            Just("nome".to_string()),
            "[a-z_\"\\\\ ]{0,6}",
        ];
        folha.prop_recursive(4, 32, 4, move |dentro| {
            prop_oneof![
                prop::collection::vec(dentro.clone(), 0..4).prop_map(Value::Array),
                prop::collection::vec((chave.clone(), dentro), 0..4)
                    .prop_map(|v| Value::Object(v.into_iter().collect())),
            ]
        })
    }

    /// Mesma forma; abaixo de chave sensível nenhum escalar sobra (texto só `*`, número só `9`).
    fn conferir(a: &Value, b: &Value, sens: bool) -> Result<(), String> {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) if x.len() == y.len() => {
                x.iter().zip(y).try_for_each(|((k, va), (kb, vb))| {
                    if k != kb {
                        return Err(format!("chave {k} virou {kb}"));
                    }
                    conferir(va, vb, sens || chave_de_segredo(k.as_bytes()))
                })
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
                .iter()
                .zip(y)
                .try_for_each(|(va, vb)| conferir(va, vb, sens)),
            (Value::String(x), Value::String(y)) if sens || sensivel(x.as_bytes()) => {
                match y.chars().all(|c| c == '*') && (x.is_empty() || !y.is_empty()) {
                    true => Ok(()),
                    false => Err(format!("{x:?} virou {y:?}")),
                }
            }
            (Value::Number(x), Value::Number(y)) if sens => {
                let noves: String = x
                    .to_string()
                    .chars()
                    .map(|c| match c.is_ascii_digit() {
                        true => '9',
                        false => c,
                    })
                    .collect();
                match serde_json::from_str::<Value>(&noves).ok().as_ref() == Some(b) {
                    true => Ok(()),
                    false => Err(format!("número {x} virou {y}")),
                }
            }
            (x, y)
                if std::mem::discriminant(x) == std::mem::discriminant(y)
                    && (matches!(x, Value::String(_) | Value::Number(_)) || x == y) =>
            {
                Ok(())
            }
            (x, y) => Err(format!("{x} virou {y}")),
        }
    }

    /// Troca letras e dígitos dentro dos textos de `c` por `\uXXXX` onde `escape` manda: o mesmo
    /// JSON, escrito de outro jeito.
    fn com_escapes(c: &str, escape: u64) -> String {
        let (mut s, mut dentro, mut barra) = (String::new(), false, false);
        for (i, ch) in c.chars().enumerate() {
            if dentro && !barra && ch.is_ascii_alphanumeric() && escape >> (i % 64) & 1 == 1 {
                s += &format!("\\u{:04x}", ch as u32);
                continue;
            }
            s.push(ch);
            match (barra, ch) {
                (true, _) => barra = false,
                (false, '\\') => barra = true,
                (false, '"') => dentro = !dentro,
                _ => {}
            }
        }
        s
    }

    proptest! {
        /// JSON válido (compacto ou formatado, com ou sem `\uXXXX`) segue válido, do mesmo tamanho
        /// e sem segredo.
        #[test]
        fn json_valido_segue_valido_sem_segredo(
            v in valor_json(),
            bonito in any::<bool>(),
            escape in prop_oneof![Just(0u64), any::<u64>()],
        ) {
            let c = match bonito {
                true => serde_json::to_string_pretty(&v).unwrap(),
                false => serde_json::to_string(&v).unwrap(),
            };
            let c = com_escapes(&c, escape);
            let s = m(&c);
            prop_assert_eq!(s.len(), c.len());
            let b: Value = serde_json::from_str(&s).map_err(|e| TestCaseError::fail(format!("{e}: {s}")))?;
            let a: Value = serde_json::from_str(&c).unwrap();
            conferir(&a, &b, false).map_err(|e| TestCaseError::fail(format!("{e}\n{c}\n{s}")))?;
        }
    }

    #[test]
    fn por_chave_em_qualquer_ancestral() {
        assert_eq!(
            m(r#"{"senha":"abc","nome":"Ana","auth":{"token":"xyz"},"n":12}"#),
            r#"{"senha":"***","nome":"Ana","auth":{"token":"***"},"n":12}"#
        );
        assert_eq!(
            m(r#"{"secret":{"a":[1,"b",{"c":-2.5e3}],"t":true,"z":null},"x":"y"}"#),
            r#"{"secret":{"a":[9,"*",{"c":-9.9e9}],"t":true,"z":null},"x":"y"}"#
        );
        assert_eq!(m(r#"{"senhas":["a","bc"]}"#), r#"{"senhas":["*","**"]}"#);
        // A chave nunca é mascarada; o valor que tem nome de chave sensível também não.
        assert_eq!(m(r#"{"tipo":"senha"}"#), r#"{"tipo":"senha"}"#);
    }

    #[test]
    fn cvv_otp_cpf_cnpj_rg_cartao_pelo_nome() {
        for k in [
            "cvv",
            "cvc",
            "otp",
            "cpf",
            "cnpj",
            "rg",
            "numero_rg",
            "cartao",
            "card_number",
            "otpCode",
        ] {
            let s = format!(r#"{{"{k}":"S3NT1N3LA","n":"ok"}}"#);
            assert_eq!(m(&s), format!(r#"{{"{k}":"*********","n":"ok"}}"#), "{k}");
        }
        for k in [
            "org",
            "organizacao",
            "rg_emissor",
            "cartao_tipo",
            "cpf_valido",
        ] {
            let s = format!(r#"{{"{k}":"ok"}}"#);
            assert_eq!(m(&s), s, "{k}");
        }
    }

    #[test]
    fn pelo_valor() {
        for v in [
            "111.444.777-35",
            "11144477735",
            "11.222.333/0001-81",
            "11222333000181",
            "4111 1111 1111 1111",
            "4111-1111-1111-1111",
            "5555555555554444",
            "Bearer abcdefghijklmnopqrstu",
        ] {
            let s = format!(r#"{{"x":"{v}"}}"#);
            assert_eq!(
                m(&s),
                format!(r#"{{"x":"{}"}}"#, "*".repeat(v.len())),
                "{v}"
            );
        }
        for v in [
            "111.444.777-36",   // dígito verificador errado
            "11111111111",      // repetido
            "4111111111111112", // Luhn errado
            "1732400000000",    // milissegundos: começa com 1
            "2026-11-24",
            "(11) 98765-4321",
        ] {
            let s = format!(r#"{{"x":"{v}"}}"#);
            assert_eq!(m(&s), s, "{v}");
        }
        // Número solto não é olhado pelo valor (só pela chave).
        assert_eq!(m(r#"{"x":11144477735}"#), r#"{"x":11144477735}"#);
        assert_eq!(m(r#"{"cpf":11144477735}"#), r#"{"cpf":99999999999}"#);
    }

    #[test]
    fn mesmo_comprimento_e_json_valido() {
        let casos = [
            r#"{"senha":"a\"b\\céção","pin":-0.5,"l":[{"token":""}]}"#,
            r#"[{"api_key":"x"},{"password":12}]"#,
            r#""Bearer abcdefghijklmnopqrstuvwxyz""#,
            "{\"secret\": \"x\" ,\r\n \"a\" : 1}",
        ];
        for c in casos {
            let s = m(c);
            assert_eq!(s.len(), c.len(), "{c}");
            let a: serde_json::Value = serde_json::from_str(c).unwrap();
            let b: serde_json::Value = serde_json::from_str(&s).unwrap();
            // Mesma forma: só os escalares mascarados mudam, sem mudar de tipo.
            fn forma(v: &serde_json::Value) -> String {
                match v {
                    serde_json::Value::Object(o) => {
                        o.iter().map(|(k, v)| format!("{k}:{}", forma(v))).collect()
                    }
                    serde_json::Value::Array(l) => l.iter().map(forma).collect(),
                    serde_json::Value::String(_) => "s".into(),
                    serde_json::Value::Number(_) => "n".into(),
                    o => o.to_string(),
                }
            }
            assert_eq!(forma(&a), forma(&b), "{c}");
        }
    }

    #[test]
    fn linha_estranha_mascara_dali_em_diante() {
        // Aspa a mais: o léxico se perde em `b` e o resto da linha sai mascarado.
        assert_eq!(
            m("{\"q\": \"a\"b\", \"senha\":\"s3\"}\n{\"senha\":\"x\",\"ok\":\"sim\"}"),
            "{\"q\": \"a\"*\", \"*****\":\"**\"}\n{\"senha\":\"*\",\"ok\":\"sim\"}"
        );
        // Texto sem fechar e não ASCII.
        assert_eq!(m("{\"token\":\"abç"), "{\"token\":\"****");
        assert_eq!(m("{\"a\":{\"secret\":[x1]}}"), "{\"a\":{\"secret\":[**]}}");
        // Sem sinal de segredo, a linha quebrada fica como está (o erro continua legível).
        assert_eq!(m("NaN"), "NaN");
        assert_eq!(m("{\"b\":tru}"), "{\"b\":tru}");
        // Valor com cara de segredo numa linha quebrada.
        assert_eq!(
            m("{\"x\": oops \"111.444.777-35"),
            "{\"x\": **** \"***.***.***-**"
        );
    }

    #[test]
    fn documento_em_varias_linhas() {
        let doc = "{\n  \"usuario\": {\n    \"senha\": \"abc\",\n    \"nome\": \"Ana\"\n  },\n  \"pin\": 1234\n}\n";
        assert_eq!(
            m(doc),
            "{\n  \"usuario\": {\n    \"senha\": \"***\",\n    \"nome\": \"Ana\"\n  },\n  \"pin\": 9999\n}\n"
        );
        // Documento que se perde abaixo de chave sensível: o resto inteiro sai mascarado.
        assert_eq!(
            m("{\"senha\": [\n  x,\n  \"a\"\n]}"),
            "{\"senha\": [\n  *,\n  \"*\"\n]}"
        );
        // Fora dela, só as linhas suspeitas.
        assert_eq!(
            m("[\n  x,\n  {\"pin\": \"1\"},\n  \"a\"\n]"),
            "[\n  x,\n  {\"***\": \"*\"},\n  \"a\"\n]"
        );
    }

    #[test]
    fn em_blocos_da_o_mesmo_que_numa_thread() {
        // Mais linhas que um bloco, com linha inválida e documento aberto no meio.
        let mut d = String::new();
        for i in 0..40_000 {
            match i % 7 {
                0 => d += &format!(r#"{{"id":{i},"senha":"s{i}","n":[1,{i}]}}"#),
                1 => d += r#"{"a": {"cvv": 123, "x": "111.444.777-35"}}"#,
                2 => d += r#"{"token": "abc" oops"#,
                3 => d += "{\"aberto\": [",
                4 => d += "",
                _ => d += &format!(r#"{{"nome":"n{i}","v":{i}.5}}"#),
            }
            d += if i % 3 == 0 { "\r\n" } else { "\n" };
        }
        let d = d.as_bytes();
        let mut so = d.to_vec();
        trechos(d, &IndiceLinhas::construir(d), |r, t| {
            aplicar(&mut so, r, t)
        });
        assert_ne!(so, d);
        assert_eq!(mascarar(d), so);
        let ind = IndiceLinhas::construir(d);
        let mut em_blocos = d.to_vec();
        let mut resto = &mut em_blocos[..];
        let mut base = 0;
        for a in (0..ind.len()).step_by(1000) {
            let e = (a + 1000).min(ind.len());
            let fim = ind.faixa(e, d).map_or(d.len(), |f| f.start);
            let (p, r) = std::mem::take(&mut resto).split_at_mut(fim - base);
            mascarar_bloco(d, &ind, a..e, None, p, base);
            base = fim;
            resto = r;
        }
        assert_eq!(em_blocos, so);
    }

    #[test]
    fn documento_em_blocos_da_o_mesmo_que_de_uma_vez() {
        // Documento formatado: segredo aberto num bloco e fechado em outro, e um que se perde.
        let mut d = String::from("{\n \"lista\": [\n");
        for i in 0..5_000 {
            d += &format!("  {{\"id\": {i}, \"senha\": \n  \"s{i}\", \"n\": \"ok\"}},\n");
        }
        d += "  {\"token\": [\n   1,\n   2\n  ]},\n  x \"111.444.777-35\"\n]}\n";
        let d = d.as_bytes();
        let ind = IndiceLinhas::construir(d);
        assert!(!por_linha(d, &ind));
        let so = mascarar(d);
        for passo in [1, 7, 1000] {
            let mut e = Estado::default();
            let mut em_blocos = d.to_vec();
            for a in (0..ind.len()).step_by(passo) {
                let z = (a + passo).min(ind.len());
                let de = ind.faixa(a, d).unwrap().start;
                let ate = ind.faixa(z, d).map_or(d.len(), |f| f.start);
                mascarar_bloco(d, &ind, a..z, Some(&mut e), &mut em_blocos[de..ate], de);
            }
            assert_eq!(em_blocos, so, "passo {passo}");
        }
        let s = String::from_utf8(so).unwrap();
        assert!(!s.contains("\"s4999\"") && !s.contains("777-35") && s.contains("\"ok\""));
    }

    #[test]
    fn escapes_do_json_nao_furam_a_mascara() {
        for c in [
            r#"{"\u0073enha":"LEAK1"}"#,
            r#"{"se\u006eha":"LEAK1"}"#,
            r#"{"tok\u0065n":"LEAK3"}"#,
            r#"{"cart\u00e3o":"LEAK4"}"#,
            r#"{"x":"\u0034111 1111 1111 1111"}"#,
            r#"{"x":"111.444.777\u002d35"}"#,
            r#"{"x":"\u0065yJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJh"}"#,
            r#"{"x":"Bearer\u0020abcdefghijklmnopqrstu"}"#,
            r#"{"x":"\ud83d\ude00", "\u0063pf": "LEAK5"}"#,
        ] {
            let s = m(c);
            let a: Value = serde_json::from_str(c).unwrap();
            let b: Value = serde_json::from_str(&s).unwrap();
            conferir(&a, &b, false).unwrap_or_else(|e| panic!("{e}: {c} -> {s}"));
            assert!(
                !s.contains("LEAK") && !s.contains("4111") && !s.contains("abcdefghij"),
                "{c} -> {s}"
            );
        }
        // Escape inválido ou sem par não quebra nem esconde à toa.
        assert_eq!(m(r#"{"x":"\ud800"}"#), r#"{"x":"\ud800"}"#);
    }

    #[test]
    fn padroes_de_credencial_e_cartao_serie_2() {
        for v in [
            "bearer abcdefghijklmnopqrstu",
            "BEARER abcdefghijklmnopqrstu",
            "AIzaSyA1234567890abcdefghijklmnopqrstu",
            "glpat-abcdefghijklmnopqrst",
            "sk_test_abcdefghijklmnopqrst",
            "ASIAIOSFODNN7EXAMPLE",
            "npm_abcdefghijklmnopqrstuvwxyz0123456789",
            "2221000000000009",
            "2720 9900 0000 0007",
        ] {
            assert!(sensivel(v.as_bytes()), "{v}");
        }
        for v in [
            "2220000000000000",
            "2721000000000004",
            "bearer curto",
            "glpat-x",
        ] {
            assert!(!sensivel(v.as_bytes()), "{v}");
        }
    }

    #[test]
    fn linha_funda_tem_teto_e_nao_solta_segredo() {
        let n = MAX_PROFUNDIDADE * 50;
        let c = format!(
            "{}{{\"senha\":\"LEAK1\"}}{}\n{{\"ok\":1}}",
            "[".repeat(n),
            "]".repeat(n)
        );
        let s = m(&c);
        assert_eq!(s.len(), c.len());
        assert!(!s.contains("LEAK1"));
        assert!(
            s.ends_with("\n{\"ok\":1}"),
            "a linha seguinte (JSONL) é lida do zero"
        );
        // A pilha para no teto: nada cresce com a profundidade.
        let mut nada = |_, _| {};
        let mut l = Lexico::novo(c.as_bytes(), Estado::default(), &mut nada);
        assert!(!l.linha(0, n + 20));
        assert_eq!(l.e.pilha.len(), MAX_PROFUNDIDADE);
        // Documento que passa do teto: o resto suspeito sai mascarado.
        let doc = format!("{}\n{{\"pin\": \"1\"}}\n", "[".repeat(n));
        assert!(!m(&doc).contains("\"1\""));
    }

    #[test]
    fn cnpj_com_digito_errado_nao_e_segredo() {
        for v in ["11.222.333/0001-82", "11222333000180", "11.222.333/0001-91"] {
            let s = format!(r#"{{"x":"{v}"}}"#);
            assert_eq!(m(&s), s, "{v}");
        }
    }

    #[test]
    fn sal_muda_por_chamada() {
        assert_ne!(sal_novo(), sal_novo());
    }

    #[test]
    fn anonimizar_linha_estranha_nao_solta_o_segredo() {
        let cru = b"{\"senha\": oops \"S3NT1N3LA\"}\n{\"x\": tru \"111.444.777-35\"}\n";
        let a = String::from_utf8(anonimizar(cru, &IndiceLinhas::construir(cru), 7)).unwrap();
        assert!(!a.contains("S3NT1N3LA") && !a.contains("777-35"), "{a}");
    }

    #[test]
    fn anonimizar_e_estavel_e_sem_o_valor() {
        let cru = b"{\"senha\":\"S3NT1N3LA\",\"n\":1}\n{\"senha\":\"S3NT1N3LA\",\"pin\":42}\n{\"senha\":\"outra\"}";
        let ind = IndiceLinhas::construir(cru);
        let a = String::from_utf8(anonimizar(cru, &ind, 7)).unwrap();
        assert!(!a.contains("S3NT1N3LA") && !a.contains("42") && !a.contains("outra"));
        let linhas: Vec<serde_json::Value> = a
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            linhas[0]["senha"], linhas[1]["senha"],
            "mesmo valor, mesmo código"
        );
        assert_ne!(linhas[0]["senha"], linhas[2]["senha"]);
        assert!(linhas[1]["pin"].as_str().unwrap().starts_with("anon:"));
        assert_eq!(linhas[0]["n"], 1);
        assert_ne!(
            anonimizar(cru, &ind, 8),
            a.as_bytes(),
            "outro sal, outros códigos"
        );
    }
}
