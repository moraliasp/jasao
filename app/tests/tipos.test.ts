// Compila os tipos TypeScript gerados pelo `nucleo::exportar::tipos` (dourados em tests/dourados,
// regravados por `ATUALIZAR_DOURADOS=1 cargo test -p nucleo --test exportar`) com o `tsc` do
// projeto, em modo estrito: o .d.ts precisa compilar, todo registro do próprio arquivo precisa
// caber no tipo e os casos marcados com `@ts-expect-error` precisam ser recusados.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const pasta = fileURLToPath(new URL("./dourados/", import.meta.url));
const tsc = fileURLToPath(new URL("../node_modules/typescript/bin/tsc", import.meta.url));
const registros = (base: string) =>
  readFileSync(join(pasta, `${base}.jsonl`), "utf8").split("\n").filter(Boolean).join(",\n");

/** Roda o `tsc` num arquivo de checagem; devolve a saída se falhar. */
function compilar(codigo: string): string | null {
  const dir = mkdtempSync(join(tmpdir(), "jasao-tipos-"));
  try {
    const f = join(dir, "checar.ts");
    writeFileSync(f, codigo);
    // prettier-ignore
    execFileSync(process.execPath, [tsc, "--noEmit", "--strict", "--skipLibCheck", "false", "--target", "es2020",
      "--lib", "es2020", "--module", "esnext", "--moduleResolution", "bundler", f], { encoding: "utf8", cwd: dir });
    return null;
  } catch (e) {
    const err = e as { stdout?: string; message: string };
    return err.stdout || err.message;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const importar = (nome: string, arquivo: string) => `import type { Registro as ${nome} } from ${JSON.stringify(join(pasta, arquivo))};\n`;

test("cada .d.ts compila e aceita todo registro do próprio arquivo", () => {
  let codigo = "";
  let i = 0;
  for (const base of ["exportar", "hostil"])
    for (const sufixo of ["", ".literal-numero"]) {
      codigo += importar(`R${i}`, `${base}${sufixo}.d.ts`);
      codigo += `export const r${i}: R${i}[] = [\n${registros(base)}\n];\n`;
      i++;
    }
  const erro = compilar(codigo);
  assert.equal(erro, null, erro ?? "");
});

test("a união discriminada recusa variante errada e campo obrigatório faltando", () => {
  const codigo =
    importar("R", "exportar.d.ts") +
    importar("L", "exportar.literal-numero.d.ts") +
    `const evento = { tipo: "evento", id: 1, moeda: "BRL", msg: "m", nivel: "info", duracao: 1 } as const;
export const ok: R = evento;
// Campo opcional (\`obs?\`) pode faltar; \`cidade\` aceita null.
export const pedido: R = { tipo: "pedido", id: 3, moeda: "BRL", cliente: { nome: "n", cidade: null }, itens: [], cupom: null };
// @ts-expect-error discriminador fora das variantes
export const e1: R = { ...evento, tipo: "outro" };
// @ts-expect-error campo obrigatório da variante faltando
export const e2: R = { tipo: "evento", id: 1, moeda: "BRL", msg: "m", nivel: "info" };
// @ts-expect-error campo de outra variante com o discriminador desta
export const e3: R = { tipo: "pagamento", id: 1, moeda: "BRL", msg: "m", nivel: "info", duracao: 1 };
// @ts-expect-error tipo do campo errado
export const e4: R = { ...evento, id: "1" };
// @ts-expect-error valor único travado (--literal)
export const e5: L = { ...evento, moeda: "USD" };
// @ts-expect-error texto numérico continua texto nos tipos
export const e6: R = { tipo: "pagamento", id: 2, moeda: "BRL", valor: 200, parcelas: 3, aprovado: true, protocolo: 1 };
`;
  const erro = compilar(codigo);
  assert.equal(erro, null, erro ?? "");
});
