//! Abertura por URL: baixa `https://…` (ou um comando cURL colado) para o cache e devolve o
//! caminho, que abre pelo caminho normal (gzip e zstd são descompactados lá, pelos bytes mágicos).
//!
//! Privacidade (ver docs/decisoes.md, "Abrir de URL"): só por pedido explícito; vai só o método,
//! os cabeçalhos e o corpo do pedido, com User-Agent `jasao/<versão>`, sem cookies do sistema;
//! cabeçalho de autenticação e `-u` nunca vão a disco nem a mensagem de erro; `http://` só com
//! `permitir_http`; TLS sempre validado; até 5 redirecionamentos, nunca de https para http, e
//! sem cabeçalho sensível quando o salto muda de servidor.

use std::fmt;
use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::compactado;
use crate::entrada::{self, Temporario};

const MIB: u64 = 1 << 20;
/// Prefixo do arquivo baixado no cache (ver `baixado`).
const MAX_REDIRECIONAMENTOS: u32 = 5;
/// Erro quando o pedido é `http://` e `permitir_http` está desligado: quem chama pergunta.
pub const ERRO_HTTP: &str = "http:// não é cifrado: qualquer um no caminho lê e altera o conteúdo; confirme para abrir assim mesmo";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Metodo {
    Get,
    Post,
}

/// O que enviar. `Debug` mascara o valor dos cabeçalhos sensíveis.
#[derive(Clone, PartialEq, Eq)]
pub struct Pedido {
    pub url: String,
    pub metodo: Metodo,
    pub cabecalhos: Vec<(String, String)>,
    pub corpo: Option<String>,
}

impl fmt::Debug for Pedido {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cab: Vec<(&str, &str)> = self
            .cabecalhos
            .iter()
            .map(|(k, v)| (k.as_str(), if sensivel(k) { "***" } else { v.as_str() }))
            .collect();
        f.debug_struct("Pedido")
            .field("url", &sem_consulta(&self.url))
            .field("metodo", &self.metodo)
            .field("cabecalhos", &cab)
            .field("corpo", &self.corpo.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Cabeçalho cujo valor é credencial: nunca aparece em erro nem em `Debug`.
fn sensivel(nome: &str) -> bool {
    let n = nome.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "authorization" | "proxy-authorization" | "cookie"
    ) || crate::estrutura::chave_de_segredo(n.as_bytes())
}

/// URL sem credencial, consulta (`?token=…`) nem fragmento, para mensagens: esquema, host,
/// porta e caminho, pelo parser WHATWG (o mesmo corte que o pedido usa; `#` ou `?` dentro da
/// senha não confundem). O que não interpreta não é mostrado.
fn sem_consulta(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let porta = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            let host = u.host_str().unwrap_or_default();
            format!("{}://{host}{porta}{}", u.scheme(), u.path())
        }
        Err(_) => "(URL inválida)".into(),
    }
}

impl Pedido {
    /// Lê o que o usuário colou: um comando cURL (começa com `curl`) ou uma URL.
    pub fn interpretar(entrada: &str) -> Result<Self, String> {
        let e = entrada.trim();
        if e == "curl"
            || e.starts_with("curl ")
            || e.starts_with("curl\t")
            || e.starts_with("curl\\")
        {
            Self::de_curl(e)
        } else {
            Self::de_url(e)
        }
    }

