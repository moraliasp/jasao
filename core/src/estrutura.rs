//! Inferência de estrutura de JSONL (um registro por linha) e de JSON único (lista no topo:
//! cada elemento é um registro; objeto no topo: um registro só).
//!
//! - Por caminho (`.usuario.endereco.cidade`, `.itens[]`, `.contagens.*`): tipos vistos com
//!   contagem e a primeira linha de cada tipo, e presença (em quantos objetos-pai o campo aparece).
//! - Leitor próprio: valida a linha inteira antes de contar, então linha inválida só soma em
//!   `invalidas`. Número é classificado pelo texto (fração ou expoente = decimal), sem `f64`.
//! - JSONL: um `Resumo` por bloco de linhas, combinados por `rayon` (mapa e redução). `combinar`
//!   é associativa e comutativa: o resultado não depende do bloco nem da ordem.
//! - Critérios de mapa e de união discriminada: `docs/decisoes.md`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc;
use std::thread;

use memchr::{memchr, memchr_iter};
use rayon::prelude::*;

use crate::busca::{BLOCO_PADRAO, fatiar};
use crate::linhas::BOM;

/// Acima disto, as chaves distintas de um objeto viram um mapa já durante a varredura (memória).
pub const LIMITE_CHAVES: usize = 1000;
/// Mínimo de chaves distintas para a regra de forma do mapa (ver `finalizar`).
pub const MAPA_MIN_CHAVES: usize = 20;
/// Um campo de texto com mais valores distintos que isto não é discriminador.
pub const DISCRIMINADOR_MAX_VALORES: usize = 32;
/// Aninhamento máximo de um registro (igual ao padrão do `serde_json`); acima, a linha é inválida.
pub const PROFUNDIDADE_MAX: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tipo {
    Nulo,
    Booleano,
    Inteiro,
    Decimal,
    Texto,
    Objeto,
    Lista,
}

impl Tipo {
    pub const TODOS: [Tipo; 7] = [
        Tipo::Nulo,
        Tipo::Booleano,
        Tipo::Inteiro,
        Tipo::Decimal,
        Tipo::Texto,
        Tipo::Objeto,
        Tipo::Lista,
    ];

    pub fn nome(self) -> &'static str {
        match self {
            Tipo::Nulo => "nulo",
            Tipo::Booleano => "booleano",
            Tipo::Inteiro => "inteiro",
            Tipo::Decimal => "decimal",
            Tipo::Texto => "texto",
            Tipo::Objeto => "objeto",
            Tipo::Lista => "lista",
        }
    }
}

/// Quantas vezes algo foi visto e a primeira linha (a partir de 0) em que apareceu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contagem {
    pub n: u64,
    linha: u64,
}

impl Default for Contagem {
    fn default() -> Self {
        Self {
            n: 0,
            linha: u64::MAX,
        }
    }
}

impl Contagem {
    fn ver(&mut self, linha: u64) {
        self.n += 1;
        self.linha = self.linha.min(linha);
    }

    fn somar(&mut self, o: Contagem) {
        self.n += o.n;
        self.linha = self.linha.min(o.linha);
    }

    /// Linha do primeiro registro em que apareceu (a partir de 0).
    pub fn primeira(&self) -> Option<u64> {
        (self.n > 0).then_some(self.linha)
    }
}

/// Um caminho da árvore de estrutura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct No {
    pub tipos: [Contagem; 7],
    /// Campo: em quantos objetos-pai aparece. Elemento (`[]`) e entrada de mapa (`.*`): quantos valores.
    pub presenca: u64,
    /// Primeira aparição (linha, posição da chave no objeto): ordena os irmãos na saída.
    ordem: (u64, u32),
    /// Chaves cruas, como estão no arquivo (sem as aspas, escapes intactos).
    pub filhos: HashMap<Box<[u8]>, No>,
    /// Objeto que é mapa: todas as entradas juntas num nó só.
    pub mapa: Option<Box<No>>,
    pub elementos: Option<Box<No>>,
    /// Maior número de chaves distintas num mesmo objeto.
    max_chaves: u32,
    /// Para o export: valor único, textos que são número e inteiro fora do alcance do JS.
    pub valores: Valores,
}

/// Valor escalar (não nulo) visto num caminho: nenhum, sempre o mesmo, ou vários.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Unico {
    #[default]
    Nenhum,
    /// Tipo e bytes como estão no arquivo (texto sem as aspas, escapes intactos).
    Um(Tipo, Box<[u8]>),
    Varios,
}

/// Maior valor guardado em `Unico::Um`; acima disso o caminho conta como `Varios`.
pub const UNICO_MAX_BYTES: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Valores {
    pub unico: Unico,
    /// Algum texto cujo conteúdo não é um número JSON (`"42"` e `"-1.5e3"` são).
    pub texto_nao_numerico: bool,
    /// Algum inteiro fora de ±(2^53 − 1), que o JavaScript não representa exato.
    pub inteiro_grande: bool,
}

impl Valores {
    fn ver(&mut self, t: Tipo, v: &[u8]) {
        // Caminho quente (todo escalar): não mexe no `unico` quando ele não muda.
        match &self.unico {
            Unico::Varios => {}
            Unico::Um(t0, v0) if *t0 == t && **v0 == *v => {}
            Unico::Nenhum if v.len() <= UNICO_MAX_BYTES => self.unico = Unico::Um(t, v.into()),
            _ => self.unico = Unico::Varios,
        }
        match t {
            Tipo::Texto if !self.texto_nao_numerico => self.texto_nao_numerico = !eh_numero(v),
            Tipo::Inteiro if v.len() > 15 => {
                let d = v.strip_prefix(b"-").unwrap_or(v);
                self.inteiro_grande |= std::str::from_utf8(d)
                    .ok()
                    .and_then(|d| d.parse::<u64>().ok())
                    .is_none_or(|n| n > (1 << 53) - 1);
            }
            _ => {}
        }
    }

    fn combinar(&mut self, o: Valores) {
        self.unico = match (std::mem::take(&mut self.unico), o.unico) {
            (Unico::Nenhum, u) | (u, Unico::Nenhum) => u,
            (Unico::Um(a, x), Unico::Um(b, y)) if a == b && x == y => Unico::Um(a, x),
            _ => Unico::Varios,
        };
        self.texto_nao_numerico |= o.texto_nao_numerico;
        self.inteiro_grande |= o.inteiro_grande;
    }
}

/// O texto inteiro é um número JSON: `-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?`.
pub fn eh_numero(v: &[u8]) -> bool {
    let mut l = Leitor::default();
    matches!(v.first(), Some(b'-' | b'0'..=b'9')) && l.numero(v).is_some() && l.p == v.len()
}

impl Default for No {
    fn default() -> Self {
        Self {
            tipos: Default::default(),
            presenca: 0,
            ordem: (u64::MAX, u32::MAX),
            filhos: HashMap::new(),
            mapa: None,
            elementos: None,
            max_chaves: 0,
            valores: Valores::default(),
        }
    }
}

