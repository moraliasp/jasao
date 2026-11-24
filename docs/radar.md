# Radar: Jasão × top 5

Placar vivo do Jasão contra o mercado. Atualizado em 22/11/2026, com `main` em 2ce72ee (#6 a #22 mergeados). A fonte de cada concorrente está em [concorrencia.md](concorrencia.md).

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
| Tabela por coluna | ✅ núcleo (#33) e interface: colunas por caminho, ordenação completa em 2º plano, fixar e ocultar | ? | ✅ | ❌ | — | ✅ |
| Copiar caminho (jq, JSONPath, Pointer) | ✅ core e CLI (#7), interface (#19) | ✅ | ✅ | ✅ | — | — |
| Linha inválida sem quebrar o resto | ✅ #7, #12 | ? | ? | ❌ | — | — |
| Consulta jq | ✅ core e CLI (#10), interface (#19) | ◐ JSONPath | ✅ | ❌ | — | ✅ (consulta Mongo) |
| Valores inteligentes (data, JWT, base64) | ❌ | ? | ? | ✅ prévias | — | — |
| Visão do arquivo inteiro (régua por variante e inválidas) | ✅ #16 | ? | ❌ | ❌ | — | ❌ |
| Resumo ao abrir (painel de entrada) | ✅ #12 | ? | ? | ❌ | — | ◐ |
| **Presença e evidência por campo** | ✅ core e CLI (#8), modo Estrutura na interface (#21): tipos e "N de M" por caminho | ❌ | ❌ | ◐ | ◐ | ✅ só no Mongo |
| **União discriminada** | ✅ core (#8) e interface (#21): uma linha por variante, Enter abre o 1º registro; falta filtro por variante | ❌ | ❌ | ◐ anyOf | ◐ | ❌ |
| **Diff estrutural: quebra × compatível** | ❌ Fase 3 | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Zod + Schema validados no arquivo** | ❌ Fase 4 | ❌ | ❌ | ◐ só Schema | ◐ sem validar | ◐ só Schema |
| Gratuito | ✅ | ❌ | ◐ | ✅ | ✅ | ✅ |

## Gaps, em ordem

1. **Benchmark público de 1 GB contra o Dadroit.** É o único argumento que o pago tem e que o Jasão pode tirar de graça. Antes, fechar o #23 (tetos de memória da árvore).
2. **Valores inteligentes** (data, JWT, base64). O JSON Hero dita o padrão.
3. **Diff e export.** Com a estrutura na interface (#21), a base está pronta. O diff que marca quebra × compatível não tem concorrente: quem chegar primeiro fica com o eixo sozinho.

## Candidatos a "muito especial", de graça

- **Laudo do arquivo ao abrir.** Antes de qualquer árvore, uma linha como "832 registros, 3 formatos por `tipo`, 4 avisos". Nenhum dos 5 abre por aí. Começou com o painel de entrada (#12), e o modo Estrutura (#21) já calcula as variantes; falta trazê-las e os avisos para a linha de abertura.
- **Régua do arquivo inteiro (#16).** Minimapa com cor por variante e linhas inválidas; nenhum dos 5 mostra o arquivo assim.
- **`jasao diff --falha-em quebra` no CI.** O oasdiff faz isso para OpenAPI; ninguém faz para dados reais.
- **Privacidade como recurso.** Tudo roda local, nada sobe, e o que parece segredo é sinalizado. O JSON Hero é web; o Jasão ganha quando o dado é sensível.

## Histórico

- 21/11/2026: radar criado. Core com índice de linhas, `.gz`/`.zst` e busca. Em PR: árvore (#6), caminhos e linhas inválidas (#7), estrutura (#8), texto (#9) e jq (#10).
- 22/11/2026: mergeados #6 a #10, #12 e #16. Novo: árvore, texto, painel de entrada e régua na interface; caminho, jq, estrutura e união discriminada no core e na CLI. CI passou a rodar em runner próprio (#15). Gap 1: busca, caminho e jq na interface.
- 22/11/2026 (tarde): busca (⌘F, contagem, marcas na régua), copiar caminho (menu e atalhos) e consulta jq (⌘J, resultado no visualizador, volta ao arquivo) em PR. Sai o antigo gap 1; benchmark contra o Dadroit vira o primeiro.
- 22/11/2026 (noite): abas de arquivo (estado por documento no Rust e no front, ⌘T/⌘W/⌘1…9/Ctrl+Tab) e arrastar vários arquivos para a janela em PR. Os exemplos embutidos viram duas abas.
- 22/11/2026: em PR, recentes (os 10 últimos, na aba vazia e no menu Arquivo) e associação de `.json`/`.jsonl`/`.ndjson` como visualizador alternativo, com abertura pelo Finder e instância única. Fica fora do placar, que compara visualização e não integração com o sistema.
- 22/11/2026 (fim do dia): mergeados #18 a #22 (exemplos, busca/caminho/jq na interface, abas, Estrutura, recentes). Presença e união discriminada passam a ✅ na interface; o Jasão fecha 20 de 26 pontos no radar 0–2, contra 13 do JSON Hero. Em PR: tetos de memória da árvore (#23). Gap 1 agora é a tabela por coluna.
- 23/11/2026: modo Tabela em PR (núcleo #33, interface empilhada): colunas por caminho, janela virtual do Rust, ordenação completa em 2º plano com progresso e cancelar, fixar e ocultar. Tabela por coluna passa a ✅; o Jasão vai a 22 de 26 pontos quando mergear. Gap 1 volta a ser o benchmark contra o Dadroit.
