// HTML dos valores inteligentes (roda com `node --test`, sem navegador): todo texto do arquivo
// sai escapado. Tirar `visivel` ou `escAtributo` de `apresenta.ts` derruba estes testes.
import assert from "node:assert/strict";
import { test } from "node:test";
import { html, type No, selo } from "../src/apresenta.ts";
import { detectar, type Inteligente, MAX_FORMATADO } from "../src/valores.ts";
import { daPrevia } from "../src/apresenta.ts";
import { t } from "../src/i18n.ts";

const b64url = (s: string) => Buffer.from(s).toString("base64url");
const HOSTIL = `<img src=x onerror=alert(1)>"'><script>x</script>`;
/** Prévia como `nucleo::arvore::previa`: até 200 bytes, cortada em fronteira de caractere, com `…`. */
export function previaDoNucleo(raw: string): string {
  const b = Buffer.from(raw);
  if (b.length <= 200) return raw;
  let fim = 200;
  while ((b[fim] & 0xc0) === 0x80) fim--;
  return `${b.subarray(0, fim).toString()}…`;
}
const no = (valor: string, chave: string | null = null): No => {
  const raw = JSON.stringify(valor);
  return { tipo: "texto", previa: previaDoNucleo(raw), chave, faixa: [0, Buffer.byteLength(raw)], erro: null };
};

