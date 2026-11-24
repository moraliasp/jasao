// Valores inteligentes: reconhece no texto de um valor (época, data ISO, cor, URL, imagem em
// base64, JWT, JSON dentro de texto) o que dá para mostrar melhor. Puro: sem DOM, sem rede, sem
// relógio (quem chama passa `agora`). Testes em app/tests/valores.test.ts.

/** Teto do texto que passa por decodificação de base64 (imagem, JWT): 1 MiB. */
export const MAX_BASE64 = 1 << 20;
/** Teto do texto que passa por `JSON.parse` como JSON dentro de texto: 1 MiB. */
export const MAX_JSON_TEXTO = 1 << 20;
/** Teto de aninhamento ao reindentar: a indentação cresce com o nível ao quadrado. */
export const MAX_NIVEL = 100;
/** Teto de uma URL que vira link. */
export const MAX_URL = 2048;
/** Igual a `nucleo::arvore::MAX_FORMATADO`: reindentado maior que isso fica no texto cru. */
export const MAX_FORMATADO = 16 << 20;

export type Inteligente =
  | { tipo: "epoca"; ms: number; unidade: "s" | "ms" }
  | { tipo: "data"; ms: number }
  | { tipo: "cor"; css: string }
  | { tipo: "url"; href: string } // `href` normalizado (ASCII, punycode): é o que se mostra e se abre
  | { tipo: "imagem"; src: string; formato: string; bytes: number } // `src` é sempre data URI base64
  | { tipo: "jwt"; cabecalho: string; carga: string } // JSON reindentado; a assinatura nunca sai daqui
  | { tipo: "json"; texto: string; itens: number };

/** Pista quando a prévia da árvore veio cortada: só o tipo, o resto vem do painel. */
export type Pista = "imagem" | "jwt" | "json" | "url";

// 2001-01-01 a 2100-01-01: fora disso um número de 10 ou 13 dígitos é mais provável ser id.
const EPOCA_MIN = 978307200000;
const EPOCA_MAX = 4102444800000;
// 10 e 13 dígitos também são id, EAN-13 e afins: só vira data com nome de campo que pareça data,
// com fronteira de palavra (snake_case ou camelCase), para `item`, `lat`, `format` não casarem.
const PISTA_SNAKE = /(^|[_-])(ts|ms|data|date|time|hora|em|at|desde|quando|momento|criado|atualizado|timestamp|epoch)$/i;
const PISTA_CAMEL = /[a-z\d](Em|At|Ts|Ms|Data|Date|Time|Hora|Timestamp)$/;
const ISO = /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,9}))?)?(Z|([+-])(\d{2}):?(\d{2}))?)?$/;
const HEX = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
const CANAL = String.raw`\s*(\d{1,3}(\.\d+)?%?)\s*`;
const ALFA = String.raw`\s*(0|1|0?\.\d+|\d{1,3}%)\s*`;
const RGB = new RegExp(`^rgba?\\(${CANAL},${CANAL},${CANAL}(,${ALFA})?\\)$`, "i");
const DATA_URI = /^data:image\/(png|jpeg|gif|webp|bmp|avif|svg\+xml);base64,([A-Za-z0-9+/]+={0,2})$/;
const B64 = /^[A-Za-z0-9+/]+={0,2}$/;
const DATA_URI_INICIO = /^data:image\/(png|jpeg|gif|webp|bmp|avif|svg\+xml);base64,[A-Za-z0-9+/]*={0,2}$/;
const B64_INICIO = /^[A-Za-z0-9+/]{16,}={0,2}$/;
// Base64 cru sem `data:`: só os formatos com assinatura inequívoca no começo.
const MAGICA: [string, string][] = [["iVBORw0KGgo", "png"], ["/9j/", "jpeg"], ["R0lGOD", "gif"]];
const JWT = /^(eyJ[A-Za-z0-9_-]+)\.(eyJ[A-Za-z0-9_-]+)\.[A-Za-z0-9_-]*$/;

/**
 * O que mostrar para o valor. `tipo` é o tipo do nó na árvore; `valor` é o texto já decodificado
 * (texto) ou cru (número); `chave`, o nome do campo, decide época em segundos.
 */
