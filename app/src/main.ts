// Tela Valores: Árvore e Texto sobre a mesma seleção. O Rust guarda o arquivo e a lista plana
// de linhas visíveis; aqui só se pede a janela que cabe na tela (o resto não existe no DOM).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { criarBusca } from "./busca";
import { type Motivo, t } from "./i18n";
import { criarRegua, type Legenda } from "./regua";
import { espaco, geometria as geo, inteiras, mostrarLinha as mostrarEm, rolarPara } from "./rolagem";
import { criarConsulta } from "./consulta";
import { copiar as copiarNo, criarMenu, FORMATOS, type Formato } from "./copiar";
import { decodificar, type JanelaTexto, marcado, resumir, linhaHtml as textoHtml } from "./texto";

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
  faixa: [number, number];
}

const ALTURA = 26; // px por linha; igual ao CSS de .row
const ALTURA_TEXTO = 20; // px por linha do Texto; igual ao CSS de .tl
const BLOCO = 300; // linhas por pedido ao Rust

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const arvore = $("arvore");
const alto = $("alto");
const linhasEl = $("linhas");
const status = $("status");
const migalhas = $("migalhas");
const copiar = $<HTMLButtonElement>("copiar");
const textoEl = $("texto");
const textoAlto = $("texto-alto");
const textoLinhas = $("texto-linhas");
const verFormatado = $<HTMLButtonElement>("ver-formatado");
const abas = { arvore: $("aba-arvore"), texto: $("aba-texto") };
type Modo = keyof typeof abas;
const fmt = (n: number) => n.toLocaleString(t.lang);
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
const icone = (id: string) => `<svg class="i" aria-hidden="true"><use href="#${id}"/></svg>`;

let total = 0; // linhas visíveis
let registros = 0;
let sel = 0;
let versao = 0; // muda a cada expandir/recolher/abrir: janela antiga deixa de valer
let cache: { inicio: number; linhas: Linha[] } | null = null;
let pedido: number | null = null; // início do pedido em voo
let modo: Modo = "arvore";
let atual: Linha | undefined; // linha da seleção; Texto e rodapé leem daqui
let texto: JanelaTexto | null = null;
let pedidoTexto: number | null = null;
let alvo = 0; // registro pedido pelo teclado no Texto, antes da resposta chegar
let selecao = 0; // descarta resposta de seleção mais velha que a última pedida
let noPainel = false; // painel de entrada no lugar do conteúdo
let irN = 0; // descarta resposta de arraste na régua mais velha que a última

const faixa = $("faixa");
const regua = criarRegua($("regua"), $("legenda"), {
  ir: async (r) => {
    if (modo === "texto") return rolarPara(textoEl, r, registros, ALTURA_TEXTO);
    const n = ++irN;
    const v = await invoke<number | null>("ir_para", { registro: r });
    if (v !== null && n === irN) rolarPara(arvore, v, total, ALTURA);
  },
  selecionar: (r) => void focarRegistro(r),
  marcasChegaram: () => agendar(),
  fmt: (n) => fmt(n),
});

const busca = criarBusca($("busca") as HTMLFormElement, {
  ir: irAoAchado,
  mudou: () => {
    regua.busca(busca.ativa());
    agendar();
  },
  voltar: () => (modo === "texto" ? textoEl : arvore).focus(),
  fmt: (n) => fmt(n),
});

/** Registros visíveis (base 0, inclusivo): retângulo da régua e "linhas X–Y de N". */
function mostrarFaixa(a: number | undefined, b: number | undefined): void {
  if (a === undefined || b === undefined) return;
  regua.janela(a, b);
  faixa.textContent = registros > 1 ? t.regua.faixa(fmt(a + 1), fmt(b + 1), fmt(registros)) : "";
}

const geometria = () => geo(arvore, total, ALTURA);

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
    return; // redesenhar pediria de novo, em laço
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

/** Prévia com os achados da busca marcados (a prévia começa em `faixa[0]` do registro). */
const previa = (l: Linha) => marcado(l.previa, l.faixa[0], busca.marcas(l.registro));

