// Visão Texto: linhas cruas do arquivo com realce. O Rust manda bytes (ver core/src/texto.rs);
// aqui só se decodifica e se marca cada trecho com o byte onde ele começa na linha.

export interface JanelaTexto {
  inicio: number;
  tamanhos: number[]; // tamanho completo de cada linha em bytes (a linha pode vir cortada)
  linhas: string[];
}

const dec = new TextDecoder();
const enc = new TextEncoder();

// shortcut: UTF-8 inválido vira U+FFFD (3 bytes) e desloca os bytes do resto daquela linha;
// a linha já aparece como inválida na árvore, então o clique nela cai no registro.
export function decodificar(inicio: number, buf: ArrayBuffer): JanelaTexto {
  const b = new Uint8Array(buf);
  const nl = b.indexOf(10);
  const cab = dec.decode(nl < 0 ? b : b.subarray(0, nl));
  const tamanhos = cab ? cab.split(" ").map(Number) : [];
  const linhas = nl < 0 ? [] : dec.decode(b.subarray(nl + 1)).split("\n");
  return { inicio, tamanhos, linhas: linhas.slice(0, tamanhos.length) };
}

// Controles bidi (U+061C, U+200E/F, U+202A–E, U+2066–9) e de largura zero (U+200B–D, U+2060, U+FEFF).
const INVISIVEIS = /[\u061c\u200b-\u200f\u202a-\u202e\u2060\u2066-\u2069\ufeff]/g;

/** Texto do arquivo como HTML: escapado e com cada invisível num `.inv` que mostra o código (CSS)
 * e o isola. O caractere continua no DOM: seleção, posição em bytes e cópia ficam iguais. */
export const visivel = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(INVISIVEIS, (c) => `<span class="inv" data-u="${codigo(c)}">${c}</span>`);
const codigo = (c: string) => c.charCodeAt(0).toString(16).toUpperCase().padStart(4, "0");
const esc = visivel;
/** O mesmo neutralizador para atributo (`title`, `aria-label`) e `textContent`, onde não há
 * HTML: cada invisível vira o código à vista, `⟦202E⟧`. */
export const visivelTexto = (s: string) =>
  s.replace(INVISIVEIS, (c) => `⟦${c.charCodeAt(0).toString(16).toUpperCase().padStart(4, "0")}⟧`);

/** Texto que vem do arquivo fora do conteúdo (`title`, nome de aba, recentes): cada invisível
 * trocado pelo código, `<U+202E>`, porque ali não cabe o `.inv` e o caractere cru inverteria o que
 * o usuário vê ou o leitor de tela lê. Para `textContent`/`title` pelo DOM. */
export const semInvisiveis = (s: string) => s.replace(INVISIVEIS, (c) => `<U+${codigo(c)}>`);

/** Ícone do sprite SVG de `index.html`, decorativo. */
export const icone = (id: string) => `<svg class="i" aria-hidden="true"><use href="#${id}"/></svg>`;

/** `semInvisiveis` escapado para atributo em HTML montado como texto. */
export const escAtributo = (s: string) =>
  semInvisiveis(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");

// Chave (texto seguido de `:`), texto, número, literal, pontuação, espaço, qualquer outro.
const TOKEN = /("(?:[^"\\]|\\.)*"?)(?=\s*:)|("(?:[^"\\]|\\.)*"?)|(-?\d[\d.eE+-]*)|(true|false|null)\b|([{}[\],:])|(\s+)|(.)/gu;
const CLASSES = ["k", "s", "n", "l", "p", "", ""];

/** Achado da busca na linha: bytes `[início, fim)` e se é o atual. */
export type Marca = [number, number, boolean];

const bytesDe = (cp: number) => (cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4);

/** Corta `tok`, que começa no byte `b` da linha, nos limites das marcas: `[texto, byte, marca]`,
 * com marca 0 (fora), 1 (achado) ou 2 (achado atual). */
export function partir(tok: string, b: number, marcas: Marca[]): [string, number, number][] {
  const fim = b + enc.encode(tok).length;
  // As marcas vêm do núcleo em ordem e sem sobreposição: busca binária pela primeira que termina
  // depois de `b` e um ponteiro que só anda. Linear na linha, não caracteres × marcas.
  let j = 0;
  for (let hi = marcas.length; j < hi; ) {
    const m = (j + hi) >> 1;
    if (marcas[m][1] <= b) j = m + 1;
    else hi = m;
  }
  if (j === marcas.length || marcas[j][0] >= fim) return [[tok, b, 0]];
  const pedacos: [string, number, number][] = [];
  let antes: Marca | undefined | null = null; // marca do pedaço anterior (null: nenhum ainda)
  let x = b;
  for (const ch of tok) {
    while (j < marcas.length && marcas[j][1] <= x) j++;
    const m = j < marcas.length && marcas[j][0] <= x ? marcas[j] : undefined;
    if (m === antes) pedacos[pedacos.length - 1][0] += ch;
    else pedacos.push([ch, x, m ? (m[2] ? 2 : 1) : 0]);
    antes = m;
    x += bytesDe(ch.codePointAt(0)!);
  }
  return pedacos;
}

const CLASSE_MARCA = ["", "ac", "ac at"];

/** `texto` (a partir do byte `b`) escapado, com os achados em `<mark>`: prévia da árvore. */
export function marcado(texto: string, b: number, marcas: Marca[]): string {
  return partir(texto, b, marcas)
    .map(([s, , k]) => (k ? `<mark class="${CLASSE_MARCA[k]}">${esc(s)}</mark>` : esc(s)))
    .join("");
}

/** HTML de uma linha; trechos dentro de `faixa` (bytes) ganham realce e os achados, marca. */
export function linhaHtml(
  texto: string,
  tamanho: number,
  faixa: [number, number] | null,
  corte: (n: number) => string,
  marcas: Marca[] = [],
): string {
  let html = "";
  let b = 0;
  for (const m of texto.matchAll(TOKEN)) {
    const tok = m[0];
    const n = enc.encode(tok).length;
    const cls = CLASSES[m.slice(1).findIndex((g) => g !== undefined)];
    const hl = faixa && b >= faixa[0] && b + n <= faixa[1] && faixa[1] > faixa[0];
    for (const [pedaco, x, k] of partir(tok, b, marcas)) {
      const c = [cls, hl ? "hl" : "", CLASSE_MARCA[k]].filter(Boolean).join(" ");
      html += `<span data-b="${x}"${c ? ` class="${c}"` : ""}>${esc(pedaco)}</span>`;
    }
    b += n;
  }
  if (tamanho > b) html += `<span class="corte">${esc(corte(tamanho - b))}</span>`;
  return html;
}

/** Texto numa linha só, cortado em `n` caracteres (confirmação de cópia). */
export function resumir(s: string, n = 60): string {
  const c = [...s.slice(0, n * 4).replace(/\s+/g, " ").trim()];
  return c.length > n ? `${c.slice(0, n - 1).join("")}…` : c.join("");
}

/** Índice (UTF-16) da linha e coluna, a partir de 1, com a coluna em caracteres (erro do filtro jq). */
export function indiceDe(s: string, linha: number, coluna: number): number {
  let i = 0;
  for (let l = 1; l < linha; l++) {
    const n = s.indexOf("\n", i);
    if (n < 0) return s.length;
    i = n + 1;
  }
  for (let c = 1; c < coluna && i < s.length && s[i] !== "\n"; c++) i += s.codePointAt(i)! > 0xffff ? 2 : 1;
  return i;
}
