// Valores inteligentes em HTML: o selo na linha da árvore e o corpo do painel do valor. Puro (sem
// Tauri nem DOM), para os testes provarem que todo texto do arquivo sai escapado: `visivel` no
// conteúdo, `escAtributo` em atributo. Quem põe na tela e abre link é `detalhe.ts`.
import { t, tamanho } from "./i18n.ts";
import { escAtributo as esc, visivel } from "./texto.ts";
import { detectar, hostDe, type Inteligente, type Pista, pista, relativo } from "./valores.ts";

/** Igual a `nucleo::arvore::MAX_PREVIA`: valor maior chega cortado na prévia. */
export const MAX_PREVIA = 200;

/** O que o selo e o painel precisam de uma linha da árvore. */
export interface No {
  tipo: string;
  previa: string;
  chave: string | null;
  faixa: [number, number];
  erro: unknown;
  /** Tem trecho mascarado (segredo): o valor só aparece pelo "Mostrar valor". */
  mascarado?: boolean;
}

const icone = (id: string) => `<svg class="i" aria-hidden="true"><use href="#${id}"/></svg>`;
const dataLocal = (ms: number) => new Date(ms).toLocaleString(t.lang, { dateStyle: "short", timeStyle: "medium" });
const bytes = (l: No) => l.faixa[1] - l.faixa[0];

/** Começo do texto numa prévia cortada: sem a aspa de abertura e sem o `…` que o núcleo põe no fim. */
const inicio = (l: No) => l.previa.slice(1, l.previa.endsWith("…") ? -1 : undefined);

/** Detecção pela prévia: completa se o valor coube nela, senão só a pista pelo começo. */
export function daPrevia(l: No): Inteligente | Pista | null {
  if (l.erro || (l.tipo !== "texto" && l.tipo !== "numero")) return null;
  if (bytes(l) > MAX_PREVIA) return l.tipo === "texto" ? pista(inicio(l), bytes(l)) : null;
  if (l.tipo === "numero") return detectar("numero", l.previa, l.chave);
  try {
    return detectar("texto", JSON.parse(l.previa), l.chave);
  } catch {
    return null;
  }
}

/**
 * Botão do selo de link: mostra o host de destino e o endereço no `title`. Na prévia cortada, o
 * host só aparece se terminou dentro dela (senão o que se vê não seria o que o clique abre).
 */
function botaoLink(endereco: string, cortado: boolean): string {
  const host = !cortado || /^https?:\/\/[^/?#]+[/?#]/.test(endereco) ? hostDe(endereco) : "";
  if (cortado) endereco += "…";
  return `<button class="lk" type="button" tabindex="-1" data-link title="${esc(endereco)}" aria-label="${esc(t.inteligente.abrir(endereco))}">${icone("link")}${esc(host || t.inteligente.abrirCurto)}</button>`;
}

/** Selo ao lado do valor na árvore ("" se não houver ou se não deu para confirmar). */
export function selo(l: No): string {
  const d = daPrevia(l);
  if (!d || d === "json") return "";
  let corpo: string;
  if (d === "url") corpo = botaoLink(inicio(l), true); // `d` só é pista com a prévia cortada
  else if (d === "imagem") corpo = `${icone("image")}${t.inteligente.imagemCurta}`;
  else if (d === "jwt") corpo = `${icone("key")}JWT`;
  else
    switch (d.tipo) {
      case "epoca":
        corpo = `${icone("cal")}${esc(dataLocal(d.ms))}`;
        break;
      case "data":
        corpo = `${icone("cal")}${esc(relativo(d.ms, Date.now(), t.lang))}`;
        break;
      case "cor":
        corpo = `<span class="sw" role="img" aria-label="${esc(t.inteligente.amostra(d.css))}" style="background:${esc(d.css)}"></span>`;
        break;
      case "url":
        corpo = botaoLink(d.href, false);
        break;
      case "imagem":
        corpo = `${icone("image")}${t.inteligente.imagemCurta}`;
        break;
      case "jwt":
        corpo = `${icone("key")}JWT`;
        break;
      case "json":
        corpo = `${icone("braces")}${t.inteligente.jsonCurto}`;
        break;
    }
  return `<span class="smart">${corpo}</span>`;
}

/** Corpo do painel do valor. A imagem sai com `data-src`: o `src` é posto pela propriedade. */
export function html(d: Inteligente): string {
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
