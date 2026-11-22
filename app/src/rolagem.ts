// Rolagem virtual compartilhada pelas visões: o elemento só tem a altura da tela mais um
// espaçador, e a linha visível sai do scrollTop.

// Navegadores limitam a altura de um elemento (~33 milhões de px). Acima disto a barra de
// rolagem passa a ser proporcional em vez de 1 px = 1 px.
export const MAX_PX = 8_000_000;

/** Primeira linha visível, deslocamento em px dentro dela e quantas linhas cabem. */
export function geometria(el: HTMLElement, total: number, altura: number) {
  const vista = el.clientHeight;
  const totalPx = total * altura;
  const altoPx = Math.min(totalPx, MAX_PX);
  const escala = altoPx > vista ? (totalPx - vista) / (altoPx - vista) : 1;
  const px = el.scrollTop * escala;
  const primeira = Math.floor(px / altura);
  return { vista, escala, primeira, resto: px - primeira * altura, cabem: Math.ceil(vista / altura) + 1 };
}

/** Altura do espaçador que dá à barra de rolagem o tamanho de `total` linhas. */
export const espaco = (el: HTMLElement, total: number, altura: number) =>
  `${Math.max(0, Math.min(total * altura, MAX_PX) - el.clientHeight)}px`;

/** Primeira e última linha inteiras na tela (a cortada em cima ou embaixo não conta). */
export function inteiras(g: { primeira: number; resto: number; vista: number }, total: number, altura: number): [number, number] {
  const a = Math.min(total - 1, g.primeira + (g.resto > 0.5 ? 1 : 0));
  return [a, Math.max(a, Math.min(total, g.primeira + Math.floor((g.resto + g.vista) / altura)) - 1)];
}

/** Rola até a linha `v` ficar no topo. */
export function rolarPara(el: HTMLElement, v: number, total: number, altura: number): void {
  el.scrollTop = Math.max(0, (v * altura) / geometria(el, total, altura).escala);
}

/** Rola o mínimo para a linha `v` ficar inteira na tela. */
export function mostrarLinha(el: HTMLElement, v: number, total: number, altura: number): void {
  const { vista, escala, primeira, cabem } = geometria(el, total, altura);
  let px: number | null = null;
  if (v <= primeira) px = v * altura;
  else if (v >= primeira + cabem - 2) px = (v + 1) * altura - vista;
  if (px !== null) el.scrollTop = Math.max(0, px / escala);
}
