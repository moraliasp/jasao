//! Perfil de um caminho da estrutura: estatísticas dos valores e registros que têm, ou não têm,
//! o caminho. Roda sob pedido (um caminho por vez), numa varredura do arquivo inteiro em 2º
//! plano, nunca no caminho da 1ª tela.
//!
//! - Mínimo e máximo exatos (número pelo valor, texto pela ordem dos bytes crus).
//! - Distribuição dos números por ordem de grandeza (exata, memória constante).
//! - Cardinalidade: exata até `K_DISTINTOS` valores distintos; acima, estimativa KMV (os K
//!   menores hashes), com `distintos_aprox`.
//! - Mais comuns: contagem exata até `CAP_COMUNS` valores distintos; acima, Misra–Gries em lote
//!   (as contagens viram limite inferior), com `comuns_aprox`.
//! - Caminho com aviso de segredo: nenhum valor sai daqui (só contagens).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use memchr::memchr_iter;
use rayon::prelude::*;
use serde::Serialize;

use crate::busca::{BLOCO_PADRAO, fatiar};
use crate::estrutura::{
    Aviso, Caminho, Entrada, Ev, Inferencia, Leitor, Seg, Tipo, registros_json_unico,
};
use crate::linhas::BOM;

/// Valores distintos contados exatos; acima disso a cardinalidade é estimada.
pub const K_DISTINTOS: usize = 4096;
/// Valores distintos guardados para os mais comuns (o lote vai até o dobro).
pub const CAP_COMUNS: usize = 1000;
/// Linhas guardadas em cada lista (com e sem o caminho); as contagens seguem até o fim.
pub const LINHAS_MAX: usize = 1000;
/// Quantos valores mais comuns saem no perfil.
pub const TOP: usize = 10;
/// Valor maior que isto sai cortado, com `…`, nos extremos e nos mais comuns (nos comuns, valores
/// com o mesmo começo contam juntos). Guarda-se 1 byte a mais para saber que houve corte; cortar
/// não muda a ordem, então mínimo e máximo continuam sendo os certos.
pub const VALOR_MAX: usize = 1024;

/// Começo do token guardado (memória por valor limitada, mesmo com texto de megabytes).
fn corte(t: &[u8]) -> &[u8] {
    &t[..t.len().min(VALOR_MAX + 1)]
}

