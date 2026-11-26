# Revisão de performance (25/11/2026)

Escopo: núcleo Rust (`core`) e CLI, nos caminhos que o app também usa. Só entrou mudança com
ganho medido de ≥ ~5% e sem mudança de comportamento. Bugs e código morto ficam para as outras
frentes de revisão; aqui só aparecem anotados.

## Como foi medido

- Máquina: macOS 27.2.1 · Apple M5 Pro · 18 núcleos · 48 GB · rustc 1.99.1, `--release`
  (perfil do repo: LTO, `codegen-units = 1`).
- Amostras de `scripts/medir.sh`: `target/medir/1024mb.jsonl` (1 GB, 6.456.439 registros) e
  `1024mb.formatado.json`.
- A máquina é compartilhada (runner de CI e outras sessões compilando). A carga variou de 8 a 157
  durante a revisão. Por isso a comparação é **A/B intercalada, trocando o binário a cada
  rodada**: `base` → `A` → `B` → `C` → `base` … Assim a carga de um momento cai sobre todas as
  versões, e a mediana é por versão. As tabelas dos achados usam essa medição.
  Binários:
  - `base` = e8e44dc;
  - `A` = só o achado 1;
  - `B` = A + foldhash na estrutura;
  - `C` = head do PR (A + achados 2 e 3 completos).
- **Pico alocado** = `peak memory footprint` do `/usr/bin/time -l` (o *phys_footprint* do macOS,
  a mesma coluna "Pico alocado" do `scripts/medir.sh`). Não é RSS: o arquivo mapeado (mmap)
  não entra.
- Perfil: `sample` do macOS num build com LTO, `strip=false` e `debug=line-tables-only`. Sem LTO
  o memchr não é inlinado e o perfil engana.
- Reproduzir:
  1. compile cada commit e copie o `target/release/jasao` como `jasao-<versão>`;
  2. alterne os binários num laço, com `/usr/bin/time -l` (ou `-p`) em cada chamada.

  `scripts/medir.sh` dá a tabela de metas de uma versão só.

## Linha de base (e8e44dc, `scripts/medir.sh`, carga 7,9)

| Medida | Mediana | Meta |
|---|---|---|
| primeira tela (abrir + 50 linhas) | 0,23 s | < 300 ms ok |
| primeira tela, JSON formatado | 0,86 s (pico 397 MB) | sem meta |
| índice de linhas | 0,22 s | < 2 s ok |
| estrutura completa (`inferir`) | **6,23 s** | **< 5 s NÃO** |
| busca literal ausente | 0,67 s | < 3 s ok |
| busca regex ausente | 0,73 s | < 3 s ok |
| exportar Schema + validar | 12,05 s | sem meta |

Em 24/11 (`docs/decisoes.md`), `inferir` media 2,1 s e exportar+validar 3,3–3,5 s. Havia
regressão.

Perfil do `inferir` (amostras no topo da pilha):

| Símbolo | Amostras |
|---|---|
| SipHash (`write` + `hash_one`) | ≈ 3.100 |
| `mascara::Lexico::linha` | 1.008 |
| malloc/free | ≈ 960 |
| memcmp | 849 |
| `IndiceLinhas::construir` | 832 |
| `memoria::reservar` | 377 |

## `inferir`, 1 GB: os três achados aplicados

Mediana de 7 rodadas, binário trocado a cada rodada, carga 26–46.

| Versão | Rodadas (s) | Mediana | vs. anterior | Pico alocado |
|---|---|---|---|---|
| base | 3,66 4,34 4,38 4,49 4,66 4,78 5,70 | 4,49 s | — | 60–65 MB |
| A (achado 1) | 1,22 1,29 1,68 1,71 1,81 1,82 1,88 | 1,71 s | −62% | 61–64 MB |
| B (+ foldhash na estrutura) | 1,04 1,26 1,38 1,46 1,50 1,80 1,91 | 1,46 s | −15% | 60–63 MB |
| C (head: + foldhash na máscara, achado 3) | 0,99 1,19 1,27 1,27 1,33 1,35 1,42 | 1,27 s | −13% | 59–61 MB |

`inferir --json` dá a mesma saída nas quatro versões; só a linha de tempo difere. Com a linha de
tempo removida, o md5 de B e C é idêntico.

### 1. Contador do alocador global disputando uma linha de cache: aplicado

- `core/src/memoria.rs:27-53` (`contando`, `liberar`, `reservar`) e o `GlobalAlloc` logo abaixo.
- Hipótese: `Contador` fazia `fetch_add`/`fetch_sub` num único `AtomicUsize` em toda alocação, de
  toda thread, mesmo sem teto. Com 18 threads do rayon alocando chaves, a linha de cache fica
  pulando entre núcleos. O teto só é ligado (`limitar`) no processo filho do jq e no `consultar`
  da CLI.
- Mudança: sem teto, nada é contado (uma leitura de `TETO`, que ninguém escreve). Com teto, cada
  alocação e liberação é contada como antes.
- **O teto ficou um pouco mais frouxo.** O heap que o processo alocou antes de `limitar` e que
  continua vivo não entra mais na conta. Antes, ele entrava. Hoje `limitar` é chamado logo no
  começo, depois de ler args, variáveis de ambiente e opções. Na CLI, isso inclui o que a
  abertura de stdin/URL deixou vivo no heap. A folga é desse tamanho; não foi medida.
- Por isso `liberar` satura em zero: liberar algo de antes de `limitar` não pode fazer o contador
  dar a volta. Teste: `memoria::testes::alocado_antes_de_limitar_e_liberado_depois_nao_passa_de_zero`.
  Ele falha se `liberar` voltar a ser `fetch_sub`. O teste `cli/tests/jq.rs`
  (`teto_de_memoria_*`) passa.