/** Nenhuma tag nem atributo injetado: só as tags que o próprio app monta. */
function semInjecao(h: string, rotulo: string) {
  assert.doesNotMatch(h, /<img src=x|<script|onerror=alert\(1\)"|" onerror/i, rotulo);
  for (const [, tag] of h.matchAll(/<\/?([a-z]+)/gi))
    assert.ok(["div", "pre", "span", "button", "svg", "use", "figure", "img", "figcaption"].includes(tag), `${rotulo}: <${tag}>`);
  // Todo atributo fecha a aspa que abriu: nada de `"` cru dentro de valor.
  assert.doesNotMatch(h.replace(/ [a-z-]+(="[^"]*")?(?=[ >\/])/g, ""), /<[a-z]+[^>]*"/i, rotulo);
}

test("JWT e JSON em texto com carga hostil saem escapados", () => {
  const jwt = `${b64url(`{"alg":"none","k":"${HOSTIL.replace(/"/g, '\\"')}"}`)}.${b64url(`{"nome":"${HOSTIL.replace(/"/g, '\\"')}"}`)}.sig`;
  const j = detectar("texto", jwt, null)!;
  assert.equal(j.tipo, "jwt");
  const hj = html(j);
  semInjecao(hj, "jwt");
  assert.match(hj, /&lt;img src=x onerror=alert\(1\)>/);
  assert.match(selo(no(jwt)), /JWT/);

  const json = detectar("texto", JSON.stringify({ a: HOSTIL, b: [HOSTIL] }), null)!;
  assert.equal(json.tipo, "json");
  const hs = html(json);
  semInjecao(hs, "json");
  assert.match(hs, /&lt;script>x&lt;\/script>/);
});

test("invisível e bidi viram marca visível no conteúdo", () => {
  const d = detectar("texto", JSON.stringify({ a: "abc\u202Edef" }), null)!;
  assert.match(html(d), /<span class="inv" data-u="202E">/);
});

test("atributos com aspas e tags no valor ficam fechados (url, cor, imagem)", () => {
  // Valores que a detecção nunca produz, para provar o escape mesmo se ela falhar.
  const forjados: Inteligente[] = [
    { tipo: "url", href: `https://x.test/"><img src=x onerror=alert(1)>` },
    { tipo: "cor", css: `red" onmouseover="alert(1)` },
    { tipo: "imagem", src: "data:image/png;base64,AAAA", formato: `png"><script>x</script>`, bytes: 3 },
  ];
  for (const d of forjados) semInjecao(html(d), d.tipo);
});

test("URL de verdade: href normalizado no atributo, host no selo, endereço no title", () => {
  assert.equal(detectar("texto", `https://exemplo.test/a?q="><img src=x>`, null), null); // aspa crua: não é link
  assert.equal(selo(no(`https://exemplo.test/a?q="><img src=x>`)), "");
  const u = `https://exemplo.test/a?q=<script>x</script>&b='1'`;
  const d = detectar("texto", u, null)!;
  assert.equal(d.tipo, "url");
  semInjecao(html(d), "url");
  const s = selo(no(u));
  semInjecao(s, "selo url");
  assert.match(s, />exemplo\.test<\/button>/);
  assert.match(s, /title="https:\/\/exemplo\.test\//);
  assert.match(s, /tabindex="-1" data-link /);
});

test("selo de prévia cortada: URL com host; JSON e log não ganham selo", () => {
  const longa = `https://exemplo.test/${"a".repeat(300)}`;
  assert.match(selo(no(longa)), />exemplo\.test<\/button>/);
  assert.equal(selo(no(`https://exemplo.test/ e depois um texto ${"x".repeat(300)}`)), "");
  assert.equal(selo(no(`[2026-11-23 10:00] log de erro ${"x".repeat(300)}`)), "");
  assert.equal(selo(no(`{"a":"${"x".repeat(300)}"}`)), "");
});

test("data do arquivo no selo e no painel, e cor com amostra", () => {
  assert.match(selo(no("#2f81f7")), /class="sw" role="img" aria-label="[^"]*#2f81f7" style="background:#2f81f7"/);
  assert.match(html(detectar("texto", "2024-02-29T13:45:00Z", null)!), /2024-02-29T13:45:00\.000Z/);
});

test("prévia cortada no formato do núcleo (200 bytes + …): JWT e imagem ganham selo", () => {
  const jwt = `${b64url('{"alg":"HS256","typ":"JWT"}')}.${b64url(JSON.stringify({ sub: "u-1", papel: "leitura", x: "a".repeat(200) }))}.assinatura`;
  const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
  const longo = png.replace(/=+$/, "") + "A".repeat(400);
  for (const [nome, v, pista, rotulo] of [
    ["jwt", jwt, "jwt", /JWT/],
    ["data uri", `data:image/png;base64,${longo}`, "imagem", /#image/],
    ["base64 cru", longo, "imagem", /#image/],
  ] as const) {
    const l = no(v);
    assert.ok(l.previa.endsWith("…") && Buffer.byteLength(l.previa) === 200 + 3, nome);
    assert.equal(daPrevia(l), pista, nome);
    assert.match(selo(l), rotulo, nome);
  }
  // Prévia com acento cortado no meio: a fronteira recua e o `…` não engana a detecção.
  assert.equal(daPrevia(no(`${"é".repeat(150)}`)), null);
});

test("URL de prévia cortada: host só se terminou dentro da prévia", () => {
  const s1 = selo(no(`https://exemplo.test/${"a".repeat(300)}`));
  assert.match(s1, />exemplo\.test<\/button>/);
  assert.match(s1, /title="https:\/\/exemplo\.test\/a+…"/);
  // Host que não coube na prévia: o selo não mostra um host que o clique talvez não abra.
  const texto = (h: string) => /<\/svg>([^<]*)<\/button>/.exec(h)?.[1];
  for (const v of [`https://${"a".repeat(300)}`, `https://${"b".repeat(60)}.${"c".repeat(60)}.${"d".repeat(60)}.${"e".repeat(60)}.test/x`]) {
    const l = no(v);
    assert.ok(l.previa.endsWith("…"), v);
    const s2 = selo(l);
    assert.match(s2, /data-link/);
    assert.equal(texto(s2), t.inteligente.abrirCurto, v);
  }
});

test("reindentado acima de MAX_FORMATADO fica no texto cru", () => {
  // ~1 MB com 99 níveis: reindentado passaria de 100 MB.
  const s = "[".repeat(99) + Array(500000).fill("1").join(",") + "]".repeat(99);
  const t0 = performance.now();
  const d = detectar("texto", s, null);
  assert.ok(performance.now() - t0 < 2000);
  assert.equal(d?.tipo, "json");
  assert.equal((d as { texto: string }).texto, s);
  assert.ok(s.length < MAX_FORMATADO);
});
