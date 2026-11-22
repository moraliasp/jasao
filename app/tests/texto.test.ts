// Lógica pura da visão Texto (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { linhaHtml, marcado, partir, resumir } from "../src/texto.ts";

test("partir sem marca devolve o token inteiro", () => {
  assert.deepEqual(partir('"abc"', 5, [[0, 2, false]]), [['"abc"', 5, 0]]);
});

test("partir corta nos limites em bytes, inclusive com acento", () => {
  // Bytes: 0 = aspas, 1-2 = é, 3-4 = ç, 5-6 = ã.
  assert.deepEqual(partir('"éçã"', 0, [[3, 5, true]]), [
    ['"é', 0, 0],
    ["ç", 3, 2],
    ['ã"', 5, 0],
  ]);
});

test("achados vizinhos ficam em pedaços separados", () => {
  assert.deepEqual(partir("aaaa", 10, [[10, 12, false], [12, 14, true]]), [
    ["aa", 10, 1],
    ["aa", 12, 2],
  ]);
});

test("linhaHtml marca o achado e mantém o byte de cada pedaço", () => {
  const html = linhaHtml('{"nome":"Ana"}', 14, null, () => "", [[10, 12, true]]);
  assert.match(html, /<span data-b="8" class="s">"A<\/span><span data-b="10" class="s ac at">na<\/span><span data-b="12" class="s">"<\/span>/);
});

test("marcado escapa e envolve só o trecho", () => {
  assert.equal(marcado('"<a>"', 3, [[5, 6, false]]), '"&lt;<mark class="ac">a</mark>>"');
});

test("resumir junta espaços e corta em n caracteres sem partir emoji", () => {
  assert.equal(resumir("a\n  b"), "a b");
  assert.equal(resumir("abcdef", 4), "abc…");
  assert.equal(resumir("😀😀😀😀😀", 3), "😀😀…");
  assert.equal(resumir("x".repeat(60)), "x".repeat(60));
});
