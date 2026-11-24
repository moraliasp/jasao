// Lógica pura do laudo do painel (roda com `node --test`, sem navegador).
import assert from "node:assert/strict";
import { test } from "node:test";
import { type Laudo, laudoHtml } from "../src/laudo.ts";
import { t } from "../src/i18n.ts";

const fmt = (n: number) => String(n);
const sem = { tabela: false, outraAba: false };

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
  assert.ok(h.includes(t.laudo.formas("2", "<code>.tipo</code>")), h);
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

test("passos: Tabela só se serve, Comparar só com outra aba, Exportar com registros", () => {
  const nenhum = laudoHtml(l, sem, t, fmt);
  assert.ok(!nenhum.includes(`data-acao="tabela"`) && !nenhum.includes(`data-acao="diff"`), nenhum);
  assert.ok(nenhum.includes(`data-acao="exportar"`), nenhum);
  const todos = laudoHtml(l, { tabela: true, outraAba: true }, t, fmt);
  assert.ok(todos.includes(`data-acao="tabela"`) && todos.includes(`data-acao="diff"`), todos);
  assert.ok(!laudoHtml(l, { tabela: null, outraAba: false }, t, fmt).includes(`data-acao="tabela"`));
  const vazio = laudoHtml({ ...l, registros: 0, discriminador: null, opcionais: [], atencao: [], invalidas: 0 }, sem, t, fmt);
  assert.ok(!vazio.includes(`data-acao="exportar"`) && vazio.includes(t.laudo.semAviso), vazio);
});

test("tetos e parcial aparecem como texto", () => {
  const h = laudoHtml({ ...l, varridas: 2, opcionais_mais: 4, atencao_mais: 7, discriminador: { ...l.discriminador!, mais: 2 } }, sem, t, fmt);
  assert.ok(h.includes(t.laudo.parcial("2", "4")), h);
  assert.ok(h.includes(t.laudo.maisOpcionais("4")) && h.includes(t.laudo.maisAvisos("7")), h);
  assert.ok(h.includes(t.laudo.formas("4", "<code>.tipo</code>")) && h.includes(t.laudo.eMais("2")), h);
});