impl No {
    pub fn tipo(&self, t: Tipo) -> Contagem {
        self.tipos[t as usize]
    }

    /// Primeira aparição (linha, posição da chave): a ordem dos irmãos.
    pub fn ordem(&self) -> (u64, u32) {
        self.ordem
    }

    /// Conjunto de tipos vistos, como máscara de bits.
    fn conjunto(&self) -> u8 {
        (0..7).fold(0, |m, i| m | (u8::from(self.tipos[i].n > 0) << i))
    }

    fn filho(&mut self, chave: &[u8]) -> &mut No {
        if self.mapa.is_none() && !self.filhos.contains_key(chave) {
            if self.filhos.len() >= LIMITE_CHAVES {
                self.colapsar();
            } else {
                self.filhos.insert(chave.into(), No::default());
            }
        }
        match &mut self.mapa {
            Some(m) => m,
            None => self.filhos.get_mut(chave).expect("inserido acima"),
        }
    }

    /// Junta todas as chaves num nó de mapa.
    fn colapsar(&mut self) {
        let mut m = self.mapa.take().map_or_else(No::default, |b| *b);
        for (_, f) in self.filhos.drain() {
            m.combinar(f);
        }
        self.mapa = Some(Box::new(m));
    }

    /// Associativa e comutativa.
    pub fn combinar(&mut self, o: No) {
        for (a, b) in self.tipos.iter_mut().zip(o.tipos) {
            a.somar(b);
        }
        self.presenca += o.presenca;
        self.ordem = self.ordem.min(o.ordem);
        self.max_chaves = self.max_chaves.max(o.max_chaves);
        self.valores.combinar(o.valores);
        combinar_opcao(&mut self.elementos, o.elementos);
        if self.mapa.is_some() || o.mapa.is_some() {
            self.colapsar();
            let m = self.mapa.as_mut().expect("colapsado acima");
            for (_, f) in o.filhos {
                m.combinar(f);
            }
            if let Some(om) = o.mapa {
                m.combinar(*om);
            }
        } else {
            for (k, f) in o.filhos {
                match self.filhos.get_mut(&k) {
                    Some(meu) => meu.combinar(f),
                    None => {
                        self.filhos.insert(k, f);
                    }
                }
            }
            if self.filhos.len() > LIMITE_CHAVES {
                self.colapsar();
            }
        }
    }

    /// Regra de forma do mapa, aplicada depois da redução (de cima para baixo): pelo menos
    /// `MAPA_MIN_CHAVES` chaves distintas, pelo menos o dobro do maior número de chaves num
    /// mesmo objeto (as chaves variam de objeto para objeto) e todos os valores com o mesmo
    /// conjunto de tipos.
    fn finalizar(&mut self) {
        let n = self.filhos.len();
        if self.mapa.is_none() && n >= MAPA_MIN_CHAVES && n >= 2 * self.max_chaves as usize {
            let mut conjuntos = self.filhos.values().map(No::conjunto);
            let primeiro = conjuntos.next();
            if conjuntos.all(|c| Some(c) == primeiro) {
                self.colapsar();
            }
        }
        for f in self.filhos.values_mut() {
            f.finalizar();
        }
        for f in [&mut self.mapa, &mut self.elementos].into_iter().flatten() {
            f.finalizar();
        }
    }
}

fn combinar_opcao(a: &mut Option<Box<No>>, b: Option<Box<No>>) {
    match (a.as_mut(), b) {
        (Some(x), Some(y)) => x.combinar(*y),
        (None, y) => *a = y,
        _ => {}
    }
}

/// Valores vistos de um campo de texto do topo, candidato a discriminador.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Candidato {
    /// Passou de `DISCRIMINADOR_MAX_VALORES`: descartado (e esvaziado).
    excesso: bool,
    valores: HashMap<Box<[u8]>, Variante>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Variante {
    registros: Contagem,
    /// Chave do topo → em quantos registros desta variante aparece.
    chaves: HashMap<Box<[u8]>, u64>,
}

impl Candidato {
    fn ver(&mut self, valor: &[u8], chaves: &[&[u8]], linha: u64) {
        if self.excesso {
            return;
        }
        if !self.valores.contains_key(valor) {
            if self.valores.len() == DISCRIMINADOR_MAX_VALORES {
                self.excesso = true;
                self.valores.clear();
                return;
            }
            self.valores.insert(valor.into(), Variante::default());
        }
        let v = self.valores.get_mut(valor).expect("inserido acima");
        v.registros.ver(linha);
        for &k in chaves {
            *v.chaves.entry(k.into()).or_default() += 1;
        }
    }

    fn combinar(&mut self, o: Candidato) {
        self.excesso |= o.excesso;
        if !self.excesso {
            for (valor, ov) in o.valores {
                let v = self.valores.entry(valor).or_default();
                v.registros.somar(ov.registros);
                for (k, n) in ov.chaves {
                    *v.chaves.entry(k).or_default() += n;
                }
            }
            self.excesso = self.valores.len() > DISCRIMINADOR_MAX_VALORES;
        }
        if self.excesso {
            self.valores.clear();
        }
    }
}

/// Resumo combinável de um conjunto de registros.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resumo {
    /// Caminho `.`: um valor por registro.
    pub raiz: No,
    /// Linhas só com espaço em branco (não são registros).
    pub vazias: u64,
    pub invalidas: Contagem,
    candidatos: HashMap<Box<[u8]>, Candidato>,
}

impl Resumo {
    pub fn registros(&self) -> u64 {
        self.raiz.presenca
    }

    /// Associativa e comutativa: o resumo do todo é a combinação dos resumos das partes.
    pub fn combinar(&mut self, o: Resumo) {
        self.raiz.combinar(o.raiz);
        self.vazias += o.vazias;
        self.invalidas.somar(o.invalidas);
        for (k, c) in o.candidatos {
            match self.candidatos.get_mut(&k) {
                Some(meu) => meu.combinar(c),
                None => {
                    self.candidatos.insert(k, c);
                }
            }
        }
    }

    /// Registra um registro já validado (`evs` sobre os bytes `b`).
    fn registrar(&mut self, evs: &[Ev], b: &[u8], linha: u64) {
        self.raiz.presenca += 1;
        if evs[0] == Ev::Obj {
            self.candidatos_do_topo(evs, b, linha);
        }
        aplicar(&mut self.raiz, evs, &mut 0, b, linha);
    }

