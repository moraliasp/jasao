//! Árvore de valores de um JSONL: a lista plana de linhas visíveis que a interface desenha.
//!
//! Cada registro (linha do arquivo) começa recolhido e ocupa 1 linha. Expandir analisa só aquele
//! registro (sonic-rs, ver docs/decisoes.md) e insere os nós abertos logo abaixo dele. A
//! interface só pede janelas `(início, quantidade)` dessa lista.
//!
//! Fidelidade: o valor escalar é mostrado como o texto cru do arquivo (número de 400 dígitos
//! fica intacto); chaves saem na ordem do arquivo, inclusive repetidas.

use std::collections::BTreeMap;
use std::io;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use crate::ArquivoLinhas;
use crate::caminho::{Caminho, Segmento};

/// Aninhamento máximo de um registro. Acima disso o registro vira linha de erro: protege a pilha
/// (a análise é recursiva) e mantém a interface legível.
pub const MAX_PROFUNDIDADE: usize = 1_000;
/// Bytes de texto cru na prévia de um valor (e no nome do campo e em cada migalha).
pub const MAX_PREVIA: usize = 200;
/// Nós materializados por registro aberto. Acima disso o nó que se abre mostra só os primeiros
/// filhos e `Linha::omitidos` diz quantos ficaram de fora: um registro largo (milhões de itens)
/// não vira centenas de MB de nós. ~100 bytes por nó: ~100 MB no pior caso.
pub const MAX_NOS: usize = 1_000_000;
/// Bytes do registro formatado (e do registro e do valor que o app aceita formatar ou copiar).
/// Aninhamento fundo multiplica a indentação: 200 KB de entrada já dariam 200 MB.
pub const MAX_FORMATADO: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tipo {
    Objeto,
    Lista,
    Texto,
    Numero,
    Booleano,
    Nulo,
    /// Registro que não é JSON válido; `erro` traz posição e motivo, `previa` o texto cru.
    Erro,
}

/// Por que um registro não abre. A interface traduz o código; o texto do parser não chega lá.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Motivo {
    Utf8,
    /// Aninhamento acima de `MAX_PROFUNDIDADE`.
    Profundidade,
    FimInesperado,
    EsperavaDoisPontos,
    EsperavaVirgulaOuFimDaLista,
    EsperavaVirgulaOuFimDoObjeto,
    EsperavaChaveOuFimDoObjeto,
    Literal,
    Valor,
    Escape,
    Numero,
    Unicode,
    Controle,
    VirgulaSobrando,
    TextoDepoisDoValor,
    Outro,
}

/// Erro de um registro: byte (base 0, na linha) e motivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ErroLinha {
    pub byte: usize,
    pub motivo: Motivo,
}

/// Uma linha pronta para desenhar. Sem o caminho do nó: ele cresce com a profundidade e, numa
/// janela de linhas fundas, a resposta ficaria quadrática (ver `Arvore::migalhas`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Linha {
    /// Linha do arquivo (base 0) a que o nó pertence.
    pub registro: usize,
    /// 0 = o registro; 1 = seus campos; e assim por diante.
    pub prof: usize,
    /// Nome do campo, cortado em `MAX_PREVIA` bytes (com "…"); `None` no registro e em item
    /// de lista. O caminho completo sai de `Arvore::caminho`.
    pub chave: Option<String>,
    /// Posição do item na lista ou do campo no objeto.
    pub pos: usize,
    pub tipo: Tipo,
    /// Texto cru do valor, cortado em `MAX_PREVIA` bytes (com "…").
    pub previa: String,
    /// Só em `Tipo::Erro`.
    pub erro: Option<ErroLinha>,
    /// Quantos campos ou itens tem (objeto e lista).
    pub filhos: usize,
    /// Quantos campos ou itens tem o pai (0 no registro).
    pub irmaos: usize,
    pub expansivel: bool,
    pub aberto: bool,
    /// Filhos que não entraram na árvore por causa de `MAX_NOS` (só em nó aberto).
    pub omitidos: usize,
    /// Bytes `[início, fim)` do valor na linha do arquivo, para a visão Texto realçar o trecho.
    /// No registro inválido, a linha inteira.
    pub faixa: [usize; 2],
    /// O valor tem segredo mascarado (ver `mascara`): a interface oferece "Mostrar valor".
    pub mascarado: bool,
}

const SEM_PAI: usize = usize::MAX;

struct No {
    pai: usize,
    prof: usize,
    chave: Option<Box<str>>,
    pos: usize,
    tipo: Tipo,
    /// Bytes do valor dentro da linha.
    faixa: Range<usize>,
    /// Quantos campos ou itens o valor tem.
    total_filhos: usize,
    /// Os que já estão em `nos` (contíguos); `None` até o nó ser aberto a 1ª vez.
    filhos: Option<Range<usize>>,
    aberto: bool,
}

impl No {
    fn filhos(&self) -> Range<usize> {
        self.filhos.clone().unwrap_or_default()
    }
}

/// Texto completo da linha de um registro que já passou por `validar`.
fn texto_de(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or_default()
}

/// Registro expandido: os nós já abertos alguma vez e seus filhos (o `nos[0]` é o próprio
/// registro) e, em ordem, os que aparecem abaixo dele. Só se analisa o que se abre.
struct Aberto {
    nos: Vec<No>,
    visiveis: Vec<usize>,
}

/// Documento aberto: o arquivo e a lista plana de linhas visíveis.
pub struct Arvore {
    // Compartilhado com a varredura em 2º plano (`resumo::varrer`).
    arquivo: Arc<ArquivoLinhas>,
    // shortcut: localizar uma linha percorre os registros abertos (O(abertos)); trocar por
    // árvore de Fenwick se surgir "expandir todos os registros".
    abertos: BTreeMap<usize, Aberto>,
    total: usize,
}

