// Tela Valores: árvore virtualizada. O Rust guarda a lista plana de linhas visíveis; aqui só se
// pede a janela que cabe na tela e se desenham essas linhas (o resto não existe no DOM).
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { type Motivo, t } from "./i18n";

type Tipo = "objeto" | "lista" | "texto" | "numero" | "booleano" | "nulo" | "erro";
interface Linha {
  registro: number;
  prof: number;
  chave: string | null;
  pos: number;
  tipo: Tipo;
  previa: string;
  erro: { byte: number; motivo: Motivo } | null;
  filhos: number;
  irmaos: number;
  expansivel: boolean;
  aberto: boolean;
  caminho: string;
  migalhas: string[];
}

const ALTURA = 26; // px por linha; igual ao CSS de .row
const BLOCO = 300; // linhas por pedido ao Rust
// Navegadores limitam a altura de um elemento (~33 milhões de px). Acima disto a barra de
// rolagem passa a ser proporcional em vez de 1 px = 1 px.
const MAX_PX = 8_000_000;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const arvore = $("arvore");
const alto = $("alto");
const linhasEl = $("linhas");
const status = $("status");
const migalhas = $("migalhas");
const copiar = $<HTMLButtonElement>("copiar");
const fmt = (n: number) => n.toLocaleString(t.lang);
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
const icone = (id: string) => `<svg class="i" aria-hidden="true"><use href="#${id}"/></svg>`;

let total = 0; // linhas visíveis
let registros = 0;
let sel = 0;
let versao = 0; // muda a cada expandir/recolher/abrir: janela antiga deixa de valer
let cache: { inicio: number; linhas: Linha[] } | null = null;
let pedido: number | null = null; // início do pedido em voo

/** Geometria da rolagem: primeira linha visível e deslocamento em px dentro dela. */
function geometria() {
  const vista = arvore.clientHeight;
  const totalPx = total * ALTURA;
  const altoPx = Math.min(totalPx, MAX_PX);
  const escala = altoPx > vista ? (totalPx - vista) / (altoPx - vista) : 1;
  const px = arvore.scrollTop * escala;
  const primeira = Math.floor(px / ALTURA);
  return { vista, escala, primeira, resto: px - primeira * ALTURA, cabem: Math.ceil(vista / ALTURA) + 1 };
}

const linhaEm = (v: number): Linha | undefined =>
  cache && v >= cache.inicio ? cache.linhas[v - cache.inicio] : undefined;

async function pedir(primeira: number, cabem: number): Promise<void> {
  const inicio = Math.max(0, primeira - Math.floor((BLOCO - cabem) / 2));
  if (pedido === inicio) return;
  pedido = inicio;
  const v = versao;
  try {
    const linhas = await invoke<Linha[]>("linhas", { inicio, quantidade: BLOCO });
    if (v === versao) cache = { inicio, linhas };
  } catch (erro) {
    status.textContent = `${t.erro}: ${erro}`;
  } finally {
    if (pedido === inicio) pedido = null;
  }
  if (v === versao) desenhar();
}

function cobre(primeira: number, cabem: number): boolean {
  if (!cache) return false;
  const fim = Math.min(primeira + cabem, total);
  return primeira >= cache.inicio && fim <= cache.inicio + cache.linhas.length;
}

const plural = (n: number, [um, varios]: string[]) => `${fmt(n)} ${n === 1 ? um : varios}`;

function valor(l: Linha): string {
  if (l.erro) {
    const msg = `${t.byte(fmt(l.erro.byte))}: ${t.motivos[l.erro.motivo] ?? t.motivos.outro}`;
    return `<span class="err">${icone("alert")}<span>${esc(msg)}</span></span><span class="val dim">${esc(l.previa)}</span>`;
  }
  if (l.tipo === "objeto" || l.tipo === "lista") {
    const n = plural(l.filhos, l.tipo === "objeto" ? t.campos : t.itens);
    const cru = l.prof === 0 && !l.aberto ? ` ${esc(l.previa)}` : "";
    return `<span class="val dim">${n}${cru}</span>`;
  }
  return `<span class="val${l.tipo === "nulo" ? " dim" : ""}">${esc(l.previa)}</span>`;
}

function chave(l: Linha): string {
  if (l.prof === 0) return t.linha(fmt(l.registro + 1));
  return l.chave === null ? String(l.pos) : l.chave === "" ? '""' : l.chave;
}

