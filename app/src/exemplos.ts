// Barra dos exemplos embutidos: aparece quando o app abre sem arquivo e some ao abrir um de verdade.
// Os exemplos vêm do binário por caminho sintético (`exemplo:<nome>`, ver src-tauri/src/exemplos.rs).
import { t } from "./i18n";

const PREFIXO = "exemplo:";
const NOMES = ["historia", "vitrine"] as const;

export function criarExemplos(mostrar: (caminho: string) => Promise<void>, abrirProprio: () => void) {
  const barra = document.createElement("div");
  barra.className = "exemplos";
  barra.setAttribute("role", "region");
  barra.setAttribute("aria-label", t.exemplos.rotulo);
  barra.hidden = true;
  const aviso = document.createElement("span");
  aviso.className = "aviso";
  aviso.textContent = t.exemplos.aviso;
  const grupo = document.createElement("div");
  grupo.className = "alternador";
  grupo.setAttribute("role", "group");
  grupo.setAttribute("aria-label", t.exemplos.alternar);
  const botoes = NOMES.map((n) => {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = t.exemplos[n];
    b.setAttribute("aria-pressed", "false");
    b.addEventListener("click", () => void mostrar(PREFIXO + n));
    grupo.append(b);
    return b;
  });
  const proprio = document.createElement("button");
  proprio.type = "button";
  proprio.className = "btn";
  proprio.textContent = t.exemplos.abrirProprio;
  proprio.addEventListener("click", abrirProprio);
  barra.append(aviso, grupo, proprio);
  document.querySelector("header.top")!.after(barra);

  return {
    /** Nome a mostrar no topo; o de um exemplo diz que é exemplo. */
    nome(caminho: string): string {
      const n = NOMES.find((x) => PREFIXO + x === caminho);
      return n ? t.exemplos.nome(t.exemplos[n]) : (caminho.split(/[\\/]/).pop() ?? caminho);
    },
    /** Depois de abrir: barra visível só em exemplo, com o botão do aberto marcado. */
    aberto(caminho: string): void {
      barra.hidden = !caminho.startsWith(PREFIXO);
      NOMES.forEach((n, i) => botoes[i].setAttribute("aria-pressed", String(PREFIXO + n === caminho)));
    },
  };
}