    fn candidatos_do_topo(&mut self, evs: &[Ev], b: &[u8], linha: u64) {
        let (mut prof, mut chave) = (0, &b[..0]);
        let mut chaves: Vec<&[u8]> = Vec::new();
        let mut textos: Vec<(&[u8], &[u8])> = Vec::new();
        for &ev in &evs[1..] {
            match ev {
                Ev::Obj | Ev::Lista => prof += 1,
                Ev::FimObj | Ev::FimLista if prof == 0 => break,
                Ev::FimObj | Ev::FimLista => prof -= 1,
                Ev::Chave(a, z) if prof == 0 => {
                    chave = &b[a..z];
                    chaves.push(chave);
                }
                Ev::Texto(a, z) if prof == 0 && !textos.iter().any(|t| t.0 == chave) => {
                    textos.push((chave, &b[a..z]));
                }
                _ => {}
            }
        }
        chaves.sort_unstable();
        chaves.dedup();
        for (k, v) in textos {
            match self.candidatos.get_mut(k) {
                Some(c) => c.ver(v, &chaves, linha),
                None => {
                    let mut c = Candidato::default();
                    c.ver(v, &chaves, linha);
                    self.candidatos.insert(k.into(), c);
                }
            }
        }
    }

    /// Aplica a regra de forma do mapa. Chamada uma vez, no resumo final.
    pub fn finalizar(&mut self) {
        self.raiz.finalizar();
    }

    /// A árvore achatada em ordem de primeira aparição, a partir de `.`.
    pub fn caminhos(&self) -> Vec<Caminho> {
        let mut v = Vec::new();
        listar(
            &self.raiz,
            ".".into(),
            Relacao::Registro,
            self.registros(),
            &mut v,
        );
        v
    }

