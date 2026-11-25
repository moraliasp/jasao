# Radar: Jasão × top 5

Placar vivo do Jasão contra o mercado. Atualizado em 24/11/2026, com `main` em 80e4967 (PRs #6 a #61 mergeados, com duas exceções: #14 fechado sem merge e #36 é issue). A fonte de cada concorrente está em [concorrencia.md](concorrencia.md).

**Premissa: o Jasão é gratuito.** A barra a bater são os pagos (Dadroit, OK JSON Pro), e o adversário de preço são os grátis (JSON Hero, quicktype, Compass). Nenhum deles entrega o pacote completo: gratuito, desktop, arquivo de 10 GB, estrutura, diff e export.

## Top 5

| # | Concorrente | Preço | Padrão que ele define |
|---|---|---|---|
| 1 | Dadroit | pago, valor não confirmado (`https://dadroit.com/pricing` dá 404 em 24/11/2026, assim como `/buy` e `/store`; terceiros se contradizem) | abrir arquivo gigante rápido |
| 2 | OK JSON | US$ 24,99 pagamento único, com teste grátis ([okjson.app](https://okjson.app), 24/11/2026) | acabamento nativo de macOS, jq/JSONPath, JSONL |
| 3 | JSON Hero | grátis, open source | navegação em colunas, busca, link compartilhável, inferência 2020-12 |
| 4 | quicktype | grátis, open source | tipos, Zod e Schema gerados a partir de amostras |
| 5 | MongoDB Compass (Schema) | grátis | presença e mistura de tipos por campo |

## Placar

✅ tem · ◐ parcial ou em PR · ❌ não tem · ? não confirmado

| Capacidade | Jasão | Dadroit | OK JSON | JSON Hero | quicktype | Compass |
|---|---|---|---|---|---|---|
| Abrir JSONL grande sem travar | ✅ índice + varredura paralela em 2º plano (#12) | ✅ | ◐ | ❌ | — | — |
| `.gz` / `.zst` | ✅ #4 | ? | ? | ❌ | — | — |
| Busca no arquivo inteiro | ✅ core (#5) e interface (#19) | ✅ | ✅ | ✅ | — | ◐ |
| Abas e arrastar vários arquivos | ✅ #20 | ? | ? | ❌ | — | ? |
| Árvore de valores | ✅ #6 | ✅ | ✅ | ✅ | — | ✅ |
| Texto cru sincronizado com a árvore | ✅ #9 | ? | ? | ◐ | — | — |
| Tabela por coluna | ✅ núcleo (#33) e interface (#34): colunas por caminho, ordenação completa em 2º plano, fixar e ocultar | ? | ✅ | ❌ | — | ✅ |
| Copiar caminho (jq, JSONPath, Pointer) | ✅ core e CLI (#7), interface (#19) | ✅ | ✅ | ✅ | — | — |
| Linha inválida sem quebrar o resto | ✅ #7, #12 | ? | ? | ❌ | — | — |
| Consulta jq | ✅ core e CLI (#10), interface (#19), consulta ao vivo, filtros salvos e exportar JSONL/CSV (#47) | ◐ JSONPath | ✅ | ❌ | — | ✅ (consulta Mongo) |
| Valores inteligentes (data, JWT, base64) | ✅ #40: selo na árvore e painel do valor com data (época e ISO), cor, URL que só abre com clique, imagem base64, JWT decodificado sem a assinatura e JSON dentro de texto | ? | ? | ✅ prévias | — | — |
| Visão do arquivo inteiro (régua por variante e inválidas) | ✅ #16 | ? | ❌ | ❌ | — | ❌ |
| Resumo ao abrir (painel de entrada e laudo) | ✅ painel (#12), laudo no painel e em `jasao laudo` (#52) | ? | ? | ❌ | — | ◐ |
| **Presença e evidência por campo** | ✅ core e CLI (#8), modo Estrutura na interface (#21): tipos e "N de M" por caminho; avisos, estatísticas por campo e registros que fogem (#43) | ❌ | ❌ | ◐ | ◐ | ✅ só no Mongo |
| **União discriminada** | ✅ core (#8) e interface (#21): uma linha por variante, Enter abre o 1º registro; filtrar a Tabela por variante ou caminho (#49) | ❌ | ❌ | ◐ anyOf | ◐ | ❌ |
| **Diff estrutural: quebra × compatível** (diff de valores à parte, ver nota) | ✅ core e CLI (#39), interface com diff de valores (#42): `jasao diff`, taxonomia no padrão oasdiff/buf, na dúvida é quebra | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Zod + Schema validados no arquivo** | ✅ core e CLI (#32), interface (#35): Zod 4 e Schema 2020-12 com união discriminada, conferidos registro a registro, 1 GB em 1–3 s; tipos TypeScript e `jasao validar` contra estrutura salva ou schema de fora (#45) | ❌ | ❌ | ◐ só Schema | ◐ sem validar | ◐ só Schema |
| Gratuito | ✅ | ❌ | ◐ | ✅ | ✅ | ✅ |

**Nota sobre o diff.** O eixo conta só o diff **estrutural** que separa quebra de mudança compatível; aí nenhum dos 5 (nem os de fora do radar) tem equivalente. Diff de **valores** entre dois JSON já existe de graça em jd, jsondiffpatch e Graphtage ([concorrencia.md](concorrencia.md), linha 27); o Jasão tem os dois (#42), mas só o estrutural é diferencial.

## Como o placar é contado

13 eixos, cada um de 0 a 2: ✅ = 2, ◐ = 1, ❌, `—` e `?` = 0 (sem fonte não pontua). Máximo 26. Linhas da tabela acima que **não** entram na soma: abas, linha inválida, visão do arquivo inteiro e resumo ao abrir (não há fonte comparável para todos os 5); "Árvore e texto" usa a linha da árvore, porque o texto cru dos concorrentes está quase todo em `?`. Arquivo vivo (#44) e formatos de entrada (#46) ficam fora até haver fonte dos concorrentes.

| Eixo | Jasão 22/11 | Jasão hoje | Dadroit | OK JSON | JSON Hero | quicktype | Compass |
|---|---|---|---|---|---|---|---|
| Arquivo grande | 2 | 2 | 2 | 1 | 0 | 0 | 0 |
| `.gz` / `.zst` | 2 | 2 | 0 | 0 | 0 | 0 | 0 |
| Busca | 2 | 2 | 2 | 2 | 2 | 0 | 1 |
| Árvore e texto | 2 | 2 | 2 | 2 | 2 | 0 | 2 |
| Tabela | 0 | 2 | 0 | 2 | 0 | 0 | 2 |
| Copiar caminho | 2 | 2 | 2 | 2 | 2 | 0 | 0 |
| Consulta jq | 2 | 2 | 1 | 2 | 0 | 0 | 2 |
| Valores inteligentes | 0 | 2 | 0 | 0 | 2 | 0 | 0 |
| Estrutura e presença | 2 | 2 | 0 | 0 | 1 | 1 | 2 |
| União discriminada | 2 | 2 | 0 | 0 | 1 | 1 | 0 |
| Diff quebra × compatível | 0 | 2 | 0 | 0 | 0 | 0 | 0 |
| Export Zod + Schema | 0 | 2 | 0 | 0 | 1 | 1 | 1 |
| Gratuito | 2 | 2 | 0 | 1 | 2 | 2 | 2 |
| **Total (de 26)** | **18** | **26** | **9** | **12** | **13** | **5** | **12** |

**26 de 26 é teto do placar, não do produto.** O placar mede o que os top 5 definiram. A seção abaixo mostra o que ficou fora dele.

## Fora do radar até hoje

Rodada ampliada de 24/11/2026: só páginas oficiais, README, documentação e API de releases, sem baixar binário de ninguém. Cada linha diz o que a ferramenta faz e o Jasão **não** faz. "Não confirmado" quando a página não diz.

### Terminal e consulta

| Ferramenta | Última versão | O que tem e o Jasão não |
|---|---|---|
| jless | v0.9.0, 17/07/2023 ([release](https://github.com/PaulJuliusMartinez/jless/releases/tag/v0.9.0)) | navegação estilo vim com contagem (`5j`, `Ctrl-d`), recolher/expandir irmãos de uma vez, busca `/` e `?` com smart case, lê stdin ([guia](https://jless.io/user-guide), [README](https://github.com/PaulJuliusMartinez/jless)) |
| fx | 40.0.2, 20/11/2026 ([release](https://github.com/antonmedv/fx/releases/tag/40.0.2)) | consulta em JavaScript, autocomplete de caminho e consulta, entrada XML/HTML/EDN, stream de valores por stdin, completion de shell ([fx.wtf](https://fx.wtf)) |
| jq | 1.8.2, 20/06/2026 ([release](https://github.com/jqlang/jq/releases/tag/jq-1.8.2)) | `--stream` para um JSON único gigante, `--seq`, precisão decimal de literais, módulos `import`/`include` ([manual](https://jqlang.org/manual/)) |
| gojq | v0.12.20, 01/11/2026 ([release](https://github.com/itchyny/gojq/releases/tag/v0.12.20)) | inteiros de precisão arbitrária, saída YAML, erro com posição na consulta ([README](https://github.com/itchyny/gojq)) |
| jaq | v3.2.0, 19/11/2026 ([release](https://github.com/01mf02/jaq/releases/tag/v3.2.0)) | entrada XML (YAML/CBOR/TOML o Jasão já lê, #46), benchmark publicado contra jq e gojq ([README](https://github.com/01mf02/jaq)). É o motor que o Jasão usa (`jaq-core`, ver decisoes.md) |
| lnav | v0.14.1, 05/09/2026 ([release](https://github.com/tstack/lnav/releases/tag/v0.14.1)) | SQL (SQLite) sobre logs, vários arquivos em ordem cronológica, pular entre erros (`e`/`E`), histograma no tempo, SSH/`docker://`/URL, marcações e notas por linha com sessão salva ([README](https://github.com/tstack/lnav), [uso](https://docs.lnav.org/en/latest/usage.html)) |

### Tabela, SQL e logs

| Ferramenta | Última versão / preço | O que tem e o Jasão não |
|---|---|---|
| VisiData | v3.5, ano não confirmado ([releases](https://github.com/saulpw/visidata/releases)); GPLv3 | frequência, agrupamento e pivot ([group](https://www.visidata.org/docs/group/)), gráfico no terminal ([graph](https://www.visidata.org/docs/graph/)), join entre arquivos ([join](https://www.visidata.org/docs/join/)), plugins ([plugins](https://www.visidata.org/docs/plugins/)), macros e sessão ([docs](https://www.visidata.org/docs/)) |
| Gigasheet | SaaS, preço não confirmado ([pricing](https://www.gigasheet.com/pricing)) | achata JSON aninhado em linhas e anuncia 1 bilhão de linhas, pivot no navegador ([MRF Viewer](https://www.gigasheet.com/products/mrf-viewer)), compartilhar por link ([colaboração](https://www.gigasheet.com/post/team-collaboration)). O site hoje vende preço de saúde e "AI Agent" ([home](https://www.gigasheet.com/)): perdeu foco de visualizador |
| Tad | 0.14.0, 21/06/2024 ([releases](https://github.com/antonycourtney/tad/releases)); MIT | pivot sobre DuckDB, histograma por coluna numérica, abre Parquet/SQLite/DuckDB e exporta Parquet ([tadviewer.com](https://www.tadviewer.com/)) |
| DuckDB | v2.0.1, 20/11/2026 ([releases](https://github.com/duckdb/duckdb/releases)); grátis | SQL sobre NDJSON com `read_json`, glob de vários arquivos, `union_by_name`, gz/zstd ([loading_json](https://duckdb.org/docs/2.0/data/json/loading_json.html)), leitura por httpfs/S3 ([json](https://duckdb.org/docs/stable/data/json/overview)), UI local de notebooks `duckdb -ui` ([ui](https://duckdb.org/docs/2.0/core_extensions/ui.html)) |
| Datasette | 1.0a43 e 0.65.6, 15/11/2026 ([datasette.io](https://datasette.io/)); grátis | SQL e API JSON, facets de contagem por valor, item de array e data ([facets](https://docs.datasette.io/en/stable/facets.html)), `datasette publish`, 153 plugins ([datasette.io](https://datasette.io/)), importa NDJSON com `--flatten` ([sqlite-utils](https://sqlite-utils.datasette.io/en/stable/cli.html)) |
| OpenObserve | v1.2.0, ano não confirmado ([releases](https://github.com/openobserve/openobserve/releases)); AGPL, nuvem US$ 0,50/GB ([pricing](https://openobserve.ai/pricing/)) | SQL e full-text sobre logs, construtor visual de consulta, dashboards, alertas ([GitHub](https://github.com/openobserve/openobserve)) |
| Grafana Loki | v3.7.10, 13/11/2026 ([releases](https://github.com/grafana/loki/releases)); AGPL-3.0 | LogQL com `\| json` e métricas derivadas dos logs ([query](https://grafana.com/docs/loki/latest/query/)). Indexa só labels, sem full-text ([README](https://github.com/grafana/loki)): o oposto do Jasão |
| Datadog Log Explorer | US$ 0,10/GB ingerido + US$ 1,70/milhão indexados ([pricing](https://www.datadoghq.com/pricing/?product=log-management)) | facets, agrupar por campo, série temporal, Saved Views, Live Tail ([explorer](https://docs.datadoghq.com/logs/explorer/)), Patterns que agrupam mensagens parecidas ([patterns](https://docs.datadoghq.com/logs/explorer/analytics/patterns/)) |

### Navegador e editores

| Ferramenta | O que tem e o Jasão não |
|---|---|
| JSON Crack / ToDiagram | grafo visual que se atualiza ao digitar, exporta PNG/JPEG/SVG, abre de URL, gera schema e mock ([jsoncrack.com](https://jsoncrack.com)); compartilhar diagramas, API e MCP ([pricing](https://todiagram.com/pricing)); extensão VS Code com ~798 mil instalações ([marketplace](https://marketplace.visualstudio.com/items?itemName=AykutSarac.jsoncrack-vscode)). Limite declarado no site de uns 300 KB: não compete em arquivo grande |
| JSON Formatter | formata sozinho qualquer resposta JSON aberta no navegador, `window.json` no DevTools; ~2 milhões de usuários, nota 1,5 ([Chrome Web Store](https://chromewebstore.google.com/detail/json-formatter/bcjindcccaagfpapjjmafapmmgkkhgoa)) |
| JSONView | o mesmo em Firefox, Chrome e Edge, 21 idiomas ([Chrome Web Store](https://chromewebstore.google.com/detail/jsonview/gmegofmjomhknnokphhckolhcffdaihd), [GitHub](https://github.com/bhollis/jsonview)) |
| VS Code | valida contra schema no editor via `$schema`/`json.schemas` e baixa do SchemaStore ([docs](https://code.visualstudio.com/docs/languages/json), [package.json](https://github.com/microsoft/vscode/blob/main/extensions/json-language-features/package.json)). Limite: outline de 5.000 símbolos (`json.maxItemsComputed`, mesma fonte); JSONL só por extensão ([vscode-json-validate](https://github.com/rioj7/vscode-json-validate)) |
| Sublime + Pretty JSON | formatar, minificar, validar com salto ao erro, jq, JSON→XML; ~1,4 milhão de instalações, sem atualização há ~2 anos ([Package Control](https://packagecontrol.io/packages/Pretty%20JSON)). Arquivo grande: sem dado oficial |

### Novidades do top 5

- **OK JSON** ([App Store](https://apps.apple.com/app/ok-json-viewer-formatter/id1576121509), [okjson.app](https://okjson.app), [docs](https://docs.okjson.app)): 3.0 (editor redesenhado, consultas salvas, sem analytics de crash), 3.1.1 (macOS 27, temas), 3.1.2 (correções); ano de parte das datas não aparece na página. Faz e o Jasão não: **editar e salvar no arquivo**, **reparar JSON quebrado**, AppleScript, Raycast, URL schemes, **Quick Look no Finder**, scripts em JavaScript, 12 idiomas e acessibilidade declarada (VoiceOver, contraste, movimento reduzido).
- **Dadroit** ([release notes](https://dadroit.com/release-notes/), [dadroit.com](https://dadroit.com)): 3.3.0 (14/07/2025, Windows ARM) e 3.4.0 (03/10/2026, build Apple Silicon). Faz e o Jasão não: **juntar vários arquivos num conjunto e buscar entre eles**, exportar nó ou arquivo para CSV/XML.

O top 5 fica como está: ele mede o visualizador desktop de JSON, e nenhum dos de fora é isso. Mas três deles definem expectativas que o placar não mede: **DuckDB** (SQL e agregação, grátis e local), **lnav** (log de verdade: vários arquivos, tempo, erros) e **JSON Crack** (o visual que as pessoas associam a "visualizar JSON"). Ficam como vigiados fixos.

## Esquecemos algo?

Conferido contra `docs/plano.md`, `docs/decisoes.md`, `docs/concorrencia.md` e o código na `main` (80e4967).

| Expectativa | Quem cria | Situação no Jasão |
|---|---|---|
| Abrir de stdin (`cat x \| jasao`) | jless, fx, jq, lnav | ✅ #55: `-` lê stdin em todo subcomando do CLI e no app (1ª instância), gravando num temporário 0600 apagado ao sair (`decisoes.md`, "stdin por temporário") |
| Abrir de URL / colar cURL | OK JSON, Dadroit, JSON Crack, lnav | ✅ #57: CLI aceita URL/cURL no lugar do arquivo, app tem "Abrir URL…" (decisões em `decisoes.md`, "Abrir de URL e colar cURL") |
| S3 e remoto (SSH) | DuckDB, lnav | ❌ não aparece em nenhum doc nem mockup |
| Assinatura e notarização do Mac | (pré-requisito de distribuir) | ❌ nenhuma menção em docs nem config; sem isso o Gatekeeper barra o download gratuito |
| Atualização automática | OK JSON (App Store) | ❌ nenhuma decisão em `decisoes.md`/`plano.md` |
| Agrupar e contar por campo, histograma | Datasette, Tad, VisiData, Datadog | ◐ contagem por valor ("Mais comuns") e faixas de número com barras no modo Estrutura (#43); clicar num valor ou faixa filtra a Tabela (#56, #61). Falta gráfico de datas no tempo |
| SQL sobre o arquivo | DuckDB, Datasette, lnav | ❌ fora do plano; `concorrencia.md` cita DuckDB como "faça você mesmo" |
| Formatar, minificar, editar e salvar | OK JSON, Pretty JSON, VS Code | ◐ "Salvar como" grava arquivo novo em JSONL ou JSON formatado/compacto (#54); editar no lugar segue fora (`plano.md`, princípio 5) |
| Reparar JSON quebrado | OK JSON | ◐ linha inválida não quebra o resto (#7, #12); não conserta |
| Vários arquivos como um conjunto | Dadroit, lnav, DuckDB | ◐ abas (#20) e linha do tempo de pasta (#51); sem busca entre arquivos |
| Quick Look no Finder | OK JSON | ❌ nenhuma menção |
| Grafo visual e exportar imagem | JSON Crack | ❌ `concorrencia.md` cita, sem decisão |
| Comparar dois arquivos | jd, jsondiffpatch, Graphtage | ✅ #39, #42 |
| Schema inferido e validação | VS Code, JSON Crack, quicktype | ✅ #32, #35, #45 |
| NDJSON comprimido | DuckDB | ✅ #4 |
| Busca regex | Dadroit, jless | ✅ na CLI (`buscar --regex`); interface não conferida nesta rodada |
| Atalhos de teclado e acessibilidade | jless (vim), OK JSON | ✅ atalhos e ARIA na árvore (`decisoes.md`, "Erro de registro, idioma e acessibilidade"); sem atalhos vim |
| Idiomas | OK JSON (12), JSONView (21) | ◐ pt-BR e inglês pelo sistema (`app/src/i18n.ts`) |
| Modo escuro | todos | ✅ segue o sistema (`prefers-color-scheme` em `app/src/main.ts`); sem decisão escrita nem seletor |
| Arrastar e "Abrir com" | todos os desktop | ✅ `decisoes.md` (Arrastar, Associação) |
| Plugins | VisiData, Datasette | ❌ fora; não recomendo (superfície de ataque num app que promete privacidade) |
| Compartilhar por link | JSON Hero, Gigasheet, ToDiagram | ❌ e deve continuar: contraria "nada sobe" |

## Fatias sugeridas (impacto ÷ esforço)

1. ✅ **`jasao/abertura/stdin`** (feito, #55): CLI lê `-` e pipe; o app aceita `jasao abrir -` gravando num temporário. Impacto alto para dev, esforço pequeno (o núcleo já indexa arquivo; é um temporário).
2. **`jasao/distribuicao/notarizacao`**: Developer ID, `hardenedRuntime`, notarização no CI de release, com decisão escrita. Impacto alto (sem isso o "grátis" não instala sem susto), esforço pequeno em código, depende de conta Apple do Eduardo.
3. ✅ **`jasao/estrutura/facetas-clique`** (feito, #56 e #61): clicar num valor de "Mais comuns" ou numa faixa de número no modo Estrutura filtra a Tabela por ele (a contagem e as faixas já existem em #43; o filtro da Tabela já existe em #49). Impacto médio, esforço pequeno: é ligar dois pedaços prontos. Gráfico de datas no tempo fica para depois, se aparecer pedido.
4. ✅ **`jasao/abertura/url`** (feito, #57): abrir `https://…` e colar cURL, baixando para o cache que o `.gz` já usa, só por ação explícita e sem enviar nada além do pedido. Impacto alto (OK JSON e Dadroit têm), esforço médio; precisa decisão de privacidade antes.
5. **`jasao/radar/benchmark-dadroit`**: o gap 1 de sempre, só leitura de páginas do Dadroit e medição nossa.
6. **`jasao/macos/quicklook`**: prévia no Finder com o laudo (#52). Impacto médio, esforço médio.
7. **`jasao/abertura/reparar`**: sugerir conserto de linha inválida (vírgula sobrando, aspas) sem gravar no original. Impacto médio, esforço médio.

Fora de propósito, registrados para não voltar: plugins, compartilhar por link, editar no lugar, SQL embutido (o caminho barato é "copiar comando DuckDB" se aparecer pedido).

## Gaps, em ordem

1. **Benchmark público de 1 GB contra o Dadroit.** O #38 mediu os tetos internos (1ª tela 0,28 s, índice 0,29 s, estrutura 3,41 s, busca 0,59–0,73 s, ≤63 MB além do mmap), mas o Dadroit não foi comparado e nada foi publicado. É o único argumento que o pago tem e que o Jasão pode tirar de graça.
2. **1ª tela no limite de 300 ms** (0,28 s no #38): qualquer regressão estoura a meta; a checagem de 10% no CI foi proposta e ainda não está ligada.

## Candidatos a "muito especial", de graça

- **Laudo do arquivo ao abrir.** ✅ mergeado (#52). Antes de qualquer árvore, o painel diz o que o arquivo é, os formatos por discriminador, os campos opcionais e os avisos, com passos clicáveis; `jasao laudo` exporta em Markdown. Nenhum dos 5, nem os de fora do radar, abre por aí (ver "Laudo ao abrir" em `decisoes.md`).
- **Régua do arquivo inteiro (#16).** Minimapa com cor por variante e linhas inválidas; nenhum dos 5 mostra o arquivo assim.
- **`jasao diff --falha-em quebra` no CI.** Já existe na CLI (#39), com códigos de saída para o pipeline. O oasdiff faz isso para OpenAPI; ninguém faz para dados reais.
- **Privacidade como recurso.** Tudo roda local, nada sobe, e o que parece segredo é sinalizado. O JSON Hero é web; o Jasão ganha quando o dado é sensível.

## Histórico

- 21/11/2026: radar criado. Core com índice de linhas, `.gz`/`.zst` e busca. Em PR: árvore (#6), caminhos e linhas inválidas (#7), estrutura (#8), texto (#9) e jq (#10).
- 22/11/2026: mergeados #6 a #10, #12 e #16. Novo: árvore, texto, painel de entrada e régua na interface; caminho, jq, estrutura e união discriminada no core e na CLI. CI passou a rodar em runner próprio (#15). Gap 1: busca, caminho e jq na interface.
- 22/11/2026 (tarde): busca (⌘F, contagem, marcas na régua), copiar caminho (menu e atalhos) e consulta jq (⌘J, resultado no visualizador, volta ao arquivo) em PR. Sai o antigo gap 1; benchmark contra o Dadroit vira o primeiro.
- 22/11/2026 (noite): abas de arquivo (estado por documento no Rust e no front, ⌘T/⌘W/⌘1…9/Ctrl+Tab) e arrastar vários arquivos para a janela em PR. Os exemplos embutidos viram duas abas.
- 22/11/2026: em PR, recentes (os 10 últimos, na aba vazia e no menu Arquivo) e associação de `.json`/`.jsonl`/`.ndjson` como visualizador alternativo, com abertura pelo Finder e instância única. Fica fora do placar, que compara visualização e não integração com o sistema.
- 22/11/2026 (fim do dia): mergeados #18 a #22 (exemplos, busca/caminho/jq na interface, abas, Estrutura, recentes). Presença e união discriminada passam a ✅ na interface; o Jasão fecha 18 de 26 pontos no radar 0–2, contra 13 do JSON Hero. Em PR: tetos de memória da árvore (#23). Gap 1 agora é a tabela por coluna.
- 23/11/2026: modo Tabela em PR (núcleo #33, interface empilhada): colunas por caminho, janela virtual do Rust, ordenação completa em 2º plano com progresso e cancelar, fixar e ocultar. Tabela por coluna passa a ✅ quando mergear. Gap 1 volta a ser o benchmark contra o Dadroit.
- 23/11/2026 (tarde): mergeados #23 e #25 (tetos de memória), #26 a #31 (segurança e CI) e #32 (export Zod e Schema no core e na CLI). Em PR: interface do export (#35).
- 24/11/2026: mergeados #33 e #34 (Tabela), #35 (Exportar na interface), #38 (benchmarks internos de 1 GB) e #39 (diff estrutural no core e na CLI). Tabela e Zod + Schema passam a ✅, diff a ◐; o Jasão vai a 23 de 26 pontos com o que está na `main`. Em PR: valores inteligentes (#40).
- 24/11/2026 (noite): mergeado #40 (valores inteligentes na árvore e no painel do valor, tudo local: o link só abre com clique e o JWT nunca é conferido nem enviado). Valores inteligentes passa a ✅ e empata com as prévias do JSON Hero. Recontagem eixo a eixo: 25 de 26 pontos, e o único ponto aberto é o diff estrutural sem interface; as somas de 22/11 (20) e 24/11 (25) foram corrigidas para 18 e 23 nesta recontagem.
- 24/11/2026 (tarde): mergeados #44 (seguir arquivo vivo, como `tail -f`), #45 (tipos TypeScript e `jasao validar` contra estrutura salva ou JSON Schema de fora, com `$ref` só local e custo limitado) e #46 (MessagePack, CBOR, BSON, YAML e TOML como entrada). O export fica mais largo, mas o placar não muda: 25 de 26. Arquivo vivo e formatos ficam fora do placar até haver fonte do que os concorrentes fazem.
- 24/11/2026 (fim do dia): mergeados #42 (interface do diff estrutural e diff de valores), #43 (avisos, estatísticas por campo e registros que fogem), #47 (jq ao vivo, filtros salvos, exportar JSONL/CSV) e #49 (filtrar a Tabela por variante ou caminho). Diff passa a ✅ e o Jasão fecha 26 de 26 pontos; o eixo do diff segue sem nenhum concorrente. Gap 1 continua o benchmark público contra o Dadroit. Em PR: máscara de segredos (#50), linha do tempo de pasta (#51) e laudo ao abrir (#52).
- 24/11/2026 (rodada ampliada): mergeados #52 (laudo ao abrir, no painel e em `jasao laudo`, 7b9b6c7), #50 (máscara de segredos e anonimização, 1aa6fee) e o radar anterior #48 (7d00a59). Placar declarado eixo a eixo (13 eixos, 0 a 2) para ser recontável; segue 26 de 26. Diff passa a dizer só "estrutural" no diferencial: diff de valores já existe em jd, jsondiffpatch e Graphtage. Varredura fora do top 5 (19 ferramentas, só páginas oficiais): o que mais falta é abrir de stdin e URL, notarização do Mac e filtrar clicando na contagem por campo. Preço do OK JSON corrigido para US$ 24,99 único; o do Dadroit fica não confirmado.
- 24/11/2026 (noite, 2): mergeados #54 (salvar como), #55 (stdin), #56 e #61 (facetas clicáveis filtram a Tabela), #57 (abrir de URL e colar cURL, só por ação explícita), #58 (licença MIT), #60 (fuzz) e o README #59. Placar segue 26 de 26: nenhum dos 13 eixos muda. Fora do placar, saem da lista de "faz e o Jasão não" o abrir de URL/cURL (OK JSON) e a importação por URL com autenticação (Dadroit, via `-H`/`-u` do cURL). Das fatias sugeridas, 1, 3 e 4 estão feitas; a próxima de maior impacto é a notarização do Mac, que depende da conta Apple.
