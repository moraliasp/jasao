// Micronavegador: régua de 44 px ao lado da Árvore e do Texto com o arquivo inteiro em baldes
// (cor da variante do discriminador, inválidas em vermelho) e o retângulo da janela visível.
// O Rust guarda uma marca por registro (`nucleo::regua`); aqui só se pede o que cabe em pixels.
import { chamar } from "./doc";
import { t } from "./i18n";

export interface Legenda { campo: string; variantes: [string, number][]; outros: number }
interface Baldes { cores: number[]; invalidas: [number, number, number][] }

const OUTROS = 5;
const MIN_JANELA = 6; // px: altura mínima do retângulo
const PERTO = 16; // px: inválidas mais próximas que isto viram um balão só
const MARCAS = 1_000; // registros por pedido de marcas (= MAX_JANELA do Rust)
const INTERVALO = 200; // ms entre releituras da régua durante a varredura

export interface Opcoes {
  /** Rola a visão até o registro `r` (arraste e teclado). */
  ir(r: number): void;
  /** Seleciona o registro `r` (clique num balão de inválidas). */
  selecionar(r: number): void;
  /** As marcas de registros visíveis chegaram: redesenhar os pontos de cor. */
  marcasChegaram(): void;
  fmt(n: number): string;
}

export function criarRegua(el: HTMLElement, legendaEl: HTMLElement, o: Opcoes) {
  el.innerHTML = `<canvas class="rb"></canvas><canvas class="rc"></canvas><canvas class="rs"></canvas><div class="janela"></div><div class="baloes"></div>`;
  const [base, cor, achadosEl] = el.querySelectorAll("canvas");
  const janelaEl = el.querySelector<HTMLElement>(".janela")!;
  const baloesEl = el.querySelector<HTMLElement>(".baloes")!;
  el.setAttribute("aria-label", t.regua.rotulo);

  let registros = 0;
  let visivel = false;
  let geracao = 0; // muda a cada arquivo: resposta velha não pinta
  let legenda: Legenda | null = null;
  let coloriu = false;
  let baldes: Baldes | null = null;
  let ultima = 0; // hora da última leitura durante a varredura
  let espera: number | null = null;
  let janela: [number, number] = [0, 0];
  let marcas: { inicio: number; valores: number[] } | null = null;
  let pedidoMarcas: number | null = null;
  let busca = false; // mostra os achados da busca
  let lendoBusca = false;
  let deNovo = false;

  const token = (nome: string) => getComputedStyle(document.documentElement).getPropertyValue(nome).trim();

  function pintar(c: HTMLCanvasElement, b: Baldes, comCor: boolean): void {
    const dpr = devicePixelRatio || 1;
    const [w, h] = [el.clientWidth, el.clientHeight];
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
    const g = c.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const paleta = ["--rg-neutro", "--rg-1", "--rg-2", "--rg-3", "--rg-4", "--rg-outros"].map(token);
    const alto = h / Math.max(1, b.cores.length);
    for (let i = 0; i < b.cores.length; i++) {
      g.fillStyle = paleta[comCor ? (b.cores[i] ?? 0) : 0] ?? paleta[0];
      g.fillRect(0, i * alto, w, Math.ceil(alto));
    }
    g.fillStyle = token("--del");
    for (const [i] of b.invalidas) g.fillRect(0, i * alto, w, Math.max(2, alto));
  }

  function desenhar(): void {
    if (!visivel || !baldes) return;
    pintar(base, baldes, false);
    if (coloriu && legenda) pintar(cor, baldes, true);
    desenharBaloes();
    desenharJanela();
  }

  const baldesK = () => Math.min(4096, Math.max(1, Math.floor(el.clientHeight)));

  /** Achados da busca: um traço na borda esquerda por balde com achado. Pedido que chega durante
   * uma leitura vira mais uma leitura no fim dela (a última sempre pinta). */
  async function lerBusca(): Promise<void> {
    if (lendoBusca) return void (deNovo = true);
    lendoBusca = true;
    const c = achadosEl;
    const g = geracao;
    const k = baldesK();
    try {
      const lista = visivel && busca && el.clientHeight ? await chamar<number[]>("regua_busca", { k }) : [];
      if (g !== geracao) return;
      const dpr = devicePixelRatio || 1;
      c.width = Math.round(el.clientWidth * dpr);
      c.height = Math.round(el.clientHeight * dpr);
      const ctx = c.getContext("2d")!;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const alto = el.clientHeight / k;
      ctx.fillStyle = token("--change");
      for (const i of lista) ctx.fillRect(0, i * alto, 10, Math.max(2, alto));
    } finally {
      lendoBusca = false;
      if (deNovo) {
        deNovo = false;
        void lerBusca();
      }
    }
  }

  async function ler(): Promise<void> {
    void lerBusca();
    if (!visivel || !el.clientHeight) return;
    const g = geracao;
    const k = baldesK();
    const b = await chamar<Baldes>("regua", { k });
    if (g !== geracao) return;
    baldes = b;
    desenhar();
  }

  /** Durante a varredura: no máximo uma leitura a cada `INTERVALO`. */
  function lerLogo(): void {
    if (espera !== null) return;
    const falta = Math.max(0, INTERVALO - (performance.now() - ultima));
    espera = window.setTimeout(() => {
      espera = null;
      ultima = performance.now();
      void ler();
    }, falta);
  }

  function desenharJanela(): void {
    const h = el.clientHeight;
    if (!registros || !h) return;
    const [a, b] = janela;
    const topo = (a / registros) * h;
    const alto = Math.max(MIN_JANELA, ((b - a + 1) / registros) * h);
    janelaEl.style.transform = `translateY(${Math.min(topo, h - alto)}px)`;
    janelaEl.style.height = `${alto}px`;
    el.setAttribute("aria-valuemax", String(registros));
    el.setAttribute("aria-valuenow", String(a + 1));
    el.setAttribute("aria-valuetext", t.regua.faixa(o.fmt(a + 1), o.fmt(b + 1), o.fmt(registros)));
  }

  /** Um balão por grupo de baldes com inválidas a menos de `PERTO` px um do outro. Balão que já
   * existia fica (senão o pulso recomeça a cada leitura); só os novos pulsam. */
  function desenharBaloes(): void {
    if (!baldes) return;
    const alto = el.clientHeight / Math.max(1, baldes.cores.length);
    const grupos: { y: number; quantas: number; primeiro: number }[] = [];
    for (const [i, quantas, primeiro] of baldes.invalidas) {
      const y = i * alto;
      const ult = grupos[grupos.length - 1];
      if (ult && y - ult.y < PERTO) {
        ult.quantas += quantas;
        ult.primeiro = Math.min(ult.primeiro, primeiro);
      } else grupos.push({ y, quantas, primeiro });
    }
    const antes = new Map([...baloesEl.children].map((b) => [(b as HTMLElement).dataset.y, b as HTMLElement]));
    for (const gr of grupos) {
      const chave = String(Math.round(gr.y));
      let b = antes.get(chave);
      antes.delete(chave);
      if (!b) {
        const novo = document.createElement("button");
        novo.type = "button";
        b = novo;
        b.className = "balao";
        b.dataset.y = chave;
        baloesEl.append(b);
      }
      b.style.top = `${Math.min(gr.y, el.clientHeight - 18)}px`;
      b.dataset.r = String(gr.primeiro);
      b.textContent = gr.quantas > 99 ? "99+" : String(gr.quantas);
      b.setAttribute("aria-label", t.regua.invalidas(gr.quantas, o.fmt(gr.primeiro + 1)));
    }
    for (const b of antes.values()) b.remove();
  }

  function desenharLegenda(): void {
    legendaEl.hidden = !(visivel && legenda);
    if (!legenda) {
      legendaEl.innerHTML = "";
      return;
    }
    const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
    const chip = (classe: string, nome: string, n: number) =>
      `<li title="${esc(o.fmt(n))} ${esc(t.registros[n === 1 ? 0 : 1])}"><i class="pt ${classe}"></i>${esc(nome)}</li>`;
    legendaEl.innerHTML =
      `<span>${t.regua.por} <code>${esc(legenda.campo.replace(/^\./, ""))}</code></span><ul>` +
      legenda.variantes.map(([v, n], i) => chip(`c${i + 1}`, v, n)).join("") +
      (legenda.outros ? chip(`c${OUTROS}`, t.regua.outros, legenda.outros) : "") +
      "</ul>";
  }

  // ---- Arraste, clique e teclado. ----
  const registroEm = (y: number) => Math.floor((y / el.clientHeight) * registros);
  const centrar = (r: number) => {
    const meio = Math.floor((janela[1] - janela[0]) / 2);
    o.ir(Math.max(0, Math.min(registros - 1, r - meio)));
  };
  let arrastando = false;
  el.addEventListener("pointerdown", (e) => {
    const b = (e.target as HTMLElement).closest<HTMLElement>(".balao");
    if (b) return o.selecionar(Number(b.dataset.r));
    arrastando = true;
    el.setPointerCapture(e.pointerId);
    centrar(registroEm(e.offsetY));
    el.focus({ preventScroll: true });
  });
  el.addEventListener("pointermove", (e) => {
    if (arrastando) centrar(registroEm(e.clientY - el.getBoundingClientRect().top));
  });
  const soltar = () => (arrastando = false);
  el.addEventListener("pointerup", soltar);
  el.addEventListener("pointercancel", soltar);
  el.addEventListener("keydown", (e) => {
    if ((e.target as HTMLElement).closest(".balao")) return;
    const pagina = Math.max(1, janela[1] - janela[0]);
    const passo = Math.max(pagina, Math.floor(registros / 10));
    const a = janela[0];
    const destino = ({
      ArrowDown: a + pagina, ArrowRight: a + pagina, ArrowUp: a - pagina, ArrowLeft: a - pagina,
      PageDown: a + passo, PageUp: a - passo, Home: 0, End: registros - 1 - pagina,
    } as Record<string, number>)[e.key];
    if (destino === undefined) return;
    e.preventDefault();
    o.ir(Math.max(0, Math.min(registros - 1, destino)));
  });
  new ResizeObserver(() => void ler()).observe(el);

  async function pedirMarcas(inicio: number): Promise<void> {
    if (pedidoMarcas === inicio) return;
    pedidoMarcas = inicio;
    const g = geracao;
    try {
      const valores = await chamar<number[]>("marcas", { inicio, quantidade: MARCAS });
      if (g === geracao) marcas = { inicio, valores };
    } finally {
      if (pedidoMarcas === inicio) pedidoMarcas = null;
    }
    if (g === geracao) o.marcasChegaram();
  }

  return {
    /** Arquivo novo: régua neutra até a cor chegar. */
    abrir(n: number, comRegua: boolean): void {
      geracao++;
      registros = n;
      visivel = comRegua && n > 1;
      el.hidden = !visivel;
      legenda = null;
      coloriu = false;
      baldes = null;
      marcas = null;
      cor.classList.remove("pronta");
      baloesEl.innerHTML = "";
      desenharLegenda();
      void ler();
    },
    /** Aviso da varredura: inválidas novas e, no fim da 2ª passada, a cor. */
    andamento(p: { legenda: Legenda | null; coloriu: boolean }): void {
      if (!visivel) return;
      if (p.coloriu && !coloriu) {
        coloriu = true;
        legenda = p.legenda;
        marcas = null; // as de antes não tinham cor
        desenharLegenda();
        void ler().then(() => cor.classList.toggle("pronta", legenda !== null));
        o.marcasChegaram();
      } else if (!coloriu) lerLogo();
    },
    /** Registros visíveis agora (base 0, inclusivo). */
    janela(a: number, b: number): void {
      janela = [a, Math.max(a, b)];
      if (visivel) desenharJanela();
    },
    redesenhar(): void {
      desenhar();
      void lerBusca();
    },
    /** Busca mudou: `ativa` liga ou desliga os traços dos achados. */
    busca(ativa: boolean): void {
      busca = ativa;
      void lerBusca();
    },
    /** Classe do ponto de cor do registro `r` ("" sem cor), pedindo as marcas se faltarem. */
    ponto(r: number): string | null {
      if (!visivel || !legenda) return null;
      if (!marcas || r < marcas.inicio || r >= marcas.inicio + marcas.valores.length) {
        if (pedidoMarcas === null) void pedirMarcas(Math.max(0, r - MARCAS / 2));
        return "";
      }
      const m = marcas.valores[r - marcas.inicio];
      return m >= 1 && m <= OUTROS ? `c${m}` : "";
    },
  };
}
