//! Transformar e salvar como: grava num arquivo NOVO o que o usuário vê (resultado do jq, filtro
//! por variante ou caminho, ou o arquivo inteiro), com transformações simples (manter, tirar e
//! renomear campos), sem nunca tocar no original. Ver `docs/decisoes.md`, "Salvar como".
//!
//! Tudo vira uma expressão jq só (`compor`) e roda no motor da consulta (`consulta::Consulta`):
//! não há segundo motor. A saída é escrita em fluxo (`Escritor`) num temporário ao lado do
//! destino, que só troca de nome no fim (`gravar_atomico`): cancelar ou falhar não deixa nada no
//! destino, e o arquivo aberto nunca é destino (`destino_protegido`).
//!
//! A entrada sai mascarada por padrão (`Mascara`): segredo cru só com escolha explícita.

use std::borrow::Cow;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::ArquivoLinhas;
use crate::caminho::texto_json;
use crate::consulta::{Consulta, LoteConsulta, ResumoConsulta};
use crate::estrutura::Seg;
use crate::filtro::Criterio;

/// Transformações simples, nos campos do topo de cada registro (objeto; o resto passa igual).
/// Ordem: `manter`, depois `tirar`, depois `renomear`, sempre pelos nomes originais.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transformacao {
    /// Só estes campos ficam (vazio: todos).
    #[serde(default)]
    pub manter: Vec<String>,
    #[serde(default)]
    pub tirar: Vec<String>,
    /// (de, para). Se `para` já existe no registro (e não é renomeado também), fica o valor
    /// renomeado, qualquer que seja a ordem dos campos. Dois pares com o mesmo `de` ou o mesmo
    /// `para` são recusados em `conferir`.
    #[serde(default)]
    pub renomear: Vec<(String, String)>,
}

/// `{"a":1,"b":1}`: conjunto de nomes para `has($k)`.
fn conjunto(nomes: &[String]) -> String {
    let itens: Vec<String> = nomes
        .iter()
        .map(|n| format!("{}:1", texto_json(n)))
        .collect();
    format!("{{{}}}", itens.join(","))
}

impl Transformacao {
    /// Recusa o renomear ambíguo: o mesmo campo renomeado duas vezes, ou dois campos com o
    /// mesmo nome novo (um dos valores sumiria).
    pub fn conferir(&self) -> Result<(), String> {
        let mut de = std::collections::HashSet::new();
        let mut para = std::collections::HashSet::new();
        for (d, p) in &self.renomear {
            if !de.insert(d) {
                return Err(format!("renomear: \"{d}\" aparece duas vezes"));
            }
            if !para.insert(p) {
                return Err(format!("renomear: dois campos viram \"{p}\""));
            }
        }
        Ok(())
    }

    /// A expressão jq, ou `None` se não transforma nada.
    pub fn jq(&self) -> Option<String> {
        let mut passos = Vec::new();
        if !self.manter.is_empty() {
            passos.push(format!(
                "with_entries(select(.key as $k | {} | has($k)))",
                conjunto(&self.manter)
            ));
        }
        if !self.tirar.is_empty() {
            passos.push(format!(
                "with_entries(select(.key as $k | {} | has($k) | not))",
                conjunto(&self.tirar)
            ));
        }
        if !self.renomear.is_empty() {
            let mapa: Vec<String> = self
                .renomear
                .iter()
                .map(|(de, para)| format!("{}:{}", texto_json(de), texto_json(para)))
                .collect();
            // O campo que já tem o nome novo (e não sai dali renomeado) dá lugar ao renomeado,
            // só quando o renomeado existe no registro.
            let de: Vec<&String> = self.renomear.iter().map(|(d, _)| d).collect();
            for (d, p) in self.renomear.iter().filter(|(_, p)| !de.contains(&p)) {
                let (d, p) = (texto_json(d), texto_json(p));
                passos.push(format!("if has({d}) then del(.[{p}]) else . end"));
            }
            passos.push(format!(
                "with_entries(.key |= ({{{}}}[.] // .))",
                mapa.join(",")
            ));
        }
        (!passos.is_empty()).then(|| {
            format!(
                "if type == \"object\" then {} else . end",
                passos.join(" | ")
            )
        })
    }
}

/// Texto JSON cru (sem aspas, escapes intactos) decodificado; escape inválido fica como está.
fn decodificar(cru: &[u8]) -> String {
    let mut s = Vec::with_capacity(cru.len() + 2);
    s.push(b'"');
    s.extend_from_slice(cru);
    s.push(b'"');
    serde_json::from_slice(&s).unwrap_or_else(|_| String::from_utf8_lossy(cru).into_owned())
}

