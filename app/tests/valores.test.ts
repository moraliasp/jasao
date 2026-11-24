// Detecção dos valores inteligentes (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { detectar, MAX_BASE64, MAX_JSON_TEXTO, MAX_NIVEL, pista, reindentar, relativo } from "../src/valores.ts";

const b64url = (s: string) => Buffer.from(s).toString("base64url");
const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

test("época em ms sempre; em segundos só com nome de campo de data", () => {
  assert.deepEqual(detectar("numero", "1700000000000", "id"), { tipo: "epoca", ms: 1700000000000, unidade: "ms" });
  assert.deepEqual(detectar("numero", "1700000000", "criado_em"), { tipo: "epoca", ms: 1700000000000, unidade: "s" });
  assert.deepEqual(detectar("numero", "1700000000", "createdAt")?.tipo, "epoca");
  assert.equal(detectar("numero", "1700000000", "id"), null);
  assert.equal(detectar("numero", "1700000000", null), null);
  assert.equal(detectar("numero", "9999999999999", "ts"), null); // além de 2100
  assert.equal(detectar("numero", "1700000000.5", "ts"), null);
  assert.equal(detectar("numero", "-1700000000", "ts"), null);
});

test("data ISO 8601 com e sem hora e fuso", () => {
  assert.deepEqual(detectar("texto", "2024-02-29T13:45:00Z", null), { tipo: "data", ms: Date.UTC(2024, 1, 29, 13, 45) });
  assert.equal(detectar("texto", "2024-02-29", null)?.tipo, "data");
  assert.equal(detectar("texto", "2024-02-29 13:45:00.123-03:00", null)?.tipo, "data");
  assert.equal(detectar("texto", "2024-13-01", null), null);
  assert.equal(detectar("texto", "2024-02-29T25:00", null), null);
  assert.equal(detectar("texto", "em 2024-02-29", null), null);
});

test("relativo usa o idioma e o sentido", () => {
  const agora = Date.UTC(2026, 10, 23, 12);
  assert.equal(relativo(agora - 3 * 864e5, agora, "pt-BR"), "há 3 dias");
  assert.equal(relativo(agora + 2 * 36e5, agora, "pt-BR"), "em 2 horas");
  assert.equal(relativo(agora - 864e5, agora, "en"), "yesterday");
  assert.equal(relativo(agora, agora, "en"), "now");
});

test("cor em hex e rgb, com canais válidos", () => {
  for (const c of ["#2f81f7", "#FFF", "#ffff", "#11223344", "rgb(47, 129, 247)", "rgba(0,0,0,0.5)", "rgb(100%, 0%, 50%)"])
    assert.deepEqual(detectar("texto", c, null), { tipo: "cor", css: c }, c);
  for (const c of ["#12345", "#ggg", "rgb(300,0,0)", "rgb(1,2)", "rgb(1,2,3);background:url(x)", "red"])
    assert.equal(detectar("texto", c, null)?.tipo === "cor", false, c);
});

test("URL http(s) normalizada; outros esquemas e espaços não viram link", () => {
  assert.deepEqual(detectar("texto", "https://exemplo.test/a?b=1", null), { tipo: "url", href: "https://exemplo.test/a?b=1" });
  // Domínio internacional vai em punycode: o que se vê é o que se abre (sem homógrafo).
  assert.deepEqual(detectar("texto", "https://exеmplo.test/", null), { tipo: "url", href: "https://xn--exmplo-4of.test/" });
  for (const u of ["javascript:alert(1)", "file:///etc/passwd", "https://a b.test", "http://", "ftp://x.test"])
    assert.notEqual(detectar("texto", u, null)?.tipo, "url", u);
  // Bidi e invisíveis no caminho saem percentualmente codificados no href: não invertem o texto.
  assert.deepEqual(detectar("texto", "https://x.test/\u202Eabc", null), { tipo: "url", href: "https://x.test/%E2%80%AEabc" });
  assert.equal(detectar("texto", "https://x.test/" + "a".repeat(3000), null), null);
});

