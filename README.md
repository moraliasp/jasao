# Jasão

Visualizador de JSON e JSONL para arquivos de qualquer tamanho. Gratuito, somente leitura, para macOS, Windows e Linux (Tauri 2 + Rust), em português e inglês.

Abrir e navegar é o básico, e nisso ele concorre com os pagos. O que só ele faz é explicar o arquivo: a estrutura que os registros realmente têm, o que mudou entre duas versões e se a mudança quebra quem consome, e o schema pronto (Zod, JSON Schema, TypeScript) já conferido contra cada registro.

## Em números

JSONL de 1 GB com 6,46 milhões de registros, Apple M5 Pro, arquivo no cache do sistema (detalhes em [docs/decisoes.md](docs/decisoes.md)):

| O quê | Medido |
|---|---|
| Abrir no app | 221 ms, com 99 MB de memória própria |
| Índice de linhas | 85 ms por GB |
| Buscar texto no arquivo inteiro | 15 ms literal, 17 ms regex |
| Validar o arquivo contra o schema gerado | 1 a 3 s |

Leitura a frio do disco não foi medida. O alvo do plano é abrir e navegar até 10 GB.

## O que ele faz

### Abrir

- **Arquivo grande sem travar.** O arquivo fica mapeado no Rust e a tela pede só a janela visível.
- **`.gz` e `.zst`** descompactados em streaming. Também abre MessagePack, CBOR, BSON, YAML e TOML.
- **Abas, arrastar e soltar, recentes, associação de arquivo.** Associa `.json`, `.jsonl` e `.ndjson` como visualizador alternativo, sem tomar o app padrão do sistema.
- **Linha inválida não derruba o resto.** Ela aparece na lista com posição e motivo.
- **Laudo ao abrir.** Um resumo do arquivo no painel e em `jasao laudo`.
- **Arquivo vivo.** Seguir/Pausar como `tail -f`; se o arquivo for truncado, rotacionado ou reescrito, reabre com aviso.

### Ver

- **Árvore** virtualizada, com migalhas do caminho.
- **Texto cru** com realce, sincronizado com a árvore.
- **Tabela.** JSONL e listas de objetos viram colunas por caminho. Ordenar milhões de linhas leva cerca de 1 s. Dá para fixar e ocultar colunas e filtrar por campo ou por variante.
- **Régua** do arquivo inteiro, por variante e com as linhas inválidas.
- **Valores inteligentes:**
  - época e data ISO viram data;
  - cor mostra amostra;
  - URL só abre com clique;
  - imagem em base64 tem prévia;
  - JWT aparece decodificado, sem a assinatura;
  - JSON dentro de texto expande.
- **Máscara de segredos.** Ligada por padrão na tela e em toda saída; "Mostrar valor" campo a campo.

### Achar

- **Busca** por texto ou regex no arquivo inteiro, com contagem e navegação.
- **Copiar caminho** em jq, JSONPath, JSON Pointer e JavaScript; valor cru ou formatado.
- **Consulta jq** ao vivo (motor `jaq`), com filtros salvos e exportação do resultado em JSONL ou CSV.

### Entender (o que ninguém mais entrega)

- **Estrutura e presença.**
  - Tipo, formato e "N de M registros" por caminho, com evidência.
  - Avisos: número como texto, data fora do ISO, campo que parece segredo.
  - Estatísticas por campo e lista dos registros que fogem do padrão.
- **União discriminada.** Uma linha por variante (`.tipo = "X"`), e Enter abre o primeiro registro daquela variante.
- **Diff estrutural, quebra × compatível.**
  - Compara duas versões do arquivo e classifica cada mudança no padrão oasdiff/buf. Na dúvida, conta como quebra.
  - Também faz o diff de valores e monta a linha do tempo de uma pasta de arquivos datados.
- **Exportar e validar.**
  - Zod 4, JSON Schema 2020-12 e tipos TypeScript, conferidos registro a registro antes de sair.
  - Valida outro arquivo contra a estrutura salva ou contra um schema de fora.
- **Salvar como.** Grava um arquivo novo com o arquivo inteiro, o resultado do jq ou o filtro da Tabela:
  - mantendo, tirando ou renomeando campos;
  - em JSONL ou JSON;
  - mascarado por padrão.

  O arquivo aberto nunca é alterado.

## Comparativo

Placar do radar em 24/11/2026: 13 eixos, de 0 a 2 pontos cada. As fontes de cada célula estão em [docs/radar.md](docs/radar.md) e [docs/concorrencia.md](docs/concorrencia.md). Sem fonte, a célula não pontua.

