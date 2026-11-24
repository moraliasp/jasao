// Modo Tabela, parte pura: formata o que `nucleo::tabela` devolve (comandos `tabela` e
// `tabela_linhas`). Sem DOM, para rodar com `node --test`; a grade virtual fica em `grade.ts`.
// Todo texto que vem do arquivo (nome de coluna, valor, linha inválida) passa por `visivel`.
import { faixa } from "./estrutura.ts";
import type { Motivo, Textos } from "./i18n.ts";
import { visivel, visivelTexto } from "./texto.ts";

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
  | { motivo: "nao_objetos"; n: number }
  | { motivo: "grande"; max: number };
export interface Ordem { coluna: number; desc: boolean }
export interface TabelaUi {
  registros: number;
  objetos: number;
  colunas: Coluna[];
  omitidas: number;
  indisponivel: Indisponivel | null;
  ordem: Ordem | null;
  /** Filtro pronto: a grade só tem os `achados`. */
  filtro: FiltroUi | null;
}
/** `comum`: token cru de um valor dos mais comuns do perfil; `faixa`: [negativo, expoente]. */
export interface FiltroUi {
  achados: number; caminho: string; tem: boolean | null; valor: string | null;
  comum?: string | null; faixa?: [boolean, number | null] | null;
}
/** O que filtrar (posições em `estrutura().caminhos` / `discriminadores`); no caminho, também um
 * valor dos mais comuns (`comum`, inteiro) ou uma faixa de número do perfil. */
export type Alvo =
  | { caminho: number; tem: boolean }
  | { discriminador: number; variante: number }
  | { caminho: number; comum: string }
  | { caminho: number; negativo: boolean; expoente: number | null };

type Fmt = (n: number) => string;
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");

/** Evento `ordenacao` do Rust. */
export interface Ordenacao { id: number; geracao: number; lidos: number; total: number; fim: boolean; falha: string | null }
/** Ordem da grade: a que vale, a que roda e o último fim recebido (pode chegar antes da resposta
 * de `ordenar`, que só então diz a geração). */
export interface EstadoOrdem {
  ordem: Ordem | null;
  pedida: { geracao: number; ordem: Ordem; lidos: number; total: number } | null;
  fim: Ordenacao | null;
}
export interface Concluida { ordem: Ordem; falha: string | null }

/** Evento `ordenacao`: progresso da pedida, ou o fim dela (ordem nova ou falha). */
export function chegou(e: EstadoOrdem, p: Ordenacao): Concluida | null {
  if (p.fim) e.fim = p;
  if (e.pedida?.geracao !== p.geracao) return null;
  if (!p.fim) {
    e.pedida.lidos = p.lidos;
    e.pedida.total = p.total;
    return null;
  }
  const ordem = e.pedida.ordem;
  e.pedida = null;
  e.fim = null;
  if (!p.falha) e.ordem = ordem;
  return { ordem, falha: p.falha };
}

/** Resposta de `ordenar`. Se o fim desta geração já chegou, conclui agora. */
export function respondeu(e: EstadoOrdem, alvo: Ordem | null, r: { geracao: number; pronta: boolean }, total: number): Concluida | null {
  if (r.pronta || !alvo) {
    e.ordem = alvo;
    e.pedida = null;
    return null;
  }
  e.pedida = { geracao: r.geracao, ordem: alvo, lidos: 0, total };
  return e.fim?.geracao === r.geracao ? chegou(e, e.fim) : null;
}

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
  return { html: `<span class="v ${c.tipo}">${visivel(c.texto)}</span>`, rotulo: visivelTexto(c.texto) };
}

/** Textos com nome ou caminho de coluna que vão para atributo ou `textContent`. */
export const rotulos = {
  titulo: (c: Coluna) => visivelTexto(c.caminho),
  mostrar: (c: Coluna, t: Textos) => t.tabela.mostrar(visivelTexto(c.nome)),
  ordenando: (c: Coluna, pct: number, t: Textos) => t.tabela.ordenando(visivelTexto(c.caminho), String(pct)),
  ordenada: (c: Coluna, desc: boolean, t: Textos) => t.tabela.ordenada(visivelTexto(c.caminho), desc),
};

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
  if (i.motivo === "nao_objetos") return m.nao_objetos(fmt(i.n));
  if (i.motivo === "grande") return m.grande(fmt(i.max));
  return m[i.motivo];
}

