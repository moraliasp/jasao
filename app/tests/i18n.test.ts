// Tradução dos códigos de erro do Rust (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
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

test("motivos de schema: frase exata por código e idioma; desconhecido cai no texto", async () => {
  const { en, pt } = await import("../src/i18n.ts");
  // [código, PT, EN]; o caminho, quando há, vem na frente com ": ".
  const casos: [string, string, string][] = [
    ["tipo|/a|integer", "/a: deve ser do tipo inteiro", "/a: must be of type integer"],
    ["tipo||integer,null", "deve ser do tipo inteiro ou nulo", "must be of type integer or null"],
    ["tipo||string", "deve ser do tipo texto", "must be of type string"],
    ["obrigatoria||nome", 'falta a propriedade obrigatória "nome"', 'missing required property "nome"'],
    ["enum||[\"x\",\"y\"]", 'deve ser um destes valores: ["x","y"]', 'must be one of: ["x","y"]'],
    ["const||\"x\"", 'deve ser exatamente "x"', 'must be exactly "x"'],
    ["minimo||5", "deve ser maior ou igual a 5", "must be at least 5"],
    ["maximo||5", "deve ser menor ou igual a 5", "must be at most 5"],
    ["minimo_exclusivo||5", "deve ser maior que 5", "must be greater than 5"],
    ["maximo_exclusivo||5", "deve ser menor que 5", "must be less than 5"],
    ["tamanho_minimo||9", "deve ter no mínimo 9 caracteres", "must have at least 9 characters"],
    ["tamanho_maximo||2", "deve ter no máximo 2 caracteres", "must have at most 2 characters"],
    ["pattern||^a%7Cb$", "não bate com o padrão ^a|b$", "does not match the pattern ^a|b$"],
    ["formato||email", 'não está no formato "email"', 'is not in the "email" format'],
    ["propriedade_extra||x", "propriedade não permitida: x", "property not allowed: x"],
    // Chave `a%7C` -> Rust escreve `/a%257C` -> volta `/a%7C` (passada única).
    ["tipo|/a%257C|string", "/a%7C: deve ser do tipo texto", "/a%7C: must be of type string"],
    ["tipo|/a%7C%25|string", "/a|%: deve ser do tipo texto", "/a|%: must be of type string"],
  ];
  for (const [c, p, e] of casos) {
    assert.equal(motivoDoRust(`jasao:schema_${c}`, pt), p, c);
    assert.equal(motivoDoRust(`jasao:schema_${c}`, en), e, c);
  }
  assert.equal(motivoDoRust("jasao:schema_novo|/a|x"), "jasao:schema_novo|/a|x");
});

test("motivos de schema: dourado compartilhado com o Rust (core/src/validar.rs)", async () => {
  const { pt } = await import("../src/i18n.ts");
  const casos: { codigo: string; pt: string }[] = JSON.parse(readFileSync(new URL("./dourados/motivos.json", import.meta.url), "utf8"));
  for (const c of casos) assert.equal(motivoDoRust(c.codigo, pt), c.pt, c.codigo);
});
