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

**Cache:** `~/Library/Caches/jasao/descompactados` no macOS (`$XDG_CACHE_HOME`/`~/.cache` no Linux, `%LOCALAPPDATA%` no Windows). Nome = FNV-1a de caminho canônico + tamanho + data de modificação do original; mudou qualquer um, é outra entrada. Escrita em `.parcial` e `rename` atômico: o que tem nome final está inteiro. Erro, cancelamento ou limite apagam o parcial; parcial com mais de um dia (processo que morreu) sai na limpeza. Teto padrão de 10 GiB; antes de descompactar, sai o usado há mais tempo até caber a estimativa; o arquivo da vez nunca sai, mesmo maior que o teto.

**Limites:** saída máxima 10 GiB (teto do plano); razão saída/compactado até 100× acima de 16 MiB livres (JSONL de log comprime 10–30×; a auditoria de segurança achou 1000× folgado demais: 10 MB viravam 10 GiB). Estourou: `ErrorKind::FileTooLarge`; a razão vem como `RazaoEstourada`, e o app pergunta se abre assim mesmo (sem teto de razão, com progresso e cancelamento); o teto absoluto não se contorna ("descompacte fora do app"). Cache: até 10 GiB, pasta 0700 e arquivos 0600 em Unix (o cache antigo é corrigido ao abrir). Espaço: o tamanho declarado (ISIZE do gzip ou 1º quadro zstd) é só dica, limitada ao que o teto deixaria sair; a reserva cresce com o que de fato sai (dobra ao passar), despejando o cache e conferindo o espaço livre a cada passo; disco cheio no meio vira `StorageFull` com mensagem. Truncado/corrompido: `InvalidData`, sem abrir o pedaço que saiu.

**`forcar` sem token:** o `abrir` aceita `forcar` direto da interface, sem token da confirmação; explorá-lo exige JS rodando na webview (XSS), que já seria falha maior. O teto de 10 GiB, o progresso e o cancelamento continuam valendo. Cancelar é por id de abertura que a interface gera, não por caminho (o mesmo arquivo pode abrir em duas abas).

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

## Inferência de estrutura (Fase 2, 21/11/2026)

**Como:** `nucleo::estrutura` lê cada linha com um tokenizador próprio que valida a linha inteira antes de mexer no resumo; linha inválida entra em `invalidas` (com a primeira) e a varredura segue. Os blocos são os mesmos de `busca::fatiar`, resumidos com `rayon` (`map`/`reduce`). `Resumo::combinar` é associativo e comutativo: contagens somam, evidência é a menor linha, ordem dos campos é (linha, posição da chave) da primeira aparição. Por isso o resultado não depende do tamanho do bloco nem da ordem da combinação (proptest de partição + teste com blocos de 1, 100 e 4096 linhas).

**Por caminho:** tipos com contagem e primeira linha (nulo, booleano, inteiro, decimal, texto, objeto, lista) e presença. Presença é relativa ao objeto pai: `.a.b` presente em 3 de 5 objetos `.a`. Em `[]` e `.*` conta valores. Chave repetida no mesmo objeto conta o valor, mas a presença só uma vez. Caminhos: `.a.b`, `.["chave estranha"]`, `.itens[]`, `.m.*`.

**Números:** classificados pelo texto, nunca via `f64`: gramática JSON `-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?`; com fração ou expoente é decimal, senão inteiro (de qualquer tamanho).

**Mapa ("mapa de X"):** dois critérios.
- Limite duro: um objeto com mais de `LIMITE_CHAVES` = 1000 chaves distintas vira `.*` já na varredura (e no `combinar`), o que limita a memória.
- Forma, decidida em `finalizar` depois da redução: ao menos `MAPA_MIN_CHAVES` = 20 chaves distintas, distintas ≥ 2× o maior número de chaves de um único objeto (as chaves variam entre registros) e todos os filhos com o mesmo conjunto de tipos.

**União discriminada:** candidatos são campos de texto no topo do registro, com até `DISCRIMINADOR_MAX_VALORES` = 32 valores distintos (acima disso o campo é descartado). É discriminador se estiver em todo registro objeto, sempre como texto, tiver de 2 a 32 valores e alguma chave aparecer em todos os registros de uma variante e em nenhum de outra. Cada variante lista chaves obrigatórias e opcionais com contagem.

**Entrada:** é "JSON único" se o primeiro byte útil for `[` ou `{` e o arquivo tiver uma linha só ou a primeira linha não for JSON válido sozinha. Lista no topo: cada elemento é um registro, lido em sequência. Objeto no topo: um registro. Se a leitura como JSON único falhar, também se lê como JSONL e fica a que rendeu mais registros (empate: JSONL).

**Cancelamento e progresso:** `AtomicBool` conferido por bloco; cancelado devolve o resumo parcial com `cancelado = true` e `varridas < total`. Progresso em linhas varridas de total, por callback na thread do chamador, a cada ~1%.

**Limites conhecidos:** profundidade máxima 128 (além disso a linha é inválida); a lista externa do JSON único não conta como nível. UTF-8 não é validado (o texto é tratado como bytes). JSON único é lido sem paralelismo.

**Medido:** `jasao inferir` em JSONL gerado de 157 MB (951 mil linhas, cache quente, Apple M5 Pro): 128–168 ms; `--json` 140 ms; o mesmo arquivo em `.zst`, 128 ms depois de descompactar.

## Painel de entrada (Fase 1, 21/11/2026)

**O que é:** ao abrir um arquivo aparece um painel não modal no lugar do conteúdo (o arquivo já está aberto): nome, tipo detectado, tempo de abertura, Tamanho, Registros, Linhas inválidas (só se houver) e cartões dos modos com tela. "Início" no cabeçalho e ⌘0 voltam a ele; as abas continuam e ficam sem seleção enquanto ele aparece.

**Tipo detectado:** `nucleo::resumo::conteudo` olha só as 1.000 primeiras linhas: vazio, JSON único, JSON em várias linhas (1ª linha inválida e o arquivo inteiro válido), JSON Lines de objetos, de valores ou misto.

