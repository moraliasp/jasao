# Radar: Jasão × top 5

Placar vivo do Jasão contra o mercado. Atualizado em 22/11/2026, com `main` em d4abef2 (#6 a #16 mergeados). A fonte de cada concorrente está em [concorrencia.md](concorrencia.md).

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
| Abas e arrastar vários arquivos | ◐ em PR | ? | ? | ❌ | — | ? |
| Árvore de valores | ✅ #6 | ✅ | ✅ | ✅ | — | ✅ |
| Texto cru sincronizado com a árvore | ✅ #9 | ? | ? | ◐ | — | — |
| Tabela por coluna | ❌ | ? | ✅ | ❌ | — | ✅ |
| Copiar caminho (jq, JSONPath, Pointer) | ✅ core e CLI (#7), interface (#19) | ✅ | ✅ | ✅ | — | — |
| Linha inválida sem quebrar o resto | ✅ #7, #12 | ? | ? | ❌ | — | — |
| Consulta jq | ✅ core e CLI (#10), interface (#19) | ◐ JSONPath | ✅ | ❌ | — | ✅ (consulta Mongo) |
| Valores inteligentes (data, JWT, base64) | ❌ | ? | ? | ✅ prévias | — | — |
| Visão do arquivo inteiro (régua por variante e inválidas) | ✅ #16 | ? | ❌ | ❌ | — | ❌ |
| Resumo ao abrir (painel de entrada) | ✅ #12 | ? | ? | ❌ | — | ◐ |
| **Presença e evidência por campo** | ◐ core e `jasao inferir` (#8); na interface só a contagem de campos | ❌ | ❌ | ◐ | ◐ | ✅ só no Mongo |
| **União discriminada** | ◐ core (#8), discriminador até 32 valores; sem interface | ❌ | ❌ | ◐ anyOf | ◐ | ❌ |
| **Diff estrutural: quebra × compatível** | ❌ Fase 3 | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Zod + Schema validados no arquivo** | ❌ Fase 4 | ❌ | ❌ | ◐ só Schema | ◐ sem validar | ◐ só Schema |
| Gratuito | ✅ | ❌ | ◐ | ✅ | ✅ | ✅ |

## Gaps, em ordem

1. **Benchmark público de 1 GB contra o Dadroit.** É o único argumento que o pago tem e que o Jasão pode tirar de graça.
2. **Estrutura (#8) na interface**, com tipo e "N de M" por caminho e as variantes da união; a régua (#16) já colore por variante. Nenhum dos 5 entrega isso fora do Mongo.
3. **Tabela por coluna e valores inteligentes.** OK JSON e JSON Hero já ditam o padrão aqui.
4. **Diff e export.** Não têm concorrente direto e podem esperar a estrutura estar sólida.

## Candidatos a "muito especial", de graça

- **Laudo do arquivo ao abrir.** Antes de qualquer árvore, uma linha como "832 registros, 3 formatos por `tipo`, 4 avisos". Nenhum dos 5 abre por aí. Começou com o painel de entrada (#12): falta mostrar variantes e avisos.
- **Régua do arquivo inteiro (#16).** Minimapa com cor por variante e linhas inválidas; nenhum dos 5 mostra o arquivo assim.
- **`jasao diff --falha-em quebra` no CI.** O oasdiff faz isso para OpenAPI; ninguém faz para dados reais.
- **Privacidade como recurso.** Tudo roda local, nada sobe, e o que parece segredo é sinalizado. O JSON Hero é web; o Jasão ganha quando o dado é sensível.

## Histórico

- 21/11/2026: radar criado. Core com índice de linhas, `.gz`/`.zst` e busca. Em PR: árvore (#6), caminhos e linhas inválidas (#7), estrutura (#8), texto (#9) e jq (#10).
- 22/11/2026: mergeados #6 a #10, #12 e #16. Novo: árvore, texto, painel de entrada e régua na interface; caminho, jq, estrutura e união discriminada no core e na CLI. CI passou a rodar em runner próprio (#15). Gap 1: busca, caminho e jq na interface.
- 22/11/2026 (tarde): busca (⌘F, contagem, marcas na régua), copiar caminho (menu e atalhos) e consulta jq (⌘J, resultado no visualizador, volta ao arquivo) em PR. Sai o antigo gap 1; benchmark contra o Dadroit vira o primeiro.
- 22/11/2026 (noite): abas de arquivo (estado por documento no Rust e no front, ⌘T/⌘W/⌘1…9/Ctrl+Tab) e arrastar vários arquivos para a janela em PR. Os exemplos embutidos viram duas abas.