    /// GET simples de `url` (`https://` ou `http://`).
    pub fn de_url(url: &str) -> Result<Self, String> {
        let url = url.trim();
        let esquema = url.split_once("://").map(|(e, _)| e.to_ascii_lowercase());
        if !matches!(esquema.as_deref(), Some("https" | "http")) {
            return Err("a URL precisa começar com https:// (ou http://)".into());
        }
        if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("a URL tem espaço ou caractere de controle".into());
        }
        if url
            .split_once("://")
            .is_some_and(|(_, r)| r.is_empty() || r.starts_with('/'))
        {
            return Err("a URL não tem servidor".into());
        }
        // A mensagem do parser não cita a entrada (que pode ter senha).
        let mut u = url::Url::parse(url).map_err(|e| format!("URL inválida ({e})"))?;
        if u.host_str().is_none_or(str::is_empty) {
            return Err("a URL não tem servidor".into());
        }
        // `usuario:senha@host` vira Basic, como no cURL: a credencial sai da URL (que aparece em
        // mensagem e é o que o servidor de destino de um redirecionamento recebe) e passa a ser
        // um cabeçalho sensível (mascarado e retirado ao mudar de servidor).
        let mut cabecalhos = Vec::new();
        if !u.username().is_empty() || u.password().is_some() {
            let dec = |s: &str| {
                percent_encoding::percent_decode_str(s)
                    .decode_utf8_lossy()
                    .into_owned()
            };
            let par = format!("{}:{}", dec(u.username()), dec(u.password().unwrap_or("")));
            cabecalhos.push(("Authorization".into(), basic(&par)));
            let _ = u.set_username("");
            let _ = u.set_password(None);
        }
        // O fragmento não vai ao servidor.
        u.set_fragment(None);
        Ok(Self {
            url: u.into(),
            metodo: Metodo::Get,
            cabecalhos,
            corpo: None,
        })
    }

    /// Interpreta um comando cURL sem executar nada: só GET e POST, `-X`, `-H`, `-d`/`--data`
    /// (e `--data-raw`, que o "Copiar como cURL" dos navegadores usa), `-u` e `--compressed`.
    /// Qualquer outra flag é recusada.
    pub fn de_curl(comando: &str) -> Result<Self, String> {
        let partes = palavras(comando)?;
        let mut it = partes.into_iter();
        if it.next().as_deref() != Some("curl") {
            return Err("o comando precisa começar com curl".into());
        }
        let mut url = None;
        let mut metodo = None;
        let mut cabecalhos = Vec::new();
        let mut dados: Vec<String> = Vec::new();
        let mut usuario = None;
        let mut comprimido = false;
        while let Some(p) = it.next() {
            // Valor colado na flag curta (`-XPOST`, `-HAccept: x`).
            let (flag, colado) = match p.as_bytes() {
                [b'-', c, _, ..] if *c != b'-' && b"XHdu".contains(c) => {
                    (p[..2].to_owned(), Some(p[2..].to_owned()))
                }
                _ => (p.clone(), None),
            };
            let mut valor = |nome: &str| {
                colado
                    .clone()
                    .or_else(|| it.next())
                    .ok_or_else(|| format!("{nome} sem valor"))
            };
            match flag.as_str() {
                "-X" | "--request" => metodo = Some(valor(&flag)?),
                "-H" | "--header" => {
                    let v = valor(&flag)?;
                    let (k, val) = v
                        .split_once(':')
                        .ok_or_else(|| "cabeçalho sem ':' (use -H 'Nome: valor')".to_owned())?;
                    let k = k.trim();
                    if k.is_empty() || !k.bytes().all(|b| b.is_ascii_graphic()) {
                        return Err("nome de cabeçalho inválido".into());
                    }
                    cabecalhos.push((k.to_owned(), val.trim().to_owned()));
                }
                "-d" | "--data" | "--data-raw" => {
                    let v = valor(&flag)?;
                    if flag != "--data-raw" && v.starts_with('@') {
                        return Err(format!("{flag} @arquivo lê arquivo local: não suportado"));
                    }
                    dados.push(v);
                }
                "-u" | "--user" => {
                    let v = valor(&flag)?;
                    if !v.contains(':') {
                        return Err("-u sem senha (use -u usuario:senha)".into());
                    }
                    usuario = Some(v);
                }
                "--compressed" => comprimido = true,
                f if f.starts_with('-') && f.len() > 1 => {
                    return Err(format!(
                        "flag do cURL não suportada: {f} (só -X GET|POST, -H, -d/--data, -u e --compressed)"
                    ));
                }
                _ if url.is_some() => return Err("mais de uma URL no comando".into()),
                _ => url = Some(p),
            }
        }
        let mut pedido = Self::de_url(&url.ok_or("o comando não tem URL")?)?;
        pedido.metodo = match metodo.as_deref().map(str::to_ascii_uppercase).as_deref() {
            None if dados.is_empty() => Metodo::Get,
            None | Some("POST") => Metodo::Post,
            Some("GET") if dados.is_empty() => Metodo::Get,
            Some("GET") => return Err("GET com -d não é suportado".into()),
            Some(m) => return Err(format!("método {m} não suportado (só GET e POST)")),
        };
        // `-H Authorization` explícito vale sobre `-u`, que vale sobre `usuario:senha@` da URL.
        let da_url = std::mem::take(&mut pedido.cabecalhos);
        if let Some(u) = usuario
            && !tem(&cabecalhos, "authorization")
        {
            cabecalhos.push(("Authorization".into(), basic(&u)));
        }
        if !tem(&cabecalhos, "authorization") {
            cabecalhos.extend(da_url);
        }
        if comprimido && !tem(&cabecalhos, "accept-encoding") {
            // Só o que o abrir descompacta.
            cabecalhos.push(("Accept-Encoding".into(), "gzip, zstd".into()));
        }
        if !dados.is_empty() {
            // Como o próprio cURL faz com -d.
            if !tem(&cabecalhos, "content-type") {
                cabecalhos.push((
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ));
            }
            pedido.corpo = Some(dados.join("&"));
        }
        pedido.cabecalhos = cabecalhos;
        Ok(pedido)
    }

    /// Nome para mostrar (aba, título): o último trecho do caminho, ou o host; nunca a consulta
    /// nem credencial.
    pub fn nome(&self) -> String {
        let u = sem_consulta(&self.url);
        let resto = u.split_once("://").map_or(u.as_str(), |(_, r)| r);
        resto
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(resto)
            .to_owned()
    }

    /// A origem para mostrar (título do laudo, mensagens): esquema, host e caminho, sem
    /// consulta, credencial nem cabeçalho.
    pub fn exibir(&self) -> String {
        sem_consulta(&self.url)
    }

    /// Se o pedido vai sem cifra (`http://`).
    pub fn inseguro(&self) -> bool {
        self.url[..5].eq_ignore_ascii_case("http:")
    }

    /// Valores que não podem aparecer em mensagem: cabeçalhos sensíveis (e o base64 do `-u`).
    fn segredos(&self) -> Vec<&str> {
        let mut s: Vec<&str> = Vec::new();
        for (k, v) in &self.cabecalhos {
            if sensivel(k) && !v.is_empty() {
                s.push(v);
                // "Bearer xyz": a parte depois do esquema também.
                if let Some((_, resto)) = v.split_once(' ') {
                    s.push(resto.trim());
                }
            }
        }
        s.retain(|x| !x.is_empty());
        s
    }

    /// Mensagem sem segredo nem URL com consulta.
    fn limpar(&self, msg: &str) -> String {
        let mut m = msg.replace(&self.url, &sem_consulta(&self.url));
        for s in self.segredos() {
            m = m.replace(s, "***");
        }
        m
    }
}