function linhaHtml(v: number, l: Linha | undefined): string {
  if (!l) return `<div class="row" aria-hidden="true"></div>`;
  const exp = l.expansivel
    ? `<span class="tg" data-tg aria-hidden="true">${icone(l.aberto ? "chev" : "right")}</span>`
    : '<span class="sp"></span>';
  return `<div class="row" id="l${v}" data-v="${v}" role="treeitem" aria-level="${l.prof + 1}"
    aria-selected="${v === sel}"${l.expansivel ? ` aria-expanded="${l.aberto}"` : ""}
    ${l.prof === 0 ? `aria-posinset="${l.registro + 1}" aria-setsize="${registros}"` : `aria-posinset="${l.pos + 1}" aria-setsize="${l.irmaos}"`}
    style="padding-left:${8 + l.prof * 20}px">${exp}<span class="key">${esc(chave(l))}</span>${valor(l)}
    <button class="ib cp" type="button" tabindex="-1" data-cp aria-label="${esc(t.copiarCaminho)} ${esc(l.caminho)}">${icone("copy")}</button></div>`;
}

let quadro = 0;
function desenhar(): void {
  quadro = 0;
  alto.style.height = `${Math.max(0, Math.min(total * ALTURA, MAX_PX) - arvore.clientHeight)}px`;
  linhasEl.style.height = `${arvore.clientHeight}px`;
  const { primeira, resto, cabem } = geometria();
  if (!cobre(primeira, cabem)) void pedir(primeira, cabem);
  const fim = Math.min(primeira + cabem, total);
  let html = "";
  for (let v = primeira; v < fim; v++) html += linhaHtml(v, linhaEm(v));
  linhasEl.innerHTML = html;
  (linhasEl.firstElementChild as HTMLElement | null)?.style.setProperty("margin-top", `${-resto}px`);
  const atual = linhaEm(sel);
  if (document.getElementById(`l${sel}`)) arvore.setAttribute("aria-activedescendant", `l${sel}`);
  else arvore.removeAttribute("aria-activedescendant");
  copiar.disabled = !atual;
  migalhas.innerHTML = atual
    ? [t.linha(fmt(atual.registro + 1)), ...atual.migalhas].map((m) => `<li>${esc(m)}</li>`).join("")
    : "";
}
const agendar = () => (quadro ||= requestAnimationFrame(desenhar));

/** Rola o mínimo para a linha `v` ficar inteira na tela. */
function mostrarLinha(v: number): void {
  const { vista, escala, primeira, cabem } = geometria();
  let px: number | null = null;
  if (v <= primeira) px = v * ALTURA;
  else if (v >= primeira + cabem - 2) px = (v + 1) * ALTURA - vista;
  if (px !== null) arvore.scrollTop = Math.max(0, px / escala);
}

function mover(v: number): void {
  if (!total) return;
  sel = Math.max(0, Math.min(total - 1, v));
  mostrarLinha(sel);
  desenhar();
}

/** Recarrega a janela atual depois de uma mudança na lista, sem piscar linhas vazias. */
async function recarregar(): Promise<void> {
  versao++;
  pedido = null;
  const { primeira, cabem } = geometria();
  const v = versao;
  const inicio = Math.max(0, primeira - Math.floor((BLOCO - cabem) / 2));
  const linhas = await invoke<Linha[]>("linhas", { inicio, quantidade: BLOCO });
  if (v !== versao) return;
  cache = { inicio, linhas };
  desenhar();
}

async function alternar(v: number): Promise<void> {
  total = await invoke<number>("alternar", { v });
  await recarregar();
}

async function aoTeclado(e: KeyboardEvent): Promise<void> {
  const l = linhaEm(sel);
  const pagina = Math.max(1, geometria().cabem - 2);
  const acoes: Record<string, () => unknown> = {
    ArrowDown: () => mover(sel + 1),
    ArrowUp: () => mover(sel - 1),
    PageDown: () => mover(sel + pagina),
    PageUp: () => mover(sel - pagina),
    Home: () => mover(0),
    End: () => mover(total - 1),
    Enter: () => l?.expansivel && alternar(sel),
    ArrowRight: () => (l?.aberto ? mover(sel + 1) : l?.expansivel && alternar(sel)),
    ArrowLeft: async () => {
      if (l?.aberto) return alternar(sel);
      const p = await invoke<number | null>("pai", { v: sel });
      if (p !== null) mover(p);
    },
  };
  const acao = acoes[e.key];
  if (!acao) return;
  e.preventDefault();
  try {
    await acao();
  } catch (erro) {
    status.textContent = `${t.erro}: ${erro}`;
  }
}

