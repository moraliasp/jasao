// Lógica pura do modo Estrutura (roda com `node --test`, sem navegador).
// O idioma de `t` vem do ambiente; as expectativas usam o próprio `t` para não depender dele.
import assert from "node:assert/strict";
import { test } from "node:test";
import { type Estrutura, itens, presenca, segmento } from "../src/estrutura.ts";
import { t } from "../src/i18n.ts";

const fmt = (n: number) => String(n);

// Mesmo formato que o comando `estrutura` devolve (ver o teste de IPC em lib.rs).
const e: Estrutura = {
  entrada: "jsonl",
  registros: 3,
  invalidas: 1,
  caminhos: [
    { caminho: ".", relacao: "registro", profundidade: 0, presenca: 3, de: 3, tipos: [["objeto", 3, 0]] },
    { caminho: ".tipo", relacao: "campo", profundidade: 1, presenca: 3, de: 3, tipos: [["texto", 3, 0]] },
    { caminho: ".x", relacao: "campo", profundidade: 1, presenca: 2, de: 3, tipos: [["inteiro", 1, 2], ["nulo", 1, 0]] },
    { caminho: ".x.a", relacao: "campo", profundidade: 2, presenca: 1, de: 1, tipos: [["lista", 1, 4]] },
    { caminho: ".x.a[]", relacao: "elemento", profundidade: 3, presenca: 5, de: 1, tipos: [["decimal", 5, 4]] },
    { caminho: '.["chave <b>"]', relacao: "campo", profundidade: 1, presenca: 1, de: 3, tipos: [["booleano", 1, 7]] },
  ],
  discriminadores: [
    { campo: ".tipo", variantes: [{ valor: "x", registros: 2, linha: 0, obrigatorias: ["tipo", "x"], opcionais: [["z", 1]] }] },
  ],
};

test("segmento é o pedaço do caminho depois do pai", () => {
  assert.equal(segmento(".a", "."), "a");
  assert.equal(segmento(".a.b", ".a"), "b");
  assert.equal(segmento(".a[]", ".a"), "[]");
  assert.equal(segmento(".m.*", ".m"), "*");
  assert.equal(segmento('.["x y"]', "."), '["x y"]');
  // Pai que não é prefixo (profundidade contada errado no núcleo): mostra o caminho inteiro.
  assert.equal(segmento(".b", ".a"), ".b");
});

test("presença: todos, N de M, itens e entradas de mapa", () => {
  const [raiz, tipo, x, , el] = e.caminhos;
  assert.equal(presenca(raiz, t, fmt), t.estrutura.registros("3"));
  assert.equal(presenca(tipo, t, fmt), t.estrutura.todos);
  assert.equal(presenca(x, t, fmt), t.estrutura.deN("2", "3"));
  assert.equal(presenca(el, t, fmt), t.estrutura.elementos("5", "1"));
  assert.equal(presenca({ ...el, relacao: "entrada" }, t, fmt), t.estrutura.entradas("5", "1"));
});

test("itens: um por caminho e um por variante, com a 1ª linha para ir à Árvore", () => {
  const it = itens(e, t, fmt);
  assert.equal(it.length, 7);
  assert.deepEqual(it.map((i) => i.linha), [0, 0, 0, 4, 4, 7, 0]);
  // `.x` tem dois tipos: mostra a contagem de cada e marca opcional.
  assert.match(it[2].html, /class="ep op"/);
  assert.ok(it[2].rotulo.includes(t.estrutura.opcional));
  assert.ok(it[2].html.includes(`${t.estrutura.tipos.inteiro} 1 | ${t.estrutura.tipos.nulo} 1`));
  // Recuo pela profundidade e nome relativo ao pai.
  assert.match(it[4].html, /padding-left:68px" title="\.x\.a\[\]">\[\]</);
  // Chave do arquivo é escapada.
  assert.ok(it[5].html.includes("chave &lt;b>"));
  assert.ok(!it[5].html.includes("<b>"));
  // Variante: campo = valor, sempre/às vezes.
  assert.equal(it[6].caminho, ".tipo");
  assert.ok(it[6].html.includes(`.tipo = "x"`));
  assert.ok(it[6].html.includes(`${t.estrutura.opcionais}: z (1)`));
});

test("singular quando há 1 item ou 1 lista", () => {
  assert.notEqual(t.estrutura.elementos("1", "1"), t.estrutura.elementos("2", "2").replace(/2/g, "1"));
});
