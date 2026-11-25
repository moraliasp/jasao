// Lógica pura do laudo do painel (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { type Laudo, laudoHtml } from "../src/laudo.ts";
import { t } from "../src/i18n.ts";

const fmt = (n: number) => String(n);
const sem = { cores: ["a"] };

// Mesmo formato que o comando `laudo` devolve (ver o teste de IPC em lib.rs).
const l: Laudo = {
  conteudo: "objetos", bytes: 80, registros: 3, varridas: 4, total: 4,
  invalidas: 1, primeira_invalida: 3, vazias: 0,
  discriminador: { campo: ".tipo", variantes: [{ valor: "a", registros: 2, linha: 0 }, { valor: "b", registros: 1, linha: 1 }], mais: 0 },
  opcionais: [{ caminho: ".x", indice: 3, presenca: 2, de: 3 }],
  opcionais_mais: 0,
  atencao: [{ aviso: "segredo", caminho: ".token", indice: 5 }],
  atencao_mais: 0,
};

test("sem laudo ainda: diz que está analisando", () => {
  assert.ok(laudoHtml(null, sem, t, fmt).includes(t.laudo.analisando));
});

test("forma, opcionais e atenção levam ao ponto certo", () => {
  const h = laudoHtml(l, sem, t, fmt);
  assert.ok(h.includes(t.laudo.formas("<code>.tipo</code>")), h);
  assert.ok(h.includes(`class="sw pt c1"`) && h.includes(`class="sw pt c5"`), h); // "b" fora da legenda: outros
  assert.ok(h.includes(`data-acao="variante" data-linha="1"`), h);
  assert.ok(h.includes(`data-acao="estrutura" data-indice="3"`), h);
  assert.ok(h.includes(`data-acao="estrutura" data-indice="5"`), h);
  assert.ok(h.includes(t.estrutura.avisos.segredo), h);
  // Linha 3 em base 0 é a 4ª linha do arquivo.
  assert.ok(h.includes(t.laudo.invalidas("1", "4")), h);
});

test("caminho e valor hostis saem escapados", () => {
  const h = laudoHtml(
    {
      ...l,
      discriminador: { campo: ".<img src=x onerror=alert(1)>", variantes: [{ valor: "<script>&", registros: 1, linha: 0 }], mais: 0 },
      atencao: [{ aviso: "<b>novo</b>", caminho: '.["‮<i>"]', indice: 0 }],
    },
    sem, t, fmt,
  );
  assert.ok(!/<(img|script|b|i)[\s>]/.test(h), h);
  assert.ok(h.includes("&lt;script>&amp;"), h);
  assert.ok(h.includes('class="inv"'), h); // o invisível fica marcado, não cru e solto
});

test("ações: Exportar e Comparar com registros; seções vazias dizem que não há nada", () => {
  const h = laudoHtml(l, sem, t, fmt);
  assert.ok(h.includes(`data-acao="exportar"`) && h.includes(`data-acao="diff"`), h);
  const vazio = laudoHtml({ ...l, registros: 0, discriminador: null, opcionais: [], atencao: [], invalidas: 0 }, sem, t, fmt);
  assert.ok(!vazio.includes(`data-acao="exportar"`) && vazio.includes(t.laudo.semAviso), vazio);
});

test("no máximo 3 achados por seção; o resto soma no \"e mais\"", () => {
  const avisos = [1, 2, 3, 4].map((i) => ({ aviso: "segredo", caminho: `.s${i}`, indice: i }));
  const h = laudoHtml({ ...l, atencao: avisos, atencao_mais: 2 }, sem, t, fmt);
  // inválidas + 4 avisos = 5 achados: mostra 3, e mais 2 + 2 do core
  assert.ok(h.includes(t.laudo.maisAvisos("4")) && !h.includes("<code>.s3</code>"), h);
});

test("tetos e parcial aparecem como texto", () => {
  const h = laudoHtml({ ...l, varridas: 2, opcionais_mais: 4, atencao_mais: 7, discriminador: { ...l.discriminador!, mais: 2 } }, sem, t, fmt);
  assert.ok(h.includes(t.laudo.parcial("2", "4")), h);
  assert.ok(h.includes(t.laudo.maisOpcionais("4")) && h.includes(t.laudo.maisAvisos("7")), h);
  assert.ok(h.includes(t.laudo.eMais("2")), h);
});

test("segredo com valores distintos: o painel não mostra valor nenhum", () => {
  // O core não manda discriminador para campo de segredo (teste em estrutura.rs e no IPC);
  // aqui, o que ele manda nesse caso: só o caminho com o aviso.
  const segredo: Laudo = { ...l, discriminador: null, atencao: [{ aviso: "segredo", caminho: ".token", indice: 1 }] };
  const h = laudoHtml(segredo, sem, t, fmt);
  assert.ok(h.includes("<code>.token</code>") && h.includes(t.estrutura.avisos.segredo), h);
  for (const v of ["hunter2xyz", "s3cr3tvalue"]) assert.ok(!h.includes(v), h);
});
