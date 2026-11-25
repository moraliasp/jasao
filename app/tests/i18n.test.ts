// Tradução dos códigos de erro do Rust (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { contagemArquivo, erroDoRust, motivoDoRust, t } from "../src/i18n.ts";

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

test("contagemArquivo: linhas enquanto infere, registros quando acaba", () => {
  const fmt = (n: number) => String(n);
  assert.equal(contagemArquivo(null, 1417, fmt), `1417 ${t.linhas[1]}`);
  assert.equal(contagemArquivo(1411, 1417, fmt), `1411 ${t.registros[1]}`);
  assert.equal(contagemArquivo(1, 3, fmt), `1 ${t.registros[0]}`);
  assert.equal(contagemArquivo(0, 3, fmt), `0 ${t.registros[1]}`, "inferência acabada com zero registros não volta para linhas");
});

test("motivos de schema: cada código vira frase PT e EN com os parâmetros; desconhecido cai no texto", async () => {
  const { en, pt } = await import("../src/i18n.ts");
  const casos: [string, string, string[]][] = [
    ["tipo|/a|integer", "/a: ", ["inteiro", "integer"]],
    ["tipo||integer,null", "", ["inteiro ou nulo", "integer or null"]],
    ["obrigatoria||nome", "", ["nome", "nome"]],
    ["enum||[\"x\",\"y\"]", "", ['["x","y"]', '["x","y"]']],
    ["const||\"x\"", "", ['"x"', '"x"']],
    ["minimo||5", "", ["5", "5"]],
    ["maximo||5", "", ["5", "5"]],
    ["minimo_exclusivo||5", "", ["5", "5"]],
    ["maximo_exclusivo||5", "", ["5", "5"]],
    ["tamanho_minimo||9", "", ["9", "9"]],
    ["tamanho_maximo||2", "", ["2", "2"]],
    ["pattern||^a%7Cb$", "", ["^a|b$", "^a|b$"]],
    ["formato||email", "", ["email", "email"]],
    ["propriedade_extra||x", "", ["x", "x"]],
  ];
  for (const [c, pre, [p, e]] of casos) {
    const pf = motivoDoRust(`jasao:schema_${c}`, pt), ef = motivoDoRust(`jasao:schema_${c}`, en);
    assert.ok(pf.startsWith(pre) && pf.includes(p) && !pf.includes("jasao:") && !pf.includes("%7C"), pf);
    assert.ok(ef.startsWith(pre) && ef.includes(e) && !ef.includes("jasao:"), ef);
    assert.notEqual(pf, ef, c);
  }
  assert.equal(motivoDoRust("jasao:schema_tipo|/a%7C%25|string", pt), "/a|%: deve ser do tipo texto");
  assert.equal(motivoDoRust("jasao:schema_novo|/a|x"), "jasao:schema_novo|/a|x");
});
