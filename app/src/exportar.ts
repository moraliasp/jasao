// Vista Exportar: Zod ou JSON Schema da estrutura inferida, gerados no núcleo (`exportar`) e
// validados contra o arquivo inteiro numa thread do Rust (evento `validacao`). O status é honesto:
// diz quantos registros passam e mostra as primeiras falhas com linha e motivo.
import { listen } from "@tauri-apps/api/event";
import { chamar, docAtivo } from "./doc";
import { copiarTexto } from "./copiar";
import { t } from "./i18n";

interface Pedido {
  zod: boolean;
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
}

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");

export function criarExportar(el: HTMLElement, o: Opcoes) {
  const x = t.exportar;
  const sel = (id: string, rotulo: string, ops: [string, string][]) =>
    `<label>${rotulo}<select id="${id}">${ops.map(([v, r]) => `<option value="${v}">${r}</option>`).join("")}</select></label>`;
  el.innerHTML = `<div class="tools">
      ${sel("ex-saida", x.saida, [["zod", "Zod"], ["schema", "JSON Schema"]])}
      ${sel("ex-unico", x.unico, [["generico", x.generico], ["literal", x.literal]])}
      ${sel("ex-numero", x.numero, [["texto", x.manterTexto], ["numero", x.tratarNumero]])}
      <div class="acts"><button class="btn" id="ex-copiar" type="button">${o.icone("copy")}<span>${x.copiar}</span></button><button class="btn" id="ex-salvar" type="button">${o.icone("down")}<span>${x.salvar}</span></button></div>
    </div>
    <p class="check" id="ex-status" role="status" aria-live="polite"></p>
    <ol class="falhas" id="ex-falhas" aria-label="${esc(x.falhasRotulo)}"></ol>
    <pre id="ex-codigo" tabindex="0" aria-label="${esc(x.codigo)}"></pre>`;
  const q = <T extends HTMLElement>(id: string) => el.querySelector<T>(`#${id}`)!;
  const [saida, unico, numero] = ["ex-saida", "ex-unico", "ex-numero"].map((id) => q<HTMLSelectElement>(id));
  const statusEl = q("ex-status");
  const falhasEl = q("ex-falhas");
  const codigoEl = q("ex-codigo");
  const botoes = [q<HTMLButtonElement>("ex-copiar"), q<HTMLButtonElement>("ex-salvar")];

  let geracao = 0; // da validação mostrada; 0: nenhuma (evento é descartado)
  let codigo = "";
  let de: number | null = null; // doc do código mostrado

  const pedido = (): Pedido => ({ zod: saida.value === "zod", literal: unico.value === "literal", numero: numero.value === "numero" });
  const estado = (msg: string, classe = "") => {
    statusEl.className = `check ${classe}`;
    statusEl.innerHTML = msg;
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
      const porque = !p.zod && p.numero ? ` ${x.textoOndeNumero}` : "";
      estado(`${o.icone("alert")}<span>${x.deN(o.fmt(v.passam), o.fmt(n))}${porque}${invalidas}</span>`, "bad");
    }
    falhasEl.innerHTML = v.falhas
      .map(([linha, motivo]) => `<li><b>${x.linha(o.fmt(linha + 1))}</b> <span>${esc(motivo)}</span></li>`)
      .join("");
  }

  void listen<Validacao>("validacao", ({ payload: v }) => {
    if (v.id !== docAtivo() || v.geracao !== geracao || el.hidden) return;
    if (v.fim) return resultado(v);
    estado(`<span>${x.validando(o.fmt(v.varridas), o.fmt(v.total))}</span>`);
  });

  /** Gera o código das opções atuais e começa a validar; sem inferência ainda, avisa e espera. */
  async function mostrar(): Promise<void> {
    const d = docAtivo();
    const p = pedido();
    const r = await chamar<{ geracao: number; codigo: string } | null>("exportar", { pedido: p });
    if (d !== docAtivo() || el.hidden) return; // trocou de aba ou de vista no meio
    falhasEl.innerHTML = "";
    if (!r) {
      de = null;
      codigo = codigoEl.textContent = "";
      pronto(false);
      return estado(`<span>${x.inferindo}</span>`);
    }
    de = d;
    geracao = r.geracao;
    codigo = codigoEl.textContent = r.codigo;
    pronto(true);
    estado(`<span>${x.validando("0", "…")}</span>`);
  }

  /** Para a validação em curso (sair da vista, trocar de aba, Esc). */
  function parar(): void {
    if (!geracao) return;
    geracao = 0;
    void chamar("cancelar_validacao").catch(() => {}); // doc fechado: nada a cancelar
  }

  const falhou = (e: unknown) => o.avisar(`${t.erro}: ${e}`);
  for (const s of [saida, unico, numero]) s.addEventListener("change", () => void mostrar().catch(falhou));
  botoes[0].addEventListener("click", () => {
    copiarTexto(codigo).then(() => o.avisar(x.copiado), falhou);
  });
  botoes[1].addEventListener("click", async () => {
    const p = pedido();
    const base = o.nome().replace(/\.(jsonl?|ndjson)(\.(gz|zst))?$/i, "") || "registro";
    try {
      const salvo = await chamar<string | null>("salvar_exportado", { pedido: p, nome: `${base}${p.zod ? ".zod.ts" : ".schema.json"}` });
      if (salvo) o.avisar(x.salvo(salvo));
    } catch (e) {
      falhou(e);
    }
  });
  el.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || !geracao) return;
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
