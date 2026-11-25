// Modo Diff, desenho puro (roda com `node --test`): a classe vem do núcleo e não é recalculada aqui.
import assert from "node:assert/strict";
import { test } from "node:test";
import { agrupar, htmlEstrutura, htmlValores, lerRegistros, mesmoArquivo, travarPar, type DiffArquivos, type DiffEstrutura } from "../src/diff.ts";
import { t } from "../src/i18n.ts";

const x = t.diff;
const fmt = (n: number) => String(n);

const est: DiffEstrutura = {
  mudancas: [
    { caminho: ".a", regra: "campo_removido", classe: "quebra", antes: [3, 3], depois: [0, 2] },
    { caminho: ".b", regra: "campo_novo_opcional", classe: "compativel", antes: [0, 3], depois: [1, 2] },
    // Classe vem pronta: uma regra "de compatível" marcada quebra fica em quebra.
    { caminho: ".c", regra: "tipo_alargado", classe: "quebra", antes: [3, 3], depois: [2, 2], tipos: [["número"], ["número", "texto"]] },
    { caminho: ".<x>", regra: "variante_nova", classe: "compativel", antes: [0, 3], depois: [1, 2], variante: "<b>" },
  ],
  iguais: [".", ".d"],
  invalidas: [0, 0],
};

test("agrupa pela classe do núcleo, na ordem recebida", () => {
  const g = agrupar(est);
  assert.deepEqual(g.quebra.map((m) => m.caminho), [".a", ".c"]);
  assert.deepEqual(g.compativel.map((m) => m.caminho), [".b", ".<x>"]);
  assert.deepEqual(g.igual, [".", ".d"]);
});

test("estrutura: resumo conta grupos, itens mostram regra, evidência e escapam HTML", () => {
  const h = htmlEstrutura(est, x, fmt);
  assert.equal(h.resumo, x.resumoEstrutura("2", "2", "2"));
  const ordem = ["dg-quebra", "dg-compativel", "dg-igual"].map((id) => h.html.indexOf(id));
  assert.ok(ordem.every((i, k) => i >= 0 && (k === 0 || i > ordem[k - 1])), "grupos na ordem quebra, compatível, igual");
  assert.ok(h.html.includes(`<code>.a</code> <b>${x.regras.campo_removido}</b> <span class="dev">${x.evidencia("3", "3")} → ${x.evidencia("0", "2")}</span>`));
  assert.match(h.html, /número → número \| texto/);
  assert.ok(!h.html.includes("<b><b>") && !h.html.includes(".<x>"), "caminho e variante escapados");
  assert.ok(h.html.includes("&lt;b>"));
});

test("estrutura: sem mudança diz que é igual; grupo vazio some; inválidas no resumo", () => {
  const h = htmlEstrutura({ mudancas: [], iguais: ["."], invalidas: [1, 0] }, x, fmt);
  assert.equal(h.resumo, `${x.semMudanca} ${x.invalidas("1", "0")}`);
  assert.ok(!h.html.includes("dg-quebra") && h.html.includes("dg-igual"));
});

const vazio: DiffArquivos = {
  registros_antes: 2, registros_depois: 2, iguais: 2, diferentes: 0, so_antes: 0, so_depois: 0,
  invalidos: 0, grandes: 0, diferencas: 0, guardados: [], cancelado: false,
};

test("valores: iguais", () => {
  const h = htmlValores(vazio, x, fmt);
  assert.equal(h.resumo, x.iguais);
  assert.equal(h.html, "");
});

