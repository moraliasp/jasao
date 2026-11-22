// Consulta jq. O Rust roda o filtro sobre o arquivo original em 2º plano (`consultar`) e grava
// os resultados num JSONL temporário, que abre no lugar do arquivo; "Voltar ao arquivo" reabre
// o original e apaga o temporário (`fechar_consulta`).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "./i18n";
import { indiceDe } from "./texto";

interface Andamento {
  geracao: number;
  resultados: number;
  linhas: number;
  total: number;
  erros: number;
  primeiro_erro: [number, string] | null;
  fim: boolean;
  cancelado: boolean;
  arquivo: string | null;
  falha: string | null;
}
interface Falha { inicio: number; fim: number; linha: number; coluna: number; mensagem: string }

export interface Opcoes {
  /** Caminho do arquivo aberto agora. */
  aberto(): string | null;
  /** Abre `caminho` na visão (resultado ou original). */
  abrir(caminho: string): Promise<void>;
  /** Esc no campo sem consulta rodando: foco de volta à visão. */
  voltar(): void;
  fmt(n: number): string;
}

export function criarConsulta(form: HTMLFormElement, o: Opcoes) {
  const campo = form.querySelector<HTMLInputElement>("input")!;
  const info = form.querySelector<HTMLElement>(".info")!;
  const cancelarBtn = form.querySelector<HTMLButtonElement>("[data-cancelar]")!;
  const voltarBtn = form.querySelector<HTMLButtonElement>("[data-voltar]")!;

  let geracao = 0;
  let esperando = false; // `consultar` em voo: só vale evento de geração nova
  let rodando = false;
  let original: string | null = null; // arquivo de onde veio o resultado aberto
  let resultado: string | null = null; // temporário aberto agora

  function mostrar(msg: string, erro = false): void {
    info.textContent = msg;
    info.classList.toggle("err", erro);
    campo.setAttribute("aria-invalid", String(erro));
    cancelarBtn.hidden = !rodando;
    voltarBtn.hidden = rodando || original === null;
  }

  async function rodar(): Promise<void> {
    const caminho = original ?? o.aberto();
    if (!campo.value.trim() || !caminho) return;
    rodando = esperando = true;
    mostrar(t.consulta.rodando);
    try {
      const g = await invoke<number>("consultar", { filtro: campo.value, caminho });
      if (g > geracao) geracao = g;
    } catch (e) {
      rodando = false;
      const f = (e as { falhas?: Falha[]; mensagem?: string }).falhas?.[0];
      if (!f) return mostrar(`${t.erro}: ${(e as { mensagem?: string }).mensagem ?? e}`, true);
      mostrar(t.consulta.sintaxe(f.linha, f.coluna, f.mensagem), true);
      const i = indiceDe(campo.value, f.linha, f.coluna);
      campo.focus();
      campo.setSelectionRange(i, i + Math.max(1, f.fim - f.inicio));
    } finally {
      esperando = false;
    }
  }

  async function fim(p: Andamento): Promise<void> {
    rodando = false;
    if (p.falha) return mostrar(`${t.erro}: ${p.falha}`, true);
    if (p.cancelado || !p.arquivo) return mostrar(t.consulta.cancelada);
    original ??= o.aberto();
    resultado = p.arquivo;
    await o.abrir(p.arquivo);
    const erro = p.primeiro_erro;
    mostrar(
      [
        t.consulta.resultados(p.resultados, o.fmt(p.resultados)),
        erro ? t.consulta.erros(o.fmt(p.erros), o.fmt(erro[0] + 1), erro[1]) : "",
      ].filter(Boolean).join(" · "),
    );
  }

  async function voltar(): Promise<void> {
    const c = original;
    if (!c) return;
    original = resultado = null;
    mostrar("");
    await o.abrir(c);
    void invoke("fechar_consulta");
    o.voltar();
  }

  form.addEventListener("submit", (e) => {
    e.preventDefault();
    void rodar();
  });
  campo.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    if (rodando) void invoke("fechar_consulta");
    else o.voltar();
  });
  campo.addEventListener("input", () => {
    if (campo.getAttribute("aria-invalid") === "true") mostrar("");
  });
  cancelarBtn.addEventListener("click", () => void invoke("fechar_consulta"));
  voltarBtn.addEventListener("click", () => void voltar());

  void listen<Andamento>("consulta", ({ payload: p }) => {
    if (p.geracao < geracao || (esperando && p.geracao === geracao)) return;
    geracao = p.geracao;
    if (p.fim) return void fim(p);
    mostrar(t.consulta.andamento(o.fmt(p.resultados), Math.floor((p.linhas / Math.max(1, p.total)) * 100)));
  });

  // Textos fixos.
  form.setAttribute("aria-label", t.consulta.rotulo);
  campo.placeholder = t.consulta.dica;
  campo.setAttribute("aria-label", t.consulta.rotulo);
  cancelarBtn.textContent = t.consulta.cancelar;
  voltarBtn.textContent = t.consulta.voltar;
  mostrar("");

  return {
    focar(): void {
      campo.focus();
      campo.select();
    },
    /** Um arquivo abriu: se não foi o resultado, a consulta deixa de valer. */
    abriu(caminho: string): void {
      if (caminho === resultado || caminho === original) return;
      if (original || rodando) void invoke("fechar_consulta");
      original = resultado = null;
      rodando = false;
      mostrar("");
    },
  };
}