**Varredura de inválidas:** `nucleo::resumo::varrer` valida todas as linhas em paralelo (`thread::scope`, blocos de 4.096 linhas por `AtomicUsize`, sem rayon) com a mesma regra da árvore (`arvore::validar`), então o número bate com o que a árvore mostra, linha vazia inclusive. O PR 7 (`erros::varrer`) trata linha vazia como não erro; quando ele entrar, alinhar as duas regras. `abrir` dispara a varredura numa thread e devolve na hora; o progresso vai pelo evento `varredura` a cada ~50 ms (`{geracao, varridas, total, invalidas, fim}`) e o comando `resumo` devolve o estado atual. Abrir outro arquivo cancela a varredura anterior (`AtomicBool`) e sobe a `geracao`, que descarta avisos atrasados. JSON em várias linhas pula a varredura: cada linha dele é inválida sozinha. No JSONL fictício de 400 MB (2,4 milhões de linhas), 2,18 milhões já estavam varridas 50 ms depois de abrir.

**Cartões declarativos:** `MODOS` em `main.ts`, um registro por modo com `disponivel` e `sempre` (entra no seletor "Abrir sempre em…"). Hoje Árvore e Texto; Tabela está na lista com `disponivel: false`, e o sugerido é o primeiro disponível (Árvore até a Tabela existir). Atalhos 1, 2… por posição; Enter abre o sugerido.

**Preferência:** "Ao abrir um arquivo" (mostrar o painel, ir ao sugerido, sempre em Árvore) fica no `localStorage` (`jasao.aoAbrir`); sem valor mostra o painel, valor desconhecido cai no sugerido, armazenamento bloqueado fica no padrão.

**Campos:** depois da varredura, a mesma thread roda a inferência do PR 8 (`estrutura::inferir`) e conta as chaves do topo dos registros (`resumo::campos`); o número chega por um último evento `varredura`. Sem registro-objeto, ou com topo que é mapa, o indicador não aparece.

**Fora desta fatia:** cartões de modos sem tela (busca, esquema, filtro).

**Conferência:** interface no navegador interno contra a ponte HTTP com `resumo` e o evento simulado por consulta a cada 50 ms; JSONL fictício de 400 MB do `gerador` com 60 linhas quebradas. Vistos: parcial "Analisando N de M", 60 inválidas no fim, pt-BR e inglês, claro e escuro, atalhos 2, ⌘0 e Enter, as três preferências. A janela nativa não foi vista.

## Consulta jq (Fase 1, 21/11/2026)

**Escolha:** `jaq` (jaq-core 3.1.2, jaq-std 3.0.4, jaq-json 2.0.3), em Rust, sem processo externo nem `libjq`. `nucleo::consulta`: `Consulta::nova(filtro)` compila uma vez; erro de compilação vem como `ErroFiltro { falhas: [Falha { faixa, linha, coluna, mensagem }] }` (linha e coluna a partir de 1, coluna em caracteres), nunca pânico. `executar_jsonl` roda o filtro em cada linha, no mesmo molde da busca (blocos de ~256 KiB que terminam em `\n`, `thread::scope`, no máximo 64 blocos adiante, reordenação na thread do chamador); `executar_documento` trata o arquivo inteiro como um valor JSON. Resultados chegam por callback em `LoteConsulta { resultados, linhas, total_linhas }`, em ordem de arquivo, cada um com a linha de origem (a partir de 0) e o JSON compacto. CLI: `jasao consultar <filtro> <arquivo> [--documento] [--tempo-max SEG]`, JSONL no stdout, erros por linha e resumo no stderr; filtro que não compila sai com código 1. `.gz`/`.zst` passam pela mesma `abrir`.

**Erros por linha:** linha só de espaço é pulada. Linha que não é JSON, com mais de 10 000 níveis de aninhamento (o mesmo teto do `jq`; sem ele, o parser recursivo do `jaq` pode estourar a pilha) ou em que o filtro falha (`"x" + 1`) conta em `ResumoConsulta::erros`; os 100 primeiros ficam com linha e motivo, e a varredura segue. Como no `jq`, os resultados que a linha produziu antes do erro ficam; os seguintes, não.

**Limites explícitos:** um trabalhador junta no máximo `max_lote` resultados (padrão 10 000) antes de entregar, e um lote nunca passa disso, mesmo numa linha que explode (`range(1e12)`). Cancelamento (`AtomicBool`) e teto de tempo opcional (`com_tempo_max`) são conferidos entre linhas e entre resultados; visto o sinal, nenhum lote sai. Filtro limitado a 16 KiB e compilado e avaliado em threads de 256 MiB de pilha (reserva virtual): o parser desce no filtro e um filtro de 16 KiB todo de `-` ou `(` compila e roda até em debug; com 128 MiB em debug, ou 64 MiB em release a partir de ~60 mil níveis, abortava.

**Limite conhecido (cancelar):** o `jaq` não tem ponto de interrupção. Filtro que gira sem entregar resultado (`last(range(1e12))`, `[range(1e12)]`, `def f: f; f`) só é visto parar quando termina, e recursão sem fundo (`def f: 1 + f; f`) estoura a pilha e aborta o processo inteiro. Por isso a CLI, com `--tempo-max`, tem um vigia que encerra o processo (código 3) dois segundos depois do teto; em recursão sem fundo a CLI continua abortando (SIGABRT), o que só derruba aquele comando. O app roda a consulta num processo filho (ver "Consulta em processo filho", 22/11/2026).

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

## Micronavegador (Fase 1, 22/11/2026)

**O que é:** régua de 44 px à direita da Árvore e do Texto com o arquivo inteiro: uma faixa por balde de registros, colorida pela variante do discriminador, inválidas em vermelho com balões numerados e o retângulo da janela visível. O cabeçalho mostra "linhas X–Y de N" (só linhas inteiras na tela). Árvore e Texto ganham um ponto de cor por registro.

**Escala:** a régua anda por registro, não por byte nem por linha da árvore expandida; o retângulo vai do primeiro ao último registro visível, com no mínimo 6 px. JSON único (uma linha ou várias) não tem régua nem faixa.

