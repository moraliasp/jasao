// Tela Valores: Árvore e Texto sobre a mesma seleção. O Rust guarda o arquivo e a lista plana
// de linhas visíveis; aqui só se pede a janela que cabe na tela (o resto não existe no DOM).
// Cada aba de arquivo é um documento no Rust; as variáveis abaixo são as da aba ativa, guardadas
// na aba ao sair dela e trazidas de volta ao voltar (ver `ativar`).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ask } from "@tauri-apps/plugin-dialog";
import { aposFechar, ciclo, EXTENSOES, nomes, porAtalho, posicaoNova, triar } from "./abas";
import { chamar, docAtivo, usarDoc } from "./doc";
import { criarBusca } from "./busca";
import { EXEMPLOS, nomeExemplo } from "./exemplos";
import { type Estrutura, type ItemE, itens as itensEstrutura } from "./estrutura";
import { erroDoRust, type Motivo, t } from "./i18n";
import { criarRegua, type Legenda } from "./regua";
import { espaco, geometria as geo, inteiras, mostrarLinha as mostrarEm, rolarPara } from "./rolagem";
import { criarConsulta } from "./consulta";
import { criarRecentes } from "./recentes";
import { criarExportar } from "./exportar";
import { criarGrade, type Ordenacao } from "./grade";
import { copiar as copiarNo, criarMenu, FORMATOS, type Formato } from "./copiar";
import {
  decodificar,
  escAtributo,
  type JanelaTexto,
  marcado,
  resumir,
  semInvisiveis,
  linhaHtml as textoHtml,
  visivel,
} from "./texto";

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
  omitidos: number; // filhos fora da árvore pelo teto de nós do registro
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
const abas = { arvore: $("aba-arvore"), texto: $("aba-texto"), estrutura: $("aba-estrutura"), tabela: $("aba-tabela"), exportar: $("aba-exportar") };
type Modo = keyof typeof abas;
const fmt = (n: number) => n.toLocaleString(t.lang);
const esc = escAtributo;
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
const estruturaEl = $("estrutura");
const estruturaLinhas = $("estrutura-linhas");
let estr: ItemE[] | null = null; // null: ainda não pedida (ou a inferência não acabou)
let selE = 0;
let registrosE = 0;
const tabelaEl = $("tabela");
const grade = criarGrade(tabelaEl, {
  geracao: () => resumo?.geracao,
  inferiu: () => !!resumo?.inferiu,
  revelar: (linha, byte) => {
    trocarModo("arvore");
    void revelar(linha, byte).catch((erro) => (status.textContent = `${t.erro}: ${erro}`));
  },
  mudou: () => {
    if (modo === "tabela") rodape();
    if (!noPainel) return;
    const f = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>("[data-modo]")?.dataset.modo;
    desenharPainel(); // cartão da Tabela liga ou desliga; o foco fica no mesmo cartão
    if (f) painelModos.querySelector<HTMLElement>(`[data-modo="${f}"]`)?.focus();
  },
  erro: (erro) => (status.textContent = `${t.erro}: ${erro}`),
  fmt: (n) => fmt(n),
});

const faixa = $("faixa");
const regua = criarRegua($("regua"), $("legenda"), {
  ir: async (r) => {
    if (modo === "texto") return rolarPara(textoEl, r, registros, ALTURA_TEXTO);
    const n = ++irN;
    const v = await chamar<number | null>("ir_para", { registro: r });
    if (v !== null && n === irN) rolarPara(arvore, v, total, ALTURA);
  },
  selecionar: (r) => void focarRegistro(r),
  marcasChegaram: () => agendar(),
  fmt: (n) => fmt(n),
});

const exportarEl = $("exportar");
const visaoAtual = () => ({ texto: textoEl, estrutura: estruturaEl, tabela: tabelaEl, exportar: exportarEl, arvore })[modo];
/** Visão sem linha de registro (Estrutura, Tabela, Exportar): achado e "ir para registro" vão para a Árvore. */
const semRegistros = () => modo === "estrutura" || modo === "tabela" || modo === "exportar";
const exportar = criarExportar(exportarEl, {
  fmt: (n) => fmt(n),
  icone: (id) => icone(id),
  nome: () => nomesDasAbas()[ativa] ?? "",
  avisar: (msg) => void (status.textContent = msg),
  erro: (e) => erroAoAbrir(e),
});

