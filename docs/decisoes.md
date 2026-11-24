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

**Medido (23/11/2026, M-series, ponte HTTP de teste + Vite, JSONL gerado de 1 GB com 11.262.093 registros de 3 tipos, 114 linhas inválidas, cache de disco quente):** abrir e inferir 1,2 s; colunas na tela logo que a inferência acaba (11 colunas). Janela: `tabela_linhas` de 60 linhas × 11 colunas em 2 ms em qualquer ponto do arquivo; salto de rolagem até a janela desenhada em 4–10 ms. Ordenar por `.id` os 11,3 milhões: ~1 s até a ordem pronta; trocar para decrescente 33 ms, sem progresso; voltar à ordem do arquivo, imediato. Cancelar no meio mantém a ordem anterior.

**Limites conhecidos:** profundidade máxima 128 (além disso a linha é inválida); a lista externa do JSON único não conta como nível. UTF-8 não é validado (o texto é tratado como bytes). JSON único é lido sem paralelismo.

**Medido:** `jasao inferir` em JSONL gerado de 157 MB (951 mil linhas, cache quente, Apple M5 Pro): 128–168 ms; `--json` 140 ms; o mesmo arquivo em `.zst`, 128 ms depois de descompactar.

## Painel de entrada (Fase 1, 21/11/2026)

**O que é:** ao abrir um arquivo aparece um painel não modal no lugar do conteúdo (o arquivo já está aberto): nome, tipo detectado, tempo de abertura, Tamanho, Registros, Linhas inválidas (só se houver) e cartões dos modos com tela. "Início" no cabeçalho e ⌘0 voltam a ele; as abas continuam e ficam sem seleção enquanto ele aparece.

**Tipo detectado:** `nucleo::resumo::conteudo` olha só as 1.000 primeiras linhas: vazio, JSON único, JSON em várias linhas (1ª linha inválida e o arquivo inteiro válido), JSON Lines de objetos, de valores ou misto.

**Varredura de inválidas:** `nucleo::resumo::varrer` valida todas as linhas em paralelo (`thread::scope`, blocos de 4.096 linhas por `AtomicUsize`, sem rayon) com a mesma regra da árvore (`arvore::validar`), então o número bate com o que a árvore mostra, linha vazia inclusive. O PR 7 (`erros::varrer`) trata linha vazia como não erro; quando ele entrar, alinhar as duas regras. `abrir` dispara a varredura numa thread e devolve na hora; o progresso vai pelo evento `varredura` a cada ~50 ms (`{geracao, varridas, total, invalidas, fim}`) e o comando `resumo` devolve o estado atual. Abrir outro arquivo cancela a varredura anterior (`AtomicBool`) e sobe a `geracao`, que descarta avisos atrasados. JSON em várias linhas pula a varredura: cada linha dele é inválida sozinha. No JSONL fictício de 400 MB (2,4 milhões de linhas), 2,18 milhões já estavam varridas 50 ms depois de abrir.

**Cartões declarativos:** `MODOS` em `main.ts`, um registro por modo com `disponivel` e `sempre` (entra no seletor "Abrir sempre em…"). Hoje Tabela, Árvore, Texto e Estrutura. O sugerido é o primeiro cartão que o arquivo aceita: a Tabela quando ela serve (antes da inferência acabar, quando o conteúdo é JSON Lines de objetos), senão a Árvore. O cartão da Tabela fica desligado, com o motivo, quando o arquivo não vira tabela (ver Modo Tabela). Atalhos 1, 2… por posição; Enter abre o sugerido.

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

**O que o app guarda entre execuções:** esta lista; e, no `localStorage` da interface, a preferência de visão ao abrir e os filtros jq salvos (ver "Consulta jq ao vivo, filtros salvos e CSV"). Nada disso sai da máquina. Além disso, o app só escreve cache descartável (descompactados de `.gz`/`.zst`, ver "Entrada compactada", e o resultado temporário da consulta jq) e o que o usuário manda salvar num diálogo; os arquivos abertos nunca são escritos. Estado salvo novo (abas restauradas, por exemplo) exige revisar esta frase.

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

**Limites conhecidos:** o cancelar mata o processo filho mesmo com o filtro preso; além dele, o filho tem teto de relógio e de heap (ver "Consulta jq ao vivo, filtros salvos e CSV"). No Windows, um temporário aberto pela visão pode não ser apagado. O destaque do erro de sintaxe usa o comprimento em bytes da faixa, aproximado para texto fora do ASCII.

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

**Interface (vista Exportar, ⌥⌘5; era ⌥⌘4 até a Tabela entrar antes dela):** o núcleo gera o código pelo comando `exportar`, que devolve o texto e uma `geracao`; a validação roda numa thread e manda o evento `validacao` (progresso com throttle e o fim). Trocar opção, aba ou vista cancela a anterior pela geração; Esc também cancela. O status é honesto: "N de M registros passam" e, no JSON Schema com "tratar como número", diz que o Schema não converte e sugere "Manter texto" ou Zod. Copiar usa `copiarTexto`; Salvar abre o diálogo de salvar no Rust (`salvar_exportado`, mesmo padrão do `escolher`), sem permissão de diálogo no front.

**Conferido no app (ponte HTTP + navegador, dados fictícios gerados na hora):** união discriminada, número como texto, campo opcional, `null` e linha quebrada (3.000 registros: 2.999 passam, 1 linha inválida à parte); chaves hostis escapadas na tela, com "Travar no valor"; pt-BR, inglês e tema escuro; Copiar (texto igual ao mostrado); Esc. Arquivo de 1 GB (6.306.549 registros, build release, arquivo em cache, 18 núcleos): validação de 1,0 a 2,9 s quando tudo passa (Zod, Schema e literal) e 2,8 a 3,3 s quando reprova; CLI 1,3 a 2,7 s.

**Pendências:** formatos (`date-time`, `email`, `uuid`) não são inferidos; uniões discriminadas só no topo (objetos aninhados com `tipo` saem como objeto com campos opcionais); `enum` para campo de poucos valores; Pydantic e afins ficam fora desta fase (tipos TS entraram em 24/11/2026, seção abaixo).

## Valores inteligentes (Fase 1, 23/11/2026)

**Escolha:** a detecção mora em TypeScript (`app/src/valores.ts`, pura, sem DOM, rede nem relógio), não no núcleo: é apresentação, e o núcleo continua entregando o texto cru. Época em segundos ou ms, só se o nome do campo parece data com fronteira de palavra (`criado_em`, `createdAt`, `ts`, `atualizado_ms`; não `item`, `lat`, `format`, `system`), porque 10 e 13 dígitos são comuns em id e EAN; 2001 a 2100. Data ISO 8601 com componentes conferidos (`2024-02-30` não é data); sem fuso é hora local, também a forma só com data, cor (hex e `rgb()`/`rgba()` com canais conferidos), URL http(s), imagem em base64 (data URI ou base64 cru com assinatura de PNG, JPEG ou GIF), JWT e JSON dentro de texto. Na Árvore, um selo ao lado do valor; abaixo dela, o painel do valor selecionado (`app/src/detalhe.ts`).

**Prévia cortada:** o selo sai da prévia da linha (200 bytes). Se o valor não coube, o selo só aparece quando a própria prévia (sem o `…` que o núcleo põe no fim do corte) confirma barato: imagem com data URI ou assinatura seguida só de base64, JWT cujo cabeçalho decodifica para JSON com `alg`, URL sem espaço que o `URL` aceita, e tamanho dentro do teto. JSON dentro de texto não ganha selo pela prévia (um `[2026-11-23] log` não vira JSON); o painel pede o valor inteiro ao Rust (`valor`), só até 1 MiB, e decide então.

**Tetos:** 1 MiB para o que passa por base64 (imagem, JWT) e por `JSON.parse` (JSON dentro de texto); 100 níveis ao reindentar (a indentação cresce com o quadrado do nível), e a reindentação é linear no tamanho (perto de 1 MiB em menos de 2 s no teste) e, se passaria de 16 MiB (o `MAX_FORMATADO` do núcleo), o painel mostra o texto cru; URL até 2.048 caracteres. O JSON é reindentado por um tokenizador, sem passar os valores pelo `JSON.parse`, então número grande e chave repetida ficam como no arquivo.

**Segurança:** todo texto do arquivo entra no HTML por `visivel`/`escAtributo`. O HTML sai de `app/src/apresenta.ts`, puro, testado com carga hostil em JWT, JSON em texto, URL, cor e imagem; tirar `visivel` ou `escAtributo` derruba os testes. O `href` da URL é o normalizado pelo `URL` (ASCII, domínio internacional em punycode, bidi e invisíveis percentualmente codificados): o que se vê é o que se abre. O selo de link mostra o destino (host, e o endereço no `title`); na prévia cortada, o host só aparece se terminou dentro dela; só o botão abre, nunca o clique na linha, pelo comando `abrir_link` do Rust, que de novo recusa o que não for http(s) em ASCII e manda ao navegador do sistema (`open`, `xdg-open`, `rundll32`); a webview nunca navega. JWT: só cabeçalho e carga, a assinatura é descartada na detecção e nunca é conferida nem enviada. Imagem: só data URI base64 do próprio arquivo, `src` posto pela propriedade; a CSP ganhou `img-src 'self' data:` e nada de rede.

**Acessibilidade:** amostra de cor com `role="img"` e rótulo; painel com `aria-live="polite"`; blocos com `tabindex="0"` e rótulo; o link do painel é botão de verdade (Tab e Enter). O selo de link na árvore fica fora da ordem do Tab (`tabindex="-1"`) para não quebrar a navegação por setas. A árvore observa a altura do painel (`ResizeObserver`) e mantém a seleção à vista, porque a imagem só ganha tamanho depois de carregar.

**Limites:** a Árvore não expande o JSON dentro de texto em nós (só o painel mostra reindentado). Base64 cru de WebP, BMP e AVIF só com `data:`. Fuso da data relativa é o da máquina.

**Conferência:** `app/tests/valores.test.ts` (12 testes, incluindo homógrafo, bidi, tetos, datas inexistentes e tempo perto de 1 MiB), `app/tests/apresenta.test.ts` (HTML com carga hostil), `link_so_http_em_ascii` no Rust, o registro "valores inteligentes" da vitrine testado em `exemplos.rs`, e a tela vista no navegador do app por uma ponte HTTP de teste para os comandos do Rust (a janela nativa do Tauri não foi aberta).

## Diff estrutural (Fase 3, 23/11/2026)

**Escolha:** `nucleo::diff` compara duas inferências já prontas (`estrutura::inferir` em cada arquivo, sem leitor novo) andando as duas árvores de `No` juntas. Cada mudança traz caminho (mesmo formato do `inferir`), regra, classe e evidência "N de M" antes e depois. Comparar a árvore, e não a lista achatada de caminhos, deixa cortar o ruído: quando um campo some ou o pai deixa de ser objeto, os filhos dele não viram mudanças à parte.

**Taxonomia** (regras do oasdiff/buf; quebra = um consumidor **ou** um validador escrito para o arquivo de antes pode falhar com o de depois; é um gate de CI, então na dúvida é quebra). Com essa definição, só campo novo opcional é compatível: o consumidor de antes não o lê e o validador de antes, sem o campo na lista de obrigatórios, aceita. Tipo que muda em qualquer direção quebra (alargar inclusive: quem lia `.a` como inteiro passa a receber `null` ou `1.5`), presença também (campo que virou opcional falta para quem o lia sempre) e variante nova também (um `switch` exaustivo ou um `enum` no discriminador do validador de antes não a conhece).

| Regra | Classe | Evidência |
|---|---|---|
| campo removido | quebra | presença antes → 0 |
| campo novo obrigatório (em todo objeto-pai) | quebra | 0 → presença |
| virou obrigatório (antes faltava em algum objeto, agora em todos) | quebra | presença de objetos-pai |
| tipo estreitado (os tipos de depois são subconjunto estrito) | quebra | valores dos tipos que mudaram, de todos os valores do caminho |
| tipo trocado (perdeu um tipo e ganhou outro, ex.: inteiro → texto) | quebra | idem |
| tipo alargado (os tipos de depois contêm os de antes; `null` novo também) | quebra | idem |
| virou opcional | quebra | presença de objetos-pai |
| variante removida (valor do discriminador que sumiu) | quebra | registros da variante, de registros-objeto |
| variante nova | quebra | como em variante removida |
| campo novo opcional | compatível | 0 → presença |

