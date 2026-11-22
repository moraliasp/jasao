// Lógica pura das abas de arquivo (sem DOM nem Tauri; testada em tests/abas.test.ts).

/** Extensões que se abrem por arraste: JSON, JSON Lines e as mesmas compactadas. */
export const EXTENSOES = ["json", "jsonl", "ndjson", "gz", "zst"];
const ACEITO = /\.(json|jsonl|ndjson)(\.(gz|zst))?$/i;

export const aceito = (caminho: string) => ACEITO.test(caminho);

/** Índice da aba nova: logo depois da ativa (ou no fim, sem ativa). */
export const posicaoNova = (n: number, ativa: number) => (ativa < 0 ? n : ativa + 1);

/** Ativa depois de fechar a aba `i`: a vizinha da direita, senão a da esquerda; -1 sem abas.
 * Fechar outra aba que não a ativa só corrige o índice da ativa. */
export function aposFechar(n: number, i: number, ativa: number): number {
  if (n <= 1) return -1;
  if (i !== ativa) return ativa > i ? ativa - 1 : ativa;
  return i < n - 1 ? i : i - 1;
}

/** Próxima aba em `delta` (Ctrl+Tab = 1, Ctrl+Shift+Tab = -1), dando a volta nas pontas. */
export const ciclo = (n: number, ativa: number, delta: number) => (n ? (((ativa + delta) % n) + n) % n : -1);

/** Cmd+1…8 vão à aba daquela posição; Cmd+9 vai à última (como nos navegadores). */
export function porAtalho(n: number, digito: number): number | null {
  if (digito < 1 || digito > 9 || !n) return null;
  if (digito === 9) return n - 1;
  return digito <= n ? digito - 1 : null;
}

const partes = (c: string) => c.split(/[\\/]/).filter(Boolean);

/** Nome de cada aba: o do arquivo; quando dois se chamam igual, cada um ganha as pastas acima
 * até se distinguirem (`dados.jsonl — a`, `dados.jsonl — b`). `null` é aba vazia. */
export function nomes(caminhos: (string | null)[], vazia: string): string[] {
  const base = caminhos.map((c) => (c === null ? null : (partes(c).pop() ?? c)));
  return caminhos.map((c, i) => {
    const b = base[i];
    if (c === null || b === null) return vazia;
    const iguais = caminhos.filter((o, j) => o !== null && o !== c && base[j] === b) as string[];
    if (!iguais.length) return b;
    const meu = partes(c).slice(0, -1);
    const outros = iguais.map((o) => partes(o).slice(0, -1));
    for (let k = 1; k <= meu.length; k++) {
      const fim = meu.slice(-k).join("/");
      if (outros.every((o) => o.slice(-k).join("/") !== fim)) return `${b} — ${fim}`;
    }
    return `${b} — ${meu.join("/") || "/"}`;
  });
}

/** Arquivos soltos na janela, separados pelo que fazer com cada um. */
export function triar(caminhos: string[], pastas: boolean[]) {
  const abrir: string[] = [];
  const recusados: string[] = [];
  const pastasSoltas: string[] = [];
  caminhos.forEach((c, i) => (pastas[i] ? pastasSoltas : aceito(c) ? abrir : recusados).push(c));
  return { abrir, recusados, pastas: pastasSoltas };
}
