// "Salvar como…": grava num arquivo NOVO o arquivo inteiro, o resultado da consulta jq ou os
// registros do filtro da Tabela, com manter/tirar/renomear, em JSONL ou JSON. O Rust escolhe o
// destino (diálogo do sistema), recusa o original e os abertos e só troca o temporário pelo
// destino no fim (`salvar_como`); progresso pelo evento `salvando`, cancelar por `cancelar_salvar`.
import { listen } from "@tauri-apps/api/event";
import type { Alvo } from "./tabela";
import { erroDoRust, t } from "./i18n";
import { chamar } from "./doc";

interface Andamento {
  id: number;
  geracao: number;
  resultados: number;
  linhas: number;
  total: number;
  fim: boolean;
  cancelado: boolean;
  arquivo: string | null;
  falha: string | null;
  erros: number; // registros descartados (linha inválida ou erro do jq)
  primeiro_erro: [number, string] | null;
}

/** O que a aba ativa oferece como fonte. */
export interface Contexto {
  id: number;
  caminho: string; // o arquivo do usuário (o original, se há resultado de consulta aberto)
  nome: string;
  jq: string | null; // filtro da consulta, se houver
  filtro: Alvo | null; // filtro da Tabela; só vale com o original aberto na aba
}

const lista = (s: string) => s.split(",").map((x) => x.trim()).filter(Boolean);

export function criarSalvarComo(dialogo: HTMLDialogElement, o: { fmt(n: number): string; avisar(msg: string): void }) {
  const s = t.salvarComo;
  dialogo.innerHTML = `
    <header><h2 id="salvar-como-titulo">${s.titulo}</h2></header>
    <form method="dialog" class="salvar-como">
      <fieldset><legend>${s.fonte}</legend>
        <label><input type="radio" name="fonte" value="tudo" checked> ${s.tudo}</label>
        <label><input type="radio" name="fonte" value="jq"> ${s.jq} <code data-jq></code></label>
        <label><input type="radio" name="fonte" value="filtro"> ${s.filtro}</label>
      </fieldset>
      <fieldset><legend>${s.campos}</legend>
        <label>${s.manter} <input name="manter" placeholder="id, nome" spellcheck="false"></label>
        <label>${s.tirar} <input name="tirar" placeholder="senha" spellcheck="false"></label>
        <label>${s.renomear} <input name="renomear" placeholder="nome=quem" spellcheck="false"></label>
        <small>${s.raiz}</small>
      </fieldset>
      <fieldset><legend>${s.formato}</legend>
        <select name="saida">
          <option value="jsonl">JSONL</option>
          <option value="json">${s.json}</option>
          <option value="json_compacto">${s.jsonCompacto}</option>
        </select>
      </fieldset>
      <fieldset><legend>${s.mascara}</legend>
        <label><input type="radio" name="mascara" value="mascarar" checked> ${s.mascarar}</label>
        <label><input type="radio" name="mascara" value="anonimizar"> ${s.anonimizar}</label>
        <label><input type="radio" name="mascara" value="sem_mascara"> ${s.semMascara}</label>
        <p class="aviso" data-aviso hidden>${s.avisoSemMascara}</p>
      </fieldset>
      <p role="status" aria-live="polite" data-info></p>
      <footer>
        <button class="btn" type="button" data-fechar>${s.fechar}</button>
        <button class="btn" type="button" data-cancelar hidden>${t.consulta.cancelar}</button>
        <button class="btn" type="submit" data-salvar>${s.salvar}</button>
      </footer>
    </form>`;
  dialogo.setAttribute("aria-labelledby", "salvar-como-titulo");
  const form = dialogo.querySelector("form")!;
  const q = <T extends Element>(sel: string) => form.querySelector<T>(sel)!;
  const info = q<HTMLElement>("[data-info]");
  const salvarBtn = q<HTMLButtonElement>("[data-salvar]");
  const cancelarBtn = q<HTMLButtonElement>("[data-cancelar]");
  const radio = (v: string) => q<HTMLInputElement>(`input[name=fonte][value=${v}]`);
  let ctx: Contexto | null = null;
  let emCurso: { id: number; geracao: number } | null = null;

  const valor = (n: string) => (form.elements.namedItem(n) as HTMLInputElement | HTMLSelectElement).value;
  const ocupado = (sim: boolean) => {
    salvarBtn.disabled = sim;
    cancelarBtn.hidden = !sim;
  };
  form.addEventListener("change", () => {
    q<HTMLElement>("[data-aviso]").hidden = valor("mascara") !== "sem_mascara";
  });
  q<HTMLButtonElement>("[data-fechar]").addEventListener("click", () => dialogo.close());
  cancelarBtn.addEventListener("click", () => emCurso && void chamar("cancelar_salvar", { id: emCurso.id }));
  // Fechar com salvamento em curso cancela: nada fica pela metade no destino.
  dialogo.addEventListener("close", () => emCurso && void chamar("cancelar_salvar", { id: emCurso.id }));

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    if (!ctx || emCurso) return;
    const c = ctx;
    const tipo = valor("fonte");
    const fonte = tipo === "jq" ? { tipo, filtro: c.jq } : tipo === "filtro" ? { tipo, alvo: c.filtro } : { tipo: "tudo" };
    const renomear: [string, string][] = [];
    for (const par of lista(valor("renomear"))) {
      const [de, para] = par.split("=").map((x) => x.trim());
      if (!de || !para) {
        info.textContent = s.renomearInvalido(par);
        return;
      }
      renomear.push([de, para]);
    }
    const pedido = {
      fonte,
      transformacao: { manter: lista(valor("manter")), tirar: lista(valor("tirar")), renomear },
      saida: valor("saida"),
      mascara: valor("mascara"),
    };
    ocupado(true);
    info.textContent = "";
    try {
      const g = await chamar<number | null>("salvar_como", { id: c.id, caminho: c.caminho, pedido, nome: c.nome });
      if (g === null) ocupado(false);
      else emCurso = { id: c.id, geracao: g };
    } catch (x) {
      ocupado(false);
      info.textContent = `${t.erro}: ${erroDoRust((x as { mensagem?: string }).mensagem ?? x)}`;
    }
  });

  void listen<Andamento>("salvando", ({ payload: p }) => {
    if (!emCurso || p.id !== emCurso.id || p.geracao !== emCurso.geracao) return;
    const pct = p.total ? Math.floor((100 * p.linhas) / p.total) : 0;
    if (!p.fim) {
      info.textContent = s.andamento(o.fmt(p.resultados), pct);
      return;
    }
    emCurso = null;
    ocupado(false);
    let msg = p.falha ? `${t.erro}: ${erroDoRust(p.falha)}` : p.cancelado ? s.cancelado : s.salvo(p.arquivo ?? "");
    if (p.arquivo && p.erros > 0) {
      const [linha, motivo] = p.primeiro_erro ?? [0, ""];
      msg += `. ${s.descartados(o.fmt(p.erros), linha + 1, motivo)}`;
    }
    info.textContent = msg;
    if (!dialogo.open) o.avisar(msg);
  });

  return {
    abrir(c: Contexto): void {
      ctx = c;
      radio("jq").disabled = !c.jq;
      radio("filtro").disabled = !c.filtro;
      q<HTMLElement>("[data-jq]").textContent = c.jq ?? "";
      if (form.querySelector<HTMLInputElement>("input[name=fonte]:checked")!.disabled) radio("tudo").checked = true;
      if (!emCurso) info.textContent = "";
      dialogo.showModal();
    },
  };
}
