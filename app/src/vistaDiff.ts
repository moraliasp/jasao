// Vista Diff: compara o doc da aba ativa com outro aberto. Estrutura pelo comando `diff_estrutura`
// (taxonomia do núcleo); valores pelo `diff_valores`, que roda numa thread do Rust e manda
// andamento e resultado no evento `diff_valores`. Desenho em `diff.ts` (puro, testado).
import { listen } from "@tauri-apps/api/event";
import { chamar, docAtivo } from "./doc";
import { porGeracao } from "./geracao";
import { t } from "./i18n";
import { htmlEstrutura, htmlValores, lerRegistros, mesmoArquivo, travarPar as travar, type DiffArquivos, type DiffEstrutura } from "./diff";
import { htmlLinha, marcar, parMarcado, type Linha } from "./linhaTempo";
import { escAtributo } from "./texto";

/** Evento `linha_do_tempo` (sem id de documento: a pasta não é um doc aberto). */
interface AndamentoLinha {
  geracao: number;
  feitas: number;
  total: number;
  resultado: Linha | null;
}

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
  /** Abas com documento aberto, na ordem da barra; `caminho` é o aberto agora (arquivo ou consulta). */
  abas: () => { doc: number; nome: string; caminho: string | null }[];
  avisar: (msg: string) => void;
  erro: (e: unknown) => string;
  /** Abre cada caminho numa aba (já concedido) e devolve o doc de cada um, `null` se não abriu. */
  abrir: (caminhos: string[]) => Promise<(number | null)[]>;
  /** Volta para a vista Diff (abrir uma aba troca de vista). */
  irParaDiff: () => void;
  /** Botão do estado vazio (só uma aba): abre outro arquivo para comparar. */
  abrirOutro: () => void;
}