fn basic(usuario_senha: &str) -> String {
    use base64::Engine;
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(usuario_senha)
    )
}

fn tem(cab: &[(String, String)], nome: &str) -> bool {
    cab.iter().any(|(k, _)| k.eq_ignore_ascii_case(nome))
}

/// Palavras de uma linha de shell, como o `sh` separaria, sem expandir nada: aspas simples,
/// duplas, `$'…'` e `\`; `\` + quebra de linha continua. Variável, substituição de comando,
/// pipe, `;`, `&` e redirecionamento são recusados (nada roda).
fn palavras(s: &str) -> Result<Vec<String>, String> {
    let recusa = |c: char| {
        Err(format!(
            "o comando tem '{c}' fora de aspas: só cURL simples, nada de shell"
        ))
    };
    let mut out = Vec::new();
    let mut atual = String::new();
    let mut tem_palavra = false;
    let mut cs = s.chars().peekable();
    while let Some(c) = cs.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if tem_palavra {
                    out.push(std::mem::take(&mut atual));
                    tem_palavra = false;
                }
            }
            '\\' => match cs.next() {
                Some('\n') => {}
                Some('\r') if cs.peek() == Some(&'\n') => {
                    cs.next();
                }
                Some(x) => {
                    atual.push(x);
                    tem_palavra = true;
                }
                None => return Err("o comando termina em \\".into()),
            },
            '\'' => {
                tem_palavra = true;
                loop {
                    match cs.next() {
                        Some('\'') => break,
                        Some(x) => atual.push(x),
                        None => return Err("aspas simples sem fechar".into()),
                    }
                }
            }
            '"' => {
                tem_palavra = true;
                loop {
                    match cs.next() {
                        Some('"') => break,
                        Some('\\') => match cs.next() {
                            Some(x @ ('"' | '\\' | '$' | '`')) => atual.push(x),
                            Some('\n') => {}
                            Some(x) => {
                                atual.push('\\');
                                atual.push(x);
                            }
                            None => return Err("aspas duplas sem fechar".into()),
                        },
                        Some(x @ ('$' | '`')) => return recusa(x),
                        Some(x) => atual.push(x),
                        None => return Err("aspas duplas sem fechar".into()),
                    }
                }
            }
            '$' if cs.peek() == Some(&'\'') => {
                cs.next();
                tem_palavra = true;
                ansi_c(&mut cs, &mut atual)?;
            }
            '$' | '`' | '|' | ';' | '&' | '<' | '>' | '(' | ')' => return recusa(c),
            x => {
                atual.push(x);
                tem_palavra = true;
            }
        }
    }
    if tem_palavra {
        out.push(atual);
    }
    Ok(out)
}

