// Copiar caminho (jq, JSONPath, JSON Pointer) e valor do nó. Os textos vêm do núcleo
// (`caminhos`, `valor`); aqui ficam a escrita na área de transferência e o menu de contexto.
import { chamar } from "./doc";

export type Formato = "jq" | "jsonpath" | "pointer" | "valor";
export const FORMATOS: Formato[] = ["jq", "jsonpath", "pointer", "valor"];

/** Texto do nó visível `v` no formato pedido. */
async function texto(v: number, formato: Formato): Promise<string> {
  const s =
    formato === "valor"
      ? await chamar<string | null>("valor", { v })
      : (await chamar<Record<string, string> | null>("caminhos", { v }))?.[formato];
  if (s == null) throw new Error(`linha ${v} fora da lista`);
  return s;
}

/**
 * Copia o texto do nó `v`. Tem de ser chamada direto no clique ou na tecla: o WebKit só aceita
 * a escrita dentro do gesto, e o texto chega depois do `invoke`. Por isso `ClipboardItem` com
 * promessa (a escrita começa já, o conteúdo vem depois); `writeText` fica de reserva.
 */
export async function copiar(v: number, formato: Formato): Promise<string> {
  const p = texto(v, formato);
  try {
    await navigator.clipboard.write([new ClipboardItem({ "text/plain": p.then((s) => new Blob([s], { type: "text/plain" })) })]);
  } catch {
    await navigator.clipboard.writeText(await p); // erro do núcleo, se houver, sobe daqui
  }
  return p;
}

/** Copia um texto que já está na interface (código gerado no Exportar), dentro do gesto. */
export const copiarTexto = (s: string): Promise<void> => navigator.clipboard.writeText(s);

/** Menu de contexto acessível: setas, Home/End, Enter, Esc ou Tab fecham e devolvem o foco. */
export function criarMenu(el: HTMLElement, escolher: (f: Formato) => void) {
  const itens = [...el.querySelectorAll<HTMLButtonElement>("[role=menuitem]")];
  let volta: HTMLElement | null = null;

  function fechar(focar = true): void {
    if (el.hidden) return;
    el.hidden = true;
    if (focar) volta?.focus({ preventScroll: true });
  }

  el.addEventListener("click", (e) => {
    const f = (e.target as HTMLElement).closest<HTMLElement>("[data-formato]")?.dataset.formato as Formato | undefined;
    if (!f) return;
    fechar();
    escolher(f); // ainda dentro do clique: a escrita na área de transferência vale
  });
  el.addEventListener("keydown", (e) => {
    const i = itens.indexOf(document.activeElement as HTMLButtonElement);
    const ir = (j: number) => itens[(j + itens.length) % itens.length].focus();
    const acoes: Record<string, () => void> = {
      ArrowDown: () => ir(i + 1),
      ArrowUp: () => ir(i - 1),
      Home: () => ir(0),
      End: () => ir(-1),
      Escape: () => fechar(),
      Tab: () => fechar(),
    };
    const a = acoes[e.key];
    if (!a) return;
    e.preventDefault();
    e.stopPropagation();
    a();
  });
  document.addEventListener("pointerdown", (e) => {
    if (!el.contains(e.target as Node)) fechar(false);
  });
  window.addEventListener("blur", () => fechar(false));

  return {
    /** Abre em (x, y) da janela, sem passar da borda; o foco volta para `de` ao fechar. */
    abrir(x: number, y: number, de: HTMLElement): void {
      volta = de;
      el.hidden = false;
      const { width, height } = el.getBoundingClientRect();
      el.style.left = `${Math.max(4, Math.min(x, innerWidth - width - 4))}px`;
      el.style.top = `${Math.max(4, Math.min(y, innerHeight - height - 4))}px`;
      itens[0].focus();
    },
    fechar,
  };
}