test("valores: registro base 0 vira 1, linha ausente é —, ação por caminho, cortados e excedentes", () => {
  const r: DiffArquivos = {
    ...vazio, iguais: 1, diferentes: 1, so_depois: 2, diferencas: 4,
    guardados: [
      { registro: 0, linha_antes: 4, linha_depois: 4, total: 3, diferencas: [
        { caminho: ".n", acao: "alterado", antes: "1", depois: "1.0" },
        { caminho: ".<i>", acao: "removido", antes: "\"<i>\"", depois: null },
      ] },
      { registro: 1, linha_antes: null, linha_depois: 7, total: 1, diferencas: [{ caminho: ".", acao: "adicionado", antes: null, depois: "{}" }] },
    ],
  };
  const h = htmlValores(r, x, fmt);
  assert.equal(h.resumo, x.resumoValores("1", "1", "0", "2", "0"));
  assert.ok(h.html.includes(x.registro("1", "5", "5")));
  assert.ok(h.html.includes(x.registro("2", "—", "8")));
  assert.ok(h.html.includes(`<code>.n</code> <b>${x.acoes.alterado}</b> <code>1</code> → <code>1.0</code>`));
  assert.ok(h.html.includes(`<li class="removido"><code>.&lt;i></code> <b>${x.acoes.removido}</b> <code>"&lt;i>"</code></li>`), "caminho escapado; removido sem seta");
  assert.ok(h.html.includes(`<b>${x.acoes.adicionado}</b> <code>{}</code>`));
  assert.ok(h.html.includes(`<li>${x.maisDiferencas("1")}</li>`));
  assert.ok(h.html.includes(x.maisRegistros("1")), "3 diferentes, 2 guardados");
});

test("lerRegistros: vazio é arquivo todo, um campo vale para os dois, base 0, lixo é ruim", () => {
  assert.equal(lerRegistros(" ", ""), null);
  assert.deepEqual(lerRegistros("1", "3"), [0, 2]);
  assert.deepEqual(lerRegistros("", "2"), [1, 1]);
  assert.deepEqual(lerRegistros(" 4 ", ""), [3, 3]);
  for (const [a, b] of [["0", ""], ["-1", ""], ["1.5", ""], ["1", "x"], ["01", ""], ["1e3", ""]]) assert.equal(lerRegistros(a, b), "ruim", `${a}|${b}`);
});

test("valores: só inválidos ou só grandes não dizem que é igual", () => {
  const inv = htmlValores({ ...vazio, iguais: 1, invalidos: 1 }, x, fmt);
  assert.equal(inv.resumo, x.resumoValores("1", "0", "0", "0", "1"));
  const gr = htmlValores({ ...vazio, iguais: 1, grandes: 2 }, x, fmt);
  assert.equal(gr.resumo, `${x.resumoValores("1", "0", "0", "0", "0")} ${x.grandes("2")}`);
});

test("estrutura: caminho igual hostil sai escapado", () => {
  const h = htmlEstrutura({ mudancas: [], iguais: [".<x>"], invalidas: [0, 0] }, x, fmt);
  assert.ok(h.html.includes("<code>.&lt;x></code>") && !h.html.includes(".<x>"));
});

test("Antes e Depois: mesmo doc ou mesmo caminho em duas abas é o mesmo arquivo", () => {
  const abas = [
    { doc: 1, caminho: "/a.jsonl" },
    { doc: 2, caminho: "/a.jsonl" },
    { doc: 3, caminho: "/b.jsonl" },
    { doc: 4, caminho: null },
  ];
  assert.ok(mesmoArquivo(abas, "1", "1"));
  assert.ok(mesmoArquivo(abas, "1", "2"));
  assert.ok(!mesmoArquivo(abas, "1", "3"));
  assert.ok(!mesmoArquivo(abas, "4", "3"));
});

test("travarPar desabilita no outro select o arquivo já escolhido, e refaz a cada troca", () => {
  // Docs 1 e 3 são o mesmo caminho em duas abas; 2 é outro arquivo.
  const abas = [{ doc: 1, caminho: "/a.jsonl" }, { doc: 2, caminho: "/b.jsonl" }, { doc: 3, caminho: "/a.jsonl" }];
  const lista = (value: string) => ({ value, options: abas.map((a) => ({ value: String(a.doc), disabled: false })) });
  const off = (l: ReturnType<typeof lista>) => l.options.filter((o) => o.disabled).map((o) => o.value);
  const [antes, depois] = [lista("1"), lista("2")];
  travarPar(abas, antes, depois);
  assert.deepEqual(off(antes), ["2"]);
  assert.deepEqual(off(depois), ["1", "3"], "o mesmo caminho em outra aba também trava");
  // O handler de `change` chama travarPar de novo: a trava segue a nova escolha.
  depois.value = "3";
  travarPar(abas, antes, depois);
  assert.deepEqual(off(antes), ["1", "3"]);
  assert.deepEqual(off(depois), ["1", "3"]);
});
