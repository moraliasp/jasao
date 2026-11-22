// Arquivos recentes: lista na tela sem arquivo e menu Arquivo › Abrir recente. A lista mora no
// Rust (recentes.json, ver src-tauri/src/recentes.rs); aqui só se mostra e se escolhe.
import { invoke } from "@tauri-apps/api/core";
import { Menu, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import { t } from "./i18n";

export interface Recente {
  caminho: string;
  nome: string;
  pasta: string;
}

/**
 * `abrir` devolve se abriu; recente que falhou e não existe mais sai da lista com `avisar`.
 * `mudou` é chamado com a lista nova sempre que ela muda (para redesenhar a tela sem arquivo).
 */
export function criarRecentes(
  abrir: (caminho: string) => Promise<boolean>,
  avisar: (texto: string) => void,
  mudou: (lista: Recente[]) => void,
) {
  let atual: Recente[] = [];

  async function escolher(r: Recente): Promise<void> {
    if (await abrir(r.caminho)) return;
    if (await invoke<boolean>("recente_ausente", { caminho: r.caminho })) {
      avisar(t.recentes.sumiu(r.nome));
      await atualizar();
    }
  }

  async function limpar(): Promise<void> {
    await invoke("limpar_recentes");
    await atualizar();
  }

  // shortcut: refaz o menu inteiro a cada mudança (≤ 10 itens); remendar se o menu crescer.
  async function menu(): Promise<void> {
    const itens = atual.map((r) => ({
      text: `${r.nome} — ${r.pasta}`,
      action: () => void escolher(r),
    }));
    const sub = await Submenu.new({
      text: t.recentes.menu,
      items: [
        ...(itens.length ? itens : [{ text: t.recentes.vazio, enabled: false }]),
        await PredefinedMenuItem.new({ item: "Separator" }),
        { text: t.recentes.limpar, enabled: itens.length > 0, action: () => void limpar() },
      ],
    });
    const barra = await Menu.default();
    // O menu padrão do Tauri tem "File" (macOS: 2º item, depois do menu do app).
    let arquivo: Submenu | undefined;
    for (const item of await barra.items()) {
      if (item instanceof Submenu && (await item.text()) === "File") arquivo = item;
    }
    if (arquivo) {
      await arquivo.setText(t.recentes.arquivo);
      await arquivo.insert(sub, 0);
    } else {
      await barra.append(await Submenu.new({ text: t.recentes.arquivo, items: [sub] }));
    }
    await barra.setAsAppMenu();
  }

  async function atualizar(): Promise<void> {
    atual = await invoke<Recente[]>("recentes");
    mudou(atual);
    await menu();
  }

  /** Lista para a tela sem arquivo: botão com nome e pasta por recente. */
  function lista(): HTMLElement {
    const el = document.createElement("nav");
    el.className = "recentes";
    el.setAttribute("aria-label", t.recentes.titulo);
    if (!atual.length) {
      el.hidden = true;
      return el;
    }
    const h = document.createElement("h2");
    h.textContent = t.recentes.titulo;
    const ul = document.createElement("ul");
    for (const r of atual) {
      const b = document.createElement("button");
      b.type = "button";
      b.title = r.caminho;
      const nome = document.createElement("span");
      nome.className = "nome";
      nome.textContent = r.nome;
      const pasta = document.createElement("span");
      pasta.className = "pasta";
      pasta.textContent = r.pasta;
      b.append(nome, pasta);
      b.addEventListener("click", () => void escolher(r));
      const li = document.createElement("li");
      li.append(b);
      ul.append(li);
    }
    el.append(h, ul);
    return el;
  }

  return { atualizar, lista, escolher };
}