### 2. SipHash nos mapas da inferência e da máscara: aplicado

- Onde mudou:
  - `core/src/estrutura.rs:16`: alias `HashMap` com `foldhash::fast::RandomState` para
    `No::filhos`, `Candidato::valores`, `Variante::chaves` e `Resumo::candidatos`;
  - `core/src/mascara.rs:161`: o cache `Estado::chaves`.
- Hipótese: o hash é o maior item do perfil. As chaves são curtas e repetidas, e para isso o
  SipHash é caro. foldhash já estava no `Cargo.lock` (via hashbrown); só virou dependência direta.
- A semente é aleatória por processo, então a resistência a HashDoS (chaves vêm de arquivo não
  confiável) fica parecida. Não se usou Fx nem hash fixo.
- A ordem de iteração já era aleatória com o std. A saída é ordenada depois, então nada muda.

### 3. `Box<[u8]>` por chave por registro em `Candidato::ver`: aplicado

- `core/src/estrutura.rs:486`: `v.chaves.entry(k.into())` alocava um `Box` para cada chave de cada
  registro, mesmo quando a chave já estava no mapa. Agora é `get_mut`, e o `Box` só nasce na 1ª
  vez que a chave aparece.
- Equivalência: a contagem é a mesma por construção (+1, ou insere 1). Duas provas:
  - o teste `discriminador_no_gerador` (`estrutura.rs`) cobre a contagem: obrigatória = chave
    contada em todos os registros da variante;
  - a saída de 1 GB é idêntica.
- O ganho de B para C (−13%) é a soma do achado 3 com o foldhash na máscara. Os dois não foram
  separados.

## Busca: sem regressão

A 1ª medição, em blocos de 5 rodadas por versão, mostrou a regex 2× mais lenta no head:
0,50 → 1,05 s e 0,92 → 1,91 s. A carga subiu de 56 para 157 durante aqueles blocos. Como a
máscara (`linhas.rs::bytes()`) passa pelo `Estado::chaves`, que mudou de hasher, refiz a medição
trocando o binário a cada rodada. 9 rodadas, carga 32–42, CI na fila e nenhum job rodando:

| Medida | Versão | Real (mediana) | CPU de usuário (mediana) |
|---|---|---|---|
| busca literal ausente | base | 0,43 s | 2,76 s |
| | A | 0,43 s | 2,78 s |
| | B | 0,48 s | 3,18 s |
| | C (head) | **0,42 s** | **2,70 s** |
| busca regex `"qtd":\d{3}` | base | 0,47 s | 3,23 s |
| | A | 0,44 s | 2,80 s |
| | B | 0,46 s | 3,14 s |
| | C (head) | **0,40 s** | **2,44 s** |

- O head não é mais lento que a base em nenhuma das duas buscas.
- Na regex, o head é 15% mais rápido em tempo real e 24% em CPU de usuário. O foldhash no cache de
  chaves da máscara explica isso.
- A "regressão" da 1ª medição era carga.

## Medição conjunta em blocos (1ª rodada, para registro)

Mediana de 5 rodadas por versão, uma versão por bloco, duas voltas (carga 56–82 na 1ª e 82–157
na 2ª).

| Medida | base v1 | head v1 | base v2 | head v2 |
|---|---|---|---|---|
| primeira tela | 0,46 s | 0,22 s | 0,25 s | 0,25 s |
| primeira tela, formatado | 0,75 s | 0,66 s | 2,72 s | 0,93 s |
| índice de linhas | 0,23 s | 0,22 s | 0,51 s | 0,34 s |
| `inferir` | 4,68 s | 1,32 s | 7,78 s | 3,89 s |
| busca literal | 0,55 s | 0,55 s | 0,91 s | 1,19 s |
| busca regex | 0,50 s | 1,05 s | 0,92 s | 1,91 s |
| exportar + validar | 8,77 s | 5,18 s | 11,32 s | 7,11 s |

- **Busca:** os números são carga. A seção anterior mostra isso com o binário trocado a cada
  rodada.
- **exportar + validar:** caiu 37 a 41% nas duas voltas, mas não foi refeito trocando o binário a
  cada rodada.

## Descartados ou propostos

| # | Onde | Hipótese | Decisão |
|---|---|---|---|
| 5 | `core/src/linhas.rs` `IndiceLinhas::construir` | memchr em uma thread; paralelizar por blocos | **descartado**: 0,22 s em 1 GB, 10× abaixo da meta de 2 s; ganho não aparece na primeira tela |
| 6 | índice de linhas do JSON formatado (8 B/linha) | pico de 397 MB na primeira tela do formatado | **proposto, grande demais**: exige índice compacto (deltas/amostragem) e mexe no contrato de `IndiceLinhas` |
| 7 | `core/src/linhas.rs` `bytes()` mascara o arquivo inteiro antes da busca | busca com máscara ~0,5 s × 0,28 s sem | **proposto**: mascarar só os trechos que casam muda o desenho da busca; sem meta estourada hoje |
| 8 | `core/src/validar.rs` `serde_json::Value` por linha | exportar+validar ainda 5 s × 3,3 s de 24/11 | **não medido a fundo**: o achado 1 explica parte. O resto pode ser carga da máquina; vale re-medir com a máquina livre antes de mexer |
| 9 | `mach_absolute_time` no perfil (445 amostras) | chamada de tempo no caminho quente | **não investigado**: chamador não localizado no perfil; fica para re-perfilar depois dos achados 1–3 |
| 10 | front-end (`app/src/grade.ts`) | — | **não medido**: já usa rAF e listeners delegados; sem harness de medição no app |

## Anotado para as outras frentes (não mexido)

- Nenhum bug ou código morto relevante visto nos caminhos medidos.
