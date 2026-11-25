// Botão Limpar dos recentes (roda com `node --test`, sem navegador): DOM mínimo e IPC simulado.
import assert from "node:assert/strict";
import { test } from "node:test";

type Ouvinte = () => unknown;
class El {
  hidden = false;
  className = "";
  textContent = "";
  title = "";
  type = "";
  filhos: El[] = [];
  ouvintes: Record<string, Ouvinte> = {};
  setAttribute() {}
  replaceChildren() { this.filhos = []; }
  append(...f: El[]) { this.filhos.push(...f); }
  addEventListener(ev: string, f: Ouvinte) { this.ouvintes[ev] = f; }
  achar(p: (e: El) => boolean): El | undefined {
    return this.filhos.find(p) ?? this.filhos.map((f) => f.achar(p)).find(Boolean);
  }
}

let lista = [{ caminho: "/a/x.jsonl", nome: "x.jsonl", pasta: "/a" }];
let falhar: string | null = null;
const chamadas: string[] = [];
let aoMudar: () => void = () => {};
const g = globalThis as Record<string, unknown>;
g.document = { createElement: () => new El() };
g.window = {
  __TAURI_INTERNALS__: {
    transformCallback: (cb: () => void) => ((aoMudar = cb), 1),
    invoke: async (cmd: string) => {
      chamadas.push(cmd);
      if (cmd === "recentes") return lista;
      if (cmd === "limpar_recentes") {
        if (falhar) throw falhar;
        lista = [];
        aoMudar(); // o Rust avisa `recentes`
      }
      return null;
    },
  },
};

const { criarRecentes } = await import("../src/recentes.ts");
const { erroDoRust, t } = await import("../src/i18n.ts");
const espera = () => new Promise((r) => setTimeout(r, 0));

test("Limpar chama limpar_recentes e a lista vazia some", async () => {
  const el = new El();
  const avisos: string[] = [];
  criarRecentes(el as unknown as HTMLElement, () => {}, (m) => avisos.push(m));
  await espera();
  assert.equal(el.hidden, false);
  const botao = el.achar((e) => e.className === "limpar")!;
  assert.equal(botao.textContent, t.recentes.limpar);
  await botao.ouvintes.click();
  await espera();
  assert.ok(chamadas.includes("limpar_recentes"));
  assert.equal(el.hidden, true);
  assert.deepEqual(avisos, []);
});

test("falha ao limpar vira aviso, e a lista fica", async () => {
  lista = [{ caminho: "/a/y.json", nome: "y.json", pasta: "/a" }];
  falhar = "jasao:negado";
  const el = new El();
  const avisos: string[] = [];
  criarRecentes(el as unknown as HTMLElement, () => {}, (m) => avisos.push(m));
  await espera();
  await el.achar((e) => e.className === "limpar")!.ouvintes.click();
  assert.deepEqual(avisos, [`${t.erro}: ${erroDoRust("jasao:negado")}`]);
  assert.equal(el.hidden, false);
});
