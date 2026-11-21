// Fase 0: prova o caminho de ponta a ponta. O Rust guarda o arquivo e o índice; aqui só se
// pede uma janela de linhas, que chega como ArrayBuffer (linhas separadas por "\n").
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

const JANELA = 200;
const status = document.getElementById("status")!;
const lista = document.getElementById("linhas") as HTMLOListElement;
const decodificador = new TextDecoder(); // UTF-8 inválido vira U+FFFD, sem exceção

async function mostrar(caminho: string): Promise<void> {
  status.textContent = `Abrindo ${caminho}…`;
  lista.replaceChildren();
  try {
    const total = await invoke<number>("abrir", { caminho });
    const quantidade = Math.min(JANELA, total);
    const bytes = await invoke<ArrayBuffer>("janela", { inicio: 0, quantidade });
    const linhas = quantidade === 0 ? [] : decodificador.decode(bytes).split("\n");
    lista.replaceChildren(
      ...linhas.map((texto) => {
        const li = document.createElement("li");
        li.textContent = texto;
        return li;
      }),
    );
    status.textContent = `${linhas.length.toLocaleString("pt-BR")} de ${total.toLocaleString("pt-BR")} linhas · ${caminho}`;
  } catch (erro) {
    status.textContent = `Erro: ${erro}`;
  }
}

document.getElementById("abrir")!.addEventListener("click", async () => {
  const caminho = await open({ multiple: false, filters: [{ name: "JSON Lines", extensions: ["jsonl", "ndjson", "json"] }] });
  if (typeof caminho === "string") await mostrar(caminho);
});

invoke<string | null>("arquivo_inicial").then((caminho) => {
  if (caminho) void mostrar(caminho);
});