/// Corpo de `$'…'` (o "Copiar como cURL" usa quando o valor tem aspas ou quebra de linha).
fn ansi_c(cs: &mut std::iter::Peekable<std::str::Chars>, atual: &mut String) -> Result<(), String> {
    // `\xNN` são bytes: acumulam e viram texto juntos, em UTF-8, como no bash (`\xc3\xa9` = é).
    let mut bytes: Vec<u8> = Vec::new();
    let fechar = |bytes: &mut Vec<u8>, atual: &mut String| {
        if !bytes.is_empty() {
            atual.push_str(&String::from_utf8_lossy(bytes));
            bytes.clear();
        }
    };
    loop {
        let c = cs.next();
        if c == Some('\\') && cs.peek() == Some(&'x') {
            cs.next();
            let h: String = (0..2).filter_map(|_| cs.next()).collect();
            bytes.push(u8::from_str_radix(&h, 16).map_err(|_| "\\x inválido em $'…'")?);
            continue;
        }
        fechar(&mut bytes, atual);
        match c {
            Some('\'') => return Ok(()),
            Some('\\') => match cs.next() {
                Some('n') => atual.push('\n'),
                Some('t') => atual.push('\t'),
                Some('r') => atual.push('\r'),
                Some(x @ ('\\' | '\'' | '"')) => atual.push(x),
                Some('u') => {
                    let h: String = (0..4).filter_map(|_| cs.next()).collect();
                    let c = u32::from_str_radix(&h, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or("\\u inválido em $'…'")?;
                    atual.push(c);
                }
                Some(x) => return Err(format!("escape \\{x} não suportado em $'…'")),
                None => return Err("$'…' sem fechar".into()),
            },
            Some(x) => atual.push(x),
            None => return Err("$'…' sem fechar".into()),
        }
    }
}

/// Como baixar. `Default` = cache do sistema, 2 GiB, 5 min, sem `http://`.
pub struct Opcoes<'a> {
    /// Pasta do cache; `None` = a dos descompactados (ver `compactado::Opcoes::dir_cache`).
    pub dir_cache: Option<PathBuf>,
    /// Tamanho máximo do que o servidor manda (compactado, se vier compactado).
    pub max_bytes: u64,
    /// Teto de relógio do pedido inteiro (conexão, redirecionamentos e corpo).
    pub tempo_max: Duration,
    /// Aceita `http://` (quem chama já avisou o usuário).
    pub permitir_http: bool,
    pub cancelar: Option<&'a AtomicBool>,
    /// Chamado com (bytes recebidos, total declarado ou 0), no máximo a cada 1 MiB.
    pub progresso: Option<&'a dyn Fn(u64, u64)>,
    /// Certificados raiz (DER) no lugar dos do sistema. Só os testes usam.
    #[doc(hidden)]
    pub raizes: Vec<Vec<u8>>,
}