**Marcas:** `nucleo::regua` guarda uma marca por registro em `Box<[AtomicU8]>` (0 neutro, 1–4 variantes, 5 outros, 255 inválida), compartilhada pelas duas passadas e lida pela interface em baldes (`regua(k)`, k = altura em px, até 4.096) e em janelas de 1.000 (`marcas`). As inválidas saem da varredura do painel (`resumo::varrer`, mesma regra da árvore), não de `erros::varrer`: o painel é dono da varredura e a régua reaproveita.

**Duas passadas:** a 1ª é a varredura do painel; a régua aparece neutra com as inválidas e se atualiza a cada ~200 ms. Depois da inferência (`estrutura::inferir`, a mesma do indicador de Campos), `regua::colorir` pinta as variantes em paralelo (rayon, blocos de 4.096) sem tocar nas inválidas, e o evento `varredura` traz `legenda` e `coloriu`. A cor entra com fade de 0,4 s; com `prefers-reduced-motion` entra direto e os balões não pulsam.

**Discriminador:** de `Resumo::discriminadores()`, vence o que cobre mais registros (empate: o primeiro). As 4 variantes mais frequentes (empate: nome) ganham cor; o resto vira "outros". Legenda "por `campo`" na barra de ferramentas. Sem discriminador a régua fica neutra, sem legenda nem pontos.

**Interação:** `role=slider` com `aria-valuetext` da faixa; arrastar centraliza a janela; setas = uma página, PgUp/PgDn = 10%, Home/End. Inválidas a menos de 16 px viram um balão só (até "99+"), que pulsa 3 vezes ao surgir e, clicado, seleciona a primeira linha do grupo.

**Medido (1 GB, 6,46 milhões de linhas do `gerador`, Apple M5 Pro):** varredura 1,6–2,2 s; inferência 3,5–11,7 s (já existia, é a do indicador de Campos); `colorir` 130–270 ms. A cor chega depois de varredura + inferência; a 2ª passada em si fica bem abaixo de 2 s, então não foi otimizada. Se o tempo até a cor incomodar, o caminho é inferir só uma amostra para achar o discriminador.

**Conferência:** navegador interno contra a ponte HTTP (atraso artificial de 1,5 s antes da cor) com JSONL fictícios: 60 mil linhas de 3 tipos com inválidas agrupadas (18.000 ×4 e 18.200 ×3 viram um balão de 7; 45.000 ×9; 54.000 ×1) e 20 mil linhas sem discriminador. Vistos: régua neutra e depois a cor, legenda, pontos na Árvore e no Texto, faixa, arraste, PgDn, clique em balão, claro e escuro, pt-BR e inglês, JSON único sem régua. A janela nativa não foi vista.

## Recentes e associação de arquivo (Fase 1, 22/11/2026)