const busca = criarBusca($("busca") as HTMLFormElement, {
  ir: irAoAchado,
  mudou: () => {
    regua.busca(busca.ativa());
    agendar();
  },
  voltar: () => visaoAtual().focus(),
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
    const linhas = await chamar<Linha[]>("linhas", { inicio, quantidade: BLOCO });
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
    const n = plural(l.filhos, l.tipo === "objeto" ? t.campos : t.itens) +
      (l.omitidos ? ` · ${t.mostrando(fmt(l.filhos - l.omitidos))}` : "");
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
    style="padding-left:${8 + l.prof * 20}px">${exp}${ponto(l.prof === 0 ? regua.ponto(l.registro) : null)}<span class="key">${visivel(chave(l))}</span>${valor(l)}
    <button class="ib cp" type="button" tabindex="-1" data-cp aria-label="${esc(t.copiarCaminho)} ${esc(chave(l))}">${icone("copy")}</button></div>`;
}

const ponto = (c: string | null) => (c === null ? "" : `<i class="pt ${c}" aria-hidden="true"></i>`);

let quadro = 0;
function desenhar(): void {
  quadro = 0;
  if (modo === "texto") return desenharTexto();
  if (semRegistros()) return;
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

let migalhasDe = ""; // doc:versão:linha das migalhas mostradas (vêm do Rust só para a seleção)

/** Migalhas e botões do rodapé da seleção, iguais nas duas visões. */
function rodape(): void {
  if (modo === "exportar") {
    copiar.disabled = verFormatado.disabled = true; // o Exportar tem o próprio Copiar
    migalhas.innerHTML = migalhasDe = "";
    return;
  }
  if (modo === "estrutura") {
    const it = estr?.[selE];
    copiar.disabled = !it;
    verFormatado.disabled = true;
    migalhas.innerHTML = it ? `<li>${visivel(it.caminho)}</li>` : "";
    migalhasDe = "";
    return;
  }
  if (modo === "tabela") {
    const c = grade.caminho();
    copiar.disabled = !c;
    verFormatado.disabled = true;
    migalhas.innerHTML = c ? `<li>${visivel(c)}</li>` : "";
    migalhasDe = "";
    return;
  }
  copiar.disabled = !atual;
  verFormatado.disabled = !atual || atual.erro !== null;
  const de = atual ? `${docAtivo()}:${versao}:${sel}` : "";
  if (de === migalhasDe) return;
  migalhasDe = de;
  if (!atual) {
    migalhas.innerHTML = "";
    return;
  }
  const inicio = t.linha(fmt(atual.registro + 1));
  migalhas.innerHTML = `<li>${visivel(inicio)}</li>`;
  chamar<string[] | null>("migalhas", { v: sel })
    .then((m) => {
      if (migalhasDe === de) migalhas.innerHTML = [inicio, ...(m ?? [])].map((x) => `<li>${visivel(x)}</li>`).join("");
    })
    .catch(() => {}); // doc fechado no meio: o próximo desenho refaz
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
  const v = versao;
  try {
    const j = decodificar(inicio, await chamar<ArrayBuffer>("texto", { inicio, quantidade: BLOCO }));
    if (v === versao) texto = j;
  } catch (erro) {
    status.textContent = `${t.erro}: ${erro}`;
    return; // redesenhar pediria de novo, em laço
  } finally {
    if (pedidoTexto === inicio) pedidoTexto = null;
  }
  if (v === versao && modo === "texto") desenharTexto();
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
  const l = linhaEm(v) ?? (await chamar<Linha[]>("linhas", { inicio: v, quantidade: 1 }))[0];
  if (n !== selecao) return;
  sel = v;
  atual = l;
  if (atual) mostrarEm(textoEl, (alvo = atual.registro), registros, ALTURA_TEXTO);
  desenharTexto();
}

async function irParaRegistro(r: number): Promise<void> {
  const n = ++selecao; // senão a resposta de uma tecla anterior devolve `alvo` para trás
  alvo = Math.max(0, Math.min(registros - 1, r));
  const v = await chamar<number | null>("ir_para", { registro: alvo });
  if (v !== null && n === selecao) await selecionar(v, n);
}

/** Clique num trecho do Texto: abre a árvore até o valor daquele byte e seleciona. */
async function revelar(registro: number, byte: number): Promise<void> {
  const r = await chamar<[number, number] | null>("revelar", { registro, byte });
  if (!r) return;
  total = r[1];
  versao++; // a lista da árvore mudou
  cache = null;
  await selecionar(r[0]);
}

/** Achado da busca: abre a árvore até o byte e seleciona, nas duas visões. */
async function irAoAchado(registro: number, byte: number): Promise<void> {
  if (semRegistros()) trocarModo("arvore"); // achado é de registro: mostra na Árvore
  const r = await chamar<[number, number] | null>("revelar", { registro, byte });
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

// ---- Estrutura: o que o núcleo inferiu, um caminho por linha. ----

/** Pede a estrutura se a inferência já acabou; senão o evento `varredura` (inferiu) chama de novo. */
async function carregarEstrutura(): Promise<void> {
  if (estr || !resumo?.inferiu) return desenharEstrutura();
  const g = resumo.geracao;
  const d = docAtivo();
  const e = await chamar<Estrutura | null>("estrutura");
  if (!e || d !== docAtivo() || g !== resumo?.geracao) return; // trocou de aba ou reabriu
  estr = itensEstrutura(e, t, fmt);
  selE = 0;
  registrosE = e.registros;
  desenharEstrutura();
}

// shortcut: sem rolagem virtual (são centenas de caminhos, não linhas do arquivo); virtualizar
// como a Árvore se aparecer arquivo com dezenas de milhares de caminhos.
function desenharEstrutura(): void {
  const resumoEl = $("estrutura-resumo");
  if (!estr) {
    resumoEl.textContent = t.estrutura.inferindo;
    estruturaLinhas.innerHTML = "";
  } else {
    resumoEl.textContent = registrosE ? t.estrutura.resumo(fmt(estr.length), fmt(registrosE)) : t.vazio;
    estruturaLinhas.innerHTML = registrosE
      ? estr.map((it, i) => `<div class="row" id="e${i}" data-i="${i}" role="option" aria-selected="${i === selE}" aria-label="${esc(it.rotulo)}">${it.html}</div>`).join("")
      : "";
  }
  const el = document.getElementById(`e${selE}`);
  if (el) {
    estruturaEl.setAttribute("aria-activedescendant", el.id);
    el.scrollIntoView({ block: "nearest" });
  } else estruturaEl.removeAttribute("aria-activedescendant");
  rodape();
}

function moverE(i: number): void {
  if (!estr?.length) return;
  selE = Math.max(0, Math.min(estr.length - 1, i));
  desenharEstrutura();
}

/** Primeira ocorrência na Árvore: o registro dela (o núcleo dá a linha, não o nó). */
async function abrirOcorrencia(): Promise<void> {
  const linha = estr?.[selE]?.linha;
  if (linha === null || linha === undefined) return;
  await focarRegistro(linha);
}

function teclaEstrutura(e: KeyboardEvent): void {
  const pagina = Math.max(1, Math.floor(estruturaEl.clientHeight / ALTURA) - 2);
  const acoes: Record<string, () => unknown> = {
    ArrowDown: () => moverE(selE + 1),
    ArrowUp: () => moverE(selE - 1),
    PageDown: () => moverE(selE + pagina),
    PageUp: () => moverE(selE - pagina),
    Home: () => moverE(0),
    End: () => moverE(Infinity),
    Enter: () => abrirOcorrencia(),
  };
  const acao = acoes[e.key];
  if (!acao) return;
  e.preventDefault();
  Promise.resolve(acao()).catch((erro) => (status.textContent = `${t.erro}: ${erro}`));
}

/** Foca `el`, a menos que o usuário esteja andando pela barra de abas (aba ainda preparando não rouba o foco). */
function focar(el: HTMLElement | null | undefined): void {
  if (!$("arquivos-lista").contains(document.activeElement)) el?.focus();
}

/** `rolagem`: [árvore, texto] guardados na aba; sem ela, a seleção vem para a vista. */
function trocarModo(m: Modo, rolagem?: [number, number]): void {
  if (m !== "exportar") exportar.parar();
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
  estruturaEl.hidden = m !== "estrutura";
  $("tabela-vista").hidden = m !== "tabela";
  exportarEl.hidden = m !== "exportar";
  if (m === "tabela") {
    grade.mostrar();
    rodape();
    focar(tabelaEl);
  } else if (m === "exportar") {
    rodape();
    void exportar.mostrar();
    focar(exportarEl);
  } else if (m === "estrutura") {
    void carregarEstrutura().catch((erro) => (status.textContent = `${t.erro}: ${erro}`));
    focar(estruturaEl);
  } else if (m === "texto") {
    alvo = atual?.registro ?? 0;
    if (rolagem) {
      textoAlto.style.height = espaco(textoEl, registros, ALTURA_TEXTO);
      textoEl.scrollTop = rolagem[1];
    } else mostrarEm(textoEl, alvo, registros, ALTURA_TEXTO);
    desenharTexto();
    focar(textoEl);
  } else {
    if (rolagem) {
      alto.style.height = espaco(arvore, total, ALTURA);
      arvore.scrollTop = rolagem[0];
    }
    void recarregar().then(() => {
      if (!rolagem) mostrarLinha(sel);
      desenhar();
    });
    focar(arvore);
  }
}

/** Recarrega a janela atual depois de uma mudança na lista, sem piscar linhas vazias. */
async function recarregar(): Promise<void> {
  versao++;
  pedido = null;
  const { primeira, cabem } = geometria();
  const v = versao;
  const inicio = Math.max(0, primeira - Math.floor((BLOCO - cabem) / 2));
  const linhas = await chamar<Linha[]>("linhas", { inicio, quantidade: BLOCO });
  if (v !== versao) return;
  cache = { inicio, linhas };
  desenhar();
}

async function alternar(v: number): Promise<void> {
  total = await chamar<number>("alternar", { v });
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
      const p = await chamar<number | null>("pai", { v: sel });
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
    const f = await chamar<{ texto: string; truncado: boolean; teto: number }>("formatado", { registro: atual.registro });
    $("formatado-texto").innerHTML = visivel(f.texto);
    if (f.truncado) $("formatado-titulo").textContent += ` · ${t.formatadoTruncado(fmt(f.teto / 2 ** 20))}`;
    dialogo.showModal();
  } catch (erro) {
    status.textContent = `${t.erro}: ${erro}`;
  }
}

let antesDaCopia: { msg: string; texto: string } | null = null;
/** Copia o nó `v` (chamar dentro do clique ou da tecla) e confirma no status e no botão. */
function copiarComo(v: number, f: Formato, botao?: HTMLElement): void {
  confirmarCopia(copiarNo(v, f), f, botao);
}
/** Confirma no status e no botão a cópia de `p` (texto copiado, no formato `f`). */
function confirmarCopia(p: Promise<string>, f: Formato, botao?: HTMLElement): void {
  p.then(
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

const consulta = criarConsulta($("consulta"), {
  aberto: () => abaAtiva()?.caminho ?? null,
  // Resultado e volta abrem na visão em uso, sem passar pelo painel de entrada.
  abrir: async (c, id) => {
    const a = lista.find((x) => x.doc === id);
    if (a) await mostrar(c, a, a === abaAtiva() ? (noPainel ? undefined : modo) : a.modo);
  },
  voltar: () => visaoAtual().focus(),
  fmt,
});

// ---- Abas de arquivo ----

/** Uma aba. Os campos de estado só valem fora da aba ativa: a ativa vive nas variáveis do módulo. */
interface Aba {
  chave: number; // id no DOM
  doc: number | null; // documento no Rust (null: aba vazia ou abrindo)
  arquivo: string | null; // arquivo do usuário (dá o nome da aba)
  caminho: string | null; // o aberto agora: o arquivo ou o resultado de uma consulta
  ms: string; // tempo de abertura, para o painel
  visao?: Modo; // visão pedida ao abrir; vale quando a aba for preparada
  total: number;
  registros: number;
  sel: number;
  modo: Modo;
  noPainel: boolean;
  resumo: Resumo | null; // null: aberta no Rust, mas ainda não preparada na tela
  atual: Linha | undefined;
  alvo: number;
  rolagem: [number, number];
  status: string;
  estr: ItemE[] | null; // Estrutura já formatada (null: ainda não pedida)
  selE: number;
  registrosE: number;
}
const listaEl = $("arquivos-lista");
const semArquivo = $("sem-arquivo");
const area = $("area");
const soltar = $("soltar");
let chaves = 0;
let lista: Aba[] = [];
let ativa = -1;
const abaAtiva = (): Aba | undefined => lista[ativa];
const abaVazia = (): Aba => ({
  chave: ++chaves, doc: null, arquivo: null, caminho: null, ms: "", total: 0, registros: 0, sel: 0, modo: "arvore",
  noPainel: false, resumo: null, atual: undefined, alvo: 0, rolagem: [0, 0], status: "",
  estr: null, selE: 0, registrosE: 0,
});
/** Acima desta soma de registros nas abas, abrir mais um avisa (o Rust guarda índice e árvore de cada). */
const AVISO_REGISTROS = 50_000_000;

function guardar(a: Aba): void {
  Object.assign(a, {
    total, registros, sel, modo, noPainel, resumo, atual, alvo, estr, selE, registrosE,
    rolagem: [arvore.scrollTop, textoEl.scrollTop], status: status.textContent ?? "",
  });
}

function nomesDasAbas(): string[] {
  const base = nomes(lista.map((a) => a.arquivo), t.abasArquivos.vazia);
  return lista.map((a, i) => (a.arquivo && nomeExemplo(a.arquivo)) || base[i]);
}

function desenharAbas(): void {
  const n = nomesDasAbas();
  listaEl.innerHTML = lista
    .map((a, i) => {
      const s = i === ativa;
      const atalho = i < 8 ? ` aria-keyshortcuts="Meta+${i + 1} Control+${i + 1}"` : "";
      // Só a ativa aponta para o painel: no WebKit, aba cujo painel tem o foco conta como selecionada.
      const painel = s ? ` aria-controls="${a.doc === null ? "sem-arquivo" : "area"}"` : "";
      return `<div class="arq" role="presentation"><button role="tab" type="button" id="arq-${a.chave}" data-i="${i}"
        aria-selected="${s}"${painel} tabindex="${s ? 0 : -1}"
        title="${esc(a.arquivo ?? n[i])}"${atalho}>${esc(n[i])}</button><button class="ib" type="button" tabindex="-1"
        data-fechar="${i}" aria-label="${esc(t.abasArquivos.fechar(n[i]))}">${icone("x")}</button></div>`;
    })
    .join("");
  const a = abaAtiva();
  if (!a) return;
  area.setAttribute("aria-labelledby", `arq-${a.chave}`);
  semArquivo.setAttribute("aria-labelledby", `arq-${a.chave}`);
  $("nome").textContent = a.arquivo ? semInvisiveis(n[ativa]) : "";
  document.getElementById(`arq-${a.chave}`)?.scrollIntoView({ block: "nearest", inline: "nearest" });
}

/** Mostra a aba `i`: guarda a que sai e traz o estado da que entra. `foco`: o foco fica na aba. */
function ativar(i: number, foco = false): void {
  const antes = abaAtiva();
  if (antes && i !== ativa) {
    exportar.parar(); // validação é do doc que sai
    guardar(antes);
  }
  ativa = i;
  const a = lista[i];
  ({ total, registros, sel, modo, noPainel, resumo, atual, alvo, estr, selE, registrosE } = a);
  status.textContent = a.status;
  mostrarCancelar();
  // Respostas em voo eram da aba que saiu.
  versao++;
  selecao++;
  irN++;
  cache = texto = null;
  pedido = pedidoTexto = null;
  usarDoc(a.doc);
  busca.usar(a.doc);
  consulta.usar(a.doc);
  desenharAbas();
  const vazia = a.doc === null;
  semArquivo.hidden = !vazia;
  area.hidden = vazia;
  inicioBtn.disabled = vazia;
  faixa.textContent = "";
  if (vazia) regua.abrir(0, false);
  else if (!resumo) void preparar(a, a.visao);
  else {
    prepararRegua();
    $("painel-nome").textContent = semInvisiveis(nomesDasAbas()[i]);
    $("painel-sub").textContent = t.painel.aberto(t.painel.conteudo[resumo.conteudo], a.ms);
    desenharPainel();
    if (noPainel) mostrarPainel();
    else trocarModo(modo, a.rolagem);
  }
  if (foco) document.getElementById(`arq-${a.chave}`)?.focus();
  else if (vazia && !a.arquivo) $("sem-arquivo-abrir").focus();
}

/** Régua do documento ativo, com a contagem e as cores que a varredura já trouxe. */
function prepararRegua(): void {
  if (!resumo) return;
  // Sem régua para JSON único ou vazio (decisão 2 do micronavegador).
  regua.abrir(registros, ["objetos", "valores", "misto"].includes(resumo.conteudo));
  regua.andamento(resumo);
  regua.busca(busca.ativa());
}

/** Abas abrindo agora, com o id da abertura: a descompactação delas tem progresso e cancelamento.
 * Id e não caminho: o mesmo arquivo pode estar abrindo em duas abas. */
const abrindo = new Map<Aba, number>();
const cancelarAbertura = $<HTMLButtonElement>("cancelar-abertura");
const mostrarCancelar = () => void (cancelarAbertura.hidden = !abrindo.has(abaAtiva()!));
const cancelarDe = (a: Aba) => {
  const abertura = abrindo.get(a);
  if (abertura) void chamar("cancelar_abertura", { abertura }).catch(() => {});
};
cancelarAbertura.textContent = t.compactado.cancelar;
cancelarAbertura.addEventListener("click", () => cancelarDe(abaAtiva()!));
void listen<{ abertura: number; lidos: number; total: number }>("descompactando", ({ payload: p }) => {
  for (const [a, abertura] of abrindo) {
    if (abertura !== p.abertura) continue;
    a.status = t.descompactando(Math.floor((p.lidos * 100) / Math.max(p.total, 1)));
    if (a === abaAtiva()) status.textContent = a.status;
  }
});

/** Erro do `abrir` no idioma da interface (os da descompactação vêm como código do Rust). */
function erroAoAbrir(erro: unknown): string {
  const e = String(erro);
  return e === "jasao:cancelada" ? t.compactado.cancelada : `${t.erro}: ${erroDoRust(e)}`;
}

/** Abre `caminho` no documento da aba `a` (cria o documento se a aba ainda não tiver); diz se abriu.
 * `forcar`: abre compactado acima da razão de bomba (o usuário confirmou). */
async function mostrar(caminho: string, a: Aba, visao?: Modo, forcar = false): Promise<boolean> {
  if (a.doc !== null) consulta.abriu(caminho, a.doc);
  a.status = t.abrindo;
  if (a === abaAtiva()) status.textContent = t.abrindo;
  try {
    // O id vem do backend já registrado: o Cancelar antes de o `abrir` começar não se perde.
    const abertura = await chamar<number>("nova_abertura");
    abrindo.set(a, abertura);
    mostrarCancelar();
    const inicio = performance.now();
    const r = await chamar<{ id: number; total: number }>("abrir", { caminho, id: a.doc, abertura, forcar }).finally(
      () => {
        if (abrindo.get(a) === abertura) abrindo.delete(a);
        mostrarCancelar();
      },
    );
    if (!lista.includes(a)) {
      void chamar("fechar", { id: r.id }).catch(() => {}); // aba fechada no meio
      return false;
    }
    const ms = fmt(Math.round(performance.now() - inicio));
    Object.assign(a, {
      doc: r.id, caminho, ms, visao, total: r.total, registros: r.total, sel: 0, atual: undefined, alvo: 0,
      resumo: null, rolagem: [0, 0], status: t.aberto(plural(r.total, t.registros), ms), estr: null, selE: 0,
    });
    const soma = lista.reduce((n, x) => n + x.registros, 0);
    if (soma > AVISO_REGISTROS) a.status += ` · ${t.abasArquivos.memoria(fmt(soma))}`;
    if (a === abaAtiva()) ativar(ativa);
    else desenharAbas();
    return true;
  } catch (erro) {
    if (!lista.includes(a)) return false; // aba fechada no meio (fechar cancela a descompactação)
    if (
      String(erro) === "jasao:razao" &&
      (await ask(t.compactado.razao, { title: t.compactado.titulo, kind: "warning" })) &&
      lista.includes(a)
    )
      return mostrar(caminho, a, visao, true);
    const msg = erroAoAbrir(erro);
    if (a.doc === null) {
      // Não abriu: a aba sai em vez de ficar quebrada; o erro aparece na aba que ficar ativa.
      const i = lista.indexOf(a);
      if (i >= 0) fecharAba(i);
      abaAtiva()!.status = status.textContent = msg;
    } else {
      a.status = msg;
      if (a === abaAtiva()) status.textContent = msg;
    }
    return false;
  }
}

/** Primeira vez na tela de um documento já aberto no Rust: resumo, régua e visão inicial. */
async function preparar(a: Aba, visao?: Modo): Promise<void> {
  const v = versao;
  try {
    const r = await chamar<Resumo>("resumo");
    if (v !== versao) return; // trocou de aba: prepara quando voltar
    resumo = r;
    prepararRegua();
    $("painel-nome").textContent = semInvisiveis(nomesDasAbas()[ativa]);
    $("painel-sub").textContent = t.painel.aberto(t.painel.conteudo[r.conteudo], a.ms);
    desenharPainel();
    const destino = visao ?? destinoAoAbrir();
    if (destino === "painel") mostrarPainel();
    await recarregar();
    if (destino === "texto") await selecionar(0);
    if (v !== versao) return;
    if (destino !== "painel") trocarModo(destino);
    busca.reabrir();
  } catch (erro) {
    if (v === versao) status.textContent = `${t.erro}: ${erro}`;
  }
}

/** Abre o arquivo numa aba: a dele, se já estiver aberto; a ativa, se vazia; senão uma nova. */
async function abrirEmAba(caminho: string): Promise<boolean> {
  const j = lista.findIndex((a) => a.arquivo === caminho);
  if (j >= 0) {
    ativar(j);
    return true;
  }
  if (abaAtiva()?.arquivo !== null) novaAba();
  const a = abaAtiva()!;
  a.arquivo = caminho;
  desenharAbas();
  return mostrar(caminho, a);
}

/** Recente ou pedido do sistema: se não abriu porque o arquivo sumiu, sai dos recentes com aviso. */
async function abrirPedido(caminho: string): Promise<void> {
  if (await abrirEmAba(caminho)) return;
  if (await invoke<boolean>("recente_ausente", { caminho })) {
    status.textContent = t.recentes.sumiu(caminho.split(/[\\/]/).pop() ?? caminho);
  }
}

function novaAba(): void {
  const i = posicaoNova(lista.length, ativa);
  lista.splice(i, 0, abaVazia());
  if (i <= ativa) ativa++;
  ativar(i);
}

/** Fecha a aba `i` e libera o documento dela; fechar a última deixa uma aba vazia. */
function fecharAba(i: number): void {
  const a = lista[i];
  if (!a) return;
  cancelarDe(a);
  if (a.doc !== null) {
    void chamar("fechar", { id: a.doc }).catch(() => {}); // já fechado: nada a liberar
    busca.fechar(a.doc);
    consulta.fechar(a.doc);
    grade.esquecer(a.doc);
  }
  const eraAtiva = i === ativa;
  const nova = aposFechar(lista.length, i, ativa);
  lista.splice(i, 1);
  if (!lista.length) lista.push(abaVazia());
  if (eraAtiva || nova < 0) {
    ativa = -1;
    ativar(Math.max(0, nova));
  } else {
    ativa = nova;
    desenharAbas();
  }
}

listaEl.addEventListener("click", (e) => {
  const alvo = e.target as HTMLElement;
  const f = alvo.closest<HTMLElement>("[data-fechar]");
  if (f) return fecharAba(Number(f.dataset.fechar));
  const tab = alvo.closest<HTMLElement>("[data-i]");
  if (tab) ativar(Number(tab.dataset.i), true);
});
listaEl.addEventListener("auxclick", (e) => {
  const tab = (e.target as HTMLElement).closest(".arq")?.querySelector<HTMLElement>("[data-i]");
  if (e.button === 1 && tab) fecharAba(Number(tab.dataset.i));
});
listaEl.addEventListener("keydown", (e) => {
  const tab = (e.target as HTMLElement).closest<HTMLElement>("[data-i]");
  if (!tab) return;
  const i = Number(tab.dataset.i);
  const n = lista.length;
  const destino =
    e.key === "ArrowRight" ? ciclo(n, i, 1)
    : e.key === "ArrowLeft" ? ciclo(n, i, -1)
    : e.key === "Home" ? 0
    : e.key === "End" ? n - 1
    : null;
  if (destino !== null) {
    e.preventDefault();
    ativar(destino, true);
  } else if (e.key === "Delete" || e.key === "Backspace") {
    e.preventDefault();
    fecharAba(i);
    document.getElementById(`arq-${abaAtiva()!.chave}`)?.focus();
  }
});
$("nova-aba").addEventListener("click", novaAba);

// ---- Painel de entrada: o que é o arquivo e por onde começar. ----

type Conteudo = keyof typeof t.painel.conteudo;
interface Andamento {
  geracao: number; varridas: number; total: number; invalidas: number; fim: boolean; campos: number | null;
  legenda: Legenda | null; coloriu: boolean; inferiu: boolean;
}
interface Resumo extends Andamento { conteudo: Conteudo; bytes: number; registros: number }

/** Modos com cartão, na ordem dos cartões e dos atalhos 1, 2…; o sugerido é o primeiro disponível.
 * `sempre`: entra no seletor "Abrir sempre em…". Modo novo entra ligando `disponivel`. */
const MODOS: { id: Modo; icone: string; disponivel: boolean; sempre: boolean }[] = [
  { id: "tabela", icone: "table", disponivel: true, sempre: true },
  { id: "arvore", icone: "binary-tree", disponivel: true, sempre: true },
  { id: "texto", icone: "file-text", disponivel: true, sempre: false },
  { id: "estrutura", icone: "list-tree", disponivel: true, sempre: false },
  { id: "exportar", icone: "braces", disponivel: true, sempre: false },
];
const disponiveis = MODOS.filter((m) => m.disponivel);
/** Modo que o arquivo aceita agora; só a Tabela depende do arquivo (`null`: inferência em curso). */
const aceita = (m: Modo): boolean | null => (m === "tabela" ? grade.disponivel() : true);
/** Primeiro cartão disponível que o arquivo aceita; com a Tabela ainda indefinida, o tipo do conteúdo decide. */
const sugerido = (): Modo =>
  disponiveis.find((m) => aceita(m.id) ?? resumo?.conteudo === "objetos")?.id ?? "arvore";
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
  return p === "painel" ? "painel" : p === "sugerido" ? sugerido() : (p as Modo);
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
  grade.inferiu(); // documento já inferido: a Tabela diz se serve (e o painel redesenha)
  const sug = sugerido();
  painelModos.innerHTML = disponiveis
    .map((m, i) => {
      const [titulo, desc] = t.painel.modos[m.id];
      const s = m.id === sug;
      const nao = aceita(m.id) === false; // indisponível para este arquivo: o cartão diz por quê
      return `<button class="modo${s ? " sugerido" : ""}" type="button" data-modo="${m.id}" aria-keyshortcuts="${i + 1}"${nao ? " disabled" : ""}>
        ${icone(m.icone)}<b>${titulo}</b><span>${nao ? esc(grade.motivo() ?? desc) : desc}</span>${s ? `<em>${t.painel.sugerido}</em>` : ""}<kbd>${i + 1}</kbd></button>`;
    })
    .join("");
}

function mostrarPainel(): void {
  if (!resumo) return;
  exportar.parar();
  noPainel = true;
  painel.hidden = false;
  conteudo.hidden = true;
  for (const aba of Object.values(abas)) aba.setAttribute("aria-selected", "false");
  focar(painelModos.querySelector<HTMLElement>(".sugerido"));
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
  if (modo === "estrutura" || modo === "tabela") {
    const c = modo === "tabela" ? grade.caminho() : estr?.[selE]?.caminho;
    // Caminho da Estrutura já é texto jq (vem do núcleo): copia direto, sem `caminhos`.
    if (c) confirmarCopia(navigator.clipboard.writeText(c).then(() => c), "jq", copiar);
  } else if (atual) copiarComo(sel, "jq", copiar);
});
estruturaEl.addEventListener("keydown", teclaEstrutura);
estruturaLinhas.addEventListener("click", (e) => {
  const row = (e.target as HTMLElement).closest<HTMLElement>("[data-i]");
  if (row) moverE(Number(row.dataset.i));
  estruturaEl.focus({ preventScroll: true });
});
estruturaLinhas.addEventListener("dblclick", (e) => {
  if ((e.target as HTMLElement).closest("[data-i]")) void abrirOcorrencia();
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
  const ordem = Object.keys(abas) as Modo[];
  const m = ordem[(ordem.indexOf(modo) + (e.key === "ArrowRight" ? 1 : ordem.length - 1)) % ordem.length];
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
    if (noPainel) trocarModo(sugerido());
    consulta.focar();
    return;
  }
  if (cmd && !e.shiftKey && e.key.toLowerCase() === "f" && resumo) {
    e.preventDefault();
    if (noPainel) trocarModo(sugerido());
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
  // Abas de arquivo: ⌘T nova, ⌘W fecha, ⌘1…9 e Ctrl+Tab / Ctrl+Shift+Tab trocam.
  if (e.ctrlKey && !e.metaKey && !e.altKey && e.key === "Tab") {
    e.preventDefault();
    return ativar(ciclo(lista.length, ativa, e.shiftKey ? -1 : 1));
  }
  if (cmd && !e.shiftKey && e.key.toLowerCase() === "t") {
    e.preventDefault();
    return novaAba();
  }
  if (cmd && !e.shiftKey && e.key.toLowerCase() === "w") {
    e.preventDefault();
    return fecharAba(ativa);
  }
  // Visões: ⌥⌘1 Árvore, ⌥⌘2 Texto, ⌥⌘3 Estrutura, ⌥⌘4 Tabela, ⌥⌘5 Exportar (`code`: com ⌥ o `key` vira outro caractere).
  if ((e.metaKey || e.ctrlKey) && e.altKey && !e.shiftKey && resumo) {
    const m = ({ Digit1: "arvore", Digit2: "texto", Digit3: "estrutura", Digit4: "tabela", Digit5: "exportar" } as const)[e.code as "Digit1" | "Digit2" | "Digit3" | "Digit4" | "Digit5"];
    if (m) {
      e.preventDefault();
      trocarModo(m);
      visaoAtual().focus(); // pedido explícito: o foco vai mesmo saindo da barra de abas
    }
    return;
  }
  if (e.altKey || e.shiftKey) return;
  if (!(e.metaKey || e.ctrlKey)) {
    // No painel: 1, 2… abrem o cartão daquela posição; Enter fora de um botão abre o sugerido.
    const alvo = e.target as HTMLElement;
    if (!noPainel || alvo.closest("select, input, dialog")) return;
    const m =
      e.key === "Enter" && !alvo.closest("button") ? sugerido()
      : /^[1-9]$/.test(e.key) ? disponiveis[Number(e.key) - 1]?.id
      : undefined;
    if (m && aceita(m) === false) return; // cartão desligado: o atalho também
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
  const i = /^[1-9]$/.test(e.key) ? porAtalho(lista.length, Number(e.key)) : null;
  if (i === null) return;
  e.preventDefault();
  ativar(i);
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
void listen<Andamento & { id: number }>("varredura", ({ payload: p }) => {
  const a = lista.find((x) => x.doc === p.id);
  const ativo = a === abaAtiva();
  const r = ativo ? resumo : a?.resumo;
  // Aviso de arquivo anterior, ou mais velho que o `resumo` já recebido: ignora.
  if (!r || p.geracao !== r.geracao || (p.varridas < r.varridas && !p.fim)) return;
  Object.assign(r, p);
  if (!ativo) return;
  desenharAndamento();
  if (p.inferiu && modo === "estrutura" && !noPainel && !estr) void carregarEstrutura();
  if (p.inferiu && modo === "exportar" && !noPainel && !exportar.temCodigo()) void exportar.mostrar();
  regua.andamento(p);
});
void listen<Ordenacao>("ordenacao", ({ payload }) => grade.ordenacao(payload));
/** Seleciona o registro `r` na visão atual e põe o foco nela; `false` se não existir. */
async function focarRegistro(r: number): Promise<boolean> {
  if (modo === "estrutura" || modo === "tabela") trocarModo("arvore"); // Estrutura e Tabela: na Árvore
  if (semRegistros()) trocarModo("arvore");
  const v = await chamar<number | null>("ir_para", { registro: r });
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
abas.estrutura.firstChild!.textContent = t.estrutura.aba;
abas.tabela.firstChild!.textContent = t.tabela.aba;
tabelaEl.setAttribute("aria-label", t.tabela.rotulo);
$("tabela-ordem").querySelector("button")!.textContent = t.tabela.cancelar;
abas.exportar.firstChild!.textContent = t.exportar.aba;
exportarEl.setAttribute("aria-label", t.exportar.rotulo);
estruturaEl.setAttribute("aria-label", t.estrutura.rotulo);
$("abas").setAttribute("aria-label", t.valores);
listaEl.setAttribute("aria-label", t.abasArquivos.rotulo);
$("nova-aba").setAttribute("aria-label", t.abasArquivos.nova);
$("nova-aba").title = t.abasArquivos.nova;
$("sem-arquivo-titulo").textContent = t.abasArquivos.semArquivo;
$("sem-arquivo-dica").textContent = t.abasArquivos.dica;
$("sem-arquivo-abrir").textContent = t.abrir;
soltar.textContent = t.abasArquivos.soltar;
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

async function abrirDialogo(): Promise<void> {
  // O diálogo roda no Rust: o que o usuário escolhe fica concedido lá (ver `autorizado`).
  for (const c of await invoke<string[]>("escolher", { extensoes: EXTENSOES })) await abrirEmAba(c);
}
$("abrir").addEventListener("click", () => void abrirDialogo());
$("sem-arquivo-abrir").addEventListener("click", () => void abrirDialogo());

// Arrastar arquivos para a janela: cada aceito numa aba; o resto vira aviso no status, sem aba.
void getCurrentWebview().onDragDropEvent(async ({ payload: p }) => {
  soltar.hidden = p.type !== "enter" && p.type !== "over";
  if (p.type !== "drop" || !p.paths.length) return;
  const { abrir, recusados, pastas } = triar(p.paths, await invoke<boolean[]>("pastas", { caminhos: p.paths }));
  const nome = (c: string) => c.split(/[\\/]/).pop() ?? c;
  for (const c of abrir) await abrirEmAba(c);
  const avisos = [
    recusados.length ? t.abasArquivos.recusados(recusados.map(nome).join(", ")) : "",
    pastas.length ? t.abasArquivos.pastas(pastas.map(nome).join(", ")) : "",
  ].filter(Boolean);
  if (avisos.length) status.textContent = [abrir.length ? abaAtiva()?.status : "", ...avisos].filter(Boolean).join(" · ");
});

criarRecentes($("recentes"), (c) => void abrirPedido(c));

// Pedidos do sistema com o app aberto (Finder, "Abrir com", menu Arquivo › Abrir recente,
// 2ª instância): o aviso não leva caminho; cada um sai da fila uma vez, numa aba.
void listen("abrir-arquivos", async () => {
  for (const c of await invoke<string[]>("arquivos_pedidos")) await abrirPedido(c);
});

// Na abertura: os arquivos pedidos pelo sistema, cada um numa aba; sem nenhum, os exemplos embutidos.
lista = [abaVazia()];
ativar(0);
invoke<string[]>("arquivos_iniciais").then(async (pedidos) => {
  const iniciais = pedidos.length ? pedidos : EXEMPLOS;
  for (const c of iniciais) await (pedidos.length ? abrirPedido(c) : abrirEmAba(c));
  const j = lista.findIndex((a) => a.arquivo === iniciais[0]);
  if (j >= 0 && j !== ativa) ativar(j);
});