/// Gerador jq dos valores no caminho (vazio se não há nenhum), na semântica de `filtro`:
/// campo só em objeto que tem a chave, `.*` só em objeto, `[]` só em lista.
fn caminho_jq(segs: &[Seg]) -> String {
    let mut s = String::from(".");
    for seg in segs {
        match seg {
            Seg::Campo(k) => {
                let t = texto_json(&decodificar(k));
                s.push_str(&format!(" | objects | select(has({t})) | .[{t}]"));
            }
            Seg::Entrada => s.push_str(" | objects | .[]"),
            Seg::Elemento => s.push_str(" | arrays | .[]"),
        }
    }
    s
}

/// `select` que deixa passar o registro que passa no `criterio` de `filtro::filtrar`.
/// Erro quando o critério não tem tradução fiel (valor comum cortado pelo perfil).
pub fn criterio_jq(criterio: &Criterio) -> Result<String, String> {
    let primeiro = |segs: &[Seg]| format!("[limit(1; {})]", caminho_jq(segs));
    Ok(match criterio {
        Criterio::Tem(segs) => format!("select({} != [])", primeiro(segs)),
        Criterio::NaoTem(segs) => format!("select({} == [])", primeiro(segs)),
        Criterio::Igual(segs, valor) => format!(
            "select({} == [{}])",
            primeiro(segs),
            texto_json(&decodificar(valor))
        ),
        Criterio::Comum(segs, comum) => {
            // shortcut: o jq compara o valor decodificado, então grafias diferentes do mesmo
            // valor (1.5 e 1.50, "é" e "\u00e9") passam juntas aqui e ficam separadas na Tabela.
            let v: serde_json::Value = serde_json::from_str(comum)
                .map_err(|_| "valor cortado: salve pela consulta jq".to_owned())?;
            format!("select(any({}; . == {v}))", caminho_jq(segs))
        }
        Criterio::Faixa(segs, negativo, expoente) => {
            // A faixa de `perfil`: 10^e ≤ |x| < 10^(e+1), com e preso entre −30 e 30.
            let cond = match expoente {
                None => ". == 0".to_owned(),
                Some(e) => {
                    let (sinal, m) = if *negativo { ("<", -1) } else { (">", 1) };
                    let mut c = format!(". {sinal} 0 and ((. * {m}) as $a | true");
                    if *e > -30 {
                        c.push_str(&format!(" and $a >= 1e{e}"));
                    }
                    if *e < 30 {
                        c.push_str(&format!(" and $a < 1e{}", e + 1));
                    }
                    c + ")"
                }
            };
            format!(
                "select(any({}; type == \"number\" and {cond}))",
                caminho_jq(segs)
            )
        }
    })
}

/// De onde vêm os registros salvos. Só texto: viaja ao processo filho, que decide `documento`
/// pelo arquivo que ele mesmo abre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", content = "jq", rename_all = "snake_case")]
pub enum Fonte {
    /// O arquivo inteiro.
    Tudo,
    /// O resultado deste filtro jq (o que a consulta mostra).
    Jq(String),
    /// Os registros que passam neste `select` (ver `Fonte::filtro`).
    Selecao(String),
}

impl Fonte {
    /// Os registros que passam no filtro da Tabela (`criterio_jq`).
    pub fn filtro(c: &Criterio) -> Result<Self, String> {
        criterio_jq(c).map(Fonte::Selecao)
    }
}

/// A expressão jq que produz o que se salva. `documento`: JSON único (a consulta roda no
/// documento inteiro); registros são os elementos da lista do topo, ou o objeto do topo, como em
/// `estrutura::registros_json_unico`.
pub fn compor(fonte: &Fonte, t: &Transformacao, documento: bool) -> String {
    let t = t.jq();
    match fonte {
        // Quebra de linha antes do `)`: comentário no fim do filtro não o engole.
        Fonte::Jq(f) => match t {
            Some(t) => format!("(\n{f}\n) | {t}"),
            None => f.to_owned(),
        },
        Fonte::Tudo if t.is_none() => ".".to_owned(),
        Fonte::Tudo | Fonte::Selecao(_) => {
            let mut passos: Vec<String> = Vec::new();
            if let Fonte::Selecao(c) = fonte {
                passos.push(c.clone());
            }
            passos.extend(t);
            let registro = passos.join(" | ");
            if documento {
                format!("if type == \"array\" then [.[] | {registro}] else {registro} end")
            } else {
                registro
            }
        }
    }
}

/// Formato do arquivo salvo (CSV sai pelo exportar do resultado da consulta).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Saida {
    /// Um resultado por linha, compacto.
    #[default]
    Jsonl,
    /// JSON formatado (dois espaços): um resultado sai sozinho; vários, numa lista.
    Json,
    /// O mesmo, numa linha só.
    JsonCompacto,
}

