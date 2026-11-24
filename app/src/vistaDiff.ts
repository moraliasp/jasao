// Vista Diff: compara o doc da aba ativa com outro aberto. Estrutura pelo comando `diff_estrutura`
// (taxonomia do núcleo); valores pelo `diff_valores`, que roda numa thread do Rust e manda
// andamento e resultado no evento `diff_valores`. Desenho em `diff.ts` (puro, testado).
import { listen } from "@tauri-apps/api/event";
import { chamar, docAtivo } from "./doc";
import { porGeracao } from "./geracao";
import { t } from "./i18n";
import { htmlEstrutura, htmlValores, lerRegistros, type DiffArquivos, type DiffEstrutura } from "./diff";
import { escAtributo } from "./texto";

interface Andamento {
  id: number;
  geracao: number;
  varridas: number;
  total: number;
  resultado: DiffArquivos | null;
}

export interface Opcoes {
  fmt: (n: number) => string;
  icone: (id: string) => string;
  /** Abas com documento aberto, na ordem da barra. */
  abas: () => { doc: number; nome: string }[];
  avisar: (msg: string) => void;
  erro: (e: unknown) => string;
}

export function criarDiff(el: HTMLElement, o: Opcoes) {
  const x = t.diff;
  el.innerHTML = `<div class="tools">
      <label>${x.antes}<select id="df-antes"></select></label>
      <label>${x.depois}<select id="df-depois"></select></label>
      <label>${x.tipo}<select id="df-tipo"><option value="estrutura">${x.estrutura}</option><option value="valores">${x.valores}</option></select></label>
      <fieldset id="df-registros" hidden><legend>${x.registros}</legend>
        <label>${x.registroAntes}<input id="df-ra" inputmode="numeric" size="6"></label>
        <label>${x.registroDepois}<input id="df-rd" inputmode="numeric" size="6"></label>
      </fieldset>
      <div class="acts"><button class="btn" id="df-comparar" type="button">${o.icone("check")}<span>${x.comparar}</span></button></div>
    </div>
    <p class="check" id="df-status" role="status" aria-live="polite"></p>
    <div class="dres" id="df-res" tabindex="0" aria-label="${escAtributo(x.rotulo)}"></div>`;
  const q = <T extends HTMLElement>(id: string) => el.querySelector<T>(`#${id}`)!;
  const [antesEl, depoisEl, tipoEl] = ["df-antes", "df-depois", "df-tipo"].map((id) => q<HTMLSelectElement>(id));
  const [raEl, rdEl] = [q<HTMLInputElement>("df-ra"), q<HTMLInputElement>("df-rd")];
  const registrosEl = q("df-registros");
  const comparar = q<HTMLButtonElement>("df-comparar");
  const statusEl = q("df-status");
  const resEl = q("df-res");

  const estado = (msg: string, classe = "") => {
    statusEl.className = `check ${classe}`;
    statusEl.innerHTML = msg;
  };
  const par = () => [Number(antesEl.value), Number(depoisEl.value)] as const;

  /** Refaz as listas de arquivos, mantendo a escolha que ainda existe; padrão: ativa × outra. */
  function opcoes(): boolean {
    const abas = o.abas();
    const ops = abas.map((a) => `<option value="${a.doc}">${escAtributo(a.nome)}</option>`).join("");
    const [va, vd] = [antesEl.value, depoisEl.value];
    antesEl.innerHTML = depoisEl.innerHTML = ops;
    const tem = (v: string) => abas.some((a) => String(a.doc) === v);
    const ativo = String(docAtivo() ?? "");
    antesEl.value = tem(va) ? va : ativo;
    const outra = abas.find((a) => String(a.doc) !== antesEl.value);
    depoisEl.value = tem(vd) && vd !== antesEl.value ? vd : String(outra?.doc ?? antesEl.value);
    const ok = abas.length > 1;
    comparar.disabled = !ok;
    if (!ok) {
      resEl.innerHTML = "";
      estado(`<span>${x.precisaDuas}</span>`);
    }
    return ok;
  }

  const aplicar = (v: Andamento) => {
    if (!v.resultado) return estado(`<span>${x.comparando(o.fmt(v.varridas), o.fmt(v.total))}</span>`);
    eventos.fixar(0);
    const r = htmlValores(v.resultado, x, o.fmt);
    const algum = v.resultado.diferentes + v.resultado.so_antes + v.resultado.so_depois + v.resultado.invalidos + v.resultado.grandes;
    estado(`${o.icone(algum ? "alert" : "check")}<span>${r.resumo}</span>`, algum ? "bad" : "");
    resEl.innerHTML = r.html;
  };
  const eventos = porGeracao<Andamento>(aplicar);
  void listen<Andamento>("diff_valores", ({ payload: v }) => {
    if (v.id !== Number(antesEl.value) || el.hidden) return;
    eventos.receber(v);
  });

  async function estrutura(): Promise<void> {
    const [a, b] = par();
    const r = await chamar<DiffEstrutura | null>("diff_estrutura", { antes: a, depois: b });
    if (el.hidden || tipoEl.value !== "estrutura" || par()[0] !== a || par()[1] !== b) return;
    if (!r) {
      resEl.innerHTML = "";
      return estado(`<span>${x.inferindo}</span>`);
    }
    const h = htmlEstrutura(r, x, o.fmt);
    const quebra = r.mudancas.some((m) => m.classe === "quebra");
    estado(`${o.icone(quebra ? "alert" : "check")}<span>${h.resumo}</span>`, quebra ? "bad" : "");
    resEl.innerHTML = h.html;
  }

  async function valores(): Promise<void> {
    const regs = lerRegistros(raEl.value, rdEl.value);
    if (regs === "ruim") return estado(`${o.icone("alert")}<span>${x.registroRuim}</span>`, "bad");
    const [a, b] = par();
    resEl.innerHTML = "";
    estado(`<span>${x.comparando("0", "…")}</span>`);
    const g = await chamar<number | null>("diff_valores", { antes: a, depois: b, registros: regs });
    if (el.hidden) return;
    if (g === null) return estado(`<span>${x.inferindo}</span>`);
    eventos.fixar(g); // aplica o resultado que chegou antes desta resposta
  }

  /** Compara o par escolhido; sem duas abas, só avisa. */
  async function rodar(): Promise<void> {
    parar();
    if (!opcoes()) return;
    if (tipoEl.value === "estrutura") await estrutura();
    else await valores();
  }

  /** Para o diff de valores em curso (sair da vista, fechar aba, Esc). */
  function parar(): void {
    if (!eventos.atual) return;
    eventos.fixar(0);
    void chamar("cancelar_diff_valores").catch(() => {});
  }

  const falhou = (e: unknown) => o.avisar(o.erro(e));
  const mudouPar = () => {
    parar();
    resEl.innerHTML = "";
    if (tipoEl.value === "estrutura") void rodar().catch(falhou);
    else estado("");
  };
  antesEl.addEventListener("change", mudouPar);
  depoisEl.addEventListener("change", mudouPar);
  tipoEl.addEventListener("change", () => {
    registrosEl.hidden = tipoEl.value !== "valores";
    mudouPar();
  });
  comparar.addEventListener("click", () => void rodar().catch(falhou));
  for (const i of [raEl, rdEl])
    i.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void rodar().catch(falhou);
    });
  el.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || !eventos.atual) return;
    e.preventDefault();
    parar();
    estado(`<span>${x.cancelado}</span>`);
  });

  return {
    /** Entrar na vista: estrutura compara na hora; valores espera o botão (pode ser longo). */
    mostrar: () => {
      if (!opcoes()) return;
      if (tipoEl.value === "estrutura") void estrutura().catch(falhou);
    },
    parar,
    /** Uma inferência terminou: se a estrutura esperava por ela, compara de novo. */
    inferiu: () => {
      if (!el.hidden && tipoEl.value === "estrutura") void estrutura().catch(falhou);
    },
    /** Abas mudaram (abriu/fechou): o diff de valores do doc fechado para. */
    abasMudaram: () => {
      const docs = new Set(o.abas().map((a) => a.doc));
      const [a, b] = par();
      if (!docs.has(a) || !docs.has(b)) parar();
      if (!el.hidden) opcoes();
    },
  };
}
