# Radar: Jasão × top 5

Placar vivo do Jasão contra o mercado. Atualizado em 21/11/2026, com `main` em 045b795 e os PRs #6 a #10 abertos. A fonte de cada concorrente está em [concorrencia.md](concorrencia.md).

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
| Abrir JSONL grande sem travar | ◐ índice de linhas no core, interface mínima | ✅ | ◐ | ❌ | — | — |
| `.gz` / `.zst` | ✅ #4 | ? | ? | ❌ | — | — |
| Busca no arquivo inteiro | ◐ pronta no core (#5), sem interface | ✅ | ✅ | ✅ | — | ◐ |
| Árvore de valores | ◐ #6 | ✅ | ✅ | ✅ | — | ✅ |
| Texto cru sincronizado com a árvore | ◐ #9 | ? | ? | ◐ | — | — |
| Tabela por coluna | ❌ | ? | ✅ | ❌ | — | ✅ |
| Copiar caminho (jq, JSONPath, Pointer) | ◐ #7 | ✅ | ✅ | ✅ | — | — |
| Linha inválida sem quebrar o resto | ◐ #7 | ? | ? | ❌ | — | — |
| Consulta jq | ◐ #10 | ◐ JSONPath | ✅ | ❌ | — | ✅ (consulta Mongo) |
| Valores inteligentes (data, JWT, base64) | ❌ | ? | ? | ✅ prévias | — | — |
| **Presença e evidência por campo** | ◐ #8 | ❌ | ❌ | ◐ | ◐ | ✅ só no Mongo |
| **União discriminada** | ◐ #8 (escopo a conferir) | ❌ | ❌ | ◐ anyOf | ◐ | ❌ |
| **Diff estrutural: quebra × compatível** | ❌ Fase 3 | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Zod + Schema validados no arquivo** | ❌ Fase 4 | ❌ | ❌ | ◐ só Schema | ◐ sem validar | ◐ só Schema |
| Gratuito | ✅ | ❌ | ◐ | ✅ | ✅ | ✅ |

## Gaps, em ordem

1. **Mergear a fila #6 a #10 e ligar busca e caminho na interface.** Com isso o Jasão empata com os 5 no básico de visualizador.
2. **Benchmark público de 1 GB contra o Dadroit.** É o único argumento que o pago tem e que o Jasão pode tirar de graça.
3. **Estrutura (#8) na interface logo depois da árvore**, com tipo e "N de M" por caminho. Nenhum dos 5 entrega isso fora do Mongo.
4. **Tabela por coluna e valores inteligentes.** OK JSON e JSON Hero já ditam o padrão aqui.
5. **Diff e export.** Não têm concorrente direto e podem esperar a estrutura estar sólida.

## Candidatos a "muito especial", de graça

- **Laudo do arquivo ao abrir.** Antes de qualquer árvore, uma linha como "832 registros, 3 formatos por `tipo`, 4 avisos". Nenhum dos 5 abre por aí.
- **`jasao diff --falha-em quebra` no CI.** O oasdiff faz isso para OpenAPI; ninguém faz para dados reais.
- **Privacidade como recurso.** Tudo roda local, nada sobe, e o que parece segredo é sinalizado. O JSON Hero é web; o Jasão ganha quando o dado é sensível.

## Histórico

- 21/11/2026: radar criado. Core com índice de linhas, `.gz`/`.zst` e busca. Em PR: árvore (#6), caminhos e linhas inválidas (#7), estrutura (#8), texto (#9) e jq (#10).