impl Saida {
    pub fn extensao(self) -> &'static str {
        match self {
            Saida::Jsonl => "jsonl",
            Saida::Json | Saida::JsonCompacto => "json",
        }
    }
}

/// Escreve os resultados (JSON compacto, um por chamada) no formato da `Saida`, em fluxo: na
/// memória só o 1º resultado, até saber se haverá lista.
pub struct Escritor<W: Write> {
    saida: Saida,
    out: W,
    n: u64,
    primeiro: Option<String>,
}

impl<W: Write> Escritor<W> {
    pub fn novo(saida: Saida, out: W) -> Self {
        Self {
            saida,
            out,
            n: 0,
            primeiro: None,
        }
    }

    fn item(&mut self, json: &str, primeiro: bool) -> io::Result<()> {
        if self.saida == Saida::JsonCompacto {
            if !primeiro {
                self.out.write_all(b",")?;
            }
            return self.out.write_all(json.as_bytes());
        }
        self.out
            .write_all(if primeiro { b"\n  " } else { b",\n  " })?;
        let f = crate::arvore::formatar(json, usize::MAX).texto;
        // Texto JSON não tem quebra de linha crua: toda quebra é de indentação.
        self.out.write_all(f.replace('\n', "\n  ").as_bytes())
    }

    pub fn escrever(&mut self, json: &str) -> io::Result<()> {
        self.n += 1;
        if self.saida == Saida::Jsonl {
            self.out.write_all(json.as_bytes())?;
            return self.out.write_all(b"\n");
        }
        match self.n {
            1 => self.primeiro = Some(json.to_owned()),
            2 => {
                self.out.write_all(b"[")?;
                let p = self.primeiro.take().unwrap_or_default();
                self.item(&p, true)?;
                self.item(json, false)?;
            }
            _ => self.item(json, false)?,
        }
        Ok(())
    }

    /// Fecha a lista (ou escreve o resultado único; nenhum resultado em JSON vira `[]`) e
    /// devolve o escritor de baixo, já com `flush`.
    pub fn terminar(mut self) -> io::Result<W> {
        match (self.saida, self.n) {
            (Saida::Jsonl, _) => {}
            (_, 0) => self.out.write_all(b"[]\n")?,
            (s, 1) => {
                let p = self.primeiro.take().unwrap_or_default();
                let p = if s == Saida::Json {
                    crate::arvore::formatar(&p, usize::MAX).texto
                } else {
                    p
                };
                self.out.write_all(p.as_bytes())?;
                self.out.write_all(b"\n")?;
            }
            (Saida::Json, _) => self.out.write_all(b"\n]\n")?,
            _ => self.out.write_all(b"]\n")?,
        }
        self.out.flush()?;
        Ok(self.out)
    }
}

/// O que acontece com os segredos (`mascara`) no arquivo salvo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mascara {
    /// Padrão: mascarados como na tela.
    #[default]
    Mascarar,
    /// Trocados por um código estável com o sal (`mascara::anonimizar`).
    Anonimizar,
    /// Como estão no arquivo. Só por escolha explícita (a interface avisa).
    SemMascara,
}

/// A entrada da consulta conforme a `mascara`. `sal` só vale para `Anonimizar`.
pub fn entrada(a: &ArquivoLinhas, mascara: Mascara, sal: u64) -> Cow<'_, [u8]> {
    match mascara {
        Mascara::Mascarar => Cow::Borrowed(a.bytes()),
        Mascara::Anonimizar => Cow::Owned(crate::mascara::anonimizar(a.cru(), a.indice(), sal)),
        Mascara::SemMascara => Cow::Borrowed(a.cru()),
    }
}

#[derive(Debug)]
pub enum ErroSalvar {
    /// O destino é um arquivo aberto (o original, ou outro protegido).
    Protegido(PathBuf),
    Cancelado,
    /// O filtro composto não compila.
    Filtro(String),
    Io(io::Error),
}

impl fmt::Display for ErroSalvar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErroSalvar::Protegido(p) => write!(
                f,
                "{}: é um arquivo aberto no jasão (o original não é sobrescrito); salve com outro nome",
                p.display()
            ),
            ErroSalvar::Cancelado => write!(f, "cancelado; nada foi gravado"),
            ErroSalvar::Filtro(m) => write!(f, "{m}"),
            ErroSalvar::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ErroSalvar {}

impl From<io::Error> for ErroSalvar {
    fn from(e: io::Error) -> Self {
        ErroSalvar::Io(e)
    }
}