/// Texto que sai no perfil: cortado em `VALOR_MAX` bytes, com `…` se havia mais.
fn texto(b: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(&b[..b.len().min(VALOR_MAX)]).into_owned();
    if b.len() > VALOR_MAX {
        s.push('…');
    }
    s
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Perfil {
    /// Registros válidos varridos.
    pub registros: u64,
    /// Registros com pelo menos um valor no caminho, e sem nenhum.
    pub com: u64,
    pub sem: u64,
    /// Primeiras `LINHAS_MAX` linhas (base 0, onde o registro começa) de cada grupo, em ordem.
    pub linhas_com: Vec<u64>,
    pub linhas_sem: Vec<u64>,
    /// Valores achados no caminho (um registro pode ter vários, em `[]` e `.*`).
    pub valores: u64,
    pub numeros: Option<Numeros>,
    pub textos: Option<Extremos>,
    /// Valores escalares distintos (texto `"1"` e número `1` contam separados).
    pub distintos: u64,
    pub distintos_aprox: bool,
    /// Valor como JSON cru (texto com aspas e escapes) e quantas vezes.
    pub comuns: Vec<(String, u64)>,
    pub comuns_aprox: bool,
    /// Caminho com aviso de segredo: sem valores (`numeros`, `textos` e `comuns` vazios).
    pub oculto: bool,
    pub cancelado: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Extremos {
    pub min: String,
    pub max: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Numeros {
    /// Texto cru do menor e do maior número (exato mesmo fora do `f64`).
    pub min: String,
    pub max: String,
    pub faixas: Vec<Faixa>,
}

/// Números com |x| em [10^expoente, 10^(expoente+1)), do sinal dado; `expoente` None é o zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Faixa {
    pub negativo: bool,
    pub expoente: Option<i32>,
    pub n: u64,
}

/// Perfil do caminho `c` (de `inf.resumo.caminhos()`) no arquivo de onde `inf` veio.
pub fn perfil(inf: &Inferencia, dados: &[u8], c: &Caminho, cancelar: &AtomicBool) -> Perfil {
    perfil_em_blocos(inf, dados, c, BLOCO_PADRAO, cancelar)
}

/// `perfil` com tamanho-alvo de bloco escolhido (testes usam blocos minúsculos).
pub fn perfil_em_blocos(
    inf: &Inferencia,
    dados: &[u8],
    c: &Caminho,
    bloco: usize,
    cancelar: &AtomicBool,
) -> Perfil {
    let ocultar = c.avisos.contains(&Aviso::Segredo);
    let segs = &c.segs[..];
    let acum = match inf.entrada {
        Entrada::Jsonl => {
            let base = if dados.starts_with(BOM) { BOM.len() } else { 0 };
            let blocos = fatiar(dados, base, bloco.max(1));
            let mut linha0 = Vec::with_capacity(blocos.len());
            let mut n = 0u64;
            for r in &blocos {
                linha0.push(n);
                n += memchr_iter(b'\n', &dados[r.clone()]).count() as u64;
            }
            blocos
                .par_iter()
                .enumerate()
                .map(|(i, r)| {
                    let mut a = Acum::new(ocultar);
                    if cancelar.load(Relaxed) {
                        a.cancelado = true;
                        return a;
                    }
                    let b = &dados[r.clone()];
                    let corpo = b.strip_suffix(b"\n").unwrap_or(b);
                    for (j, linha) in corpo.split(|&c| c == b'\n').enumerate() {
                        a.registro(linha, linha0[i] + j as u64, segs);
                    }
                    a
                })
                .reduce(
                    || Acum::new(ocultar),
                    |mut a, b| {
                        a.combinar(b);
                        a
                    },
                )
        }
        Entrada::JsonUnico => {
            let mut a = Acum::new(ocultar);
            registros_json_unico(dados, cancelar, |linha, b| {
                a.registro(b, linha, segs);
                true
            });
            a.cancelado = cancelar.load(Relaxed);
            a
        }
    };
    acum.fim()
}

/// Número já lido e o texto cru dele.
type Num = (f64, Box<[u8]>);
/// Menor e maior vistos.
type Extremo<T> = Option<(T, T)>;

/// Acumulador combinável (associativo; a ordem das linhas é a dos blocos).
struct Acum {
    ocultar: bool,
    cancelado: bool,
    leitor: Leitor,
    evs: Vec<Ev>,
    registros: u64,
    com: u64,
    sem: u64,
    linhas_com: Vec<u64>,
    linhas_sem: Vec<u64>,
    valores: u64,
    num: Extremo<Num>,
    txt: Extremo<Box<[u8]>>,
    /// Chave de `faixa` → quantos.
    faixas: BTreeMap<i32, u64>,
    hashes: BTreeSet<u64>,
    cortou_hashes: bool,
    comuns: HashMap<Box<[u8]>, u64>,
    reduziu_comuns: bool,
}

impl Acum {
    fn new(ocultar: bool) -> Self {
        Self {
            ocultar,
            cancelado: false,
            leitor: Leitor::default(),
            evs: Vec::new(),
            registros: 0,
            com: 0,
            sem: 0,
            linhas_com: Vec::new(),
            linhas_sem: Vec::new(),
            valores: 0,
            num: None,
            txt: None,
            faixas: BTreeMap::new(),
            hashes: BTreeSet::new(),
            cortou_hashes: false,
            comuns: HashMap::new(),
            reduziu_comuns: false,
        }
    }

    /// Um registro (linha do JSONL ou elemento do JSON único); inválido e vazio não contam.
    fn registro(&mut self, b: &[u8], linha: u64, segs: &[Seg]) {
        if b.iter().all(|c| matches!(c, b' ' | b'\t' | b'\r')) {
            return;
        }
        let mut evs = std::mem::take(&mut self.evs);
        evs.clear();
        self.leitor.p = 0;
        if !(self.leitor.valor(b, &mut evs) && self.leitor.resto_vazio(b)) {
            self.evs = evs;
            return;
        }
        self.registros += 1;
        let antes = self.valores;
        coletar(&evs, &mut 0, b, segs, &mut |ev| self.valor(ev, b));
        let (n, l) = if self.valores > antes {
            (&mut self.com, &mut self.linhas_com)
        } else {
            (&mut self.sem, &mut self.linhas_sem)
        };
        *n += 1;
        if l.len() < LINHAS_MAX {
            l.push(linha);
        }
        self.evs = evs;
    }

    fn valor(&mut self, ev: Ev, b: &[u8]) {
        self.valores += 1;
        let (tipo, token) = match ev {
            Ev::Escalar(t, a, z) => (t, &b[a..z]),
            Ev::Texto(a, z) => (Tipo::Texto, &b[a - 1..z + 1]),
            _ => return, // objeto e lista: contam como valor, sem estatística
        };
        let mut h = DefaultHasher::new();
        (tipo as u8, token).hash(&mut h);
        self.hashes.insert(h.finish());
        if self.hashes.len() > K_DISTINTOS {
            self.hashes.pop_last();
            self.cortou_hashes = true;
        }
        if self.ocultar {
            return;
        }
        let curto = corte(token);
        match self.comuns.get_mut(curto) {
            Some(n) => *n += 1,
            None => {
                if self.comuns.len() >= 2 * CAP_COMUNS {
                    self.reduzir();
                }
                self.comuns.insert(curto.into(), 1);
            }
        }
        match tipo {
            Tipo::Inteiro | Tipo::Decimal => {
                let x = std::str::from_utf8(token)
                    .ok()
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                *self.faixas.entry(faixa(x)).or_default() += 1;
                self.ver_num(((x, curto.into()), (x, curto.into())));
            }
            Tipo::Texto => self.ver_txt((curto.into(), curto.into())),
            _ => {}
        }
    }

    fn ver_num(&mut self, (lo, hi): (Num, Num)) {
        match &mut self.num {
            None => self.num = Some((lo, hi)),
            Some((a, z)) => {
                if lo.0 < a.0 {
                    *a = lo;
                }
                if hi.0 > z.0 {
                    *z = hi;
                }
            }
        }
    }

    fn ver_txt(&mut self, (lo, hi): (Box<[u8]>, Box<[u8]>)) {
        match &mut self.txt {
            None => self.txt = Some((lo, hi)),
            Some((a, z)) => {
                if lo < *a {
                    *a = lo;
                }
                if hi > *z {
                    *z = hi;
                }
            }
        }
    }

    /// Misra–Gries em lote: tira de todos a contagem do (CAP+1)-ésimo maior e descarta quem
    /// zerou. Sobram no máximo `CAP_COMUNS`; quem aparece em mais de n/(CAP+1) nunca sai.
    fn reduzir(&mut self) {
        if self.comuns.len() <= CAP_COMUNS {
            return;
        }
        let mut ns: Vec<u64> = self.comuns.values().copied().collect();
        let (_, &mut corte, _) = ns.select_nth_unstable_by(CAP_COMUNS, |a, b| b.cmp(a));
        self.comuns.retain(|_, n| {
            *n = n.saturating_sub(corte);
            *n > 0
        });
        self.reduziu_comuns = true;
    }

    fn combinar(&mut self, o: Acum) {
        self.cancelado |= o.cancelado;
        self.registros += o.registros;
        self.com += o.com;
        self.sem += o.sem;
        for (meu, outro) in [
            (&mut self.linhas_com, o.linhas_com),
            (&mut self.linhas_sem, o.linhas_sem),
        ] {
            meu.extend(outro);
            meu.truncate(LINHAS_MAX);
        }
        self.valores += o.valores;
        if let Some(n) = o.num {
            self.ver_num(n);
        }
        if let Some(t) = o.txt {
            self.ver_txt(t);
        }
        for (k, n) in o.faixas {
            *self.faixas.entry(k).or_default() += n;
        }
        self.hashes.extend(o.hashes);
        self.cortou_hashes |= o.cortou_hashes;
        while self.hashes.len() > K_DISTINTOS {
            self.hashes.pop_last();
            self.cortou_hashes = true;
        }
        for (k, n) in o.comuns {
            *self.comuns.entry(k).or_default() += n;
        }
        self.reduziu_comuns |= o.reduziu_comuns;
        if self.comuns.len() >= 2 * CAP_COMUNS {
            self.reduzir();
        }
    }

    fn fim(self) -> Perfil {
        let distintos = match (self.cortou_hashes, self.hashes.last()) {
            // KMV: K hashes uniformes em [0, 2^64) cujo maior é m estimam (K-1)·2^64/m distintos.
            (true, Some(&m)) => ((K_DISTINTOS - 1) as f64 * (u64::MAX as f64 / m as f64)) as u64,
            _ => self.hashes.len() as u64,
        };
        // Sem reduzir aqui: até 2·CAP distintos sem corte a contagem é exata.
        let mut comuns: Vec<(String, u64)> =
            self.comuns.iter().map(|(k, &n)| (texto(k), n)).collect();
        comuns.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        comuns.truncate(TOP);
        Perfil {
            registros: self.registros,
            com: self.com,
            sem: self.sem,
            linhas_com: self.linhas_com,
            linhas_sem: self.linhas_sem,
            valores: self.valores,
            numeros: self.num.map(|(a, z)| Numeros {
                min: texto(&a.1),
                max: texto(&z.1),
                faixas: self
                    .faixas
                    .iter()
                    .map(|(&k, &n)| Faixa {
                        negativo: k < 0,
                        expoente: (k != 0).then(|| k.abs() - 32),
                        n,
                    })
                    .collect(),
            }),
            textos: self.txt.map(|(a, z)| Extremos {
                min: texto(&a),
                max: texto(&z),
            }),
            distintos,
            distintos_aprox: self.cortou_hashes,
            comuns,
            comuns_aprox: self.reduziu_comuns,
            oculto: self.ocultar,
            cancelado: self.cancelado,
        }
    }
}

/// Ordem de grandeza e = ⌊log10 |x|⌋ (de −30 a 30) como chave que ordena do mais negativo ao
/// mais positivo: ±(e + 32), e 0 para o zero.
fn faixa(x: f64) -> i32 {
    if x == 0.0 || x.is_nan() {
        return 0;
    }
    let e = x.abs().log10().floor().clamp(-30.0, 30.0) as i32 + 32;
    if x < 0.0 { -e } else { e }
}

/// Pula um valor inteiro (a partir de `evs[*i]`).
fn pular(evs: &[Ev], i: &mut usize) {
    let mut prof = 0u32;
    loop {
        let e = evs[*i];
        *i += 1;
        match e {
            Ev::Obj | Ev::Lista => prof += 1,
            Ev::FimObj | Ev::FimLista => prof -= 1,
            Ev::Chave(..) => continue,
            _ => {}
        }
        if prof == 0 {
            return;
        }
    }
}

/// Chama `f` com cada valor do caminho `segs` dentro do valor em `evs[*i]`, e avança `i` para
/// depois dele. Recursão limitada pelo tamanho do caminho (≤ `PROFUNDIDADE_MAX`).
fn coletar(evs: &[Ev], i: &mut usize, b: &[u8], segs: &[Seg], f: &mut impl FnMut(Ev)) {
    let Some((s, resto)) = segs.split_first() else {
        f(evs[*i]);
        return pular(evs, i);
    };
    match (evs[*i], s) {
        (Ev::Obj, Seg::Campo(_) | Seg::Entrada) => {
            *i += 1;
            while let Ev::Chave(a, z) = evs[*i] {
                *i += 1;
                match s {
                    Seg::Campo(k) if b[a..z] != **k => pular(evs, i),
                    _ => coletar(evs, i, b, resto, f),
                }
            }
            *i += 1;
        }
        (Ev::Lista, Seg::Elemento) => {
            *i += 1;
            while evs[*i] != Ev::FimLista {
                coletar(evs, i, b, resto, f);
            }
            *i += 1;
        }
        _ => pular(evs, i),
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::estrutura::{MAPA_MIN_CHAVES, inferir_em_blocos};

    fn nao() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn perfil_de(dados: &[u8], c: &str, bloco: usize) -> Perfil {
        let i = inferir_em_blocos(dados, 7, &nao(), |_| {});
        let cam = i
            .resumo
            .caminhos()
            .into_iter()
            .find(|x| x.caminho == c)
            .unwrap_or_else(|| panic!("falta {c}"));
        perfil_em_blocos(&i, dados, &cam, bloco, &nao())
    }

    const BASE: &[u8] = b"{\"v\":-5}\n\n{\"v\":0}\n{\"w\":1}\nlixo\n{\"v\":0.5}\n{\"v\":120}\n{\"v\":\"b\"}\n{\"v\":\"a\"}\n{\"v\":null}\n{\"v\":120}\n";

    #[test]
    fn estatisticas_de_numeros_e_textos() {
        let p = perfil_de(BASE, ".v", 7);
        assert_eq!((p.registros, p.com, p.sem, p.valores), (9, 8, 1, 8));
        let n = p.numeros.as_ref().unwrap();
        assert_eq!((n.min.as_str(), n.max.as_str()), ("-5", "120"));
        let f = |negativo, expoente, n| Faixa {
            negativo,
            expoente,
            n,
        };
        assert_eq!(
            n.faixas,
            [
                f(true, Some(0), 1),
                f(false, None, 1),
                f(false, Some(-1), 1),
                f(false, Some(2), 2)
            ]
        );
        let t = p.textos.as_ref().unwrap();
        assert_eq!((t.min.as_str(), t.max.as_str()), ("\"a\"", "\"b\""));
        assert_eq!((p.distintos, p.distintos_aprox), (7, false));
        assert_eq!(p.comuns[0], ("120".into(), 2));
        assert_eq!(p.comuns.len(), 7);
        assert!(!p.comuns_aprox && !p.oculto && !p.cancelado);
    }

    #[test]
    fn registros_com_e_sem_contam_linha_em_branco_e_invalida_fora() {
        let p = perfil_de(BASE, ".v", 7);
        assert_eq!(p.linhas_com, [0, 2, 5, 6, 7, 8, 9, 10]);
        assert_eq!(p.linhas_sem, [3]);
        let w = perfil_de(BASE, ".w", 7);
        assert_eq!((w.com, w.sem, w.linhas_com.as_slice()), (1, 8, &[3][..]));
    }

    #[test]
    fn mesmo_resultado_em_qualquer_bloco() {
        let dados: String = (0..300)
            .map(|i| match i % 4 {
                0 => format!("{{\"v\":{}}}\n", i % 17),
                1 => format!("{{\"v\":\"t{}\"}}\n", i % 5),
                2 => "{\"x\":1}\n".into(),
                _ => format!("{{\"v\":[{i}]}}\n"),
            })
            .collect();
        let base = perfil_de(dados.as_bytes(), ".v", usize::MAX);
        for bloco in [1, 7, 64, 1000] {
            assert_eq!(
                perfil_de(dados.as_bytes(), ".v", bloco),
                base,
                "bloco {bloco}"
            );
        }
        assert_eq!(base.sem, 75);
    }

    #[test]
    fn caminho_aninhado_em_lista_e_mapa() {
        let d = b"{\"a\":[{\"b\":1},{\"c\":2},{\"b\":3}],\"m\":{\"x\":{\"k\":1},\"y\":{\"k\":2}}}\n{\"a\":[]}\n{\"a\":{\"b\":9}}\n";
        let p = perfil_de(d, ".a[].b", 7);
        assert_eq!((p.registros, p.com, p.sem, p.valores), (3, 1, 2, 2));
        let n = p.numeros.unwrap();
        assert_eq!(
            (n.min.as_str(), n.max.as_str()),
            ("1", "3"),
            "objeto em .a não é lista"
        );
        // Mapa: chaves variadas (uma por registro) viram `.*`.
        let m: String = (0..MAPA_MIN_CHAVES * 2)
            .map(|i| format!("{{\"m\":{{\"id{i}\":{{\"k\":{}}}}}}}\n", i % 3))
            .chain(["{\"m\":{}}\n".to_string()])
            .collect();
        let i = inferir_em_blocos(m.as_bytes(), 7, &nao(), |_| {});
        let alvo = [
            Seg::Campo((*b"m").into()),
            Seg::Entrada,
            Seg::Campo((*b"k").into()),
        ];
        let cam = i.resumo.caminhos().into_iter().find(|c| c.segs == alvo);
        let p = perfil_em_blocos(&i, m.as_bytes(), &cam.expect(".m.*.k"), 7, &nao());
        let n = MAPA_MIN_CHAVES as u64 * 2;
        assert_eq!((p.com, p.sem, p.valores, p.distintos), (n, 1, n, 3));
        assert_eq!(p.linhas_sem, [n]);
    }

    #[test]
    fn composto_conta_como_valor_sem_estatistica() {
        let p = perfil_de(b"{\"v\":{\"a\":1}}\n{\"v\":[1,2]}\n", ".v", 7);
        assert_eq!((p.com, p.valores, p.distintos), (2, 2, 0));
        assert!(p.numeros.is_none() && p.comuns.is_empty());
    }

    #[test]
    fn cardinalidade_exata_ate_k_e_estimada_acima() {
        let gerar = |n: usize| -> String { (0..n).map(|i| format!("{{\"v\":{i}}}\n")).collect() };
        let p = perfil_de(gerar(K_DISTINTOS).as_bytes(), ".v", 4096);
        assert_eq!(
            (p.distintos, p.distintos_aprox),
            (K_DISTINTOS as u64, false)
        );
        let n = 50_000;
        let p = perfil_de(gerar(n).as_bytes(), ".v", 4096);
        assert!(p.distintos_aprox);
        let erro = (p.distintos as f64 - n as f64).abs() / n as f64;
        assert!(erro < 0.1, "estimou {} para {n}", p.distintos);
        // Valores repetidos não inflam a estimativa.
        let rep: String = (0..n).map(|i| format!("{{\"v\":{}}}\n", i % 100)).collect();
        assert_eq!(perfil_de(rep.as_bytes(), ".v", 4096).distintos, 100);
    }

    #[test]
    fn comuns_aproximados_mantem_o_frequente() {
        let mut d = String::new();
        for i in 0..6000 {
            d.push_str(&format!("{{\"v\":\"u{i}\"}}\n"));
            if i % 2 == 0 {
                d.push_str("{\"v\":\"frequente\"}\n");
            }
        }
        let p = perfil_de(d.as_bytes(), ".v", 2048);
        assert!(p.comuns_aprox);
        let (v, n) = &p.comuns[0];
        assert_eq!(v, "\"frequente\"");
        // Misra–Gries: limite inferior, erro ≤ total/(CAP+1).
        let total = 9000;
        assert!(
            *n <= 3000 && *n + total / (CAP_COMUNS as u64 + 1) * 2 >= 3000,
            "{n}"
        );
        assert!(p.comuns.len() <= TOP);
    }

    /// Acumulador de um bloco só, com os textos dados como valores.
    fn acum_com(vals: impl IntoIterator<Item = String>) -> Acum {
        let mut a = Acum::new(false);
        for v in vals {
            let b = format!("\"{v}\"");
            a.valor(Ev::Texto(1, b.len() - 1), b.as_bytes());
        }
        a
    }

    #[test]
    fn memoria_de_um_bloco_tem_teto() {
        // Sem `combinar`: o teto tem de valer dentro do próprio bloco.
        let a = acum_com((0..5000).map(|i| format!("u{i}")));
        assert_eq!(a.hashes.len(), K_DISTINTOS);
        assert!(a.cortou_hashes);
        assert!(a.comuns.len() <= 2 * CAP_COMUNS, "{}", a.comuns.len());
        assert!(a.reduziu_comuns);
    }

    #[test]
    fn valor_enorme_e_guardado_cortado() {
        // O que fica na memória do bloco (não só o que sai no perfil) tem teto por valor.
        let a = acum_com(["a".repeat(100_000), "b".repeat(100_000)]);
        let (lo, hi) = a.txt.as_ref().unwrap();
        assert_eq!((lo.len(), hi.len()), (VALOR_MAX + 1, VALOR_MAX + 1));
        assert!(a.comuns.keys().all(|k| k.len() == VALOR_MAX + 1));
    }

    #[test]
    fn combinar_tambem_tem_teto() {
        let mut a = acum_com((0..1500).map(|i| format!("a{i}")));
        a.combinar(acum_com((0..1500).map(|i| format!("b{i}"))));
        assert!(a.comuns.len() <= 2 * CAP_COMUNS, "{}", a.comuns.len());
        assert!(a.reduziu_comuns);
        let mut h = acum_com((0..3000).map(|i| format!("a{i}")));
        h.combinar(acum_com((0..3000).map(|i| format!("b{i}"))));
        assert_eq!(h.hashes.len(), K_DISTINTOS);
        assert!(h.cortou_hashes);
    }

    #[test]
    fn sem_corte_a_contagem_dos_comuns_e_exata() {
        // 1500 distintos cabem no lote (2·CAP): nada é reduzido, nem no fim.
        let mut d = String::new();
        for i in 0..1500 {
            d.push_str(&format!("{{\"v\":\"k{i}\"}}\n"));
        }
        d.push_str(&"{\"v\":\"k0\"}\n".repeat(50));
        let p = perfil_de(d.as_bytes(), ".v", 4096);
        assert!(!p.comuns_aprox);
        assert_eq!(p.comuns[0], ("\"k0\"".to_string(), 51));
    }

    #[test]
    fn valor_enorme_sai_cortado() {
        let grande = |c: char| c.to_string().repeat(5_000_000);
        let num = format!("1{}", "0".repeat(3000));
        let d = format!(
            "{{\"v\":\"{}\"}}\n{{\"v\":\"{}\"}}\n{{\"v\":{num}}}\n{{\"v\":-{num}}}\n",
            grande('a'),
            grande('b')
        );
        let p = perfil_de(d.as_bytes(), ".v", 1 << 20);
        let t = p.textos.as_ref().unwrap();
        let n = p.numeros.as_ref().unwrap();
        for s in [&t.min, &t.max, &n.min, &n.max] {
            assert!(s.len() <= VALOR_MAX + '…'.len_utf8(), "{}", s.len());
            assert!(s.ends_with('…'));
        }
        // O corte não troca a ordem nem o valor do número.
        assert!(t.min.starts_with("\"aaa") && t.max.starts_with("\"bbb"));
        assert!(n.min.starts_with("-1000") && n.max.starts_with("1000"));
        assert_eq!(
            n.faixas.len(),
            2,
            "faixa pelo número inteiro: {:?}",
            n.faixas
        );
        assert!(p.comuns.iter().all(|(v, _)| v.len() <= VALOR_MAX + 3));
        // Valor curto não ganha `…`.
        let p = perfil_de(BASE, ".v", 7);
        assert!(!p.textos.unwrap().max.contains('…'));
    }

    #[test]
    fn listas_de_linhas_param_no_teto_e_contagem_segue() {
        let d: String = (0..LINHAS_MAX + 500).map(|_| "{\"v\":1}\n").collect();
        let p = perfil_de(d.as_bytes(), ".v", 333);
        assert_eq!(p.com, (LINHAS_MAX + 500) as u64);
        assert_eq!(p.linhas_com, (0..LINHAS_MAX as u64).collect::<Vec<_>>());
    }

    #[test]
    fn json_unico_usa_a_linha_do_elemento() {
        let d = b"[\n {\"v\":1},\n {\"w\":2},\n {\"v\":3}\n]\n";
        let p = perfil_de(d, ".v", 7);
        assert_eq!((p.registros, p.com, p.sem), (3, 2, 1));
        assert_eq!(
            (p.linhas_com.as_slice(), p.linhas_sem.as_slice()),
            (&[1, 3][..], &[2][..])
        );
    }

    #[test]
    fn segredo_nao_sai_do_perfil() {
        let d = b"{\"senha\":\"hunter2-muito-secreta\"}\n{\"senha\":\"outra\"}\n{\"x\":1}\n";
        let p = perfil_de(d, ".senha", 7);
        assert!(p.oculto);
        assert_eq!((p.com, p.sem, p.distintos), (2, 1, 2));
        assert!(p.textos.is_none() && p.numeros.is_none() && p.comuns.is_empty());
        let tudo = format!("{p:?}{}", serde_json::to_string(&p).unwrap());
        assert!(!tudo.contains("hunter2") && !tudo.contains("outra"));
    }

    #[test]
    fn cancelado_avisa() {
        let i = inferir_em_blocos(BASE, 7, &nao(), |_| {});
        let cam = i
            .resumo
            .caminhos()
            .into_iter()
            .find(|c| c.caminho == ".v")
            .unwrap();
        let p = perfil_em_blocos(&i, BASE, &cam, 7, &AtomicBool::new(true));
        assert!(p.cancelado);
    }

    #[test]
    fn faixa_ordena_por_grandeza() {
        assert_eq!(faixa(0.0), 0);
        assert!(faixa(-100.0) < faixa(-1.0) && faixa(-1.0) < 0);
        assert!(0 < faixa(0.01) && faixa(0.01) < faixa(9.9) && faixa(9.9) < faixa(10.0));
        assert_eq!(faixa(1e300), faixa(1e31));
    }
}
