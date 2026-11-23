// Tradução dos códigos de erro do Rust (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { erroDoRust, t } from "../src/i18n.ts";

test("códigos da descompactação e da concessão viram texto, ao abrir e na consulta", () => {
  assert.equal(erroDoRust("jasao:razao"), t.compactado.recusada);
  assert.equal(erroDoRust("jasao:teto"), t.compactado.teto);
  assert.equal(erroDoRust("jasao:negado"), t.negado);
  assert.equal(erroDoRust("jasao:cancelada"), t.compactado.cancelada);
  for (const c of ["jasao:razao", "jasao:teto", "jasao:negado"]) assert.ok(!erroDoRust(c).includes("jasao:"));
});

test("outro erro passa como veio", () => {
  assert.equal(erroDoRust("a.jsonl: sem permissão"), "a.jsonl: sem permissão");
  assert.equal(erroDoRust(new Error("x")), "Error: x");
});
