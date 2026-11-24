// Consulta jq. O Rust roda o filtro sobre o arquivo original em 2º plano (`consultar`) e grava
// os resultados num JSONL temporário, que abre no lugar do arquivo; "Voltar ao arquivo" reabre
// o original e apaga o temporário (`fechar_consulta`). Cada documento (aba) tem a sua consulta.
// Ao vivo: digitar roda o filtro depois de uma pausa (`ESPERA`); o Rust cancela a anterior. O
// resultado aberto sai em JSONL ou CSV (`salvar_consulta`).
import { listen } from "@tauri-apps/api/event";
import { chamar } from "./doc";
import { alternarFiltro, CHAVE_FILTROS, lerFiltros } from "./filtros";
import { erroDoRust, t } from "./i18n";
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
  cortado: boolean; // a saída passou do teto: resultado parcial
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

/** Pausa na digitação antes de rodar ao vivo. */
const ESPERA = 400;

export function criarConsulta(form: HTMLFormElement, o: Opcoes) {
  const campo = form.querySelector<HTMLInputElement>("input")!;
  const info = form.querySelector<HTMLElement>(".info")!;
  const cancelarBtn = form.querySelector<HTMLButtonElement>("[data-cancelar]")!;
  const voltarBtn = form.querySelector<HTMLButtonElement>("[data-voltar]")!;
  const anonBtn = form.querySelector<HTMLButtonElement>("[data-anonimizar]")!;
  const anonimizar = () => anonBtn.getAttribute("aria-pressed") === "true";
  const salvarBtn = form.querySelector<HTMLButtonElement>("[data-salvar]")!;
  const sugestoes = form.querySelector<HTMLDataListElement>("datalist")!;
  const exportarBtns = form.querySelectorAll<HTMLButtonElement>("[data-exportar]");

  /** Consulta de um documento. */
  interface Estado {
    id: number;
    filtro: string; // o do campo (guardado ao trocar de aba)
    rodado: string; // o último que foi ao Rust
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
    id, filtro: "", rodado: "", geracao: 0, esperando: false, rodando: false, fonte: null, original: null, resultado: null, msg: "", erro: false,
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
    for (const b of exportarBtns) b.hidden = !s || s.rodando || s.resultado === null;
  }

  // Filtros salvos. Armazenamento bloqueado: a lista fica só nesta execução.
  let filtros: string[] = [];
  try {
    filtros = lerFiltros(localStorage.getItem(CHAVE_FILTROS));
  } catch {}
  function desenharFiltros(): void {
    sugestoes.replaceChildren(
      ...filtros.map((f) => {
        const op = document.createElement("option");
        op.value = f; // texto do usuário: nunca como HTML
        return op;
      }),
    );
    marcarSalvo();
  }
  function marcarSalvo(): void {
    const salvo = filtros.includes(campo.value);
    salvarBtn.setAttribute("aria-pressed", String(salvo));
    salvarBtn.disabled = !campo.value.trim();
    salvarBtn.title = salvo ? t.consulta.esquecer : t.consulta.salvar;
    salvarBtn.setAttribute("aria-label", salvarBtn.title);
  }

  let espera: ReturnType<typeof setTimeout> | undefined;

  /** `aoVivo`: veio da digitação; erro de sintaxe aparece, mas o cursor fica onde está. */
  async function rodar(aoVivo = false): Promise<void> {
    clearTimeout(espera);
    const e = s;
    const caminho = e?.original ?? o.aberto();
    if (!e || !campo.value.trim() || !caminho) return;
    if (aoVivo && campo.value === e.rodado && (e.rodando || e.resultado)) return;
    Object.assign(e, { filtro: campo.value, rodado: campo.value, fonte: caminho, rodando: true, esperando: true });
    mostrar(e, t.consulta.rodando);
    try {
      const g = await chamar<number>("consultar", { id: e.id, filtro: e.filtro, caminho, anonimizar: anonimizar() });
      if (g > e.geracao) e.geracao = g;
    } catch (x) {
      e.rodando = false;
      const f = (x as { falhas?: Falha[]; mensagem?: string }).falhas?.[0];
      if (!f) return mostrar(e, `${t.erro}: ${erroDoRust((x as { mensagem?: string }).mensagem ?? x)}`, true);
      mostrar(e, t.consulta.sintaxe(f.linha, f.coluna, f.mensagem), true);
      if (e !== s || aoVivo) return;
      const i = indiceDe(campo.value, f.linha, f.coluna);
      campo.focus();
      campo.setSelectionRange(i, i + Math.max(1, f.fim - f.inicio));
    } finally {
      e.esperando = false;
    }
  }

  async function fim(e: Estado, p: Andamento): Promise<void> {
    e.rodando = false;
    if (p.falha) return mostrar(e, `${t.erro}: ${erroDoRust(p.falha)}`, true);
    if (p.cancelado || !p.arquivo) return mostrar(e, t.consulta.cancelada);
    e.original ??= e.fonte;
    e.resultado = p.arquivo;
    await o.abrir(p.arquivo, e.id);
    const erro = p.primeiro_erro;
    mostrar(
      e,
      [
        t.consulta.resultados(p.resultados, o.fmt(p.resultados)),
        p.cortado ? t.consulta.cortada : "",
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

  async function exportar(e: Estado, csv: boolean): Promise<void> {
    if (!e.resultado || e.rodando) return;
    const nome = (e.original ?? "").split(/[\\/]/).pop()!.replace(/\.(jsonl?|ndjson)(\.(gz|zst))?$/i, "") || "consulta";
    try {
      const salvo = await chamar<string | null>("salvar_consulta", { id: e.id, arquivo: e.resultado, csv, nome });
      if (salvo) mostrar(e, t.consulta.salvo(salvo));
    } catch (x) {
      mostrar(e, `${t.erro}: ${erroDoRust((x as { mensagem?: string }).mensagem ?? x)}`, true);
    }
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
  salvarBtn.addEventListener("click", () => {
    filtros = alternarFiltro(filtros, campo.value);
    try {
      localStorage.setItem(CHAVE_FILTROS, JSON.stringify(filtros));
    } catch {}
    desenharFiltros();
  });
  for (const b of exportarBtns) b.addEventListener("click", () => s && void exportar(s, b.dataset.exportar === "csv"));
  campo.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    if (s?.rodando) void cancelar(s);
    else o.voltar();
  });
  campo.addEventListener("input", () => {
    if (campo.getAttribute("aria-invalid") === "true") mostrar(s, "");
    marcarSalvo();
    clearTimeout(espera);
    if (campo.value.trim()) espera = setTimeout(() => void rodar(true), ESPERA);
  });
  cancelarBtn.addEventListener("click", () => s && void cancelar(s));
  voltarBtn.addEventListener("click", () => s && void voltar(s));
  anonBtn.addEventListener("click", () => {
    anonBtn.setAttribute("aria-pressed", String(!anonimizar()));
    if (campo.value.trim()) void rodar(); // o resultado na tela era do outro modo
  });

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
  anonBtn.title = t.consulta.anonimizar;
  anonBtn.setAttribute("aria-label", t.consulta.anonimizar);
  for (const b of exportarBtns) b.textContent = b.dataset.exportar === "csv" ? t.consulta.csv : t.consulta.jsonl;
  desenharFiltros();
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
      clearTimeout(espera); // a pausa era da aba que sai
      if (s) s.filtro = campo.value;
      if (id === null) s = null;
      else {
        if (!estados.has(id)) estados.set(id, novo(id));
        s = estados.get(id)!;
      }
      campo.value = s?.filtro ?? "";
      marcarSalvo();
      mostrar(s, s?.msg ?? "", s?.erro);
    },
    /** Aba fechada: esquece a consulta dela (o Rust apaga o temporário ao fechar o documento). */
    fechar: (id: number) => void estados.delete(id),
  };
}