/// Se `destino` é um dos `protegidos`: mesmo caminho real (link resolvido) ou, no Unix, o mesmo
/// arquivo (`dev`, `ino`; pega link físico). Destino que não existe não é nenhum deles.
pub fn destino_protegido(destino: &Path, protegidos: &[PathBuf]) -> bool {
    let Ok(real) = std::fs::canonicalize(destino) else {
        return false;
    };
    #[cfg(unix)]
    let id = |p: &Path| {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
    };
    #[cfg(not(unix))]
    let id = |_: &Path| None::<(u64, u64)>;
    let alvo = id(&real);
    protegidos.iter().any(|p| {
        std::fs::canonicalize(p).is_ok_and(|c| c == real) || (alvo.is_some() && id(p) == alvo)
    })
}

/// Temporário (`.jasao-salvar-*`) na pasta do destino, para gravar e depois `persistir`. Recusa
/// destino protegido. Solto sem `persistir`, o temporário é apagado.
pub fn temporario(
    destino: &Path,
    protegidos: &[PathBuf],
) -> Result<tempfile::NamedTempFile, ErroSalvar> {
    if destino_protegido(destino, protegidos) {
        return Err(ErroSalvar::Protegido(destino.to_owned()));
    }
    let pasta = match destino.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    Ok(tempfile::Builder::new()
        .prefix(".jasao-salvar-")
        .tempfile_in(pasta)?)
}

/// Troca o temporário pelo destino com `rename` (atômico na mesma pasta). Confere de novo os
/// protegidos: o destino pode ter virado um deles enquanto gravava. `cru` (sem máscara): o
/// arquivo fica 0600, só do dono; mascarado ou anonimizado fica 0644, como um arquivo novo comum.
pub fn persistir(
    tmp: tempfile::NamedTempFile,
    destino: &Path,
    protegidos: &[PathBuf],
    cru: bool,
) -> Result<(), ErroSalvar> {
    if destino_protegido(destino, protegidos) {
        return Err(ErroSalvar::Protegido(destino.to_owned()));
    }
    // O temporário nasce 0600 (tempfile); só o mascarado abre para 0644.
    #[cfg(unix)]
    if !cru {
        use std::os::unix::fs::PermissionsExt;
        let _ = tmp
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o644));
    }
    tmp.as_file().sync_all()?;
    tmp.persist(destino).map_err(|e| ErroSalvar::Io(e.error))?;
    Ok(())
}

/// Grava em `destino` o que `escrever` põe no arquivo, por `temporario` e `persistir`: erro (ou
/// `Cancelado`) no meio não deixa nada no destino, e o que já existia lá fica intacto.
pub fn gravar_atomico<T>(
    destino: &Path,
    protegidos: &[PathBuf],
    cru: bool,
    escrever: impl FnOnce(&mut File) -> Result<T, ErroSalvar>,
) -> Result<T, ErroSalvar> {
    let mut tmp = temporario(destino, protegidos)?;
    let r = escrever(tmp.as_file_mut())?;
    persistir(tmp, destino, protegidos, cru)?;
    Ok(r)
}

