// Valores inteligentes na tela: o selo na linha da árvore e o painel do valor selecionado.
// A detecção é de `valores.ts`; aqui só se monta o HTML (todo texto do arquivo passa por
// `visivel`/`escAtributo`) e se abre link pelo Rust, que manda ao navegador do sistema.
import { invoke } from "@tauri-apps/api/core";
import { chamar } from "./doc";
import { t, tamanho } from "./i18n";
import { escAtributo as esc, visivel } from "./texto";
import { detectar, type Inteligente, MAX_BASE64, type Pista, pista, relativo } from "./valores";

/** Igual a `nucleo::arvore::MAX_PREVIA`: valor maior chega cortado na prévia. */
const MAX_PREVIA = 200;

/** O que o selo e o painel precisam de uma linha da árvore. */
export interface No {
  tipo: string;
  previa: string;
  chave: string | null;
  faixa: [number, number];
  erro: unknown;
}

const icone = (id: string) => `<svg class="i" aria-hidden="true"><use href="#${id}"/></svg>`;
const dataLocal = (ms: number) => new Date(ms).toLocaleString(t.lang, { dateStyle: "short", timeStyle: "medium" });
const texto = (l: No) => l.tipo === "texto" || l.tipo === "numero";

/** Detecção pela prévia: completa se o valor coube nela, senão só a pista pelo começo. */
function daPrevia(l: No): Inteligente | Pista | null {
  if (l.erro || !texto(l)) return null;
  if (l.faixa[1] - l.faixa[0] > MAX_PREVIA) return l.tipo === "texto" ? pista(l.previa.slice(1)) : null;
  if (l.tipo === "numero") return detectar("numero", l.previa, l.chave);
  try {
    return detectar("texto", JSON.parse(l.previa), l.chave);
  } catch {
    return null;
  }
}

/** Valor completo do nó `v` quando a prévia veio cortada (dentro do teto de base64). */
async function completo(l: No, v: number): Promise<Inteligente | null> {
  if (l.faixa[1] - l.faixa[0] > MAX_BASE64 + 2) return null; // +2: as aspas
  const s = await chamar<string | null>("valor", { v }).catch(() => null);
  return s === null ? null : detectar("texto", s, l.chave);
}

/** Selo ao lado do valor na árvore ("" se não houver). */
export function selo(l: No): string {
  const d = daPrevia(l);
  if (!d) return "";
  const k = typeof d === "string" ? d : d.tipo;
  const link = `<button class="lk" type="button" tabindex="-1" data-link>${icone("link")}${t.inteligente.abrirCurto}</button>`;
  const corpo: Record<typeof k, () => string> = {
    epoca: () => `${icone("cal")}${esc(dataLocal((d as { ms: number }).ms))}`,
    data: () => `${icone("cal")}${esc(relativo((d as { ms: number }).ms, Date.now(), t.lang))}`,
    cor: () => {
      const c = (d as { css: string }).css;
      return `<span class="sw" role="img" aria-label="${esc(t.inteligente.amostra(c))}" style="background:${esc(c)}"></span>`;
    },
    url: () => link,
    imagem: () => `${icone("image")}${t.inteligente.imagemCurta}`,
    jwt: () => `${icone("key")}JWT`,
    json: () => `${icone("braces")}${t.inteligente.jsonCurto}`,
  };
  return `<span class="smart">${corpo[k]()}</span>`;
}

/** Abre no navegador do sistema; o Rust recusa o que não for http(s). */
const abrir = (url: string) => invoke("abrir_link", { url });

/** Clique no selo de link da linha `v` (a prévia pode ter vindo cortada: pede o valor inteiro). */
export async function abrirLinkDe(l: No, v: number, avisar: (m: string) => void): Promise<void> {
  const d = daPrevia(l);
  const u = typeof d === "string" ? await completo(l, v) : d;
  if (u?.tipo === "url") await abrir(u.href).catch((e) => avisar(t.inteligente.erroLink(String(e))));
}

function html(d: Inteligente): string {
  const T = t.inteligente;
  const rot = (s: string) => `<div class="rot">${esc(s)}</div>`;
  const pre = (s: string, rotulo: string) => `<pre tabindex="0" aria-label="${esc(rotulo)}">${s}</pre>`;
  switch (d.tipo) {
    case "epoca":
    case "data": {
      const quando = `${new Date(d.ms).toISOString()} (${T.utc})\n${dataLocal(d.ms)} (${T.local})\n${relativo(d.ms, Date.now(), t.lang)}`;
      const r = d.tipo === "epoca" ? T.epoca(d.unidade) : T.data;
      return rot(r) + pre(esc(quando), r);
    }
    case "cor":
      return rot(T.cor) + `<span class="smart"><span class="sw" role="img" aria-label="${esc(T.amostra(d.css))}" style="background:${esc(d.css)}"></span>${esc(d.css)}</span>`;
    case "url":
      return rot(T.url) + `<button class="lk" type="button" data-link="${esc(d.href)}" aria-label="${esc(T.abrir(d.href))}">${icone("link")}<span>${esc(d.href)}</span></button>`;
    case "imagem": {
      const desc = T.previaImagem(d.formato.toUpperCase().replace("+XML", ""), tamanho(d.bytes));
      return rot(T.imagem) + `<figure><img data-src alt="${esc(desc)}"><figcaption>${esc(desc)}</figcaption></figure>`;
    }
    case "jwt":
      return rot(T.jwt) + `<div class="par"><div>${rot(T.cabecalho)}${pre(visivel(d.cabecalho), T.cabecalho)}</div><div>${rot(T.carga)}${pre(visivel(d.carga), T.carga)}</div></div>`;
    case "json": {
      const r = T.json(T.itens(d.itens));
      return rot(r) + pre(visivel(d.texto), r);
    }
  }
}

/** Painel do valor selecionado, abaixo da árvore. */
/** `aoRedimensionar`: o painel mudou de altura e a árvore também (manter a seleção à vista). */
export function criarDetalhe(el: HTMLElement, avisar: (m: string) => void, aoRedimensionar: () => void) {
  let de = ""; // doc:versão:linha mostrada; descarta resposta de seleção antiga
  el.setAttribute("aria-label", t.inteligente.rotulo);
  el.addEventListener("click", (e) => {
    const b = (e.target as HTMLElement).closest<HTMLElement>("[data-link]");
    if (b?.dataset.link) void abrir(b.dataset.link).catch((err) => avisar(t.inteligente.erroLink(String(err))));
  });

  function desenhar(d: Inteligente | null): void {
    el.hidden = !d;
    el.innerHTML = d ? html(d) : "";
    // `src` pela propriedade, nunca no HTML: é sempre data URI base64 conferido em `valores.ts`.
    if (d?.tipo === "imagem") el.querySelector<HTMLImageElement>("img[data-src]")!.src = d.src;
  }
  // Observa a altura em vez de avisar no `desenhar`: a imagem só ganha tamanho depois de carregar.
  new ResizeObserver(aoRedimensionar).observe(el);

  return {
    /** Mostra o valor da linha `v` (ou esconde). `chave` muda quando a seleção muda. */
    async mostrar(chave: string, l: No | undefined, v: number): Promise<void> {
      if (chave === de) return;
      de = chave;
      const d = l ? daPrevia(l) : null;
      if (typeof d !== "string") return desenhar(d);
      desenhar(null);
      const c = await completo(l!, v);
      if (de === chave) desenhar(c);
    },
  };
}
