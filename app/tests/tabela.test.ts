// Lógica pura do modo Tabela (roda com `node --test`, sem navegador).
// O idioma de `t` vem do ambiente; as expectativas usam o próprio `t` para não depender dele.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { t } from "../src/i18n.ts";
import { type Celula, type LinhaT, celula, detalhe, invalida, largura, motivo, ordemVisivel, proximaOrdem } from "../src/tabela.ts";

const fmt = (n: number) => String(n);
const cel = (tipo: Celula["tipo"], texto: string, filhos = 0): Celula => ({ tipo, texto, filhos, byte: 0 });

test("ordem visível: fixadas primeiro, na ordem em que foram fixadas; ocultas fora", () => {
  assert.deepEqual(ordemVisivel(4, [], new Set()), [0, 1, 2, 3]);
  assert.deepEqual(ordemVisivel(4, [2, 0], new Set()), [2, 0, 1, 3]);
  assert.deepEqual(ordemVisivel(4, [2, 0], new Set([0, 3])), [2, 1]);
  assert.deepEqual(ordemVisivel(2, [5], new Set()), [0, 1]); // índice de outro documento: ignorado
});

test("clique no cabeçalho: crescente, decrescente, ordem do arquivo", () => {
  const a = proximaOrdem(null, 1);
  assert.deepEqual(a, { coluna: 1, desc: false });
  const b = proximaOrdem(a, 1);
  assert.deepEqual(b, { coluna: 1, desc: true });
  assert.equal(proximaOrdem(b, 1), null);
  assert.deepEqual(proximaOrdem(b, 0), { coluna: 0, desc: false }); // outra coluna recomeça
});

test("ausente, null do JSON, objeto e lista têm cara diferente", () => {
  const aus = celula(null, t, fmt);
  const nulo = celula(cel("nulo", "null"), t, fmt);
  assert.match(aus.html, /class="aus"/);
  assert.equal(aus.rotulo, t.tabela.ausente);
  assert.match(nulo.html, /class="v nulo">null</);
  assert.notEqual(aus.html, nulo.html);
  const o = celula(cel("objeto", "", 3), t, fmt);
  assert.match(o.html, /data-abrir/);
  assert.match(o.html, />\{3\}</);
  assert.equal(o.rotulo, t.tabela.objeto("3"));
  assert.match(celula(cel("lista", "", 0), t, fmt).html, />\[0\]</);
});

test("número grande e texto aparecem como estão no arquivo", () => {
  assert.match(celula(cel("numero", "12345678901234567890.000"), t, fmt).html, />12345678901234567890\.000</);
  assert.match(celula(cel("texto", '"a\\u00e9"'), t, fmt).html, />"a\\u00e9"</);
});

test("texto hostil não vira HTML nem engana a direção", () => {
  const h = celula(cel("texto", '"<img src=x onerror=alert(1)>&"'), t, fmt).html;
  assert.ok(!h.includes("<img"), h);
  assert.match(h, /&lt;img/);
  assert.match(h, /&amp;"/);
  const b = celula(cel("texto", '"abc\u202Edef"'), t, fmt).html;
  // Mesmo neutralizador do Texto e da Árvore: o RLO fica isolado, com o código à vista.
  assert.match(b, /<span class="inv" data-u="202E">\u202e<\/span>/);
});

test("linha inválida: motivo com byte e texto cru neutralizado", () => {
  const l: LinhaT = { registro: 4, linha: 4, byte: 99, celulas: [], erro: { byte: 7, motivo: "outro" }, previa: '{"a":<b>' };
  const r = invalida(l, t, fmt);
  assert.match(r.html, /class="err"/);
  assert.ok(r.html.includes(t.byte("7")));
  assert.ok(!r.html.includes("<b>"));
  assert.ok(r.rotulo.startsWith(t.tabela.invalida));
});

test("motivo de indisponível e detalhe do cabeçalho", () => {
  assert.equal(motivo({ motivo: "nao_lista" }, t, fmt), t.tabela.indisponivel.nao_lista);
  assert.equal(motivo({ motivo: "nao_objetos", n: 2 }, t, fmt), t.tabela.indisponivel.nao_objetos("2"));
  const c = { nome: "id", caminho: ".id", presenca: 3, tipos: ["numero"] };
  assert.ok(detalhe(c, 3, t, fmt).endsWith(t.estrutura.todos));
  assert.ok(detalhe({ ...c, presenca: 1 }, 3, t, fmt).endsWith(t.estrutura.deN("1", "3")));
  assert.equal(largura(c), 110);
  assert.equal(largura({ ...c, nome: "x".repeat(200) }), 280);
});

test("o CSS isola o texto das células e do cabeçalho", () => {
  const css = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  assert.match(css, /\.grade \.td \.v, \.grade \.td \.cru \{[^}]*unicode-bidi:\s*isolate/);
  assert.match(css, /\.grade \.th \.nm \{[^}]*unicode-bidi:\s*isolate/);
});