function valor(l: Linha): string {
  if (l.erro) {
    const msg = `${t.byte(fmt(l.erro.byte))}: ${t.motivos[l.erro.motivo] ?? t.motivos.outro}`;
    return `<span class="err">${icone("alert")}<span>${esc(msg)}</span></span><span class="val dim">${previa(l)}</span>`;
  }
  if (l.tipo === "objeto" || l.tipo === "lista") {
    const n = plural(l.filhos, l.tipo === "objeto" ? t.campos : t.itens);
    const cru = l.prof === 0 && !l.aberto ? ` ${previa(l)}` : "";
    return `<span class="val dim">${n}${cru}</span>`;
  }
  return `<span class="val${l.tipo === "nulo" ? " dim" : ""}">${previa(l)}</span>`;
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
    style="padding-left:${8 + l.prof * 20}px">${exp}${ponto(l.prof === 0 ? regua.ponto(l.registro) : null)}<span class="key">${esc(chave(l))}</span>${valor(l)}
    <button class="ib cp" type="button" tabindex="-1" data-cp aria-label="${esc(t.copiarCaminho)} ${esc(l.caminho)}">${icone("copy")}</button></div>`;
}

const ponto = (c: string | null) => (c === null ? "" : `<i class="pt ${c}" aria-hidden="true"></i>`);

let quadro = 0;
function desenhar(): void {
  quadro = 0;
  if (modo === "texto") return desenharTexto();
  alto.style.height = espaco(arvore, total, ALTURA);
  linhasEl.style.height = `${arvore.clientHeight}px`;
  const g = geometria();
  const { primeira, resto, cabem } = g;
  if (total && !cobre(primeira, cabem)) void pedir(primeira, cabem);
  const fim = Math.min(primeira + cabem, total);
  const [va, vb] = inteiras(g, total, ALTURA);
  mostrarFaixa(linhaEm(va)?.registro, linhaEm(vb)?.registro);
  let html = "";
  for (let v = primeira; v < fim; v++) html += linhaHtml(v, linhaEm(v));
  linhasEl.innerHTML = html || (resumo && !registros ? `<p class="vazio">${t.vazio}</p>` : "");
  (linhasEl.firstElementChild as HTMLElement | null)?.style.setProperty("margin-top", `${-resto}px`);
  atual = linhaEm(sel);
  if (document.getElementById(`l${sel}`)) arvore.setAttribute("aria-activedescendant", `l${sel}`);
  else arvore.removeAttribute("aria-activedescendant");
  rodape();
}
const agendar = () => (quadro ||= requestAnimationFrame(desenhar));

/** Migalhas e botões do rodapé da seleção, iguais nas duas visões. */
function rodape(): void {
  copiar.disabled = !atual;
  verFormatado.disabled = !atual || atual.erro !== null;
  migalhas.innerHTML = atual
    ? [t.linha(fmt(atual.registro + 1)), ...atual.migalhas].map((m) => `<li>${esc(m)}</li>`).join("")
    : "";
}

const mostrarLinha = (v: number) => mostrarEm(arvore, v, total, ALTURA);

function mover(v: number): void {
  if (!total) return;
  sel = Math.max(0, Math.min(total - 1, v));
  mostrarLinha(sel);
  desenhar();
}

// ---- Texto: uma linha por registro, na ordem do arquivo. ----

const geoTexto = () => geo(textoEl, registros, ALTURA_TEXTO);

async function pedirTexto(primeira: number, cabem: number): Promise<void> {
  const inicio = Math.max(0, primeira - Math.floor((BLOCO - cabem) / 2));
  if (pedidoTexto === inicio) return;
  pedidoTexto = inicio;
  try {
    texto = decodificar(inicio, await invoke<ArrayBuffer>("texto", { inicio, quantidade: BLOCO }));
  } catch (erro) {
    status.textContent = `${t.erro}: ${erro}`;
    return; // redesenhar pediria de novo, em laço
  } finally {
    if (pedidoTexto === inicio) pedidoTexto = null;
  }
  if (modo === "texto") desenharTexto();
}

function desenharTexto(): void {
  textoAlto.style.height = espaco(textoEl, registros, ALTURA_TEXTO);
  textoLinhas.style.height = `${textoEl.clientHeight}px`;
  const g = geoTexto();
  const { primeira, resto, cabem } = g;
  const fim = Math.min(primeira + cabem, registros);
  if (registros) mostrarFaixa(...inteiras(g, registros, ALTURA_TEXTO));
  if (registros && (!texto || primeira < texto.inicio || fim > texto.inicio + texto.linhas.length)) void pedirTexto(primeira, cabem);
  const digitos = String(registros).length;
  let html = "";
  for (let r = primeira; r < fim; r++) {
    const i = texto ? r - texto.inicio : -1;
    const linha = texto && i >= 0 ? texto.linhas[i] : undefined;
    const marcado = atual?.registro === r;
    const p = regua.ponto(r);
    const corpo =
      linha === undefined ? ""
      : textoHtml(linha, texto!.tamanhos[i], marcado ? atual!.faixa : null, (n) => t.maisBytes(fmt(n)), busca.marcas(r));
    html += `<div class="tl" id="t${r}" data-r="${r}" role="option" aria-selected="${marcado}"
      aria-label="${esc(t.linha(fmt(r + 1)))}"><span class="nl" style="width:${digitos + (p === null ? 2 : 4)}ch">${ponto(p)}${r + 1}</span><span class="tx">${corpo}</span></div>`;
  }
  textoLinhas.innerHTML = html;
  (textoLinhas.firstElementChild as HTMLElement | null)?.style.setProperty("margin-top", `${-resto}px`);
  if (atual && document.getElementById(`t${atual.registro}`)) textoEl.setAttribute("aria-activedescendant", `t${atual.registro}`);
  else textoEl.removeAttribute("aria-activedescendant");
  rodape();
}

/** Seleciona a linha visível `v` da árvore a partir do Texto (o cache da árvore pode não cobri-la). */
async function selecionar(v: number, n = ++selecao): Promise<void> {
  const l = linhaEm(v) ?? (await invoke<Linha[]>("linhas", { inicio: v, quantidade: 1 }))[0];
  if (n !== selecao) return;
  sel = v;
  atual = l;
  if (atual) mostrarEm(textoEl, (alvo = atual.registro), registros, ALTURA_TEXTO);
  desenharTexto();
}

async function irParaRegistro(r: number): Promise<void> {
  const n = ++selecao; // senão a resposta de uma tecla anterior devolve `alvo` para trás
  alvo = Math.max(0, Math.min(registros - 1, r));
  const v = await invoke<number | null>("ir_para", { registro: alvo });
  if (v !== null && n === selecao) await selecionar(v, n);
}

/** Clique num trecho do Texto: abre a árvore até o valor daquele byte e seleciona. */
async function revelar(registro: number, byte: number): Promise<void> {
  const r = await invoke<[number, number] | null>("revelar", { registro, byte });
  if (!r) return;
  total = r[1];
  versao++; // a lista da árvore mudou
  cache = null;
  await selecionar(r[0]);
}

/** Achado da busca: abre a árvore até o byte e seleciona, nas duas visões. */
async function irAoAchado(registro: number, byte: number): Promise<void> {
  const r = await invoke<[number, number] | null>("revelar", { registro, byte });
  if (!r) return;
  total = r[1];
  if (modo === "texto") {
    versao++;
    cache = null;
    await selecionar(r[0]);
    // Linha longa: traz o achado atual para a vista na horizontal também.
    document.querySelector(`#t${registro} .ac.at`)?.scrollIntoView({ block: "nearest", inline: "center" });
  } else {
    sel = r[0];
    mostrarLinha(sel);
    await recarregar();
  }
}

