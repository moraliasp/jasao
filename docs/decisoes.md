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