/// Roda `expressao` sobre `entrada` e escreve os resultados em `out` no formato `saida`, sem
/// teto de saída (é o arquivo pedido). `ao_lote` recebe (linhas varridas, total, resultados
/// gravados). Cancelado: `Err(Cancelado)`, e quem chamou descarta o que foi escrito.
pub fn escrever(
    entrada: &[u8],
    documento: bool,
    expressao: &str,
    saida: Saida,
    cancelar: &AtomicBool,
    out: impl Write,
    mut ao_lote: impl FnMut(usize, usize, u64),
) -> Result<ResumoConsulta, ErroSalvar> {
    use std::sync::atomic::Ordering::Relaxed;
    let consulta = Consulta::nova(expressao).map_err(|e| ErroSalvar::Filtro(e.to_string()))?;
    let mut esc = Escritor::novo(saida, io::BufWriter::new(out));
    let mut erro = None;
    let mut gravados = 0u64;
    let lote = |l: LoteConsulta| {
        for r in &l.resultados {
            if let Err(e) = esc.escrever(&r.json) {
                erro.get_or_insert(e);
                cancelar.store(true, Relaxed);
                return;
            }
            gravados += 1;
        }
        ao_lote(l.linhas, l.total_linhas, gravados);
    };
    let resumo = if documento {
        consulta.executar_documento(entrada, cancelar, lote)
    } else {
        consulta.executar_jsonl(entrada, cancelar, lote)
    };
    if let Some(e) = erro {
        return Err(e.into());
    }
    if resumo.cancelado {
        return Err(ErroSalvar::Cancelado);
    }
    esc.terminar()?;
    Ok(resumo)
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::sync::atomic::Ordering::Relaxed;

    fn rodar(entrada: &[u8], documento: bool, expr: &str, saida: Saida) -> String {
        let mut out = Vec::new();
        escrever(
            entrada,
            documento,
            expr,
            saida,
            &AtomicBool::new(false),
            &mut out,
            |_, _, _| {},
        )
        .unwrap();
        String::from_utf8(out).unwrap()
    }

    fn jsonl(entrada: &[u8], fonte: Fonte, t: &Transformacao) -> String {
        rodar(entrada, false, &compor(&fonte, t, false), Saida::Jsonl)
    }

    const DADOS: &[u8] = b"{\"a\":1,\"b\":2,\"c\":3}\n[1]\n{\"a\":4,\"d\":{\"e\":5}}\n";

    #[test]
    fn manter() {
        let t = Transformacao {
            manter: vec!["a".into(), "c".into()],
            ..Default::default()
        };
        assert_eq!(
            jsonl(DADOS, Fonte::Tudo, &t),
            "{\"a\":1,\"c\":3}\n[1]\n{\"a\":4}\n"
        );
    }

    #[test]
    fn tirar() {
        let t = Transformacao {
            tirar: vec!["a".into(), "d".into()],
            ..Default::default()
        };
        assert_eq!(
            jsonl(DADOS, Fonte::Tudo, &t),
            "{\"b\":2,\"c\":3}\n[1]\n{}\n"
        );
    }

    #[test]
    fn renomear() {
        let t = Transformacao {
            renomear: vec![("a".into(), "x".into()), ("b".into(), "a".into())],
            ..Default::default()
        };
        // Pelos nomes originais: `b` vira `a` mesmo com `a` virando `x`.
        assert_eq!(
            jsonl(DADOS, Fonte::Tudo, &t),
            "{\"x\":1,\"a\":2,\"c\":3}\n[1]\n{\"x\":4,\"d\":{\"e\":5}}\n"
        );
    }

    /// `a=b` com `b` já no registro: fica o valor de `a`, em qualquer ordem dos campos; sem `a`
    /// no registro, `b` fica como está. Pares ambíguos são recusados antes.
    #[test]
    fn renomear_sobre_campo_existente() {
        let t = Transformacao {
            renomear: vec![("a".into(), "b".into())],
            ..Default::default()
        };
        assert_eq!(
            jsonl(
                b"{\"a\":1,\"b\":2}\n{\"b\":2,\"a\":1}\n{\"b\":3}\n",
                Fonte::Tudo,
                &t
            ),
            "{\"b\":1}\n{\"b\":1}\n{\"b\":3}\n"
        );
        assert!(t.conferir().is_ok());
        let dois = |a: (&str, &str), b: (&str, &str)| Transformacao {
            renomear: vec![(a.0.into(), a.1.into()), (b.0.into(), b.1.into())],
            ..Default::default()
        };
        assert!(dois(("a", "x"), ("b", "x")).conferir().is_err());
        assert!(dois(("a", "x"), ("a", "y")).conferir().is_err());
        assert!(dois(("a", "b"), ("b", "a")).conferir().is_ok());
    }

    #[test]
    fn nomes_com_aspas_e_barra_nao_injetam() {
        let t = Transformacao {
            tirar: vec!["a\") | halt_error(\"x".into()],
            renomear: vec![("b".into(), "nó \"novo\"\\".into())],
            ..Default::default()
        };
        assert_eq!(
            jsonl(b"{\"a\":1,\"b\":2}\n", Fonte::Tudo, &t),
            "{\"a\":1,\"nó \\\"novo\\\"\\\\\":2}\n"
        );
    }

    #[test]
    fn combinado_na_ordem() {
        let t = Transformacao {
            manter: vec!["a".into(), "b".into(), "c".into()],
            tirar: vec!["c".into()],
            renomear: vec![("a".into(), "z".into())],
        };
        assert_eq!(
            jsonl(DADOS, Fonte::Tudo, &t),
            "{\"z\":1,\"b\":2}\n[1]\n{\"z\":4}\n"
        );
    }

    #[test]
    fn fonte_jq_e_transformacao_depois() {
        let t = Transformacao {
            tirar: vec!["b".into()],
            ..Default::default()
        };
        // Comentário no fim do filtro não engole o resto.
        let f = "select(type == \"object\") | {a, b} # comentário";
        assert_eq!(
            jsonl(DADOS, Fonte::Jq(f.into()), &t),
            "{\"a\":1}\n{\"a\":4}\n"
        );
        assert_eq!(
            jsonl(DADOS, Fonte::Jq(".a?".into()), &Transformacao::default()),
            "1\n4\n"
        );
    }

    #[test]
    fn fonte_filtro_como_no_nucleo() {
        let d = [
            Seg::Campo(Box::from(&b"d"[..])),
            Seg::Campo(Box::from(&b"e"[..])),
        ];
        let t = Transformacao::default();
        assert_eq!(
            jsonl(DADOS, Fonte::filtro(&Criterio::Tem(&d)).unwrap(), &t),
            "{\"a\":4,\"d\":{\"e\":5}}\n"
        );
        assert_eq!(
            jsonl(DADOS, Fonte::filtro(&Criterio::NaoTem(&d)).unwrap(), &t),
            "{\"a\":1,\"b\":2,\"c\":3}\n[1]\n"
        );
        // `null` explícito conta como valor presente (como em `filtro`).
        let a = [Seg::Campo(Box::from(&b"a"[..]))];
        assert_eq!(
            jsonl(
                b"{\"a\":null}\n{}\n",
                Fonte::filtro(&Criterio::Tem(&a)).unwrap(),
                &t
            ),
            "{\"a\":null}\n"
        );
        let tipo = [Seg::Campo(Box::from(&b"tipo"[..]))];
        let v = b"{\"tipo\":\"x\\u00e9\"}\n{\"tipo\":\"y\"}\n{\"tipo\":1}\n";
        assert_eq!(
            jsonl(
                v,
                Fonte::filtro(&Criterio::Igual(&tipo, b"x\\u00e9")).unwrap(),
                &t
            ),
            "{\"tipo\":\"xé\"}\n"
        );
        let itens = [
            Seg::Campo(Box::from(&b"l"[..])),
            Seg::Elemento,
            Seg::Campo(Box::from(&b"k"[..])),
        ];
        assert_eq!(
            jsonl(
                b"{\"l\":[{},{\"k\":0}]}\n{\"l\":{\"k\":0}}\n",
                Fonte::filtro(&Criterio::Tem(&itens)).unwrap(),
                &t
            ),
            "{\"l\":[{},{\"k\":0}]}\n"
        );
    }

    /// Faceta da Estrutura (valor comum, faixa) salva os mesmos registros que a Tabela filtra.
    #[test]
    fn fonte_filtro_de_faceta() {
        let t = Transformacao::default();
        let n = [Seg::Campo(Box::from(&b"n"[..]))];
        let dados = b"{\"n\":\"a\\\"b\"}\n{\"n\":\"a\\\\(b\"}\n{\"n\":null}\n{\"n\":[1.5,7]}\n\
{\"n\":-0.5}\n{\"n\":0}\n{\"n\":10}\n{\"n\":99}\n{\"n\":100}\n{\"n\":1e31}\n{}\n";
        let todas: Vec<&str> = std::str::from_utf8(dados).unwrap().lines().collect();
        let so = |c: Criterio| -> Vec<usize> {
            let r = jsonl(dados, Fonte::filtro(&c).unwrap(), &t);
            r.lines()
                .map(|l| todas.iter().position(|x| x.replace(' ', "") == l).unwrap())
                .collect()
        };
        assert_eq!(so(Criterio::Comum(&n, r#""a\"b""#)), [0]);
        assert_eq!(so(Criterio::Comum(&n, r#""a\\(b""#)), [1]);
        assert_eq!(so(Criterio::Comum(&n, "null")), [2]);
        let el = [Seg::Campo(Box::from(&b"n"[..])), Seg::Elemento];
        assert_eq!(so(Criterio::Comum(&el, "1.5")), [3]); // qualquer elemento da lista
        assert_eq!(so(Criterio::Faixa(&el, false, Some(0))), [3]); // 1.5 ou 7
        assert_eq!(so(Criterio::Faixa(&n, false, Some(1))), [6, 7]); // 10, 99
        assert_eq!(so(Criterio::Faixa(&n, false, Some(2))), [8]);
        assert_eq!(so(Criterio::Faixa(&n, true, Some(-1))), [4]);
        assert_eq!(so(Criterio::Faixa(&n, false, None)), [5]);
        assert_eq!(so(Criterio::Faixa(&n, false, Some(30))), [9]); // preso em 30
        // Valor cortado pelo perfil não tem tradução fiel: recusa em vez de salvar outro conjunto.
        assert!(Fonte::filtro(&Criterio::Comum(&n, "\"abc…")).is_err());
    }

    /// Chave e valor com `"` e `\(` (interpolação do jq) entram como texto JSON, não como código.
    #[test]
    fn filtro_escapa_chave_e_valor() {
        // No arquivo: {"k\"\\(x":"v\"\\(y"} — chave k"\(x, valor v"\(y.
        let dados = b"{\"k\\\"\\\\(x\":\"v\\\"\\\\(y\"}\n{\"k\\\"\\\\(x\":\"outro\"}\n{\"k\":1}\n";
        let chave = [Seg::Campo(Box::from(&br#"k\"\\(x"#[..]))];
        let t = Transformacao::default();
        let linhas = |s: &str| s.lines().map(str::to_owned).collect::<Vec<_>>();
        let todas = linhas(std::str::from_utf8(dados).unwrap());
        assert_eq!(
            linhas(&jsonl(
                dados,
                Fonte::filtro(&Criterio::Tem(&chave)).unwrap(),
                &t
            )),
            todas[..2]
        );
        assert_eq!(
            linhas(&jsonl(
                dados,
                Fonte::filtro(&Criterio::Igual(&chave, br#"v\"\\(y"#)).unwrap(),
                &t
            )),
            todas[..1]
        );
    }

    #[test]
    fn documento_registros_sao_os_elementos() {
        let t = Transformacao {
            tirar: vec!["b".into()],
            ..Default::default()
        };
        let lista = b"[{\"a\":1,\"b\":2},\n {\"a\":3}]";
        let tipo = [Seg::Campo(Box::from(&b"b"[..]))];
        let expr = compor(&Fonte::filtro(&Criterio::Tem(&tipo)).unwrap(), &t, true);
        assert_eq!(
            rodar(lista, true, &expr, Saida::JsonCompacto),
            "[{\"a\":1}]\n"
        );
        let expr = compor(&Fonte::Tudo, &t, true);
        assert_eq!(
            rodar(b"{\"a\":1,\"b\":2}", true, &expr, Saida::JsonCompacto),
            "{\"a\":1}\n"
        );
    }

    #[test]
    fn formatos_de_saida() {
        let um = b"{\"a\":[1,{}],\"b\":\"x\\ny\"}";
        assert_eq!(
            rodar(um, true, ".", Saida::Json),
            "{\n  \"a\": [\n    1,\n    {}\n  ],\n  \"b\": \"x\\ny\"\n}\n"
        );
        assert_eq!(
            rodar(um, true, ".", Saida::JsonCompacto),
            "{\"a\":[1,{}],\"b\":\"x\\ny\"}\n"
        );
        let varios = b"{\"a\":1}\n2\n";
        assert_eq!(
            rodar(varios, false, ".", Saida::Json),
            "[\n  {\n    \"a\": 1\n  },\n  2\n]\n"
        );
        assert_eq!(
            rodar(varios, false, ".", Saida::JsonCompacto),
            "[{\"a\":1},2]\n"
        );
        assert_eq!(rodar(varios, false, ".", Saida::Jsonl), "{\"a\":1}\n2\n");
        assert_eq!(rodar(varios, false, "empty", Saida::Json), "[]\n");
        assert_eq!(rodar(varios, false, "empty", Saida::Jsonl), "");
        // Lista formatada é JSON válido, e o mesmo valor.
        let v: serde_json::Value =
            serde_json::from_str(&rodar(varios, false, ".", Saida::Json)).unwrap();
        assert_eq!(v, serde_json::json!([{"a": 1}, 2]));
    }

    fn arquivo(conteudo: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("original.jsonl");
        std::fs::write(&p, conteudo).unwrap();
        (d, p)
    }

    fn residuos(pasta: &Path) -> Vec<String> {
        std::fs::read_dir(pasta)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".jasao-salvar-"))
            .collect()
    }

    #[test]
    fn recusa_o_original_por_qualquer_caminho() {
        let (d, orig) = arquivo(b"{\"a\":1}\n");
        let protegidos = [orig.clone()];
        let pela_pasta = d.path().join(".").join("original.jsonl");
        let ligado = d.path().join("link.jsonl");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&orig, &ligado).unwrap();
        let fisico = d.path().join("fisico.jsonl");
        std::fs::hard_link(&orig, &fisico).unwrap();
        for destino in [&orig, &pela_pasta, &ligado, &fisico] {
            let r = gravar_atomico(destino, &protegidos, false, |f| Ok(f.write_all(b"x")?));
            assert!(matches!(r, Err(ErroSalvar::Protegido(_))), "{destino:?}");
        }
        assert_eq!(std::fs::read(&orig).unwrap(), b"{\"a\":1}\n");
        assert!(residuos(d.path()).is_empty());
        // Outro nome na mesma pasta grava.
        let novo = d.path().join("novo.jsonl");
        gravar_atomico(&novo, &protegidos, false, |f| Ok(f.write_all(b"x")?)).unwrap();
        assert_eq!(std::fs::read(&novo).unwrap(), b"x");
    }

    /// Cada porta recusa sozinha: o temporário nem nasce, e o `rename` final confere de novo
    /// (o destino pode ter virado link para o original enquanto gravava).
    #[test]
    fn temporario_e_persistir_conferem_cada_um() {
        let (d, orig) = arquivo(b"{\"a\":1}\n");
        let protegidos = [orig.clone()];
        assert!(matches!(
            temporario(&orig, &protegidos),
            Err(ErroSalvar::Protegido(_))
        ));
        let depois = d.path().join("depois.jsonl");
        let mut tmp = temporario(&depois, &protegidos).unwrap();
        tmp.write_all(b"x").unwrap();
        std::fs::hard_link(&orig, &depois).unwrap();
        assert!(matches!(
            persistir(tmp, &depois, &protegidos, false),
            Err(ErroSalvar::Protegido(_))
        ));
        assert_eq!(std::fs::read(&orig).unwrap(), b"{\"a\":1}\n");
        assert!(residuos(d.path()).is_empty());
    }

    #[test]
    fn rename_atomico_sem_meio_termo() {
        let (d, _) = arquivo(b"");
        let destino = d.path().join("saida.json");
        std::fs::write(&destino, b"antigo").unwrap();
        // Enquanto grava, o destino continua o antigo inteiro; o novo só aparece no fim.
        gravar_atomico(&destino, &[], false, |f| {
            f.write_all(b"novo-parte-1")?;
            assert_eq!(std::fs::read(&destino).unwrap(), b"antigo");
            assert_eq!(residuos(d.path()).len(), 1, "grava no temporário ao lado");
            f.write_all(b"-2")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(&destino).unwrap(), b"novo-parte-1-2");
        assert!(residuos(d.path()).is_empty());
        // Falha no meio: o antigo fica intacto e o temporário some.
        let r = gravar_atomico(&destino, &[], false, |f| {
            f.write_all(b"meio")?;
            Err::<(), _>(ErroSalvar::Io(io::Error::other("disco cheio")))
        });
        assert!(r.is_err());
        assert_eq!(std::fs::read(&destino).unwrap(), b"novo-parte-1-2");
        assert!(residuos(d.path()).is_empty());
    }

    /// Sem máscara o arquivo tem os segredos: fica só do dono (0600). Mascarado: 0644.
    #[cfg(unix)]
    #[test]
    fn cru_fica_so_do_dono() {
        use std::os::unix::fs::PermissionsExt;
        let (d, _) = arquivo(b"");
        let modo = |cru: bool, nome: &str| {
            let p = d.path().join(nome);
            gravar_atomico(&p, &[], cru, |f| Ok(f.write_all(b"x")?)).unwrap();
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(modo(true, "cru.jsonl"), 0o600);
        assert_eq!(modo(false, "mascarado.jsonl"), 0o644);
        // Sobrescrever um 0644 existente com cru também fecha.
        assert_eq!(modo(true, "mascarado.jsonl"), 0o600);
    }

    #[test]
    fn cancelar_nao_deixa_residuo() {
        let (d, orig) = arquivo(b"");
        let dados: Vec<u8> = (0..200_000)
            .flat_map(|i| format!("{{\"i\":{i}}}\n").into_bytes())
            .collect();
        let destino = d.path().join("grande.jsonl");
        let cancelar = AtomicBool::new(false);
        let r = gravar_atomico(&destino, &[orig], false, |f| {
            escrever(&dados, false, ".", Saida::Jsonl, &cancelar, f, |_, _, n| {
                if n > 1000 {
                    cancelar.store(true, Relaxed);
                }
            })
        });
        assert!(matches!(r, Err(ErroSalvar::Cancelado)), "{r:?}");
        assert!(!destino.exists());
        assert!(residuos(d.path()).is_empty());
    }

    #[test]
    fn mascara_por_padrao() {
        let (_d, p) =
            arquivo(b"{\"senha\":\"S3NT1N3LA-1\",\"n\":1}\n{\"senha\":\"S3NT1N3LA-1\"}\n");
        let a = ArquivoLinhas::abrir(&p).unwrap();
        let salvar = |m: Mascara, sal| rodar(&entrada(&a, m, sal), false, ".", Saida::Jsonl);
        let padrao = salvar(Mascara::default(), 0);
        assert!(!padrao.contains("S3NT1N3LA"), "{padrao}");
        assert!(padrao.contains("\"***********\""), "{padrao}");
        let anon = salvar(Mascara::Anonimizar, 7);
        assert!(
            !anon.contains("S3NT1N3LA") && anon.contains("anon:"),
            "{anon}"
        );
        let l: Vec<serde_json::Value> = anon
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(l[0]["senha"], l[1]["senha"]);
        // Cru só pedindo.
        assert!(salvar(Mascara::SemMascara, 0).contains("S3NT1N3LA-1"));
    }
}
