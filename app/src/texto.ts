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

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;");

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
  if (!marcas.some(([i, f]) => i < fim && f > b)) return [[tok, b, 0]];
  const pedacos: [string, number, number][] = [];
  let antes: Marca | undefined | null = null; // marca do pedaço anterior (null: nenhum ainda)
  let x = b;
  for (const ch of tok) {
    const m = marcas.find(([i, f]) => x >= i && x < f);
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
