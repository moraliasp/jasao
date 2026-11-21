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

/** HTML de uma linha; trechos dentro de `faixa` (bytes) ganham realce. */
export function linhaHtml(texto: string, tamanho: number, faixa: [number, number] | null, corte: (n: number) => string): string {
  let html = "";
  let b = 0;
  for (const m of texto.matchAll(TOKEN)) {
    const tok = m[0];
    const n = enc.encode(tok).length;
    const cls = CLASSES[m.slice(1).findIndex((g) => g !== undefined)];
    const hl = faixa && b >= faixa[0] && b + n <= faixa[1] && faixa[1] > faixa[0];
    const c = [cls, hl ? "hl" : ""].filter(Boolean).join(" ");
    html += `<span data-b="${b}"${c ? ` class="${c}"` : ""}>${esc(tok)}</span>`;
    b += n;
  }
  if (tamanho > b) html += `<span class="corte">${esc(corte(tamanho - b))}</span>`;
  return html;
}
