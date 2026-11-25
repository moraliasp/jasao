// Tradução dos códigos de erro do Rust (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { erroDoRust, motivoDoRust, t } from "../src/i18n.ts";

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

test("motivo do validador: código jasao: vira texto; o do schema passa como veio", () => {
  // Os códigos que `core/src/validar.rs` devolve além dos de `arvore::Motivo`.
  for (const c of ["valor", "numero_fora_do_alcance", "aninhamento", "recusado_pelo_validador"]) {
    const m = motivoDoRust(`jasao:${c}`);
    assert.ok(m && !m.includes("jasao:") && m !== t.motivos.outro, c);
  }
  assert.equal(motivoDoRust("jasao:desconhecido"), t.motivos.outro);
  assert.equal(motivoDoRust("/a: 1 is not of type \"string\""), "/a: 1 is not of type \"string\"");
});