As regras continuam separadas (alargado × estreitado × trocado, virou obrigatório × opcional) para a pessoa saber a direção; a classe é que é a mesma. Decimal aceita inteiro (como `number` no JSON Schema): inteiro → decimal é alargado, decimal → inteiro é estreitado, e `{inteiro, decimal}` → `{decimal}` não é mudança. Primeira versão (PR #39) dava alargar, virar opcional e variante nova como compatíveis, pela ótica só do validador; o verificador reprovou: um gate de CI lê a união das duas óticas.

**Variantes:** valem para campos que são discriminador (critério da inferência) em pelo menos um lado. No outro lado, o campo conta se também for discriminador ou se tiver um valor só, texto, em todo registro-objeto (o caso de um arquivo com uma variante só).

**CLI:** `jasao diff antes depois [--json] [--falha-em quebra|qualquer]`. Uma linha por mudança (`quebra      .a.b  campo removido  (3 de 5 → 0 de 4)`), resumo no stderr; `--json` dá `{"quebras","compativeis","invalidas","mudancas":[{caminho, regra, classe, antes:{n,de}, depois:{n,de}, tipos?|variante?}]}`. Sai com 1 se `--falha-em` pegar alguma mudança da classe pedida, 2 em uso errado e 3 quando não deu para comparar: arquivo que não abre (nos outros comandos erro de execução sai com 1, mas aqui 1 é "quebra" e o gate não pode confundir) ou registro inválido em qualquer lado com `--falha-em` (as linhas ignoradas podem esconder uma quebra; falso verde não). Sem a flag, os inválidos viram uma linha `aviso:` no fim do stdout e `"invalidas":{"antes","depois"}` no `--json`. A saída passa pelo mesmo `Terminal` do `inferir` (chave hostil não manda controle ao terminal).

**Limites conhecidos:**
- Lista ou mapa vazio de um lado: `[]`/`.*` que só existem do outro lado não são comparados (lista que ficou vazia não é campo removido).
- Objeto que vira mapa (ou o contrário) entre os arquivos: os campos de um lado aparecem como removidos ou novos, sem uma regra "virou mapa".
- Discriminador que deixou de sê-lo com vários valores (ex.: as variantes pararam de se separar por chave) não é comparado por variante; os campos continuam comparados um a um.
- Só a união discriminada do topo; chaves obrigatórias por variante não são comparadas (os campos são, no caminho geral).
- Formato, faixa de valores e enum não entram: não são inferidos ainda.
- Custo: duas inferências completas (a comparação é linear no número de caminhos).

**Testado:** dourados por regra da taxonomia (a linha exata de cada uma, em `core/src/diff.rs`), aninhado/elemento/mapa e a regra do decimal; guardas de `valores()` (valor único mas ausente em algum registro; texto único com `null` ao lado) e da regra de presença (não vale em `[]`), cada uma com um caso que cai se ela sair; proptest de que A contra A não tem mudanças e de que acrescentar registros a A só gera regras de alargar; CLI com chave hostil no terminal, `--json` e os três modos de `--falha-em`; teste de integração pelo binário (`cli/tests/diff.rs`) com os códigos 0, 1, 2 e 3; alvo de fuzz `diff` (A contra A vazio, `n ≤ de` e B→A espelho de A→B). O fuzz curto passou de 25 para 23 s por alvo para o alvo novo caber no tempo do job (14 × 23 s ≈ 13 × 25 s).

## Diff na interface e diff de valores (Fase 3, 24/11/2026)

**Interface do diff estrutural.** Visão Diff (⌥⌘6) compara o doc da aba ativa com outro aberto, escolhidos em dois `select`. O comando `diff_estrutura` usa as duas inferências já prontas no Rust (`None` enquanto alguma corre; a vista refaz ao chegar `inferiu` de qualquer um dos dois) e devolve as mudanças do núcleo com `regra` e `classe` já decididas, mais `diff::iguais` (caminhos dos dois lados sem mudança). A interface só agrupa pelo campo `classe` (quebra, compatível, igual), na ordem do núcleo; nada de taxonomia em TypeScript. Cada item mostra caminho, regra traduzida, tipos ou variante e a evidência "N de M" antes → depois.

**Diff de valores.** `nucleo::valores` compara registro a registro **por posição** (o 1º com o 1º): cada registro é validado e andado em árvore, objeto por chave (chave duplicada: vale a primeira, como no resto do app), lista por índice `[i]`, folha pelo texto cru. Fiel ao byte: `1` ≠ `1.0` e um escape ≠ o caractere literal; atalho de igualdade é bytes iguais depois do `trim`. Resultado por caminho: adicionado, removido, alterado, com prévia de até `arvore::MAX_PREVIA` bytes. Registro que sobra de um lado entra como `.` adicionado/removido; registro inválido de qualquer lado conta em `invalidos` e não é comparado. Também compara um par escolhido (`registros`: "registro 3 de antes × 7 de depois").

**Memória.** Uma thread por arquivo lê do `mmap` e manda lotes de 1024 fatias (ponteiros, não cópias) por um `sync_channel(4)`: no máximo ~6 lotes vivos por lado. Isso limita o *fluxo*, não o *registro*: comparar um par valida e anda os dois registros inteiros, e isso custa ~2× o tamanho do registro por lado (medido na revisão: um JSON único de 83 MB, objeto no topo, que é um registro só, chegou a 365 MB de RSS). Por isso há teto por registro, `valores::MAX_BYTES_REGISTRO` = 16 MiB: par com algum lado acima dele (e bytes diferentes) não é comparado e conta em `grandes`; bytes iguais seguem `iguais` em qualquer tamanho. Medido com `/usr/bin/time -l`, release: par de 16 MiB (logo abaixo do teto) ≈ 68 MB de RSS; par de 87 MB (acima) ≈ 177 MB, que são as páginas do `mmap` tocadas pelo leitor, não análise. O que fica guardado também tem teto: os primeiros 500 registros diferentes (`MAX_REGISTROS`) e 50 diferenças por registro (`MAX_POR_REGISTRO`, aplicado no arquivo todo e no par escolhido), com totais contados até o fim; pior caso ≈ 500 × 50 × ~0,5 KB ≈ 12 MB. Cancelável (Esc, trocar de vista, fechar uma das abas) e com andamento "N de M linhas". Comparar registro gigante em fluxo, sem materializar listas e campos, fica para quando aparecer o pedido.

**Alinhar por chave fica fora.** Inserir um registro no meio de um JSONL desloca tudo e o diff mostra cada registro seguinte como diferente. Alinhar por uma chave (`.id`) exigiria índice de um dos lados em memória (fura o teto em arquivo de 10 GB) ou ordenação externa. Quando aparecer o pedido, o caminho é índice chave → posição em disco.

**CLI:** `jasao diff --valores antes depois [--json] [--falha-em qualquer]`. Texto: `registro N (linha a → b)` e uma linha recuada por diferença, resumo no stderr. Sai com 1 se algo difere com `--falha-em qualquer`, 3 com inválidos ou registros acima do teto (`grandes`, também avisado no stderr) e `--falha-em`; `--falha-em quebra` com `--valores` é uso errado (2). O tipo de cada arquivo vem de `estrutura::entrada_provavel` (olha só o começo); o app usa o da inferência.

**Testado:** dourados de `valores::par` por ação, teto de 50 diferenças nos dois caminhos (`arquivos` e `registros`), `trim` nas pontas, registro acima de 16 MiB em `grandes`, prévia cortada, arquivos por posição com sobras, JSON único por elemento, teto de 500 registros atravessando lotes, par escolhido, cancelamento; `diff::iguais`; CLI com `--valores`, saída neutralizada e `--falha-em`; na interface (`app/tests/diff.test.ts`), agrupamento pela `classe` recebida (uma regra "de compatível" marcada quebra fica em quebra), ordem e escape dos grupos, registro/linha base 0 → 1, "… e mais N", caminho hostil escapado (diferença e grupo igual), removido sem seta, só inválidos ou só grandes não dizem "iguais" e leitura dos campos de registro, cada teste derrubando uma mutação plausível. **Não testado:** a vista no app rodando (sem teste de interface; só build e tipos) e o diff de valores em arquivo de 1 GB com muitos registros (sem benchmark; RSS medido só nos pares de 16 e 87 MB acima).

## Medidas de desempenho, 24/11/2026

Metas da seção 3 do `plano.md`, medidas pelo CLI de release num JSONL de 1 GB do gerador (`gerar … 1024 1`: 1.073.741.850 bytes, 6.456.439 linhas), com o arquivo já no cache de páginas.

**Máquina:** MacBook com Apple M5 Pro, 18 núcleos, 48 GB, macOS 27.2.1. É mais forte que a de referência do plano (8 núcleos), então estes números não valem para ela. **Versão:** `aeb17f3` (main) + esta fatia (bench, script e docs; nenhuma mudança em `core/src` nem `cli/src`), Rust 1.99.1. **Comando:** `scripts/medir.sh` (mediana de 5 rodadas; memória é o maior *peak memory footprint* do `/usr/bin/time -l`, que não conta as páginas limpas do `mmap`). **Carga:** load average de 25 (1 min) antes de medir, com outras sessões ativas na mesma máquina.

| Medida | Meta | Medido | Pico alocado | RSS máximo (com `mmap`) |
|---|---|---|---|---|
| Primeira tela (`janela arquivo 0 50`) | < 300 ms | 0,26 s | 51 MB | 1076 MB |
| Índice de linhas (`linhas`) | < 2 s | 0,23 s | 51 MB | 1076 MB |
| Estrutura completa (`inferir`) | < 5 s | 2,11 s | 62 MB | 1087 MB |
| Busca até o fim, literal ausente | < 3 s | 0,28 s | 52 MB | 1077 MB |
| Busca até o fim, regex ausente | < 3 s | 0,27 s | 53 MB | 1078 MB |
| Exportar Schema + validar o arquivo | sem meta | 3,52 s | 65 MB | 1092 MB |

**Memória além do `mmap`:** a meta de < 300 MB passou com folga em todas as linhas (no máximo 65 MB). O script confere cada linha e sai com erro se alguma meta não passar.

**Primeira tela no limite:** em 23/11, sobre `cf7cc7f` e com load average entre 187 e 211, deu 0,28 s numa rodada e 0,34 s em outra, e esta última não passou. O motivo é estrutural: `ArquivoLinhas::abrir_com` constrói o índice de linhas inteiro antes da primeira janela, e o app abre pelo mesmo caminho. Assim, a primeira tela custa o índice de 1 GB (0,23 a 0,29 s) mais a janela, e a folga até 300 ms some com carga na máquina. A correção seria mostrar a primeira janela antes do índice completo (índice parcial ou progressivo); não foi feita nesta fatia. A medida é do processo CLI, sem a renderização do webview: no app, o número real é maior.

**Não medidos aqui:** estrutura parcial em < 500 ms, primeiro resultado da busca em < 200 ms (o bench `busca` mede, mas não entrou na tabela), rolagem e expandir em 60 fps (bench `arvore`, em 64 MB) e o teto de 10 GB.

**Benchmarks:** `cargo bench -p nucleo` cobre índice (`indice`), estrutura (`estrutura`), busca até o primeiro achado e até o fim (`busca`), árvore (`arvore`) e, desde esta fatia, exportar Schema/Zod e validar (`exportar`). O tamanho vem de `NUCLEO_BENCH_MB`. Os arquivos ficam em memória ou em `target/medir/`, fora do git.

**Regressão de 10% no CI: não ligada.** O CI roda no mesmo runner, nesta mesma máquina compartilhada, e a carga muda os tempos bem além de 10%. Primeira tela: 0,34 s contra 0,28 s com a mesma carga. Estrutura: 3,41 s com load ~190 contra 2,11 s com load ~25. Um limite de 10% reprovaria por ruído. Além disso, os benches de 1 GB levam minutos e não cabem no tempo do runner único.

**Dadroit:** não está instalado nesta máquina, então a comparação de abertura do mesmo arquivo fica pendente.

## Modo Tabela (Fase 2, 23/11/2026)

**O que é:** JSONL de objetos, ou JSON único que é lista de objetos, vira grade: uma linha por registro, uma coluna por campo do 1º nível. Colunas vêm da inferência (`estrutura::inferir`), na ordem em que aparecem pela primeira vez, até 200 (`MAX_COLUNAS`); as demais são contadas e avisadas na barra. Ordenar, fixar e ocultar colunas; ⌥⌘4 e cartão no painel.

**Nada inteiro no front:** `tabela_linhas(id, inicio, quantidade, colunas)` devolve só a janela visível e só as colunas visíveis; o bloco encolhe com muitas colunas (30 mil células por pedido, 60 a 300 linhas). Sem virtualização horizontal: com 200 colunas o DOM tem ~30 linhas × 200 células.

**Fiel ao byte:** a célula mostra o texto cru do arquivo (número grande e string com escapes como estão; strings com aspas, igual à prévia da Árvore). Objeto e lista mostram `{n}`/`[n]` e abrem a Árvore no valor (clique, Enter ou duplo clique). Ausente (`—`, apagado) é diferente de `null`. Linha inválida ocupa a linha inteira com o byte e o motivo do erro e o texto cru, sem quebrar a grade. Todo texto do arquivo (nome de coluna, célula, linha inválida) passa por `visivel`, o mesmo neutralizador de bidi/invisíveis do Texto e da Árvore, e fica em `unicode-bidi: isolate`.

**Ordenação, decisão:** ordenação completa no Rust, em 2º plano, com progresso e cancelar; sem amostragem escondida. `ordenar` extrai em paralelo (rayon, blocos de 65.536 registros) uma chave de 24 bytes por registro (classe, número e posição dos bytes crus no arquivo, sem copiar texto) e ordena só a permutação (`u32` por registro; com o buffer do `par_sort_by`, ~30 B por registro no pico, estimado pelos tamanhos, não medido). A ordem fica guardada no documento; trocar de direção na mesma coluna é instantâneo (inverte só os presentes) e voltar à ordem do arquivo também. O evento `ordenacao` (`{id, geracao, lidos, total, fim, falha}`) leva o progresso; a interface ignora geração velha.
- Crescente por classe: número (pelo valor decimal exato: o `f64` decide quando difere, que o arredondamento é monótono; no empate do `f64`, acima de 2^53 ou fora do alcance, compara os dígitos; empate de valor pelo texto cru), texto (bytes), booleano, objeto, lista, `null`. Ausente e linha inválida vão sempre para o fim, na ordem do arquivo, nas duas direções.
- Teto: **50 milhões de registros** (`MAX_REGISTROS`, ~1,5 GB no pico da ordenação). Acima disso `ordenar` recusa antes de alocar (`grande`) e a barra diz o teto; a grade continua na ordem do arquivo. O mesmo teto vale para listar os elementos de um JSON único (16 B por elemento, 800 MB): acima dele o arquivo fica indisponível para Tabela com o motivo `grande`. Um JSONL de 10 GB com linhas de 30 B (~330 M registros) abre na Tabela mas não ordena.
- Cancelar: a extração para no próximo bloco; o `sort` final não para no meio, mas o resultado é descartado (o núcleo confere o `cancelar` depois do `sort` e `cancelar_ordenacao` sobe a geração, então uma ordem pronta de geração velha nunca é publicada). O evento de fim chega com `falha: cancelada` e a ordem anterior continua.
- Descartadas: ordenar só a janela (mente sobre o arquivo); amostra com aviso (o pedido foi sem amostragem escondida, e a ordenação completa coube no tempo medido abaixo).

**Indisponível, com motivo:** arquivo vazio, JSON único que não é lista, registros que não são objetos (lista mista, JSONL de escalares; o número vai no motivo), objetos que são mapas (chaves variam), só objetos vazios, JSON único inválido, JSON único com mais de 50 milhões de elementos. O cartão do painel fica desligado com o motivo; o modo, se aberto, mostra o mesmo texto.

**Teclado e ARIA:** `role="grid"` com `aria-rowcount`/`aria-colcount`, `aria-rowindex`/`aria-colindex` e `aria-activedescendant` na célula; cabeçalho com `aria-sort`. Setas movem a célula; Home/End vão à 1ª/última coluna, ⌘Home ao cabeçalho e ⌘End à última linha (padrão de grid do WAI); PageUp/PageDown. No cabeçalho (seta para cima da 1ª linha): Enter ordena (crescente → decrescente → arquivo), F fixa/solta, Delete oculta. Colunas ocultas viram botões na barra para voltar. A última coluna visível não se oculta.

**Limites conhecidos:**
- JSON em várias linhas (formatado): o registro é a linha, então a tabela não se aplica (`json_invalido`/`nao_objetos`); abrir na Árvore a partir da célula cai no início da linha.
- A régua não aparece no modo Tabela (o registro selecionado não é posição no arquivo depois de ordenar).
- Fixar/ocultar/ordem valem por documento na sessão; não são guardados.


## Avisos da estrutura (Fase 2, 24/11/2026)

**Onde rodam:** na própria inferência (`estrutura::inferir`, 2º plano), com flags baratas por texto que param de ser conferidas na primeira vez que viram verdade. Nada na 1ª tela. Cada caminho leva `avisos` (`numero_como_texto`, `data_fora_iso`, `valor_unico`, `opaco`, `segredo`); a Estrutura mostra uma etiqueta por aviso e o nome entra no rótulo do leitor de tela.

**Critérios, decididos para não gritar à toa:**
- **Número como texto:** todo texto visto no caminho é um número JSON válido **e** pelo menos um deles não é código. **Código** = só dígitos, 8 ou mais: CEP sem traço (`20040020`), telefone (`11987654321`), CPF, CNPJ, cartão. Esses são identificadores guardados certo como texto, então não avisam. CEP com zero à esquerda (`01310100`) nem é número JSON. Com traço, parênteses ou espaço (`01310-100`, `(11) 98765-4321`) é texto. Um único texto não numérico no caminho cala o aviso.
- **Data fora do ISO:** o texto inteiro é uma data em grupos separados por `/`, `-` ou `.`, com dia e mês nas faixas válidas, e hora opcional (`HH:MM`). Avisam `24/11/2026`, `11-24-26`, `24.11.2026`, `2026/11/24`. Não avisam o ISO (`2026-11-24`, com ou sem hora), versão (`1.2.30`, `10.11.12`, porque ano de 2 dígitos com ponto fica de fora), data dentro de frase, telefone com traço e mês 13.
- **Valor único:** pelo menos 2 valores escalares não nulos, todos o mesmo valor, e nenhum objeto ou lista no caminho. Um valor só, ou só `null`, não avisa.
- **Opaco:** texto que é JSON (`{"…}` ou `[{"…]`/`["…]`), `data:…;base64,` ou base64 com 100 ou mais caracteres. Hex puro (hash SHA) e texto curto ficam de fora.
- **Parece segredo:** pelo **valor** (JWT `eyJ…` com 2 pontos, `-----BEGIN … PRIVATE KEY`, chave AWS `AKIA` + 16, prefixos `ghp_`/`gho_`/`ghs_`/`github_pat_`/`xoxb-`/`xoxp-`/`xoxa-`/`sk-`/`sk_live_`/`rk_live_` com 20 ou mais caracteres sem espaço, `Bearer `/`Basic ` + credencial sem espaço) ou pelo **nome** de qualquer chave acima, se o caminho tem valor escalar (texto, número ou booleano): `.senha.valor`, `.credentials.user` e `.pin` (número) avisam. O nome é quebrado em palavras (`_`, `-`, `.`, espaço, camelCase), e a **última** palavra decide: `senha`, `password`, `passwd`, `pwd`, `pass`, `passcode`, `passphrase`, `pin`, `secret`, `segredo`, `token`, `apikey`, `credential`, `credencial`, `authorization`, `privatekey`, os plurais (menos `tokens`: em `max_tokens` é contagem), ou o par `api/private/secret/access` + `key(s)` e `chave(s)` + `privada(s)/api`. Assim `token_type`, `password_hash`, `secretaria` e `max_tokens` não avisam. Falso positivo aqui é o lado seguro: o perfil só esconde o que tem o aviso.

**Segredo só aponta:** o aviso leva o caminho, nunca o valor. O perfil de um caminho com esse aviso sai `oculto`: só contagens, sem mínimo, máximo nem mais comuns, e o valor não é guardado nem cortado (o teste confere que não aparece no `Debug` nem no JSON). A interface não registra valor em log. Mascarar o segredo na Árvore e no Texto é da Fase 5 (feito: "Máscara de segredos").

## Estatísticas por campo e registros que fogem (Fase 2, 24/11/2026)

**Sob pedido, não na inferência:** `perfil::perfil` varre o arquivo inteiro para **um** caminho (o selecionado na Estrutura), em paralelo por blocos como a busca. O comando `perfil(id, indice)` roda fora da thread da interface, e pedir outro caminho cancela o anterior. A interface espera 200 ms parada na linha antes de pedir. Fazer isso para todos os caminhos dentro da inferência multiplicaria memória e tempo pelo número de caminhos.

**O que sai, e o que é aproximado:**
- Mínimo e máximo exatos. Número pelo valor (`f64`), devolvendo o texto cru do arquivo; texto pela ordem dos bytes.
- Distribuição dos números por ordem de grandeza (`1e2…1e3`, sinal à parte, zero sozinho), exata e com memória constante.
- **Cardinalidade:** exata até 4.096 valores distintos. Acima disso, estimativa KMV (os 4.096 menores hashes), com erro típico de ~1,6%, e a interface escreve "aproximado". Texto `"1"` e número `1` contam separados.
- **Mais comuns (10):** contagem exata até 1.000 valores distintos. Acima disso, Misra–Gries em lote (cada bloco segura até 2.000 e reduz para 1.000): as contagens viram limite inferior, quem aparece em mais de 1/1.001 dos valores nunca some, e a interface escreve "aproximado". Mínimo, máximo e mais comuns guardam no máximo 1 KiB + 1 byte por valor e saem cortados com `…` (cortar não muda a ordem, então o mínimo e o máximo seguem certos; o número é comparado pelo valor inteiro). Até 2.000 distintos sem corte nenhum a contagem dos comuns é exata e não sai "aproximado".
- Memória por bloco: no máximo 4.096 hashes, 2.000 valores de até 1 KiB e 2 × 1.000 linhas, independente do tamanho do arquivo.

**Registros que fogem:** o perfil conta os registros com pelo menos um valor no caminho e os sem nenhum, e guarda as primeiras 1.000 linhas de cada grupo, em ordem; as contagens seguem até o fim. A lista sai no painel como botões, e o clique abre o registro na Árvore (o mesmo `focarRegistro` do Enter na Estrutura). Linha em branco e linha inválida não entram em nenhum dos dois grupos. Filtrar a Tabela por esses registros fica para a fatia do filtro por variante.

**Medido (24/11/2026, JSONL de 1 GB de `examples/gerar`, M5 Pro, load ~90):** perfil de um caminho entre 0,30 s e 1,0 s. O mais lento foi `.id`, com 6,46 milhões de valores distintos: a estimativa deu 6.469.431, erro de 0,2%. A inferência com os avisos levou 2,01 s no `medir.sh`, contra a meta de 5 s. A 1ª tela levou 0,21 s, contra a meta de 300 ms, e não mudou porque os avisos não estão nesse caminho.

## Laudo ao abrir (24/11/2026)

**De onde sai:** `nucleo::laudo::Laudo::de` lê só o que a inferência já calculou (`Resumo::caminhos`, `discriminadores`, inválidas e vazias). Custo proporcional ao número de caminhos, sem reler o arquivo. No app ele é montado na mesma thread de 2º plano, logo depois de `estrutura::inferir`, e chega no mesmo aviso que a Estrutura (`inferiu`). A 1ª tela não muda: o laudo não está no caminho dela. O comando `laudo(id)` devolve `null` até lá. Reabrir o documento zera o laudo antes de a nova inferência começar.

**O que entra, com teto fixo** (a memória não cresce com o arquivo):
- **O que é:** tipo de conteúdo, tamanho e registros.
- **Forma:** as 3 variantes com mais registros do 1º discriminador, com a 1ª linha de cada uma e quantas ficaram de fora. Depois, até 5 campos do topo que faltam em algum registro, na ordem do arquivo. "Do topo" é `relacao == campo` com um segmento só, porque a profundidade engana com chave como `.["x y"]`.
- **Atenção:** linhas inválidas com a 1ª, linhas vazias e até 8 avisos do #43, em ordem de prioridade (segredo, opaco, número como texto, data fora do ISO, valor único) e, no empate, na ordem do arquivo. O resto vira "e mais N". Segredo só leva o caminho, e o teste confere que o valor não aparece no Markdown, no `Debug` nem no JSON.
- Texto vindo do arquivo (caminho, valor de variante) é cortado em 120 caracteres com `…`.

**Passos clicáveis no painel:** cada variante abre o 1º registro dela na Árvore. Cada opcional e cada aviso abre a Estrutura naquele caminho, onde o perfil mostra os registros com e sem ele. Os outros passos só aparecem quando valem: "Abrir na Tabela" quando a Tabela serve, "Exportar schema" quando há registros e "Comparar com outro arquivo aberto" quando há outra aba com arquivo. Esse último abre o Diff do #42 e não traz lógica de diff nova. O HTML passa caminho e valor por `visivel`.

**CLI:** `jasao laudo arquivo [--json] [--teto SEG]` imprime o laudo em Markdown (ou JSON), pelo mesmo `Terminal` que neutraliza controles e bidi. **Teto de tempo:** padrão de 30 s. Ao estourar, a inferência é cancelada e o laudo sai parcial ("X de Y linhas lidas (o teto de tempo parou a leitura)"), com os números só das linhas lidas. O app não usa teto: a inferência já é cancelável, e fechar ou reabrir a aba a cancela. O nome do arquivo nos próximos passos sai entre aspas simples de shell quando não é só ASCII simples.

**Segredo nunca vira discriminador:** `Resumo::discriminadores` pula o campo com o mesmo critério do aviso `Segredo` (valor com cara de chave ou nome como `token`/`senha`). Senão os valores sairiam como variantes no laudo, na legenda da régua, na Estrutura, no Zod do exportar e no diff. A correção fica na função compartilhada, não em cada saída. Testes com 2 valores distintos no core (estrutura e laudo), na CLI (Markdown e `--json`), no IPC (resumo, laudo e estrutura) e no painel.

**Sem registro lido, sem afirmação:** com 0 registros (arquivo vazio, ou o teto parou antes do 1º), o Markdown diz "sem base" em vez de "um formato só", "nenhum campo falta" ou "nenhum aviso". O painel já esconde esses itens com `registros == 0`.

**Códigos de saída do `laudo`:** 0 quando o laudo sai, mesmo parcial, com linhas inválidas ou binário (o texto diz). É relatório, não gate: para gate há `validar` e `diff --falha-em`. 1 quando o arquivo não abre, 2 em uso errado. Teste pelo binário em `cli/tests/laudo.rs`.

**Limite conhecido (fatia futura):** o teto de tempo só é olhado entre registros, dentro de `estrutura::inferir`. Um registro gigante não é cortado: uma linha de 286 MB com `--teto 0.2` levou 1,64 s. Para cortar, o parser da inferência precisa checar o `parar` dentro do registro. Fica para uma fatia de `estrutura`, junto com o mesmo efeito no cancelamento do app.

## Tipos TypeScript e validar contra schema de fora (Fase 4, 24/11/2026)

**Tipos TS:** `exportar::tipos` imprime a mesma `Forma` do Zod e do Schema como `export type Registro = …`. União discriminada vira união de objetos com o discriminador literal (`| { tipo: "evento"; … }`); campo opcional, `campo?:`; nullable, `| null`; mapa, `Record<string, T>`; lista, `Array<T>`; objeto sem campo visto, `Record<string, unknown>`. Chaves e literais passam pelo mesmo `exportar::aspas`; identificador ASCII sai sem aspas e `__proto__` sai como `["__proto__"]`. **"Tratar como número" não se aplica aos tipos:** o tipo descreve o JSON como está, e tipo não converte; texto numérico continua `string` (a interface desliga a opção). A autovalidação usa `schema_da_entrada_zod`, que aceita exatamente o que o tipo descreve. Prova: dourados `app/tests/dourados/*.d.ts` e `app/tests/tipos.test.ts`, que roda o `tsc` do projeto em modo estrito: todo registro do próprio arquivo precisa caber no tipo, e casos com `@ts-expect-error` (discriminador errado, campo obrigatório faltando, tipo errado, valor travado, texto numérico) precisam ser recusados. Trocar `obs?` por `obs`, o literal do discriminador por `string`, `valor: string` por `number` ou tirar o `| null` reprova o teste.

**Validar contra a estrutura ou contra um schema de fora.** "Estrutura salva" é um JSON Schema: o que `exportar --schema` grava (`--schema`) ou a estrutura inferida de um arquivo de referência (`--como`). CLI: `jasao validar <arquivo> --schema <schema.json> | --como <referência> [--literal]`. Saída: uma linha por falha (`linha N: caminho: motivo`, as 20 primeiras, neutralizadas no terminal) e o resumo no stderr. **Códigos de saída para CI:** 0 tudo passa; 1 algum registro falha ou alguma linha não é JSON; 2 uso errado; 3 não deu para validar (schema recusado, arquivo que não abre). Na interface (vista Exportar): "Validar outro arquivo…" valida um arquivo escolhido contra a estrutura do documento com as opções atuais, e "Validar com schema de fora…" valida o documento contra um schema escolhido. Os dois diálogos são do backend (como `escolher`), a webview não manda caminho; o resultado volta pelo mesmo evento `validacao` com geração, cancelar e Esc. Schema recusado aparece no status como erro.

**Schema de fora é entrada não confiável.** `validar::compilar` é o único caminho para compilar schema (o gerado também passa por ele):
- **Tamanho:** até 1 MiB (`MAX_SCHEMA`); `ler_schema` não lê mais que 1 MiB + 1 byte do disco.
- **Profundidade:** até 64 níveis de objeto/lista (`MAX_PROFUNDIDADE`), conferida numa varredura própria antes do `jsonschema`. O `serde_json` já para em 128.
- **`$ref` só local:** `$ref`, `$dynamicRef` e `$recursiveRef` precisam começar com `#`. URL (`https:`, `http:`, `file:`, `//`) e caminho relativo são recusados com mensagem, e além disso o `jsonschema` roda em modo `offline()`. Nada é buscado na rede nem no disco. Um `$schema` desconhecido também é recusado (não é buscado). Nome de campo `$ref` dentro de `properties`/`$defs` e afins, e `$ref` dentro de `const`/`enum`/`default`/`examples`, não contam (são dados, não referências).
- **Custo limitado na compilação** (corrige a 1ª versão do #45, que só contava `$ref` e aceitava uma bomba de fan-out: 40 níveis de `allOf` com dois `$ref` ao nível seguinte, 81 `$ref`, travavam `jasao validar` por mais de 30 s em release). `compilar` expande o schema seguindo cada `$ref` como se o alvo estivesse copiado no lugar, só pelas palavras que aplicam schema, e recusa:
  - **schema expandido com mais de 100 mil valores** (`MAX_EXPANDIDO`). A bomba passa disso a partir de ~15 níveis; com 40 é recusada em 0,26 s pelo CLI de release (saída 3).
  - **mais de 1024 objetos/listas encadeados** (`MAX_ENCADEADO`: aninhamento e `$ref` seguidos). Uma cadeia de `$ref` estoura a pilha de 2 MiB das threads do rayon: em debug, 2000 passam e 3000 estouram; release aguenta 4000. Margem de 2×. A própria expansão roda numa thread de 32 MiB.
  - **recursão que não anda no dado** (`{"$ref":"#"}`, a↔b, `allOf` voltando para o mesmo nó). Antes eram aceitas; agora são recusadas.
  - **recursão com duas voltas em que uma é começo da outra** no caminho do dado (`properties.x` com dois `$ref` ao topo; voltas `[x]` e `[x, x]`). Aí o mesmo dado é alcançado de vários jeitos e o custo cresce exponencialmente com a profundidade do registro. Voltas livres de prefixo passam: árvore (`properties.a` → `#`), "qualquer valor JSON" (objeto e lista voltando ao topo) e o mesmo `$def` recursivo usado em dois campos.
  - **`$ref` para âncora** (`#nome`) e **`$id` abaixo da raiz**, porque a expansão resolve todo `$ref` como ponteiro a partir da raiz. Ponteiro com `~0`, `~1` e `%XX` é aceito.
- **A garantia, de fato:** schema aceito não tem custo exponencial. Cada nó do dado recebe no máximo da ordem de 100 mil aplicações de subschema, então validar um registro custa até da ordem de 100 mil × o tamanho do registro. É linear, mas a constante não é pequena: um schema perto do teto contra registros grandes ainda é lento, e o cancelamento só é conferido entre registros. A regra de recursão é conservadora. Recusa schemas legítimos como o meta-schema do JSON Schema e `items` junto de `additionalItems` voltando ao mesmo nó; isso pode ser afrouxado se aparecer caso real.
- **`pattern` no motor `regex`** (`PatternOptions::regex()`): tempo linear, sem retrocesso. Lookaround e backreference não compilam (schema recusado); `^(a+)+$` contra 5.000 `a` + `b` termina (o teste exige menos de 5 s).
- **Atalho conhecido:** `$ref` relativo a um `$id` embutido (`{"$id":"https://x/a","$ref":"b"}`) é recusado mesmo sendo resolvível sem rede. Só vale aceitar se aparecer schema real que precise disso.

**Sem dependência nova:** tudo com `jsonschema` 0.63 (já no projeto, sem features padrão), `serde_json` e o `tsc` que já é devDependency do `app`.

**Provas:** `core/tests/exportar.rs` (ref remoto em https/http/file/relativo/`//` e dentro de `anyOf`; recursão sem avançar recusada e árvore/"qualquer JSON"/`$def` reusado aceitos; bomba de fan-out com 15, 40 e 300 níveis recusada em menos de 2 s e com 8 validando; fan-out na recursão (direto, por `$def` alcançado duas vezes, Fibonacci, `items`+`additionalItems`) recusado; fundo e grande demais; cadeia de 20 mil `$ref` recusada e no limite validando numa thread de 2 MiB. Cada regra derruba uma mutação: sem teto expandido, sem recusa de volta vazia, sem fechar voltas ao sair do alvo, campos diferentes cruzando, tamanhos diferentes cruzando; regex sem retrocesso; meta-schema desconhecido); testes da CLI (`opcoes_validar`, falha listada e neutralizada, `--como`, `--literal`, ref remoto recusado); `comandos_pelo_ipc` no app (formato `ts`, validar outro arquivo, schema de fora que reprova e schema hostil recusado).

**Não conferido:** os dois botões novos da vista Exportar não foram abertos no app (o comando e o evento foram testados pelo IPC, os diálogos não); `jasao validar` em arquivo de 1 GB não foi medido.

## Arquivo vivo (Fase 5, 24/11/2026)

**Escolha:** seguir um JSONL que cresce (como `tail -f`) sem mapear o original. `nucleo::Vivo` lê o arquivo do usuário só com `read` e copia as linhas completas para um arquivo do próprio app na pasta de cache (`create_new`, 0600, apagado do diretório logo depois de aberto no Unix). Árvore, índice e Tabela leem o mapa dessa cópia, que só cresce: truncar, rotacionar ou reescrever o original não derruba o processo.

**Por que a cópia:** `mmap` de um arquivo que encolhe dá SIGBUS ao tocar uma página que deixou de existir (achado 14), e não há como o processo se proteger depois de mapeado. Ler com `read` e mapear algo que só o app escreve fecha o problema. Custo: o arquivo seguido ocupa o dobro em disco enquanto a aba está aberta (a cópia some ao fechar). Teto: o mesmo `max_saida` da descompactação (10 GiB).

**Detectar reescrita:** a cada leitura, (1) identidade do arquivo, `(dev, ino)` no Unix, pega a rotação (`mv log log.1; novo log`); (2) os últimos 64 bytes já copiados ("rabo") são conferidos no original e pegam truncamento e reescrita no lugar, mesmo quando o arquivo novo é maior. Qualquer diferença vira `Mudanca::Reescrito`: o app reabre do zero e avisa no status. O 1º `seguir` confere o mesmo contra uma `Marca` (identidade, tamanho e últimos 64 bytes, lidos com `read`) tirada no `abrir`: se o arquivo foi trocado entre abrir e seguir, reabre em vez de acrescentar à árvore de antes. Pausar não solta a cópia: retomar confere contra a última leitura, então arquivo trocado durante a pausa também reabre. Arquivo que sumiu (no meio de uma rotação, apagado) vira erro, e a interface pausa com o motivo.

**Linha parcial:** tudo o que chega vai para a cópia, mas só o trecho até o último `\n` é mapeado e indexado; o resto entra quando a linha terminar. A parcial não fica na memória nem é relida a cada leitura, e o rabo a inclui (reescrita no meio dela é notada). O teto de 10 GiB vale para tudo o que foi lido, parcial inclusive: um JSON minificado de uma linha que cresce sem `\n` para no teto com erro.

**Incremental:** `IndiceLinhas::estender` indexa só os bytes novos; `Arvore::crescer` mantém os registros abertos e soma as linhas novas. A varredura em 2º plano (inválidas, inferência, Tabela, régua) é refeita a cada crescimento, pelo mesmo caminho da abertura (`instalar`).

**Interface:** botão Seguir/Pausar (`aria-pressed`) ao lado do status, só para arquivo do usuário aberto como está (não exemplo, não resultado de consulta). Polling a cada 1 s (`seguir`); pausar só para de chamar (leitura em voo na hora da pausa é aplicada na aba, que fica igual ao documento no Rust). Se a tela está no fim, rola até as linhas novas; senão acumula "+N novas", que leva ao fim ao clicar.

**Sem dependência nova:** polling em vez de `notify`. Uma leitura sem novidade custa um `stat` e 64 bytes; `notify` traria código de plataforma (FSEvents, inotify, ReadDirectoryChangesW) para ganhar menos de 1 s de latência.

**Achado 14: resolvido só no modo seguir.** Abrir sem seguir continua mapeando o original (é o que dá a 1ª tela em 0,28 s num arquivo de 1 GB); copiar sempre custaria o arquivo inteiro antes da 1ª tela. Se o arquivo encolher por baixo de uma aba que não está seguindo, o app ainda pode cair.

**Testado (núcleo, `vivo::testes`):** `crescimento`, `linha_parcial`, `truncamento_sem_sigbus` (2.000 linhas, trunca a 0, lê todos os bytes do mapa antigo; depois reescrita no lugar maior), `rotacao` (Unix), `recusa_compactado`, `copia_nao_fica_no_disco`, `estender_igual_a_construir`, `teto`, `parcial_reescrita`, `continuar_confere_a_marca`; no app, `seguir_pelo_ipc` (concessão exigida, crescimento com linha parcial, truncamento reabre, troca durante a pausa reabre, troca entre abrir e o 1º seguir reabre). Mutações plausíveis, todas derrubadas: mapear o original em vez da cópia (`truncamento_sem_sigbus` morre de SIGBUS, prova do achado 14), tirar o cheque de identidade (`rotacao`), tirar o cheque do rabo (`truncamento_sem_sigbus`), copiar até o fim em vez de até o último `\n` (`linha_parcial`), esquecer o início do 1º bloco novo em `estender` (`estender_igual_a_construir`, `crescimento`); `Marca` sempre conferindo (`continuar_confere_a_marca`); sem teto (`teto`); rabo sem a linha parcial (`parcial_reescrita`); mapear a parcial ou contá-la como copiada (`linha_parcial`).

**Não testado:** Windows (a identidade só existe no Unix; lá só o rabo detecta reescrita, e rotação com prefixo igual passa); a interface à mão (botão, indicador, rolar ao fim); desempenho de seguir um arquivo grande que cresce rápido (cada crescimento refaz a varredura inteira e clona o índice); a janela curta, no 1º seguir, em que threads antigas ainda seguram o mapa do original; a janela entre conferir o rabo e copiar (reescrita exatamente nesse intervalo pode juntar bytes velhos e novos numa leitura; a seguinte confere o rabo de novo); a janela entre mapear no `abrir` e ler a `Marca` (troca nela faz o 1º seguir acrescentar ao arquivo trocado).

## Consulta jq ao vivo, filtros salvos e CSV (Fase 5, 24/11/2026)

**Linha de comando:** `jasao jq '<filtro>' arquivo [--saida jsonl|csv] [--max-resultados N] [--max-mb N] [--tempo-max S] [--max-memoria-mb N]` (`consultar` continua valendo). JSONL é o padrão. Passar de um teto de saída imprime o que coube, avisa no stderr e sai com código 1: resultado parcial nunca passa por completo em script. A consulta roda num processo filho (o próprio binário, com `JASAO_CLI_FILHO` no ambiente e o stdio herdado): o pai a mata no relógio e troca a morte por sinal (teto de memória, pilha estourada por `def f: 1 + f; f`) por mensagem e código 3, sem abortar.

**Tetos, cada um com teste que falha se for desligado:**

| Teto | Padrão | Ao estourar |
|---|---|---|
| Resultados no total (`com_teto_saida`) | sem teto; CLI `--max-resultados` | para tudo, `cortado` |
| Bytes de saída no total | app 1 GiB (`filho::TETO_SAIDA`); CLI `--max-mb`, CSV sem `--max-mb` 256 MiB | para tudo, `cortado` |
| Bytes por lote de um trabalhador | 8 MiB (`MAX_BYTES_LOTE_PADRAO`) | entrega o lote e segue: a memória de bloco adiantado fica limitada mesmo com resultados grandes |
| Relógio de parede do filho | app 300 s (`filho::TEMPO_MAX`); CLI `--tempo-max` + 2 s, ou 300 s sem ele | o pai mata o filho, vale antes do 1º resultado; falha "passou de N s" (app) ou código 3 (CLI) |
| Heap do filho (`nucleo::memoria`) | app 4 GiB (`filho::TETO_MEMORIA`); CLI 4096 MiB, `--max-memoria-mb` | o filho aborta; o pai reconhece a marca no stderr e devolve "passou do teto de memória (N MiB)" (app) ou código 3 (CLI) |

O teto de saída é imposto no receptor, na ordem do arquivo: o resultado parcial é sempre um prefixo do completo. Filtro que gira sem entregar nada (`last(range(1e12))`, `until(false; .)`) ou junta tudo numa expressão só (`[range(1e9)] | length`) não para por teto de saída: quem o para é o relógio e o teto de heap do filho. Os tetos por registro (resultados e tempo) saíram: só eram conferidos entre resultados e o produto não os ligava.

**Teto de memória:** alocador global que conta o heap vivo (`nucleo::memoria::Contador`, declarado no app e na CLI; só o filho liga o teto). Alocação que passaria do teto é recusada, o Rust aborta o filho e a 1ª recusa escreve uma marca direto no fd 2 (sem alocar e sem o lock do `stderr`, que a CLI segura durante a consulta). Não é `setrlimit`: no macOS `RLIMIT_AS` não vale e contaria o arquivo mapeado, que não é heap.

**Ao vivo:** digitar roda o filtro depois de 400 ms sem tecla; o Rust cancela a consulta anterior ao receber a nova (o filho dela é morto) e descarta o fim e o temporário de geração velha. Filtro que não compila (no meio da digitação) não cancela nem apaga o resultado mostrado: `consultar` compila antes de reiniciar. Enter roda na hora. Ao vivo, erro de sintaxe aparece na linha de estado mas o cursor não pula para o erro (só no Enter), e abrir o resultado não tira o foco do campo. O mesmo filtro com resultado aberto não roda de novo.

**Filtros salvos:** botão ★ ao lado do campo salva ou tira o filtro atual; a lista aparece como sugestão do campo (`<datalist>`, preenchida por `option.value`, nunca HTML). Fica no `localStorage` (chave `jasao.filtros`): até 50 filtros de até 16 KiB (o teto do Rust). O que vem de lá é lido como não confiável: JSON fora do formato vira lista vazia, item que não é texto ou passa do teto é descartado. Armazenamento bloqueado: a lista vale só na execução.

**Salvar resultado:** botões "Salvar JSONL…" e "Salvar CSV…" com resultado aberto. O IPC (`salvar_consulta`) só aceita o temporário atual do próprio documento: caminho vindo da interface que não seja esse é recusado antes do diálogo. JSONL é cópia do temporário. CSV são duas passadas no temporário (colunas, depois linhas), memória só dos nomes das colunas.

**Salvar não sobrescreve o que está aberto:** o destino escolhido no diálogo é recusado ("é um arquivo aberto no jasão") quando é, pelo caminho canônico ou pelo mesmo (dev, ino) no Unix, um arquivo concedido nesta execução (abertos, resultados temporários), um recente ou o próprio resultado. Pega link simbólico e hard link. A gravação vai para um temporário `.jasao-salvar-*` na pasta do destino e só no fim vira o destino por `rename`: falha no meio (colunas demais, disco cheio) não deixa o destino pela metade nem apaga o que havia lá.

**Regra do CSV** (`core/src/csv.rs`):
- Coluna = caminho jq da folha relativo ao resultado (`.a.b`, `.itens[0]`, `.["com espaço"]`); resultado escalar vai na coluna `.`. Objeto e lista vazios são folhas (`{}`, `[]`).
- Colunas na ordem da primeira aparição. Teto de 10.000 colunas e 256 níveis: passar é erro, sem arquivo parcial.
- Célula: `null` vazia, booleano como texto, número exato como veio (inteiro grande não perde dígito), caminho ausente vazio.
- **Injeção de fórmula:** texto e nome de coluna que começam com `=`, `+`, `-`, `@`, tab, CR ou as formas de largura total `＝ ＋ － ＠` ganham `'` na frente, também quando antes deles só há espaço (qualquer branco Unicode), NUL ou U+FEFF, que algumas planilhas descartam antes de ler a fórmula. Número não (`-5` continua número).
- Aspas RFC 4180 quando a célula tem `,`, `"`, CR ou LF; linhas em CRLF.
- **UTF-8 sem BOM.** O BOM ajuda o Excel a acertar os acentos ao abrir com duplo clique, mas vira lixo no 1º nome de coluna para `cut`, `csvkit`, pandas sem `encoding="utf-8-sig"` e para quem concatena arquivos. O CSV daqui é para ferramenta; no Excel, importar como UTF-8.
- Resultado vazio vira arquivo vazio, sem linha de cabeçalho em branco.

**Parquet fica fora.** O crate `parquet` puxa o `arrow` e centenas de dependências transitivas, com vários MB a mais no binário, para um formato que quem precisa converte do JSONL com uma linha (DuckDB, pandas). Volta à mesa se aparecer uso real.

## Formatos de entrada: MessagePack, CBOR, BSON, YAML e TOML (Fase 5, 24/11/2026)

**Converter para JSONL no cache, não um segundo modelo.** `nucleo::formatos` lê cada formato e grava JSONL (um documento por linha) no mesmo cache dos `.gz`/`.zst` (`compactado::em_cache`, que ganhou uma etiqueta na chave com o formato e a versão da conversão). Daí em diante é o caminho de sempre: `mmap`, índice de linhas, árvore, busca, jq, Estrutura, Tabela, Exportar e Diff funcionam sem mudança. A conversão é feita uma vez; reabrir usa o cache (chave: caminho canônico, tamanho, mtime e etiqueta). `ArquivoLinhas::abrir_com` é o único ponto de entrada, então app e CLI ganham os formatos juntos. Somente leitura: nada é gravado de volta.

**Detecção.** Pela extensão: `.msgpack`/`.mpk`, `.cbor`, `.bson`, `.yaml`/`.yml`, `.toml`; `.json`, `.jsonl` e `.ndjson` nunca passam pela conversão (custo zero para a primeira tela). Sem extensão conhecida, pela assinatura: CBOR com a marca de autodescrição (`D9 D9 F7`); YAML começando em `%YAML` ou `---` (depois do BOM); BSON com o `int32` do 1º documento entre 5 e o tamanho do arquivo, tipo do 1º elemento válido e byte zero no fim declarado (texto JSON não tem byte zero). MessagePack e TOML não têm assinatura confiável: só pela extensão. No app, o filtro do diálogo e o arrastar aceitam as extensões novas (`abas.ts`); associação de arquivo do sistema continua só para JSON/JSONL, para não disputar `.yaml`/`.toml` com editores.

**Representação do que não tem par em JSON.**
- Inteiro fora de ±2^53 (MessagePack `uint64`, CBOR bignum, YAML/TOML): sai com todos os dígitos. É JSON válido; o app já mostra o número cru.
- NaN e ±infinito: textos `"NaN"`, `"Infinity"`, `"-Infinity"` (JSON não tem). Real sai na forma mais curta que volta ao mesmo valor (`f32` como `f32`: `0.1`, não `0.10000000149011612`).
- Binário (MessagePack `bin`, CBOR byte string): texto base64 padrão, com `=`.
- Chave que não é texto (MessagePack, CBOR e YAML aceitam qualquer valor como chave): o JSON compacto dela vira a chave (`1` → `"1"`, `[1,2]` → `"[1,2]"`). Chave repetida fica repetida, como no JSONL cru.
- MessagePack ext: `{"$ext": tipo, "$dados": base64}`; o timestamp (ext −1) vira ISO 8601 em UTC (anos 0–9999; fora disso, a forma genérica).
- CBOR: marcas (tags) são transparentes: sai o valor marcado, sem interpretar (data da tag 1 sai como o número de segundos; fração decimal da tag 4 sai como a lista `[expoente, mantissa]`; URI sai como texto). Exceção: bignum (2 e 3) de até 1 KiB vira inteiro decimal; acima, base64. `undefined` vira `null`; valor simples não atribuído vira `"simple(N)"`.
- BSON: documentos concatenados (o que o `mongodump` grava), cada um uma linha, no Extended JSON relaxado do MongoDB (`{"$oid": …}`, `{"$date": "…Z"}`, `{"$binary": …}`, `{"$numberDecimal": …}`), o mesmo do `mongoexport`. Código com escopo vira `{"$code": …, "$scope": {…}}`.
- YAML: esquema core do YAML 1.2 (`yes`/`no`/`on` são texto; `0x10` e `0o7` são inteiros; zero à esquerda é removido; real que estoura o `f64`, como `1e400`, vira `"Infinity"`, igual a `.inf`); datas ficam texto; vários documentos (`---`) viram várias linhas; `<<` (merge do YAML 1.1) fica como chave comum, sem mesclar.
- TOML: data e hora viram texto RFC 3339; chaves na ordem do arquivo (feature `preserve_order`).

**Tetos (`formatos::Limites`, cada um com teste em `core/tests/formatos.rs` que falha se ele for desligado).** Entrada é não confiável: erro sempre `InvalidData` com mensagem ("YAML: aliases expandem mais de 1000000 valores (bomba de aliases?)…"), nunca pânico.
- Profundidade: 1000 níveis (`arvore::MAX_PROFUNDIDADE`), então tudo que converte a árvore abre. No YAML o limite real é menor: o parser do `yaml-rust2` recusa a partir de 255 níveis ("recursion limit exceeded"). Os leitores próprios e os percursos são iterativos (pilha no heap); 100 mil níveis dão erro, não estouro de pilha (teste).
- Nós: 2^31 valores no arquivo, aliases expandidos inclusive.
- Saída: os mesmos tetos dos compactados: `max_saida` (10 GiB) e no máximo `max_razao` (100×) o arquivo, com piso de 16 MiB; parcial é apagado. Sem o "abrir assim mesmo" dos compactados: conversão legítima não chega a 100× (o pior caso sem alias é um caractere de controle, 1 byte → 6).
- Bomba de aliases do YAML ("billion laughs"): aliases podem acrescentar no máximo 1 milhão de valores e 256 MiB de JSON (a variante com poucos nós e âncora gorda: um texto de 16 MiB repetido 600 vezes); cada âncora guarda no máximo 16 MiB de JSON para repetir (acima disso, alias para ela é erro). O alias passa pelo mesmo teto de saída que o resto.
- Tags YAML fora do esquema (`!python/object`, `!Ref`, `!custom`, `!<tag:…>`) são recusadas; `!!str`, `!!int`, `!!float`, `!!bool`, `!!null`, `!!seq`, `!!map`, `!!binary`, `!!timestamp` e `!` passam.
- Comprimento declarado mentiroso (MessagePack `array32`/`map32`/`bin32`/`str32`, CBOR com tamanho de 64 bits, BSON `int32`): conferido contra o que resta do arquivo antes de qualquer leitura; nunca vira alocação. Contêiner com N itens exige ao menos N bytes restantes.
- Chave que não é texto: até 64 KiB de JSON.
- Texto/binário CBOR em pedaços (indefinido) precisa ser juntado na memória: 64 MiB.
- TOML: arquivo de no máximo 1 MiB. O `toml` monta a árvore inteira, que mede até ~180× o arquivo (medido: 5,5 MB de tabelas pequenas → 965 MB de RSS; 4 MB de `a=[1,1,…]` → 378 MB); com 1 MiB, o pico fica perto de 100 MB. Configuração real (Cargo.lock, pyproject) cabe com folga.

**Arquivo grande.** MessagePack, CBOR, BSON e YAML são lidos por `mmap` e convertidos em fluxo, com memória proporcional à profundidade (e às âncoras do YAML), não ao tamanho; progresso e cancelamento como na descompactação. TOML não tem leitura em fluxo no `toml`: arquivo acima de 1 MiB é recusado com mensagem ("TOML é carregado inteiro na memória"); a mensagem de erro de sintaxe é cortada em 500 caracteres (o `toml` cita a linha, que pode ser o arquivo todo). O cache gasta disco: o JSONL convertido costuma ter 1–3× o original.

**Dependências novas.**
- `yaml-rust2` 0.13 (MIT): parser YAML 1.2 puro Rust, mantido (fork ativo do `yaml-rust`), com API de eventos: dá para converter em fluxo e controlar âncora, alias e tag sem montar a árvore. `serde_yaml` está arquivado; `serde_yml` teve problemas de manutenção.
- `bson` 3.1 (MIT OR Apache-2.0), o crate oficial do MongoDB: `RawDocument` lê sobre o `mmap` sem copiar e gera o Extended JSON relaxado; features `serde` e `serde_json-1` para isso. Puxa `bitvec`, `rand`, `serde_bytes`, `hashlink` e afins (MIT/Apache).
- `toml` 1.2 (MIT OR Apache-2.0), que já vinha transitivo pelo Tauri; entra direto no núcleo com `parse`, `serde` e `preserve_order` (esta puxa o `indexmap`, já na árvore).
- MessagePack e CBOR têm leitor próprio (~250 linhas): os crates (`rmpv`, `ciborium`) montam a árvore inteira em memória e alocam pelo tamanho declarado em alguns caminhos; o formato é simples e o leitor próprio confere cada comprimento contra o arquivo.

**Fuzz:** um alvo por decodificador (`formato_msgpack`, `formato_cbor`, `formato_bson`, `formato_yaml`, `formato_toml`), todos com tetos pequenos e conferindo que a saída é JSONL válido (`serde_json`). Com 18 alvos, o fuzz curto passou de 23 para 16 s por alvo (`FUZZ_SEG`, CI e `gates.sh`), para caber no mesmo tempo do job (18 × 16 ≈ 13 × 23).

**Seguir (arquivo vivo):** recusado para os formatos convertidos (`Vivo` confere `formatos::detectar`; teste `recusa_convertido`), porque a cópia seria dos bytes crus. O botão Seguir ainda aparece; ao clicar, a interface pausa com o motivo.

**Limites conhecidos:** YAML/TOML/binários compactados (`.yaml.gz`) não abrem (compactado só JSON/JSONL; dá para somar depois encadeando as duas conversões). A mensagem de progresso do app ainda diz "descompactando" durante a conversão. Posição no erro do YAML é em caracteres, nos binários em bytes.

## Linha do tempo de uma pasta (Fase 3, 24/11/2026)

**O que é.** Uma pasta com versões datadas do mesmo arquivo (`export-2024-01-01.jsonl`, `export-2024-01-02.jsonl`…): o núcleo (`nucleo::linha_do_tempo`) lista as versões em ordem, infere a estrutura de cada uma e compara com a anterior que abriu, usando o mesmo diff de estrutura (quebra × compatível) da visão Diff. Na CLI: `jasao linha-do-tempo <pasta> [--json]`. No app: visão Diff, tipo "Linha do tempo", botão Pasta… (diálogo no Rust) ou soltar uma pasta na janela com um arquivo aberto; marcar duas versões e "Comparar os dois marcados" abre as duas em abas e mostra o diff de estrutura delas (a mais velha como antes). Histórico git fica fora (decisão 4 do plano).

**Ordem.** Data no nome quando *todos* os nomes têm uma: `AAAA-MM-DD`, `AAAA_MM_DD`, `AAAA.MM.DD` ou `AAAAMMDD` (anos 1970–2100, dia validado pelo mês), com hora opcional `[T_- ]HH[:MM[:SS]]`, lida como UTC; dígito colado antes ou depois não conta (`120240101` não é data), mas sufixo numérico depois de `_` ou `-` vale e a data fica sem hora (`export-2024-01-01_001.jsonl`, achado do verificador: um nome assim derrubava a pasta inteira para o `mtime`). Se algum nome não tem data, a pasta inteira vai pelo `mtime` (misturar as duas origens daria uma ordem que ninguém consegue conferir). Empate: pelo nome. A interface diz qual origem valeu.

**A pasta é entrada não confiável.**
- Só o nível da pasta (sem descer em subpastas), só arquivo comum com extensão que o app abre (a mesma lista de `abas.ts`), sem ocultos.
- Link simbólico só entra se o alvo canônico mora dentro da pasta e é arquivo comum; link para fora conta como ignorado (teste no núcleo, na CLI e no IPC).
- No app, a pasta precisa estar concedida (diálogo ou arraste). Pasta concedida não concede o que está dentro dela: o comando `linha_do_tempo` concede só as versões que listou, depois de passar nos tetos, para a interface abrir os dois pontos escolhidos. Teste de IPC: pasta não concedida é negada; acima do teto nenhum arquivo fica concedido; `notas.txt` e o link para fora continuam negados.

**Tetos (`LIMITES`, cada um com teste no ponto de chamada real).**
- 100 mil entradas lidas da pasta, de qualquer tipo: passar é erro, não lista parcial.
- 500 versões: passar é erro antes de abrir qualquer arquivo (teste na CLI com 500 e 501, e no IPC com 501).
- Prazo de 10 minutos para o percurso: estourou, o resultado sai parcial com aviso (`parada: "prazo"`). O prazo e o cancelamento (Esc, trocar de vista) valem entre versões e no meio de uma inferência (a cada ~1% dela); a abertura em si (descompactar, converter) só é interrompida pelo cancelamento, não pelo prazo.
- 200 mudanças guardadas por versão; o resto só conta ("… e mais N").
- Memória: só dois `Resumo` vivos (o anterior e o atual), não um por versão.

**Nunca mapeia o arquivo do usuário.** Na pasta qualquer versão pode ser truncada por outro processo durante a varredura, e `mmap` do original dá SIGBUS e derruba o processo (no app, o Tauri inteiro; achado 14, reproduzido pelo verificador com `: > arquivo` no meio da inferência de um JSONL de 3 GB e da conversão de um MessagePack de 6 GB). `percorrer` abre cada versão sempre com `ArquivoLinhas::abrir_copia_com` (quem chama não escolhe a abertura):
- JSONL cru é copiado com `read` para uma cópia anônima do app (a mesma de `vivo`, apagada do diretório ao criar) e é a cópia que se mapeia;
- MessagePack, CBOR, BSON, YAML e TOML também: a conversão lê dessa cópia e escreve em outra cópia anônima, sem passar pelo cache de conversões (que mapeia o original, como ao abrir avulso);
- gzip e zstd já eram lidos em fluxo com `read` e descompactados para o cache.

Tamanho e `mtime` são conferidos antes de abrir e depois de inferir: se algum mudou, o passo sai "o arquivo mudou durante a leitura" e o seguinte compara com a última que abriu. Custo: uma cópia em disco por vez (duas para formato convertido) e o tempo de copiar; convertido é convertido de novo a cada percurso. Teto da cópia: `Opcoes::max_saida` (10 GiB). Na linha do tempo, uma versão acima disso vira passo com erro ("arquivo maior que …"), embora o mesmo arquivo abra avulso (avulso ele é mapeado, não copiado). Testes, cada um com a mutação conferida: JSONL truncado depois de abrir e MessagePack truncado no meio da conversão deixam o processo vivo e o passo com erro (com o `mmap` do original os dois morrem de SIGBUS); reescrita com o mesmo tamanho e outro `mtime` vira erro; teto da cópia com 8 e 9 bytes.

**Versão que não abre** (sem permissão, formato quebrado) vira um item "não abriu: …" e a seguinte compara com a última que abriu (o item diz "contra X" quando não é a vizinha).

**Limites conhecidos.** Compactados passam pelo cache em disco, como ao abrir um a um (convertidos, por cópias anônimas; ver acima): 500 `.gz` grandes ocupam o disco do cache. Soltar uma pasta sem arquivo aberto ainda só avisa (a visão Diff precisa de uma aba com documento). Entre listar e abrir há a mesma janela de troca de arquivo (TOCTOU) já registrada para os arquivos concedidos. Nome que não é UTF-8 vira texto com `to_string_lossy` (no núcleo e em `PassoUi.caminho`): aparece com `�` e o caminho mandado à interface não reabre esse arquivo; fatia futura (levar o caminho como bytes ou por índice da lista).

## Máscara de segredos (Fase 5, 24/11/2026)

**Escolha: um ponto só.** `ArquivoLinhas::bytes()` devolve os bytes já mascarados, do mesmo tamanho do original e ainda JSON válido. Árvore, Texto, formatado, valor/copiar, busca, Estrutura, perfil, Tabela, Exportar, jq, Diff e o CLI leem dali, então nenhum caminho de saída precisa lembrar de mascarar: o que esquece, mascara assim mesmo. O original sai só por `cru()`/`cru_de()`, em lugares que não devolvem valor: o motivo e o byte do erro de sintaxe na Árvore, na Tabela e no `jasao erros` (a máscara mudaria o erro), a marca `mascarado` da linha (compara os dois), o `validar` (a regra do schema vale para o valor de verdade; a mensagem usa `masked()` do `jsonschema`, sem o valor) e a anonimização. E pelo `valor_cru`, que a interface só chama no clique de "Mostrar valor".

**Como mascara.** `nucleo::mascara` percorre o arquivo sob demanda, por blocos (ver Desempenho; léxico próprio, sem montar árvore; JSONL linha a linha, documento de ponta a ponta) e marca trechos:
- texto abaixo de chave sensível, em qualquer ancestral (`{"senha": {"a": "x"}}` mascara `x`), ou cujo valor tem cara de segredo: as aspas ficam, cada byte vira `*`;
- número abaixo de chave sensível: cada dígito vira `9` (`4821` → `9999`), continua número;
- `true`, `false` e `null` não mudam (não carregam segredo e mascarar mudaria o tipo).
O mapa mascarado é uma cópia privada do `mmap` (`map_copy`, copy-on-write): só as páginas com segredo ocupam memória; arquivo sem segredo não aloca nada.

**O que é sensível.** Chave: a heurística do #43 (`chave_de_segredo`: password, senha, secret, token, api_key, pass/pin/passcode, plurais) mais `cvv`, `cvc`, `otp`, `otp_code`, `cpf`, `cnpj`, `rg`, `cartao`, `card_number`/`cartao_numero`. Vale **qualquer palavra** da chave (`_`, `-`, `.`, espaço e caixa separam: `cpf_cliente`, `cpfCliente`, `senhaAtual`, `token_value`, `password_confirmation`), menos quando a última diz que é metadado do segredo e não ele (`token_type`, `password_hash`, `cpf_valido`, `rg_emissor`, `cartao_tipo`, `token_expires`; lista em `chave_de_segredo`). A chave é comparada sem acento (`cartão`, `número_cartão`, também na forma decomposta) e com os escapes do JSON resolvidos (`"\u0073enha"` é `senha`). Valor (texto, sob qualquer chave), também com os escapes resolvidos: o `parece_segredo` do #43 (`Bearer`/`Basic` em qualquer caixa, JWT, chaves da AWS `AKIA`/`ASIA`, GitHub, GitLab `glpat-`, npm `npm_`, Slack, Google `AIza`, `sk-`/`sk_live_`/`sk_test_`) e documentos com dígito verificador: CPF (com ou sem pontuação, mód. 11), CNPJ e cartão de 13 a 19 dígitos por Luhn (começa com 3–6 ou 2221–2720, a série 2 da Mastercard; só dígitos, espaço e hífen). Sequência repetida (`111.111.111-11`) não conta. Número cru não é conferido pelo valor (todo id de 11 dígitos viraria suspeito).

**Linha inválida.** Até o ponto estranho, mascara como de costume. Dali ao fim da linha (no documento de várias linhas, ao fim do arquivo) mascara letras, dígitos e não ASCII se houver sinal de segredo: chave sensível aberta, ou algum pedaço da linha (entre aspas ou entre espaço e pontuação) é chave ou valor sensível. Sem sinal, fica como está: `{"b":tru}` continua legível para o usuário achar o erro. Limite: uma linha quebrada com um segredo sem nome nem formato conhecido sai crua, como sairia numa linha válida. **Profundidade:** o léxico para em `arvore::MAX_PROFUNDIDADE` (1 000) contêineres abertos; o que passa disso é tratado como linha estranha (mesma regra acima). A pilha não cresce com a linha: uma linha de 200 MB de `[` não aloca mais que a pilha do teto.

**Revelar.** Linha (ou contêiner) com trecho mascarado vem com `mascarado: true`; o painel do valor mostra o aviso e, só em folha (texto, número, `true`/`false`/`null`), o botão "Mostrar valor", que chama `valor_cru` para aquele campo. O núcleo recusa o resto (`valor_cru` de contêiner ou de linha inválida dá `None`): revelar o contêiner mostraria todos os segredos de dentro de uma vez e mostra como texto (`textContent`, invisíveis à vista). Nada é guardado: mudar a seleção esconde de novo. Copiar continua mascarado. O CLI tem `--sem-mascara`, em qualquer posição, para ver o arquivo como está.

**Anonimizar (jq).** No app, o botão "anon" da consulta; no CLI, `consultar --anonimizar`. O filtro roda sobre uma versão em que cada valor mascarado vira `"anon:<16 hex>"`: SipHash (`DefaultHasher`) do sal da sessão com o valor original. Mesmo valor, mesmo código dentro da sessão (dá para contar, agrupar e juntar); entre sessões, códigos diferentes. Número sensível vira texto `"anon:…"`. **Não é criptografia:** 64 bits, função não feita para isso; um valor de domínio pequeno (PIN de 4 dígitos, CVV) se descobre testando todos contra o código enquanto a sessão está aberta, para quem tiver o sal. O sal é aleatório (`RandomState`), só em memória: no app, um por processo; no CLI, um por execução. No app ele vai ao processo filho do jq pela variável `JASAO_CONSULTA_SAL`, que outros processos do mesmo usuário conseguem ler enquanto a consulta roda. **Teto e cancelamento:** a versão anonimizada é uma cópia na memória do tamanho do arquivo (os comprimentos mudam, não dá para mapear). No app ela é feita no processo filho, que o pai mata ao cancelar ou ao passar do tempo: cancelar funciona, mas um arquivo de vários GB anonimizado ocupa essa memória no filho enquanto dura. Fatia futura: anonimizar em fluxo, bloco a bloco, entregando ao jq sem a cópia inteira.

**Testado.** Núcleo (`mascara::testes`): chave em qualquer ancestral, cvv/otp/cpf/cnpj/rg/cartão pelo nome, CPF/CNPJ/cartão pelo valor (positivos e negativos: dígito errado, repetidos, Luhn errado, datas, telefones), mesmo tamanho e JSON válido, linha estranha com e sem sinal, documento de várias linhas, anonimizar estável e sem o valor, e um proptest (JSON qualquer, compacto ou formatado, com chaves sensíveis: segue válido, do mesmo tamanho, e nenhum escalar sobra abaixo de chave sensível). App (`segredo_nao_sai_pelo_ipc`, dados sintéticos): toda resposta de linhas, texto, valor, formatado, estrutura, perfil de cada caminho, tabela, exportar Schema/Zod/TS com literais, validar contra schema de fora, busca (o segredo não acha nada; o nome acha) e o arquivo da consulta jq, mascarada e anonimizada, passa por um crivo com os valores sentinela; `valor_cru` devolve exatamente o que foi escondido. CLI (`cli/tests/mascara.rs`): janela, buscar, consultar (incluindo `--documento`, `--anonimizar` e erro do jq), erros, inferir, exportar com `--literal`, validar e diff, pelo binário; `--sem-mascara` mostra. **Mutação:** fazer `bytes()` devolver o original derruba os dois testes de garantia. Na correção pedida pelo verificador (PR #50), cada furo ganhou teste que a mutação correspondente derruba: escapes `\uXXXX` na chave e no valor (`escapes_do_json_nao_furam_a_mascara`, e o proptest troca letras por escapes); acento em `cartão` e chave por palavra (`chave_de_segredo_por_palavra`); série 2 da Mastercard e prefixos novos (`padroes_de_credencial_e_cartao_serie_2`); linha com 50 mil `[` e um segredo dentro (`linha_funda_tem_teto_e_nao_solta_segredo`); documento em blocos igual a de uma vez (`documento_em_blocos_da_o_mesmo_que_de_uma_vez`, blocos de 1, 7 e 1 000 linhas); a 1ª tela do documento mascara só até o bloco que mostra (`documento_mascara_so_os_blocos_ate_a_janela`); JSONL não vira documento (U); linha estranha na anonimização (X); CNPJ com dígito errado (4); sal por chamada (L); arquivo vivo que cresce sai mascarado (N, `vivo::testes::crescimento_sai_mascarado`); `valor_cru` de contêiner dá `None`.

**Desempenho.** A primeira versão mascarava o arquivo inteiro ao abrir, numa thread só: em 1 GB a primeira tela foi de 0,26 s para 2,1 s e o índice de 0,23 s para 2,15 s, as duas acima da meta. Agora a máscara é preguiçosa e por blocos de 16 384 linhas (`linhas::Mascara`). A `janela` mascara só os blocos que mostra. `bytes()` (busca, consulta, estrutura, exportação) mascara o resto em paralelo (rayon) antes de devolver o arquivo. Cada bloco é escrito uma vez só, dentro do seu `OnceLock`. Num JSONL cada linha é lida do zero, então os blocos não dependem um do outro. Documento em várias linhas também vai por blocos, mas em ordem: o estado do léxico (pilha, chave, cache de chaves, `mascara::Estado`) passa de um bloco ao seguinte sob um `Mutex`, e a `janela` lê só até o bloco que mostra. Sem segredo nenhum, `bytes()` devolve o próprio arquivo: ler a cópia mapearia as mesmas páginas de novo (RSS em dobro). `scripts/medir.sh`, 1 GB sem segredo, mediana de 3, carga 7,5, M5 Pro, 24/11/2026: primeira tela 0,23 s, índice 0,23 s, estrutura 2,12 s, busca literal 0,49 s e regex 0,47 s, exportar + validar 3,28 s, pico alocado ≤ 64 MB. Todas as metas batidas. A busca até o fim paga a máscara do arquivo inteiro: era 0,28 s antes da máscara e continua abaixo de 3 s. Com carga alta (40 a 60, outras sessões compilando) os números dobram; a primeira tela passa dos 300 ms com ou sem máscara. JSON formatado (o mesmo 1 GB, um campo por linha: 51,6 M linhas), primeira tela: 0,63 s com máscara e 0,62 s sem (`--sem-mascara`), mesmo pico alocado (~400 MB) nas duas. A máscara custa ~0,01 a 0,07 s; o resto é o índice de linhas (8 bytes por linha, montado inteiro ao abrir), que já existia e passa sozinho dos 300 ms e dos 300 MB com tantas linhas. Por isso essas duas linhas do `medir.sh` saem sem meta. Fatia futura: índice de linhas preguiçoso ou compacto (não é desta). Testes: `mascara::testes::em_blocos_da_o_mesmo_que_numa_thread` (blocos = uma thread, com linha inválida e documento aberto no meio) e `linhas::testes::mascara_por_blocos_sob_demanda` (a janela de um bloco adiante sai mascarada sem `bytes()`, e `bytes()` mascara o resto). A mutação que pula o bloco na `janela` derruba esse teste.

**Fatias futuras (não feitas aqui):** CPF ou token dentro de texto livre (`"obs": "cpf 111.444.777-35 do cliente"` só sai mascarado se a chave for sensível); CPF ou token usado como chave de mapa (`{"111.444.777-35": {...}}`: a chave nunca é mascarada, é estrutura); anonimizar em fluxo (acima).

**Não testado:** a interface à mão (painel, botão, "anon"); YAML/TOML/binários convertidos passam pelo mesmo `ArquivoLinhas`, mas sem teste próprio. O export JSONL/CSV do jq (#47) é coberto: no app, `gravar_resultado` em JSONL e CSV sobre o resultado mascarado (`segredo_nao_sai_pelo_ipc`); no CLI, `jq --saida csv|jsonl`, com e sem `--anonimizar` (`cli/tests/mascara.rs`). A cópia anonimizada é feita no processo filho e conta no teto de heap dele (`filho::TETO_MEMORIA`, `--max-memoria-mb`).

## Filtro por variante e por caminho (Fase 2, 24/11/2026)

**O que faz:** a partir da Estrutura (tecla F na linha) ou do painel de estatísticas ("filtrar estes N", com e sem o caminho), filtra os registros de uma variante (`.tipo = "X"`) ou os que têm / não têm um caminho. A Tabela passa a mostrar só os filtrados, com chip na barra (critério, "N de M registros" e × para tirar). Na Árvore, decisão do Eduardo "A, só navegação": ⌘↓/⌘↑ anda entre os registros filtrados e a Árvore não filtra. Se a Tabela não serve para o arquivo, o filtro abre a Árvore direto.

**Como roda:** `nucleo::filtro` varre o arquivo inteiro em 2º plano, em paralelo por blocos como `perfil` e a busca, e devolve a lista crescente dos registros que passam no espaço de índices da Tabela (JSONL: linha; JSON único: elemento). No app, o comando `filtrar` devolve a geração e o evento `filtragem` leva progresso e fim; `cancelar_filtro`, `limpar_filtro` e `filtrado(posicao)` (linha e byte do k-ésimo filtrado, para a Árvore) completam. Filtro novo derruba a ordem (a ordem guarda a versão do filtro sobre a qual foi feita, e `vista()` descarta a de outra versão); a interface pede a mesma ordem de novo sobre o filtro novo. O critério de variante compara o valor cru do discriminador do topo, como a inferência conta (chave repetida: vale a 1ª).

**Teto:** `MAX_FILTRADOS` = 50 milhões de índices de 4 bytes (200 MB), o mesmo da ordenação. Ao passar, a varredura para, o evento traz `falha: "grande"` e o filtro anterior continua valendo. O teto é do documento (`EstadoFiltro.teto`) e o teste `comandos_pelo_ipc` o baixa para 1 no call site real.

**Medido (24/11/2026, JSONL de 1 GB de `examples/gerar`, 6.456.439 linhas, M5 Pro, load ~25):** filtro por variante entre 0,30 e 0,39 s (cerca de 2,15 milhões de achados cada, iguais às contagens da inferência); "tem"/"não tem" um caminho entre 0,34 e 0,41 s. `medir.sh` na mesma fatia: 1ª tela 0,20 s (meta 300 ms) e inferência 2,05 s, sem mudança, porque o filtro só roda sob pedido.

**Fora desta fatia:** filtrar a própria Árvore; ⌘↓ parte da última posição navegada, não do cursor; filtro por forma sem discriminador; combinar filtros; guardar o filtro entre sessões; variante cujo valor tem UTF-8 inválido não casa (o valor chega à interface já convertido com perda).

**Não conferido:** a interface não foi aberta no app nesta fatia. O comando e o evento foram testados pelo IPC (`comandos_pelo_ipc`), e a lógica pura da barra, do chip e da Estrutura pelos testes do `app`.

## Salvar como: transformar em arquivo novo (Fase 5, 24/11/2026)

**O que faz.** Grava num arquivo **novo** o arquivo inteiro, o resultado da consulta jq (#47) ou os registros do filtro da Tabela (#49), com três transformações nos campos do topo de cada registro: manter, tirar e renomear (`antigo=novo`). Formatos: JSONL, JSON formatado (dois espaços) e JSON compacto; um resultado sai sozinho, vários saem numa lista. CSV continua só no exportar do resultado da consulta. No app, botão "Salvar como…" na barra da consulta; no CLI, `jasao transformar <arq> [--jq F] [--manter a,b] [--tirar a,b] [--renomear x=y,...] [--saida jsonl|json|json-compacto] [--anonimizar] [--sobrescrever] -o novo`.

**Renomear sobre campo existente.** `a=b` com `b` já no registro: fica o valor de `a`, em qualquer ordem dos campos (antes, o resultado dependia da ordem e um valor sumia calado). O `b` antigo só sai quando o registro tem `a`; sem `a`, o registro passa igual. Pares ambíguos (o mesmo campo renomeado duas vezes, ou dois campos com o mesmo nome novo) são recusados antes de gravar (`Transformacao::conferir`), no app e no CLI. Trocar dois nomes (`a=b,b=a`) vale.

**Registros descartados.** Linha que não é JSON ou erro do jq num registro não para o salvamento: o registro fica de fora e é contado. O app recebe a contagem e o primeiro erro no evento final (`erros`, `primeiro_erro`) e mostra no diálogo. O CLI imprime a contagem e os primeiros no stderr e sai com 3 (0 gravou tudo · 1 não gravou · 2 uso errado · 3 gravou, mas descartou).

**Como roda.** Tudo vira uma expressão jq (`nucleo::transformar::compor`): `with_entries` com o conjunto de nomes como objeto (nome com aspas ou barra não injeta nada), aplicada só a objetos. Fonte jq: `(F) | T`. Fonte filtro: o mesmo `select` do #49 (`criterio_jq`), seguido de `T`. Em JSON único, registro é cada elemento da lista do topo, ou o objeto do topo, como na Tabela. Quem compõe é o processo filho (o mesmo do jq, `filho::Salvar` na variável `JASAO_CONSULTA_SALVAR`), porque só ele abre o arquivo que vai ler: com o resultado de uma consulta aberto na aba, o arquivo da fonte é o original, e o formato dele (JSON único ou JSONL) pode não ser o do que está na tela. O pai só confere que compila. Variável presente e ilegível encerra o filho sem gravar: nunca cai na consulta comum nem perde a máscara.

**Nunca o original.** O destino vem do diálogo do sistema, no Rust (a interface não manda caminho). `transformar::temporario` recusa o arquivo aberto, os recentes e os concedidos, comparando o caminho canônico (pega `./`, `..` e link simbólico) e, no Unix, dispositivo + inode (pega link físico). No Windows `id()` é `None`: só o caminho canônico protege, e um link físico para o original não é pego. Windows não é alvo hoje. O resultado vai para um temporário oculto (`.jasao-salvar-*`) na pasta do destino. Só no fim, sem falha nem cancelamento, `persistir` confere de novo (o destino pode ter virado link para o original enquanto gravava), faz `fsync` e troca por `rename`. Cancelar mata o filho e o temporário some no `drop`. O destino que já existia fica como estava. O `gravar_resultado` do #47 passou a usar as mesmas duas funções.

**Máscara.** Padrão: mascarado, como em todo caminho de saída. "Anonimizar" usa o sal da sessão (app) ou um sal novo por execução (CLI). "Sem máscara" só por escolha explícita: no app, com aviso no diálogo; no CLI, com o `--sem-mascara` global, que avisa no stderr. O arquivo sem máscara fica 0600 (só do dono, como o temporário nasce); o mascarado e o anonimizado ficam 0644. No CLI, destino que já existe só com `--sobrescrever` (o original é recusado mesmo com ele); a conferência é antes de gravar, e outro processo criando o destino no meio ganha a corrida. No app, o diálogo do sistema já pergunta antes de sobrescrever.

**Arquivo grande.** O filho grava em fluxo, sem o teto de saída nem o relógio da consulta (é o arquivo pedido), mas com o teto de heap (`TETO_MEMORIA`, 4 GiB). Progresso pelo evento `salvando` e cancelar por `cancelar_salvar`. Fechar o diálogo também cancela. Anonimizar copia o arquivo na memória do filho (ver "Máscara de segredos"). O CLI roda no próprio processo, sem progresso nem cancelar: morto por sinal, pode sobrar um `.jasao-salvar-*` oculto na pasta, nunca o destino pela metade.

**Testado.** Núcleo (`transformar::testes`): cada transformação, nomes com aspas e barra, ordem manter → tirar → renomear, fonte jq e fonte filtro, documento (elementos da lista do topo), os três formatos, recusa do original por caminho direto, `./`, link simbólico e link físico, `temporario` e `persistir` recusando cada um sozinho, `rename` sem meio-termo, cancelar sem resíduo, máscara por padrão, permissão 0600 só no cru, renomear sobre campo existente nas duas ordens e pares ambíguos, chave e valor com `"` e `\(` no filtro. App (`salvar_como_pelo_ipc`): recusa do original e de link para ele, máscara padrão, anonimizar e sem máscara, fonte jq com tirar e renomear em JSON, manter, fonte filtro em JSON compacto, original JSON único com um JSONL aberto na aba, permissão 0600 do cru e 0644 do mascarado, erro do jq contado com o primeiro erro, renomear ambíguo recusado, e cancelar sem destino nem temporário. CLI (`cli/tests/transformar.rs`, pelo binário): recusa do original e de link, máscara padrão, `--anonimizar`, `--sem-mascara`, tirar + renomear, manter, `--jq` com `--saida json`, `json-compacto`, `-o` ausente (código 2), destino existente sem `--sobrescrever` (código 1, intacto), aviso e 0600 do `--sem-mascara`, renomear ambíguo (código 1) e descartados (código 3 com a contagem).

**Mutação.** Cada proteção foi tirada e o teste dela caiu: recusa em `temporario` (núcleo e IPC), recusa em `persistir` (núcleo), dispositivo + inode em `destino_protegido` (núcleo), `Mascara` padrão (núcleo e IPC), `Mascarar` lendo os bytes crus (núcleo, IPC e CLI), máscara padrão do CLI (CLI), `rename` só no sucesso (núcleo e IPC), cada transformação (núcleo), compor no filho com o formato do arquivo da fonte (IPC), 0600 do cru (núcleo, IPC e CLI), renomear sobre campo existente e só com o renomeado presente (núcleo), cada recusa de `conferir` (núcleo; a de nome novo repetido também IPC e CLI), código 3, contagem, `--sobrescrever` e aviso do `--sem-mascara` (CLI), e o escape de `caminho_jq` e de `criterio_jq` (núcleo). Sobrevive tirar só a comparação do caminho canônico: no Unix, dispositivo + inode já pega o link simbólico e o `./`; ela vale para Windows, que não é testado aqui. O script ficou fora do repositório.

**Fora desta fatia.** Edição no lugar e desfazer; Parquet; transformar campos aninhados (só o topo de cada registro); combinar fontes (jq sobre o filtro da Tabela); CSV no "Salvar como" (continua no exportar da consulta); progresso e cancelar no CLI; anonimizar em fluxo. A fonte filtro fica desligada com resultado de consulta aberto na aba (o filtro é do arquivo na tela, não do original).

**Não conferido.** O diálogo não foi aberto no app à mão nesta fatia. O comando, o evento e o cancelar foram testados pelo IPC; a interface passou só por `tsc` e pelos testes do `app`. A mensagem de descartados no diálogo também não foi vista na tela.

## stdin por temporário (24/11/2026)

**O que faz:** `-` no lugar de um arquivo lê stdin em todo comando do CLI que abre arquivo (`cat x.jsonl | jasao laudo -`). Um `-` por execução (stdin se lê uma vez): `diff - -` é erro claro antes de ler ("stdin só se lê uma vez…"), com código 3. Arquivo chamado `-` abre como `./-` (está no USO). Em `buscar`, só o primeiro argumento é arquivo; `-` no lugar do padrão é o padrão. `linha-do-tempo` recebe pasta e não lê stdin. No app, `-` no argv da 1ª instância abre stdin numa aba.

**Sem pipe implícito (mudança na revisão do #55):** a 1ª versão lia stdin sozinha nos comandos de um arquivo quando não vinha arquivo e stdin era pipe. Efeito colateral: `sleep 4 | jasao linhas` ficava esperando o EOF em vez de mostrar o uso. Agora stdin só é lido com `-` explícito; sem arquivo, o comando dá o uso com código 2, como antes da fatia. Explícito é previsível em script e não depende de adivinhar se stdin "está pronto".

**Por que temporário e não streaming:** o núcleo todo trabalha sobre `mmap` e índice de linhas de um arquivo (janela, busca, consulta, máscara por blocos, arquivo vivo). Ler em fluxo pediria um segundo caminho em cada um. `nucleo::entrada::gravar` copia o fluxo para um arquivo e o resto abre esse caminho como qualquer outro, inclusive gzip/zstd (detectados pelos bytes). O custo é o disco: stdin de N bytes ocupa N bytes até o comando acabar, e a primeira saída só vem depois de stdin fechar (não serve para `tail -f | jasao`). A peça genérica é `nucleo::entrada::criar(prefixo, op)`: cria `<prefixo>.<pid>.<nanos>.parcial` travado, 0600, registrado para o sinal, e devolve o arquivo para escrita; `gravar` é `criar("entrada")` mais cópia, teto e espaço. A fatia de abrir de URL usa `criar("download")` (ou `gravar`) e herda sinal e órfãos sem código novo. Prefixos aceitos ficam em `PREFIXOS`; outro prefixo é recusado, para nenhum temporário escapar da limpeza de órfãos.

**Onde e com que permissão:** na pasta do cache do app (`…/jasao/descompactados`, a mesma dos descompactados, 0700 e recusada se for link), arquivo criado com `create_new` e modo 0600 (sem janela em que outro usuário leia), nome `entrada.<pid>.<nanos>.parcial`.

**Teto e espaço:** `Opcoes::max_saida`, o mesmo do descompactado (10 GiB). Passou, erro com o teto na mensagem e o temporário apagado. Antes de criar e a cada 64 MiB gravados, `fs4::available_space` confere que sobram 64 MiB no disco do cache; se não, erro "sem espaço para gravar stdin…" e o temporário sai. Para teste, `JASAO_TETO_STDIN_TESTE` (bytes) baixa o teto do CLI; nunca sobe acima do padrão.

**Limpeza, caso a caso:**
- **Fim normal e erro:** o `Temporario` apaga o arquivo no `Drop`. No CLI ele vive até o fim do `main`; o filho da consulta (`supervisionar`) recebe o caminho e stdin nulo, e o pai só solta o temporário depois que o filho acaba. No app ele fica no estado `Stdin` e sai no `RunEvent::Exit`.
- **SIGINT e SIGTERM no CLI:** antes de ler stdin o CLI chama `nucleo::entrada::apagar_no_sinal`, que liga uma vez por processo um tratador (`ctrlc`, feature `termination`) que apaga todos os temporários vivos de `criar`, de qualquer prefixo, e sai com 130. O app não liga (a janela trata o próprio fim). Na consulta, o sinal mata o pai e apaga o temporário; o filho que já estava rodando segue até o fim do seu teto de relógio lendo o arquivo já desvinculado (o espaço volta quando ele fecha).
- **Morte sem chance de limpar (SIGKILL, crash, queda de energia), CLI e app:** o arquivo fica no cache. O dono segura uma trava exclusiva (`flock`) no temporário enquanto vive, e o sistema solta a trava quando o processo morre, de qualquer jeito. `nucleo::entrada::limpar_orfaos` apaga todo `<prefixo>.*.parcial` de `PREFIXOS` (`entrada`, `download`) que consegue travar e que tem mais de 60 s (carência para o intervalo entre criar e travar); o `.parcial` do descompactado não tem prefixo nem trava e fica de fora. Ela roda no começo de todo `criar` (leitura de stdin no CLI e no app, download) e na abertura do app, com ou sem `-`. Ou seja: depois de um SIGKILL, o stdin fica em claro (0600, pasta 0700) até a próxima execução de `jasao … -` ou a próxima abertura do app, sem prazo se nenhuma das duas acontecer. A limpeza de um dia do cache (`compactado::limpar`) continua valendo como rede extra.

**Erros:** stdin vazio ("stdin vazio: nada para abrir"), stdin no terminal com `-`, dois `-`, acima do teto ou sem espaço: mensagem no stderr e código 1 (3 em `diff` e `validar`, o "não deu para abrir" deles). Nas mensagens e no laudo, o temporário aparece como `-`.

**App, o mínimo:** `-` só vale na 1ª instância. Com o app já aberto, o single-instance repassa o argv da 2ª, que é quem tem o stdin e sai em seguida; a 1ª não tem como lê-lo, então o `-` é ignorado (fica para uma fatia que grave o temporário antes de repassar). A leitura de stdin roda numa thread aberta no `setup`: a janela abre sem esperar o EOF, e o arquivo entra pela mesma fila do "abrir com" (`abertura::pedir`) quando a cópia acaba. A concessão do temporário vale só nesta execução (`Concedidos` é memória do processo) e ele não entra nos recentes (`lembrar` pula o caminho guardado no estado `Stdin`). A aba mostra o nome do temporário.

**Testado:** núcleo (`entrada::testes`): 0600, apagado no `Drop` e fora do registro de vivos, vazio e teto dão erro sem sobra, órfão destravado e velho de `entrada` e de `download` sai e o travado, o novo, o `.parcial` do descompactado e outro arquivo ficam; prefixo fora de `PREFIXOS` é recusado. CLI pelo binário (`cli/tests/stdin.rs`, com `HOME`/`XDG_CACHE_HOME` numa pasta própria): pipe de JSONL (`laudo -`, `jq … -`, `linhas -`, `buscar -`), pipe sem `-` dá uso (2), JSON em várias linhas, `diff - -` com o erro claro, stdin vazio (1, e 3 em `validar`), conteúdo inválido e binário, teto pela variável de teste (17 bytes num teto de 16 dá erro; 16 passa), `./-` abre o arquivo `-`, SIGTERM no meio de um stdin sem fim (sai com 130 e o temporário some), órfão de outro processo sai na próxima leitura e o travado fica. App (`recentes_pelo_ipc`): o temporário abre pelo IPC e a lista de recentes continua vazia. **Mutação:** sem `remove_file` no `Drop`, os testes do CLI caem; sem `apagar_vivos` no tratador, cai o do SIGTERM; sem `limpar_orfaos` no `gravar`, cai o de órfão; sem o desvio em `lembrar`, cai o de recentes. Rodado à mão: `app -` com stdin que só fecha em 4 s mostra a janela em 2 s, um órfão de janeiro some na abertura, e o temporário sai no ⌘Q.

**Não testado:** o app com `-` por teste automático (lê o stdin real do processo); SIGKILL de verdade (o teste simula o órfão com arquivo destravado e velho); Windows (fora do escopo).

## Abrir de URL e colar cURL (24/11/2026)

**O que faz:** abre JSON/JSONL de uma URL `https://…` ou de um comando cURL colado (o "Copiar como cURL" do navegador). No CLI, a URL ou o cURL vão no lugar do `<arquivo>` de qualquer subcomando de leitura (`jasao linhas 'https://…'`, `jasao janela "curl -H '…' https://…" 0`); é a forma mais simples, porque todos já passam pelo mesmo `abrir()`, e não cria subcomando novo. No app, o botão "Abrir URL…" (barra e tela sem arquivo) abre um campo que aceita os dois. `.gz` e `.zst` abrem transparentes: o corpo é guardado como veio e o abridor de sempre descompacta pelos bytes mágicos.

**Privacidade (decidido antes de escrever código):**
- Só baixa por ação explícita (o comando no CLI, o botão Abrir no app). Nunca baixa sozinho, não pré-carrega e não segue link achado no conteúdo.
- Envia só o pedido: método, cabeçalhos e corpo colados, mais um User-Agent genérico `jasao/<versão>`. Sem telemetria, sem cookies do sistema (o recurso de cookies do ureq está desligado).
- Do cURL interpreta só GET e POST (`-X`/`--request`), `-H`/`--header`, `-d`/`--data` (e o sinônimo `--data-raw`), `-u`/`--user` e `--compressed`. Qualquer outra opção é recusada com mensagem que a nomeia (`-L`, `-k`, `--insecure`, `-o`…). O comando nunca vai para um shell: é tokenizado pelo próprio núcleo (aspas simples, duplas, `$'…'`, barra invertida e continuação de linha). `$`, crase, `|`, `;`, `&`, `<`, `>` e parênteses fora de aspas são recusados, porque mudariam o sentido num shell. `-d @arquivo` é recusado (leria arquivo local).
- `-d` sem `-X` vira POST com `Content-Type: application/x-www-form-urlencoded`, como o curl faz. GET com `-d` é recusado.
- Cabeçalho de autorização (`Authorization`, `Proxy-Authorization`, `Cookie` e qualquer nome que `chave_de_segredo` reconhece) e `-u` (vira `Authorization: Basic`) nunca vão para disco, histórico ou log. O `Debug` do pedido os mascara, e todo erro passa por `Pedido::limpar`: troca cada segredo (e o que vem depois de `Bearer `) por `***`, e a URL pela versão sem consulta nem credencial. A mensagem de status HTTP não traz o corpo nem a frase do servidor (onde um eco do segredo poderia vir). O app esvazia o campo ao fechar.
- `http://` só depois de aviso explícito: no CLI, a flag `--http` (com aviso em stderr); no app, confirmação ("Conexão sem cifra"). Certificados TLS são sempre validados (raízes webpki embutidas; não há opção de pular).
- Teto explícito de tamanho (2 GiB, conferido no `Content-Length` e durante a leitura) e de tempo (300 s para o pedido inteiro). O download vai para um temporário de `nucleo::entrada::criar("download", …)` (o mesmo mecanismo do stdin, ver "stdin por temporário"): `download.<pid>.<ns>.parcial` no cache, 0600, travado enquanto o dono vive. `url::baixar` devolve o `Temporario`; quem chama o segura enquanto o arquivo estiver aberto (o CLI até o fim do `main`, o app até o `RunEvent::Exit`) e o `Drop` o apaga. Erro, teto e cancelamento apagam na hora. Não há `.dados` no cache nem código próprio de parcial ou despejo.
- Redirecionamentos seguidos pelo núcleo, não pelo ureq (`max_redirects(0)` e laço em `url::pedir`), para não depender do padrão da biblioteca: no máximo 5 (o 6º é erro). Nenhum salto vai de `https` para `http` (o pedido nem sai). Para outro servidor (esquema, host ou porta diferentes), todo cabeçalho que `sensivel()` classifica (`Authorization`, `Proxy-Authorization`, `Cookie`, `X-Api-Key`, `X-Auth-Token`…) é retirado, e 307/308 com corpo é recusado (reenviaria o corpo a quem o usuário não escolheu). 303, e 301/302 de POST, viram GET sem corpo. O teto de tempo vale para a cadeia inteira.
- `usuario:senha@host` na URL vira `Authorization: Basic`, como no curl, e sai da URL: assim cai nas mesmas regras (mascarado, retirado ao mudar de servidor). `-H Authorization` vale sobre `-u`, que vale sobre a URL. A URL é interpretada pelo parser WHATWG (crate `url`, já no lockfile pelo Tauri): `?` ou `#` sem codificar dentro da senha tornam a URL inválida, e a mensagem não cita a entrada.
- Onde a origem aparece (título e comandos do laudo, erro de abertura, aba do app) vai a URL limpa (`Pedido::exibir`: esquema, host, porta e caminho) ou o nome do arquivo (`Pedido::nome`), nunca a entrada crua.
- O baixado não entra nos recentes do app: é temporário, some ao fechar (o desvio é `entrada::temporario`, o mesmo nome que cobre o stdin).

**Dependência HTTP: ureq 3 sem recursos padrão, só `rustls`.** Bloqueante, sem tokio nem hyper: o núcleo já é síncrono e o download é um laço de leitura com cancelamento entre blocos. Vem com rustls + ring + webpki-roots (sem OpenSSL, sem depender do sistema). Recursos `gzip`, `cookies` e afins ficam desligados: o gzip já é nosso, e cookie não deve existir. reqwest puxaria tokio e hyper para fazer o mesmo pedido; chamar o `curl` do sistema seria executar um processo com o que o usuário colou, o que a decisão proíbe. `rustls` entra também como dependência direta só para reconhecer o erro de certificado que o ureq devolve dentro de um `io::Error`.

**Limites conhecidos:**
- Cancelar vale entre blocos de 256 KiB: um servidor que para de mandar só cai no teto de tempo.
- Proxy das variáveis de ambiente (`HTTPS_PROXY`…) é respeitado (padrão do ureq): é configuração do usuário, e o proxy vê o mesmo que veria de qualquer outro programa.
- Token no caminho da URL (`/api/<token>/dados`) aparece nas mensagens: só a consulta, a credencial e os cabeçalhos são limpos. Não há como saber que trecho do caminho é segredo.
- Sinal: o CLI liga `entrada::apagar_no_sinal` antes de baixar, então Ctrl-C e SIGTERM apagam o download (sai com 130). SIGKILL (inclusive o do supervisor do `consultar` ao estourar memória) e crash deixam o arquivo, destravado: `entrada::limpar_orfaos` o apaga no próximo `criar` (stdin ou download) ou na abertura do app, depois de 60 s.
- `--http` (como `--sem-mascara`) é tirado de qualquer posição da linha de comando: não dá para buscar o texto literal `--http` com `jasao buscar`.
- Começou em `http` (com aviso): a cadeia de saltos não é conferida (já era sem cifra).
- O progresso no app mostra MiB recebidos, sem porcentagem (o total nem sempre vem).

**Testado (servidor local, sem rede externa):** núcleo (`core/tests/url.rs`): 200 com JSON e com JSONL (User-Agent, sem cookie, 0600), gzip com `--compressed`, 404 com o segredo do `-H Authorization` ecoado na frase, no corpo e na consulta, e ausente do erro e do `Debug`, tempo esgotado, acima do teto com e sem `Content-Length`, `http` só com permissão, flag de cURL não suportada, `https`→`http` recusado (o servidor `http` não recebe nada), certificado autoassinado recusado com as raízes do sistema e aceito com a raiz do teste (gerada na hora pelo rcgen), e cancelamento; em todos os erros o cache fica sem parcial. Redirecionamentos: mesmo servidor mantém os cabeçalhos; para `localhost` (o primeiro é `127.0.0.1`) nenhum dos sensíveis chega e o comum sim; 307/308 com corpo para outro host recusado sem o outro receber nada; 5 saltos passam e o 6º falha. POST com `-d` e `-u`: corpo, `Content-Type` e Basic chegam, e nem o erro nem o `Debug` trazem senha, corpo ou base64. `usuario:senha@` vai como Basic e some do erro. Teto somando vários pedaços (chunked, sem `Content-Length`). Unidade (`url::testes`): cURL do navegador, recusas, `-u` vira Basic e some do `Debug`, `?`/`#` na senha, precedência de `-H`/`-u`/URL, `$'\xc3\xa9'` = `é`; o baixado é um temporário `download.*` e o `Drop` o apaga. CLI (`cli/tests/url.rs`): `--http` exigido e o aviso, segredo fora do erro, `-L` recusado; cada subcomando que aceita arquivo (linhas, janela, buscar, consultar, transformar, erros, inferir, exportar, validar com `--schema` e `--como`, diff, laudo), em 200 e 404, por URL com credencial e consulta e por cURL com cabeçalho, sem segredo em stdout nem stderr; o laudo traz a origem limpa; o download some ao terminar e no SIGTERM no meio (sai com 130). **Mutação:** sem `apagar_no_sinal` no `baixar` do CLI cai o do SIGTERM; sem o guarda `Baixados` no `main` cai o de terminar. App (`baixar_url_pelo_ipc`): `jasao:http` antes da confirmação, baixa e abre o concedido, nome da aba sem consulta, segredo fora do erro.

**Não conferido:** a interface (campo, confirmação, progresso) não foi aberta no app nesta fatia; a lógica de comando e erros está coberta pelo IPC.
