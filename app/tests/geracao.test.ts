import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { eventoVale, porGeracao } from "../src/geracao.ts";

type Ev = { geracao: number; fim: boolean };

test("o fim que chega antes da geração ser fixada é aplicado ao fixar", () => {
  const vistos: Ev[] = [];
  const e = porGeracao<Ev>((v) => vistos.push(v));
  e.receber({ geracao: 1, fim: false });
  e.receber({ geracao: 1, fim: true }); // arquivo pequeno: terminou antes do `exportar` responder
  assert.deepEqual(vistos, []);
  e.fixar(1);
  assert.deepEqual(vistos, [{ geracao: 1, fim: true }]);
  e.receber({ geracao: 1, fim: false }); // atrasado da mesma geração ainda passa
  assert.equal(vistos.length, 2);
});

test("evento de geração velha ou parada é descartado", () => {
  const vistos: Ev[] = [];
  const e = porGeracao<Ev>((v) => vistos.push(v));
  e.receber({ geracao: 0, fim: true });
  e.fixar(2);
  e.receber({ geracao: 1, fim: true });
  assert.deepEqual(vistos, []);
  e.receber({ geracao: 3, fim: true }); // da próxima, ainda sem resposta
  e.fixar(0); // parar (Esc, troca de vista): o guardado some
  e.fixar(3);
  assert.deepEqual(vistos, []);
  assert.equal(e.atual, 3);
});

test("fim de uma geração mais nova sobrevive a fixar uma mais velha (respostas fora de ordem)", () => {
  const vistos: Ev[] = [];
  const e = porGeracao<Ev>((v) => vistos.push(v));
  e.receber({ geracao: 2, fim: true });
  e.fixar(1); // resposta da 1ª chamada chega depois do fim da 2ª
  e.fixar(2);
  assert.deepEqual(vistos, [{ geracao: 2, fim: true }]);
});

test("consulta: evento de consulta cancelada não vale, nem o `fim` emitido junto", () => {
  const e = { rodando: true, esperando: false, geracao: 2 };
  assert.equal(eventoVale(e, 2), true);
  assert.equal(eventoVale(e, 3), true);
  assert.equal(eventoVale(e, 1), false); // geração velha
  assert.equal(eventoVale({ ...e, esperando: true }, 2), false); // `consultar` em voo
  assert.equal(eventoVale({ ...e, rodando: false }, 2), false); // cancelada
  assert.equal(eventoVale({ ...e, rodando: false }, 3), false);
  assert.equal(eventoVale(undefined, 2), false);
});

test("consulta.ts filtra o evento de consulta com eventoVale", () => {
  // Sem Tauri no node: o call site é conferido na fonte (o bug era o guard inline sem `rodando`).
  const fonte = readFileSync(new URL("../src/consulta.ts", import.meta.url), "utf8");
  assert.match(fonte, /const e = estados\.get\(p\.id\);\s*if \(!e \|\| !eventoVale\(e, p\.geracao\)\) return;/);
});
