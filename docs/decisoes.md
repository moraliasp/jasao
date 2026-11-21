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

## Formatos de caminho e linhas inválidas (Fase 1, 21/11/2026)

**Caminho (`nucleo::caminho`):** `Caminho(Vec<Segmento>)`, segmento é `Chave(String)` ou `Indice(usize)`, sem dependência nova. Quatro formatos, conferidos na fonte das RFCs:
- jq: `.a.b[0]`; chave que não é identificador ASCII, ou é palavra-chave do jq (`if`, `and`…), vai como `["…"]` no escape do JSON.
- JSONPath (RFC 9535): `.chave` quando cabe em `member-name-shorthand` (inclui não ASCII, como `$.ação`); senão `['…']` com o escape do Normalized Path (§2.7): só `'`, `\` e controles; `\b \f \n \r \t` e o resto `\u00xx` minúsculo.
- JSON Pointer (RFC 6901): `/a/b/0`, `~` → `~0`, `/` → `~1`; a raiz é `""`.
- JavaScript: `dado.a`, ponto só para identificador ASCII que não é palavra reservada (`class`, `default`… vão entre colchetes, embora o ES5 aceite o ponto); acento vai entre colchetes por não termos a tabela Unicode `ID_Start`.

Leitura de volta para jq e JSON Pointer, com `proptest` de ida e volta. O Pointer não distingue índice de chave numérica: na leitura, token no formato `array-index` (`0`, `12`, nunca `012`) vira `Indice`; `-` vira `Chave("-")`.

**Linhas inválidas (`nucleo::erros`):** validador próprio sobre bytes, sem montar o valor e sem recursão (pilha explícita), porque `main` não tinha validação por linha e um parser de terceiros não dá motivo legível em pt-BR nem a coluna do jeito que queremos. Gramática da RFC 8259; chave duplicada e `\ud800` solto passam (são sintaxe válida). Um motivo por linha, o primeiro: vírgula sobrando, falta vírgula, falta valor, falta `:`, chave sem aspas, aspas simples, valor inválido, número malformado, texto sem fechamento, escape inválido, controle em texto, conteúdo depois do valor, UTF-8 inválido, linha truncada. UTF-8 inválido ganha se vier antes (ou no mesmo byte) do primeiro erro de sintaxe. Linha só com espaço do JSON conta como em branco, não erro.

A varredura reaproveita o motor da busca: o laço paralelo de `Busca::executar` virou `busca::em_blocos` (genérico no resultado do bloco), usado pelos dois, com a mesma garantia de cancelamento. Teto `max_guardados` (padrão 10.000) corta o que é guardado em cada bloco e o que é entregue; as contagens (`linhas`, `invalidas`, `em_branco`) são sempre do arquivo inteiro. `deslocamento` é o byte do início da linha; `coluna`, o byte do erro dentro dela.

**CLI:** `jasao erros <arquivo> [--json] [--max N]`, linha e coluna a partir de 1, sai com 1 se houver linha inválida (erro de leitura também sai com 1, como nos outros comandos).

**Medido:** 105 MB gerado (699.775 linhas, 139 inválidas injetadas de 6 tipos, 89 em branco), Apple M5 Pro, 18 núcleos, cache quente: **6–7 ms** de varredura, 0,01 s o processo inteiro; mesmo resultado pelo `.gz`. Contagem igual à do `json.loads` do Python linha a linha.

## Parser da árvore: sonic-rs (Fase 1, 21/11/2026)

**Escolha:** `sonic-rs` (0.5) analisa o registro ao expandir e valida cada linha que aparece na janela.

**Medido** (`cargo bench -p nucleo --bench parsers`, Apple M5 Pro): validar e percorrer todos os nós de 20 mil linhas do gerador (chave e texto cru de cada valor, o que a árvore desenha):

| Parser | Tempo | Vazão | Fidelidade |
|---|---|---|---|
| `simd-json` 0.19 (fita, com cópia da linha) | 6,1 ms | 512 MiB/s | número vira `u64`/`i64`/`f64`: perde o texto; reescreve o buffer |
| `sonic-rs` 0.5 (iteradores preguiçosos) | 7,5 ms | 415 MiB/s | texto cru de cada valor, chave repetida e ordem, erro com byte |
| `serde_json` 1.0 + `RawValue` | 8,7 ms | 358 MiB/s | igual ao sonic-rs, com mais código (`Visitor` próprio) |

**Por quê:** os três ficam abaixo de 0,5 µs por registro típico, longe dos 16 ms de um quadro; o que decide é fidelidade. O `simd-json` é 20% mais rápido mas não guarda o texto do número (o número de 400 dígitos não sobrevive) e exige cópia mutável da linha. Entre os dois fiéis, o `sonic-rs` é 15% mais rápido e dá `to_object_iter`/`to_array_iter` prontos, com `as_raw_str()` apontando para os bytes do `mmap`, e `Error::offset()` para a posição do erro.

**Árvore** (`cargo bench -p nucleo --bench arvore`, JSONL de 64 MB): janela de 100 linhas recolhidas **57 µs** (valida e conta os campos de cada linha); janela de 100 linhas dentro de registros abertos **14 µs**; expandir e recolher um registro **~1 µs**.

**Limites:** `MAX_PROFUNDIDADE = 1.000` níveis (passou disso, a linha vira erro com posição e motivo, antes de qualquer recursão); prévia de `MAX_PREVIA = 200` bytes de texto cru. O erro é estruturado (`byte` na linha, base 0, e `motivo` em código) e a interface traduz o motivo; ver abaixo.

## Janela da árvore como JSON (Fase 1, 21/11/2026)

**Escolha:** o comando `linhas(inicio, quantidade)` devolve `Vec<Linha>` serializado (JSON), com teto de 1.000 linhas por pedido. O comando cru `janela` da Fase 0 saiu: a tela Valores não mostra texto cru.

**Por quê:** cada linha da árvore é estruturada (profundidade, chave, tipo, prévia de até 200 caracteres, caminho, migalhas) e a interface pede ~300 por vez; montar isso em bytes próprios não pagaria. Medido no JSONL de 1 GB do gerador (6.456.439 registros, Apple M5 Pro): abrir 143–161 ms; `janela` de 100 linhas 66 µs no núcleo; 41 saltos aleatórios de rolagem com a janela certa na tela em 10 ms de mediana e 16 ms no pior caso; expandir+recolher ~1 µs. O texto cru volta por `ipc::Response` quando houver a visão de texto sincronizado.

**Rolagem:** a altura do espaçador para em 8 milhões de px (navegadores cortam elementos perto de 33 milhões); acima disso a barra de rolagem é proporcional e 1 px de barra cobre várias linhas.

## Erro de registro, idioma e acessibilidade (Fase 1, 21/11/2026)

**Erro estruturado:** cada linha inválida traz `erro: { byte, motivo }`, com `motivo` em código `snake_case` (16 casos: `utf8`, `profundidade`, `fim_inesperado`, `literal`…), e a prévia passa a ser o texto cru da linha. A interface mostra `byte N: <motivo traduzido>` e o texto cru ao lado. O sonic-rs 0.5 não expõe o código do erro (`error_code` é `pub(crate)`), então o motivo sai do prefixo da mensagem (`do_sonic`, tabela `MAPA`); mensagem nova cai em `outro`, e o teste `motivos_do_sonic` acusa quando uma atualização do sonic-rs mudar o texto.

**Limite conhecido:** a validação (`from_str::<IgnoredAny>`) não confere os dígitos de `\uXXXX`: `"\uZZZZ"` passa como válido. Trocar por `sonic_rs::Value` se isso aparecer em arquivo real.

**Idioma:** pt-BR quando `navigator.language` começa com `pt`, inglês nos demais (`app/src/i18n.ts`). Sem seletor: o sistema decide.

**Acessibilidade:** os nós da árvore têm `aria-level`, `aria-posinset` e `aria-setsize` (o núcleo devolve `irmaos`, o número de filhos do pai).

**Caminho copiado:** relativo ao registro (`.a.b[0]`), por design: é o filtro certo para `jq '<caminho>' arquivo.jsonl`, que processa linha a linha.

**IPC:** o teste `comandos_pelo_ipc` (`app/src-tauri`) chama os comandos pelo runtime mock do Tauri (`tauri::test`), com o mesmo `Builder` do app; cobre serialização e erros sem abrir janela. A janela nativa não foi vista nesta fatia.

## Visão Texto e barra de modos (Fase 1, 21/11/2026)

**Escolha:** abas Árvore e Texto (⌘/Ctrl+1 e 2, ←/→ na barra) sobre o mesmo arquivo aberto. O Texto mostra uma linha do arquivo por linha da tela (20 px), virtualizado com a mesma rolagem da árvore (`app/src/rolagem.ts`).

**Bytes, não JSON:** `texto(inicio, quantidade)` devolve `tauri::ipc::Response` (ArrayBuffer). A 1ª linha do buffer traz o tamanho completo de cada linha; cada linha vem cortada em `MAX_COLUNAS = 4096` bytes sem partir caractere UTF-8, e a tela mostra "… +N bytes". Assim uma linha de 1 GB não vai inteira para a interface. Realce por regex no TypeScript, sobre o texto que já chegou.

**Sincronia:** cada trecho realçado leva o byte inicial; clique chama `revelar(registro, byte)`, que abre a árvore até o valor daquele byte (byte em chave, `:` ou `,` conta para o valor seguinte) e devolve `[linha visível, total]`. No outro sentido, a seleção da árvore realça no Texto a faixa de bytes do nó (`Linha.faixa`). ↑↓/PgUp/PgDn/Home/End andam por registro; Enter volta para a Árvore.

**Ver formatado:** `formatado(registro)` devolve o registro com um campo por linha, preservando os bytes crus de chaves e escalares (número grande, escapes); teto de 16 MB. Abre num `<dialog>`; linha inválida desabilita o botão.

**Corrigido no caminho:** um erro de IPC em `pedir` fazia o redesenho pedir de novo, em laço (vinha da árvore do PR 6); agora o erro vai para o status e para. Teclas rápidas no Texto descartam a resposta de tecla anterior (contador `selecao`).

**Fora desta fatia:** quebra de linha longa no Texto; JSON único em várias linhas aparece como está no arquivo, uma linha por vez (o índice é de linhas); seleção parcial dentro do formatado.

**Conferência:** pela interface no navegador interno contra uma ponte HTTP com os mesmos comandos do núcleo (JSONL fictício de 502 linhas, uma inválida e uma de 9 KB; JSON único fictício). A janela nativa não foi vista.
