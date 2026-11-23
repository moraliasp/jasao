// Modo Tabela, parte pura: formata o que `nucleo::tabela` devolve (comandos `tabela` e
// `tabela_linhas`). Sem DOM, para rodar com `node --test`; a grade virtual fica em `grade.ts`.
// Todo texto que vem do arquivo (nome de coluna, valor, linha inválida) passa por `visivel`.
import type { Motivo, Textos } from "./i18n.ts";
import { visivel } from "./texto.ts";

export type TipoValor = "objeto" | "lista" | "texto" | "numero" | "booleano" | "nulo";
export interface Coluna { nome: string; caminho: string; presenca: number; tipos: string[] }
export interface Celula { tipo: TipoValor; texto: string; filhos: number; byte: number }
export interface LinhaT {
  registro: number;
  linha: number;
  byte: number;
  celulas: (Celula | null)[]; // null: campo ausente (diferente de null do JSON)
  erro: { byte: number; motivo: Motivo } | null;
  previa: string | null;
}
export type Indisponivel =
  | { motivo: "vazio" | "nao_lista" | "mapa" | "sem_campos" | "json_invalido" | "cancelado" }
  | { motivo: "nao_objetos"; n: number };
export interface Ordem { coluna: number; desc: boolean }
export interface TabelaUi {
  registros: number;
  objetos: number;
  colunas: Coluna[];
  omitidas: number;
  indisponivel: Indisponivel | null;
  ordem: Ordem | null;
}

type Fmt = (n: number) => string;
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");

/** Colunas na ordem da tela: as fixadas primeiro (na ordem em que foram fixadas), depois as
 * outras na ordem do arquivo; ocultas ficam fora. Índices de `TabelaUi.colunas`. */
export function ordemVisivel(n: number, fixas: number[], ocultas: Set<number>): number[] {
  const f = fixas.filter((c) => c < n && !ocultas.has(c));
  const resto = [];
  for (let c = 0; c < n; c++) if (!ocultas.has(c) && !f.includes(c)) resto.push(c);
  return [...f, ...resto];
}

/** Clique no cabeçalho: crescente → decrescente → ordem do arquivo. */
export function proximaOrdem(atual: Ordem | null, coluna: number): Ordem | null {
  if (atual?.coluna !== coluna) return { coluna, desc: false };
  return atual.desc ? null : { coluna, desc: true };
}

/** Largura da coluna em px pelo nome (fonte mono de 12,5 px ≈ 7,6 px por caractere). */
export const largura = (c: Coluna) => Math.round(Math.min(280, Math.max(110, c.nome.length * 7.6 + 56)));

/** Célula como HTML e como texto para leitor de tela. */
export function celula(c: Celula | null | undefined, t: Textos, fmt: Fmt): { html: string; rotulo: string } {
  const tb = t.tabela;
  if (!c) return { html: `<span class="aus" title="${esc(tb.ausente)}">—</span>`, rotulo: tb.ausente };
  if (c.tipo === "objeto" || c.tipo === "lista") {
    const resumo = c.tipo === "objeto" ? `{${fmt(c.filhos)}}` : `[${fmt(c.filhos)}]`;
    const rotulo = c.tipo === "objeto" ? tb.objeto(fmt(c.filhos)) : tb.lista(fmt(c.filhos));
    return { html: `<span class="res" data-abrir title="${esc(`${rotulo} · ${tb.abrirNaArvore}`)}">${resumo}</span>`, rotulo };
  }
  return { html: `<span class="v ${c.tipo}">${visivel(c.texto)}</span>`, rotulo: c.texto };
}

/** Linha inválida: uma célula só, com o motivo e o texto cru. */
export function invalida(l: LinhaT, t: Textos, fmt: Fmt): { html: string; rotulo: string } {
  const msg = l.erro ? `${t.byte(fmt(l.erro.byte))}: ${t.motivos[l.erro.motivo] ?? t.motivos.outro}` : t.motivos.outro;
  return {
    html: `<span class="err">${esc(msg)}</span><span class="cru">${visivel(l.previa ?? "")}</span>`,
    rotulo: `${t.tabela.invalida}: ${msg}`,
  };
}

/** Por que o arquivo não vira tabela. */
export function motivo(i: Indisponivel, t: Textos, fmt: Fmt): string {
  const m = t.tabela.indisponivel;
  return i.motivo === "nao_objetos" ? m.nao_objetos(fmt(i.n)) : m[i.motivo];
}

/** Cabeçalho: tipos vistos e presença ("todos" ou "N de M"). */
export function detalhe(c: Coluna, objetos: number, t: Textos, fmt: Fmt): string {
  const tipos = c.tipos.map((x) => t.estrutura.tipos[x] ?? x).join(" | ");
  const pr = c.presenca === objetos ? t.estrutura.todos : t.estrutura.deN(fmt(c.presenca), fmt(objetos));
  return `${tipos} · ${pr}`;
}
