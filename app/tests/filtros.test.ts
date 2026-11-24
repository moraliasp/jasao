// Filtros jq salvos (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { alternarFiltro, lerFiltros, MAX_FILTROS, MAX_TAMANHO } from "../src/filtros.ts";

test("lê só textos válidos, sem repetir", () => {
  assert.deepEqual(lerFiltros(null), []);
  assert.deepEqual(lerFiltros("{"), []);
  assert.deepEqual(lerFiltros('{"a":1}'), []);
  assert.deepEqual(lerFiltros('[".a", 1, null, "", "  ", {"x":1}, ".a", ".b"]'), [".a", ".b"]);
});

test("teto de tamanho e de quantidade ao ler", () => {
  const grande = "x".repeat(MAX_TAMANHO + 1);
  assert.deepEqual(lerFiltros(JSON.stringify([grande, "x".repeat(MAX_TAMANHO)])), ["x".repeat(MAX_TAMANHO)]);
  // Bytes UTF-8, como no Rust: 2 bytes por "é".
  assert.deepEqual(lerFiltros(JSON.stringify(["é".repeat(MAX_TAMANHO / 2 + 1)])), []);
  const muitos = Array.from({ length: MAX_FILTROS + 10 }, (_, i) => `.k${i}`);
  assert.equal(lerFiltros(JSON.stringify(muitos)).length, MAX_FILTROS);
});

test("alternar salva no topo e tira se já estava", () => {
  assert.deepEqual(alternarFiltro([".a"], ".b"), [".b", ".a"]);
  assert.deepEqual(alternarFiltro([".b", ".a"], ".b"), [".a"]);
  assert.deepEqual(alternarFiltro([".a"], "   "), [".a"]);
  assert.deepEqual(alternarFiltro([], "x".repeat(MAX_TAMANHO + 1)), []);
  const cheia = Array.from({ length: MAX_FILTROS }, (_, i) => `.k${i}`);
  const nova = alternarFiltro(cheia, ".novo");
  assert.equal(nova.length, MAX_FILTROS);
  assert.equal(nova[0], ".novo");
});
