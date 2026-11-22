// Arquivos recentes na aba vazia. A lista e o menu Arquivo › Abrir recente moram no Rust
// (src-tauri/src/recentes.rs); aqui só se desenha a lista, refeita a cada aviso `recentes`.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "./i18n";

export interface Recente {
  caminho: string;
  nome: string;
  pasta: string;
}

/** Desenha os recentes em `el` (botão com nome e pasta por recente) e chama `escolher` no clique. */
export function criarRecentes(el: HTMLElement, escolher: (caminho: string) => void): void {
  el.setAttribute("aria-label", t.recentes.titulo);
  async function desenhar(): Promise<void> {
    const lista = await invoke<Recente[]>("recentes");
    el.hidden = !lista.length;
    el.replaceChildren();
    if (!lista.length) return;
    const h = document.createElement("h2");
    h.textContent = t.recentes.titulo;
    const ul = document.createElement("ul");
    for (const r of lista) {
      const b = document.createElement("button");
      b.type = "button";
      b.title = r.caminho;
      const nome = document.createElement("span");
      nome.textContent = r.nome;
      const pasta = document.createElement("span");
      pasta.className = "pasta";
      pasta.textContent = r.pasta;
      b.append(nome, pasta);
      b.addEventListener("click", () => escolher(r.caminho));
      const li = document.createElement("li");
      li.append(b);
      ul.append(li);
    }
    el.append(h, ul);
  }
  void listen("recentes", () => void desenhar());
  void desenhar();
}
