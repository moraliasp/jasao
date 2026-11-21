# Decisões

## Janelas de linhas entre Rust e interface (Fase 0, 21/11/2026)

**Escolha:** o comando `janela(inicio, quantidade)` devolve `tauri::ipc::Response` com as linhas cruas separadas por `\n`; o JS recebe `ArrayBuffer` e decodifica com `TextDecoder`.

**Por quê:** a documentação atual do Tauri 2 (v2.tauri.app, "Calling Rust", tauri 2.12.7 no docs.rs) diz que retorno `Serialize` vira JSON e pesa em dado grande, e indica `ipc::Response` para devolver bytes sem serializar. Uma janela é um pedido e uma resposta, então não precisa de `Channel`. `Channel<InvokeResponseBody>` com `InvokeResponseBody::Raw` fica para fluxo contínuo (busca, progresso de varredura). `Vec<u8>` e `Channel<Vec<u8>>` vão como array JSON de números: não usar. Linha não contém `\n`, então o separador é inequívoco; UTF-8 inválido chega cru e vira U+FFFD só na tela.

**Limites:** `MAX_JANELA = 10.000` linhas por pedido. `abrir` é `async` para indexar fora da thread principal.

## Índice de linhas: medido × meta

Meta do plano: índice completo de 1 GB em < 2 s (≈ 0,5 GB/s). Máquina: Apple M5 Pro, 18 núcleos.

| Medida | Resultado |
|---|---|
| `cargo bench -p nucleo --bench indice` (256 MB, em memória) | **11,4 GiB/s** (21,9 ms) |
| idem com `NUCLEO_BENCH_MB=1024` | **11,7 GiB/s** (85 ms por GB) |
| `cli linhas` em 1 GB gerado (mmap, cache do sistema quente) | 154 ms |

Uma thread com `memchr` passa a meta com folga de ~20×, por isso o `rayon` previsto no plano ficou de fora até um benchmark pedir. Leitura a frio do disco não foi medida.

## Nome: Jasão (21/11/2026)

Decisão do Eduardo: **Jasão** substitui o provisório "Dr. J". "Jasão" é o nome de exibição (título da janela, README, `productName`); `jasao`, sem acento, é o identificador (binário da CLI e bundle `br.com.moralia.jasao`). O app não usa `mainBinaryName = "jasao"`: o `tauri build` grava o binário renomeado em `target/release/jasao`, o mesmo caminho do binário da CLI, e um sobrescreve o outro. Se o binário do app dentro do pacote precisar ser ASCII, dar outro nome (ex.: `jasao-app`).

Mora em `app/src-tauri/tauri.conf.json` (`productName`, `identifier`) e em `cli/Cargo.toml` (`[[bin]] name`). Os crates continuam neutros (`nucleo`, `cli`, `app`), como pede o brief da Fase 0. O pacote do `core/` se chama `nucleo` porque `core` colide com o crate embutido do Rust. Pasta do repositório (`drj`) e branch ficam para depois.

## Entrada compactada: gzip e zstd (Fase 1, 21/11/2026)

**Escolha:** formato pelos bytes mágicos (`1f 8b` gzip, `28 b5 2f fd` zstd), nunca pela extensão. O compactado é descompactado em fluxo para um arquivo de cache e o cache é aberto pelo mesmo `mmap` + índice do cru. `ArquivoLinhas::abrir` aceita os três casos; `abrir_com(caminho, &Opcoes)` dá progresso (bytes lidos do compactado), cancelamento (`AtomicBool`) e limites. gzip por `flate2::MultiGzDecoder` (backend padrão, `miniz_oxide`), zstd pelo crate `zstd` (libzstd); ambos leem vários membros/quadros concatenados.

**Cache:** `~/Library/Caches/jasao/descompactados` no macOS (`$XDG_CACHE_HOME`/`~/.cache` no Linux, `%LOCALAPPDATA%` no Windows). Nome = FNV-1a de caminho canônico + tamanho + data de modificação do original; mudou qualquer um, é outra entrada. Escrita em `.parcial` e `rename` atômico: o que tem nome final está inteiro. Erro, cancelamento ou limite apagam o parcial; parcial com mais de um dia (processo que morreu) sai na limpeza. Teto padrão de 20 GiB; antes de descompactar, sai o usado há mais tempo até caber a estimativa; o arquivo da vez nunca sai, mesmo maior que o teto.

**Limites:** saída máxima 10 GiB (teto do plano); razão saída/compactado até 1000× acima de 16 MiB livres. Estourou: `ErrorKind::FileTooLarge` com tamanhos e a saída ("descompacte fora do app"). Espaço livre conferido antes contra a estimativa (ISIZE do gzip ou tamanho do 1º quadro zstd, nunca menos que o compactado); disco cheio no meio vira `StorageFull` com mensagem. Truncado/corrompido: `InvalidData`, sem abrir o pedaço que saiu.

**Vazão medida** (`cargo bench -p nucleo --bench compactado`, 256 MB de JSONL gerado, em memória, saída descartada; Apple M5 Pro):

