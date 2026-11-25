# Revisão: bugs e qualidade

Escopo: `core/src/`, `cli/src/main.rs`, `app/src-tauri/src/`, `app/src/*.ts`. Conta como bug só o que tem prova: teste que falha antes da correção e passa depois. O que ficou sem prova é **suspeita** e não foi corrigido. Achado comprovado cuja correção é decisão de produto ou de formato fica como **confirmado, não corrigido**, com o motivo.

## Corrigidos (com teste de regressão)

| severidade | arquivo:linha | cenário | status |
|---|---|---|---|
| alta | core/src/valores.rs (`par`, `arquivos`, `registros`); cli `diff_valores`; app/src-tauri/src/lib.rs:1656 | `diff --valores` comparava os bytes já mascarados: um segredo trocado saía como "iguais" com rc=0, mesmo com `--falha-em qualquer`. Agora compara o cru e mostra a prévia mascarada (`valores::Lado`). O diff passa a revelar *que* o segredo mudou, nunca o valor. Testes: `compara_cru_mostra_mascarado` (núcleo) e `diff_valores_segredo_trocado_e_diferenca_mascarada` (CLI, fail→pass: rc 0 → 1). Entrou no commit 1ab4c52, depois da verificação de bf26185 | corrigido |
| alta | core/src/arvore.rs:650 | `\u` sem 4 hexadecimais (`"\uABC\""`, `\uZZZZ`, `\u12`) passava na validação e a linha virava "válida". Também furava o limite de profundidade, porque a varredura pulava o escape errado. Teste: `escape_u_sem_4_hexa_e_erro_sem_panic_nem_burlar_profundidade` | corrigido |
| média | core/src/linha_do_tempo.rs (`hora`, `fuso`) | O fuso do nome do arquivo era ignorado: `…T10:00:00-03:00` ordenava antes de `…T12:00:00Z`, e `T10:00-03:00` virava 10:00:03. Agora `Z`/`±HH:MM`/`±HHMM` depois da hora leva o horário para UTC. O separador dos minutos vale para os segundos, então `10-00-03` continua sendo 10:00:03. Teste: `datas_no_nome` (fail→pass: 10:00Z → 13:00Z). Entrou depois da verificação de bf26185 | corrigido |
| média | cli/src/main.rs:294, 853 | `jasao transformar x -o -` lia o stdin e gravava a saída no temporário do stdin. Agora o `-` depois de `-o`/`--destino` não conta como stdin, e `-o -` dá erro de uso. Teste: `transformar_com_destino_traco_nao_le_stdin` | corrigido |
| baixa | cli/src/main.rs:353 | `CURL https://…` era tratado como remoto e depois falhava com a mensagem enganosa "a URL precisa começar com https://". `remoto()` agora aceita só o que `Pedido::interpretar` aceita. Teste: `remoto_reconhece_so_o_que_interpretar_aceita` | corrigido |
| média | core/src/compactado.rs:510 | `limpar` propagava ENOENT quando um arquivo do cache sumia no meio da varredura (outra instância limpando ao mesmo tempo): 16 erros em 2000 rodadas antes, 0 depois. Só `NotFound` é pulado; qualquer outro erro sobe como antes. Teste: `limpar_nao_falha_com_arquivo_sumindo_no_meio` | corrigido |
| baixa | core/src/tabela.rs:293 | A prévia de uma linha inválida cortava os bytes no meio de um caractere multibyte e mostrava um U+FFFD espúrio. A prévia passa por `arvore::previa`, a mesma da Árvore, então uma linha longa agora termina em `…` (antes era cortada seca). Teste: `previa_de_linha_invalida_nao_parte_caractere` | corrigido |
| média | app/src/geracao.ts:15 | `fixar(g)` descartava o `fim` já guardado de uma geração mais nova quando as respostas chegavam fora de ordem, e a tela ficava esperando. Teste: "fim de uma geração mais nova sobrevive a fixar uma mais velha" | corrigido |
| baixa | app/src/estrutura.ts:118 | `corte` partia um par substituto (emoji) no meio, deixando um caractere quebrado no rótulo. Teste em `estrutura.test.ts` | corrigido |
| baixa | app/src/exportar.ts:71 → app/src/texto.ts (`aposSpan`) | `String.replace` interpretava `$&`, `$1` etc. vindos do nome do arquivo na mensagem. Agora a substituição é por função, extraída para `aposSpan`. Teste em `texto.test.ts` (fail→pass) | corrigido |
| média | app/src/consulta.ts:216 → app/src/geracao.ts (`eventoVale`) | O `fim` de uma consulta cancelada, emitido no mesmo instante do cancelamento, ainda abria o resultado. O guard agora exige `rodando` e foi extraído para `eventoVale`. Teste em `geracao.test.ts` (fail→pass) | corrigido |