async function copiarCaminho(l: Linha, botao: HTMLElement): Promise<void> {
  await navigator.clipboard.writeText(l.caminho);
  const antes = botao.innerHTML;
  botao.innerHTML = botao === copiar ? `${icone("check")}<span>${t.copiado}</span>` : icone("check");
  setTimeout(() => (botao.innerHTML = antes), 1500);
}

async function mostrar(caminho: string): Promise<void> {
  status.textContent = t.abrindo;
  $("nome").textContent = caminho.split(/[\\/]/).pop() ?? caminho;
  try {
    const inicio = performance.now();
    total = registros = await invoke<number>("abrir", { caminho });
    sel = 0;
    arvore.scrollTop = 0;
    await recarregar();
    status.textContent = t.aberto(plural(registros, t.registros), fmt(Math.round(performance.now() - inicio)));
    if (!registros) linhasEl.innerHTML = `<p class="vazio">${t.vazio}</p>`;
    arvore.focus();
  } catch (erro) {
    status.textContent = `${t.erro}: ${erro}`;
  }
}

arvore.addEventListener("scroll", agendar, { passive: true });
new ResizeObserver(agendar).observe(arvore);
arvore.addEventListener("keydown", (e) => void aoTeclado(e));
linhasEl.addEventListener("click", (e) => {
  const alvo = e.target as HTMLElement;
  const row = alvo.closest<HTMLElement>(".row[data-v]");
  if (!row) return;
  const v = Number(row.dataset.v);
  const l = linhaEm(v);
  sel = v;
  if (alvo.closest("[data-cp]") && l) void copiarCaminho(l, alvo.closest("[data-cp]")!);
  else if (alvo.closest("[data-tg]")) void alternar(v);
  else desenhar();
  arvore.focus({ preventScroll: true });
});
linhasEl.addEventListener("dblclick", (e) => {
  const row = (e.target as HTMLElement).closest<HTMLElement>(".row[data-v]");
  if (row && !(e.target as HTMLElement).closest("[data-tg],[data-cp]") && linhaEm(Number(row.dataset.v))?.expansivel)
    void alternar(Number(row.dataset.v));
});
copiar.addEventListener("click", () => {
  const l = linhaEm(sel);
  if (l) void copiarCaminho(l, copiar);
});
$("ir").addEventListener("submit", async (e) => {
  e.preventDefault();
  const n = Number(($("linha") as HTMLInputElement).value);
  const v = Number.isInteger(n) && n >= 1 ? await invoke<number | null>("ir_para", { registro: n - 1 }) : null;
  if (v === null) {
    status.textContent = t.foraDoArquivo(String(n), fmt(registros));
    return;
  }
  mover(v);
  arvore.focus();
});
// Textos fixos do HTML no idioma do sistema.
document.documentElement.lang = t.lang;
$("abrir").textContent = t.abrir;
$("aba").textContent = t.valores;
$("rotulo-linha").textContent = t.irParaLinha;
copiar.querySelector("span")!.textContent = t.copiarCaminho;
migalhas.setAttribute("aria-label", t.caminhoSelecionado);
$("tema").setAttribute("aria-label", t.tema);
arvore.setAttribute("aria-label", t.valoresDoArquivo);

$("tema").addEventListener("click", () => {
  const raiz = document.documentElement;
  raiz.dataset.theme = raiz.dataset.theme === "dark" ? "light" : "dark";
});
if (matchMedia("(prefers-color-scheme: dark)").matches) document.documentElement.dataset.theme = "dark";

$("abrir").addEventListener("click", async () => {
  const caminho = await open({ multiple: false, filters: [{ name: "JSON Lines", extensions: ["jsonl", "ndjson", "json"] }] });
  if (typeof caminho === "string") await mostrar(caminho);
});

invoke<string | null>("arquivo_inicial").then((caminho) => {
  if (caminho) void mostrar(caminho);
});
