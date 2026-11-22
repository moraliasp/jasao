// Lógica pura das abas de arquivo (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { aceito, aposFechar, ciclo, nomes, porAtalho, posicaoNova, triar } from "../src/abas.ts";

test("aceita JSON, JSON Lines e compactados; recusa o resto", () => {
  for (const c of ["a.json", "b.JSONL", "/x/c.ndjson", "d.jsonl.gz", "e.json.zst", "f.NDJSON.GZ"]) assert.ok(aceito(c), c);
  for (const c of ["foto.png", "a.json.bak", "a.gz", "a.zst", "jsonl", "a.json.gz.txt", "pasta"]) assert.ok(!aceito(c), c);
});

test("aba nova entra logo depois da ativa", () => {
  assert.equal(posicaoNova(0, -1), 0);
  assert.equal(posicaoNova(3, 0), 1);
  assert.equal(posicaoNova(3, 2), 3);
});

test("fechar a ativa passa à vizinha da direita, senão à da esquerda", () => {
  assert.equal(aposFechar(3, 0, 0), 0); // a que era a 2ª vira a 1ª
  assert.equal(aposFechar(3, 1, 1), 1);
  assert.equal(aposFechar(3, 2, 2), 1);
  assert.equal(aposFechar(1, 0, 0), -1);
});

test("fechar outra aba mantém a ativa", () => {
  assert.equal(aposFechar(3, 0, 2), 1);
  assert.equal(aposFechar(3, 2, 0), 0);
});

test("Ctrl+Tab dá a volta nas pontas", () => {
  assert.equal(ciclo(3, 2, 1), 0);
  assert.equal(ciclo(3, 0, -1), 2);
  assert.equal(ciclo(3, 1, 1), 2);
  assert.equal(ciclo(0, -1, 1), -1);
});

test("Cmd+1…8 vão à posição; Cmd+9 à última", () => {
  assert.equal(porAtalho(3, 1), 0);
  assert.equal(porAtalho(3, 3), 2);
  assert.equal(porAtalho(3, 4), null);
  assert.equal(porAtalho(3, 9), 2);
  assert.equal(porAtalho(12, 9), 11);
  assert.equal(porAtalho(0, 1), null);
});

test("nomes iguais ganham a pasta de cima até se distinguirem", () => {
  assert.deepEqual(nomes(["/a/x/dados.jsonl", "/b/x/dados.jsonl", "/c/outro.json", null], "Nova aba"), [
    "dados.jsonl — a/x",
    "dados.jsonl — b/x",
    "outro.json",
    "Nova aba",
  ]);
  assert.deepEqual(nomes(["/a/dados.json", "/b/dados.json"], ""), ["dados.json — a", "dados.json — b"]);
  assert.deepEqual(nomes(["C:\\a\\d.json", "C:\\b\\d.json"], ""), ["d.json — a", "d.json — b"]);
  assert.deepEqual(nomes(["/d.json", "/a/d.json"], ""), ["d.json — /", "d.json — a"]);
});

test("triar separa os que abrem, os de formato recusado e as pastas", () => {
  assert.deepEqual(triar(["/a.jsonl", "/foto.png", "/dados", "/b.json.gz"], [false, false, true, false]), {
    abrir: ["/a.jsonl", "/b.json.gz"],
    recusados: ["/foto.png"],
    pastas: ["/dados"],
  });
});