impl Arvore {
    pub fn abrir(caminho: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self::new(ArquivoLinhas::abrir(caminho)?))
    }

    pub fn new(arquivo: ArquivoLinhas) -> Self {
        let total = arquivo.linhas();
        Self {
            arquivo: Arc::new(arquivo),
            abertos: BTreeMap::new(),
            total,
        }
    }

    pub fn registros(&self) -> usize {
        self.arquivo.linhas()
    }

    /// Troca o arquivo pelo mesmo com linhas a mais no fim (arquivo vivo, ver `vivo`): os
    /// registros abertos continuam abertos, porque os bytes de antes não mudam.
    pub fn crescer(&mut self, arquivo: ArquivoLinhas) {
        let novas = arquivo.linhas().saturating_sub(self.registros());
        self.arquivo = Arc::new(arquivo);
        self.total += novas;
    }

    /// Total de linhas visíveis.
    pub fn total(&self) -> usize {
        self.total
    }

    /// `validar` sobre `bytes` (já mascarados), mas com byte e motivo do erro tirados do arquivo
    /// cru: o trecho mascarado daria outro erro (ver `cru_de`).
    fn validar_registro<'a>(&'a self, bytes: &'a [u8]) -> Result<&'a str, ErroLinha> {
        validar(bytes).map_err(|e| validar(self.arquivo.cru_de(bytes)).err().unwrap_or(e))
    }

    fn bytes(&self, registro: usize) -> &[u8] {
        self.arquivo
            .janela(registro, 1)
            .first()
            .copied()
            .unwrap_or_default()
    }

    /// Linha visível `v` → (registro, posição em `visiveis` ou `None` se é a linha do registro).
    fn localizar(&self, v: usize) -> Option<(usize, Option<usize>)> {
        if v >= self.total {
            return None;
        }
        let mut extra = 0;
        for (&r, a) in &self.abertos {
            let linha_r = r + extra;
            if v <= linha_r {
                return Some((v - extra, None));
            }
            if v <= linha_r + a.visiveis.len() {
                return Some((r, Some(v - linha_r - 1)));
            }
            extra += a.visiveis.len();
        }
        Some((v - extra, None))
    }

    /// Linha visível do registro `r` (para "ir para linha").
    pub fn linha_do_registro(&self, r: usize) -> Option<usize> {
        (r < self.registros()).then(|| {
            r + self
                .abertos
                .range(..r)
                .map(|(_, a)| a.visiveis.len())
                .sum::<usize>()
        })
    }

    /// Até `quantidade` linhas a partir da linha visível `inicio`.
    pub fn janela(&self, inicio: usize, quantidade: usize) -> Vec<Linha> {
        let mut saida = Vec::with_capacity(quantidade.min(self.total.saturating_sub(inicio)));
        let Some((mut r, mut pos)) = self.localizar(inicio) else {
            return saida;
        };
        while saida.len() < quantidade && r < self.registros() {
            let aberto = self.abertos.get(&r);
            match (pos, aberto) {
                (None, _) => {
                    saida.push(self.linha_registro(r));
                    pos = aberto.map(|_| 0);
                    if aberto.is_none() {
                        r += 1;
                    }
                }
                (Some(p), Some(a)) if p < a.visiveis.len() => {
                    saida.push(linha_no(r, self.bytes(r), &a.nos, a.visiveis[p]));
                    pos = Some(p + 1);
                }
                _ => {
                    r += 1;
                    pos = None;
                }
            }
        }
        for l in &mut saida {
            let f = l.faixa[0]..l.faixa[1];
            l.mascarado =
                self.bytes(l.registro).get(f.clone()) != self.arquivo.linha_crua(l.registro).get(f);
        }
        saida
    }

    fn linha_registro(&self, r: usize) -> Linha {
        let bytes = self.bytes(r);
        if let Some(a) = self.abertos.get(&r) {
            return linha_no(r, bytes, &a.nos, 0);
        }
        let mut linha = Linha {
            registro: r,
            prof: 0,
            chave: None,
            pos: 0,
            tipo: Tipo::Erro,
            previa: String::new(),
            erro: None,
            filhos: 0,
            irmaos: 0,
            expansivel: false,
            aberto: false,
            omitidos: 0,
            faixa: [0, bytes.len()],
            mascarado: false,
        };
        match self.validar_registro(bytes) {
            Err(e) => {
                linha.erro = Some(e);
                linha.previa = previa(String::from_utf8_lossy(bytes).trim_matches(ESPACO));
            }
            Ok(texto) => {
                let raw = texto.trim_matches(ESPACO);
                let ini = raw.as_ptr() as usize - texto.as_ptr() as usize;
                linha.faixa = [ini, ini + raw.len()];
                linha.tipo = tipo_de(raw);
                linha.filhos = contar(raw);
                linha.expansivel = linha.filhos > 0;
                linha.previa = previa(raw);
            }
        }
        linha
    }

    /// Expande ou recolhe a linha visível `v`. Devolve `false` se ela não é expansível.
    pub fn alternar(&mut self, v: usize) -> bool {
        let Some((r, pos)) = self.localizar(v) else {
            return false;
        };
        let Some(p) = pos else {
            if let Some(a) = self.abertos.remove(&r) {
                self.total -= a.visiveis.len();
                return true;
            }
            let Ok(nos) = construir(self.bytes(r)) else {
                return false;
            };
            if nos[0].total_filhos == 0 {
                return false;
            }
            let mut visiveis = Vec::new();
            descendentes_visiveis(&nos, 0, &mut visiveis);
            self.total += visiveis.len();
            self.abertos.insert(r, Aberto { nos, visiveis });
            return true;
        };
        let texto = texto_de(
            self.arquivo
                .janela(r, 1)
                .first()
                .copied()
                .unwrap_or_default(),
        );
        let a = self
            .abertos
            .get_mut(&r)
            .expect("localizar só devolve pos de aberto");
        let id = a.visiveis[p];
        if a.nos[id].total_filhos == 0 {
            return false;
        }
        let antes = a.visiveis.len();
        if a.nos[id].aberto {
            let prof = a.nos[id].prof;
            let fim = a.visiveis[p + 1..]
                .iter()
                .position(|&n| a.nos[n].prof <= prof)
                .map_or(a.visiveis.len(), |i| p + 1 + i);
            a.visiveis.drain(p + 1..fim);
            a.nos[id].aberto = false;
        } else {
            preencher(texto, &mut a.nos, id);
            a.nos[id].aberto = true;
            let mut novos = Vec::new();
            descendentes_visiveis(&a.nos, id, &mut novos);
            a.visiveis.splice(p + 1..p + 1, novos);
        }
        self.total = self.total + a.visiveis.len() - antes;
        true
    }

    /// Abre o caminho até o nó mais fundo que contém `byte` (base 0, na linha do registro) e
    /// devolve a linha visível dele: é o clique na visão Texto. Byte em chave, `:` ou `,` conta
    /// para o valor seguinte; byte no `{`/`[` de abertura, no fechamento ou fora do valor fica no
    /// nó de fora. Registro inválido devolve a linha do registro; registro inexistente, `None`.
    pub fn revelar(&mut self, registro: usize, byte: usize) -> Option<usize> {
        let linha_r = self.linha_do_registro(registro)?;
        if !self.abertos.contains_key(&registro) && !self.alternar(linha_r) {
            return Some(linha_r);
        }
        let texto = texto_de(
            self.arquivo
                .janela(registro, 1)
                .first()
                .copied()
                .unwrap_or_default(),
        );
        let a = self.abertos.get_mut(&registro)?;
        let mut id = 0;
        while byte > a.nos[id].faixa.start && byte < a.nos[id].faixa.end {
            preencher(texto, &mut a.nos, id);
            let Some(f) = a.nos[id].filhos().find(|&f| a.nos[f].faixa.end > byte) else {
                break;
            };
            id = f;
            if a.nos[f].faixa.start > byte {
                break;
            }
        }
        let mut n = a.nos[id].pai;
        while n != SEM_PAI {
            a.nos[n].aberto = true;
            n = a.nos[n].pai;
        }
        let antes = a.visiveis.len();
        a.visiveis.clear();
        descendentes_visiveis(&a.nos, 0, &mut a.visiveis);
        self.total = self.total + a.visiveis.len() - antes;
        if id == 0 {
            return Some(linha_r);
        }
        a.visiveis
            .iter()
            .position(|&n| n == id)
            .map(|p| linha_r + 1 + p)
    }

    /// O registro com um campo ou item por linha e dois espaços por nível. Só o espaço entre
    /// os valores muda: chaves e escalares saem com o texto cru do arquivo (escape, número de
    /// 400 dígitos e chave repetida intactos). Corta em `MAX_FORMATADO` bytes e diz que cortou.
    pub fn formatado(&self, registro: usize) -> Result<Formatado, ErroLinha> {
        let texto = self.validar_registro(self.bytes(registro))?;
        Ok(formatar(texto.trim_matches(ESPACO), MAX_FORMATADO))
    }

    /// Arquivo por baixo da árvore (a visão Texto lê as linhas cruas dele).
    pub fn arquivo(&self) -> &ArquivoLinhas {
        &self.arquivo
    }

    /// O mesmo arquivo, para ler fora da trava do estado (varredura em 2º plano).
    pub fn compartilhado(&self) -> Arc<ArquivoLinhas> {
        Arc::clone(&self.arquivo)
    }

    /// Caminho do nó da linha visível `v`, a partir do registro (linha de registro: a raiz).
    pub fn caminho(&self, v: usize) -> Option<Caminho> {
        let (r, Some(p)) = self.localizar(v)? else {
            return Some(Caminho::raiz());
        };
        let a = &self.abertos[&r];
        let mut segmentos = Vec::with_capacity(a.nos[a.visiveis[p]].prof);
        let mut n = a.visiveis[p];
        while a.nos[n].pai != SEM_PAI {
            segmentos.push(match &a.nos[n].chave {
                Some(k) => Segmento::Chave(k.to_string()),
                None => Segmento::Indice(a.nos[n].pos),
            });
            n = a.nos[n].pai;
        }
        segmentos.reverse();
        Some(Caminho(segmentos))
    }

    /// Rótulos do caminho de `v` para as migalhas, cada um cortado como a prévia:
    /// `["pedidos", "[0]", "nome completo"]`. Pedido só para a linha selecionada.
    pub fn migalhas(&self, v: usize) -> Option<Vec<String>> {
        Some(
            self.caminho(v)?
                .0
                .into_iter()
                .map(|s| match s {
                    Segmento::Chave(k) => previa(&k),
                    Segmento::Indice(i) => format!("[{i}]"),
                })
                .collect(),
        )
    }

    /// Texto do valor da linha visível `v`, para copiar: texto JSON sai decodificado (sem aspas
    /// nem escapes); o resto, cru como no arquivo. Registro inválido: a linha inteira.
    pub fn valor(&self, v: usize) -> Option<String> {
        self.valor_de(v, self.bytes(self.localizar(v)?.0))
    }

    /// Como `valor`, sem a máscara de segredos: só o valor da linha `v`, que o usuário pediu para
    /// ver ("Mostrar valor"). Nada mais sai cru: só folha (texto, número, `true`, `false`,
    /// `null`); contêiner ou linha inválida revelaria todos os segredos de dentro, e dá `None`.
    pub fn valor_cru(&self, v: usize) -> Option<String> {
        let bytes = self.arquivo.linha_crua(self.localizar(v)?.0);
        let t = self.trecho(v, bytes)?.trim_ascii();
        let folha = (t.first() == Some(&b'"') && sonic_rs::from_slice::<String>(t).is_ok())
            || matches!(t, b"true" | b"false" | b"null")
            || crate::estrutura::eh_numero(t);
        folha.then(|| self.valor_de(v, bytes)).flatten()
    }

    /// O pedaço de `bytes` (a linha do registro de `v`) que é o valor da linha visível `v`.
    fn trecho<'b>(&self, v: usize, bytes: &'b [u8]) -> Option<&'b [u8]> {
        let (r, p) = self.localizar(v)?;
        let faixa = match p {
            Some(p) => {
                let a = &self.abertos[&r];
                a.nos[a.visiveis[p]].faixa.clone()
            }
            None => 0..bytes.len(),
        };
        Some(bytes.get(faixa).unwrap_or_default())
    }

    fn valor_de(&self, v: usize, bytes: &[u8]) -> Option<String> {
        let cru = String::from_utf8_lossy(self.trecho(v, bytes)?);
        if cru.starts_with('"')
            && let Ok(texto) = sonic_rs::from_str::<String>(&cru)
        {
            return Some(texto);
        }
        Some(cru.into_owned())
    }

    /// Linha visível do pai de `v` (seta para a esquerda); `None` para linha de registro.
    pub fn pai(&self, v: usize) -> Option<usize> {
        let (r, Some(p)) = self.localizar(v)? else {
            return None;
        };
        let a = &self.abertos[&r];
        let prof = a.nos[a.visiveis[p]].prof;
        let acima = a.visiveis[..p]
            .iter()
            .rposition(|&n| a.nos[n].prof < prof)
            .map_or(0, |i| i + 1);
        Some(v - p - 1 + acima)
    }
}