/** Cabeçalho: tipos vistos e presença ("todos" ou "N de M"). */
export function detalhe(c: Coluna, objetos: number, t: Textos, fmt: Fmt): string {
  const tipos = c.tipos.map((x) => t.estrutura.tipos[x] ?? x).join(" | ");
  const pr = c.presenca === objetos ? t.estrutura.todos : t.estrutura.deN(fmt(c.presenca), fmt(objetos));
  return `${tipos} · ${pr}`;
}

/** Evento `filtragem` do Rust. */
export interface Filtragem { id: number; geracao: number; lidos: number; total: number; fim: boolean; achados: number; falha: string | null }
/** Filtro em curso e o último fim recebido (pode chegar antes da resposta de `filtrar`). */
export interface EstadoFiltro {
  filtrando: { geracao: number; rotulo: string; lidos: number; total: number } | null;
  fimF: Filtragem | null;
}

/** Evento `filtragem`: progresso do pedido, ou o fim dele (devolvido). */
export function filtroChegou(e: EstadoFiltro, p: Filtragem): Filtragem | null {
  if (p.fim) e.fimF = p;
  if (e.filtrando?.geracao !== p.geracao) return null;
  if (!p.fim) {
    e.filtrando.lidos = p.lidos;
    e.filtrando.total = p.total;
    return null;
  }
  e.filtrando = null;
  e.fimF = null;
  return p;
}

/** Resposta de `filtrar` (a geração). Se o fim dela já chegou, conclui agora. */
export function filtroRespondeu(e: EstadoFiltro, geracao: number, rotulo: string): Filtragem | null {
  e.filtrando = { geracao, rotulo, lidos: 0, total: 0 };
  return e.fimF?.geracao === geracao ? filtroChegou(e, e.fimF) : null;
}

/** `.tipo = "x"`, `com .a.b`, `sem .a.b`, `.n = 1.5`, `.n em 1e2…1e3`: texto puro (vai para
 * `textContent` ou por `esc`). */
export function rotuloFiltro(f: Omit<FiltroUi, "achados">, t: Textos): string {
  const tf = t.tabela.filtro;
  const c = visivelTexto(f.caminho);
  if (f.comum != null) return tf.comum(c, visivelTexto(f.comum));
  if (f.faixa) return tf.faixa(c, faixa({ negativo: f.faixa[0], expoente: f.faixa[1] }));
  if (f.valor !== null) return tf.igual(c, visivelTexto(f.valor));
  return f.tem === false ? tf.sem(c) : tf.com(c);
}

/** Chip do filtro pronto (com o botão de tirar) ou o progresso do que roda, como HTML. */
export function chipFiltro(e: EstadoFiltro, info: TabelaUi | null, t: Textos, fmt: Fmt): string {
  const tf = t.tabela.filtro;
  const p = e.filtrando;
  if (p) {
    const pct = p.total ? Math.floor((p.lidos * 100) / p.total) : 0;
    return `<progress max="100" value="${pct}"></progress><span>${esc(tf.filtrando(p.rotulo, String(pct)))}</span>` +
      `<button class="btn" type="button" data-cancelar-filtro>${esc(t.tabela.cancelar)}</button>`;
  }
  const f = info?.filtro;
  if (!f) return "";
  const rot = rotuloFiltro(f, t);
  return `<span class="chip on" title="${esc(rot)}"><span class="tx">${esc(rot)} · ${esc(tf.achados(fmt(f.achados), fmt(info.registros)))}</span>` +
    `<button class="ib" type="button" data-limpar-filtro aria-label="${esc(tf.limpar)}" title="${esc(tf.limpar)}">` +
    `<svg class="i" aria-hidden="true"><use href="#x"/></svg></button></span>`;
}

/** Registros na grade: os filtrados, ou todos. */
export const linhasNaGrade = (i: TabelaUi) => i.filtro?.achados ?? i.registros;