async function teclaTexto(e: KeyboardEvent): Promise<void> {
  const r = alvo;
  const pagina = Math.max(1, geoTexto().cabem - 2);
  const acoes: Record<string, () => unknown> = {
    ArrowDown: () => irParaRegistro(r + 1),
    ArrowUp: () => irParaRegistro(r - 1),
    PageDown: () => irParaRegistro(r + pagina),
    PageUp: () => irParaRegistro(r - pagina),
    Home: () => irParaRegistro(0),
    End: () => irParaRegistro(registros - 1),
    Enter: () => trocarModo("arvore"),
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

function trocarModo(m: Modo): void {
  modo = m;
  noPainel = false;
  painel.hidden = true;
  conteudo.hidden = false;
  for (const [k, aba] of Object.entries(abas)) {
    aba.setAttribute("aria-selected", String(k === m));
    aba.tabIndex = k === m ? 0 : -1;
  }
  arvore.hidden = m !== "arvore";
  textoEl.hidden = m !== "texto";
  if (m === "texto") {
    alvo = atual?.registro ?? 0;
    mostrarEm(textoEl, alvo, registros, ALTURA_TEXTO);
    desenharTexto();
    textoEl.focus();
  } else {
    void recarregar().then(() => {
      mostrarLinha(sel);
      desenhar();
    });
    arvore.focus();
  }
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

async function abrirFormatado(): Promise<void> {
  if (!atual) return;
  const dialogo = $<HTMLDialogElement>("formatado");
  $("formatado-titulo").textContent = `${t.registroFormatado} · ${t.linha(fmt(atual.registro + 1))}`;
  try {
    $("formatado-texto").textContent = await invoke<string>("formatado", { registro: atual.registro });
    dialogo.showModal();
  } catch (erro) {
    status.textContent = `${t.erro}: ${erro}`;
  }
}

let antesDaCopia: { msg: string; texto: string } | null = null;
/** Copia o nó `v` (chamar dentro do clique ou da tecla) e confirma no status e no botão. */
function copiarComo(v: number, f: Formato, botao?: HTMLElement): void {
  copiarNo(v, f).then(
    (s) => {
      const msg = t.copiar.feito(t.copiar[f], resumir(s));
      // Volta o status de antes, se nada mais o trocou nesse meio-tempo.
      antesDaCopia = { msg, texto: antesDaCopia?.texto ?? status.textContent ?? "" };
      status.textContent = msg;
      setTimeout(() => {
        if (antesDaCopia?.msg !== msg) return;
        if (status.textContent === msg) status.textContent = antesDaCopia.texto;
        antesDaCopia = null;
      }, 2500);
      if (!botao) return;
      const antes = botao.innerHTML;
      botao.innerHTML = botao === copiar ? `${icone("check")}<span>${t.copiado}</span>` : icone("check");
      setTimeout(() => (botao.innerHTML = antes), 1500);
    },
    (erro) => (status.textContent = `${t.erro}: ${erro}`),
  );
}
const menu = criarMenu($("menu"), (f) => copiarComo(sel, f));

let aberto: string | null = null;
const consulta = criarConsulta($("consulta"), {
  aberto: () => aberto,
  // Resultado e volta abrem na visão em uso, sem passar pelo painel de entrada.
  abrir: (c) => mostrar(c, noPainel ? undefined : modo),
  voltar: () => (modo === "texto" ? textoEl : arvore).focus(),
  fmt,
});

async function mostrar(caminho: string, visao?: Modo): Promise<void> {
  consulta.abriu(caminho);
  aberto = caminho;
  status.textContent = t.abrindo;
  const nome = caminho.split(/[\\/]/).pop() ?? caminho;
  $("nome").textContent = nome;
  try {
    const inicio = performance.now();
    total = registros = await invoke<number>("abrir", { caminho });
    const ms = fmt(Math.round(performance.now() - inicio));
    sel = 0;
    atual = undefined;
    texto = null;
    arvore.scrollTop = textoEl.scrollTop = 0;
    resumo = await invoke<Resumo>("resumo");
    // Sem régua para JSON único ou vazio (decisão 2 do micronavegador).
    regua.abrir(registros, ["objetos", "valores", "misto"].includes(resumo.conteudo));
    regua.andamento(resumo);
    status.textContent = t.aberto(plural(registros, t.registros), ms);
    $("painel-nome").textContent = nome;
    $("painel-sub").textContent = t.painel.aberto(t.painel.conteudo[resumo.conteudo], ms);
    desenharPainel();
    inicioBtn.disabled = false;
    const destino = visao ?? destinoAoAbrir();
    if (destino === "painel") mostrarPainel();
    await recarregar();
    if (destino === "texto") await selecionar(0);
    if (destino !== "painel") trocarModo(destino);
    busca.reabrir();
  } catch (erro) {
    status.textContent = `${t.erro}: ${erro}`;
  }
}

// ---- Painel de entrada: o que é o arquivo e por onde começar. ----

type Conteudo = keyof typeof t.painel.conteudo;
interface Andamento {
  geracao: number; varridas: number; total: number; invalidas: number; fim: boolean; campos: number | null;
  legenda: Legenda | null; coloriu: boolean;
}
interface Resumo extends Andamento { conteudo: Conteudo; bytes: number; registros: number }

/** Modos com cartão, na ordem dos cartões e dos atalhos 1, 2…; o sugerido é o primeiro disponível.
 * `sempre`: entra no seletor "Abrir sempre em…". Modo novo entra ligando `disponivel`. */
const MODOS: { id: Modo | "tabela"; icone: string; disponivel: boolean; sempre: boolean }[] = [
  { id: "tabela", icone: "table", disponivel: false, sempre: true },
  { id: "arvore", icone: "binary-tree", disponivel: true, sempre: true },
  { id: "texto", icone: "file-text", disponivel: true, sempre: false },
];
const disponiveis = MODOS.filter((m) => m.disponivel) as { id: Modo; icone: string; sempre: boolean }[];
const sugerido = disponiveis[0].id;
const PREFERENCIA = "jasao.aoAbrir";
const opcoes = ["painel", "sugerido", ...disponiveis.filter((m) => m.sempre).map((m) => m.id)];

const painel = $("painel");
const conteudo = $("conteudo");
const inicioBtn = $<HTMLButtonElement>("inicio");
const painelModos = $("painel-modos");
const preferencia = $<HTMLSelectElement>("painel-pref");
let resumo: Resumo | null = null;

function lerPreferencia(): string {
  let v: string | null = null;
  try {
    v = localStorage.getItem(PREFERENCIA);
  } catch {} // armazenamento bloqueado: fica o padrão
  if (v === null) return "painel";
  return opcoes.includes(v) ? v : "sugerido";
}

function destinoAoAbrir(): Modo | "painel" {
  const p = lerPreferencia();
  return p === "painel" ? "painel" : p === "sugerido" ? sugerido : (p as Modo);
}

const tamanho = (b: number) => {
  const [unit, base] = b >= 1e9 ? ["gigabyte", 1e9] : b >= 1e6 ? ["megabyte", 1e6] : b >= 1e3 ? ["kilobyte", 1e3] : ["byte", 1];
  return new Intl.NumberFormat(t.lang, { style: "unit", unit, unitDisplay: "short", maximumFractionDigits: 1 }).format(b / (base as number));
};

/** Selo e indicadores: refeitos a cada aviso de progresso da varredura. */
function desenharAndamento(): void {
  if (!resumo) return;
  const selo = $("painel-selo");
  selo.classList.toggle("fim", resumo.fim);
  selo.innerHTML = resumo.fim
    ? `${icone("check")}<span>${t.painel.completa}</span>`
    : `<span>${t.painel.analisando(fmt(resumo.varridas), fmt(resumo.total))}</span>`;
  const indicador = (rotulo: string, valor: string, classe = "") => `<div class="${classe}"><dt>${rotulo}</dt><dd>${valor}</dd></div>`;
  $("painel-indic").innerHTML =
    indicador(t.painel.tamanho, tamanho(resumo.bytes)) +
    indicador(t.painel.registros, fmt(resumo.registros)) +
    (resumo.campos ? indicador(t.painel.campos, fmt(resumo.campos)) : "") +
    (resumo.invalidas ? indicador(t.painel.invalidas, fmt(resumo.invalidas), "aviso") : "");
}

function desenharPainel(): void {
  desenharAndamento();
  painelModos.innerHTML = disponiveis
    .map((m, i) => {
      const [titulo, desc] = t.painel.modos[m.id];
      const s = m.id === sugerido;
      return `<button class="modo${s ? " sugerido" : ""}" type="button" data-modo="${m.id}" aria-keyshortcuts="${i + 1}">
        ${icone(m.icone)}<b>${titulo}</b><span>${desc}</span>${s ? `<em>${t.painel.sugerido}</em>` : ""}<kbd>${i + 1}</kbd></button>`;
    })
    .join("");
}

function mostrarPainel(): void {
  if (!resumo) return;
  noPainel = true;
  painel.hidden = false;
  conteudo.hidden = true;
  for (const aba of Object.values(abas)) aba.setAttribute("aria-selected", "false");
  painelModos.querySelector<HTMLElement>(".sugerido")?.focus();
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
  if (alvo.closest("[data-cp]") && l) copiarComo(v, "jq", alvo.closest<HTMLElement>("[data-cp]")!);
  else if (alvo.closest("[data-tg]")) void alternar(v);
  else desenhar();
  arvore.focus({ preventScroll: true });
});
linhasEl.addEventListener("dblclick", (e) => {
  const row = (e.target as HTMLElement).closest<HTMLElement>(".row[data-v]");
  if (row && !(e.target as HTMLElement).closest("[data-tg],[data-cp]") && linhaEm(Number(row.dataset.v))?.expansivel)
    void alternar(Number(row.dataset.v));
});
linhasEl.addEventListener("contextmenu", (e) => {
  const row = (e.target as HTMLElement).closest<HTMLElement>(".row[data-v]");
  if (!row) return;
  e.preventDefault();
  sel = Number(row.dataset.v);
  desenhar();
  menu.abrir(e.clientX, e.clientY, arvore);
});
copiar.addEventListener("click", () => {
  if (atual) copiarComo(sel, "jq", copiar);
});
verFormatado.addEventListener("click", () => void abrirFormatado());
$("formatado-fechar").addEventListener("click", () => $<HTMLDialogElement>("formatado").close());
textoEl.addEventListener("scroll", () => (quadro ||= requestAnimationFrame(desenhar)), { passive: true });
new ResizeObserver(agendar).observe(textoEl);
textoEl.addEventListener("keydown", (e) => void teclaTexto(e));
textoLinhas.addEventListener("click", (e) => {
  const alvo = e.target as HTMLElement;
  const tl = alvo.closest<HTMLElement>(".tl[data-r]");
  if (!tl) return;
  const b = alvo.closest<HTMLElement>("[data-b]")?.dataset.b;
  void revelar(Number(tl.dataset.r), b === undefined ? 0 : Number(b)).catch((erro) => (status.textContent = `${t.erro}: ${erro}`));
  textoEl.focus({ preventScroll: true });
});
textoLinhas.addEventListener("contextmenu", (e) => {
  const alvo = e.target as HTMLElement;
  const tl = alvo.closest<HTMLElement>(".tl[data-r]");
  if (!tl) return;
  e.preventDefault();
  const b = alvo.closest<HTMLElement>("[data-b]")?.dataset.b;
  revelar(Number(tl.dataset.r), b === undefined ? 0 : Number(b)).then(
    () => menu.abrir(e.clientX, e.clientY, textoEl),
    (erro) => (status.textContent = `${t.erro}: ${erro}`),
  );
});
for (const [k, aba] of Object.entries(abas)) aba.addEventListener("click", () => trocarModo(k as Modo));
$("abas").addEventListener("keydown", (e) => {
  if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
  const m: Modo = modo === "arvore" ? "texto" : "arvore";
  trocarModo(m);
  abas[m].focus();
});
document.addEventListener("keydown", (e) => {
  const cmd = (e.metaKey || e.ctrlKey) && !e.altKey;
  if (cmd && e.key.toLowerCase() === "g" && resumo) {
    e.preventDefault();
    void (e.shiftKey ? busca.anterior() : busca.proximo()).catch((erro) => (status.textContent = `${t.erro}: ${erro}`));
    return;
  }
  if (cmd && !e.shiftKey && e.key.toLowerCase() === "j" && resumo) {
    e.preventDefault();
    if (noPainel) trocarModo(sugerido);
    consulta.focar();
    return;
  }
  if (cmd && !e.shiftKey && e.key.toLowerCase() === "f" && resumo) {
    e.preventDefault();
    if (noPainel) trocarModo(sugerido);
    busca.focar();
    return;
  }
  // Copiar: menu pela tecla de contexto ou ⇧F10; ⇧⌘C copia o caminho jq; ⌘C, o valor (sem texto selecionado).
  const visao = (e.target as HTMLElement).closest<HTMLElement>("#arvore, #texto");
  if (visao && atual && !noPainel && (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10"))) {
    e.preventDefault();
    const r = (document.getElementById(modo === "texto" ? `t${atual.registro}` : `l${sel}`) ?? visao).getBoundingClientRect();
    menu.abrir(r.left + 24, r.bottom, visao);
    return;
  }
  if (cmd && e.key.toLowerCase() === "c" && atual && !noPainel) {
    const vale = e.shiftKey ? !(e.target as HTMLElement).closest("input, textarea") : visao && getSelection()?.isCollapsed !== false;
    if (vale) {
      e.preventDefault();
      copiarComo(sel, e.shiftKey ? "jq" : "valor");
      return;
    }
  }
  if (e.altKey || e.shiftKey) return;
  if (!(e.metaKey || e.ctrlKey)) {
    // No painel: 1, 2… abrem o cartão daquela posição; Enter fora de um botão abre o sugerido.
    const alvo = e.target as HTMLElement;
    if (!noPainel || alvo.closest("select, input, dialog")) return;
    const m =
      e.key === "Enter" && !alvo.closest("button") ? sugerido
      : /^[1-9]$/.test(e.key) ? disponiveis[Number(e.key) - 1]?.id
      : undefined;
    if (!m) return;
    e.preventDefault();
    trocarModo(m);
    return;
  }
  if (e.key === "0") {
    e.preventDefault();
    mostrarPainel();
    return;
  }
  const m = ({ "1": "arvore", "2": "texto" } as const)[e.key as "1" | "2"];
  if (!m) return;
  e.preventDefault();
  trocarModo(m);
});
painelModos.addEventListener("click", (e) => {
  const m = (e.target as HTMLElement).closest<HTMLElement>("[data-modo]")?.dataset.modo;
  if (m) trocarModo(m as Modo);
});
inicioBtn.addEventListener("click", mostrarPainel);
preferencia.addEventListener("change", () => {
  try {
    localStorage.setItem(PREFERENCIA, preferencia.value);
  } catch {} // armazenamento bloqueado: vale só nesta sessão
});
void listen<Andamento>("varredura", ({ payload: p }) => {
  // Aviso de arquivo anterior, ou mais velho que o `resumo` já recebido: ignora.
  if (!resumo || p.geracao !== resumo.geracao || (p.varridas < resumo.varridas && !p.fim)) return;
  Object.assign(resumo, p);
  desenharAndamento();
  regua.andamento(p);
});
/** Seleciona o registro `r` na visão atual e põe o foco nela; `false` se não existir. */
async function focarRegistro(r: number): Promise<boolean> {
  const v = await invoke<number | null>("ir_para", { registro: r });
  if (v === null) return false;
  if (modo === "texto") {
    await selecionar(v);
    textoEl.focus();
  } else {
    mover(v);
    arvore.focus();
  }
  return true;
}

$("ir").addEventListener("submit", async (e) => {
  e.preventDefault();
  const n = Number(($("linha") as HTMLInputElement).value);
  if (!(Number.isInteger(n) && n >= 1 && (await focarRegistro(n - 1)))) status.textContent = t.foraDoArquivo(String(n), fmt(registros));
});
// Textos fixos do HTML no idioma do sistema.
document.documentElement.lang = t.lang;
$("abrir").textContent = t.abrir;
abas.arvore.firstChild!.textContent = t.arvore;
abas.texto.firstChild!.textContent = t.texto;
$("abas").setAttribute("aria-label", t.valores);
verFormatado.querySelector("span")!.textContent = t.verFormatado;
$("formatado-fechar").setAttribute("aria-label", t.fechar);
textoEl.setAttribute("aria-label", t.textoDoArquivo);
$("rotulo-linha").textContent = t.irParaLinha;
copiar.querySelector("span")!.textContent = t.copiarCaminho;
$("menu").setAttribute("aria-label", t.copiar.menu);
for (const f of FORMATOS) document.querySelector(`[data-formato="${f}"] span`)!.textContent = t.copiar[f];
migalhas.setAttribute("aria-label", t.caminhoSelecionado);
$("tema").setAttribute("aria-label", t.tema);
arvore.setAttribute("aria-label", t.valoresDoArquivo);
inicioBtn.querySelector("span")!.textContent = t.inicio;
$("painel-pergunta").textContent = t.painel.pergunta;
$("painel-rotulo").textContent = t.painel.aoAbrir;
preferencia.innerHTML = opcoes.map((o) => `<option value="${o}">${t.painel.pref[o as keyof typeof t.painel.pref]}</option>`).join("");
preferencia.value = lerPreferencia();

$("tema").addEventListener("click", () => {
  const raiz = document.documentElement;
  raiz.dataset.theme = raiz.dataset.theme === "dark" ? "light" : "dark";
  regua.redesenhar();
});
if (matchMedia("(prefers-color-scheme: dark)").matches) document.documentElement.dataset.theme = "dark";

$("abrir").addEventListener("click", async () => {
  const caminho = await open({ multiple: false, filters: [{ name: "JSON Lines", extensions: ["jsonl", "ndjson", "json"] }] });
  if (typeof caminho === "string") await mostrar(caminho);
});

invoke<string | null>("arquivo_inicial").then((caminho) => {
  if (caminho) void mostrar(caminho);
});