**Recentes:** os últimos 10 arquivos abertos (fixo, sem configuração), mais novo primeiro; abrir de novo sobe o arquivo para o topo. Ficam em `recentes.json` no diretório de dados do app (`~/Library/Application Support/br.com.moralia.jasao/` no macOS, `%APPDATA%\br.com.moralia.jasao\` no Windows, `~/.local/share/br.com.moralia.jasao/` no Linux): uma lista JSON só de caminhos, sem conteúdo de arquivo, que nunca sai da máquina. Código em `app/src-tauri/src/recentes.rs`.

**É o único dado que o app guarda entre execuções.** Além dele, o app só escreve cache descartável (descompactados de `.gz`/`.zst`, ver "Entrada compactada", e o resultado temporário da consulta jq); os arquivos abertos nunca são escritos. Preferência nova, abas restauradas ou qualquer outro estado salvo exige revisar esta frase.

**Gravação:** temporário na mesma pasta + `rename` (`tempfile::persist`), com `sync_all` antes: quem lê vê a lista antiga ou a nova, nunca metade. Arquivo ausente, ilegível ou corrompido vira lista vazia, sem erro e sem travar a abertura; a próxima abertura regrava a lista. Falha ao gravar só vai para o log: não impede abrir o arquivo. "Limpar recentes" apaga o arquivo, em vez de gravar lista vazia.

**O que entra:** só o arquivo aberto por `abrir` em documento novo (diálogo, arrastar, linha de comando, sistema, recentes). Exemplos embutidos (`exemplo:`) e o resultado da consulta jq não entram. Recente que sumiu do disco sai da lista ao ser escolhido (`recente_ausente`), com aviso na barra de status; a lista não é conferida no disco ao mostrar, para não tocar volumes de rede ou desmontados a cada aba nova.

**Onde aparecem:** na aba vazia (lista com nome e pasta) e no menu nativo Arquivo › Abrir recente, montado em Rust junto com o resto do menu do macOS e refeito na thread principal a cada mudança (o AppKit não aceita menu de outra thread). Escolher um recente segue o mesmo caminho de um pedido do sistema: abre numa aba nova, ou foca a aba que já tem o arquivo. Fora do macOS ainda não há menu de recentes, só a lista.

**Associação:** `bundle.fileAssociations` declara `.json` (com `public.json`, o tipo do sistema) e `.jsonl`/`.ndjson` (`application/x-ndjson`), com `role: Viewer` e `rank: Alternate`: o Jasão aparece em "Abrir com", mas não pede para ser o app padrão de nenhum tipo. Os compactados ficam de fora: `.gz` e `.zst` são genéricos, e declarar o Jasão para eles o ofereceria para qualquer arquivo compactado; `.json.gz` não se declara por extensão dupla. A mesma declaração vira `MimeType=` no `.desktop` do Linux e associação no instalador do Windows, mas só o macOS foi testado.

**Abertura pelo sistema:** mesmo desenho do Leve (`open.rs`), em `app/src-tauri/src/abertura.rs`. Tudo entra numa fila e cada caminho sai uma vez só. No macOS, Finder, "Abrir com" e `open -a` chegam como `RunEvent::Opened`, inclusive com o app fechado, e costumam vir antes do `setup`, sem garantia de ordem. Por isso `arquivos_iniciais` espera a abertura assentar (já chegou arquivo, ou 500 ms depois do `Ready`, teto de 2 s) antes de a interface decidir entre os arquivos pedidos e os exemplos. Com o app aberto, o evento enfileira, avisa a interface (`abrir-arquivos`, sem caminho) e traz a janela à frente; ela tira a fila com `arquivos_pedidos` e abre uma aba por arquivo, ou foca a aba de quem já está aberto. No Windows e no Linux o arquivo vem por argv: na 1ª instância, lido no `setup`; nas seguintes, `tauri-plugin-single-instance` repassa argv e pasta atual para a que já está aberta e encerra a nova. Argumento que começa com `-` não é caminho.

## Exemplos embutidos (Fase 1, 22/11/2026)

**O que é:** o app aberto sem arquivo mostra dois exemplos que vêm dentro do binário, como o Leve faz com o `sample:`. O primeiro é a história e a gramática do JSON (`historia.json`, em pt-BR, conferido contra RFC 4627, RFC 7159, RFC 8259, ECMA-404, RFC 6901 e RFC 9535, sem fato não verificado). O segundo é uma vitrine de tudo o que o app faz (`vitrine.jsonl`, 1.417 linhas). O registro-guia da vitrine sugere buscas, regex, jq e caminhos para testar. Com arquivo na linha de comando, os exemplos nunca aparecem.

**Como abre:** o Rust decide (`exemplos::inicial`): sem argumento, `arquivo_inicial` devolve `exemplo:historia`. `abrir` reconhece o caminho sintético (`exemplos::embutido`) e monta um `ArquivoLinhas::estatico` sobre os bytes do `include_bytes!`. Daí em diante o caminho é o de qualquer arquivo: índice, varredura, régua, árvore, texto, busca, jq. Não se usa arquivo temporário porque nada vai para o disco e o app continua só leitura. Atalho conhecido: um arquivo de verdade chamado literalmente `exemplo:historia`, passado por caminho relativo na linha de comando, abre o exemplo.

**Tela (substituída pela seção "Abas de arquivo e arrastar"):** não há sistema de abas de arquivos. Uma barra abaixo do topo (`role=region`) diz que é um exemplo embutido. Ela tem um alternador de dois botões com `aria-pressed` (História do JSON / Vitrine do Jasão) e "Abrir arquivo próprio…", que chama o mesmo diálogo do "Abrir…". O nome no topo ganha "(exemplo)". Abrir um arquivo real substitui o exemplo e esconde a barra. Código em `app/src/exemplos.ts`; `main.ts` ganhou só três ganchos.

**Por que a história é uma linha só:** JSON em várias linhas abre como texto linha a linha, e JSON em uma linha abre como `JsonUnico` (árvore de um registro). Para ler a história na Árvore, ela precisa ser `JsonUnico`.

**Vitrine:** tem todos os tipos e os vazios, 12 níveis de aninhamento, um array de 1.000 itens, unicode e escapes (incluindo U+2028), uma string acima do teto de colunas do Texto, números extremos como texto cru (`-0`, `5e-324`, `1e400`), inteiros grandes que o jq devolve exatos, chave duplicada, chaves com `/` e `~`, e um registro de valores inteligentes (época, data, cores, URL, imagem em base64, JWT fictício e JSON dentro de texto). São 1.400 registros de 4 tipos para colorir a régua e 6 linhas inválidas de propósito, 5 com motivos diferentes e 1 vazia. Os arquivos são gerados de forma determinística por `scripts/exemplos.py`, e os testes em `app/src-tauri/src/exemplos.rs` conferem cada capacidade e cada sugestão do guia pelo `nucleo`.

**Achado:** o `jaq` aceita `NaN` como literal (vira `null`), mas o `nucleo` o trata como inválido. A linha inválida de "NaN" virou "vírgula decimal" para a contagem bater nos dois. A divergência fica como pendência.

## Busca, copiar caminho e jq na interface (Fase 1, 22/11/2026)

**Busca:** barra própria abaixo da barra de modos, aberta por ⌘F; Enter / ⇧Enter / ⌘G / ⇧⌘G andam pelos achados, Esc volta o foco à visão. O comando roda `nucleo::busca` numa thread, emite lotes por evento com a geração da busca e guarda o `AtomicBool` para cancelar ao digitar de novo ou fechar; o front descarta evento de geração velha. Até 1 milhão de achados navegáveis (a contagem segue até o fim); a navegação começa no primeiro achado do arquivo, não no mais próximo da seleção. Na árvore o destaque fica na prévia do valor; no Texto, sobre a linha. Os achados viram marcas na régua. Regex inválida limpa os achados e mostra o erro no campo.

**Copiar caminho:** os textos vêm do núcleo (`Arvore::caminho` e `Arvore::valor`, comandos `caminhos` e `valor`), não do front; o caminho que o front já montava fica só no rótulo acessível da linha. Menu de contexto em HTML (botão direito, tecla Menu ou ⇧F10), com setas, Home/End e Esc; ⇧⌘C copia o caminho jq e ⌘C copia o valor quando não há texto selecionado (com seleção, ⌘C fica com o navegador). A escrita usa `navigator.clipboard.write` com `ClipboardItem` de promessa, para o WebKit aceitar a escrita dentro do gesto mesmo com o texto chegando depois do `invoke`: sem plugin e sem permissão nova. Valor acima de 16 MB é recusado. A confirmação aparece na barra de status por 2,5 s.

**Consulta jq:** campo na mesma faixa da busca, aberto por ⌘J. Roda sempre sobre o arquivo **original** (uma consulta nova não encadeia sobre o resultado anterior). `consultar` compila o filtro (erro de sintaxe volta com linha e coluna e o front seleciona o trecho no campo), escolhe modo documento para JSON único ou de várias linhas e JSONL para o resto, e grava os resultados, num processo filho, num JSONL temporário (`jasao-consulta-<pid>-<geração>-<nanos>.jsonl`, criado com `create_new` e 0600). No fim, o temporário abre na visão atual como um arquivo comum, sem passar pelo painel de entrada; "Voltar ao arquivo" reabre o original. O temporário é apagado ao voltar, numa consulta nova, ao cancelar e ao sair do app; por isso cancelar uma consulta feita por cima de um resultado aberto volta ao arquivo original.

**Limites conhecidos:** sem teto de tempo na interface (o usuário cancela, e o cancelar mata o processo filho mesmo com o filtro preso; ver "Consulta em processo filho"). No Windows, um temporário aberto pela visão pode não ser apagado. O destaque do erro de sintaxe usa o comprimento em bytes da faixa, aproximado para texto fora do ASCII.

## Abas de arquivo e arrastar (Fase 1, 22/11/2026)

**Estado por documento:** o Rust guarda um mapa `id → documento` (índice, árvore, varredura, busca e consulta de cada arquivo). `abrir` devolve o id e todos os comandos e eventos levam o id; id desconhecido volta como erro legível ("documento N não está aberto"), não pânico. `fechar` libera o documento e cancela busca, varredura e consulta dele. No front, cada aba guarda modo, expansão, seleção, rolagem, régua, busca e consulta jq; a aba ativa vive nas variáveis do módulo e é guardada ao sair (`guardar`) e restaurada ao entrar (`ativar`). Respostas que chegam depois da troca são descartadas pela `versao`; aba aberta no Rust mas ainda não mostrada é preparada quando ativada.

**Barra:** `role=tablist` abaixo do topo, um botão por aba com tabindex móvel (setas, Home/End, Delete fecha) e botão de fechar fora da ordem do Tab. Nomes iguais ganham as pastas de cima até se distinguirem (`dados.jsonl — a`). Atalhos: ⌘T nova, ⌘W fecha, ⌘1…8 posição, ⌘9 última, Ctrl+Tab / Ctrl+⇧Tab. As visões Árvore/Texto passaram para ⌥⌘1/⌥⌘2. Fechar a última aba deixa uma aba vazia com o estado sem arquivo. Abrir um arquivo já aberto foca a aba dele. Os dois exemplos embutidos viram duas abas; o alternador saiu. Só a aba ativa leva `aria-controls`: no WebKit, uma aba cujo painel tem o foco conta como selecionada, e com todas apontando para o mesmo painel o VoiceOver anunciava todas como selecionadas (visto no app nativo).

**⌘W no macOS:** o menu padrão do Tauri liga ⌘W a fechar a janela antes de o webview ver a tecla. O app monta o próprio menu (app, Editar, Janela) sem "Fechar janela", e ⌘W fica para a aba.

**Arrastar:** `getCurrentWebview().onDragDropEvent` do Tauri 2, sem plugin e sem permissão nova em `capabilities`. Sobreposição de alvo enquanto o arquivo paira. Abrem `.json`, `.jsonl`, `.ndjson` e os mesmos com `.gz`/`.zst`, cada um numa aba; outro formato e pasta viram aviso na barra de status, sem abrir aba. O teste de pasta é um comando (`pastas`), porque a extensão não diz se o caminho é pasta. O diálogo "Abrir…" passou a aceitar vários arquivos.

**Memória:** cada documento aberto guarda o próprio índice e árvore. Acima de 50 milhões de registros somados nas abas, abrir mais um avisa no status. Medido no app release (Apple M5 Pro) com o JSONL de 1 GB e 6,46 milhões de linhas do `gerador`: abre em 221 ms, com 99 MB de memória própria (`vmmap`, physical footprint). O RSS de 1,2 GB é quase todo o arquivo mapeado, que o sistema recupera. Então 50 milhões de registros dão cerca de 770 MB. Fechar a aba devolve a memória (RSS de 1,22 GB para 171 MB no debug). Abrir é assíncrono, então arquivo grande não trava a interface.

**Fora:** recentes, associação de tipo, restaurar abas, reordenar e destacar abas.

## Modo Estrutura (Fase 2, 22/11/2026)

**O que é:** terceiro modo ao lado de Árvore e Texto (⌥⌘3), com a estrutura inferida pelo `nucleo::estrutura`: uma linha por caminho (campo recuado pela profundidade | tipos vistos | presença) e, depois dos caminhos, uma linha por variante de cada discriminador (`.campo = "valor"` | sempre / às vezes | N registros). Segue a tabela do `docs/mockup-json.html`. O front não infere nada: `app/src/estrutura.ts` só formata o que o comando devolve.

**Sem trabalho novo pesado:** o `abrir` já roda `estrutura::inferir` em segundo plano (para o indicador de Campos e a régua), com cancelamento por `AtomicBool` e `geracao`. O resultado agora fica guardado em `Varredura.estrutura` (`Arc`), e o comando `estrutura` só o lê: devolve `null` enquanto a inferência não terminou. O evento `varredura` ganhou `inferiu`; a tela mostra "Inferindo…" e carrega quando ele chega.

**Presença:** campo = "todos" ou "N de M" (M = objetos onde o pai apareceu), opcional em destaque; `[]` = "N itens em M listas"; mapa (`*`) = "N entradas em M objetos". Vários tipos no mesmo caminho mostram a contagem de cada um.

**Ir para a ocorrência:** Enter ou duplo clique troca para a Árvore no registro da 1ª ocorrência (`Contagem::primeira()`, a linha base 0, que na Árvore é o índice do registro). Vai ao registro, não ao nó exato.

**Com busca, jq e Ir para linha:** a Estrutura não tem registros. Achado da busca e Ir para linha trocam para a Árvore; o resultado do jq abre na Estrutura do arquivo de resultado; Esc na barra devolve o foco à Estrutura.

**Teclado e leitor de tela:** `role=listbox` com `aria-activedescendant`, linhas `role=option` com rótulo completo (caminho, tipos, presença, opcional); setas, PgUp/PgDn, Home/End, Enter. O resumo (`role=status`) diz quantos caminhos e registros e que Enter abre a Árvore. Copiar caminho copia o caminho da linha.

**Conferência:** front no navegador interno contra a ponte HTTP (a mesma dos PRs #12, #16 e #18) com os comandos reais: JSONL fictício de 3.000 linhas (3 tipos, 3 inválidas, campo opcional com dois tipos), JSON único fictício e os dois exemplos embutidos. Vistos: Estrutura pelo painel (3) e por ⌥⌘3, caminhos, variantes, opcional, Enter levando à Árvore no registro certo, clique e duplo clique, `aria-activedescendant` e rótulo da linha, pt-BR e inglês, claro e escuro. A janela nativa abriu (`pnpm tauri dev`), mas não foi vista: o shell não tem permissão de Gravação de Tela.

**Por aba:** cada documento guarda a própria inferência no Rust (`estrutura` leva o `id`, como os outros comandos) e cada aba guarda no front a lista formatada e a seleção. Trocar de aba não refaz o pedido; resposta que chega depois da troca é descartada (documento ativo e `geracao`).

**Pendências:** abrir o nó exato na Árvore (não só o registro); progresso e resultado parcial da inferência (meta de 500 ms do plano); virtualizar a lista (hoje desenha tudo; `shortcut:` no código); filtro por variante como no mockup.

## Consulta em processo filho (22/11/2026)

**Problema** (auditoria de segurança, achados 6, 8 e 9): no app, a consulta jq rodava dentro do processo. Filtro preso (`def f: f; f`) não via o cancelar, recursão sem fundo (`def f: 1 + f; f`) abortava o app inteiro, a saída não tinha teto (um `range(1e12)` enchia o disco) e `consultar` era síncrono, de modo que abrir um FIFO travava a interface.

**Escolha:** o app reexecuta o próprio binário (`std::env::current_exe()`) com `JASAO_CONSULTA_FILHO=<arquivo>` no ambiente (filtro e destino em outras duas variáveis); o `main` vê a variável antes de subir o Tauri e roda só a consulta (`app/src-tauri/src/filho.rs`). Foi escolhido em vez de empacotar a CLI `jasao` como sidecar: não muda empacotamento nem `tauri.conf.json`, e o filho reaproveita `abrir_linhas` (exemplos embutidos, `.gz`/`.zst`) e `nucleo::consulta` sem protocolo novo de argumentos. O filho grava os resultados direto no temporário que o pai criou (`create_new`, 0600) e manda andamento e fim como `AndamentoConsulta` em JSON, um por linha, no stderr.

**O que passou a valer:**
- Cancelar (botão, Esc, consulta nova, voltar, fechar a aba) mata o filho: vale para filtro que gira sem entregar.
- Recursão sem fundo ou falta de memória derrubam só o filho; o pai devolve `falha` ("a consulta parou sem terminar…", com a última linha do stderr do filho) e o app segue.
- Teto de 1 GiB no resultado em disco (`filho::TETO_SAIDA`): ao passar, o filho para, o que já foi gravado abre normalmente e o evento traz `cortado: true`; a interface avisa "resultado parcial".
- `consultar` é `async` (fora da thread da interface) e não abre o arquivo: confere com `metadata` (que não bloqueia) e recusa o que não é arquivo comum (FIFO, socket, dispositivo) com erro claro. O filtro ainda é compilado no pai, só para devolver o erro de sintaxe com posição (compilar é limitado por `MAX_FILTRO`).
- Compactado: quem descompacta é o filho (pelo cache; em geral já está lá desde a abertura), então o Cancelar também interrompe a descompactação, matando o filho. O `.parcial` de um filho morto fica no cache até a limpeza de parciais com mais de um dia (o `kill` não deixa o filho apagar). Razão e teto estourados no filho chegam como `jasao:razao`/`jasao:teto` e a interface traduz como na abertura.
- Se o app morrer, o stdin do filho (um pipe que o pai segura) fecha e o filho sai; não fica consulta órfã girando.

**Testado:** `filho::testes` (teto, cancelar com filtro preso, recursão sem fundo) e `comandos_pelo_ipc` (recursão pelo comando, FIFO recusado em menos de 2 s). Nos testes o filho é o próprio binário de teste rodando só `filho::testes::modo_filho`.

**Não verificado:** janela nativa aberta (o filho no `.app` empacotado do macOS e no Windows, onde o binário é do subsistema gráfico).

## Caminhos concedidos no IPC (achado 13, 23/11/2026)

**Escolha:** `abrir`, `consultar` e `pastas` só aceitam caminho que o backend concedeu nesta execução (diálogo, agora no Rust pelo comando `escolher`; arraste, pelo evento de soltar no Rust; argv, associação e 2ª instância; o resultado de consulta que ele mesmo cria) ou que está em `recentes.json` (só recebe caminho que já passou pela checagem e mora no diretório de dados, fora do alcance da webview), comparando o canônico e abrindo o canônico; o resto recebe `jasao:negado`, exista o arquivo ou não, e `recente_ausente` só olha o disco para caminho da lista.

**Arraste e ordem dos eventos:** no Tauri 2.12 o evento de soltar chega à interface antes do handler do Rust que concede. Não importa: a interface chama primeiro `pastas`, comando síncrono que roda na thread principal, a mesma do handler, e só então `abrir`.

**Limite residual (TOCTOU):** entre o `canonicalize` do `autorizado` e a abertura há uma janela em que o arquivo concedido pode ser trocado (por exemplo, por um link para outro arquivo). Explorar exige escrever na pasta do arquivo concedido, com o mesmo usuário do app; quem já faz isso lê o arquivo sem passar pelo Jasão. Abrir uma vez e passar o descritor adiante (ao núcleo e ao filho da consulta) fecharia a janela, mas muda a API de abertura do núcleo e o protocolo do filho; não vale o custo agora.

**Testado:** `caminhos_concedidos_pelo_ipc` (não concedido, pasta, `..` e link resolvidos pelo alvo real, link e pasta-link repontados depois da concessão, `autorizado` devolvendo o canônico, consulta recusada sem mexer na que está em curso) e `fontes_de_concessao` (soltar, pedido do sistema, argv e diálogo concedem).

## Exportar Zod e JSON Schema (Fase 4, 23/11/2026)

**Um gerador só, em Rust.** `nucleo::exportar` monta um modelo (`Forma`) a partir do `Resumo` da inferência e o imprime em dois formatos: JSON Schema 2020-12 e Zod 4. Não usamos `z.toJSONSchema()` (gerar o Zod e derivar o Schema dele) porque o app não pode depender de Node em runtime. O Schema é a fonte da autovalidação; o Zod sai da mesma `Forma`, e os dois são conferidos por testes separados.

**Forma do que sai** (segue o `docs/mockup-json.html`):
- Objetos permissivos: sem `additionalProperties: false` nem `.strict()`. A estrutura descreve o que foi visto, não proíbe o resto.
- Campo opcional quando a presença é menor que 100% (`required` / `.optional()`); nullable quando `null` foi visto (`"null"` no `type` / `.nullable()`).
- União discriminada (`z.discriminatedUnion` / `anyOf` de objetos) quando todos os registros são objetos e há discriminador no topo; com mais de um candidato, vence o de menos variantes. Na variante, o discriminador vira literal e os outros campos usam a forma geral do caminho.
- Mapa (`.*`) vira `z.record(z.string(), …)` / `additionalProperties`; lista, `z.array` / `items`.
- "Campo com um valor só": genérico (padrão) ou travado no valor (`const` / `z.literal`). Só trava escalar de um tipo, valor de até 256 bytes, e número só se for inteiro seguro.
- "Número guardado como texto": manter (Schema `{type: string, pattern}`, Zod `z.string().regex(padrão)`) ou tratar como número (Schema `number`, Zod `z.string().regex(padrão).transform(Number)`). Com "tratar como número", o Schema reprova os registros que têm o texto, e a interface diz isso; o Zod aceita, porque confere o texto e converte. `z.coerce.number()` foi descartado: aceita `null`, `""`, `[]`, `[7]` e `true`, e aí o Zod aceitaria mais que o Schema que mede o "N de M". A autovalidação do Zod usa o Schema da entrada que ele aceita (`schema_da_entrada_zod`: o mesmo com `numero` desligado), equivalente ao Zod gerado; o validador é o `jsonschema`, não o Zod, e o `pnpm test` confere os dois nos dourados. Literal vence número‑como‑texto.
- Inteiro maior que 2^53 visto no caminho: `z.number()` em vez de `z.int()`, porque `z.int()` recusa inteiro não seguro (Zod 4.6.7).

**Segurança.** Nome de campo e literal só entram no código por `exportar::aspas`: string JSON com `\uXXXX` para `"`, `\`, controles C0/C1 e DEL, `<`, `>`, `&` (nada de `</script>`), U+2028/2029 e os invisíveis e bidi (U+061C, U+200B–200F, U+202A–202E, U+2060–2069, U+FEFF). A saída é ASCII nesses pontos e continua JSON válido, então também é literal válido em TS. No Zod, identificador ASCII sai sem aspas e `__proto__` sai como chave computada (`["__proto__"]`), que é propriedade comum e não troca o protótipo. Nenhum outro valor do arquivo entra no código. Teste com chaves hostis em `core/tests/exportar.rs` e `app/tests/dourados/hostil.jsonl`.

**Autovalidação com validador independente.** `nucleo::validar` confere o arquivo inteiro contra o Schema gerado com o crate `jsonschema` 0.63 (MIT), lendo cada registro com `serde_json` (não com o leitor da inferência). JSONL em blocos paralelos (rayon, mesmo `fatiar` da busca); JSON único em lotes paralelos sobre `estrutura::registros_json_unico`. Progresso por bytes, cancelamento por `AtomicBool`, resultado "N de M registros passam", linhas que não são JSON à parte, e as 20 primeiras falhas (menor linha) com caminho e motivo; no `anyOf`, o motivo vem do ramo cujo discriminador bateu (sem erro de `const`) e, entre eles, do de menos erros; contar só erros escolhia o ramo errado quando o registro tinha vários campos em falta. Guardadas as 20 falhas, o resto usa `is_valid` (sem montar o erro): no arquivo de 1 GB que reprova 2,1 M registros, a validação caiu de 13,3 s para 3,3 s no app.

**Dependência nova: `jsonschema` (com `default-features = false`) e `serde_json`.** Precisávamos de um validador que não fosse o nosso gerador, senão a prova é circular. `jsonschema` é o validador 2020-12 mais completo e mais rápido em Rust, puro Rust, MIT. Sem as features padrão não puxa cliente HTTP nem TLS (sem `reqwest`/`aws-lc`): `$ref` remoto não é resolvido, e o Schema gerado não tem. Entram 23 crates no lock, todas MIT, Apache‑2.0 ou MIT‑0 (`fancy-regex`, `referencing`, `fluent-uri`, `num-*`, `uuid-simd` e afins). `serde_json` já vinha transitivo e passa a ser direto (é a entrada do `jsonschema`). Alternativa considerada: `boon` 0.6, menor, mas menos usado e mais lento.

**Provas:** dourados de cada formato e das duas combinações de opções (`app/tests/dourados/*.zod.ts` e `*.schema.json`, regravados por `ATUALIZAR_DOURADOS=1`); proptest em que todo registro de JSONL aleatório passa no Schema gerado (opções padrão e literal), conferido pelo `jsonschema`; caso que reprova (número como texto + tratar como número, com linha e motivo); `app/tests/zod.test.ts` roda os `.zod.ts` dourados com o `zod@4` de verdade (devDependency do `app`) no `pnpm test`.

**CLI:** `jasao exportar --zod|--schema [--literal] [--numero-texto manter|numero] arquivo` imprime o código no stdout e, no stderr, "N de M registros passam" e as falhas; sai com 1 se alguma falhar.

**Limites conhecidos:** JSON único com objeto no topo é um registro só: o `serde_json` o lê inteiro e a validação não cancela no meio dele (cancela entre registros). O fuzz curto passou de 27 para 25 s por alvo (`FUZZ_SEG`, CI e `gates.sh`) para o alvo novo `exportar` caber no mesmo tempo do job (13 × 25 s ≈ 12 × 27 s).

**Interface (vista Exportar, ⌥⌘4):** o núcleo gera o código pelo comando `exportar`, que devolve o texto e uma `geracao`; a validação roda numa thread e manda o evento `validacao` (progresso com throttle e o fim). Trocar opção, aba ou vista cancela a anterior pela geração; Esc também cancela. O status é honesto: "N de M registros passam" e, no JSON Schema com "tratar como número", diz que o Schema não converte e sugere "Manter texto" ou Zod. Copiar usa `copiarTexto`; Salvar abre o diálogo de salvar no Rust (`salvar_exportado`, mesmo padrão do `escolher`), sem permissão de diálogo no front.

**Conferido no app (ponte HTTP + navegador, dados fictícios gerados na hora):** união discriminada, número como texto, campo opcional, `null` e linha quebrada (3.000 registros: 2.999 passam, 1 linha inválida à parte); chaves hostis escapadas na tela, com "Travar no valor"; pt-BR, inglês e tema escuro; Copiar (texto igual ao mostrado); Esc. Arquivo de 1 GB (6.306.549 registros, build release, arquivo em cache, 18 núcleos): validação de 1,0 a 2,9 s quando tudo passa (Zod, Schema e literal) e 2,8 a 3,3 s quando reprova; CLI 1,3 a 2,7 s.

**Pendências:** formatos (`date-time`, `email`, `uuid`) não são inferidos; uniões discriminadas só no topo (objetos aninhados com `tipo` saem como objeto com campos opcionais); `enum` para campo de poucos valores; tipos TS puros, Pydantic e afins ficam fora desta fase.

## Valores inteligentes (Fase 1, 23/11/2026)

**Escolha:** a detecção mora em TypeScript (`app/src/valores.ts`, pura, sem DOM, rede nem relógio), não no núcleo: é apresentação, e o núcleo continua entregando o texto cru. Época em segundos ou ms, só se o nome do campo parece data com fronteira de palavra (`criado_em`, `createdAt`, `ts`, `atualizado_ms`; não `item`, `lat`, `format`, `system`), porque 10 e 13 dígitos são comuns em id e EAN; 2001 a 2100. Data ISO 8601 com componentes conferidos (`2024-02-30` não é data); sem fuso é hora local, também a forma só com data, cor (hex e `rgb()`/`rgba()` com canais conferidos), URL http(s), imagem em base64 (data URI ou base64 cru com assinatura de PNG, JPEG ou GIF), JWT e JSON dentro de texto. Na Árvore, um selo ao lado do valor; abaixo dela, o painel do valor selecionado (`app/src/detalhe.ts`).

**Prévia cortada:** o selo sai da prévia da linha (200 bytes). Se o valor não coube, o selo só aparece quando a própria prévia (sem o `…` que o núcleo põe no fim do corte) confirma barato: imagem com data URI ou assinatura seguida só de base64, JWT cujo cabeçalho decodifica para JSON com `alg`, URL sem espaço que o `URL` aceita, e tamanho dentro do teto. JSON dentro de texto não ganha selo pela prévia (um `[2026-11-23] log` não vira JSON); o painel pede o valor inteiro ao Rust (`valor`), só até 1 MiB, e decide então.

**Tetos:** 1 MiB para o que passa por base64 (imagem, JWT) e por `JSON.parse` (JSON dentro de texto); 100 níveis ao reindentar (a indentação cresce com o quadrado do nível), e a reindentação é linear no tamanho (perto de 1 MiB em menos de 2 s no teste) e, se passaria de 16 MiB (o `MAX_FORMATADO` do núcleo), o painel mostra o texto cru; URL até 2.048 caracteres. O JSON é reindentado por um tokenizador, sem passar os valores pelo `JSON.parse`, então número grande e chave repetida ficam como no arquivo.

**Segurança:** todo texto do arquivo entra no HTML por `visivel`/`escAtributo`. O HTML sai de `app/src/apresenta.ts`, puro, testado com carga hostil em JWT, JSON em texto, URL, cor e imagem; tirar `visivel` ou `escAtributo` derruba os testes. O `href` da URL é o normalizado pelo `URL` (ASCII, domínio internacional em punycode, bidi e invisíveis percentualmente codificados): o que se vê é o que se abre. O selo de link mostra o destino (host, e o endereço no `title`); na prévia cortada, o host só aparece se terminou dentro dela; só o botão abre, nunca o clique na linha, pelo comando `abrir_link` do Rust, que de novo recusa o que não for http(s) em ASCII e manda ao navegador do sistema (`open`, `xdg-open`, `rundll32`); a webview nunca navega. JWT: só cabeçalho e carga, a assinatura é descartada na detecção e nunca é conferida nem enviada. Imagem: só data URI base64 do próprio arquivo, `src` posto pela propriedade; a CSP ganhou `img-src 'self' data:` e nada de rede.

**Acessibilidade:** amostra de cor com `role="img"` e rótulo; painel com `aria-live="polite"`; blocos com `tabindex="0"` e rótulo; o link do painel é botão de verdade (Tab e Enter). O selo de link na árvore fica fora da ordem do Tab (`tabindex="-1"`) para não quebrar a navegação por setas. A árvore observa a altura do painel (`ResizeObserver`) e mantém a seleção à vista, porque a imagem só ganha tamanho depois de carregar.

**Limites:** a Árvore não expande o JSON dentro de texto em nós (só o painel mostra reindentado). Base64 cru de WebP, BMP e AVIF só com `data:`. Fuso da data relativa é o da máquina.

**Conferência:** `app/tests/valores.test.ts` (12 testes, incluindo homógrafo, bidi, tetos, datas inexistentes e tempo perto de 1 MiB), `app/tests/apresenta.test.ts` (HTML com carga hostil), `link_so_http_em_ascii` no Rust, o registro "valores inteligentes" da vitrine testado em `exemplos.rs`, e a tela vista no navegador do app por uma ponte HTTP de teste para os comandos do Rust (a janela nativa do Tauri não foi aberta).
