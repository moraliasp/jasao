// Valores inteligentes na tela: o painel do valor selecionado e o clique em link. A detecção é de
// `valores.ts` e o HTML, de `apresenta.ts` (puro e testado); aqui se busca o valor inteiro quando a
// prévia veio cortada e se abre link pelo Rust, que manda ao navegador do sistema.
import { invoke } from "@tauri-apps/api/core";
import { chamar } from "./doc";
import { daPrevia, html, type No } from "./apresenta";
import { t } from "./i18n";
import { visivel, visivelTexto } from "./texto";
import { detectar, type Inteligente, MAX_BASE64 } from "./valores";

export { type No, selo } from "./apresenta";

/** Valor completo do nó `v` quando a prévia veio cortada (dentro do teto de base64). */
async function completo(l: No, v: number): Promise<Inteligente | null> {
  if (l.faixa[1] - l.faixa[0] > MAX_BASE64 + 2) return null; // +2: as aspas
  const s = await chamar<string | null>("valor", { v }).catch(() => null);
  return s === null ? null : detectar("texto", s, l.chave);
}

/** Abre no navegador do sistema; o Rust recusa o que não for http(s). */
const abrir = (url: string) => invoke("abrir_link", { url });

/** Clique no selo de link da linha `v` (a prévia pode ter vindo cortada: pede o valor inteiro). */
export async function abrirLinkDe(l: No, v: number, avisar: (m: string) => void): Promise<void> {
  const d = daPrevia(l);
  const u = typeof d === "string" ? await completo(l, v) : d;
  if (u?.tipo === "url") await abrir(u.href).catch((e) => avisar(t.inteligente.erroLink(String(e))));
  else avisar(t.inteligente.naoLink);
}

/**
 * Painel do valor selecionado, abaixo da árvore. `aoRedimensionar`: o painel mudou de altura e a
 * árvore também (manter a seleção à vista).
 */
export function criarDetalhe(el: HTMLElement, avisar: (m: string) => void, aoRedimensionar: () => void) {
  let de = ""; // doc:versão:linha mostrada; descarta resposta de seleção antiga
  let oculto = -1; // linha mascarada mostrada agora (o "Mostrar valor" revela esta)
  el.setAttribute("aria-label", t.inteligente.rotulo);
  el.addEventListener("click", (e) => {
    const alvo = e.target as HTMLElement;
    if (alvo.closest("[data-revelar]")) return void revelar(de, oculto);
    const b = alvo.closest<HTMLElement>("[data-link]");
    if (b?.dataset.link) void abrir(b.dataset.link).catch((err) => avisar(t.inteligente.erroLink(String(err))));
  });

  /** Segredo mascarado: aviso e, na folha, botão; o valor original só vem no clique, campo a
   * campo (contêiner não revela: o backend também recusa). */
  function mascarado(v: number, folha: boolean): void {
    oculto = v;
    el.hidden = false;
    el.innerHTML = `<div class="rot">${visivel(t.mascara.rotulo)}</div><p>${visivel(t.mascara.explica)}</p>` +
      (folha ? `<button class="btn" type="button" data-revelar>${visivel(t.mascara.mostrar)}</button>` : "");
  }
  /** Busca o valor cru e mostra como texto (nunca HTML); nada guarda: outra seleção mascara de novo. */
  async function revelar(chave: string, v: number): Promise<void> {
    const s = await chamar<string | null>("valor_cru", { v }).catch((e) => (avisar(`${t.erro}: ${String(e)}`), null));
    if (s === null || chave !== de) return;
    const pre = document.createElement("pre");
    pre.textContent = visivelTexto(s);
    el.querySelector("[data-revelar]")?.replaceWith(pre);
  }

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
      oculto = -1;
      if (l?.mascarado) return mascarado(v, l.tipo !== "objeto" && l.tipo !== "lista" && !l.erro);
      const d = l ? daPrevia(l) : null;
      if (typeof d !== "string") return desenhar(d);
      desenhar(null);
      const c = await completo(l!, v);
      if (de === chave) desenhar(c);
    },
  };
}