    /// Campos de texto do topo que particionam os registros em formatos diferentes. Critério:
    /// presente em todo registro-objeto, sempre texto, de 2 a `DISCRIMINADOR_MAX_VALORES`
    /// valores, e alguma chave do topo presente em todos os registros de uma variante e em
    /// nenhum de outra.
    pub fn discriminadores(&self) -> Vec<Discriminador> {
        let objetos = self.raiz.tipo(Tipo::Objeto).n;
        let mut saida = Vec::new();
        for (campo, c) in &self.candidatos {
            let Some(no) = self.raiz.filhos.get(campo) else {
                continue;
            };
            if c.excesso
                || c.valores.len() < 2
                || no.presenca != objetos
                || no.conjunto() != 1 << Tipo::Texto as u8
            {
                continue;
            }
            let variantes: Vec<&Variante> = c.valores.values().collect();
            let separa = variantes.iter().any(|a| {
                a.chaves.iter().any(|(k, &n)| {
                    n == a.registros.n && variantes.iter().any(|b| !b.chaves.contains_key(k))
                })
            });
            if !separa {
                continue;
            }
            let ordem_chave = |k: &[u8]| (self.raiz.filhos.get(k).map(|f| f.ordem), k.to_vec());
            let mut vs: Vec<VarianteDiscriminada> = c
                .valores
                .iter()
                .map(|(valor, v)| {
                    let mut chaves: Vec<(&Box<[u8]>, &u64)> = v.chaves.iter().collect();
                    chaves.sort_by_key(|(k, _)| ordem_chave(k));
                    VarianteDiscriminada {
                        valor: texto(valor),
                        registros: v.registros,
                        obrigatorias: chaves
                            .iter()
                            .filter(|(_, n)| **n == v.registros.n)
                            .map(|(k, _)| texto(k))
                            .collect(),
                        opcionais: chaves
                            .iter()
                            .filter(|(_, n)| **n < v.registros.n)
                            .map(|(k, n)| (texto(k), **n))
                            .collect(),
                    }
                })
                .collect();
            vs.sort_by(|a, b| (a.registros.linha, &a.valor).cmp(&(b.registros.linha, &b.valor)));
            saida.push((
                no.ordem,
                Discriminador {
                    campo: juntar(".", &segmento(campo)),
                    cru: campo.clone(),
                    variantes: vs,
                },
            ));
        }
        saida.sort_by_key(|a| a.0);
        saida.into_iter().map(|(_, d)| d).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relacao {
    /// `.`: um valor por registro; `de` = registros.
    Registro,
    /// Campo de objeto; `de` = quantas vezes o pai foi objeto.
    Campo,
    /// `[]`: `presenca` = elementos; `de` = quantas vezes o pai foi lista.
    Elemento,
    /// `.*` de mapa: `presenca` = entradas; `de` = quantas vezes o pai foi objeto.
    Entrada,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caminho {
    pub caminho: String,
    pub relacao: Relacao,
    pub profundidade: usize,
    pub presenca: u64,
    pub de: u64,
    pub tipos: [Contagem; 7],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discriminador {
    pub campo: String,
    /// Chave como está no arquivo (sem aspas, escapes intactos).
    pub cru: Box<[u8]>,
    pub variantes: Vec<VarianteDiscriminada>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarianteDiscriminada {
    /// Valor como está no arquivo (escapes intactos).
    pub valor: String,
    pub registros: Contagem,
    pub obrigatorias: Vec<String>,
    /// Chave e em quantos registros da variante aparece.
    pub opcionais: Vec<(String, u64)>,
}

fn texto(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `.chave` se for identificador, senão `["chave"]` (como está no arquivo).
fn segmento(k: &[u8]) -> String {
    let ident = |c: u8, inicio: bool| {
        c == b'_' || c >= 0x80 || c.is_ascii_alphabetic() || (!inicio && c.is_ascii_digit())
    };
    let ok = !k.is_empty() && ident(k[0], true) && k[1..].iter().all(|&c| ident(c, false));
    if ok {
        format!(".{}", texto(k))
    } else {
        format!("[\"{}\"]", texto(k))
    }
}

fn juntar(caminho: &str, seg: &str) -> String {
    match (caminho, seg.starts_with('[')) {
        (".", true) => format!(".{seg}"),
        (".", false) => seg.to_string(),
        _ => format!("{caminho}{seg}"),
    }
}

fn listar(no: &No, caminho: String, relacao: Relacao, de: u64, v: &mut Vec<Caminho>) {
    let profundidade = v.last().map_or(0, |_| caminho.matches(['.', '[']).count());
    let objetos = no.tipo(Tipo::Objeto).n;
    let listas = no.tipo(Tipo::Lista).n;
    v.push(Caminho {
        caminho: caminho.clone(),
        relacao,
        profundidade,
        presenca: no.presenca,
        de,
        tipos: no.tipos,
    });
    let mut filhos: Vec<_> = no.filhos.iter().collect();
    filhos.sort_by(|a, b| (a.1.ordem, a.0).cmp(&(b.1.ordem, b.0)));
    for (k, f) in filhos {
        listar(
            f,
            juntar(&caminho, &segmento(k)),
            Relacao::Campo,
            objetos,
            v,
        );
    }
    if let Some(m) = &no.mapa {
        listar(m, juntar(&caminho, ".*"), Relacao::Entrada, objetos, v);
    }
    if let Some(e) = &no.elementos {
        listar(e, juntar(&caminho, "[]"), Relacao::Elemento, listas, v);
    }
}

// ---------------------------------------------------------------------------------------------
// Leitor: valida um valor JSON e o converte numa lista plana de eventos.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ev {
    Obj,
    FimObj,
    Lista,
    FimLista,
    Chave(usize, usize),
    Texto(usize, usize),
    Escalar(Tipo, usize, usize),
}

#[derive(Default)]
struct Leitor {
    p: usize,
    /// Quebras de linha atravessadas (JSON único com várias linhas).
    linha: u64,
    /// `true` = objeto aberto, `false` = lista aberta.
    pilha: Vec<bool>,
}

impl Leitor {
    fn ws(&mut self, b: &[u8]) {
        while let Some(&c) = b.get(self.p) {
            match c {
                b'\n' => self.linha += 1,
                b' ' | b'\t' | b'\r' => {}
                _ => break,
            }
            self.p += 1;
        }
    }

    fn resto_vazio(&mut self, b: &[u8]) -> bool {
        self.ws(b);
        self.p == b.len()
    }

    /// Lê um valor completo a partir de `p`, sem recursão. `false` = inválido.
    fn valor(&mut self, b: &[u8], evs: &mut Vec<Ev>) -> bool {
        self.pilha.clear();
        'valor: loop {
            self.ws(b);
            match b.get(self.p) {
                Some(&c @ (b'{' | b'[')) => {
                    let obj = c == b'{';
                    if self.pilha.len() == PROFUNDIDADE_MAX {
                        return false;
                    }
                    self.p += 1;
                    evs.push(if obj { Ev::Obj } else { Ev::Lista });
                    self.ws(b);
                    if b.get(self.p) == Some(if obj { &b'}' } else { &b']' }) {
                        self.p += 1;
                        evs.push(if obj { Ev::FimObj } else { Ev::FimLista });
                    } else {
                        self.pilha.push(obj);
                        if obj && !self.chave(b, evs) {
                            return false;
                        }
                        continue 'valor;
                    }
                }
                Some(b'"') => match self.texto(b) {
                    Some((a, z)) => evs.push(Ev::Texto(a, z)),
                    None => return false,
                },
                Some(b't') if self.literal(b, b"true") => {
                    evs.push(Ev::Escalar(Tipo::Booleano, self.p - 4, self.p))
                }
                Some(b'f') if self.literal(b, b"false") => {
                    evs.push(Ev::Escalar(Tipo::Booleano, self.p - 5, self.p))
                }
                Some(b'n') if self.literal(b, b"null") => {
                    evs.push(Ev::Escalar(Tipo::Nulo, self.p - 4, self.p))
                }
                Some(b'-' | b'0'..=b'9') => match (self.p, self.numero(b)) {
                    (a, Some(t)) => evs.push(Ev::Escalar(t, a, self.p)),
                    (_, None) => return false,
                },
                _ => return false,
            }
            // Depois de um valor: fecha contêineres ou segue para o próximo item.
            loop {
                let Some(&obj) = self.pilha.last() else {
                    return true;
                };
                self.ws(b);
                match b.get(self.p) {
                    Some(b',') => {
                        self.p += 1;
                        if obj && !self.chave(b, evs) {
                            return false;
                        }
                        continue 'valor;
                    }
                    Some(b'}') if obj => evs.push(Ev::FimObj),
                    Some(b']') if !obj => evs.push(Ev::FimLista),
                    _ => return false,
                }
                self.p += 1;
                self.pilha.pop();
            }
        }
    }

    fn literal(&mut self, b: &[u8], lit: &[u8]) -> bool {
        let ok = b[self.p..].starts_with(lit);
        if ok {
            self.p += lit.len();
        }
        ok
    }

    fn chave(&mut self, b: &[u8], evs: &mut Vec<Ev>) -> bool {
        self.ws(b);
        if b.get(self.p) != Some(&b'"') {
            return false;
        }
        let Some((a, z)) = self.texto(b) else {
            return false;
        };
        evs.push(Ev::Chave(a, z));
        self.ws(b);
        if b.get(self.p) != Some(&b':') {
            return false;
        }
        self.p += 1;
        true
    }

    /// Texto a partir da aspa em `p`; devolve a faixa do conteúdo (sem aspas, escapes intactos).
    /// Bytes não ASCII passam sem conferir UTF-8.
    fn texto(&mut self, b: &[u8]) -> Option<(usize, usize)> {
        self.p += 1;
        let ini = self.p;
        loop {
            match *b.get(self.p)? {
                b'"' => {
                    self.p += 1;
                    return Some((ini, self.p - 1));
                }
                b'\\' => match *b.get(self.p + 1)? {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => self.p += 2,
                    b'u' => {
                        let hex = b.get(self.p + 2..self.p + 6)?;
                        if !hex.iter().all(u8::is_ascii_hexdigit) {
                            return None;
                        }
                        self.p += 6;
                    }
                    _ => return None,
                },
                c if c < 0x20 => return None,
                _ => self.p += 1,
            }
        }
    }

    fn digitos(&mut self, b: &[u8]) -> usize {
        let ini = self.p;
        while b.get(self.p).is_some_and(u8::is_ascii_digit) {
            self.p += 1;
        }
        self.p - ini
    }

    /// `-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?`; decimal se tiver fração ou expoente.
    fn numero(&mut self, b: &[u8]) -> Option<Tipo> {
        if b[self.p] == b'-' {
            self.p += 1;
        }
        match b.get(self.p)? {
            b'0' => self.p += 1,
            b'1'..=b'9' => {
                self.digitos(b);
            }
            _ => return None,
        }
        let mut t = Tipo::Inteiro;
        if b.get(self.p) == Some(&b'.') {
            self.p += 1;
            if self.digitos(b) == 0 {
                return None;
            }
            t = Tipo::Decimal;
        }
        if let Some(b'e' | b'E') = b.get(self.p) {
            self.p += 1;
            if let Some(b'+' | b'-') = b.get(self.p) {
                self.p += 1;
            }
            if self.digitos(b) == 0 {
                return None;
            }
            t = Tipo::Decimal;
        }
        Some(t)
    }
}

/// Aplica os eventos de um valor (a partir de `evs[*i]`) ao nó. Recursão limitada por
/// `PROFUNDIDADE_MAX`, já conferida pelo leitor.
fn aplicar(no: &mut No, evs: &[Ev], i: &mut usize, b: &[u8], linha: u64) {
    let ev = evs[*i];
    *i += 1;
    match ev {
        Ev::Escalar(t, a, z) => {
            no.tipos[t as usize].ver(linha);
            if t != Tipo::Nulo {
                no.valores.ver(t, &b[a..z]);
            }
        }
        Ev::Texto(a, z) => {
            no.tipos[Tipo::Texto as usize].ver(linha);
            no.valores.ver(Tipo::Texto, &b[a..z]);
        }
        Ev::Lista => {
            no.tipos[Tipo::Lista as usize].ver(linha);
            while evs[*i] != Ev::FimLista {
                let e = no.elementos.get_or_insert_default();
                e.presenca += 1;
                e.ordem = e.ordem.min((linha, 0));
                aplicar(e, evs, i, b, linha);
            }
            *i += 1;
        }
        Ev::Obj => {
            no.tipos[Tipo::Objeto as usize].ver(linha);
            // Chaves já vistas neste objeto: chave duplicada conta o valor, mas não a presença.
            let mut vistas: Vec<&[u8]> = Vec::new();
            let mut muitas: Option<HashSet<&[u8]>> = None;
            let mut distintas = 0u32;
            while let Ev::Chave(a, z) = evs[*i] {
                *i += 1;
                let k = &b[a..z];
                let nova = match &mut muitas {
                    Some(h) => h.insert(k),
                    None if vistas.contains(&k) => false,
                    None => {
                        vistas.push(k);
                        if vistas.len() > 32 {
                            muitas = Some(vistas.drain(..).collect());
                        }
                        true
                    }
                };
                let f = no.filho(k);
                if nova {
                    f.presenca += 1;
                    f.ordem = f.ordem.min((linha, distintas));
                    distintas += 1;
                }
                aplicar(f, evs, i, b, linha);
            }
            *i += 1; // FimObj
            no.max_chaves = no.max_chaves.max(distintas);
        }
        Ev::FimObj | Ev::FimLista | Ev::Chave(..) => unreachable!("eventos desalinhados"),
    }
}

/// Resumo das linhas de `b` (JSONL), sendo `linha0` o número da primeira.
pub fn resumir_linhas(b: &[u8], linha0: u64) -> Resumo {
    let mut r = Resumo::default();
    if b.is_empty() {
        return r;
    }
    let corpo = b.strip_suffix(b"\n").unwrap_or(b);
    let mut l = Leitor::default();
    let mut evs = Vec::new();
    for (i, linha) in corpo.split(|&c| c == b'\n').enumerate() {
        let n = linha0 + i as u64;
        if linha.iter().all(|c| matches!(c, b' ' | b'\t' | b'\r')) {
            r.vazias += 1;
            continue;
        }
        evs.clear();
        l.p = 0;
        if l.valor(linha, &mut evs) && l.resto_vazio(linha) {
            r.registrar(&evs, linha, n);
        } else {
            r.invalidas.ver(n);
        }
    }
    r
}

// ---------------------------------------------------------------------------------------------
// Varredura do arquivo.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrada {
    Jsonl,
    /// Um valor JSON no arquivo todo (lista: cada elemento é um registro).
    JsonUnico,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progresso {
    pub varridas: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inferencia {
    pub entrada: Entrada,
    /// Já finalizado (regra de mapa aplicada).
    pub resumo: Resumo,
    /// Linhas lidas de `total`. Menos que `total`: cancelado ou JSON único interrompido por erro.
    pub varridas: u64,
    pub total: u64,
    pub cancelado: bool,
}

/// Infere a estrutura de `dados` inteiro. `ao_progresso` roda na thread do chamador, a cada
/// ~1% e no fim. Ligar `cancelar` para a varredura: o resultado é o parcial das linhas lidas.
pub fn inferir(
    dados: &[u8],
    cancelar: &AtomicBool,
    ao_progresso: impl FnMut(Progresso),
) -> Inferencia {
    inferir_em_blocos(dados, BLOCO_PADRAO, cancelar, ao_progresso)
}

/// `inferir` com tamanho-alvo de bloco escolhido (testes usam blocos minúsculos).
pub fn inferir_em_blocos(
    dados: &[u8],
    bloco: usize,
    cancelar: &AtomicBool,
    mut ao_progresso: impl FnMut(Progresso),
) -> Inferencia {
    let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
    let corpo = &dados[base..];
    let total = (memchr_iter(b'\n', corpo).count()
        + usize::from(!corpo.is_empty() && !corpo.ends_with(b"\n"))) as u64;
    let mut inf = match json_unico(dados, base, total, cancelar, &mut ao_progresso) {
        Some(j) if j.resumo.invalidas.n == 0 => j,
        // JSON único quebrado: fica a leitura com mais registros; no empate, JSONL (que conta
        // todas as linhas em vez de parar no erro).
        Some(j) => {
            let l = jsonl(dados, base, bloco, total, cancelar, &mut ao_progresso);
            if l.resumo.registros() >= j.resumo.registros() {
                l
            } else {
                j
            }
        }
        None => jsonl(dados, base, bloco, total, cancelar, &mut ao_progresso),
    };
    inf.resumo.finalizar();
    inf
}

fn jsonl(
    dados: &[u8],
    base: usize,
    bloco: usize,
    total: u64,
    cancelar: &AtomicBool,
    ao_progresso: &mut impl FnMut(Progresso),
) -> Inferencia {
    let blocos = fatiar(dados, base, bloco.max(1));
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
    let resumo = thread::scope(|s| {
        let (tx, rx) = mpsc::channel::<u64>();
        let (blocos, linha0) = (&blocos, &linha0);
        let h = s.spawn(move || {
            blocos
                .par_iter()
                .enumerate()
                .map(|(i, r)| {
                    if cancelar.load(Relaxed) {
                        return Resumo::default();
                    }
                    let b = &dados[r.clone()];
                    let res = resumir_linhas(b, linha0[i]);
                    let n = memchr_iter(b'\n', b).count() + usize::from(!b.ends_with(b"\n"));
                    let _ = tx.send(n as u64);
                    res
                })
                .reduce(Resumo::default, |mut a, b| {
                    a.combinar(b);
                    a
                })
        });
        let mut ultimo = 0;
        for n in rx {
            varridas += n;
            if varridas == total || varridas - ultimo >= total / 100 {
                ultimo = varridas;
                ao_progresso(Progresso { varridas, total });
            }
        }
        h.join().expect("varredura em pânico")
    });
    Inferencia {
        entrada: Entrada::Jsonl,
        resumo,
        varridas,
        total,
        cancelado: varridas < total,
    }
}

/// `Some` quando o arquivo é um valor JSON só: começa com `[` ou `{` e tem uma linha só, ou a
/// primeira linha sozinha não é JSON válido. Lido em sequência, elemento a elemento.
fn json_unico(
    dados: &[u8],
    base: usize,
    total: u64,
    cancelar: &AtomicBool,
    ao_progresso: &mut impl FnMut(Progresso),
) -> Option<Inferencia> {
    let mut l = Leitor {
        p: base,
        ..Leitor::default()
    };
    l.ws(dados);
    let ini = l.p;
    let topo = *dados.get(ini)?;
    if topo != b'[' && topo != b'{' {
        return None;
    }
    let fim1 = memchr(b'\n', &dados[ini..]).map_or(dados.len(), |p| ini + p);
    let mut so_uma = Leitor {
        p: fim1,
        ..Leitor::default()
    };
    if !so_uma.resto_vazio(dados) {
        let primeira = &dados[ini..fim1];
        let mut t = Leitor::default();
        if t.valor(primeira, &mut Vec::new()) && t.resto_vazio(primeira) {
            return None;
        }
    }

    let mut r = Resumo::default();
    let mut evs = Vec::new();
    let inf = |r: Resumo, varridas: u64, cancelado| Inferencia {
        entrada: Entrada::JsonUnico,
        resumo: r,
        varridas,
        total,
        cancelado,
    };
    if topo == b'{' {
        // shortcut: objeto no topo vira um registro só e seus eventos ficam todos na memória;
        // dividir por campo se aparecer objeto único de vários GB.
        let linha = l.linha;
        if l.valor(dados, &mut evs) && l.resto_vazio(dados) {
            r.registrar(&evs, dados, linha);
            return Some(inf(r, total, false));
        }
        r.invalidas.ver(l.linha);
        return Some(inf(r, l.linha, false));
    }
    // shortcut: lista no topo é lida numa thread só; paralelizar exige achar as fronteiras dos
    // elementos antes. Fazer se o benchmark de JSON único grande pedir.
    l.p += 1;
    l.ws(dados);
    if dados.get(l.p) == Some(&b']') {
        l.p += 1;
        if !l.resto_vazio(dados) {
            r.invalidas.ver(l.linha);
        }
        return Some(inf(r, total, false));
    }
    let mut ultimo = 0;
    loop {
        if cancelar.load(Relaxed) {
            return Some(inf(r, l.linha, true));
        }
        l.ws(dados);
        let linha = l.linha;
        evs.clear();
        if !l.valor(dados, &mut evs) {
            r.invalidas.ver(l.linha);
            return Some(inf(r, l.linha, false));
        }
        r.registrar(&evs, dados, linha);
        if l.linha - ultimo >= total / 100 {
            ultimo = l.linha;
            ao_progresso(Progresso {
                varridas: l.linha,
                total,
            });
        }
        l.ws(dados);
        match dados.get(l.p) {
            Some(b',') => l.p += 1,
            Some(b']') => {
                l.p += 1;
                if !l.resto_vazio(dados) {
                    r.invalidas.ver(l.linha);
                    return Some(inf(r, l.linha, false));
                }
                ao_progresso(Progresso {
                    varridas: total,
                    total,
                });
                return Some(inf(r, total, false));
            }
            _ => {
                r.invalidas.ver(l.linha);
                return Some(inf(r, l.linha, false));
            }
        }
    }
}

/// Registros de um arquivo que `inferir` leu como JSON único, em ordem: `f(linha, bytes)` para
/// cada elemento da lista do topo, ou uma vez com o objeto do topo. Para no primeiro trecho
/// inválido, ao cancelar ou quando `f` devolve `false`. Devolve a última linha alcançada.
pub fn registros_json_unico<'a>(
    dados: &'a [u8],
    cancelar: &AtomicBool,
    mut f: impl FnMut(u64, &'a [u8]) -> bool,
) -> u64 {
    let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
    let mut l = Leitor {
        p: base,
        ..Leitor::default()
    };
    l.ws(dados);
    let mut evs = Vec::new();
    match dados.get(l.p) {
        Some(b'{') => {
            f(l.linha, &dados[l.p..]);
        }
        Some(b'[') => {
            l.p += 1;
            l.ws(dados);
            if dados.get(l.p) == Some(&b']') {
                return l.linha;
            }
            while !cancelar.load(Relaxed) {
                l.ws(dados);
                let (ini, linha) = (l.p, l.linha);
                evs.clear();
                if !l.valor(dados, &mut evs) || !f(linha, &dados[ini..l.p]) {
                    break;
                }
                l.ws(dados);
                match dados.get(l.p) {
                    Some(b',') => l.p += 1,
                    _ => break,
                }
            }
        }
        _ => {}
    }
    l.linha
}

#[cfg(test)]
mod testes {
    use super::*;
    use proptest::prelude::*;
    use serde_json::Value;

    fn inf(dados: &[u8]) -> Inferencia {
        inferir_em_blocos(dados, 7, &AtomicBool::new(false), |_| {})
    }

    fn caminho(i: &Inferencia, c: &str) -> Caminho {
        i.resumo
            .caminhos()
            .into_iter()
            .find(|x| x.caminho == c)
            .unwrap_or_else(|| panic!("falta {c}: {:?}", nomes(i)))
    }

    fn nomes(i: &Inferencia) -> Vec<String> {
        i.resumo.caminhos().into_iter().map(|c| c.caminho).collect()
    }

    fn n(c: &Caminho, t: Tipo) -> (u64, Option<u64>) {
        let x = c.tipos[t as usize];
        (x.n, x.primeira())
    }

    #[test]
    fn opcional() {
        let i = inf(b"{\"a\":1,\"b\":2}\n{\"a\":3}\n");
        assert_eq!(i.entrada, Entrada::Jsonl);
        assert_eq!(i.resumo.registros(), 2);
        let b = caminho(&i, ".b");
        assert_eq!((b.presenca, b.de), (1, 2));
        assert_eq!((caminho(&i, ".a").presenca, caminho(&i, ".a").de), (2, 2));
        assert_eq!(nomes(&i), [".", ".a", ".b"]);
    }

    #[test]
    fn tipos_mistos_com_evidencia() {
        let dados = b"{\"v\":1}\n{\"v\":1.5}\n{\"v\":\"x\"}\n{\"v\":null}\n{\"v\":true}\n{\"v\":[]}\n{\"v\":{}}\n{\"v\":2}\n";
        let i = inf(dados);
        let v = caminho(&i, ".v");
        assert_eq!(n(&v, Tipo::Inteiro), (2, Some(0)));
        assert_eq!(n(&v, Tipo::Decimal), (1, Some(1)));
        assert_eq!(n(&v, Tipo::Texto), (1, Some(2)));
        assert_eq!(n(&v, Tipo::Nulo), (1, Some(3)));
        assert_eq!(n(&v, Tipo::Booleano), (1, Some(4)));
        assert_eq!(n(&v, Tipo::Lista), (1, Some(5)));
        assert_eq!(n(&v, Tipo::Objeto), (1, Some(6)));
    }

    #[test]
    fn numero_pelo_texto() {
        let dados = b"[-0, 12345678901234567890123456789, 1e5, 1.0, -2.5E-3, 0]";
        let i = inf(dados);
        assert_eq!(i.entrada, Entrada::JsonUnico);
        let r = caminho(&i, ".");
        assert_eq!(r.tipos[Tipo::Inteiro as usize].n, 3);
        assert_eq!(r.tipos[Tipo::Decimal as usize].n, 3);
        for ruim in ["01", "1.", ".5", "-", "1e", "+1", "1e+"] {
            let i = inf(format!("{ruim}\n").as_bytes());
            assert_eq!(i.resumo.invalidas.n, 1, "{ruim}");
        }
    }

    #[test]
    fn aninhado_e_lista_de_objetos() {
        let dados = r#"{"usuario":{"endereco":{"cidade":"Exemplópolis"}},"itens":[{"sku":"A"},{"sku":"B","qtd":2}]}
{"usuario":{"endereco":{}},"itens":[]}
"#;
        let i = inf(dados.as_bytes());
        let c = caminho(&i, ".usuario.endereco.cidade");
        assert_eq!((c.presenca, c.de), (1, 2));
        let e = caminho(&i, ".itens[]");
        assert_eq!((e.relacao, e.presenca, e.de), (Relacao::Elemento, 2, 2));
        let q = caminho(&i, ".itens[].qtd");
        assert_eq!((q.presenca, q.de), (1, 2));
        assert_eq!(caminho(&i, ".itens[].sku").presenca, 2);
    }

    #[test]
    fn chave_duplicada_e_chave_estranha() {
        let i = inf(r#"{"a":1,"a":"x","com espaço":true,"a\"b":null}"#.as_bytes());
        let a = caminho(&i, ".a");
        assert_eq!((a.presenca, a.de), (1, 1));
        assert_eq!(
            a.tipos[Tipo::Inteiro as usize].n + a.tipos[Tipo::Texto as usize].n,
            2
        );
        caminho(&i, r#".["com espaço"]"#);
        caminho(&i, r#".["a\"b"]"#);
    }

    #[test]
    fn linha_invalida_conta_e_segue() {
        let dados = b"{\"a\":1}\n{\"a\":\n   \n[1,2,]\n{\"a\":2} lixo\n\"ok\"\n{\"a\":3}";
        let i = inf(dados);
        assert_eq!(i.entrada, Entrada::Jsonl);
        assert_eq!(i.resumo.invalidas.n, 3);
        assert_eq!(i.resumo.invalidas.primeira(), Some(1));
        assert_eq!(i.resumo.vazias, 1);
        assert_eq!(i.resumo.registros(), 3);
        assert_eq!(caminho(&i, ".a").presenca, 2);
        assert_eq!((i.varridas, i.total), (7, 7));
    }

    #[test]
    fn profundidade_maxima() {
        let ok = "[".repeat(PROFUNDIDADE_MAX) + &"]".repeat(PROFUNDIDADE_MAX);
        assert_eq!(
            inf(format!("{ok}\n{ok}\n").as_bytes()).resumo.invalidas.n,
            0
        );
        let fundo = "[".repeat(PROFUNDIDADE_MAX + 1) + &"]".repeat(PROFUNDIDADE_MAX + 1);
        // A 1ª linha válida garante JSONL (em JSON único, a lista do topo não conta nível).
        let i = inf(format!("1\n{fundo}\n{fundo}\n").as_bytes());
        assert_eq!(i.resumo.invalidas.n, 2);
    }

    #[test]
    fn arquivo_vazio_e_so_bom() {
        for dados in [&b""[..], b"\xEF\xBB\xBF", b"\n\n"] {
            let i = inf(dados);
            assert_eq!(i.resumo.registros(), 0);
            assert_eq!(i.resumo.invalidas.n, 0);
            assert!(!i.cancelado);
            assert!(i.resumo.discriminadores().is_empty());
        }
    }

    #[test]
    fn json_unico_lista_e_objeto() {
        let lista = b"\xEF\xBB\xBF[\n  {\"a\": 1},\n  {\"a\": 2,\n   \"b\": \"x\"}\n]\n";
        let i = inf(lista);
        assert_eq!(i.entrada, Entrada::JsonUnico);
        assert_eq!(i.resumo.registros(), 2);
        let b = caminho(&i, ".b");
        assert_eq!((b.presenca, b.de), (1, 2));
        assert_eq!(n(&b, Tipo::Texto), (1, Some(2)));
        assert_eq!((i.varridas, i.total), (5, 5));

        let obj = inf(b"{\n \"a\": [1, 2]\n}");
        assert_eq!(obj.entrada, Entrada::JsonUnico);
        assert_eq!(obj.resumo.registros(), 1);
        assert_eq!(caminho(&obj, ".a[]").presenca, 2);

        // Linha única que é uma lista: elementos são registros.
        assert_eq!(inf(b"[{\"a\":1},{\"a\":2}]").resumo.registros(), 2);
        // Várias linhas válidas começando com lista: JSONL.
        assert_eq!(inf(b"[1]\n[2]\n").entrada, Entrada::Jsonl);
    }

    #[test]
    fn json_unico_quebrado() {
        // Lista interrompida: fica o que veio antes, com a linha do erro.
        let i = inf(b"[\n{\"a\":1},\n{\"a\":2} x\n{\"a\":3}\n]\n");
        assert_eq!(i.entrada, Entrada::JsonUnico);
        assert_eq!(i.resumo.registros(), 2);
        assert_eq!(i.resumo.invalidas.n, 1);
        assert_eq!(i.resumo.invalidas.primeira(), Some(2));
        assert_eq!((i.varridas, i.total), (2, 5));
        // JSONL cuja primeira linha está quebrada não é JSON único.
        let i = inf(b"{\"a\":\n{\"a\":1}\n{\"a\":2}\n");
        assert_eq!(i.entrada, Entrada::Jsonl);
        assert_eq!((i.resumo.registros(), i.resumo.invalidas.n), (2, 1));
    }

    #[test]
    fn discriminador_no_gerador() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 200_000, 3).unwrap();
        let i = inf(&dados);
        let d = i.resumo.discriminadores();
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].campo, ".tipo");
        let mut valores: Vec<_> = d[0].variantes.iter().map(|v| v.valor.as_str()).collect();
        valores.sort();
        assert_eq!(valores, ["evento", "pedido", "sensor"]);
        let pedido = d[0].variantes.iter().find(|v| v.valor == "pedido").unwrap();
        assert_eq!(
            pedido.obrigatorias,
            ["tipo", "id", "quando", "cliente", "itens", "total"]
        );
        assert!(pedido.opcionais.is_empty());
        let soma: u64 = d[0].variantes.iter().map(|v| v.registros.n).sum();
        assert_eq!(soma, i.resumo.registros());
    }

    #[test]
    fn mapa_por_forma_e_por_limite() {
        // Chaves variáveis, mesmo formato: mapa.
        let mut dados = String::new();
        for r in 0..30 {
            dados += &format!("{{\"m\":{{\"k{r}\":{r},\"k{}\":1}},\"largo\":{{", r + 100);
            // Registro largo e fixo: 25 chaves sempre as mesmas, não é mapa.
            for c in 0..25 {
                dados += &format!("{}\"c{c}\":\"x\"", if c == 0 { "" } else { "," });
            }
            dados += "}}\n";
        }
        let i = inf(dados.as_bytes());
        let m = caminho(&i, ".m.*");
        assert_eq!((m.relacao, m.presenca, m.de), (Relacao::Entrada, 60, 30));
        assert_eq!(m.tipos[Tipo::Inteiro as usize].n, 60);
        caminho(&i, ".largo.c24");
        assert!(!nomes(&i).iter().any(|c| c.starts_with(".m.k")));

        // Formatos diferentes, mas acima do limite duro: mapa mesmo assim.
        let mut grande = String::from("{");
        for c in 0..=LIMITE_CHAVES {
            let v = if c % 2 == 0 { "1" } else { "\"x\"" };
            grande += &format!("{}\"c{c}\":{v}", if c == 0 { "" } else { "," });
        }
        grande += "}\n";
        let i = inf(grande.as_bytes());
        let m = caminho(&i, ".*");
        assert_eq!(m.presenca, LIMITE_CHAVES as u64 + 1);
        assert_eq!(nomes(&i).len(), 2);
    }

    #[test]
    fn independe_do_bloco() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 100_000, 11).unwrap();
        dados.extend_from_slice(b"{quebrado\n\n");
        let base = inferir(&dados, &AtomicBool::new(false), |_| {});
        assert_eq!(base.resumo.invalidas.n, 1);
        for bloco in [1, 100, 4096] {
            let i = inferir_em_blocos(&dados, bloco, &AtomicBool::new(false), |_| {});
            assert_eq!(i, base, "bloco {bloco}");
        }
    }

    #[test]
    fn progresso_e_cancelamento() {
        let mut dados = Vec::new();
        crate::gerador::gerar(&mut dados, 2 << 20, 5).unwrap();
        let mut ultimo = Progresso {
            varridas: 0,
            total: 0,
        };
        let i = inferir_em_blocos(&dados, 4096, &AtomicBool::new(false), |p| {
            assert!(p.varridas > ultimo.varridas);
            ultimo = p;
        });
        assert_eq!((ultimo.varridas, ultimo.total), (i.total, i.total));
        assert!(!i.cancelado);

        // Cancelado antes de começar: nada lido, determinístico.
        let i = inferir_em_blocos(&dados, 4096, &AtomicBool::new(true), |_| {});
        assert!(i.cancelado);
        assert_eq!((i.varridas, i.resumo.registros()), (0, 0));
        // No meio: quanto já foi lido depende das threads; só o que vale sempre.
        let cancelar = AtomicBool::new(false);
        let i = inferir_em_blocos(&dados, 4096, &cancelar, |_| cancelar.store(true, Relaxed));
        assert_eq!(i.cancelado, i.varridas < i.total);
        assert_eq!(i.resumo.registros(), i.varridas);
    }

    // ---- propriedades ----

    fn valor() -> impl Strategy<Value = Value> {
        let folha = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            (-1000i32..1000).prop_map(|x| Value::from(f64::from(x) / 4.0)),
            "[a-c ]{0,3}".prop_map(Value::String),
        ];
        folha.prop_recursive(4, 32, 4, |dentro| {
            prop_oneof![
                prop::collection::vec(dentro.clone(), 0..4).prop_map(Value::Array),
                prop::collection::vec(("[a-d]|tipo|x y", dentro), 0..5)
                    .prop_map(|kv| Value::Object(kv.into_iter().collect())),
            ]
        })
    }

    fn registros() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(
            prop_oneof![
                8 => valor().prop_map(|v| v.to_string()),
                1 => Just("{quebrado".to_string()),
                1 => Just("  ".to_string()),
            ],
            0..40,
        )
    }

