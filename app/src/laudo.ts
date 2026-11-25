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

/** O que depende do resto da interface: valores da legenda da régua, na ordem das cores c1…c4
 * (o formato fora dela é "outros", c5). */
export interface Contexto { cores: string[] }

/** Ações dos botões, lidas por delegação de clique em `main.ts`. */
export type Acao = "variante" | "estrutura" | "exportar" | "diff";

const botao = (acao: Acao, rotulo: string, dado = "", classe = "lk") =>
  `<button type="button" class="${classe}" data-acao="${acao}"${dado}>${rotulo}</button>`;

/** Até `MAX` achados por seção; o resto vira "e mais n". */
const MAX = 3;

/** HTML do laudo no inspetor; `null` enquanto a inferência não acabou. */
export function laudoHtml(l: Laudo | null, ctx: Contexto, t: Textos, fmt: (n: number) => string): string {
  const L = t.laudo;
  if (!l) return `<p class="more">${L.analisando}</p>`;
  const c = (s: string) => `<code>${visivel(s)}</code>`;
  const lista = (xs: string[], resto: number, mais: (n: string) => string) => {
    const corte = Math.max(0, xs.length - MAX) + resto;
    return `<ul class="find">${xs.slice(0, MAX).map((x) => `<li>${x}</li>`).join("")}${corte ? `<li class="more">${mais(fmt(corte))}</li>` : ""}</ul>`;
  };
  let html = l.varridas < l.total ? `<p class="aviso">${L.parcial(fmt(l.varridas), fmt(l.total))}</p>` : "";

  const d = l.discriminador;
  if (d) {
    const topo = Math.max(1, ...d.variantes.map((v) => v.registros));
    const linhas = d.variantes.map((v) => {
      const i = ctx.cores.indexOf(v.valor);
      const cor = i >= 0 && i < 4 ? i + 1 : 5;
      const nome = v.linha === null ? c(v.valor) : botao("variante", c(v.valor), ` data-linha="${v.linha}" title="${L.verVariante}"`);
      return `<span class="sw pt c${cor}"></span><span class="nm">${nome}</span><span class="n">${fmt(v.registros)}</span>` +
        `<span class="bar"><span class="pt c${cor}" style="width:${((v.registros / topo) * 100).toFixed(1)}%"></span></span>`;
    });
    html += `<h4>${L.formas(c(d.campo))}</h4><div class="fmt">${linhas.join("")}</div>` +
      (d.mais ? `<p class="more">${L.eMais(fmt(d.mais))}</p>` : "");
  } else if (l.registros) html += `<h4>${L.forma}</h4><p class="more">${L.umaForma}</p>`;

  const atencao: string[] = [];
  if (l.invalidas) atencao.push(`<span class="bad">${L.invalidas(fmt(l.invalidas), l.primeira_invalida === null ? null : fmt(l.primeira_invalida + 1))}</span>`);
  if (l.vazias) atencao.push(`<span>${L.vazias(fmt(l.vazias))}</span>`);
  for (const a of l.atencao)
    atencao.push(botao("estrutura", `${c(a.caminho)} <span>${t.estrutura.avisos[a.aviso] ?? visivel(a.aviso)}</span>`, ` data-indice="${a.indice}"`));
  html += `<h4>${L.atencao}</h4>` + (atencao.length || l.atencao_mais ? lista(atencao, l.atencao_mais, L.maisAvisos) : `<p class="more">${L.semAviso}</p>`);

  if (l.registros) {
    const opc = l.opcionais.map((o) => botao("estrutura", `${c(o.caminho)} <span>${L.falta(fmt(o.de - o.presenca))}</span>`, ` data-indice="${o.indice}"`));
    html += `<h4>${L.opcionais}</h4>` + (opc.length || l.opcionais_mais ? lista(opc, l.opcionais_mais, L.maisOpcionais) : `<p class="more">${L.semOpcional}</p>`);
    html += `<div class="acts">${botao("exportar", L.exportar, "", "btn")}${botao("diff", L.comparar, "", "btn")}</div>`;
  }
  return html;
}
