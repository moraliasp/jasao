// Lógica pura do modo Estrutura (roda com `node --test`, sem navegador).
// O idioma de `t` vem do ambiente; as expectativas usam o próprio `t` para não depender dele.
import assert from "node:assert/strict";
import { test } from "node:test";
import { corte, detalhe, type Estrutura, faceta, faixa, itens, type Perfil, presenca, segmento } from "../src/estrutura.ts";
import { t } from "../src/i18n.ts";

const fmt = (n: number) => String(n);

// Mesmo formato que o comando `estrutura` devolve (ver o teste de IPC em lib.rs).
const e: Estrutura = {
  entrada: "jsonl",
  registros: 3,
  invalidas: 1,
  caminhos: [
    { caminho: ".", relacao: "registro", profundidade: 0, presenca: 3, de: 3, tipos: [["objeto", 3, 0]], avisos: [] },
    { caminho: ".tipo", relacao: "campo", profundidade: 1, presenca: 3, de: 3, tipos: [["texto", 3, 0]], avisos: [] },
    { caminho: ".x", relacao: "campo", profundidade: 1, presenca: 2, de: 3, tipos: [["inteiro", 1, 2], ["nulo", 1, 0]], avisos: [] },
    { caminho: ".x.a", relacao: "campo", profundidade: 2, presenca: 1, de: 1, tipos: [["lista", 1, 4]], avisos: [] },
    { caminho: ".x.a[]", relacao: "elemento", profundidade: 3, presenca: 5, de: 1, tipos: [["decimal", 5, 4]], avisos: [] },
    { caminho: '.["chave <b>"]', relacao: "campo", profundidade: 1, presenca: 1, de: 3, tipos: [["booleano", 1, 7]], avisos: [] },
  ],
  discriminadores: [
    {
      campo: ".tipo",
      variantes: [
        { valor: "x", registros: 2, linha: 0, obrigatorias: ["tipo", "x"], opcionais: [["z", 1]] },
        { valor: "y", registros: 1, linha: 4, obrigatorias: ["tipo"], opcionais: [] },
      ],
    },
    // Segundo discriminador: com o primeiro, {discriminador, variante} trocados viram outro alvo.
    {
      campo: ".k",
      variantes: [
        { valor: "p", registros: 2, linha: 0, obrigatorias: ["k"], opcionais: [] },
        { valor: "q", registros: 1, linha: 7, obrigatorias: ["k"], opcionais: [] },
      ],
    },
  ],
};

test("segmento é o pedaço do caminho depois do pai", () => {
  assert.equal(segmento(".a", "."), "a");
  assert.equal(segmento(".a.b", ".a"), "b");
  assert.equal(segmento(".a[]", ".a"), "[]");
  assert.equal(segmento(".m.*", ".m"), "*");
  assert.equal(segmento('.["x y"]', "."), '["x y"]');
  // Pai que não é prefixo (profundidade contada errado no núcleo): mostra o caminho inteiro.
  assert.equal(segmento(".b", ".a"), ".b");
});

test("presença: todos, N de M, itens e entradas de mapa", () => {
  const [raiz, tipo, x, , el] = e.caminhos;
  assert.equal(presenca(raiz, t, fmt), t.estrutura.registros("3"));
  assert.equal(presenca(tipo, t, fmt), t.estrutura.todos);
  assert.equal(presenca(x, t, fmt), t.estrutura.deN("2", "3"));
  assert.equal(presenca(el, t, fmt), t.estrutura.elementos("5", "1"));
  assert.equal(presenca({ ...el, relacao: "entrada" }, t, fmt), t.estrutura.entradas("5", "1"));
});

