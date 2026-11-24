// Modo Tabela, parte com DOM: grade virtual com cabeçalho fixo. As linhas vêm do Rust em
// janelas (`tabela_linhas`, só as colunas visíveis); o arquivo nunca vem inteiro para cá.
// Ordenar e filtrar rodam no Rust em 2º plano (`ordenar`/`filtrar` + eventos `ordenacao`/`filtragem`).
// Estado por documento.
import { chamar, docAtivo } from "./doc.ts";
import { t } from "./i18n.ts";
import { espaco, geometria, mostrarLinha } from "./rolagem.ts";
import {
  type Alvo, type Coluna, type Concluida, type EstadoFiltro, type EstadoOrdem, type Filtragem, type LinhaT, type Ordem, type Ordenacao, type TabelaUi,
  celula, chegou, chipFiltro, detalhe, filtroChegou, filtroRespondeu, invalida, largura, linhasNaGrade, motivo, ordemVisivel, proximaOrdem, respondeu, rotulos,
} from "./tabela.ts";
import { visivel } from "./texto.ts";

const ALTURA = 26; // px por linha; igual ao CSS de .grade .td
const CAB = 44; // px do cabeçalho; igual ao CSS de .grade .thead .tr
const CELULAS = 30_000; // células por pedido: com muitas colunas o bloco de linhas encolhe

export type { Alvo, Filtragem, Ordenacao };

interface Estado extends EstadoOrdem, EstadoFiltro {
  de: number; // geração da varredura: outra = documento reaberto, estado novo
  info: TabelaUi | null;
  pedindo: boolean;
  fixas: number[];
  ocultas: Set<number>;
  aviso: string; // fim da última ordenação (falha ou ordem nova)
  lin: number; // célula selecionada; -1 = cabeçalho
  col: number; // posição na ordem visível
  topo: number;
  esquerda: number;
  filtros: number; // quantas vezes o filtro trocou: entra na chave do cache
  pos: number; // posição no filtro da navegação ⌘↓/⌘↑ na Árvore; -1 = ainda não andou
  alvo: Alvo | null; // o filtro aplicado (para o "Salvar como")
}

interface Deps {
  geracao: () => number | undefined; // da varredura do documento ativo
  inferiu: () => boolean; // a Tabela já foi montada (ou recusada) no Rust
  revelar: (linha: number, byte: number) => void;
  mudou: () => void; // seleção ou disponibilidade mudou: painel e rodapé
  erro: (e: unknown) => void;
  fmt: (n: number) => string;
}

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
const icone = (id: string) => `<svg class="i" aria-hidden="true"><use href="#${id}"/></svg>`;

