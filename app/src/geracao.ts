/** Eventos de uma tarefa do Rust numerada por geração. Passa o evento da geração atual; o de uma
 * geração mais nova fica guardado até ela ser fixada, porque em arquivo pequeno o `fim` chega antes
 * do comando devolver o número da geração. Geração 0: nenhuma (descarta). */
export function porGeracao<T extends { geracao: number }>(aplicar: (v: T) => void) {
  let atual = 0;
  let adiantado: T | null = null;
  return {
    receber(v: T): void {
      if (v.geracao > atual) adiantado = v;
      else if (v.geracao === atual && atual) aplicar(v);
    },
    fixar(g: number): void {
      atual = g;
      const v = adiantado;
      adiantado = null;
      if (g && v?.geracao === g) aplicar(v);
    },
    get atual(): number {
      return atual;
    },
  };
}
