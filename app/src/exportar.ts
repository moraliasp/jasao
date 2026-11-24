// Vista Exportar: Zod, JSON Schema ou tipos TypeScript da estrutura inferida, gerados no núcleo (`exportar`) e
// validados contra o arquivo inteiro numa thread do Rust (evento `validacao`). O status é honesto:
// diz quantos registros passam e mostra as primeiras falhas com linha e motivo. Também valida
// outro arquivo contra esta estrutura, ou este arquivo contra um JSON Schema de fora; os dois
// são escolhidos no diálogo do backend e reusam o mesmo evento.
import { listen } from "@tauri-apps/api/event";
import { chamar, docAtivo } from "./doc";
import { copiarTexto } from "./copiar";
import { EXTENSOES } from "./abas";
import { porGeracao } from "./geracao";
import { t } from "./i18n";

interface Pedido {
  formato: "zod" | "schema" | "ts";
  literal: boolean;
  numero: boolean;
}
interface Validacao {
  id: number;
  geracao: number;
  varridas: number;
  total: number;
  fim: boolean;
  passam: number;
  falham: number;
  invalidas: number;
  falhas: [number, string][];
}

export interface Opcoes {
  fmt: (n: number) => string;
  icone: (id: string) => string;
  /** Nome do arquivo da aba, base da sugestão ao salvar. */
  nome: () => string;
  avisar: (msg: string) => void;
  /** Erro de comando no idioma da interface (o mesmo tradutor dos códigos `jasao:*` da abertura). */
  erro: (e: unknown) => string;
}

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");

