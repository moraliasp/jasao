# Radar: Jasão × top 5

Placar vivo do Jasão contra o mercado. Atualizado em 24/11/2026, com `main` em 0f4af8a (#6 a #46 mergeados). A fonte de cada concorrente está em [concorrencia.md](concorrencia.md).

**Premissa: o Jasão é gratuito.** A barra a bater são os pagos (Dadroit, OK JSON Pro), e o adversário de preço são os grátis (JSON Hero, quicktype, Compass). Nenhum deles entrega o pacote completo: gratuito, desktop, arquivo de 10 GB, estrutura, diff e export.

## Top 5

| # | Concorrente | Preço | Padrão que ele define |
|---|---|---|---|
| 1 | Dadroit | pago (~$98/ano, não confirmado) | abrir arquivo gigante rápido |
| 2 | OK JSON | grátis + Pro $17,99 | acabamento nativo de macOS, jq/JSONPath, JSONL |
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
| Consulta jq | ✅ core e CLI (#10), interface (#19) | ◐ JSONPath | ✅ | ❌ | — | ✅ (consulta Mongo) |
| Valores inteligentes (data, JWT, base64) | ✅ #40: selo na árvore e painel do valor com data (época e ISO), cor, URL que só abre com clique, imagem base64, JWT decodificado sem a assinatura e JSON dentro de texto | ? | ? | ✅ prévias | — | — |
| Visão do arquivo inteiro (régua por variante e inválidas) | ✅ #16 | ? | ❌ | ❌ | — | ❌ |
| Resumo ao abrir (painel de entrada) | ✅ #12 | ? | ? | ❌ | — | ◐ |
| **Presença e evidência por campo** | ✅ core e CLI (#8), modo Estrutura na interface (#21): tipos e "N de M" por caminho | ❌ | ❌ | ◐ | ◐ | ✅ só no Mongo |
| **União discriminada** | ✅ core (#8) e interface (#21): uma linha por variante, Enter abre o 1º registro; falta filtro por variante | ❌ | ❌ | ◐ anyOf | ◐ | ❌ |
| **Diff estrutural: quebra × compatível** | ◐ core e CLI (#39): `jasao diff`, taxonomia no padrão oasdiff/buf, na dúvida é quebra; falta interface e diff de valores | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Zod + Schema validados no arquivo** | ✅ core e CLI (#32), interface (#35): Zod 4 e Schema 2020-12 com união discriminada, conferidos registro a registro, 1 GB em 1–3 s; tipos TypeScript e `jasao validar` contra estrutura salva ou schema de fora (#45) | ❌ | ❌ | ◐ só Schema | ◐ sem validar | ◐ só Schema |
| Gratuito | ✅ | ❌ | ◐ | ✅ | ✅ | ✅ |

## Gaps, em ordem

1. **Benchmark público de 1 GB contra o Dadroit.** O #38 mediu os tetos internos (1ª tela 0,28 s, índice 0,29 s, estrutura 3,41 s, busca 0,59–0,73 s, ≤63 MB além do mmap), mas o Dadroit não foi comparado e nada foi publicado. É o único argumento que o pago tem e que o Jasão pode tirar de graça.
2. **Diff na interface**, mais o diff de valores. O core e a CLI (#39) já classificam quebra × compatível; a interface está em PR (#42). Nenhum concorrente faz isso.
3. **1ª tela no limite de 300 ms** (0,28 s no #38): qualquer regressão estoura a meta; a checagem de 10% no CI foi proposta e ainda não está ligada.

## Candidatos a "muito especial", de graça

- **Laudo do arquivo ao abrir.** Antes de qualquer árvore, uma linha como "832 registros, 3 formatos por `tipo`, 4 avisos". Nenhum dos 5 abre por aí. Começou com o painel de entrada (#12), e o modo Estrutura (#21) já calcula as variantes; falta trazê-las e os avisos para a linha de abertura.
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
- 24/11/2026 (tarde): mergeados #44 (seguir arquivo vivo, como `tail -f`), #45 (tipos TypeScript e `jasao validar` contra estrutura salva ou JSON Schema de fora, com `$ref` só local e custo limitado) e #46 (MessagePack, CBOR, BSON, YAML e TOML como entrada). O export fica mais largo, mas o placar não muda: 25 de 26. Arquivo vivo e formatos ficam fora do placar até haver fonte do que os concorrentes fazem. Em PR: interface do diff (#42), avisos da Estrutura (#43) e jq ao vivo (#47).
