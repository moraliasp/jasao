// Busca no arquivo inteiro. O Rust varre em 2º plano (`buscar`) e guarda os achados; aqui ficam
// a barra, a contagem e qual achado é o atual. O realce pede só os achados da janela visível.
// Cada documento (aba) tem a sua busca: termo, opções, contagem e achado atual.
import { listen } from "@tauri-apps/api/event";
import { chamar } from "./doc";
import { t } from "./i18n";
import type { Marca } from "./texto";

interface Andamento { id: number; geracao: number; achados: number; guardados: number; varridos: number; total: number; fim: boolean }
type Trio = [number, number, number]; // registro, byte na linha, comprimento

/** Estado da busca de um documento. */
interface Estado {
  termo: string;
  caixa: boolean;
  regex: boolean;
  geracao: number; // da busca atual; evento de geração menor é de busca velha
  esperando: boolean; // `buscar` em voo: até a resposta, só vale evento de geração nova
  andamento: Andamento | null;
  atual: number; // índice do achado atual (-1: nenhum ainda)
  erro: string;
  janela: { inicio: number; fim: number; indice: number; lista: Trio[] } | null;
  pedido: boolean;
}
const novo = (): Estado => ({
  termo: "", caixa: false, regex: false, geracao: 0, esperando: false, andamento: null, atual: -1, erro: "", janela: null, pedido: false,
});

const ESPERA = 200; // ms sem digitar antes de buscar
const JANELA = 1_000; // registros por pedido de achados (= MAX_JANELA do Rust)
const MAX_JANELA = 10_000; // achados por pedido (= MAX_ACHADOS_JANELA do Rust)

export interface Opcoes {
  /** Leva a visão ao achado: registro e byte na linha. */
  ir(registro: number, byte: number): Promise<void>;
  /** Achados ou atual mudaram: redesenhar realce e régua. */
  mudou(): void;
  /** Esc no campo: foco de volta à visão. */
  voltar(): void;
  fmt(n: number): string;
}