export function criarExportar(el: HTMLElement, o: Opcoes) {
  const x = t.exportar;
  const sel = (id: string, rotulo: string, ops: [string, string][]) =>
    `<label>${rotulo}<select id="${id}">${ops.map(([v, r]) => `<option value="${v}">${r}</option>`).join("")}</select></label>`;
  el.innerHTML = `<div class="tools">
      ${sel("ex-saida", x.saida, [["zod", "Zod"], ["schema", "JSON Schema"], ["ts", "TypeScript"]])}
      ${sel("ex-unico", x.unico, [["generico", x.generico], ["literal", x.literal]])}
      ${sel("ex-numero", x.numero, [["texto", x.manterTexto], ["numero", x.tratarNumero]])}
      <div class="acts"><button class="btn" id="ex-copiar" type="button">${o.icone("copy")}<span>${x.copiar}</span></button><button class="btn" id="ex-salvar" type="button">${o.icone("down")}<span>${x.salvar}</span></button><button class="btn" id="ex-outro" type="button"><span>${x.validarOutro}</span></button><button class="btn" id="ex-de-fora" type="button"><span>${x.validarDeFora}</span></button></div>
    </div>
    <p class="check" id="ex-status" role="status" aria-live="polite"></p>
    <ol class="falhas" id="ex-falhas" aria-label="${esc(x.falhasRotulo)}"></ol>
    <pre id="ex-codigo" tabindex="0" aria-label="${esc(x.codigo)}"></pre>`;
  const q = <T extends HTMLElement>(id: string) => el.querySelector<T>(`#${id}`)!;
  const [saida, unico, numero] = ["ex-saida", "ex-unico", "ex-numero"].map((id) => q<HTMLSelectElement>(id));
  const statusEl = q("ex-status");
  const falhasEl = q("ex-falhas");
  const codigoEl = q("ex-codigo");
  const botoes = ["ex-copiar", "ex-salvar", "ex-outro", "ex-de-fora"].map((id) => q<HTMLButtonElement>(id));

  let codigo = "";
  let de: number | null = null; // doc do código mostrado
  // O que está sendo validado: este arquivo (null), outro arquivo ou este contra um schema de fora.
  let alvo: { nome: string; deFora: boolean } | null = null;

  const pedido = (): Pedido => ({ formato: saida.value as Pedido["formato"], literal: unico.value === "literal", numero: numero.value === "numero" });
  const estado = (msg: string, classe = "") => {
    statusEl.className = `check ${classe}`;
    const sobre = alvo ? `<b>${esc(alvo.deFora ? x.contraSchema(alvo.nome) : alvo.nome)}</b> · ` : "";
    statusEl.innerHTML = msg.replace("<span>", `<span>${sobre}`);
  };
  const pronto = (ok: boolean) => botoes.forEach((b) => (b.disabled = !ok));

  function resultado(v: Validacao): void {
    const n = v.passam + v.falham;
    const p = pedido();
    const invalidas = v.invalidas ? ` ${x.invalidas(o.fmt(v.invalidas))}` : "";
    if (!v.falham && v.total === 0 && v.falhas.length) {
      // Schema que não compila é bug do gerador: aparece, não some.
      estado(`${o.icone("alert")}<span>${esc(x.naoCompila(v.falhas[0][1]))}</span>`, "bad");
      falhasEl.innerHTML = "";
      return;
    }
    if (!v.falham) estado(`${o.icone("check")}<span>${x.passam(o.fmt(n))}${invalidas}</span>`);
    else {
      // JSON Schema não converte: "tratar como número" com texto no arquivo reprova o próprio arquivo.
      const porque = p.formato === "schema" && p.numero && !alvo?.deFora ? ` ${x.textoOndeNumero}` : "";
      estado(`${o.icone("alert")}<span>${x.deN(o.fmt(v.passam), o.fmt(n))}${porque}${invalidas}</span>`, "bad");
    }
    falhasEl.innerHTML = v.falhas
      .map(([linha, motivo]) => `<li><b>${x.linha(o.fmt(linha + 1))}</b> <span>${esc(motivo)}</span></li>`)
      .join("");
  }

  const aplicar = (v: Validacao) => {
    if (v.fim) return resultado(v);
    estado(`<span>${x.validando(o.fmt(v.varridas), o.fmt(v.total))}</span>`);
  };
  const eventos = porGeracao<Validacao>(aplicar); // geração da validação mostrada
  void listen<Validacao>("validacao", ({ payload: v }) => {
    if (v.id !== docAtivo() || el.hidden) return;
    eventos.receber(v);
  });

  /** Gera o código das opções atuais e começa a validar; sem inferência ainda, avisa e espera. */
  async function mostrar(): Promise<void> {
    const d = docAtivo();
    const p = pedido();
    const r = await chamar<{ geracao: number; codigo: string } | null>("exportar", { pedido: p });
    if (d !== docAtivo() || el.hidden) return; // trocou de aba ou de vista no meio
    falhasEl.innerHTML = "";
    alvo = null;
    if (!r) {
      de = null;
      codigo = codigoEl.textContent = "";
      pronto(false);
      return estado(`<span>${x.inferindo}</span>`);
    }
    de = d;
    codigo = codigoEl.textContent = r.codigo;
    pronto(true);
    estado(`<span>${x.validando("0", "…")}</span>`);
    eventos.fixar(r.geracao); // aplica o `fim` que chegou antes desta resposta
  }

  /** Para a validação em curso (sair da vista, trocar de aba, Esc). */
  function parar(): void {
    if (!eventos.atual) return;
    eventos.fixar(0);
    void chamar("cancelar_validacao").catch(() => {}); // doc fechado: nada a cancelar
  }

  /** Valida outro arquivo (ou contra schema de fora) escolhido no diálogo do backend. */
  async function validarEscolhido(cmd: "validar_outro" | "validar_com_schema"): Promise<void> {
    const d = docAtivo();
    const args = cmd === "validar_outro" ? { pedido: pedido(), extensoes: EXTENSOES } : {};
    let r: { geracao: number; nome: string } | null;
    try {
      r = await chamar<{ geracao: number; nome: string } | null>(cmd, args);
    } catch (e) {
      // Schema de fora recusado (ref remoto, fundo ou grande demais) ou arquivo que não abre.
      if (d !== docAtivo() || el.hidden) return;
      parar();
      alvo = null;
      falhasEl.innerHTML = "";
      return estado(`${o.icone("alert")}<span>${esc(o.erro(e))}</span>`, "bad");
    }
    if (!r || d !== docAtivo() || el.hidden) return; // cancelou o diálogo, trocou de aba ou de vista
    alvo = { nome: r.nome, deFora: cmd === "validar_com_schema" };
    falhasEl.innerHTML = "";
    estado(`<span>${x.validando("0", "…")}</span>`);
    eventos.fixar(r.geracao);
  }

  const falhou = (e: unknown) => o.avisar(o.erro(e));
  const ajustar = () => (numero.disabled = saida.value === "ts"); // tipos não convertem texto em número
  saida.addEventListener("change", ajustar);
  for (const s of [saida, unico, numero]) s.addEventListener("change", () => void mostrar().catch(falhou));
  botoes[2].addEventListener("click", () => void validarEscolhido("validar_outro"));
  botoes[3].addEventListener("click", () => void validarEscolhido("validar_com_schema"));
  botoes[0].addEventListener("click", () => {
    copiarTexto(codigo).then(() => o.avisar(x.copiado), falhou);
  });
  botoes[1].addEventListener("click", async () => {
    const p = pedido();
    const base = o.nome().replace(/\.(jsonl?|ndjson)(\.(gz|zst))?$/i, "") || "registro";
    try {
      const ext = { zod: ".zod.ts", schema: ".schema.json", ts: ".d.ts" }[p.formato];
      const salvo = await chamar<string | null>("salvar_exportado", { pedido: p, nome: `${base}${ext}` });
      if (salvo) o.avisar(x.salvo(salvo));
    } catch (e) {
      falhou(e);
    }
  });
  el.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || !eventos.atual) return;
    e.preventDefault();
    parar();
    estado(`<span>${x.cancelada}</span>`);
  });

  return {
    mostrar: () => mostrar().catch(falhou),
    parar,
    /** Já mostra o código deste doc (evento de progresso da varredura não precisa refazer). */
    temCodigo: () => de !== null && de === docAtivo(),
  };
}
