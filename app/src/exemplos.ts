// Exemplos embutidos: o app sem arquivo abre os dois, cada um na sua aba.
// Vêm do binário por caminho sintético (`exemplo:<nome>`, ver src-tauri/src/exemplos.rs).
import { t } from "./i18n";

const PREFIXO = "exemplo:";
const NOMES = ["historia", "vitrine"] as const;

/** Caminhos dos exemplos, na ordem das abas. */
export const EXEMPLOS = NOMES.map((n) => PREFIXO + n);

/** Nome de aba de um exemplo (diz que é exemplo); `null` se o caminho não for de exemplo. */
export function nomeExemplo(caminho: string): string | null {
  const n = NOMES.find((x) => PREFIXO + x === caminho);
  return n ? t.exemplos.nome(t.exemplos[n]) : null;
}
