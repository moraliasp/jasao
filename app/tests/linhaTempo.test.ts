// Linha do tempo, desenho e escolha dos dois pontos (roda com `node --test`).
import assert from "node:assert/strict";
import { test } from "node:test";
import { htmlLinha, marcar, parMarcado, type Linha, type Passo } from "../src/linhaTempo.ts";
import { t } from "../src/i18n.ts";

const x = t.diff;
const fmt = (n: number) => String(n);
const passo = (p: Partial<Passo>): Passo => ({
  caminho: "/p/a.jsonl",
  nome: "a.jsonl",
  data: "2024-01-01 00:00:00",
  erro: null,
  registros: 3,
  invalidas: 0,
  contra: null,
  quebras: 0,
  compativeis: 0,
  mudancas: [],
  mais: 0,
  ...p,
});

const linha: Linha = {
  origem: "nome",
  ignorados: 2,
  parada: null,
  passos: [
    passo({ nome: "<img src=x onerror=alert(1)>.jsonl" }),
    passo({ nome: "b.jsonl", erro: "<b>sem permissão</b>" }),
    passo({
      nome: "c.jsonl",
      contra: 0,
      quebras: 1,
      compativeis: 1,
      mudancas: [
        { caminho: ".<x>", regra: "campo_removido", classe: "quebra", antes: [3, 3], depois: [0, 2] },
        { caminho: ".n", regra: "campo_novo_opcional", classe: "compativel", antes: [0, 3], depois: [1, 2] },
      ],
      mais: 7,
    }),
    passo({ nome: "d.jsonl", contra: 2 }),
  ],
};

test("escapa nome, erro e caminho vindos do disco", () => {
  const { html } = htmlLinha(linha, x, fmt, []);
  assert.ok(!html.includes("<img"), html);
  assert.ok(!html.includes("<b>sem"), html);
  assert.ok(!html.includes("<x>"), html);
  assert.ok(html.includes("&lt;img src=x onerror=alert(1)>.jsonl"));
});

test("conta versões, ignorados, quebras e o resto além do teto", () => {
  const { resumo, html } = htmlLinha(linha, x, fmt, []);
  assert.equal(resumo, `${x.linha.resumo("4", x.linha.origem.nome)} · ${x.linha.ignorados("2")}`);
  assert.ok(html.includes(x.linha.primeira("3")));
  assert.ok(html.includes(x.linha.mudou("1", "1")));
  // Erro no meio: c compara com a (não com b), e diz contra quem.
  assert.ok(html.includes(x.linha.contra("&lt;img src=x onerror=alert(1)>.jsonl")));
  assert.ok(html.includes(x.maisDiferencas("7")));
  assert.ok(html.includes(x.linha.igual));
  // Quem tem quebra vem aberto e marcado; quem não abriu não se marca.
  assert.match(html, /class="lpasso quebra"><label>.*?<details open>/);
  assert.equal((html.match(/type="checkbox"/g) ?? []).length, 3);
  assert.ok(!html.includes('data-i="1"'));
});

test("parou no prazo: avisa no resumo", () => {
  const { resumo } = htmlLinha({ ...linha, ignorados: 0, parada: "prazo" }, x, fmt, []);
  assert.ok(resumo.endsWith(x.linha.prazo));
});

test("marca no máximo dois, em ordem do tempo", () => {
  let m = marcar([], 3);
  m = marcar(m, 0);
  assert.deepEqual(parMarcado(m), [0, 3]);
  m = marcar(m, 2); // terceiro: sai o primeiro marcado (3)
  assert.deepEqual(m, [0, 2]);
  assert.deepEqual(parMarcado(marcar(m, 2)), null);
  const { html } = htmlLinha(linha, x, fmt, m);
  assert.ok(html.includes('data-i="0" checked') && html.includes('data-i="2" checked'));
  assert.ok(!html.includes('data-i="3" checked'));
});