export function detectar(tipo: string, valor: string, chave: string | null): Inteligente | null {
  if (tipo === "numero") return epoca(valor, chave);
  if (tipo !== "texto" || !valor) return null;
  const s = valor;
  const iso = ISO.exec(s);
  if (iso) {
    const ms = dataIso(iso);
    return ms === null ? null : { tipo: "data", ms };
  }
  if (HEX.test(s) || (RGB.test(s) && canaisValidos(s))) return { tipo: "cor", css: s };
  if (/^https?:\/\//i.test(s)) return url(s);
  if (s.length > MAX_BASE64) return null;
  const img = imagem(s);
  if (img) return img;
  const jwt = JWT.exec(s);
  if (jwt) {
    const cabecalho = jsonDe(base64url(jwt[1]));
    const carga = jsonDe(base64url(jwt[2]));
    if (cabecalho && carga) return { tipo: "jwt", cabecalho, carga };
  }
  return jsonNoTexto(s);
}

/**
 * Pista pela prévia cortada da árvore (o começo do texto, sem as aspas finais), conferida no que a
 * prévia já mostra: cada caractere no alfabeto do tipo, cabeçalho do JWT decodificado, começo de
 * URL válido. `bytes` é o tamanho do valor no arquivo, para não prometer o que passa dos tetos.
 * JSON não dá para confirmar pelo começo (`[2026-11-23] log` também começa com `[`): a pista
 * "json" só diz ao painel que vale buscar o valor inteiro, e o selo da árvore não a mostra.
 */
export function pista(inicio: string, bytes: number): Pista | null {
  if (bytes <= MAX_BASE64 && (DATA_URI_INICIO.test(inicio) || (MAGICA.some(([m]) => inicio.startsWith(m)) && B64_INICIO.test(inicio))))
    return "imagem";
  const jwt = /^(eyJ[A-Za-z0-9_-]+)\.eyJ[A-Za-z0-9_-]*$/.exec(inicio);
  if (jwt && bytes <= MAX_BASE64 && /"alg"\s*:/.test(jsonDe(base64url(jwt[1])) ?? "")) return "jwt";
  if (bytes <= MAX_URL && /^https?:\/\//i.test(inicio) && !/[\s\p{Cc}"\\]/u.test(inicio) && URL.canParse(inicio)) return "url";
  if (bytes <= MAX_JSON_TEXTO && /^\s*[{[]/.test(inicio)) return "json";
  return null;
}

/** Host para mostrar no selo de link (vazio se não der). */
export function hostDe(u: string): string {
  try {
    return new URL(u).host;
  } catch {
    return "";
  }
}

function epoca(cru: string, chave: string | null): Inteligente | null {
  if (!/^\d{10}(\d{3})?$/.test(cru)) return null;
  const emMs = cru.length === 13;
  const ms = emMs ? Number(cru) : Number(cru) * 1000;
  if (ms < EPOCA_MIN || ms >= EPOCA_MAX) return null;
  if (!chave || !(PISTA_SNAKE.test(chave) || PISTA_CAMEL.test(chave))) return null;
  return { tipo: "epoca", ms, unidade: emMs ? "ms" : "s" };
}

/**
 * Instante da data ISO, conferindo cada componente (2024-02-30 e 25:00 não passam) sem depender
 * do `Date.parse`. Sem fuso, é hora local, também na forma só com a data.
 */
function dataIso(m: RegExpExecArray): number | null {
  const [a, me, d, h = 0, mi = 0, se = 0] = [1, 2, 3, 4, 5, 6].map((i) => (m[i] === undefined ? undefined : Number(m[i])));
  const ms = m[7] ? Number(m[7].slice(0, 3).padEnd(3, "0")) : 0;
  if (me! < 1 || me! > 12 || h! > 23 || mi! > 59 || se! > 59) return null;
  const dia = new Date(Date.UTC(a!, me! - 1, d!));
  if (dia.getUTCFullYear() !== a || dia.getUTCMonth() !== me! - 1 || dia.getUTCDate() !== d) return null;
  if (!m[8]) return new Date(a!, me! - 1, d!, h, mi, se, ms).getTime();
  const [oh, om] = [Number(m[10] ?? 0), Number(m[11] ?? 0)];
  if (oh > 23 || om > 59) return null;
  const desvio = m[8] === "Z" ? 0 : (m[9] === "-" ? -1 : 1) * (oh * 60 + om) * 6e4;
  return Date.UTC(a!, me! - 1, d!, h, mi, se, ms) - desvio;
}

function canaisValidos(s: string): boolean {
  const m = RGB.exec(s)!;
  return [m[1], m[3], m[5]].every((c) => (c.endsWith("%") ? parseFloat(c) <= 100 : parseFloat(c) <= 255));
}

function url(s: string): Inteligente | null {
  if (s.length > MAX_URL || /[\s\p{Cc}]/u.test(s)) return null;
  try {
    const u = new URL(s);
    return (u.protocol === "http:" || u.protocol === "https:") && u.hostname ? { tipo: "url", href: u.href } : null;
  } catch {
    return null;
  }
}

function imagem(s: string): Inteligente | null {
  const m = DATA_URI.exec(s);
  if (m) return { tipo: "imagem", src: s, formato: m[1], bytes: bytesB64(m[2]) };
  const magica = MAGICA.find(([p]) => s.startsWith(p));
  if (!magica || s.length < 16 || !B64.test(s)) return null;
  const formato = magica[1];
  return { tipo: "imagem", src: `data:image/${formato};base64,${s}`, formato, bytes: bytesB64(s) };
}

const bytesB64 = (b: string) => Math.floor((b.length * 3) / 4) - (b.endsWith("==") ? 2 : b.endsWith("=") ? 1 : 0);

/** Base64url → texto UTF-8 (`null` se não for UTF-8 válido). */
function base64url(s: string): string | null {
  try {
    const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
    return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
  } catch {
    return null;
  }
}

/** Objeto JSON reindentado (`null` se não for objeto JSON). */
function jsonDe(s: string | null): string | null {
  if (s === null || !/^\s*\{/.test(s)) return null;
  try {
    JSON.parse(s);
  } catch {
    return null;
  }
  return reindentar(s);
}

function jsonNoTexto(s: string): Inteligente | null {
  if (s.length > MAX_JSON_TEXTO || !/^\s*[{[]/.test(s)) return null;
  let v: unknown;
  try {
    v = JSON.parse(s);
  } catch {
    return null; // inclui RangeError de aninhamento fundo demais
  }
  const texto = reindentar(s);
  if (texto === null) return null;
  return { tipo: "json", texto, itens: Array.isArray(v) ? v.length : Object.keys(v as object).length };
}

/**
 * JSON válido com um campo por linha, sem passar os valores por `JSON.parse`: número grande e
 * chave repetida ficam como estão no texto (a mesma regra do `formatado` do núcleo). `null` acima
 * de `MAX_NIVEL` níveis; o próprio `s` (cru) se o reindentado passaria de `MAX_FORMATADO`.
 */
export function reindentar(s: string, passo = "  "): string | null {
  // Pedaços num array (e não `out +=` com regex no fim): vazio só desfaz o último pedaço, e o
  // custo fica linear no tamanho do texto.
  const out: string[] = [];
  let nivel = 0;
  let abriu = false; // o último pedaço é a quebra logo depois de `{`/`[`
  let indentacao = 0; // bytes que a reindentação acrescenta (quebras, recuo, espaço após `:`); os tokens somam no máximo `s.length`
  const quebra = () => {
    indentacao += 1 + passo.length * nivel;
    return "\n" + passo.repeat(nivel);
  };
  for (const [tok] of s.matchAll(/"(?:[^"\\]|\\.)*"|[{}[\],:]|[^\s{}[\],:"]+|\s+/g)) {
    const c = tok[0];
    if (/^\s/.test(c)) continue;
    if (c === "{" || c === "[") {
      if (++nivel > MAX_NIVEL) return null;
      out.push(tok, quebra());
      abriu = true;
      continue;
    }
    if (c === "}" || c === "]") {
      nivel--;
      if (abriu) out.pop(); // vazio: `{` seguido de `}` fica `{}`
      else out.push(quebra());
      out.push(tok);
    } else if (c === ",") out.push(",", quebra());
    else if (c === ":") {
      out.push(": ");
      indentacao++; // o espaço depois de `:`
    }
    else out.push(tok);
    abriu = false;
    if (indentacao + s.length > MAX_FORMATADO) return s;
  }
  return out.join("");
}

const UNIDADES: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 864e5],
  ["month", 30 * 864e5],
  ["week", 7 * 864e5],
  ["day", 864e5],
  ["hour", 36e5],
  ["minute", 6e4],
  ["second", 1e3],
];

/** "há 3 dias", "daqui a 2 horas", no idioma `lang`. */
export function relativo(ms: number, agora: number, lang: string): string {
  const d = ms - agora;
  const [u, tam] = UNIDADES.find(([, t]) => Math.abs(d) >= t) ?? ["second", 1e3];
  return new Intl.RelativeTimeFormat(lang, { numeric: "auto" }).format(Math.round(d / tam), u);
}
