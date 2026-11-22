// Consulta jq. O Rust roda o filtro sobre o arquivo original em 2º plano (`consultar`) e grava
// os resultados num JSONL temporário, que abre no lugar do arquivo; "Voltar ao arquivo" reabre
// o original e apaga o temporário (`fechar_consulta`). Cada documento (aba) tem a sua consulta.
import { listen } from "@tauri-apps/api/event";
import { chamar } from "./doc";
import { t } from "./i18n";
import { indiceDe } from "./texto";

interface Andamento {
  id: number;
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
  /** Caminho do arquivo aberto agora na aba ativa. */
  aberto(): string | null;
  /** Abre `caminho` no documento `id` (resultado ou original), mesmo que a aba não seja a ativa. */
  abrir(caminho: string, id: number): Promise<void>;
  /** Esc no campo sem consulta rodando: foco de volta à visão. */
  voltar(): void;
  fmt(n: number): string;
}

export function criarConsulta(form: HTMLFormElement, o: Opcoes) {
  const campo = form.querySelector<HTMLInputElement>("input")!;
  const info = form.querySelector<HTMLElement>(".info")!;
  const cancelarBtn = form.querySelector<HTMLButtonElement>("[data-cancelar]")!;
  const voltarBtn = form.querySelector<HTMLButtonElement>("[data-voltar]")!;

  /** Consulta de um documento. */
  interface Estado {
    id: number;
    filtro: string;
    geracao: number;
    esperando: boolean; // `consultar` em voo: só vale evento de geração nova
    rodando: boolean;
    fonte: string | null; // arquivo sobre o qual a consulta em curso roda
    original: string | null; // arquivo de onde veio o resultado aberto
    resultado: string | null; // temporário aberto agora
    msg: string;
    erro: boolean;
  }
  const novo = (id: number): Estado => ({
    id, filtro: "", geracao: 0, esperando: false, rodando: false, fonte: null, original: null, resultado: null, msg: "", erro: false,
  });
  const estados = new Map<number, Estado>();
  let s: Estado | null = null; // o da aba ativa (null: aba vazia)

  /** Guarda a mensagem de `e` e, se for a aba ativa, mostra. */
  function mostrar(e: Estado | null, msg: string, erro = false): void {
    if (e) Object.assign(e, { msg, erro });
    if (e !== s) return;
    info.textContent = msg;
    info.classList.toggle("err", erro);
    campo.setAttribute("aria-invalid", String(erro));
    cancelarBtn.hidden = !s?.rodando;
    voltarBtn.hidden = !s || s.rodando || s.original === null;
  }

  async function rodar(): Promise<void> {
    const e = s;
    const caminho = e?.original ?? o.aberto();
    if (!e || !campo.value.trim() || !caminho) return;
    Object.assign(e, { filtro: campo.value, fonte: caminho, rodando: true, esperando: true });
    mostrar(e, t.consulta.rodando);
    try {
      const g = await chamar<number>("consultar", { id: e.id, filtro: e.filtro, caminho });
      if (g > e.geracao) e.geracao = g;
    } catch (x) {
      e.rodando = false;
      const f = (x as { falhas?: Falha[]; mensagem?: string }).falhas?.[0];
      if (!f) return mostrar(e, `${t.erro}: ${(x as { mensagem?: string }).mensagem ?? x}`, true);
      mostrar(e, t.consulta.sintaxe(f.linha, f.coluna, f.mensagem), true);
      if (e !== s) return;
      const i = indiceDe(campo.value, f.linha, f.coluna);
      campo.focus();
      campo.setSelectionRange(i, i + Math.max(1, f.fim - f.inicio));
    } finally {
      e.esperando = false;
    }
  }

  async function fim(e: Estado, p: Andamento): Promise<void> {
    e.rodando = false;
    if (p.falha) return mostrar(e, `${t.erro}: ${p.falha}`, true);
    if (p.cancelado || !p.arquivo) return mostrar(e, t.consulta.cancelada);
    e.original ??= e.fonte;
    e.resultado = p.arquivo;
    await o.abrir(p.arquivo, e.id);
    const erro = p.primeiro_erro;
    mostrar(
      e,
      [
        t.consulta.resultados(p.resultados, o.fmt(p.resultados)),
        erro ? t.consulta.erros(o.fmt(p.erros), o.fmt(erro[0] + 1), erro[1]) : "",
      ].filter(Boolean).join(" · "),
    );
  }

  async function voltar(e: Estado): Promise<void> {
    const c = e.original;
    if (!c) return;
    e.original = e.resultado = null;
    mostrar(e, "");
    await o.abrir(c, e.id);
    void chamar("fechar_consulta", { id: e.id });
    if (e === s) o.voltar();
  }

  /** Cancelar não recebe `fim` (a geração já mudou); o resultado anterior, se aberto, foi apagado. */
  async function cancelar(e: Estado): Promise<void> {
    e.rodando = false;
    if (e.resultado) await voltar(e);
    else void chamar("fechar_consulta", { id: e.id });
    mostrar(e, t.consulta.cancelada);
  }

  form.addEventListener("submit", (e) => {
    e.preventDefault();
    void rodar();
  });
  campo.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    if (s?.rodando) void cancelar(s);
    else o.voltar();
  });
  campo.addEventListener("input", () => {
    if (campo.getAttribute("aria-invalid") === "true") mostrar(s, "");
  });
  cancelarBtn.addEventListener("click", () => s && void cancelar(s));
  voltarBtn.addEventListener("click", () => s && void voltar(s));

  void listen<Andamento>("consulta", ({ payload: p }) => {
    const e = estados.get(p.id);
    if (!e || p.geracao < e.geracao || (e.esperando && p.geracao === e.geracao)) return;
    e.geracao = p.geracao;
    if (p.fim) return void fim(e, p);
    mostrar(e, t.consulta.andamento(o.fmt(p.resultados), Math.floor((p.linhas / Math.max(1, p.total)) * 100)));
  });

  // Textos fixos.
  form.setAttribute("aria-label", t.consulta.rotulo);
  campo.placeholder = t.consulta.dica;
  campo.setAttribute("aria-label", t.consulta.rotulo);
  cancelarBtn.textContent = t.consulta.cancelar;
  voltarBtn.textContent = t.consulta.voltar;
  mostrar(null, "");

  return {
    focar(): void {
      campo.focus();
      campo.select();
    },
    /** Um arquivo abriu no documento `id`: se não foi o resultado, a consulta dele deixa de valer. */
    abriu(caminho: string, id: number): void {
      const e = estados.get(id);
      if (!e || caminho === e.resultado || caminho === e.original) return;
      if (e.original || e.rodando) void chamar("fechar_consulta", { id });
      Object.assign(e, { original: null, resultado: null, rodando: false });
      mostrar(e, "");
    },
    /** Troca de aba: mostra a consulta do documento `id` (nova, se ainda não houver). */
    usar(id: number | null): void {
      if (s) s.filtro = campo.value;
      if (id === null) s = null;
      else {
        if (!estados.has(id)) estados.set(id, novo(id));
        s = estados.get(id)!;
      }
      campo.value = s?.filtro ?? "";
      mostrar(s, s?.msg ?? "", s?.erro);
    },
    /** Aba fechada: esquece a consulta dela (o Rust apaga o temporário ao fechar o documento). */
    fechar: (id: number) => void estados.delete(id),
  };
}