export function criarDiff(el: HTMLElement, o: Opcoes) {
  const x = t.diff;
  const v = t.diffVazio;
  el.innerHTML = `<section class="empty" id="df-vazio" hidden>${o.icone("comparar")}<h3>${v.titulo}</h3><p>${v.texto}</p>
      <button class="btn pri" id="df-abrir" type="button">${v.abrir}</button><div class="recent" id="df-recentes"></div>
      <button class="lk" id="df-ir-linha" type="button">${x.linhaDoTempo}…</button></section>
    <div class="tools">
      <label class="df-par">${x.antes}<select id="df-antes"></select></label>
      <label class="df-par">${x.depois}<select id="df-depois"></select></label>
      <label>${x.tipo}<select id="df-tipo"><option value="estrutura">${x.estrutura}</option><option value="valores">${x.valores}</option><option value="linha">${x.linhaDoTempo}</option></select></label>
      <fieldset id="df-registros" hidden><legend>${x.registros}</legend>
        <label>${x.registroAntes}<input id="df-ra" inputmode="numeric" size="6"></label>
        <label>${x.registroDepois}<input id="df-rd" inputmode="numeric" size="6"></label>
      </fieldset>
      <div class="acts df-par"><button class="btn" id="df-comparar" type="button">${o.icone("check")}<span>${x.comparar}</span></button></div>
      <div class="acts" id="df-linha" hidden><button class="btn" id="df-pasta" type="button"><span>${x.pasta}</span></button><button class="btn" id="df-marcados" type="button" disabled>${o.icone("check")}<span>${x.compararMarcados}</span></button></div>
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
  const linhaEl = q("df-linha");
  const marcadosEl = q<HTMLButtonElement>("df-marcados");
  const vazioEl = q("df-vazio");
  const toolsEl = el.querySelector<HTMLElement>(".tools")!;
  q("df-abrir").addEventListener("click", o.abrirOutro);
  /** Última linha do tempo recebida e os pontos marcados nela (índices de `passos`). */
  let linha: Linha | null = null;
  let marcados: number[] = [];

  const estado = (msg: string, classe = "") => {
    statusEl.className = `check ${classe}`;
    statusEl.innerHTML = msg;
  };
  const par = () => [Number(antesEl.value), Number(depoisEl.value)] as const;
  const travarPar = (abas = o.abas()) => travar(abas, antesEl, depoisEl);

  /** Refaz as listas de arquivos, mantendo a escolha que ainda existe; padrão: ativa × outra. */
  function opcoes(): boolean {
    if (tipoEl.value === "linha") {
      vazioEl.hidden = true;
      toolsEl.hidden = statusEl.hidden = resEl.hidden = false;
      return true;
    }
    const abas = o.abas();
    const ops = abas.map((a) => `<option value="${a.doc}">${escAtributo(a.nome)}</option>`).join("");
    const [va, vd] = [antesEl.value, depoisEl.value];
    antesEl.innerHTML = depoisEl.innerHTML = ops;
    const tem = (v: string) => abas.some((a) => String(a.doc) === v);
    const ativo = String(docAtivo() ?? "");
    antesEl.value = tem(va) ? va : ativo;
    const outra = abas.find((a) => !mesmoArquivo(abas, String(a.doc), antesEl.value));
    depoisEl.value = tem(vd) && !mesmoArquivo(abas, vd, antesEl.value) ? vd : String(outra?.doc ?? "");
    travarPar(abas);
    const ok = !!outra;
    comparar.disabled = !ok;
    vazioEl.hidden = ok;
    toolsEl.hidden = statusEl.hidden = resEl.hidden = !ok;
    if (!ok) resEl.innerHTML = "";
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

  const desenharLinha = () => {
    if (!linha) return;
    const h = htmlLinha(linha, x, o.fmt, marcados);
    const quebra = linha.passos.some((p) => p.quebras > 0);
    estado(`${o.icone(quebra ? "alert" : "check")}<span>${h.resumo}</span>`, quebra ? "bad" : "");
    resEl.innerHTML = h.html;
    marcadosEl.disabled = !parMarcado(marcados);
  };
  const aplicarLinha = (v: AndamentoLinha) => {
    if (!v.resultado) return estado(`<span>${x.linha.percorrendo(o.fmt(v.feitas), o.fmt(v.total))}</span>`);
    linhas.fixar(0);
    linha = v.resultado;
    marcados = [];
    desenharLinha();
  };
  const linhas = porGeracao<AndamentoLinha>(aplicarLinha);
  void listen<AndamentoLinha>("linha_do_tempo", ({ payload: v }) => {
    if (el.hidden || tipoEl.value !== "linha") return;
    linhas.receber(v);
  });

  /** Percorre a pasta (já concedida: escolhida no diálogo ou solta na janela). */
  async function percorrer(pasta: string): Promise<void> {
    parar();
    linha = null;
    marcados = [];
    resEl.innerHTML = "";
    marcadosEl.disabled = true;
    estado(`<span>${x.linha.percorrendo("0", "…")}</span>`);
    const g = await chamar<number>("linha_do_tempo", { pasta });
    if (el.hidden || tipoEl.value !== "linha") return void chamar("cancelar_linha_do_tempo").catch(() => {});
    linhas.fixar(g);
  }

  /** Abre as duas versões marcadas e compara a estrutura delas (antes = a mais velha). */
  async function compararMarcados(): Promise<void> {
    const p = parMarcado(marcados);
    if (!linha || !p) return estado(`<span>${x.linha.marque}</span>`);
    const [a, b] = await o.abrir(p.map((i) => linha!.passos[i].caminho));
    if (a === null || b === null) return;
    tipoEl.value = "estrutura";
    modoTipo();
    opcoes();
    [antesEl.value, depoisEl.value] = [String(a), String(b)];
    travarPar();
    o.irParaDiff();
  }

  /** Compara o par escolhido; sem duas abas, só avisa. */
  async function rodar(): Promise<void> {
    parar();
    if (!opcoes()) return;
    if (tipoEl.value === "estrutura") await estrutura();
    else await valores();
  }

  /** Para o diff de valores ou a linha do tempo em curso (sair da vista, fechar aba, Esc). */
  function parar(): void {
    if (eventos.atual) {
      eventos.fixar(0);
      void chamar("cancelar_diff_valores").catch(() => {});
    }
    if (linhas.atual) {
      linhas.fixar(0);
      void chamar("cancelar_linha_do_tempo").catch(() => {});
    }
  }
  const emCurso = () => eventos.atual || linhas.atual;
  /** Mostra os controles do tipo escolhido: o par de abas ou a pasta da linha do tempo. */
  const modoTipo = () => {
    const l = tipoEl.value === "linha";
    registrosEl.hidden = tipoEl.value !== "valores";
    linhaEl.hidden = !l;
    for (const e of el.querySelectorAll<HTMLElement>(".df-par")) e.hidden = l;
  };

  const falhou = (e: unknown) => o.avisar(o.erro(e));
  const mudouPar = () => {
    parar();
    resEl.innerHTML = "";
    if (tipoEl.value === "estrutura") void rodar().catch(falhou);
    else if (tipoEl.value === "linha" && linha) desenharLinha();
    else estado(tipoEl.value === "linha" ? `<span>${x.linha.escolha}</span>` : "");
  };
  for (const sel of [antesEl, depoisEl])
    sel.addEventListener("change", () => {
      travarPar();
      mudouPar();
    });
  tipoEl.addEventListener("change", () => {
    modoTipo();
    mudouPar();
  });
  q("df-ir-linha").addEventListener("click", () => {
    tipoEl.value = "linha";
    opcoes();
    modoTipo();
    mudouPar();
  });
  q("df-pasta").addEventListener("click", async () => {
    try {
      const p = await chamar<string | null>("escolher_pasta");
      if (p) await percorrer(p);
    } catch (e) {
      falhou(e);
    }
  });
  marcadosEl.addEventListener("click", () => void compararMarcados().catch(falhou));
  resEl.addEventListener("change", (e) => {
    const i = Number((e.target as HTMLElement).dataset.i);
    if (!linha || !Number.isInteger(i)) return;
    marcados = marcar(marcados, i);
    for (const c of resEl.querySelectorAll<HTMLInputElement>("input[data-i]")) c.checked = marcados.includes(Number(c.dataset.i));
    marcadosEl.disabled = !parMarcado(marcados);
  });
  comparar.addEventListener("click", () => void rodar().catch(falhou));
  for (const i of [raEl, rdEl])
    i.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void rodar().catch(falhou);
    });
  el.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || !emCurso()) return;
    e.preventDefault();
    parar();
    estado(`<span>${x.cancelado}</span>`);
  });

  return {
    /** Entrar na vista: estrutura compara na hora; valores espera o botão (pode ser longo). */
    mostrar: () => {
      if (!opcoes()) return;
      if (tipoEl.value === "estrutura") void estrutura().catch(falhou);
      else if (tipoEl.value === "linha" && !linha && !linhas.atual) estado(`<span>${x.linha.escolha}</span>`);
    },
    /** Pasta solta na janela: linha do tempo dela (a vista já está à mostra). */
    linhaDoTempo: (pasta: string) => {
      tipoEl.value = "linha";
      modoTipo();
      void percorrer(pasta).catch(falhou);
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
      // Só o diff de valores depende das abas; a linha do tempo é de uma pasta.
      if ((!docs.has(a) || !docs.has(b)) && eventos.atual) {
        eventos.fixar(0);
        void chamar("cancelar_diff_valores").catch(() => {});
      }
      if (!el.hidden) opcoes();
    },
  };
}
