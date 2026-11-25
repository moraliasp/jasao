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
      // O de uma geração ainda mais nova continua guardado (respostas fora de ordem).
      if (!g || !v || v.geracao <= g) adiantado = null;
      if (g && v?.geracao === g) aplicar(v);
    },
    get atual(): number {
      return atual;
    },
  };
}

/** Se um evento de consulta vale para o estado `e`. Não vale se não há consulta rodando
 * (cancelada, ou outro arquivo aberto: o `fim` emitido no mesmo instante não abre), se é de geração
 * velha, ou se `consultar` está em voo e o evento ainda é da geração atual. */
export const eventoVale = (
  e: { rodando: boolean; esperando: boolean; geracao: number } | undefined,
  geracao: number,
): boolean => !!e && e.rodando && geracao >= e.geracao && !(e.esperando && geracao === e.geracao);