impl Default for Opcoes<'_> {
    fn default() -> Self {
        Self {
            dir_cache: None,
            max_bytes: 2 << 30,
            tempo_max: Duration::from_secs(300),
            permitir_http: false,
            cancelar: None,
            progresso: None,
            raizes: Vec::new(),
        }
    }
}

fn erro(kind: ErrorKind, msg: impl Into<String>) -> io::Error {
    io::Error::new(kind, msg.into())
}

/// Baixa `pedido` para um temporário `download` (ver `entrada::criar`: 0600, apagado no `Drop`,
/// no sinal se quem chama ligou `entrada::apagar_no_sinal`, ou como órfão) e o devolve; quem
/// chama o segura enquanto o arquivo estiver aberto. Erro, cancelamento ou teto apagam o
/// temporário. A mensagem de erro nunca traz credencial.
pub fn baixar(pedido: &Pedido, op: &Opcoes) -> io::Result<Temporario> {
    if pedido.inseguro() && !op.permitir_http {
        return Err(erro(ErrorKind::PermissionDenied, ERRO_HTTP));
    }
    let cop = compactado::Opcoes {
        dir_cache: op.dir_cache.clone(),
        ..compactado::Opcoes::default()
    };
    let (tmp, mut saida) = entrada::criar("download", &cop)?;
    transferir(pedido, op, &mut saida).map_err(|e| {
        // O cancelamento segue reconhecível (`compactado::cancelada`); não traz texto do pedido.
        if compactado::cancelada(&e) {
            e
        } else {
            erro(e.kind(), pedido.limpar(&e.to_string()))
        }
    })?;
    saida.sync_all()?;
    Ok(tmp)
}

