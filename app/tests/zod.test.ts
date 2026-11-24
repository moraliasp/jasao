// Roda o Zod gerado pelo `nucleo::exportar` (dourados em tests/dourados, regravados por
// `ATUALIZAR_DOURADOS=1 cargo test -p nucleo --test exportar`) com o `zod@4` de verdade.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const pasta = new URL("./dourados/", import.meta.url);
const linhas = (base: string): unknown[] =>
  readFileSync(new URL(`${base}.jsonl`, pasta), "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));

for (const base of ["exportar", "hostil"]) {
  for (const sufixo of ["", ".literal-numero"]) {
    test(`${base}${sufixo}.zod.ts aceita todo registro do próprio arquivo`, async () => {
      const { Registro } = await import(new URL(`${base}${sufixo}.zod.ts`, pasta).href);
      const regs = linhas(base);
      assert.ok(regs.length > 0);
      regs.forEach((r, i) => {
        const res = Registro.safeParse(r);
        assert.ok(res.success, `linha ${i + 1}: ${res.error?.message}`);
      });
    });
  }
}

test("tratar como número converte o texto numérico e trava o valor único", async () => {
  const { Registro } = await import(new URL("exportar.literal-numero.zod.ts", pasta).href);
  const pagamento = linhas("exportar")[1] as Record<string, unknown>;
  const v = Registro.parse(pagamento);
  assert.equal(v.valor, 200);
  assert.equal(v.moeda, "BRL");
  assert.ok(!Registro.safeParse({ ...pagamento, moeda: "USD" }).success);
  // Como o Schema da entrada: só texto no padrão numérico (`z.coerce.number()` aceitaria todos).
  for (const valor of [null, "", [], [7], true, "1,5", 200])
    assert.ok(!Registro.safeParse({ ...pagamento, valor }).success, JSON.stringify(valor));
});

test("chaves hostis chegam iguais ao objeto validado", async () => {
  const { Registro } = await import(new URL("hostil.zod.ts", pasta).href);
  const r = linhas("hostil")[0] as Record<string, unknown>;
  const v = Registro.parse(r);
  for (const k of Object.keys(r)) if (k !== "__proto__") assert.deepEqual(v[k], r[k], JSON.stringify(k));
  assert.equal(Object.getPrototypeOf(v), Object.prototype);
});
