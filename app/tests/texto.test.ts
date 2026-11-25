// Lógica pura da visão Texto (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { aposSpan, escAtributo, indiceDe, semInvisiveis, linhaHtml, type Marca, marcado, partir, resumir, visivel } from "../src/texto.ts";

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

/** Lista que conta as leituras de posição: mede o trabalho de `partir` sem depender do relógio. */
function contada(marcas: Marca[]): [Marca[], () => number] {
  let n = 0;
  const p = new Proxy(marcas, {
    get(alvo, k, r) {
      if (typeof k === "string" && /^\d+$/.test(k)) n++;
      return Reflect.get(alvo, k, r);
    },
  });
  return [p, () => n];
}

test("partir é linear: linha longa com muitos achados não lê marcas × caracteres", () => {
  // Um texto de 4002 bytes com achado a cada 2 bytes (2000 marcas).
  const tok = `"${"a".repeat(4000)}"`;
  const lista: Marca[] = Array.from({ length: 2000 }, (_, i) => [1 + 2 * i, 2 + 2 * i, i === 7]);
  const [marcas, leituras] = contada(lista);
  const p = partir(tok, 0, marcas);
  assert.equal(p.length, 4001); // aspas+achado+fora alternados, a aspa final junta com o último "a"
  assert.deepEqual(p.slice(0, 3), [['"', 0, 0], ["a", 1, 1], ["a", 2, 0]]);
  assert.deepEqual(p[15], ["a", 15, 2]);
  assert.ok(leituras() < 10 * tok.length, `${leituras()} leituras`);
  // Muitos tokens na linha, cada um com seu achado: linhaHtml também não pode ser tokens × marcas.
  const linha = `[${Array(3000).fill("1").join(",")}]`;
  const [m2, l2] = contada(Array.from({ length: 3000 }, (_, i): Marca => [1 + 2 * i, 2 + 2 * i, false]));
  const html = linhaHtml(linha, linha.length, null, () => "", m2);
  assert.equal(html.match(/class="n ac"/g)?.length, 3000);
  assert.ok(l2() < 20 * linha.length, `${l2()} leituras`);
});

test("visivel mostra controles bidi e de largura zero sem tirá-los do texto", () => {
  const s = "a\u202eb\u200bc\u2066d\u061ce\ufefff<";
  const html = visivel(s);
  for (const u of ["202E", "200B", "2066", "061C", "FEFF"]) assert.match(html, new RegExp(`<span class="inv" data-u="${u}">`));
  // O texto do DOM (sem as tags) é o original: a cópia por seleção entrega o mesmo texto.
  assert.equal(html.replace(/<[^>]+>/g, "").replace(/&lt;/g, "<").replace(/&amp;/g, "&"), s);
  assert.equal(visivel("comum é ok"), "comum é ok");
});

test("visivel cobre a lista inteira dos invisíveis e só ela", () => {
  // Os mesmos 17 de `perigoso` na CLI (cli/src/main.rs).
  const lista = [0x61c, 0x200b, 0x200c, 0x200d, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2060, 0x2066, 0x2067, 0x2068, 0x2069, 0xfeff];
  for (const cp of lista) {
    const u = cp.toString(16).toUpperCase().padStart(4, "0");
    assert.equal(visivel(`a${String.fromCodePoint(cp)}b`), `a<span class="inv" data-u="${u}">${String.fromCodePoint(cp)}</span>b`, u);
  }
  // Vizinhos que não entram: NBSP, U+200A, U+2010, U+202F, U+2065, U+206A, U+FEFE.
  for (const cp of [0xa0, 0x200a, 0x2010, 0x202f, 0x2065, 0x206a, 0xfefe]) assert.equal(visivel(String.fromCodePoint(cp)), String.fromCodePoint(cp));
});

test("linhaHtml e marcado marcam o invisível e mantêm o byte de cada pedaço", () => {
  const html = linhaHtml('{"a\u202e":1}', 10, null, () => "");
  assert.match(html, /<span data-b="1" class="k">"a<span class="inv" data-u="202E">\u202e<\/span>"<\/span>/);
  assert.match(html, /<span data-b="7" class="p">:<\/span>/); // U+202E ocupa 3 bytes
  assert.match(marcado("x\u200dy", 0, []), /data-u="200D"/);
});

test("o CSS isola chaves, valores e o marcador dos invisíveis", () => {
  const css = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  assert.match(css, /\.inv\s*\{[^}]*unicode-bidi:\s*isolate/);
  assert.match(css, /\.inv::before\s*\{[^}]*attr\(data-u\)/);
  assert.match(css, /\.row \.key,[^{]*\{[^}]*unicode-bidi:\s*isolate/);
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

test("indiceDe converte linha e coluna (em caracteres) do erro do filtro", () => {
  assert.equal(indiceDe(".a | (", 1, 6), 5);
  assert.equal(indiceDe(".a |\n  ]", 2, 3), 7);
  assert.equal(indiceDe("\"😀\" | (", 1, 7), 7); // emoji conta 1 coluna e 2 unidades UTF-16
  assert.equal(indiceDe("x", 3, 1), 1);
});

test("atributo não leva invisível cru: troca pelo código e escapa aspas", () => {
  const a = escAtributo('a\u202eb"<&\u2066');
  assert.equal(a, "a&lt;U+202E>b&quot;&lt;&amp;&lt;U+2066>");
  assert.doesNotMatch(a, /[\u202a-\u202e\u2066-\u2069]/);
});

test("semInvisiveis troca cada invisível pelo código e não escapa nada (vai por textContent/title)", () => {
  assert.equal(semInvisiveis("fatura\u202eldt.jsonl"), "fatura<U+202E>ldt.jsonl");
  assert.equal(semInvisiveis("a\u200bb & <c>"), "a<U+200B>b & <c>");
  assert.doesNotMatch(semInvisiveis("x\u2066\u2069\ufeff"), /[\u2066\u2069\ufeff]/);
});

test("aposSpan: `$` do nome do arquivo não vira padrão do replace", () => {
  const nome = "<b>a$&b$1$$.json</b> · ";
  assert.equal(aposSpan("ok <span>3 linhas</span>", nome), `ok <span>${nome}3 linhas</span>`);
  assert.equal(aposSpan("sem marcador", nome), "sem marcador");
});
