# Revisão: código morto e duplicação

Base: `main` em `f5fa2e0`. Linhas citadas são as dessa base. Ferramentas: `rg`, `cargo machete`
(já instalado), `tsc` (`noUnusedLocals`) e compilação. `cargo udeps` não está instalado e não foi
instalado.

## Resumo

- **Removido (4 itens):** 4 re-exports do `nucleo` sem consumidor, uma constante TS, uma chave de
  i18n.
- **Deduplicado (1):** `icone` (3 cópias idênticas) passa a vir de `texto.ts`.
- **Ligado:** o comando Tauri `limpar_recentes` não tinha botão; agora tem, na lista de recentes.
- **Mantido com motivo:** falsos positivos de varredura.
- **Dedup proposto, não feito:** `esc` local × `escAtributo` (muda saída) e três `decodificar` no
  núcleo (semânticas diferentes).

Nenhuma remoção muda comportamento: tudo o que saiu não tinha leitor.

## Tabela

| Item | Arquivo:linha | Evidência | Decisão |
|---|---|---|---|
| re-export `nucleo::ErroBusca` | `core/src/lib.rs:33` | `rg 'nucleo::(ErroBusca\|Lote\|Resumo\|Formato)\b\|nucleo::\{[^}]*\b(ErroBusca\|Lote\|Resumo\|Formato)\b'` no repo inteiro (cli, app, core, benches, testes, fuzz, exemplos, docs): 0 | **Removido** do `pub use`. O tipo segue em `nucleo::busca::ErroBusca`. |
| re-export `nucleo::Lote` | `core/src/lib.rs:33` | idem | **Removido**; segue em `nucleo::busca::Lote`. |
| re-export `nucleo::Resumo` (o de `busca`) | `core/src/lib.rs:33` | idem; ainda convivia com `pub mod resumo` (ambíguo para quem lê) | **Removido**; segue em `nucleo::busca::Resumo`. |
| re-export `nucleo::Formato` | `core/src/lib.rs:34` | idem | **Removido**; segue em `nucleo::compactado::Formato`. |
| `export const INVALIDA = 255` | `app/src/regua.ts:10` | `rg -w INVALIDA app/src app/tests`: só a definição. A cor de inválida vem pronta do Rust (`nucleo::regua::INVALIDA`). | **Removido.** |
| chave i18n `tabela.ajuda` (pt e en) | `app/src/i18n.ts:224`, `:663` | script que percorre `t` e procura cada chave fora do bloco de definição (`src/*.ts`, `index.html`): só `ajuda` aparece sem leitor; `git log -S ajuda -- app/src`: nunca teve leitor | **Removido.** Se a intenção era descrever a grade para leitor de tela, isso é feature, não código morto (anotado abaixo). |
| `icone` triplicado | `app/src/main.ts:77`, `app/src/grade.ts:46`, `app/src/apresenta.ts:22` | corpo idêntico, byte a byte | **Deduplicado**: `export const icone` em `app/src/texto.ts`, importado pelos três (todos já importavam de `texto`). |
| comando `limpar_recentes` | `app/src-tauri/src/lib.rs:683`, registrado em `:3079` | `rg limpar_recentes`: só o registro e o teste `recentes_pelo_ipc` (`lib.rs:5070`); nenhum `chamar`/`invoke` no front, nenhum item de menu. `docs/decisoes.md:253` descreve "Limpar recentes" como decisão. | **Ligado na UI** neste PR: botão "Limpar" no cabeçalho da lista de recentes da aba vazia (`app/src/recentes.ts`). |
| demais comandos Tauri | `app/src-tauri/src/lib.rs:3072-3132` | `comm` entre os 59 registrados e as strings de `chamar(...)`/`invoke(...)` em `app/src` | Todos chamados. Nenhum `invoke` para comando inexistente. |
| `serde_json` em `core/fuzz/Cargo.toml` | `core/fuzz/Cargo.toml:15` | `cargo machete` acusa | **Mantido (falso positivo):** usado em `core/fuzz/comum/formatos.rs:29`, incluído por `#[path]` nos alvos `formato_*`. |
| demais dependências Rust | `Cargo.toml` de core, cli, app | `cargo machete`: nada além do item acima | Nada a remover. |
| dependências npm | `app/package.json` | `@tauri-apps/api` (9 arquivos), `plugin-dialog` (`main.ts:8`), `zod` (`tests/dourados/*.zod.ts`), `vite`/`typescript`/`@tauri-apps/cli` (scripts) | Todas em uso. |
| `pub` do núcleo usado só no próprio módulo: `Atencao`, `MAX_OPCIONAIS`, `aviso_pt` (laudo), `Extremos`, `Numeros`, `TOP` (perfil), `LoteErros` (erros), `MAX_LOTE_PADRAO`, `com_max_bytes_lote` (consulta), `UNICO_MAX_BYTES` (estrutura), `de_curl` (url), `mascarar_em` (mascara) | `core/src/*` | varredura dos 320 nomes `pub` do núcleo: estes aparecem só no arquivo de definição, mas **com uso** (campo de struct pública, chamada interna, teste) | **Mantido.** Não é código morto; no máximo `pub` largo demais. Estreitar para `pub(crate)` não muda nada para quem usa e só gera conflito com as outras frentes. `com_max_bytes_lote` só tem leitor em teste (`consulta.rs:913`). |
| exports TS usados só no próprio arquivo: `MAX_PREVIA`, `Diferenca`, `RegistroDiferente`, `CaminhoE`, `MAX_PX`, `TipoValor`, `Indisponivel` | `app/src/*` | sem importador fora, mas usados dentro (tipos de assinatura exportada) | **Mantido.** Tirar o `export` não remove código. |
| `#[allow(unused_mut)]` | `app/src-tauri/src/filho.rs:53` | o `mut` só é usado sob `#[cfg(test)]` (linhas seguintes) | **Mantido**, legítimo. |
| `#[cfg(...)]` | core, cli, app | todos `unix`/`macos`/`windows`/`test`/`desktop` com par para a outra plataforma | **Mantido.** Nenhum esconde código sem uso. |
| scripts | `scripts/{exemplos,gates,medir}.sh/py` | citados em `CONTRIBUTING.md`, `README.md`, `ci.yml`, `docs/decisoes.md`, `app/src-tauri/src/exemplos.rs` | **Mantido.** |
| exemplo `gerar` | `core/examples/gerar.rs` | usado por `scripts/medir.sh` e `README.md` | **Mantido.** |
| `docs/mockup-visualizacao.html` | `docs/` | nenhum link no repo (o `mockup-json.html` é citado por `app/index.html` e `decisoes.md`) | **Mantido.** É referência de design, não código; decidir se arquiva. |
| `esc` local × `escAtributo` | `app/src/tabela.ts:48`, `app/src/grade.ts:45`, `app/src/exportar.ts:40` × `app/src/texto.ts:44` | os três locais fazem `& < "` e são idênticos entre si; `escAtributo` faz o mesmo **depois** de `semInvisiveis` | **Dedup proposto, não feito:** trocar pelo `escAtributo` muda a saída (invisíveis viram `<U+…>`). Ver nota para a frente de bugs. |
| `decodificar` ×3 | `core/src/transformar.rs:112`, `core/src/tabela.rs:437`, `core/src/exportar.rs:59` | mesmo propósito (texto JSON cru → `String`), mas serde vs sonic e `String` com fallback vs `Option` | **Dedup proposto, não feito:** não é troca mecânica; unificar exige escolher uma semântica. |
| helpers de teste repetidos (`arquivo`, `jsonl`) | `core/src/{resumo,regua,laudo,transformar,...}.rs` em `mod testes` | mesmos 3-4 linhas em vários módulos de teste | **Mantido.** Só teste; um módulo comum de teste custa mais que as cópias. |

## Notas para as outras frentes (não mexido aqui)

- **Bugs:** `esc` em `tabela.ts`, `grade.ts` e `exportar.ts` monta atributos (`title`, `aria-label`)
  sem `semInvisiveis`, ao contrário de `main.ts`, `estrutura.ts` e `apresenta.ts`. Nome de campo com
  caractere bidi/invisível aparece cru nesses três. Se for para neutralizar, trocar por
  `escAtributo` resolve e remove a duplicação.
- **Produto:** `tabela.ajuda` (removida)
  sugere que a grade teria uma descrição de teclado para leitor de tela; hoje não tem.