/// Filhos de `id` que aparecem, em ordem, descendo nos que estão abertos.
fn descendentes_visiveis(nos: &[No], id: usize, saida: &mut Vec<usize>) {
    for f in nos[id].filhos() {
        saida.push(f);
        if nos[f].aberto {
            descendentes_visiveis(nos, f, saida);
        }
    }
}

pub(crate) const ESPACO: &[char] = &[' ', '\t', '\n', '\r'];

/// Registro formatado; ver `Arvore::formatado`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Formatado {
    pub texto: String,
    /// Passou de `MAX_FORMATADO` e `texto` é só o começo.
    pub truncado: bool,
    /// Teto aplicado, em bytes (a interface mostra no aviso de corte).
    pub teto: usize,
}

/// Reindenta `raw` (JSON já validado) numa passada só, sem montar nós, e para em `max` bytes.
pub(crate) fn formatar(raw: &str, max: usize) -> Formatado {
    let b = raw.as_bytes();
    let mut saida = String::with_capacity(raw.len().min(max) * 2);
    let quebra = |saida: &mut String, nivel: usize| {
        saida.push('\n');
        saida.extend(std::iter::repeat_n("  ", nivel));
    };
    let (mut i, mut nivel) = (0, 0usize);
    while i < b.len() && saida.len() <= max {
        match b[i] {
            b'{' | b'[' => {
                let fecho = if b[i] == b'{' { b'}' } else { b']' };
                let j = i
                    + 1
                    + b[i + 1..]
                        .iter()
                        .take_while(|c| ESPACO.contains(&(**c as char)))
                        .count();
                if b.get(j) == Some(&fecho) {
                    // Vazio sai cru, como no arquivo (`{}`, `[ ]`).
                    saida.push_str(&raw[i..=j]);
                    i = j + 1;
                } else {
                    saida.push(b[i] as char);
                    nivel += 1;
                    quebra(&mut saida, nivel);
                    i += 1;
                }
            }
            b'}' | b']' => {
                nivel = nivel.saturating_sub(1);
                quebra(&mut saida, nivel);
                saida.push(b[i] as char);
                i += 1;
            }
            b',' => {
                saida.push(',');
                quebra(&mut saida, nivel);
                i += 1;
            }
            b':' => {
                saida.push_str(": ");
                i += 1;
            }
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'"' => {
                let mut j = i + 1;
                while b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                saida.push_str(&raw[i..=j]);
                i = j + 1;
            }
            _ => {
                let n = b[i..]
                    .iter()
                    .position(|c| b",:]} \t\n\r".contains(c))
                    .unwrap_or(b.len() - i);
                saida.push_str(&raw[i..i + n]);
                i += n;
            }
        }
    }
    let truncado = i < b.len() || saida.len() > max;
    if saida.len() > max {
        let mut fim = max;
        while !saida.is_char_boundary(fim) {
            fim -= 1;
        }
        saida.truncate(fim);
    }
    Formatado {
        texto: saida,
        truncado,
        teto: max,
    }
}