export function criarBusca(form: HTMLFormElement, o: Opcoes) {
  const campo = form.querySelector<HTMLInputElement>("input")!;
  const caixa = form.querySelector<HTMLButtonElement>("[data-caixa]")!;
  const regex = form.querySelector<HTMLButtonElement>("[data-regex]")!;
  const info = form.querySelector<HTMLElement>(".info")!;
  const anteriorBtn = form.querySelector<HTMLButtonElement>("[data-anterior]")!;
  const proximoBtn = form.querySelector<HTMLButtonElement>("[data-proximo]")!;

  const estados = new Map<number, Estado>();
  let s = novo(); // o do documento ativo
  let espera: number | undefined;

  const ligado = (b: HTMLButtonElement) => b.getAttribute("aria-pressed") === "true";

  function mostrarInfo(): void {
    const { andamento: e, erro, atual } = s;
    info.classList.toggle("err", erro !== "");
    info.textContent =
      erro ? erro
      : !campo.value ? ""
      : !e ? t.busca.buscando
      : e.fim && !e.achados ? t.busca.nenhum
      : [
          t.busca.posicao(atual < 0 ? "–" : o.fmt(atual + 1), o.fmt(e.achados)),
          e.fim ? "" : `${Math.floor((e.varridos / Math.max(1, e.total)) * 100)}%`,
          e.achados > e.guardados && e.fim ? t.busca.limite(o.fmt(e.guardados)) : "",
        ].filter(Boolean).join(" · ");
    anteriorBtn.disabled = proximoBtn.disabled = !e?.guardados;
  }

  async function buscar(): Promise<void> {
    clearTimeout(espera);
    espera = undefined;
    const e = s; // a resposta vale para este documento, mesmo que a aba mude no meio
    Object.assign(e, { termo: campo.value, atual: -1, janela: null, erro: "", andamento: null, esperando: true });
    mostrarInfo();
    try {
      const g = await chamar<number>("buscar", { padrao: e.termo, regex: e.regex, ignorarCaixa: !e.caixa });
      if (g > e.geracao) e.geracao = g;
    } catch (erro) {
      e.erro = String(erro);
    } finally {
      e.esperando = false;
    }
    if (e !== s) return;
    mostrarInfo();
    o.mudou();
  }

  async function ir(delta: number): Promise<void> {
    if (espera !== undefined) await buscar();
    const e = s;
    const n = e.andamento?.guardados ?? 0;
    if (!n) return;
    e.atual = e.atual < 0 ? (delta > 0 ? 0 : n - 1) : (e.atual + delta + n) % n;
    mostrarInfo();
    const a = await chamar<Trio | null>("achado", { i: e.atual });
    if (e !== s) return;
    if (a) await o.ir(a[0], a[1]);
    o.mudou();
  }

  async function pedirJanela(r: number): Promise<void> {
    const e = s;
    e.pedido = true;
    const g = e.geracao;
    const inicio = Math.max(0, r - JANELA / 2);
    try {
      const [indice, lista] = await chamar<[number, Trio[]]>("achados", { inicio, quantidade: JANELA });
      if (g !== e.geracao) return;
      // Lista cortada no teto: só os registros antes do último vêm completos.
      const fim = lista.length === MAX_JANELA ? Math.max(lista[lista.length - 1][0], r + 1) : inicio + JANELA;
      e.janela = { inicio, fim, indice, lista };
    } finally {
      e.pedido = false;
    }
    if (e === s) o.mudou();
  }

  campo.addEventListener("input", () => {
    clearTimeout(espera);
    espera = window.setTimeout(() => void buscar(), ESPERA);
  });
  campo.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      o.voltar();
    }
  });
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    void ir(1);
  });
  // Enter envia o form; Shift+Enter não envia em todo WebView, então vai aqui.
  campo.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.shiftKey) {
      e.preventDefault();
      void ir(-1);
    }
  });
  for (const b of [caixa, regex])
    b.addEventListener("click", () => {
      b.setAttribute("aria-pressed", String(!ligado(b)));
      s.caixa = ligado(caixa);
      s.regex = ligado(regex);
      if (campo.value) void buscar();
    });
  anteriorBtn.addEventListener("click", () => void ir(-1));
  proximoBtn.addEventListener("click", () => void ir(1));

  void listen<Andamento>("busca", ({ payload: p }) => {
    const e = estados.get(p.id);
    if (!e || p.geracao < e.geracao || (e.esperando && p.geracao === e.geracao)) return;
    e.geracao = p.geracao;
    e.andamento = p;
    e.janela = null; // chegaram achados novos na faixa
    if (e !== s) return;
    mostrarInfo();
    o.mudou();
  });

  // Textos fixos.
  form.setAttribute("aria-label", t.busca.rotulo);
  campo.placeholder = t.busca.dica;
  campo.setAttribute("aria-label", t.busca.rotulo);
  caixa.setAttribute("aria-label", t.busca.caixa);
  caixa.title = t.busca.caixa;
  regex.setAttribute("aria-label", t.busca.regex);
  regex.title = t.busca.regex;
  anteriorBtn.setAttribute("aria-label", t.busca.anterior);
  anteriorBtn.title = t.busca.anterior;
  proximoBtn.setAttribute("aria-label", t.busca.proximo);
  proximoBtn.title = t.busca.proximo;
  mostrarInfo();

  return {
    focar(): void {
      campo.focus();
      campo.select();
    },
    proximo: () => ir(1),
    anterior: () => ir(-1),
    /** Arquivo novo no documento ativo: o Rust já limpou a busca; refaz com o termo do campo. */
    reabrir(): void {
      if (campo.value) void buscar();
      else {
        Object.assign(s, { andamento: null, janela: null, atual: -1 });
        mostrarInfo();
      }
    },
    /** Troca de aba: mostra a busca do documento `id` (nova, se ainda não houver). */
    usar(id: number | null): void {
      if (espera !== undefined) void buscar(); // termo digitado ainda sem busca: vale para a aba que sai
      s.termo = campo.value;
      if (id === null) s = novo();
      else {
        const e = estados.get(id) ?? novo();
        estados.set(id, (s = e));
      }
      campo.value = s.termo;
      caixa.setAttribute("aria-pressed", String(s.caixa));
      regex.setAttribute("aria-pressed", String(s.regex));
      mostrarInfo();
    },
    /** Aba fechada: esquece a busca dela. */
    fechar: (id: number) => void estados.delete(id),
    /** Há achados para mostrar (régua). */
    ativa: () => (s.andamento?.guardados ?? 0) > 0,
    /** Achados do registro `r` como marcas em bytes, pedindo a janela se faltar. */
    marcas(r: number): Marca[] {
      const { andamento, janela, atual } = s;
      if (!andamento?.guardados) return [];
      if (!janela || r < janela.inicio || r >= janela.fim) {
        if (!s.pedido) void pedirJanela(r);
        return [];
      }
      const { lista, indice } = janela;
      let i = 0;
      while (i < lista.length && lista[i][0] < r) i++;
      const marcas: Marca[] = [];
      for (; i < lista.length && lista[i][0] === r; i++) marcas.push([lista[i][1], lista[i][1] + lista[i][2], indice + i === atual]);
      return marcas;
    },
  };
}
