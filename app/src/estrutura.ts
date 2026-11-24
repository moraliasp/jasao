// Modo Estrutura: desenha o que `nucleo::estrutura` inferiu (comando `estrutura`). Lógica pura,
// sem DOM, para rodar com `node --test`. Nada de inferência aqui: só formatação.
import type { Textos } from "./i18n.ts";
import { type Alvo, type FiltroUi, rotuloFiltro } from "./tabela.ts";
import { escAtributo, visivel } from "./texto.ts";

export interface CaminhoE {
  caminho: string;
  relacao: "registro" | "campo" | "elemento" | "entrada";
  profundidade: number;
  presenca: number;
  de: number;
  tipos: [string, number, number | null][];
  /** `numero_como_texto`, `data_fora_iso`, `valor_unico`, `opaco`, `segredo` (só aponta o caminho). */
  avisos: string[];
}
export interface Variante { valor: string; registros: number; linha: number | null; obrigatorias: string[]; opcionais: [string, number][] }
export interface Estrutura {
  entrada: "jsonl" | "json";
  registros: number;
  invalidas: number;
  caminhos: CaminhoE[];
  discriminadores: { campo: string; variantes: Variante[] }[];
}

/** Uma linha da lista: caminho ou variante. `linha` = 1º registro (base 0) para ir à Árvore;
 * `indice` = posição em `caminhos` (para o comando `perfil`), `null` nas variantes.
 * `alvo`/`valor`: o que F filtra (caminho: os que têm; variante: os do formato). */
export interface ItemE {
  html: string; rotulo: string; caminho: string; linha: number | null; indice: number | null; alvo: Alvo; valor: string | null;
}

/** O que o comando `perfil` devolve (ver `nucleo::perfil::Perfil`). */
export interface Perfil {
  registros: number;
  com: number;
  sem: number;
  linhas_com: number[];
  linhas_sem: number[];
  valores: number;
  numeros: { min: string; max: string; faixas: { negativo: boolean; expoente: number | null; n: number }[] } | null;
  textos: { min: string; max: string } | null;
  distintos: number;
  distintos_aprox: boolean;
  comuns: [string, number][];
  comuns_aprox: boolean;
  oculto: boolean;
}

type Fmt = (n: number) => string;
const esc = escAtributo;

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
  const out: ItemE[] = e.caminhos.map((c, indice) => {
    pais[c.profundidade] = c.caminho;
    const opcional = c.relacao === "campo" && c.presenca < c.de;
    const nome = c.relacao === "registro" ? t.estrutura.registro : segmento(c.caminho, pais[c.profundidade - 1]);
    const ti = tipos(c, t, fmt);
    const pr = presenca(c, t, fmt);
    const av = c.avisos.map((a) => t.estrutura.avisos[a] ?? a);
    return {
      caminho: c.caminho,
      linha: primeira(c),
      indice,
      alvo: { caminho: indice, tem: true },
      valor: null,
      rotulo: `${c.caminho}: ${ti}; ${pr}${opcional ? `; ${t.estrutura.opcional}` : ""}${av.map((a) => `; ${t.estrutura.aviso}: ${a}`).join("")}`,
      html: `<span class="ec" style="padding-left:${8 + c.profundidade * 20}px" title="${esc(c.caminho)}">${visivel(nome)}</span>` +
        `<span class="et">${esc(ti)}${av.map((a) => ` <span class="ea">${esc(a)}</span>`).join("")}</span><span class="ep${opcional ? " op" : ""}">${esc(pr)}</span>`,
    };
  });
  for (const [discriminador, d] of e.discriminadores.entries())
    for (const [variante, v] of d.variantes.entries()) {
      const opc = v.opcionais.map(([k, n]) => `${k} (${fmt(n)})`).join(", ");
      const det = `${t.estrutura.obrigatorias}: ${v.obrigatorias.join(", ") || "—"}${opc ? ` · ${t.estrutura.opcionais}: ${opc}` : ""}`;
      out.push({
        caminho: d.campo,
        linha: v.linha,
        indice: null,
        alvo: { discriminador, variante },
        valor: v.valor,
        rotulo: `${t.estrutura.formato(d.campo, v.valor)}: ${t.estrutura.registros(fmt(v.registros))}; ${det}`,
        html: `<span class="ec" style="padding-left:8px">${visivel(`${d.campo} = "${v.valor}"`)}</span>` +
          `<span class="et">${visivel(det)}</span><span class="ep">${esc(t.estrutura.registros(fmt(v.registros)))}</span>`,
      });
    }
  return out;
}

/** Valor cru do núcleo para mostrar: cortado em `MAX` caracteres visíveis. */
const MAX = 80;
const corte = (v: string) => (v.length > MAX ? `${v.slice(0, MAX)}…` : v);
const curto = (v: string) => visivel(corte(v));