pub(crate) fn tipo_de(raw: &str) -> Tipo {
    match raw.as_bytes().first() {
        Some(b'{') => Tipo::Objeto,
        Some(b'[') => Tipo::Lista,
        Some(b'"') => Tipo::Texto,
        Some(b't' | b'f') => Tipo::Booleano,
        Some(b'n') => Tipo::Nulo,
        _ => Tipo::Numero,
    }
}

/// Campos ou itens de um valor já validado.
pub(crate) fn contar(raw: &str) -> usize {
    match tipo_de(raw) {
        Tipo::Objeto => sonic_rs::to_object_iter(raw).count(),
        Tipo::Lista => sonic_rs::to_array_iter(raw).count(),
        _ => 0,
    }
}

pub(crate) fn previa(raw: &str) -> String {
    if raw.len() <= MAX_PREVIA {
        return raw.to_owned();
    }
    let mut fim = MAX_PREVIA;
    while !raw.is_char_boundary(fim) {
        fim -= 1;
    }
    format!("{}…", &raw[..fim])
}

/// Confere UTF-8, profundidade e sintaxe.
pub(crate) fn validar(bytes: &[u8]) -> Result<&str, ErroLinha> {
    let erro = |byte, motivo| ErroLinha { byte, motivo };
    let texto = std::str::from_utf8(bytes).map_err(|e| erro(e.valid_up_to(), Motivo::Utf8))?;
    // O sonic-rs aceita `\u` sem 4 dígitos hexa e engole os 4 bytes seguintes (até `\"`): aí
    // `formatar` e `passa_da_profundidade`, que leem a string do jeito certo, divergem dele.
    if let Some(pos) = escape_u_invalido(bytes) {
        return Err(erro(pos, Motivo::Unicode));
    }
    if let Some(pos) = passa_da_profundidade(bytes, MAX_PROFUNDIDADE) {
        return Err(erro(pos, Motivo::Profundidade));
    }
    // IgnoredAny no sonic-rs valida o valor inteiro e o que vem depois dele.
    sonic_rs::from_str::<serde::de::IgnoredAny>(texto).map_err(|e| do_sonic(&e))?;
    Ok(texto)
}

/// O sonic-rs não expõe o código do erro, só a mensagem; o mapa é pelo começo dela
/// (versão fixada no Cargo.lock; o teste `motivos_do_sonic` acusa se mudar).
fn do_sonic(e: &sonic_rs::Error) -> ErroLinha {
    const MAPA: &[(&str, Motivo)] = &[
        ("EOF while parsing", Motivo::FimInesperado),
        (
            "Expected this character to be a ':'",
            Motivo::EsperavaDoisPontos,
        ),
        (
            "Expected this character to be either a ',' or a ']'",
            Motivo::EsperavaVirgulaOuFimDaLista,
        ),
        (
            "Expected this character to be either a ',' or a '}'",
            Motivo::EsperavaVirgulaOuFimDoObjeto,
        ),
        (
            "Expected this character to be '\"' or '}'",
            Motivo::EsperavaChaveOuFimDoObjeto,
        ),
        ("Invalid literal", Motivo::Literal),
        ("Invalid JSON value", Motivo::Valor),
        ("Invalid escape", Motivo::Escape),
        ("Invalid number", Motivo::Numero),
        ("Invalid unicode", Motivo::Unicode),
        ("Invalid surrogate", Motivo::Unicode),
        ("Invalid UTF-8", Motivo::Utf8),
        ("Control character", Motivo::Controle),
        (
            "JSON has a comma after the last value",
            Motivo::VirgulaSobrando,
        ),
        (
            "JSON has non-whitespace trailing characters",
            Motivo::TextoDepoisDoValor,
        ),
    ];
    let texto = e.to_string();
    let motivo = MAPA
        .iter()
        .find(|(m, _)| texto.starts_with(m))
        .map_or(Motivo::Outro, |&(_, m)| m);
    ErroLinha {
        byte: e.offset(),
        motivo,
    }
}

