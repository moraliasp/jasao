// Modo Estrutura: desenha o que `nucleo::estrutura` inferiu (comando `estrutura`). Lógica pura,
// sem DOM, para rodar com `node --test`. Nada de inferência aqui: só formatação.
import type { Textos } from "./i18n.ts";

export interface CaminhoE {
  caminho: string;
  relacao: "registro" | "campo" | "elemento" | "entrada";
  profundidade: number;
  presenca: number;
  de: number;
  tipos: [string, number, number | null][];
}
export interface Variante { valor: string; registros: number; linha: number | null; obrigatorias: string[]; opcionais: [string, number][] }
export interface Estrutura {
  entrada: "jsonl" | "json";
  registros: number;
  invalidas: number;
  caminhos: CaminhoE[];
  discriminadores: { campo: string; variantes: Variante[] }[];
}

/** Uma linha da lista: caminho ou variante. `linha` = 1º registro (base 0) para ir à Árvore. */
export interface ItemE { html: string; rotulo: string; caminho: string; linha: number | null }

type Fmt = (n: number) => string;
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");

/** Último segmento do caminho em relação ao pai (`.a.b` sob `.a` → `b`; `[]` e `*` ficam). */
export function segmento(c: string, pai: string | undefined): string {
  if (pai === undefined) return c;
  if (!c.startsWith(pai === "." ? "" : pai)) return c;
  const s = c.slice(pai === "." ? 0 : pai.length);
  return s.replace(/^\.(?=.)/, "");
}

export function tipos(c: CaminhoE, t: Textos, fmt: Fmt): string {
  const so = c.tipos.length === 1;
  return c.tipos.map(([nome, n]) => (so ? t.estrutura.tipos[nome] ?? nome : `${t.estrutura.tipos[nome] ?? nome} ${fmt(n)}`)).join(" | ");
}

/** "todos", "3 de 5", "12 itens em 4 listas", "mapa: 40 entradas em 2 objetos". */
export function presenca(c: CaminhoE, t: Textos, fmt: Fmt): string {
  const e = t.estrutura;
  if (c.relacao === "registro") return e.registros(fmt(c.presenca));
  if (c.relacao === "elemento") return e.elementos(fmt(c.presenca), fmt(c.de));
  if (c.relacao === "entrada") return e.entradas(fmt(c.presenca), fmt(c.de));
  return c.presenca === c.de ? e.todos : e.deN(fmt(c.presenca), fmt(c.de));
}

const primeira = (c: CaminhoE) =>
  c.tipos.reduce<number | null>((m, [, , l]) => (l === null ? m : m === null ? l : Math.min(m, l)), null);

export function itens(e: Estrutura, t: Textos, fmt: Fmt): ItemE[] {
  const pais: string[] = [];
  const out: ItemE[] = e.caminhos.map((c) => {
    pais[c.profundidade] = c.caminho;
    const opcional = c.relacao === "campo" && c.presenca < c.de;
    const nome = c.relacao === "registro" ? t.estrutura.registro : segmento(c.caminho, pais[c.profundidade - 1]);
    const ti = tipos(c, t, fmt);
    const pr = presenca(c, t, fmt);
    return {
      caminho: c.caminho,
      linha: primeira(c),
      rotulo: `${c.caminho}: ${ti}; ${pr}${opcional ? `; ${t.estrutura.opcional}` : ""}`,
      html: `<span class="ec" style="padding-left:${8 + c.profundidade * 20}px" title="${esc(c.caminho)}">${esc(nome)}</span>` +
        `<span class="et">${esc(ti)}</span><span class="ep${opcional ? " op" : ""}">${esc(pr)}</span>`,
    };
  });
  for (const d of e.discriminadores)
    for (const v of d.variantes) {
      const opc = v.opcionais.map(([k, n]) => `${k} (${fmt(n)})`).join(", ");
      const det = `${t.estrutura.obrigatorias}: ${v.obrigatorias.join(", ") || "—"}${opc ? ` · ${t.estrutura.opcionais}: ${opc}` : ""}`;
      out.push({
        caminho: d.campo,
        linha: v.linha,
        rotulo: `${t.estrutura.formato(d.campo, v.valor)}: ${t.estrutura.registros(fmt(v.registros))}; ${det}`,
        html: `<span class="ec" style="padding-left:8px">${esc(`${d.campo} = "${v.valor}"`)}</span>` +
          `<span class="et">${esc(det)}</span><span class="ep">${esc(t.estrutura.registros(fmt(v.registros)))}</span>`,
      });
    }
  return out;
}