/** Faixa de grandeza: `0`, `1e2…1e3`, `−1e0…1e1`. */
export function faixa(f: { negativo: boolean; expoente: number | null }): string {
  if (f.expoente === null) return "0";
  return `${f.negativo ? "−" : ""}1e${f.expoente}…1e${f.expoente + 1}`;
}

/** Linhas (base 0) como botões `data-l` (a Árvore abre o registro), 1ª linha = 1, e o botão
 * `data-filtrar` que filtra todos eles (`tem` = com ou sem o caminho). */
function linhas(rot: string, n: number, ls: number[], tem: boolean, t: Textos, fmt: Fmt): string {
  if (!n) return "";
  const mais = n > ls.length ? ` <span class="pm">${esc(t.estrutura.primeiras(fmt(ls.length)))}</span>` : "";
  const filtrar = `<button class="lk" type="button" data-filtrar="${tem}">${esc(t.estrutura.filtrar(fmt(n)))}</button>`;
  return `<p class="rot">${esc(rot)}: ${esc(fmt(n))}${mais} · ${filtrar}</p><p class="pl">` +
    ls.map((l) => `<button class="lk" type="button" data-l="${l}">${esc(fmt(l + 1))}</button>`).join(" ") + "</p>";
}

/** Painel do caminho selecionado: estatísticas e registros que têm, ou não têm, o caminho. */
export function detalhe(caminho: string, p: Perfil, t: Textos, fmt: Fmt): string {
  const e = t.estrutura;
  const aprox = (sim: boolean) => (sim ? ` <span class="pm">${esc(e.aproximado)}</span>` : "");
  let h = `<p class="rot">${esc(e.perfil)} · ${visivel(caminho)}</p>`;
  if (p.oculto) h += `<p class="pm">${esc(e.oculto)}</p>`;
  h += `<p>${esc(e.valores(fmt(p.valores)))} · ${esc(e.distintos(fmt(p.distintos)))}${aprox(p.distintos_aprox)}</p>`;
  // Cada faixa e cada valor comum é um botão (`data-faixa`/`data-comum` = posição no perfil):
  // filtra a Tabela por ele (ver `faceta`). O rótulo leva o valor cortado, como na tela.
  const rot = (f: Omit<FiltroUi, "achados">, n: number) => esc(e.facetar(rotuloFiltro(f, t), fmt(n)));
  const base = { caminho, tem: null, valor: null };
  if (p.numeros) {
    const max = Math.max(...p.numeros.faixas.map((f) => f.n));
    h += `<p class="rot">${esc(e.numeros)}</p><p>${esc(e.minMax)}: ${curto(p.numeros.min)} … ${curto(p.numeros.max)}</p>` +
      p.numeros.faixas.map((f, i) =>
        `<button class="fx" type="button" data-faixa="${i}" aria-label="${rot({ ...base, faixa: [f.negativo, f.expoente] }, f.n)}">` +
        `<span>${esc(faixa(f))}</span><i style="width:${Math.max(1, Math.round((f.n / max) * 100))}%"></i><span>${esc(fmt(f.n))}</span></button>`).join("");
  }
  if (p.textos) h += `<p class="rot">${esc(e.textos)}</p><p>${esc(e.minMax)}: ${curto(p.textos.min)} … ${curto(p.textos.max)}</p>`;
  if (p.comuns.length)
    h += `<p class="rot">${esc(e.comuns)}${aprox(p.comuns_aprox)}</p><ol class="cm">` +
      p.comuns.map(([v, n], i) =>
        `<li><button class="fc" type="button" data-comum="${i}" aria-label="${rot({ ...base, comum: corte(v) }, n)}">` +
        `<span>${curto(v)}</span><span>${esc(fmt(n))}</span></button></li>`).join("") + "</ol>";
  return h + linhas(e.com, p.com, p.linhas_com, true, t, fmt) + linhas(e.sem, p.sem, p.linhas_sem, false, t, fmt);
}

/** Clique num valor comum (`data-comum`) ou numa faixa (`data-faixa`) do painel: o alvo do
 * filtro, com o valor inteiro do perfil (o painel mostra cortado), e o rótulo do chip. `indice` =
 * posição do caminho em `estrutura().caminhos`. `null` se o botão não aponta para nada. */
export function faceta(
  indice: number, caminho: string, p: Perfil, b: { comum?: string; faixa?: string }, t: Textos,
): { alvo: Alvo; rotulo: string } | null {
  const base = { caminho, tem: null, valor: null };
  if (b.comum !== undefined) {
    const v = p.comuns[Number(b.comum)]?.[0];
    if (v === undefined) return null;
    return { alvo: { caminho: indice, comum: v }, rotulo: rotuloFiltro({ ...base, comum: v }, t) };
  }
  const f = b.faixa === undefined ? undefined : p.numeros?.faixas[Number(b.faixa)];
  if (!f) return null;
  return {
    alvo: { caminho: indice, negativo: f.negativo, expoente: f.expoente },
    rotulo: rotuloFiltro({ ...base, faixa: [f.negativo, f.expoente] }, t),
  };
}