export function criarGrade(el: HTMLElement, deps: Deps) {
  const linhasEl = el.querySelector<HTMLElement>(".rows")!;
  const alto = el.querySelector<HTMLElement>(".alto")!;
  const resumoEl = document.getElementById("tabela-resumo")!;
  const ocultasEl = document.getElementById("tabela-ocultas")!;
  const ordEl = document.getElementById("tabela-ordem")!;
  const barraEl = ordEl.querySelector("progress")!;
  const ordTexto = ordEl.querySelector("span")!;
  const cancelar = ordEl.querySelector("button")!;
  const filtroEl = document.getElementById("tabela-filtro")!;
  const fmt = deps.fmt;
  const docs = new Map<number, Estado>();
  // Só o corpo rola sob o cabeçalho: as contas de rolagem usam a altura sem ele.
  const vp = {
    get clientHeight() { return Math.max(ALTURA, el.clientHeight - CAB); },
    get scrollTop() { return el.scrollTop; },
    set scrollTop(v: number) { el.scrollTop = v; },
  } as unknown as HTMLElement;
  let cache: { chave: string; inicio: number; linhas: LinhaT[] } | null = null;
  let pedido: string | null = null;
  let quadro = 0;

  function estado(): Estado | null {
    const d = docAtivo();
    const g = deps.geracao();
    if (d === null || g === undefined) return null;
    let e = docs.get(d);
    if (!e || e.de !== g) {
      e = { de: g, info: null, pedindo: false, fixas: [], ocultas: new Set(), ordem: null, pedida: null, fim: null, aviso: "", lin: 0, col: 0, topo: 0, esquerda: 0,
        filtrando: null, fimF: null, filtros: 0, pos: -1, alvo: null };
      docs.set(d, e);
    }
    return e;
  }
  const visiveis = (e: Estado) => ordemVisivel(e.info?.colunas.length ?? 0, e.fixas, e.ocultas);
  const pronta = (e: Estado | null): e is Estado & { info: TabelaUi } => !!e?.info && !e.info.indisponivel;

  /** Pede as colunas ao Rust quando a inferência acabou (uma vez por documento). */
  async function carregar(): Promise<void> {
    const e = estado();
    if (!e || e.info || e.pedindo || !deps.inferiu()) return;
    const d = docAtivo();
    e.pedindo = true;
    try {
      const r = await chamar<TabelaUi | null>("tabela");
      if (!r || docs.get(d!) !== e) return;
      e.info = r;
      e.ordem = r.ordem;
    } catch (erro) {
      deps.erro(erro);
    } finally {
      e.pedindo = false;
    }
    if (docAtivo() !== d) return;
    deps.mudou();
    barra();
    agendar();
  }

  const chaveDe = (e: Estado, vis: number[]) => `${docAtivo()}:${e.de}:${e.filtros}:${e.ordem?.coluna}:${e.ordem?.desc}:${vis.join(",")}`;

  /** O filtro trocou no Rust (e a ordem foi junto): colunas e contagem de novo, do topo. Se havia
   * ordem, pede a mesma sobre o filtro novo. */
  async function recarregar(e: Estado): Promise<void> {
    const ordem = e.pedida?.ordem ?? e.ordem;
    Object.assign(e, { info: null, ordem: null, pedida: null, fim: null, lin: 0, topo: 0, pos: -1 });
    e.filtros++;
    cache = null;
    if (docs.get(docAtivo()!) === e) el.scrollTop = 0;
    await carregar();
    if (ordem && pronta(e) && estado() === e) await aplicarOrdem(e, ordem);
  }

  async function pedir(chave: string, vis: number[], primeira: number, cabem: number): Promise<void> {
    const q = Math.max(cabem * 2, Math.min(300, Math.floor(CELULAS / Math.max(1, vis.length))));
    const inicio = Math.max(0, primeira - Math.floor((q - cabem) / 2));
    const k = `${chave}@${inicio}`;
    if (pedido === k) return;
    pedido = k;
    try {
      const linhas = await chamar<LinhaT[]>("tabela_linhas", { inicio, quantidade: q, colunas: vis });
      const e = estado();
      if (e && chaveDe(e, visiveis(e)) === chave) cache = { chave, inicio, linhas };
    } catch (erro) {
      deps.erro(erro);
      return; // redesenhar pediria de novo, em laço
    } finally {
      if (pedido === k) pedido = null;
    }
    agendar();
  }

  function cabecalho(e: Estado & { info: TabelaUi }, vis: number[], esquerdas: number[], nFix: number): string {
    let html = `<div class="th n" role="columnheader" aria-colindex="1">#</div>`;
    vis.forEach((c, j) => {
      const col = e.info.colunas[c];
      const o = e.ordem?.coluna === c ? e.ordem : null;
      const fix = j < nFix;
      const sort = o ? ` aria-sort="${o.desc ? "descending" : "ascending"}"` : "";
      const seta = o ? `<span class="seta" aria-hidden="true">${o.desc ? "↓" : "↑"}</span>` : "";
      html += `<div class="th${fix ? " fix" : ""}${fix && j === nFix - 1 ? " ult" : ""}${e.lin === -1 && e.col === j ? " sel" : ""}"
        role="columnheader" id="h${j}" data-j="${j}" aria-colindex="${j + 2}"${sort}${fix ? ` style="left:${esquerdas[j]}px"` : ""}
        title="${esc(rotulos.titulo(col))}"><span class="nm">${visivel(col.nome)}${seta}</span><span class="dt">${esc(detalhe(col, e.info.objetos, t, fmt))}</span>
        <span class="acoes"><button class="ib" type="button" tabindex="-1" data-fixar="${c}" aria-pressed="${fix}"
        aria-label="${esc(fix ? t.tabela.soltar : t.tabela.fixar)}" title="${esc(fix ? t.tabela.soltar : t.tabela.fixar)}">${icone("pin")}</button><button
        class="ib" type="button" tabindex="-1" data-ocultar="${c}" aria-label="${esc(t.tabela.ocultar)}" title="${esc(t.tabela.ocultar)}">${icone("x")}</button></span></div>`;
    });
    return `<div class="thead" role="rowgroup"><div class="tr" role="row" aria-rowindex="1">${html}</div></div>`;
  }

  function linha(e: Estado, v: number, l: LinhaT | undefined, vis: number[], esquerdas: number[], nFix: number): string {
    const ri = `aria-rowindex="${v + 2}"`;
    if (!l) return `<div class="tr" role="row" ${ri}><div class="td n" role="rowheader" aria-colindex="1"></div></div>`;
    let html = `<div class="td n" role="rowheader" aria-colindex="1">${fmt(l.registro + 1)}</div>`;
    if (l.erro) {
      const c = invalida(l, t, fmt);
      const s = e.lin === v ? " sel" : "";
      html += `<div class="td erro${s}" role="gridcell" id="c${v}-0" data-j="0" aria-colindex="2" aria-colspan="${vis.length}"
        aria-label="${esc(c.rotulo)}">${c.html}</div>`;
    } else
      vis.forEach((_, j) => {
        const c = celula(l.celulas[j], t, fmt);
        const fix = j < nFix;
        const cl = `td${fix ? " fix" : ""}${fix && j === nFix - 1 ? " ult" : ""}${e.lin === v && e.col === j ? " sel" : ""}`;
        const rot = l.celulas[j]?.tipo === "texto" || l.celulas[j]?.tipo === "numero" ? "" : ` aria-label="${esc(c.rotulo)}"`;
        html += `<div class="${cl}" role="gridcell" id="c${v}-${j}" data-j="${j}" aria-colindex="${j + 2}"${rot}${fix ? ` style="left:${esquerdas[j]}px"` : ""}>${c.html}</div>`;
      });
    return `<div class="tr" role="row" ${ri} data-v="${v}">${html}</div>`;
  }

  function desenhar(): void {
    quadro = 0;
    const e = estado();
    if (!pronta(e)) {
      const msg = e?.info?.indisponivel ? motivo(e.info.indisponivel, t, fmt) : t.tabela.inferindo;
      linhasEl.innerHTML = `<p class="vazio">${esc(msg)}</p>`;
      linhasEl.style.height = alto.style.height = "";
      el.removeAttribute("aria-activedescendant");
      el.removeAttribute("aria-rowcount");
      return;
    }
    const n = linhasNaGrade(e.info);
    const vis = visiveis(e);
    const larguras = vis.map((c) => largura(e.info.colunas[c]));
    const nLarg = Math.round(String(n).length * 7.6 + 20);
    const nFix = e.fixas.filter((c) => !e.ocultas.has(c)).length;
    const esquerdas: number[] = [];
    larguras.reduce((x, w, j) => ((esquerdas[j] = x), x + w), nLarg);
    linhasEl.style.setProperty("--cols", `${nLarg}px ${larguras.map((w) => `${w}px`).join(" ")}`);
    el.setAttribute("aria-rowcount", String(n + 1));
    el.setAttribute("aria-colcount", String(vis.length + 1));
    alto.style.height = espaco(vp, n, ALTURA);
    linhasEl.style.height = `${el.clientHeight}px`;
    const g = geometria(vp, n, ALTURA);
    const { primeira, resto, cabem } = g;
    const fim = Math.min(primeira + cabem, n);
    const chave = chaveDe(e, vis);
    const c = cache?.chave === chave ? cache : null;
    if (n && (!c || primeira < c.inicio || fim > c.inicio + c.linhas.length)) void pedir(chave, vis, primeira, cabem);
    let corpo = "";
    for (let v = primeira; v < fim; v++) corpo += linha(e, v, c && v >= c.inicio ? c.linhas[v - c.inicio] : undefined, vis, esquerdas, nFix);
    linhasEl.innerHTML = cabecalho(e, vis, esquerdas, nFix) +
      `<div class="tbody" role="rowgroup">${corpo || `<p class="vazio">${esc(t.vazio)}</p>`}</div>`;
    (linhasEl.querySelector(".tbody")?.firstElementChild as HTMLElement | null)?.style.setProperty("margin-top", `${-resto}px`);
    const id = e.lin === -1 ? `h${e.col}` : document.getElementById(`c${e.lin}-${e.col}`) ? `c${e.lin}-${e.col}` : `c${e.lin}-0`;
    if (document.getElementById(id)) el.setAttribute("aria-activedescendant", id);
    else el.removeAttribute("aria-activedescendant");
  }
  const agendar = () => void (quadro ||= requestAnimationFrame(desenhar));

  /** Barra acima da grade: contagem, colunas omitidas e ocultas, ordenação em curso. */
  function barra(): void {
    const e = estado();
    if (!pronta(e)) {
      resumoEl.textContent = "";
      ocultasEl.innerHTML = "";
      ordEl.hidden = true;
      filtroEl.innerHTML = chipFiltro(e ?? { filtrando: null, fimF: null }, null, t, fmt);
      return;
    }
    const i = e.info;
    filtroEl.innerHTML = chipFiltro(e, i, t, fmt);
    resumoEl.textContent = t.tabela.resumo(fmt(i.colunas.length), fmt(i.registros)) + (i.omitidas ? ` · ${t.tabela.omitidas(fmt(i.omitidas))}` : "");
    const oc = [...e.ocultas].sort((a, b) => a - b);
    ocultasEl.innerHTML = oc.length
      ? `<span>${esc(t.tabela.ocultas)}</span>` + oc.map((c) => {
        const col = i.colunas[c];
        return `<button class="chip" type="button" data-mostrar="${c}" aria-label="${esc(rotulos.mostrar(col, t))}">${visivel(col.nome)}${icone("plus")}</button>`;
      }).join("")
      : "";
    const p = e.pedida;
    ordEl.hidden = !p && !e.aviso;
    barraEl.hidden = cancelar.hidden = !p;
    if (p) {
      const pct = Math.floor((p.lidos * 100) / Math.max(1, p.total));
      barraEl.value = pct;
      ordTexto.textContent = rotulos.ordenando(i.colunas[p.ordem.coluna], pct, t);
    } else ordTexto.textContent = e.aviso;
  }

  function selecionar(e: Estado & { info: TabelaUi }, lin: number, col: number): void {
    const n = linhasNaGrade(e.info);
    const vis = visiveis(e);
    e.lin = Math.max(-1, Math.min(n - 1, lin));
    e.col = Math.max(0, Math.min(vis.length - 1, col));
    if (e.lin >= 0) mostrarLinha(vp, e.lin, n, ALTURA);
    // Na horizontal: a coluna inteira à vista, sem ficar sob as fixadas.
    const nFix = e.fixas.filter((c) => !e.ocultas.has(c)).length;
    if (e.col >= nFix) {
      const larg = vis.map((c) => largura(e.info.colunas[c]));
      const nLarg = Math.round(String(n).length * 7.6 + 20);
      const fixo = nLarg + larg.slice(0, nFix).reduce((a, b) => a + b, 0);
      const x = nLarg + larg.slice(0, e.col).reduce((a, b) => a + b, 0);
      if (x - fixo < el.scrollLeft) el.scrollLeft = x - fixo;
      else if (x + larg[e.col] > el.scrollLeft + el.clientWidth) el.scrollLeft = x + larg[e.col] - el.clientWidth;
    }
    desenhar();
    deps.mudou();
  }

  async function ordenar(c: number): Promise<void> {
    const e = estado();
    if (!pronta(e)) return;
    await aplicarOrdem(e, proximaOrdem(e.pedida?.ordem ?? e.ordem, c));
  }

  async function aplicarOrdem(e: Estado & { info: TabelaUi }, alvo: Ordem | null): Promise<void> {
    const r = await chamar<{ geracao: number; pronta: boolean }>("ordenar", { coluna: alvo?.coluna ?? null, desc: alvo?.desc ?? false });
    if (estado() !== e) return;
    e.aviso = "";
    // O evento de fim pode ter chegado antes desta resposta: `respondeu` o aplica.
    avisar(e, respondeu(e, alvo, r, linhasNaGrade(e.info)));
    barra();
    desenhar();
  }

  function avisar(e: Estado, f: Concluida | null): void {
    if (!f || !e.info) return;
    e.aviso = f.falha ? (t.tabela.falha[f.falha] ?? f.falha) : rotulos.ordenada(e.info.colunas[f.ordem.coluna], f.ordem.desc, t);
  }

  function fixar(c: number): void {
    const e = estado();
    if (!pronta(e)) return;
    const vis = visiveis(e);
    const atual = vis[e.col];
    e.fixas = e.fixas.includes(c) ? e.fixas.filter((x) => x !== c) : [...e.fixas, c];
    const nova = visiveis(e).indexOf(atual); // a seleção segue a mesma coluna
    selecionar(e, e.lin, nova < 0 ? 0 : nova);
  }

  function ocultar(c: number, mostrar = false): void {
    const e = estado();
    if (!pronta(e)) return;
    if (mostrar) e.ocultas.delete(c);
    else if (visiveis(e).length > 1) e.ocultas.add(c); // a última coluna fica
    barra();
    selecionar(e, e.lin, e.col);
  }

  function abrir(e: Estado, v: number, j: number): void {
    const l = cache && v >= cache.inicio ? cache.linhas[v - cache.inicio] : undefined;
    if (!l) return;
    const c = l.erro ? null : l.celulas[j];
    deps.revelar(l.linha, c ? c.byte : l.byte);
    void e;
  }

  function aoTeclado(ev: KeyboardEvent): void {
    const e = estado();
    if (!pronta(e)) return;
    const vis = visiveis(e);
    const pagina = Math.max(1, Math.floor(vp.clientHeight / ALTURA) - 1);
    const cmd = ev.metaKey || ev.ctrlKey;
    const { lin, col } = e;
    const acoes: Record<string, () => unknown> = {
      ArrowDown: () => selecionar(e, lin + 1, col),
      ArrowUp: () => selecionar(e, lin - 1, col),
      ArrowRight: () => selecionar(e, lin, col + 1),
      ArrowLeft: () => selecionar(e, lin, col - 1),
      PageDown: () => selecionar(e, Math.max(0, lin) + pagina, col),
      PageUp: () => selecionar(e, lin - pagina, col),
      Home: () => selecionar(e, cmd ? -1 : lin, 0),
      End: () => selecionar(e, cmd ? linhasNaGrade(e.info) - 1 : lin, vis.length - 1),
      Enter: () => (lin === -1 ? ordenar(vis[col]) : abrir(e, lin, col)),
    };
    if (lin === -1) {
      acoes.f = () => fixar(vis[col]);
      acoes.Delete = acoes.Backspace = () => ocultar(vis[col]);
    }
    const acao = acoes[ev.key];
    if (!acao || ev.altKey || (cmd && !["Home", "End"].includes(ev.key))) return;
    ev.preventDefault();
    Promise.resolve(acao()).catch(deps.erro);
  }

  el.addEventListener("keydown", aoTeclado);
  el.addEventListener("scroll", () => {
    const e = estado();
    if (e) [e.topo, e.esquerda] = [el.scrollTop, el.scrollLeft];
    agendar();
  }, { passive: true });
  new ResizeObserver(agendar).observe(el);
  linhasEl.addEventListener("click", (ev) => {
    const alvo = ev.target as HTMLElement;
    const e = estado();
    if (!pronta(e)) return;
    const fx = alvo.closest<HTMLElement>("[data-fixar]")?.dataset.fixar;
    const oc = alvo.closest<HTMLElement>("[data-ocultar]")?.dataset.ocultar;
    const cel = alvo.closest<HTMLElement>("[data-j]");
    const v = alvo.closest<HTMLElement>("[data-v]")?.dataset.v;
    el.focus({ preventScroll: true });
    if (fx !== undefined) return fixar(Number(fx));
    if (oc !== undefined) return ocultar(Number(oc));
    if (!cel) return;
    const j = Number(cel.dataset.j);
    if (v === undefined) {
      selecionar(e, -1, j);
      void ordenar(visiveis(e)[j]).catch(deps.erro);
    } else {
      selecionar(e, Number(v), j);
      if (alvo.closest("[data-abrir]")) abrir(e, Number(v), j);
    }
  });
  linhasEl.addEventListener("dblclick", (ev) => {
    const alvo = ev.target as HTMLElement;
    const v = alvo.closest<HTMLElement>("[data-v]")?.dataset.v;
    const j = alvo.closest<HTMLElement>("[data-j]")?.dataset.j;
    const e = estado();
    if (e && v !== undefined && j !== undefined && !alvo.closest("[data-abrir]")) abrir(e, Number(v), Number(j));
  });
  ocultasEl.addEventListener("click", (ev) => {
    const c = (ev.target as HTMLElement).closest<HTMLElement>("[data-mostrar]")?.dataset.mostrar;
    if (c !== undefined) ocultar(Number(c), true);
  });
  cancelar.addEventListener("click", () => void chamar("cancelar_ordenacao").catch(deps.erro));
  filtroEl.addEventListener("click", (ev) => {
    const alvo = ev.target as HTMLElement;
    if (alvo.closest("[data-cancelar-filtro]")) void chamar("cancelar_filtro").catch(deps.erro);
    else if (alvo.closest("[data-limpar-filtro]")) void limpar().catch(deps.erro);
  });

  /** Fim de um filtro: aviso na barra e, se deu certo, a grade com os achados. Devolve o aviso
   * (a Árvore mostra no rodapé). */
  function filtrou(e: Estado, f: Filtragem, rotulo: string): string {
    const tf = t.tabela.filtro;
    e.aviso = f.falha ? (tf.falha[f.falha] ?? f.falha) : "";
    if (!f.falha) void recarregar(e).catch(deps.erro);
    return e.aviso || tf.filtrados(rotulo, fmt(f.achados));
  }

  async function limpar(): Promise<void> {
    const e = estado();
    if (!e) return;
    await chamar("limpar_filtro");
    if (estado() !== e) return;
    e.filtrando = null;
    e.alvo = null;
    e.aviso = "";
    await recarregar(e);
  }

  return {
    /** Modo Tabela na tela: pede as colunas se preciso e volta à rolagem guardada. */
    mostrar(): void {
      const e = estado();
      void carregar();
      barra();
      if (e) [el.scrollTop, el.scrollLeft] = [e.topo, e.esquerda];
      desenhar();
    },
    /** A inferência do documento ativo acabou: as colunas já podem vir (o painel quer saber). */
    inferiu: () => void carregar(),
    /** `null`: ainda não se sabe (inferindo). */
    disponivel(): boolean | null {
      const i = estado()?.info;
      return i ? !i.indisponivel : null;
    },
    motivo(): string | null {
      const i = estado()?.info?.indisponivel;
      return i ? motivo(i, t, fmt) : null;
    },
    /** Caminho jq da coluna selecionada (rodapé e "Copiar caminho"). */
    caminho(): string | null {
      const e = estado();
      if (!pronta(e)) return null;
      const c: Coluna | undefined = e.info.colunas[visiveis(e)[e.col]];
      return c?.caminho ?? null;
    },
    ordenacao(p: Ordenacao): void {
      const e = docs.get(p.id);
      if (!e?.info) return;
      avisar(e, chegou(e, p));
      if (p.id !== docAtivo()) return;
      barra();
      if (p.fim) desenhar();
    },
    /** Filtra os registros (em 2º plano). Devolve o aviso se já acabou; `null` se segue rodando
     * ou se a inferência ainda não acabou. */
    async filtrar(alvo: Alvo, rotulo: string): Promise<string | null> {
      const e = estado();
      if (!e) return null;
      const g = await chamar<number | null>("filtrar", { alvo });
      if (g === null || estado() !== e) return null;
      e.aviso = "";
      e.alvo = alvo;
      const f = filtroRespondeu(e, g, rotulo);
      const msg = f ? filtrou(e, f, rotulo) : null;
      barra();
      return msg;
    },
    /** Evento `filtragem`: o aviso quando acaba (para a Árvore), `null` enquanto roda. */
    filtragem(p: Filtragem): string | null {
      const e = docs.get(p.id);
      if (!e) return null;
      const rotulo = e.filtrando?.rotulo ?? "";
      const f = filtroChegou(e, p);
      const msg = f ? filtrou(e, f, rotulo) : null;
      if (p.id === docAtivo()) barra();
      return msg;
    },
    limpar,
    /** O filtro aplicado no documento ativo, se houver. */
    alvo: (): Alvo | null => estado()?.alvo ?? null,
    /** ⌘↓/⌘↑ na Árvore: a próxima posição no filtro e quantos há; `null` sem filtro. */
    vizinho(passo: 1 | -1): { posicao: number; total: number } | null {
      const e = estado();
      if (e && !e.info) void carregar(); // filtro que acabou com outro documento na tela
      const total = e?.info?.filtro?.achados ?? 0;
      if (!e || !total) return null;
      e.pos = e.pos < 0 ? (passo > 0 ? 0 : total - 1) : Math.max(0, Math.min(total - 1, e.pos + passo));
      return { posicao: e.pos, total };
    },
    esquecer: (doc: number) => void docs.delete(doc),
    agendar,
  };
}