/// Byte do primeiro `\u` sem 4 dígitos hexa logo depois. Barra invertida só é válida dentro de
/// texto, e lá vem sempre em par com o caractere escapado: basta pular de par em par.
fn escape_u_invalido(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while let Some(p) = memchr::memchr(b'\\', &bytes[i..]) {
        let j = i + p;
        if bytes.get(j + 1) == Some(&b'u')
            && !bytes
                .get(j + 2..j + 6)
                .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit))
        {
            return Some(j);
        }
        i = j + 2;
        if i >= bytes.len() {
            break;
        }
    }
    None
}

/// Primeiro byte em que o aninhamento passa de `max`, ignorando o que está em texto.
pub(crate) fn passa_da_profundidade(bytes: &[u8], max: usize) -> Option<usize> {
    let (mut prof, mut em_texto, mut escape) = (0usize, false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if em_texto {
            match (escape, b) {
                (true, _) => escape = false,
                (false, b'\\') => escape = true,
                (false, b'"') => em_texto = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => em_texto = true,
            b'{' | b'[' => {
                prof += 1;
                if prof > max {
                    return Some(i);
                }
            }
            b'}' | b']' => prof = prof.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// Valida o registro e analisa a raiz e seus filhos; o resto fica para quando abrir.
fn construir(bytes: &[u8]) -> Result<Vec<No>, ErroLinha> {
    let texto = validar(bytes)?;
    let raw = texto.trim_matches(ESPACO);
    let ini = raw.as_ptr() as usize - texto.as_ptr() as usize;
    let mut nos = vec![No {
        pai: SEM_PAI,
        prof: 0,
        chave: None,
        pos: 0,
        tipo: tipo_de(raw),
        faixa: ini..ini + raw.len(),
        total_filhos: contar(raw),
        filhos: None,
        aberto: true,
    }];
    preencher(texto, &mut nos, 0);
    Ok(nos)
}

/// Põe em `nos` os filhos de `id` (uma vez só), até `MAX_NOS` no registro.
fn preencher(texto: &str, nos: &mut Vec<No>, id: usize) {
    if nos[id].filhos.is_some() {
        return;
    }
    let raw = &texto[nos[id].faixa.clone()];
    let base = texto.as_ptr() as usize;
    let ini = nos.len();
    let cabem = MAX_NOS.saturating_sub(ini).min(nos[id].total_filhos);
    let (prof, tipo) = (nos[id].prof + 1, nos[id].tipo);
    let mut novo = |chave: Option<Box<str>>, valor: &str| {
        let inicio = valor.as_ptr() as usize - base;
        nos.push(No {
            pai: id,
            prof,
            chave,
            pos: nos.len() - ini,
            tipo: tipo_de(valor),
            faixa: inicio..inicio + valor.len(),
            total_filhos: contar(valor),
            filhos: None,
            aberto: false,
        });
    };
    // O registro já passou por `validar`: o iterador não erra.
    match tipo {
        Tipo::Objeto => sonic_rs::to_object_iter(raw)
            .take(cabem)
            .flatten()
            .for_each(|(k, v)| novo(Some(Box::from(&*k)), v.as_raw_str())),
        Tipo::Lista => sonic_rs::to_array_iter(raw)
            .take(cabem)
            .flatten()
            .for_each(|v| novo(None, v.as_raw_str())),
        _ => {}
    }
    let fim = nos.len();
    nos[id].filhos = Some(ini..fim);
}

fn linha_no(registro: usize, bytes: &[u8], nos: &[No], id: usize) -> Linha {
    let no = &nos[id];
    let aberto = no.total_filhos > 0 && no.aberto;
    Linha {
        registro,
        prof: no.prof,
        chave: no.chave.as_deref().map(previa),
        pos: no.pos,
        tipo: no.tipo,
        previa: previa(texto_de(bytes).get(no.faixa.clone()).unwrap_or_default()),
        erro: None,
        filhos: no.total_filhos,
        irmaos: nos.get(no.pai).map_or(0, |p| p.total_filhos),
        expansivel: no.total_filhos > 0,
        aberto,
        omitidos: if aberto {
            no.total_filhos - no.filhos().len()
        } else {
            0
        },
        faixa: [no.faixa.start, no.faixa.end],
        mascarado: false,
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use proptest::prelude::*;
    use std::io::Write;

    fn arvore(conteudo: &str) -> Arvore {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(conteudo.as_bytes()).unwrap();
        Arvore::abrir(f.path()).unwrap()
    }

    /// Caminho jq da linha visível `v`.
    fn jq(a: &Arvore, v: usize) -> String {
        a.caminho(v).unwrap().jq()
    }

    /// (prof, caminho, prévia) de toda a lista visível.
    fn tudo(a: &Arvore) -> Vec<(usize, String, String)> {
        a.janela(0, usize::MAX)
            .into_iter()
            .enumerate()
            .map(|(v, l)| (l.prof, jq(a, v), l.previa))
            .collect()
    }

    const DOIS: &str = "{\"a\":1,\"b\":{\"c\":[true,null]},\"d\":\"x\"}\n[1,2]\n";

    #[test]
    fn expandir_e_recolher() {
        let mut a = arvore(DOIS);
        assert_eq!(a.total(), 2);
        assert!(a.alternar(0));
        assert_eq!(a.total(), 5);
        let linhas = a.janela(0, 10);
        let caminhos: Vec<_> = (0..5).map(|v| jq(&a, v)).collect();
        assert_eq!(caminhos, [".", ".a", ".b", ".d", "."]);
        assert_eq!(linhas[1].previa, "1");
        assert_eq!(linhas[1].tipo, Tipo::Numero);
        assert_eq!(linhas[2].filhos, 1);
        assert_eq!(linhas[4].registro, 1);
        assert_eq!(linhas[4].filhos, 2);

        // Abre .b e depois .b.c; recolher .b esconde os dois, reabrir mostra .c ainda aberto.
        assert!(a.alternar(2));
        assert!(a.alternar(3));
        assert_eq!(a.total(), 8);
        assert_eq!(jq(&a, 4), ".b.c[0]");
        assert_eq!(a.migalhas(4).unwrap(), ["b", "c", "[0]"]);
        assert_eq!(a.pai(5), Some(3));
        assert_eq!(a.pai(3), Some(2));
        assert_eq!(a.pai(2), Some(0));
        assert_eq!(a.pai(0), None);
        assert!(a.alternar(2));
        assert_eq!(a.total(), 5);
        assert!(a.alternar(2));
        assert_eq!(a.total(), 8);
        assert!(!a.alternar(1), "escalar não expande");

        // Registro seguinte, depois do primeiro aberto.
        assert_eq!(a.linha_do_registro(1), Some(7));
        assert!(a.alternar(7));
        assert_eq!(
            tudo(&a)[7..],
            [
                (0, ".".into(), "[1,2]".into()),
                (1, ".[0]".into(), "1".into()),
                (1, ".[1]".into(), "2".into())
            ]
        );
        assert_eq!(a.pai(9), Some(7));

        // Recolher o primeiro registro tira seus 6 nós; o segundo continua aberto.
        assert!(a.alternar(0));
        assert_eq!(a.total(), 4);
        assert_eq!(a.linha_do_registro(1), Some(1));
    }

    #[test]
    fn janelas_nas_bordas() {
        let mut a = arvore(DOIS);
        a.alternar(0);
        assert_eq!(a.janela(5, 10), vec![]);
        assert_eq!(a.janela(usize::MAX, 10), vec![]);
        assert_eq!(a.janela(0, 0), vec![]);
        assert_eq!(a.janela(4, 10).len(), 1);
        assert_eq!(a.janela(3, 2)[1].registro, 1);
        // Janela começando no meio do registro aberto.
        assert_eq!(jq(&a, 2), ".b");
        assert!(!a.alternar(99));
        assert_eq!(arvore("").janela(0, 10), vec![]);
    }

    #[test]
    fn linha_invalida_nao_impede_o_resto() {
        let mut a = arvore("{\"a\":1}\n{\"a\":tru}\n[1,]\n{\"a\":1} x\n\"\\x\"\n[1]\n");
        let linhas = a.janela(0, 10);
        let tipos: Vec<_> = linhas.iter().map(|l| l.tipo).collect();
        assert_eq!(
            tipos,
            [
                Tipo::Objeto,
                Tipo::Erro,
                Tipo::Erro,
                Tipo::Erro,
                Tipo::Erro,
                Tipo::Lista
            ]
        );
        let erros: Vec<_> = linhas[1..5].iter().map(|l| l.erro.unwrap()).collect();
        let e = |byte, motivo| ErroLinha { byte, motivo };
        assert_eq!(
            erros,
            [
                e(8, Motivo::Literal),
                e(3, Motivo::Valor),
                e(8, Motivo::TextoDepoisDoValor),
                e(2, Motivo::Escape),
            ]
        );
        assert_eq!(linhas[1].previa, "{\"a\":tru}", "erro mostra o texto cru");
        assert!(!a.alternar(1), "linha inválida não expande");
        assert!(a.alternar(5));
        assert_eq!(a.total(), 7);
    }

    #[test]
    fn utf8_invalido() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"\"a\xff\"\n").unwrap();
        let a = Arvore::abrir(f.path()).unwrap();
        let l = &a.janela(0, 1)[0];
        assert_eq!(
            (l.tipo, l.erro, l.previa.as_str()),
            (
                Tipo::Erro,
                Some(ErroLinha {
                    byte: 2,
                    motivo: Motivo::Utf8
                }),
                "\"a\u{fffd}\""
            )
        );
    }

    #[test]
    fn numero_de_400_digitos_fica_intacto() {
        let n = "9".repeat(400);
        let mut a = arvore(&format!("{{\"n\":{n}.5e-3}}\n"));
        a.alternar(0);
        let l = &a.janela(1, 1)[0];
        assert_eq!(l.tipo, Tipo::Numero);
        // A prévia corta em MAX_PREVIA, mas sem arredondar nada.
        assert_eq!(l.previa, format!("{}…", &n[..MAX_PREVIA]));
        let curto = "123456789012345678901234567890.000";
        let mut a = arvore(&format!("[{curto}]"));
        a.alternar(0);
        assert_eq!(a.janela(1, 1)[0].previa, curto);
    }

    #[test]
    fn chave_duplicada_e_ordem_preservadas() {
        let mut a = arvore("{\"z\":1,\"a\":2,\"z\":3,\"nome completo\":4,\"\":5}");
        a.alternar(0);
        let l: Vec<_> = a
            .janela(1, 10)
            .into_iter()
            .enumerate()
            .map(|(i, l)| (l.chave.unwrap(), jq(&a, i + 1), l.previa))
            .collect();
        let s = |a: &str, b: &str, c: &str| (a.to_owned(), b.to_owned(), c.to_owned());
        assert_eq!(
            l,
            [
                s("z", ".z", "1"),
                s("a", ".a", "2"),
                s("z", ".z", "3"),
                s("nome completo", ".[\"nome completo\"]", "4"),
                s("", ".[\"\"]", "5"),
            ]
        );
    }

    #[test]
    fn escape_u_sem_4_hexa_e_erro_sem_panic_nem_burlar_profundidade() {
        let erro = |l: &str| validar(l.as_bytes()).err().map(|e| e.motivo);
        assert_eq!(erro(r#"{"k":"\uABC\"}"#), Some(Motivo::Unicode));
        assert_eq!(erro(r#""\uZZZZ""#), Some(Motivo::Unicode));
        assert_eq!(erro(r#""\u12""#), Some(Motivo::Unicode));
        assert_eq!(erro(r#""\\u""#), None); // `\\` seguido de `u` é só a letra
        assert_eq!(erro(r#""\u00e9\n""#), None);
        let n = 5000;
        let burla = format!("[\"\\uABC\\\",{}1{}]", "[".repeat(n), "]".repeat(n));
        assert!(validar(burla.as_bytes()).is_err());
        // Antes: a árvore dava a linha como válida e `formatado` estourava o índice.
        let a = arvore("{\"k\":\"\\uABC\\\"}\n");
        assert_eq!(a.janela(0, 1)[0].tipo, Tipo::Erro);
    }

    #[test]
    fn profundidade_no_limite_abre_e_acima_vira_erro() {
        let fundo = |n: usize| format!("{}{}\n", "[".repeat(n), "]".repeat(n));
        let mut a = arvore(&(fundo(MAX_PROFUNDIDADE) + &fundo(MAX_PROFUNDIDADE + 1)));
        let linhas = a.janela(0, 2);
        assert_eq!(linhas[0].tipo, Tipo::Lista);
        assert_eq!(linhas[1].tipo, Tipo::Erro);
        assert_eq!(
            linhas[1].erro,
            Some(ErroLinha {
                byte: 1000,
                motivo: Motivo::Profundidade
            })
        );
        // Abre até o fundo, um nível por vez.
        for v in 0..MAX_PROFUNDIDADE - 1 {
            assert!(a.alternar(v), "nível {v}");
        }
        let ultimo = &a.janela(MAX_PROFUNDIDADE - 1, 1)[0];
        assert_eq!((ultimo.prof, ultimo.filhos), (MAX_PROFUNDIDADE - 1, 0));
        // Colchete dentro de texto não conta.
        assert_eq!(
            passa_da_profundidade(
                format!("[\"{}\"]", "[".repeat(5000)).as_bytes(),
                MAX_PROFUNDIDADE
            ),
            None
        );
    }

    #[test]
    fn escalar_e_vazios_no_registro() {
        let a = arvore("42\n{}\n[]\n  {\"a\" : 1}  \n");
        let l = a.janela(0, 4);
        assert_eq!(
            l.iter()
                .map(|l| (l.tipo, l.expansivel, l.previa.as_str()))
                .collect::<Vec<_>>(),
            [
                (Tipo::Numero, false, "42"),
                (Tipo::Objeto, false, "{}"),
                (Tipo::Lista, false, "[]"),
                (Tipo::Objeto, true, "{\"a\" : 1}"),
            ]
        );
    }

    #[test]
    fn motivos_do_sonic() {
        let casos = [
            ("{\"a\":1", Motivo::FimInesperado),
            ("{\"a\" 1}", Motivo::EsperavaDoisPontos),
            ("[1 2]", Motivo::EsperavaVirgulaOuFimDaLista),
            ("{\"a\":1 \"b\":2}", Motivo::EsperavaVirgulaOuFimDoObjeto),
            ("{1:2}", Motivo::EsperavaChaveOuFimDoObjeto),
            ("[nul]", Motivo::Literal),
            ("[-]", Motivo::Numero),
            ("\"a\tb\"", Motivo::Controle),
            ("{\"a\":1,}", Motivo::EsperavaChaveOuFimDoObjeto),
        ];
        for (json, esperado) in casos {
            let e = validar(json.as_bytes()).expect_err(json);
            assert_eq!(e.motivo, esperado, "{json}");
        }
    }

    #[test]
    fn irmaos_contam_os_filhos_do_pai() {
        let mut a = arvore("{\"a\":[1,2,3],\"b\":0}");
        a.alternar(0);
        a.alternar(1);
        let l: Vec<_> = a.janela(0, 10).iter().map(|l| (l.pos, l.irmaos)).collect();
        assert_eq!(l, [(0, 0), (0, 2), (0, 3), (1, 3), (2, 3), (1, 2)]);
    }

    #[test]
    fn revelar_byte_do_texto() {
        // {"a":1,"b":{"c":[true,null]},"d":"x"}: `true` em 17, `{` de .b em 11, chave a em 2.
        let mut a = arvore(DOIS);
        assert_eq!(a.revelar(0, 18), Some(4));
        assert_eq!(jq(&a, 4), ".b.c[0]");
        assert_eq!(a.revelar(0, 2).map(|v| jq(&a, v)), Some(".a".into()));
        assert_eq!(a.revelar(0, 11).map(|v| jq(&a, v)), Some(".b".into()));
        assert_eq!(a.revelar(0, 0), Some(0));
        assert_eq!(a.revelar(0, 99), Some(0), "fora do valor fica no registro");
        // Registro 1 vem depois das 7 linhas do registro 0 aberto; byte 1 é o `1` de [1,2].
        assert_eq!(a.revelar(1, 1), Some(8));
        assert_eq!(jq(&a, 8), ".[0]");
        assert_eq!(a.total(), 10);
        assert_eq!(a.revelar(2, 0), None);

        let mut inv = arvore("{x\n[]\n");
        assert_eq!(inv.revelar(0, 1), Some(0));
        assert_eq!(inv.revelar(1, 0), Some(1), "registro sem filhos fica nele");
    }

    #[test]
    fn caminho_e_valor_para_copiar() {
        let mut a = arvore("{\"a b\":[\"x\\ny\",{\"if\":1.50}]}\n\"só\"\n{\"c\":tru}\n");
        a.alternar(0);
        a.alternar(1);
        // 0 registro, 1 ."a b", 2 [0], 3 [1]
        a.alternar(3);
        let c = a.caminho(4).unwrap();
        assert_eq!(c.jq(), ".[\"a b\"][1][\"if\"]");
        assert_eq!(c.jsonpath(), "$['a b'][1].if");
        assert_eq!(c.json_pointer(), "/a b/1/if");
        assert_eq!(a.valor(4).as_deref(), Some("1.50"));
        assert_eq!(a.valor(2).as_deref(), Some("x\ny"));
        assert_eq!(a.valor(3).as_deref(), Some("{\"if\":1.50}"));
        assert_eq!(a.caminho(0), Some(Caminho::raiz()));
        assert_eq!(a.valor(5).as_deref(), Some("só"));
        assert_eq!(a.valor(6).as_deref(), Some("{\"c\":tru}"));
        assert_eq!(a.caminho(7), None);
        assert_eq!(a.valor(7), None);
    }

    #[test]
    fn faixa_do_valor_na_linha() {
        let mut a = arvore(" {\"a\":1} \n{x\n");
        assert_eq!(a.janela(0, 2)[0].faixa, [1, 8]);
        assert_eq!(a.janela(0, 2)[1].faixa, [0, 2]);
        assert!(a.alternar(0));
        assert_eq!(a.janela(1, 1)[0].faixa, [6, 7]);
    }

    #[test]
    fn formatado_mantem_bytes() {
        let a = arvore(concat!(
            r#"{"n":123456789012345678901234567890, "ké":{},"ké":[1,{"x":"a\"b"}],"v":[]}"#,
            "\n{x\n"
        ));
        let esperado = r#"{
  "n": 123456789012345678901234567890,
  "ké": {},
  "ké": [
    1,
    {
      "x": "a\"b"
    }
  ],
  "v": []
}"#;
        let f = a.formatado(0).unwrap();
        assert_eq!((f.texto.as_str(), f.truncado), (esperado, false));
        assert!(a.formatado(1).is_err());
        assert_eq!(arvore("7\n").formatado(0).unwrap().texto, "7");
        assert_eq!(arvore(" [ ] \n").formatado(0).unwrap().texto, "[ ]");
    }

    #[test]
    fn formatado_da_o_erro_do_cru_como_a_linha() {
        // Achado do fuzz: a máscara troca o `\xff` e o formatado acusava outro erro que a linha.
        let dados: &'static [u8] = b"\x0b\0\0]rg[\xff~\n]rg[[[";
        let a = Arvore::new(ArquivoLinhas::estatico(dados));
        let e = ErroLinha {
            byte: 7,
            motivo: Motivo::Utf8,
        };
        assert_eq!(a.janela(0, 1)[0].erro, Some(e));
        assert_eq!(a.formatado(0), Err(e));
    }

    /// Lista de `n` cadeias `[[[…0…]]]` de `prof` níveis: pouca entrada, muita indentação.
    fn cadeias(n: usize, prof: usize) -> String {
        let cadeia = format!("{}0{}", "[".repeat(prof), "]".repeat(prof));
        format!("[{}]\n", vec![cadeia; n].join(","))
    }

    #[test]
    fn formatado_tem_teto_e_avisa() {
        // 200 KB que, formatados por inteiro, dariam ~200 MB.
        let a = arvore(&cadeias(100, MAX_PROFUNDIDADE - 1));
        let f = a.formatado(0).unwrap();
        assert!(f.truncado);
        assert!(f.texto.len() <= MAX_FORMATADO && f.texto.len() > MAX_FORMATADO - 4_096);
        // O corte respeita UTF-8 e o teto vale para qualquer `max`.
        let f = formatar(r#"{"ação":"ééé"}"#, 12);
        assert!(f.truncado && f.texto.len() <= 12, "{f:?}");
        // `{\n  "ação"…`: o byte 7 cai no meio do `ç` (bytes 6–7) e o 9 no meio do `ã` (8–9);
        // o corte recua para a fronteira anterior em vez de partir o caractere.
        for (max, esperado) in [(7, "{\n  \"a"), (9, "{\n  \"aç")] {
            let f = formatar(r#"{"ação":"ééé"}"#, max);
            assert_eq!((f.texto.as_str(), f.truncado), (esperado, true));
            assert!(std::str::from_utf8(f.texto.as_bytes()).is_ok() && f.texto.len() <= max);
        }
        assert!(!formatar("[1]", 7).truncado);
    }

    fn valor_json() -> impl Strategy<Value = serde_json::Value> {
        use serde_json::Value;
        let folha = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            "[ -~é\\n\"\\\\]{0,6}".prop_map(Value::String),
        ];
        folha.prop_recursive(5, 48, 5, |dentro| {
            prop_oneof![
                prop::collection::vec(dentro.clone(), 0..5).prop_map(Value::Array),
                prop::collection::vec(("[a-z:,{ ]{0,4}", dentro), 0..5)
                    .prop_map(|v| Value::Object(v.into_iter().collect())),
            ]
        })
    }

    proptest! {
        /// Sem chave repetida nem número reescrito, o formatado é o `to_string_pretty`.
        #[test]
        fn formatar_igual_ao_pretty(v in valor_json()) {
            let f = formatar(&serde_json::to_string(&v).unwrap(), MAX_FORMATADO);
            prop_assert_eq!(f.texto, serde_json::to_string_pretty(&v).unwrap());
            prop_assert!(!f.truncado);
        }
    }

    #[test]
    fn janela_funda_nao_cresce_com_a_profundidade() {
        // 1000 níveis de objeto com chave de 990 bytes (~1 MB): cada linha levava o caminho
        // inteiro, e a janela de 1000 linhas passava de 900 MB.
        let k = "k".repeat(990);
        let prof = MAX_PROFUNDIDADE - 1;
        let reg = format!(
            "{}0{}\n",
            format!("{{\"{k}\":").repeat(prof),
            "}".repeat(prof)
        );
        let mut a = arvore(&reg);
        assert_eq!(a.revelar(0, reg.find('0').unwrap()), Some(prof));
        let linhas = a.janela(0, 1_000);
        assert_eq!(linhas.len(), prof + 1);
        let bytes = serde_json::to_vec(&linhas).unwrap().len();
        // Cada linha: prévia e chave de até `MAX_PREVIA` bytes e campos de tamanho fixo.
        assert!(bytes < linhas.len() * (2 * MAX_PREVIA + 300), "{bytes} B");
        assert_eq!(
            linhas[1].chave.as_deref(),
            Some(format!("{}…", &k[..MAX_PREVIA]).as_str())
        );
        // O caminho completo sai sob demanda, para uma linha só.
        assert_eq!(a.caminho(prof).unwrap().0.len(), prof);
        let m = a.migalhas(prof).unwrap();
        assert_eq!(m.len(), prof);
        assert!(m.iter().all(|s| s.len() <= MAX_PREVIA + 3));
    }

    #[test]
    fn alternar_so_analisa_o_que_abre() {
        // Abrir o registro de uma cadeia funda analisa só a raiz e o 1º nível.
        let mut a = arvore(&cadeias(1, MAX_PROFUNDIDADE - 1));
        assert!(a.alternar(0));
        assert_eq!(a.abertos[&0].nos.len(), 2);
        assert!(a.alternar(1));
        assert_eq!(a.abertos[&0].nos.len(), 3);

        // Registro largo: no máximo `MAX_NOS` nós, e a linha diz quantos ficaram de fora.
        let n = MAX_NOS + 10;
        let mut a = arvore(&format!("[{}0]\n", "0,".repeat(n - 1)));
        assert!(a.alternar(0));
        assert_eq!(a.abertos[&0].nos.len(), MAX_NOS);
        assert_eq!(a.total(), MAX_NOS);
        let raiz = &a.janela(0, 1)[0];
        assert_eq!((raiz.filhos, raiz.omitidos), (n, n - (MAX_NOS - 1)));
        assert_eq!(
            a.janela(MAX_NOS - 1, 5).len(),
            1,
            "a lista acaba no último nó"
        );
    }

    #[test]
    fn teto_de_nos_vale_para_o_registro_todo() {
        // Dois filhos de 600 mil itens: o 1º cabe inteiro, o 2º só no que sobra de `MAX_NOS`.
        let k = 600_000;
        let filho = format!("[{}0]", "0,".repeat(k - 1));
        let mut a = arvore(&format!("[{filho},{filho}]\n"));
        assert!(a.alternar(0));
        assert!(a.alternar(1));
        assert_eq!(a.abertos[&0].nos.len(), 3 + k);
        let segundo = 2 + k;
        assert!(a.alternar(segundo));
        assert_eq!(a.abertos[&0].nos.len(), MAX_NOS);
        let l = &a.janela(segundo, 1)[0];
        assert_eq!((l.filhos, l.omitidos), (k, k - (MAX_NOS - 3 - k)));
        assert_eq!(a.janela(1, 1)[0].omitidos, 0);
    }
}
