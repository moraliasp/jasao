# Revisão de performance (25/11/2026)

Escopo: núcleo Rust (`core`) e CLI, nos caminhos que o app também usa. Só entrou mudança com
ganho medido de ≥ ~5% e sem mudança de comportamento. Bugs e código morto ficam para as outras
frentes de revisão; aqui só aparecem anotados.

## Como foi medido

- Máquina: macOS 27.2.1 · Apple M5 Pro · 18 núcleos · 48 GB · rustc 1.99.1, `--release`
  (perfil do repo: LTO, `codegen-units = 1`).
- Amostras de `scripts/medir.sh`: `target/medir/1024mb.jsonl` (1 GB, 6.456.439 registros) e
  `1024mb.formatado.json`.
- A máquina é compartilhada (runner de CI e outras sessões). A carga variou de 8 a 157 durante a
  medição. Por isso a comparação é **A/B intercalada**: binários de cada versão rodam em
  sequência, várias voltas, mediana de 5 rodadas por medida. Com carga alta, o número absoluto
  sobe. A comparação vale dentro da mesma volta.
- Perfil: `sample` do macOS num build com LTO, `strip=false` e `debug=line-tables-only`. Sem LTO
  o memchr não é inlinado e o perfil engana.
- Reproduzir: `scripts/medir.sh` (tabela de metas). Para o A/B, compile cada commit, copie o
  `target/release/jasao` e alterne os binários com `/usr/bin/time -l`.

## Linha de base (f5fa2e0, carga 7,9)

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

Perfil do `inferir` (amostras no topo da pilha): SipHash (`write` + `hash_one`) ≈ 3.100,
`mascara::Lexico::linha` 1.008, memcmp 849, `IndiceLinhas::construir` 832, malloc/free ≈ 960,
`memoria::reservar` 377.

## Achados

### 1. Contador do alocador global disputando uma linha de cache: aplicado

- `core/src/memoria.rs:27-53` (`contando`, `reservar`, `liberar`) e o `GlobalAlloc` logo abaixo.
- Hipótese: `Contador` fazia `fetch_add`/`fetch_sub` num único `AtomicUsize` em toda alocação, de
  toda thread, mesmo sem teto. Com 18 threads do rayon alocando chaves, a linha de cache fica
  pulando entre núcleos. O teto só é ligado (`limitar`) no processo filho do jq e no `consultar`
  da CLI.
- Mudança: sem teto, nada é contado (uma leitura de `TETO`, que ninguém escreve). Com teto, a
  conta é a mesma de antes. `liberar` satura em zero, porque o que foi alocado antes de `limitar`
  e liberado depois não foi somado. O teste `cli/tests/jq.rs` (`teto_de_memoria_*`) passa.
- `inferir`, 1 GB, mediana de 3, intercalado (carga 33–83):

  | Volta | base | com a mudança |
  |---|---|---|
  | 1 | 4,13 s | 1,89 s |
  | 2 | 3,97 s | 1,55 s |
  | 3 | 6,55 s | 3,23 s |

  Saída de `inferir --json` idêntica (só a linha de tempo difere).

### 2. SipHash nos mapas da inferência e da máscara: aplicado

- `core/src/estrutura.rs:16`: alias `HashMap` com `foldhash::fast::RandomState` para `No::filhos`,
  `Candidato::valores`, `Variante::chaves` e `Resumo::candidatos`. Também
  `core/src/mascara.rs:161`, o cache `Estado::chaves`.
- Hipótese: o hash é o maior item do perfil. As chaves são curtas e repetidas, e para isso o
  SipHash é caro. foldhash já estava no `Cargo.lock` (via hashbrown); só virou dependência direta.
  A semente é aleatória por processo, então a resistência a HashDoS (chaves vêm de arquivo não
  confiável) fica parecida. Não se usou Fx/hash fixo.
- A ordem de iteração já era aleatória com o std. A saída é ordenada depois, então nada muda.
- `inferir`, 1 GB, mediana de 3, intercalado contra o achado 1 sozinho:

  | Volta | só achado 1 | + foldhash |
  |---|---|---|
  | 1 | 1,89 s | 1,58 s |
  | 2 | 1,55 s | 1,33 s |
  | 3 | 3,23 s | 2,51 s |

### 3. `Box<[u8]>` por chave por registro em `Candidato::ver`: aplicado

- `core/src/estrutura.rs:486`: `v.chaves.entry(k.into())` alocava um `Box` para cada chave de cada
  registro, mesmo quando a chave já estava no mapa. Agora é `get_mut`, e o `Box` só nasce na 1ª
  vez que a chave aparece.
- `inferir`, 1 GB, mediana de 3, intercalado contra o achado 2 (carga 39–81, mesma ordem de
  grandeza do ruído, mas venceu nas 3 voltas): 2,88 → 2,20 · 2,38 → 2,28 · 3,42 → 2,36 s
  (junto com o foldhash na máscara). Saída de `inferir --json` com md5 idêntico.
- Teste de equivalência: a contagem é a mesma por construção (+1 ou insere 1). O teste
  `discriminador_no_gerador` (`estrutura.rs`) cobre a contagem: obrigatória = chave contada em
  todos os registros da variante, e a saída de 1 GB foi comparada
  byte a byte.

### 4. Resultado conjunto (base × achados 1+2+3): mediana de 5, duas voltas intercaladas

| Medida | base v1 | nova v1 | base v2 | nova v2 | Decisão |
|---|---|---|---|---|---|
| primeira tela | 0,46 s | 0,22 s | 0,25 s | 0,25 s | ruído; sem perda |
| primeira tela, formatado | 0,75 s | 0,66 s | 2,72 s | 0,93 s | ruído; pico 397 MB igual |
| índice de linhas | 0,23 s | 0,22 s | 0,51 s | 0,34 s | sem mudança esperada |
| **`inferir`** | **4,68 s** | **1,32 s** | **7,78 s** | **3,89 s** | **−50 a −72%** |
| busca literal | 0,55 s | 0,55 s | 0,91 s | 1,19 s | ruído (carga 85→130) |
| busca regex | 0,50 s | 1,05 s | 0,92 s | 1,91 s | ruído: rodadas de 0,46 a 2,16 s |
| **exportar + validar** | **8,77 s** | **5,18 s** | **11,32 s** | **7,11 s** | **−37 a −41%** |
| pico alocado `inferir` | 62 MB | 61 MB | 62 MB | 61 MB | igual |

Carga na volta 1: 56–82. Na volta 2: 82–157. A busca não toca nos mapas mudados, e a máscara só
mudou no cache de chaves. A diferença na regex acompanha a subida da carga dentro da volta 2: a
volta 1 tinha rodadas da nova em 0,46 s, igual à base. `inferir` voltou a bater a meta de < 5 s
mesmo com carga 125.

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