| | Jasão | Dadroit | OK JSON | JSON Hero | quicktype | MongoDB Compass |
|---|---|---|---|---|---|---|
| Abrir JSONL grande | ✅ | ✅ | ◐ | ❌ | — | — |
| `.gz` / `.zst` | ✅ | ? | ? | ❌ | — | — |
| Busca no arquivo inteiro | ✅ | ✅ | ✅ | ✅ | — | ◐ |
| Árvore | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| Tabela por coluna | ✅ | ? | ✅ | ❌ | — | ✅ |
| Copiar caminho | ✅ | ✅ | ✅ | ✅ | — | — |
| Consulta | ✅ jq | ◐ JSONPath | ✅ | ❌ | — | ✅ Mongo |
| Valores inteligentes | ✅ | ? | ? | ✅ | — | — |
| Estrutura e presença por campo | ✅ | ❌ | ❌ | ◐ | ◐ | ✅ só no Mongo |
| União discriminada | ✅ | ❌ | ❌ | ◐ | ◐ | ❌ |
| Diff quebra × compatível | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Zod + Schema validados no arquivo | ✅ | ❌ | ❌ | ◐ só Schema | ◐ sem validar | ◐ só Schema |
| Preço | grátis | pago, valor não confirmado | US$ 24,99 pagamento único | grátis | grátis | grátis |
| **Pontos (de 26)** | **26** | **9** | **12** | **13** | **5** | **12** |

✅ tem · ◐ parcial · ❌ não tem · — fora da proposta · ? não confirmado

26 é o teto do placar, não do produto: o placar mede o que os cinco concorrentes definiram.

- **Diff de valores** existe de graça em jd, jsondiffpatch e Graphtage. O diferencial é o diff estrutural que separa quebra de mudança compatível.
- **Fora do top 5:** ferramentas de terminal (jless, fx, jq), de tabela e SQL (VisiData, DuckDB) e de navegador (JSON Crack). Estão em [docs/radar.md](docs/radar.md), "Fora do radar até hoje".

## Linha de comando

O mesmo núcleo do app, para scripts e CI. Por padrão, os segredos saem mascarados (`--sem-mascara` mostra).

```bash
jasao laudo dados.jsonl                                   # resumo do arquivo
jasao buscar dados.jsonl 'pedido-42' --ignorar-caixa
jasao jq '.cliente.id' dados.jsonl --saida csv
jasao inferir dados.jsonl --json                          # estrutura por caminho
jasao exportar --zod dados.jsonl > schema.ts              # ou --schema, --ts
jasao validar novos.jsonl --schema schema.json            # 0 passa · 1 algum falha
jasao diff ontem.jsonl hoje.jsonl --falha-em quebra       # reprova o CI se quebrar
jasao linha-do-tempo pasta-de-dumps/
jasao transformar dados.jsonl -o limpo.jsonl --tirar senha,token
```

`jasao` sem argumentos lista todos os comandos e os códigos de saída.

## Desenvolvimento

Plano em [docs/plano.md](docs/plano.md); decisões e medidas em [docs/decisoes.md](docs/decisoes.md); como contribuir em [CONTRIBUTING.md](CONTRIBUTING.md).

| Pasta | O que é |
|---|---|
| `core/` | biblioteca Rust (pacote `nucleo`), sem Tauri: leitura e índices |
| `cli/` | linha de comando fina sobre o `core` |
| `app/` | casca Tauri 2 + Vite + TypeScript sem framework (`app/src-tauri` é o crate Rust) |

### Rodar

```bash
cargo run --release -p nucleo --example gerar -- /tmp/amostra.jsonl 100 1   # 100 MB, semente 1
cargo run --release -p cli -- linhas /tmp/amostra.jsonl
cargo run --release -p cli -- janela /tmp/amostra.jsonl 0 5
cd app && pnpm install && pnpm tauri dev -- -- /tmp/amostra.jsonl          # o caminho é opcional
```

`-` no lugar de um arquivo lê stdin (um `-` por execução; sem `-`, stdin não é lido; arquivo chamado `-`: `./-`). No app, `-` abre stdin numa aba, se o app ainda não estava aberto:

```bash
curl -s https://exemplo.test/eventos.jsonl | jasao laudo -
cat eventos.jsonl | jasao jq '.tipo' -
cat eventos.jsonl | caminho/do/binario/do/app -
```

### Testar

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # precisa de app/dist: rode `pnpm build` em app/ antes
cargo test --workspace
```

Fuzz (nightly + `cargo install cargo-fuzz`):

```bash
cd core/fuzz && cargo +nightly fuzz run indice_linhas --fuzz-dir . -- -max_total_time=30
```

### Medir

```bash
cargo bench -p nucleo --bench indice              # NUCLEO_BENCH_MB=1024 para 1 GB
```

### Nome

"Jasão" na tela; `jasao`, sem acento, em identificadores. Mora em dois arquivos, e só neles:

- `app/src-tauri/tauri.conf.json`: `productName` (nome do app e título da janela) e `identifier` (bundle);
- `cli/Cargo.toml`: `[[bin]] name` (binário de linha de comando).

Pacotes e pastas têm nomes neutros e não mudam.