| Formato | Compactado | Saída |
|---|---|---|
| gzip nível 6 | ~41 MB | **866 MiB/s** (296 ms) |
| zstd nível 3 | ~44 MB | **2,1 GiB/s** (118 ms) |

Pela `cli` com disco (200 MB, cache do sistema quente): gzip 327 ms, zstd 255 ms até o índice pronto; reaproveitando o cache, 27 ms. 1 GB em gzip leva ~1,2 s só de descompactação: dentro dos 2 s do índice, por isso o backend `zlib-rs` do `flate2` ficou de fora até um benchmark pedir.

## Busca no arquivo inteiro (Fase 1, 21/11/2026)

**Como:** `nucleo::busca` varre os bytes crus em blocos de ~256 KiB que terminam em `\n`, um por trabalhador (`std::thread::scope`, sem `rayon`). Cada bloco devolve seus achados e quantas quebras tem; a thread do chamador remonta a ordem e entrega `Lote { achados, varridos, total }` por callback, já com o número de linha absoluto. Literal sensível à caixa usa `memchr::memmem`; ignorar caixa e regex usam `regex::bytes` (Unicode, `multi_line` e `crlf`). Cancelamento por `AtomicBool`. Um trabalhador não adianta mais de 64 blocos além do último entregue, o que limita a memória dos achados à espera.

**Semântica:** busca por linha, nas regras do índice (BOM fora, `\r` de CRLF fora). Achado nunca cruza `\n`: se a regex casar através da quebra, a busca é refeita só dentro da linha. Achado vazio é ignorado. Padrão vazio e literal com `\n` são recusados com mensagem. Limite conhecido: no modo CRLF da `regex`, `$` também casa antes de um `\r` solto, que para o índice é dado.

**Compactado:** a busca recebe os bytes de `ArquivoLinhas::bytes()`, que para `.gz`/`.zst` já é o cache descompactado; `jasao buscar` abre pela mesma `abrir` dos outros comandos, então busca em compactado sem código a mais.

**Cancelamento:** o sinal é conferido antes de entregar cada bloco, não a cada mensagem do canal: uma mensagem pode liberar vários blocos que esperavam o anterior, e conferir só na chegada deixava sair um lote depois de cancelar (falha intermitente no Linux do CI, reproduzida no Mac com `taskpolicy -c background` em 13 de 300 rodadas; 0 de 300 depois da correção). Garantia: visto o sinal, nenhum lote sai e `Resumo::varridos` para no último bloco aceito.

**API para a interface:** o comando Tauri roda `executar` numa thread, repassa cada `Lote` por `Channel` e guarda o `AtomicBool` para o botão de cancelar. A contagem total vem no `Resumo`; `--max` da CLI só limita o que imprime, a varredura segue até o fim.

**Medido × meta** (`cargo bench -p nucleo --bench busca`, 1 GB gerado em memória, Apple M5 Pro, 18 núcleos):

| Caso | Até o 1º achado | Até o fim |
|---|---|---|
| literal `tempo esgotado` (430 mil achados) | 0,54 ms | 15,4 ms |
| literal ausente | – | 15,8 ms |
| ignorar caixa `TEMPO ESGOTADO` | 0,29 ms | 54,7 ms |
| regex `"origem":"192\.0\.2\.25\d"` (43 mil achados) | 0,23 ms | 19,0 ms |
| regex ausente `"qtd":\d{3}` | – | 17,6 ms |
| **meta do plano** | **< 200 ms** | **< 3 s** |

`jasao buscar` no mesmo 1 GB em arquivo (mmap, cache quente): 15 ms literal, 17 ms regex, 39 ms ignorando caixa, contagens iguais às do `grep -o`. A folga é de duas ordens de grandeza com o arquivo na memória; leitura a frio do disco não foi medida e deve dominar.

## Consulta jq (Fase 1, 21/11/2026)

**Escolha:** `jaq` (jaq-core 3.1.2, jaq-std 3.0.4, jaq-json 2.0.3), em Rust, sem processo externo nem `libjq`. `nucleo::consulta`: `Consulta::nova(filtro)` compila uma vez; erro de compilação vem como `ErroFiltro { falhas: [Falha { faixa, linha, coluna, mensagem }] }` (linha e coluna a partir de 1, coluna em caracteres), nunca pânico. `executar_jsonl` roda o filtro em cada linha, no mesmo molde da busca (blocos de ~256 KiB que terminam em `\n`, `thread::scope`, no máximo 64 blocos adiante, reordenação na thread do chamador); `executar_documento` trata o arquivo inteiro como um valor JSON. Resultados chegam por callback em `LoteConsulta { resultados, linhas, total_linhas }`, em ordem de arquivo, cada um com a linha de origem (a partir de 0) e o JSON compacto. CLI: `jasao consultar <filtro> <arquivo> [--documento] [--tempo-max SEG]`, JSONL no stdout, erros por linha e resumo no stderr; filtro que não compila sai com código 1. `.gz`/`.zst` passam pela mesma `abrir`.

