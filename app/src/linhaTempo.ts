// Linha do tempo de uma pasta, lógica pura (roda com `node --test`): desenha o que o comando
// `linha_do_tempo` percorreu (cada versão contra a anterior que abriu) e guarda a escolha dos dois
// pontos a comparar. Nome e erro vêm do disco: tudo passa por `visivel`.
import type { Textos } from "./i18n.ts";
import { itemMudanca, type Mudanca } from "./diff.ts";
import { visivel } from "./texto.ts";

export interface Passo {
  /** Canônico e já concedido pelo backend. */
  caminho: string;
  nome: string;
  data: string;
  erro: string | null;
  registros: number;
  invalidas: number;
  /** Índice da versão contra a qual comparou; `null` na primeira que abriu. */
  contra: number | null;
  quebras: number;
  compativeis: number;
  mudancas: Mudanca[];
  mais: number;
}
export interface Linha {
  origem: "nome" | "mtime";
  ignorados: number;
  parada: "prazo" | "cancelado" | null;
  passos: Passo[];
}

type X = Textos["diff"];
type Fmt = (n: number) => string;

/** Marca ou desmarca `i`; com um terceiro, sai o marcado primeiro (ficam no máximo dois). */
export function marcar(marcados: number[], i: number): number[] {
  return marcados.includes(i) ? marcados.filter((m) => m !== i) : [...marcados, i].slice(-2);
}

/** Os dois marcados em ordem do tempo (antes, depois), ou `null` sem dois. */
export function parMarcado(marcados: number[]): [number, number] | null {
  if (marcados.length !== 2) return null;
  const [a, b] = marcados;
  return a < b ? [a, b] : [b, a];
}

/** Resumo (texto, para o status) e o HTML da linha: uma versão por item, com caixa para marcar. */
export function htmlLinha(l: Linha, x: X, fmt: Fmt, marcados: number[]): { resumo: string; html: string } {
  const y = x.linha;
  const partes = [y.resumo(fmt(l.passos.length), y.origem[l.origem])];
  if (l.ignorados) partes.push(y.ignorados(fmt(l.ignorados)));
  if (l.parada === "prazo") partes.push(y.prazo);
  const item = (p: Passo, i: number) => {
    const quebra = p.quebras > 0;
    const anterior = p.contra !== null && p.contra !== i - 1 ? ` ${y.contra(visivel(l.passos[p.contra]?.nome ?? ""))}` : "";
    const soma = p.erro !== null
      ? y.erro(visivel(p.erro))
      : p.contra === null
        ? y.primeira(fmt(p.registros))
        : p.quebras + p.compativeis
          ? `${y.mudou(fmt(p.quebras), fmt(p.compativeis))}${anterior}`
          : `${y.igual}${anterior}`;
    const mais = p.mais ? `<li>${x.maisDiferencas(fmt(p.mais))}</li>` : "";
    const lista = p.mudancas.length
      ? `<details${quebra ? " open" : ""}><summary>${y.detalhes}</summary><ul>${p.mudancas.map((m) => itemMudanca(m, x, fmt)).join("")}${mais}</ul></details>`
      : "";
    const marca = p.erro === null ? `<input type="checkbox" data-i="${i}"${marcados.includes(i) ? " checked" : ""}>` : "";
    return `<li class="lpasso${quebra ? " quebra" : ""}${p.erro !== null ? " erro" : ""}"><label>${marca} <time>${visivel(p.data)}</time> <code>${visivel(p.nome)}</code></label> <span class="lsoma">${soma}</span>${lista}</li>`;
  };
  return { resumo: partes.join(" · "), html: `<ol class="linha">${l.passos.map(item).join("")}</ol>` };
}
