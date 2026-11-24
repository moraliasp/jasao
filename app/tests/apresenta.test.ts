// HTML dos valores inteligentes (roda com `node --test`, sem navegador): todo texto do arquivo
// sai escapado. Tirar `visivel` ou `escAtributo` de `apresenta.ts` derruba estes testes.
import assert from "node:assert/strict";
import { test } from "node:test";
import { html, type No, selo } from "../src/apresenta.ts";
import { detectar, type Inteligente } from "../src/valores.ts";

const b64url = (s: string) => Buffer.from(s).toString("base64url");
const HOSTIL = `<img src=x onerror=alert(1)>"'><script>x</script>`;
const no = (valor: string, chave: string | null = null): No => {
  const previa = JSON.stringify(valor);
  return { tipo: "texto", previa: previa.slice(0, 200), chave, faixa: [0, previa.length], erro: null };
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