test("imagem: data URI e base64 cru com assinatura; nada de rede", () => {
  const uri = `data:image/png;base64,${PNG}`;
  assert.deepEqual(detectar("texto", uri, null), { tipo: "imagem", src: uri, formato: "png", bytes: 70 });
  assert.deepEqual(detectar("texto", PNG, null), { tipo: "imagem", src: uri, formato: "png", bytes: 70 });
  assert.equal(detectar("texto", "data:image/png;base64,abc\"><script>", null)?.tipo, undefined);
  assert.equal(detectar("texto", "data:text/html;base64,PHNjcmlwdD4=", null), null);
  assert.equal(detectar("texto", "data:image/png;base64," + "A".repeat(MAX_BASE64), null), null);
});

test("JWT: cabeçalho e carga decodificados, assinatura descartada", () => {
  const jwt = `${b64url('{"alg":"HS256","typ":"JWT"}')}.${b64url('{"sub":"u-1","nome":"Zé","n":12345678901234567890}')}.assinatura-secreta`;
  const r = detectar("texto", jwt, null);
  assert.equal(r?.tipo, "jwt");
  if (r?.tipo !== "jwt") return;
  assert.equal(r.cabecalho, '{\n  "alg": "HS256",\n  "typ": "JWT"\n}');
  assert.match(r.carga, /"nome": "Zé"/);
  assert.match(r.carga, /12345678901234567890/); // número grande intacto
  assert.doesNotMatch(JSON.stringify(r), /assinatura/);
  assert.notEqual(detectar("texto", `${b64url("{}")}.${b64url("nao json")}.x`, null)?.tipo, "jwt");
});

test("JSON dentro de texto, com teto e sem perder número grande", () => {
  assert.deepEqual(detectar("texto", '{"a":1,"b":[1,2],"c":{}}', null), {
    tipo: "json",
    texto: '{\n  "a": 1,\n  "b": [\n    1,\n    2\n  ],\n  "c": {}\n}',
    itens: 3,
  });
  assert.equal(detectar("texto", '[1,"a, b: [c]"]', null)?.tipo, "json");
  assert.match((detectar("texto", '{"n":12345678901234567890,"n":2}', null) as { texto: string }).texto, /12345678901234567890,\n  "n": 2/);
  assert.equal(detectar("texto", "{nao é json}", null), null);
  assert.equal(detectar("texto", "[" + "1,".repeat(MAX_JSON_TEXTO) + "1]", null), null);
  assert.equal(detectar("texto", "[".repeat(100000) + "]".repeat(100000), null), null);
  assert.equal(detectar("texto", "[".repeat(MAX_NIVEL) + "]".repeat(MAX_NIVEL), null)?.tipo, "json");
});

test("texto comum e outros tipos não viram nada", () => {
  for (const s of ["", "olá", "123", "<img src=x onerror=alert(1)>", "eyJ.eyJ.x"]) assert.equal(detectar("texto", s, null), null, s);
  assert.equal(detectar("booleano", "true", "ts"), null);
  assert.equal(detectar("objeto", "{}", null), null);
});

test("pista pela prévia cortada", () => {
  assert.equal(pista("data:image/png;base64,iVBO"), "imagem");
  assert.equal(pista(PNG.slice(0, 20)), "imagem");
  assert.equal(pista("eyJhbGciOi.eyJzdWIi"), "jwt");
  assert.equal(pista(' {"a":'), "json");
  assert.equal(pista("https://exemplo.test/muito/longo"), "url");
  assert.equal(pista("Parágrafo 1"), null);
});

test("reindentar preserva texto com pontuação dentro de strings", () => {
  assert.equal(reindentar('{"a":"x, {y}: [z]","b":[]}'), '{\n  "a": "x, {y}: [z]",\n  "b": []\n}');
});
