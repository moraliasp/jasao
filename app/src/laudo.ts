// Laudo do painel de entrada: desenha o que `nucleo::laudo` montou (comando `laudo`). Lógica pura,
// sem DOM, para rodar com `node --test`. Texto do arquivo (caminho, valor) só passa por `visivel`.
import type { Textos } from "./i18n.ts";
import { visivel } from "./texto.ts";

/** Mesmo formato que o comando `laudo` devolve (ver `nucleo::laudo::Laudo`). */
export interface Laudo {
  conteudo: string;
  bytes: number;
  registros: number;
  varridas: number;
  total: number;
  invalidas: number;
  primeira_invalida: number | null;
  vazias: number;
  discriminador: { campo: string; variantes: { valor: string; registros: number; linha: number | null }[]; mais: number } | null;
  opcionais: { caminho: string; indice: number; presenca: number; de: number }[];
  opcionais_mais: number;
  atencao: { aviso: string; caminho: string; indice: number }[];
  atencao_mais: number;
}

/** O que depende do resto da interface: a Tabela serve (`null`: ainda não se sabe) e há outra aba
 * com arquivo para comparar. */
export interface Contexto { tabela: boolean | null; outraAba: boolean }

/** Ações dos botões, lidas por delegação de clique em `main.ts`. */
export type Acao = "tabela" | "variante" | "estrutura" | "exportar" | "diff";

const botao = (acao: Acao, rotulo: string, dado = "") =>
  `<button type="button" class="lk" data-acao="${acao}"${dado}>${rotulo}</button>`;

/** HTML do laudo; `null` enquanto a inferência não acabou. */
export function laudoHtml(l: Laudo | null, ctx: Contexto, t: Textos, fmt: (n: number) => string): string {
  const L = t.laudo;
  if (!l) return `<h2>${L.titulo}</h2><p class="pm">${L.analisando}</p>`;
  const c = (s: string) => `<code>${visivel(s)}</code>`;
  const forma: string[] = [];
  const d = l.discriminador;
  if (d) {
    const vs = d.variantes.map((v) => {
      const txt = `${c(v.valor)} (${fmt(v.registros)})`;
      return v.linha === null ? txt : botao("variante", txt, ` data-linha="${v.linha}" title="${L.verVariante}"`);
    });
    forma.push(`${L.formas(fmt(d.variantes.length + d.mais), c(d.campo))}: ${vs.join(", ")}${d.mais ? ` ${L.eMais(fmt(d.mais))}` : ""}`);
  } else if (l.registros) forma.push(L.umaForma);
  for (const o of l.opcionais) forma.push(botao("estrutura", L.falta(c(o.caminho), fmt(o.de - o.presenca), fmt(o.de)), ` data-indice="${o.indice}"`));
  if (l.opcionais_mais) forma.push(L.maisOpcionais(fmt(l.opcionais_mais)));
  else if (!l.opcionais.length && l.registros) forma.push(L.semOpcional);

  const atencao: string[] = [];
  if (l.invalidas) atencao.push(L.invalidas(fmt(l.invalidas), l.primeira_invalida === null ? null : fmt(l.primeira_invalida + 1)));
  if (l.vazias) atencao.push(L.vazias(fmt(l.vazias)));
  for (const a of l.atencao) atencao.push(botao("estrutura", `${c(a.caminho)}: ${t.estrutura.avisos[a.aviso] ?? visivel(a.aviso)}`, ` data-indice="${a.indice}"`));
  if (l.atencao_mais) atencao.push(L.maisAvisos(fmt(l.atencao_mais)));
  if (!atencao.length) atencao.push(L.semAviso);

  const passos: string[] = [];
  if (ctx.tabela) passos.push(botao("tabela", L.abrirTabela));
  if (l.registros) passos.push(botao("exportar", L.exportar));
  if (ctx.outraAba) passos.push(botao("diff", L.comparar));

  const ul = (xs: string[]) => `<ul>${xs.map((x) => `<li>${x}</li>`).join("")}</ul>`;
  const parcial = l.varridas < l.total ? `<p class="aviso">${L.parcial(fmt(l.varridas), fmt(l.total))}</p>` : "";
  return `<h2>${L.titulo}</h2>${parcial}` +
    (forma.length ? `<h3>${L.forma}</h3>${ul(forma)}` : "") +
    `<h3>${L.atencao}</h3>${ul(atencao)}` +
    (passos.length ? `<div class="passos">${passos.join("")}</div>` : "");
}