fn agente(pedido: &Pedido, op: &Opcoes) -> ureq::Agent {
    use ureq::tls::{Certificate, RootCerts, TlsConfig};
    let mut tls = TlsConfig::builder();
    if !op.raizes.is_empty() {
        let certs: Vec<Certificate<'static>> = op
            .raizes
            .iter()
            .map(|d| Certificate::from_der(d).to_owned())
            .collect();
        tls = tls.root_certs(RootCerts::new_with_certs(&certs));
    }
    ureq::Agent::config_builder()
        .tls_config(tls.build())
        .https_only(!pedido.inseguro())
        // Os redirecionamentos são seguidos à mão (ver `transferir`), para controlar o que vai
        // ao servidor seguinte.
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent(concat!("jasao/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

fn traduzir(e: ureq::Error, op: &Opcoes) -> io::Error {
    use ureq::Error as E;
    match e {
        E::Timeout(_) => tempo_esgotado(op),
        E::RequireHttpsOnly(_) => erro(
            ErrorKind::PermissionDenied,
            "redirecionamento de https para http recusado",
        ),
        E::Tls(_) | E::Rustls(_) => erro(
            ErrorKind::InvalidData,
            format!("certificado TLS recusado ({e})"),
        ),
        E::Io(io) if io.kind() == ErrorKind::TimedOut => tempo_esgotado(op),
        E::Io(io) if io.get_ref().is_some_and(|i| i.is::<rustls::Error>()) => erro(
            ErrorKind::InvalidData,
            format!("certificado TLS recusado ({io})"),
        ),
        E::Io(io) => io,
        e => erro(ErrorKind::Other, e.to_string()),
    }
}

fn tempo_esgotado(op: &Opcoes) -> io::Error {
    erro(
        ErrorKind::TimedOut,
        format!("tempo esgotado ({} s)", op.tempo_max.as_secs_f32()),
    )
}

fn grande(op: &Opcoes) -> io::Error {
    erro(
        ErrorKind::FileTooLarge,
        format!(
            "o conteúdo passa de {} MiB, o teto do download; baixe fora do app e abra o arquivo",
            op.max_bytes.div_ceil(MIB)
        ),
    )
}

type Resposta = ureq::http::Response<ureq::Body>;

/// Um pedido, sem seguir redirecionamento, com o tempo que sobra do teto.
fn enviar(
    agente: &ureq::Agent,
    url: &str,
    metodo: Metodo,
    cab: &[(String, String)],
    corpo: Option<&str>,
    tempo: Duration,
) -> Result<Resposta, ureq::Error> {
    match metodo {
        Metodo::Get => {
            let mut r = agente.get(url).config().timeout_global(Some(tempo)).build();
            for (k, v) in cab {
                r = r.header(k, v);
            }
            r.call()
        }
        Metodo::Post => {
            let mut r = agente
                .post(url)
                .config()
                .timeout_global(Some(tempo))
                .build();
            for (k, v) in cab {
                r = r.header(k, v);
            }
            r.send(corpo.unwrap_or(""))
        }
    }
}

/// Faz o pedido seguindo até `MAX_REDIRECIONAMENTOS` saltos. A cada salto: nunca de https para
/// http; para outro servidor (esquema, host ou porta), os cabeçalhos sensíveis (`sensivel`) não
/// vão, e 307/308 de POST é recusado, com ou sem corpo (repetiria o POST a quem o usuário não escolheu);
/// 303, e 301/302 de POST, viram GET sem corpo (como navegadores e cURL).
fn pedir(pedido: &Pedido, op: &Opcoes) -> io::Result<Resposta> {
    let agente = agente(pedido, op);
    let inicio = Instant::now();
    let mut url = url::Url::parse(&pedido.url)
        .map_err(|e| erro(ErrorKind::InvalidInput, format!("URL inválida ({e})")))?;
    let mut metodo = pedido.metodo;
    let mut corpo = pedido.corpo.clone();
    let mut cab = pedido.cabecalhos.clone();
    let mut saltos = 0;
    loop {
        let tempo = op
            .tempo_max
            .checked_sub(inicio.elapsed())
            .filter(|t| !t.is_zero())
            .ok_or_else(|| tempo_esgotado(op))?;
        let resp = enviar(&agente, url.as_str(), metodo, &cab, corpo.as_deref(), tempo)
            .map_err(|e| traduzir(e, op))?;
        let st = resp.status().as_u16();
        let destino = match resp.headers().get("location") {
            Some(l) if matches!(st, 301 | 302 | 303 | 307 | 308) => l,
            _ => return Ok(resp),
        };
        if saltos == MAX_REDIRECIONAMENTOS {
            return Err(erro(
                ErrorKind::InvalidData,
                format!("mais de {MAX_REDIRECIONAMENTOS} redirecionamentos"),
            ));
        }
        saltos += 1;
        let prox = destino
            .to_str()
            .ok()
            .and_then(|l| url.join(l).ok())
            .ok_or_else(|| {
                erro(
                    ErrorKind::InvalidData,
                    "redirecionamento sem destino válido",
                )
            })?;
        match (url.scheme(), prox.scheme()) {
            (_, "https") | ("http", "http") => {}
            ("https", "http") => {
                return Err(erro(
                    ErrorKind::PermissionDenied,
                    "redirecionamento de https para http recusado",
                ));
            }
            _ => {
                return Err(erro(
                    ErrorKind::InvalidData,
                    "redirecionamento para esquema não suportado",
                ));
            }
        }
        if prox.origin() != url.origin() {
            // 307/308 repetem o método: um POST (com ou sem corpo) não vai a quem o usuário não escolheu.
            if matches!(st, 307 | 308) && (corpo.is_some() || metodo == Metodo::Post) {
                return Err(erro(
                    ErrorKind::PermissionDenied,
                    format!(
                        "redirecionamento {st} de POST para outro servidor ({}) recusado",
                        sem_consulta(prox.as_str())
                    ),
                ));
            }
            cab.retain(|(k, _)| !sensivel(k));
        }
        if st == 303 || (matches!(st, 301 | 302) && metodo == Metodo::Post) {
            metodo = Metodo::Get;
            corpo = None;
            cab.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
        }
        url = prox;
    }
}

fn transferir(pedido: &Pedido, op: &Opcoes, saida: &mut File) -> io::Result<()> {
    let resp = pedir(pedido, op)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(erro(
            ErrorKind::Other,
            format!(
                "o servidor respondeu HTTP {} {} para {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                sem_consulta(&pedido.url)
            ),
        ));
    }
    let cod = resp
        .headers()
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("identity")
        .trim()
        .to_ascii_lowercase();
    // gzip e zstd saem no abrir, pelos bytes mágicos; o resto não daria para ler.
    if !matches!(cod.as_str(), "identity" | "gzip" | "x-gzip" | "zstd" | "") {
        return Err(erro(
            ErrorKind::InvalidData,
            format!("o servidor mandou compactado em {cod}, que o Jasão não lê (só gzip e zstd)"),
        ));
    }
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
        .unwrap_or(0);
    if total > op.max_bytes {
        return Err(grande(op));
    }
    let mut corpo = resp.into_body().into_with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 256 * 1024];
    let (mut lidos, mut avisado) = (0u64, 0u64);
    loop {
        if op.cancelar.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(compactado::cancelado());
        }
        let n = match corpo.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::TimedOut => return Err(tempo_esgotado(op)),
            Err(e) => return Err(e),
        };
        lidos += n as u64;
        if lidos > op.max_bytes {
            return Err(grande(op));
        }
        saida.write_all(&buf[..n])?;
        if let Some(p) = op.progresso
            && lidos - avisado >= MIB
        {
            avisado = lidos;
            p(lidos, total);
        }
    }
    if let Some(p) = op.progresso {
        p(lidos, total.max(lidos));
    }
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn curl_do_navegador() {
        let p = Pedido::interpretar(
            "curl 'https://api.example.com/v1/x?a=1' \\\n  -H 'accept: application/json' \\\n  -H $'x-nota: it\\'s' \\\n  --data-raw '{\"q\":1}' --compressed",
        )
        .unwrap();
        assert_eq!(p.url, "https://api.example.com/v1/x?a=1");
        assert_eq!(p.metodo, Metodo::Post);
        assert_eq!(p.corpo.as_deref(), Some("{\"q\":1}"));
        assert!(p.cabecalhos.contains(&("x-nota".into(), "it's".into())));
        assert!(tem(&p.cabecalhos, "accept-encoding"));
    }

    #[test]
    fn curl_recusa_o_resto() {
        for c in [
            "curl -L https://example.com",
            "curl -sS https://example.com",
            "curl -o x https://example.com",
            "curl -X PUT https://example.com",
            "curl https://example.com; rm -rf ~",
            "curl https://example.com | sh",
            "curl \"https://example.com/$HOME\"",
            "curl -d @segredo.txt https://example.com",
            "curl -u fulano https://example.com",
            "curl ftp://example.com",
            "curl https://a.example https://b.example",
        ] {
            assert!(Pedido::interpretar(c).is_err(), "{c}");
        }
        let e = Pedido::interpretar("curl -k https://example.com").unwrap_err();
        assert!(e.contains("-k") && e.contains("não suportada"), "{e}");
    }

    #[test]
    fn curl_usuario_vira_basic_e_some_do_debug() {
        let p = Pedido::interpretar("curl -u fulano:s3nh4 -XGET https://example.com/x?token=abc")
            .unwrap();
        assert_eq!(p.metodo, Metodo::Get);
        let (_, v) = p
            .cabecalhos
            .iter()
            .find(|(k, _)| k == "Authorization")
            .unwrap();
        assert_eq!(v, "Basic ZnVsYW5vOnMzbmg0");
        let d = format!("{p:?}");
        assert!(!d.contains("ZnVsYW5v") && !d.contains("abc"), "{d}");
        assert_eq!(
            p.limpar(&format!("falhou {v} em {}", p.url)),
            "falhou *** em https://example.com/x"
        );
    }

    #[test]
    fn url_simples() {
        assert!(Pedido::interpretar("https://example.com/a.json").is_ok());
        assert!(
            Pedido::interpretar("HTTP://example.com")
                .unwrap()
                .inseguro()
        );
        assert!(Pedido::interpretar("file:///etc/passwd").is_err());
        assert!(Pedido::interpretar("https:///x").is_err());
        assert!(Pedido::interpretar("https://exa mple.com").is_err());
        assert_eq!(
            sem_consulta("https://u:p@h.example/a?b=c#d"),
            "https://h.example/a"
        );
        let nome = |u: &str| Pedido::de_url(u).unwrap().nome();
        assert_eq!(nome("https://h.example/dados/a.jsonl?token=x"), "a.jsonl");
        assert_eq!(nome("https://u:p@h.example/"), "h.example");
    }

    #[test]
    fn senha_com_interrogacao_ou_cerquilha_nao_vaza() {
        // O corte manual em `?`/`#` mostrava "https://u:a" (com a senha "a"); o parser WHATWG
        // não aceita a autoridade, e nada da entrada aparece.
        for u in ["https://u:a?b@h.example/x", "https://u:a#b@h.example/x"] {
            let e = Pedido::de_url(u).unwrap_err();
            assert!(
                !e.contains("a?b") && !e.contains("a#b") && !e.contains("u:a"),
                "{e}"
            );
            assert_eq!(sem_consulta(u), "(URL inválida)");
        }
        // Codificada, a senha é aceita, sai da URL e vira Basic.
        let p = Pedido::de_url("https://fu%40l:s%23n?q=1#f").unwrap_err();
        assert!(!p.contains("s%23n"), "{p}");
        let p = Pedido::de_url("https://fu%40l:s%3Fn%23x@h.example:8443/a/b.jsonl?t=1#f").unwrap();
        assert_eq!(p.url, "https://h.example:8443/a/b.jsonl?t=1");
        assert_eq!(p.exibir(), "https://h.example:8443/a/b.jsonl");
        assert_eq!(
            p.cabecalhos,
            vec![("Authorization".into(), basic("fu@l:s?n#x"))]
        );
        // -u vale sobre a URL; -H Authorization vale sobre os dois.
        let c = |s: &str| Pedido::interpretar(s).unwrap().cabecalhos;
        assert_eq!(
            c("curl -u a:b https://x:y@h.example/"),
            vec![("Authorization".into(), basic("a:b"))]
        );
        assert_eq!(
            c("curl -H 'Authorization: Bearer t' -u a:b https://x:y@h.example/"),
            vec![("Authorization".into(), "Bearer t".into())]
        );
    }

    #[test]
    fn ansi_c_hex_e_utf8() {
        let p =
            Pedido::interpretar("curl -H $'x-nome: Jos\\xc3\\xa9 \\x41\\u00e9' https://h.example/")
                .unwrap();
        assert_eq!(p.cabecalhos, vec![("x-nome".into(), "José Aé".into())]);
    }
}
