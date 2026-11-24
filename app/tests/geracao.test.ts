import assert from "node:assert/strict";
import { test } from "node:test";
import { porGeracao } from "../src/geracao.ts";

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