    fn tipo_de(v: &Value) -> Tipo {
        match v {
            Value::Null => Tipo::Nulo,
            Value::Bool(_) => Tipo::Booleano,
            Value::Number(x) if x.is_f64() => Tipo::Decimal,
            Value::Number(_) => Tipo::Inteiro,
            Value::String(_) => Tipo::Texto,
            Value::Array(_) => Tipo::Lista,
            Value::Object(_) => Tipo::Objeto,
        }
    }

    /// Verificador mínimo: o tipo do valor foi visto no caminho, toda chave tem caminho (ou
    /// cai no mapa) e todo campo sempre presente está no objeto.
    fn aceita(no: &No, v: &Value) -> bool {
        if no.tipo(tipo_de(v)).n == 0 {
            return false;
        }
        match v {
            Value::Object(m) => {
                let cruas: Vec<String> = m
                    .keys()
                    .map(|k| serde_json::to_string(k).unwrap()[1..].to_string())
                    .map(|k| k[..k.len() - 1].to_string())
                    .collect();
                let filhos_ok = cruas.iter().zip(m.values()).all(|(k, x)| {
                    no.mapa
                        .as_deref()
                        .or_else(|| no.filhos.get(k.as_bytes()))
                        .is_some_and(|f| aceita(f, x))
                });
                let objetos = no.tipo(Tipo::Objeto).n;
                let obrigatorios_ok = no.filhos.iter().all(|(k, f)| {
                    f.presenca < objetos || cruas.iter().any(|c| c.as_bytes() == &**k)
                });
                filhos_ok && obrigatorios_ok
            }
            Value::Array(a) => a
                .iter()
                .all(|x| no.elementos.as_deref().is_some_and(|e| aceita(e, x))),
            _ => true,
        }
    }