## Confirmados, não corrigidos (decisão de produto ou de formato)

| severidade | arquivo:linha | cenário | status |
|---|---|---|---|
| média | core/src/estrutura.rs:767 | `chave_de_segredo` não pega chave concatenada (`ACCESSTOKEN`, `newpassword`). Alargar a heurística muda o que é mascarado | confirmado, não corrigido |
| média | core/src/mascara.rs:37 | CPF ou cartão guardado como número, sob chave neutra, não é mascarado | confirmado, não corrigido |
| média | cli/src/main.rs (`supervisionar`, ~706) | No SIGTERM do pai, o filho `jq` fica órfão e sem prazo. A correção (grupo de processos ou handler de sinal) não é mínima | confirmado, não corrigido |
| média | core/src/filtro.rs:203 vs core/src/transformar.rs:143 | Com chave duplicada, o filtro da Tabela usa a primeira ocorrência e o "Salvar como" (jaq) usa a última | confirmado, não corrigido |
| baixa/média | core/src/perfil.rs:486 vs core/src/transformar.rs:126 | Chave com escape (`{"a":1}`) dá resultado diferente na Tabela e no arquivo salvo | confirmado, não corrigido |
| baixa/média | core/src/perfil.rs:277-289, 411 | Mínimo e máximo do perfil erram além da precisão do f64; `1e-400` cai na faixa do zero | confirmado, não corrigido |
| baixa/média | core/src/arvore.rs:285 | Uma linha JSONL só com espaço aparece como erro (FimInesperado) na Árvore, contrariando `docs/decisoes.md:83` e `erros::validar_linha`, que a contam como em branco | confirmado, não corrigido |
| baixa | core/src/busca.rs:172, 206 | Regex com `\A` e `^`/`$` sem multiline depende do tamanho do bloco | confirmado, não corrigido |
| baixa | core/src/formatos.rs (`cbor()`, braço `(6, Some(t))`) | Uma tag CBOR sem item no fim do arquivo é engolida sem erro | confirmado, não corrigido |

## Suspeitas (sem prova, sem correção)

| severidade | arquivo:linha | cenário | status |
|---|---|---|---|
| média | app/src-tauri/src/lib.rs:2876-2897, 3293 | ⌘Q durante "Salvar como" deixa um `.jasao-salvar-*` para trás | suspeita |
| baixa | app/src-tauri/src/lib.rs:3274 | `std::env::args()` entra em pânico com argv que não é UTF-8 | suspeita |
| baixa | core/src/entrada.rs:146 | Se `try_clone` falha, o erro sobe, mas o temporário só some no `limpar_orfaos` | suspeita |
| baixa | core/src/linhas.rs:96 | `linha_do_byte` devolve None para os bytes 0-2 do BOM (parece inalcançável) | suspeita |
| baixa | core/src/compactado.rs | A chave de cache (caminho + tamanho + mtime) pode ficar velha se o arquivo for reescrito no mesmo segundo com o mesmo tamanho | suspeita |
| baixa | core/src/csv.rs:136, 123 | U+200B antes de `=` fura a proteção contra fórmula; `-0` vira `0` | suspeita |
| baixa | core/src/regua.rs:109-115 | A cor da régua compara o texto cru, com escapes | suspeita |
| baixa | core/src/validar.rs:420, 233 | Texto da mensagem errado; nomes em `dependencies` | suspeita |
| baixa | core/src/busca.rs | Comportamento de `\z` | suspeita |

## Para outras frentes

| severidade | arquivo:linha | cenário | status |
|---|---|---|---|
| baixa | core/src/arvore.rs (`revelar`) | O(n²) em linha muito profunda | para frente perf |
| baixa | core/src/linhas.rs:363 | `ArquivoLinhas::linha_crua` só é usada em testes | para frente morto |

Conferidos e limpos: redirecionamento em `url.rs` (troca de origem tira cabeçalhos sensíveis, https→http recusado, POST 307/308 para outro host recusado), `memoria.rs`, `recentes.rs`, a borda de ulp e o rótulo com sinal em facetas/régua.
