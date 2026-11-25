// Modo Diff, lógica pura (roda com `node --test`): desenha o que o núcleo comparou (comandos
// `diff_estrutura` e `diff_valores`). A classificação quebra × compatível vem pronta do Rust;
// aqui só agrupa pelo campo `classe` e formata.
import type { Textos } from "./i18n.ts";
import { visivel } from "./texto.ts";

export interface Mudanca {
  caminho: string;
  regra: string;
  classe: "quebra" | "compativel";
  antes: [number, number];
  depois: [number, number];
  tipos?: [string[], string[]];
  variante?: string;
}
export interface DiffEstrutura {
  mudancas: Mudanca[];
  iguais: string[];
  invalidas: [number, number];
}
export interface Diferenca {
  caminho: string;
  acao: "adicionado" | "removido" | "alterado";
  antes: string | null;
  depois: string | null;
}
export interface RegistroDiferente {
  registro: number;
  linha_antes: number | null;
  linha_depois: number | null;
  total: number;
  diferencas: Diferenca[];
}
export interface DiffArquivos {
  registros_antes: number;
  registros_depois: number;
  iguais: number;
  diferentes: number;
  so_antes: number;
  so_depois: number;
  invalidos: number;
  /** Pares acima do teto por registro (16 MiB), contados e não comparados. */
  grandes: number;
  diferencas: number;
  guardados: RegistroDiferente[];
  cancelado: boolean;
}

type X = Textos["diff"];
type Fmt = (n: number) => string;

/** Grupos da tela, na ordem do Rust dentro de cada um. */
export function agrupar(d: DiffEstrutura) {
  return {
    quebra: d.mudancas.filter((m) => m.classe === "quebra"),
    compativel: d.mudancas.filter((m) => m.classe === "compativel"),
    igual: d.iguais,
  };
}

const grupo = (id: string, titulo: string, n: string, itens: string[]) =>
  `<section class="dgrupo ${id}" aria-labelledby="dg-${id}"><h3 id="dg-${id}">${titulo} <span>${n}</span></h3><ul>${itens.join("")}</ul></section>`;

/** Uma mudança em `<li>`: caminho, regra, tipos ou variante, e a evidência antes → depois. */
export function itemMudanca(m: Mudanca, x: X, fmt: Fmt): string {
  const ev = (e: [number, number]) => x.evidencia(fmt(e[0]), fmt(e[1]));
  const detalhe = m.tipos
    ? ` <span class="dtipos">${m.tipos[0].join(" | ") || "∅"} → ${m.tipos[1].join(" | ") || "∅"}</span>`
    : m.variante !== undefined
      ? ` <code>${visivel(m.variante)}</code>`
      : "";
  return `<li><code>${visivel(m.caminho)}</code> <b>${x.regras[m.regra] ?? m.regra}</b>${detalhe} <span class="dev">${ev(m.antes)} → ${ev(m.depois)}</span></li>`;
}

/** Resumo (texto puro, para o status) e o HTML dos grupos com itens. */
export function htmlEstrutura(d: DiffEstrutura, x: X, fmt: Fmt): { resumo: string; html: string } {
  const g = agrupar(d);
  const resumo = d.mudancas.length
    ? x.resumoEstrutura(fmt(g.quebra.length), fmt(g.compativel.length), fmt(g.igual.length))
    : x.semMudanca;
  const item = (m: Mudanca) => itemMudanca(m, x, fmt);
  const html = [
    g.quebra.length ? grupo("quebra", x.grupos.quebra, fmt(g.quebra.length), g.quebra.map(item)) : "",
    g.compativel.length ? grupo("compativel", x.grupos.compativel, fmt(g.compativel.length), g.compativel.map(item)) : "",
    g.igual.length ? grupo("igual", x.grupos.igual, fmt(g.igual.length), g.igual.map((c) => `<li><code>${visivel(c)}</code></li>`)) : "",
  ].join("");
  const [a, b] = d.invalidas;
  return { resumo: a || b ? `${resumo} ${x.invalidas(fmt(a), fmt(b))}` : resumo, html };
}

/** Resumo e HTML do diff de valores: um bloco por registro diferente guardado. */
export function htmlValores(r: DiffArquivos, x: X, fmt: Fmt): { resumo: string; html: string } {
  const algum = r.diferentes + r.so_antes + r.so_depois;
  const base = algum || r.invalidos || r.grandes
    ? x.resumoValores(fmt(r.iguais), fmt(r.diferentes), fmt(r.so_antes), fmt(r.so_depois), fmt(r.invalidos))
    : x.iguais;
  const resumo = r.grandes ? `${base} ${x.grandes(fmt(r.grandes))}` : base;
  const linha = (l: number | null) => (l === null ? "—" : fmt(l + 1));
  const valor = (v: string | null) => (v === null ? "" : `<code>${visivel(v)}</code>`);
  const dif = (d: Diferenca) =>
    `<li class="${d.acao}"><code>${visivel(d.caminho)}</code> <b>${x.acoes[d.acao]}</b> ${
      d.acao === "alterado" ? `${valor(d.antes)} → ${valor(d.depois)}` : valor(d.antes ?? d.depois)
    }</li>`;
  const blocos = r.guardados.map((g, i) => {
    const mais = g.total > g.diferencas.length ? `<li>${x.maisDiferencas(fmt(g.total - g.diferencas.length))}</li>` : "";
    return `<section class="dgrupo" aria-labelledby="dr-${i}"><h3 id="dr-${i}">${x.registro(fmt(g.registro + 1), linha(g.linha_antes), linha(g.linha_depois))}</h3><ul>${g.diferencas.map(dif).join("")}${mais}</ul></section>`;
  });
  const fora = algum - r.guardados.length;
  if (fora > 0) blocos.push(`<p class="dmais">${x.maisRegistros(fmt(fora))}</p>`);
  return { resumo, html: blocos.join("") };
}

/** Campos "registro de antes/depois" (1, 2, 3…): `null` se os dois estão vazios (arquivo todo);
 * um vazio usa o número do outro; `"ruim"` se algum não é inteiro positivo. Devolve base 0. */
export function lerRegistros(a: string, b: string): [number, number] | null | "ruim" {
  [a, b] = [a.trim(), b.trim()];
  if (!a && !b) return null;
  const n = (s: string) => (/^[1-9]\d*$/.test(s) ? Number(s) - 1 : NaN);
  const [x, y] = [n(a || b), n(b || a)];
  return Number.isSafeInteger(x) && Number.isSafeInteger(y) ? [x, y] : "ruim";
}

/** Dois docs são o mesmo arquivo: o mesmo doc ou o mesmo caminho aberto em duas abas. */
export function mesmoArquivo(abas: { doc: number; caminho: string | null }[], a: string, b: string): boolean {
  if (a === b) return true;
  const caminho = (v: string) => abas.find((x) => String(x.doc) === v)?.caminho ?? null;
  const ca = caminho(a);
  return ca !== null && ca === caminho(b);
}