    proptest! {
        #[test]
        fn combinar_qualquer_particao_da_o_todo(
            linhas in registros(),
            cortes in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
            inverter in any::<bool>(),
        ) {
            let texto = linhas.join("\n");
            let todo = resumir_linhas(texto.as_bytes(), 0);
            let mut pontos: Vec<usize> = cortes.iter().map(|c| c.index(linhas.len() + 1)).collect();
            pontos.extend([0, linhas.len()]);
            pontos.sort();
            let mut partes: Vec<Resumo> = pontos
                .windows(2)
                .map(|w| {
                    let parte = linhas[w[0]..w[1]].join("\n");
                    resumir_linhas(parte.as_bytes(), w[0] as u64)
                })
                .collect();
            if inverter {
                partes.reverse();
            }
            // Combina em árvore (pares), não só da esquerda para a direita.
            while partes.len() > 1 {
                let mut prox = Vec::new();
                let mut it = partes.into_iter();
                while let Some(mut a) = it.next() {
                    if let Some(b) = it.next() {
                        a.combinar(b);
                    }
                    prox.push(a);
                }
                partes = prox;
            }
            let combinado = partes.pop().unwrap_or_default();
            prop_assert_eq!(combinado, todo);
        }

        #[test]
        fn todo_registro_e_aceito_pela_estrutura(linhas in registros()) {
            let texto = linhas.join("\n");
            let i = inferir_em_blocos(texto.as_bytes(), 16, &AtomicBool::new(false), |_| {});
            prop_assume!(i.entrada == Entrada::Jsonl);
            for l in &linhas {
                if let Ok(v) = serde_json::from_str::<Value>(l) {
                    prop_assert!(aceita(&i.resumo.raiz, &v), "recusou {l}");
                }
            }
        }
    }
}
