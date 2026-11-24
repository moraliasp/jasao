// Filtros jq salvos: lista guardada no `localStorage` desta máquina. O que vem de lá é tratado
// como não confiável (outra versão do app, edição à mão): só textos, com teto de tamanho e de
// quantidade.
export const CHAVE_FILTROS = "jasao.filtros";
export const MAX_FILTROS = 50;
/** O mesmo teto do Rust (`MAX_FILTRO`, em bytes UTF-8). */
export const MAX_TAMANHO = 16 << 10;

const valido = (f: unknown): f is string =>
  typeof f === "string" && f.trim() !== "" && new TextEncoder().encode(f).length <= MAX_TAMANHO;

/** Lê a lista guardada; qualquer coisa fora do formato vira lista vazia ou é descartada. */
export function lerFiltros(bruto: string | null): string[] {
  let v: unknown;
  try {
    v = JSON.parse(bruto ?? "[]");
  } catch {
    return [];
  }
  if (!Array.isArray(v)) return [];
  return [...new Set(v.filter(valido))].slice(0, MAX_FILTROS);
}

/** Salva `f` no topo, ou tira se já estava. Filtro vazio ou grande demais não entra. */
export function alternarFiltro(lista: string[], f: string): string[] {
  if (lista.includes(f)) return lista.filter((x) => x !== f);
  return valido(f) ? [f, ...lista].slice(0, MAX_FILTROS) : lista;
}