**Erros por linha:** linha só de espaço é pulada. Linha que não é JSON, com mais de 10 000 níveis de aninhamento (o mesmo teto do `jq`; sem ele, o parser recursivo do `jaq` pode estourar a pilha) ou em que o filtro falha (`"x" + 1`) conta em `ResumoConsulta::erros`; os 100 primeiros ficam com linha e motivo, e a varredura segue. Como no `jq`, os resultados que a linha produziu antes do erro ficam; os seguintes, não.

**Limites explícitos:** um trabalhador junta no máximo `max_lote` resultados (padrão 10 000) antes de entregar, e um lote nunca passa disso, mesmo numa linha que explode (`range(1e12)`). Cancelamento (`AtomicBool`) e teto de tempo opcional (`com_tempo_max`) são conferidos entre linhas e entre resultados; visto o sinal, nenhum lote sai. Filtro limitado a 16 KiB e compilado e avaliado em threads de 256 MiB de pilha (reserva virtual): o parser desce no filtro e um filtro de 16 KiB todo de `-` ou `(` compila e roda até em debug; com 128 MiB em debug, ou 64 MiB em release a partir de ~60 mil níveis, abortava.

**Limite conhecido (cancelar):** o `jaq` não tem ponto de interrupção. Filtro que gira sem entregar resultado (`last(range(1e12))`, `[range(1e12)]`, `def f: f; f`) só é visto parar quando termina, e recursão sem fundo (`def f: 1 + f; f`) estoura a pilha e aborta o processo inteiro. Por isso a CLI, com `--tempo-max`, tem um vigia que encerra o processo (código 3) dois segundos depois do teto. Recomendação para a interface: rodar a consulta num processo filho (`jasao consultar`) e matar o processo no cancelar, em vez de rodar o `jaq` dentro do processo do app.

**Fidelidade de número** (testado em `consulta::testes::fidelidade_de_numero`):

| Entrada | Filtro | Saída | Nota |
|---|---|---|---|
| `12345678901234567890123` | `.` | `12345678901234567890123` | inteiro grande é exato (BigInt) |
| `9007199254740993` (2^53 + 1) | `. + 1` | `9007199254740994` | soma, subtração e produto de inteiros são exatos |
| `9007199254740993` | `. / 1` | `9007199254740992.0` | divisão vai para `f64` e perde |
| `0.1000000000000000000000001` | `.` | igual à entrada | decimal sai como veio enquanto não entra em conta |
| `0.1000000000000000000000001` | `. * 1` | `0.1` | em conta, decimal vira `f64` |
| `1e400` | `.` | `1e400` | fora do `f64`, preservado como texto |
| — | `[nan, infinite]` | `[null,1.7976931348623157e308]` | como o `jq`: NaN vira `null`, infinito o maior `f64` |

Outros comportamentos do `jaq` que o usuário pode estranhar: chave duplicada (`{"a":1,"b":2,"a":3}`) fica com o último valor na posição da primeira (`{"a":3,"b":2}`); a ordem das chaves do arquivo é mantida (`keys` ordena, `keys_unsorted` não); o leitor aceita além do JSON estrito `NaN`, `Infinity` e comentários `#`. `input`, `inputs` e `$ENV` não existem (erro de compilação); `env` existe.

**Pendência:** `--slurp` (todas as linhas num array) ficou de fora desta fatia.

**Medido** (`jasao consultar`, 157 MB de JSONL gerado, 951 144 linhas, cache quente, saída descartada, Apple M5 Pro, 18 núcleos; mediana de 3):

| Filtro | Resultados | Tempo |
|---|---|---|
| `.` | 951 144 | 473 ms |
| `keys \| length` | 951 144 | 473 ms |
| `select(.tipo == "pedido") \| .id` | 317 330 | 667 ms |
| `.itens[]?.preco \| tonumber` | 792 828 | 812 ms |
| `[.. \| numbers] \| add` | 951 144 | 4,08 s |

O mesmo `select` no `jq` 1.7 (uma thread) levou 1,67 s. Em `.gz` (31 MB), descompactação + consulta: 552 ms. Pico de memória com `.`: ~270 MB. `..` é o caso lento do `jaq` (~10× o `.`); não investigado.

**Licenças** (registro, sem decisão): `jaq-core`, `jaq-std` e `jaq-json` são MIT. Dependências que vêm com eles: MIT (`hifijson`, `libm`, `typed-arena`, `urlencoding`, `bytes`); MIT OR Apache-2.0 (`base64`, `bstr`, `dyn-clone`, `hashbrown`, `indexmap`, `equivalent`, `log`, `num-bigint`, `num-integer`, `num-traits`, `once_cell`, `regex-automata`, `regex-bites`, `regex-syntax`); Unlicense OR MIT (`aho-corasick`, `jiff`, `jiff-core`, `memchr`); Zlib (`foldhash`); Apache-2.0 OR BSL-1.0 (`ryu`); **Apache-2.0 OR GPL-2.0-only** (`self_cell`, usado pelo `jaq-json`; a opção Apache-2.0 evita a GPL, mas fica o registro para quem decidir a licença do Jasão).