test("itens: um por caminho e um por variante, com a 1ª linha para ir à Árvore", () => {
  const it = itens(e, t, fmt);
  assert.equal(it.length, 10);
  assert.deepEqual(it.map((i) => i.linha), [0, 0, 0, 4, 4, 7, 0, 4, 0, 7]);
  // `.x` tem dois tipos: mostra a contagem de cada e marca opcional.
  assert.match(it[2].html, /class="ep op"/);
  assert.ok(it[2].rotulo.includes(t.estrutura.opcional));
  assert.ok(it[2].html.includes(`${t.estrutura.tipos.inteiro} 1 | ${t.estrutura.tipos.nulo} 1`));
  // Recuo pela profundidade e nome relativo ao pai.
  assert.match(it[4].html, /padding-left:76px" title="\.x\.a\[\]">\[\]</);
  // Chave do arquivo é escapada.
  assert.ok(it[5].html.includes("chave &lt;b>"));
  assert.ok(!it[5].html.includes("<b>"));
  // Variante: campo = valor, sempre/às vezes.
  assert.equal(it[6].caminho, ".tipo");
  assert.ok(it[6].html.includes(`.tipo = "x"`));
  assert.ok(it[6].html.includes(`${t.estrutura.opcionais}: z (1)`));
  // Índice em `caminhos` para o comando `perfil`; variante não tem.
  assert.deepEqual(it.map((i) => i.indice), [0, 1, 2, 3, 4, 5, null, null, null, null]);
  // F filtra: caminho → os que têm; variante → os do formato (índices do discriminador).
  assert.deepEqual(it[2].alvo, { caminho: 2, tem: true });
  assert.deepEqual(it[6].alvo, { discriminador: 0, variante: 0 });
  assert.equal(it[6].valor, "x");
  assert.deepEqual(it[7].alvo, { discriminador: 0, variante: 1 });
  assert.deepEqual(it[8].alvo, { discriminador: 1, variante: 0 });
  assert.deepEqual(it[9].alvo, { discriminador: 1, variante: 1 });
  assert.ok(it[8].html.includes(`.k = "p"`));
  assert.equal(it[9].valor, "q");
  assert.equal(it[2].valor, null);
});

test("avisos viram etiqueta e entram no rótulo de leitor de tela", () => {
  const c = { ...e.caminhos[1], avisos: ["numero_como_texto", "segredo"] };
  const [, it] = itens({ ...e, caminhos: [e.caminhos[0], c], discriminadores: [] }, t, fmt);
  const nomes = [t.estrutura.avisos.numero_como_texto, t.estrutura.avisos.segredo];
  assert.equal((it.html.match(/class="ea"/g) ?? []).length, 2);
  for (const n of nomes) assert.ok(it.html.includes(n) && it.rotulo.includes(n), n);
  assert.ok(!itens(e, t, fmt)[1].html.includes('class="ea"'));
});

const p: Perfil = {
  registros: 9, com: 8, sem: 1, linhas_com: [0, 2], linhas_sem: [3], valores: 8,
  numeros: { min: "-5", max: "120", faixas: [{ negativo: true, expoente: 0, n: 1 }, { negativo: false, expoente: null, n: 1 }, { negativo: false, expoente: 2, n: 2 }] },
  textos: { min: '"a"', max: '"<b>"' },
  distintos: 7, distintos_aprox: false, comuns: [["120", 2], ['"<i>"', 1]], comuns_aprox: true, oculto: false,
};

test("faixa de grandeza", () => {
  assert.equal(faixa({ negativo: false, expoente: null }), "0");
  assert.equal(faixa({ negativo: false, expoente: 2 }), "1e2…1e3");
  assert.equal(faixa({ negativo: true, expoente: -1 }), "−1e-1…1e0");
  assert.equal(faixa({ negativo: false, expoente: -30 }), "…1e-29"); // inclui 1e-35
  assert.equal(faixa({ negativo: true, expoente: 30 }), "−1e30…");
});

test("painel do perfil: extremos, faixas, comuns e linhas com e sem (1ª linha = 1)", () => {
  const h = detalhe(".v", p, t, fmt);
  assert.ok(h.includes("-5 … 120"));
  assert.equal((h.match(/class="fx"/g) ?? []).length, 3);
  assert.match(h, /width:100%/); // a faixa mais cheia
  assert.match(h, /width:50%/);
  // Texto do arquivo escapado nos extremos e nos comuns.
  assert.ok(!h.includes("<b>") && !h.includes("<i>"));
  // Só os comuns são aproximados aqui.
  assert.equal((h.match(new RegExp(t.estrutura.aproximado, "g")) ?? []).length, 1);
  assert.deepEqual([...h.matchAll(/data-l="(\d+)">(\d+)</g)].map((m) => [m[1], m[2]]), [["0", "1"], ["2", "3"], ["3", "4"]]);
  assert.ok(h.includes(t.estrutura.primeiras("2")), "8 com, só 2 linhas guardadas");
  // Um botão de filtrar para os com e outro para os sem.
  assert.deepEqual([...h.matchAll(/data-filtrar="(\w+)"/g)].map((m) => m[1]), ["true", "false"]);
  assert.ok(!h.includes(t.estrutura.oculto));
});

test("painel de caminho oculto: só contagens", () => {
  const o: Perfil = { ...p, numeros: null, textos: null, comuns: [], comuns_aprox: false, distintos_aprox: true, oculto: true };
  const h = detalhe(".senha", o, t, fmt);
  assert.ok(h.includes(t.estrutura.oculto));
  assert.ok(!h.includes('class="fx"') && !h.includes('class="cm"'));
  assert.ok(h.includes(t.estrutura.aproximado), "distintos aproximados");
  assert.ok(h.includes('data-l="3"'));
});

test("sem registros sem o caminho, a lista some", () => {
  const h = detalhe(".v", { ...p, sem: 0, linhas_sem: [] }, t, fmt);
  assert.ok(!h.includes(t.estrutura.sem));
  assert.ok(!h.includes('data-filtrar="false"'));
});

test("singular quando há 1 item ou 1 lista", () => {
  assert.notEqual(t.estrutura.elementos("1", "1"), t.estrutura.elementos("2", "2").replace(/2/g, "1"));
});

test("caminho com bidi não chega cru ao title", () => {
  const b: Estrutura = {
    ...e,
    caminhos: [{ caminho: ".a\u202eb", relacao: "campo", profundidade: 1, presenca: 1, de: 1, tipos: [["texto", 1, 0]], avisos: [] }],
    discriminadores: [],
  };
  const [it] = itens(b, t, fmt);
  assert.match(it.html, /title="\.a&lt;U\+202E>b"/);
});

test("facetas: cada valor comum e cada faixa é um botão com rótulo, e o filtro leva o valor inteiro", () => {
  const longo = `"${"x".repeat(200)}"`;
  const comuns: [string, number][] = [
    ['"a\\"b"', 4], ['"a\\\\(b"', 3], ["null", 3], ["true", 2], ["false", 2], ["1.50", 2], [longo, 1], ['"<img src=x onerror=1>"', 1], ['"\u202Eevil"', 1],
  ];
  const q: Perfil = { ...p, comuns };
  const h = detalhe(".v", q, t, fmt);
  assert.deepEqual([...h.matchAll(/data-comum="(\d+)"/g)].map((m) => Number(m[1])), comuns.map((_, i) => i));
  assert.deepEqual([...h.matchAll(/data-faixa="(\d+)"/g)].map((m) => Number(m[1])), [0, 1, 2]);
  // Botão de verdade (teclado e foco), com rótulo para leitor de tela.
  assert.equal((h.match(/<button class="f[cx]" type="button" data-(comum|faixa)="\d+" aria-label="[^"<]+">/g) ?? []).length, comuns.length + 3);
  // Nada do arquivo vira HTML nem quebra o atributo; bidi não chega cru ao rótulo.
  assert.ok(!h.includes("<img") && !h.includes("\u202E\""), h);
  assert.ok(!/aria-label="[^"]*\u202E/.test(h));
  // Na tela o longo aparece cortado; o filtro usa o inteiro.
  assert.ok(!h.includes(longo));
  for (const [i, [v]] of comuns.entries()) {
    const f = faceta(4, ".v", q, { comum: String(i) }, t);
    assert.deepEqual(f?.alvo, { caminho: 4, comum: v });
    assert.equal(f?.rotulo, t.tabela.filtro.comum(".v", v.replace("\u202E", "⟦202E⟧")));
  }
  assert.equal((faceta(4, ".v", q, { comum: "6" }, t)?.alvo as { comum: string }).comum.length, 202);
  assert.equal(faceta(4, ".v", q, { comum: "99" }, t), null);
  assert.equal(faceta(4, ".v", q, {}, t), null);
});

test("facetas: faixa vira sinal e grandeza, inclusive a última e a do zero", () => {
  const fs = p.numeros!.faixas;
  assert.deepEqual(faceta(2, ".v", p, { faixa: "0" }, t), {
    alvo: { caminho: 2, negativo: true, expoente: 0 }, rotulo: t.tabela.filtro.faixa(".v", "−1e0…1e1"),
  });
  assert.deepEqual(faceta(2, ".v", p, { faixa: "1" }, t)?.alvo, { caminho: 2, negativo: false, expoente: null });
  // A última faixa (a do máximo, 120) é a de 1e2…1e3: o núcleo põe o máximo dentro dela.
  const ultima = faceta(2, ".v", p, { faixa: String(fs.length - 1) }, t);
  assert.deepEqual(ultima?.alvo, { caminho: 2, negativo: false, expoente: 2 });
  assert.equal(ultima?.rotulo, t.tabela.filtro.faixa(".v", "1e2…1e3"));
  assert.equal(faceta(2, ".v", p, { faixa: String(fs.length) }, t), null);
  assert.equal(faceta(2, ".v", { ...p, numeros: null }, { faixa: "0" }, t), null);
});

test("corte não parte um par substituto", () => {
  const v = `${"a".repeat(79)}😀resto`;
  assert.ok(corte(v).isWellFormed());
  assert.equal(corte(v), `${"a".repeat(79)}…`);
  assert.equal(corte("curto"), "curto");
});

test("detalhe: caminho sem escalares (a raiz) não mostra '0 distintos'", () => {
  const raiz: Perfil = { ...p, distintos: 0, distintos_aprox: false, numeros: null, textos: null, comuns: [], comuns_aprox: false };
  assert.ok(!detalhe(".", raiz, t, fmt).includes(t.estrutura.distintos("0")));
  assert.ok(detalhe(".x", p, t, fmt).includes(t.estrutura.distintos("7")), "com escalares, distintos aparece");
});
