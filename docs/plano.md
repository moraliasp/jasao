# Plano do app: Jasão

Visualizador de JSON/JSONL de ponta, somente leitura, no molde do Leve (Tauri 2, Rust, Vite + TypeScript sem framework, pt-BR e inglês). Decisão do Eduardo em 21/11/2026: o visualizador é produto de primeira linha, não um acessório da Estrutura. Estrutura, Diff e Exportar continuam sendo o que ninguém mais entrega.

## 1. Princípios

1. **O arquivo nunca vai inteiro para a tela.** O Rust guarda o arquivo e os índices; a interface pede janelas de linhas.
2. **Varredura completa, nunca amostra escondida.** Resultado parcial aparece enquanto roda, sempre com "N de M linhas".
3. **Fiel ao byte.** Número grande, ordem das chaves, chave duplicada e linha inválida aparecem como estão no arquivo.
4. **Tudo cancelável e nada trava.** Operação longa roda fora da thread da interface, com progresso.
5. **Somente leitura.** Edição fica fora até o resto estar sólido.

## 2. Arquitetura

- **`core` (crate Rust, sem Tauri):** leitura, índices, inferência, diff, busca, consulta, exportação. É o que o app e a linha de comando usam.
- **`cli`:** binário fino sobre o `core`.
- **`app`:** casca Tauri + interface. Só desenha e pede janelas.

Peças do `core`:

| Peça | Como |
|---|---|
| Leitura | `mmap` do arquivo; gzip/zstd descompactados em streaming para um cache em disco |
| Índice de linhas (JSONL) | varredura paralela de quebras de linha (`memchr` + `rayon`) |
| Índice de JSON único grande | só os filhos do nível aberto; cada subárvore é indexada ao expandir |
| Análise | parser SIMD (`simd-json` ou `sonic-rs`, decidir por benchmark); número preservado como texto |
| Árvore na tela | o Rust mantém a lista plana de linhas visíveis; a interface virtualiza |
| Inferência | um resumo por bloco de linhas, combinável; `rayon` faz mapa e redução |
| Busca | sobre os bytes crus (`memchr`/`regex`), resultado em fluxo, depois mapeado para caminho |
| Consulta | jq nativo em Rust (`jaq`) |
| Validação | `jsonschema` (crate) contra o arquivo inteiro |

A confirmar na documentação antes de codar: canal binário do Tauri 2 para as janelas de linhas, e o custo real de cada parser no nosso caso.

## 3. Metas de desempenho (alvos; medidas em `decisoes.md`, "Medidas de desempenho")

Máquina de referência: MacBook com Apple Silicon, 8 núcleos. Arquivo de referência: JSONL de 1 GB.

| Medida | Alvo |
|---|---|
| Primeira tela depois de abrir | < 300 ms |
| Índice de linhas completo | < 2 s |
| Estrutura completa do arquivo | < 5 s, com parcial visível em < 500 ms |
| Rolagem e expandir/recolher | 60 quadros por segundo, resposta < 16 ms |
| Busca de texto no arquivo todo | primeiro resultado < 200 ms; fim < 3 s |
| Memória além do `mmap` | < 300 MB |
| Teto de tamanho | 10 GB abre e navega; acima disso, avisa |

Cada alvo vira benchmark (`criterion`). A meta é rodar no CI e reprovar regressão acima de 10%, mas isso ainda não está ligado: o runner único tem ruído acima de 10% (ver `decisoes.md`, "Medidas de desempenho").

## 4. Robustez

- **Invariante central, testada por propriedade (`proptest`):** todo registro do arquivo passa na estrutura inferida e no Zod/Schema exportado.
- **Fuzzing (`cargo-fuzz`)** em leitura, índice e inferência: nenhuma entrada derruba o processo.
- **Corpus hostil:** JSONTestSuite, arquivo truncado, UTF-8 inválido, BOM, CRLF, linha de 1 GB, profundidade 10.000, chave duplicada, número de 400 dígitos, arquivo vazio.
- **Limites explícitos** de profundidade, tamanho de linha e memória, com mensagem que diz o problema e a saída.
- **Arquivo que muda por baixo:** detecta, reindexa só o acréscimo, avisa se foi reescrito.
- **Testes dourados** para o Diff e para cada formato de exportação.
- **Acessibilidade:** árvore com papéis ARIA e navegação completa por teclado.

## 5. Funcionalidades por fase

Cada fase termina com algo utilizável. Fatias seguem `<app>/<tema>/<sub-tema>`.

### Fase 0: fundação
Repositório, três crates, CI com testes, fuzz curto e benchmarks, corpus, gerador de arquivos de teste grandes.

### Fase 1: visualizador
- **Abrir:** arrastar, associação de arquivo, linha de comando, área de transferência, abas, arquivos recentes. Feitos: linha de comando, arrastar e abas (22/11/2026); associação de `.json`, `.jsonl` e `.ndjson` como visualizador alternativo, sem tomar o padrão do sistema, e recentes (os 10 últimos, gravados só na máquina; ver decisoes.md).
- **Árvore:** virtualizada, expandir por nível, migalhas do caminho, ir para linha, marcadores.
- **Texto cru:** com realce, sincronizado com a árvore (clicar num leva ao outro).
- **Tabela:** JSONL e listas de objetos viram colunas por caminho; ordenar, fixar e esconder colunas.
- **Busca:** por chave, valor ou expressão regular, no arquivo inteiro, com contagem e navegação.
- **Copiar:** caminho em jq, JSONPath, JSON Pointer e JavaScript; valor cru ou formatado.
- **Valores inteligentes:** época vira data, data mostra "há 3 dias", cor mostra amostra, URL vira link, imagem em base64 tem prévia, JWT abre decodificado, JSON dentro de texto expande. Feito (23/11/2026; ver decisoes.md).
- **Erros:** linha inválida aparece na lista, com posição e motivo, sem impedir o resto.
- **Entrada compactada:** `.gz` e `.zst`.

### Fase 2: estrutura
- Tipo, presença, formato e evidência por caminho; união discriminada; união dentro de objeto; objeto que é mapa.
- Avisos: número como texto, data fora do ISO, valor único, campo opaco, "parece segredo".
- **Estatísticas por campo:** mínimo, máximo, distribuição, cardinalidade, valores mais comuns.
- **Registros que fogem:** da estrutura para a lista de linhas que têm (ou não têm) um campo.

### Fase 3: diff
- Diff de estrutura com quebra × compatível, usando a taxonomia do oasdiff/buf.
- Diff de valores entre dois registros ou dois arquivos.
- Linha do tempo de uma pasta de arquivos datados.

### Fase 4: exportar e validar
- Zod 4 e JSON Schema 2020-12, validados contra o arquivo; tipos TypeScript.
- Validar outro arquivo contra a estrutura salva ou contra um schema de fora.
- Depois: Pydantic, serde, Go, SQL.

### Fase 5: potência
- **Consulta jq** com resultado ao vivo e filtros salvos; exportar resultado em JSONL, CSV e Parquet.
- **Acompanhar arquivo vivo** (como `tail -f`) e vigiar pasta.
- **Linha de comando:** `inferir`, `diff --falha-em quebra`, `validar`, para CI.
- **Formatos binários:** MessagePack, CBOR, BSON; YAML e TOML como entrada.
- **Máscara de segredos** na tela e na exportação; anonimização.

### Fora por enquanto
Edição, decriptar age/gpg, colaboração, nuvem.

## 6. Decisões em aberto

1. **Nome.** Decidido em 21/11/2026: **Jasão** (substitui o provisório "Dr. J"; ver `docs/decisoes.md`). Falta conferir INPI/USPTO.
2. **Pasta do repositório.** Sugestão: `~/Code/moralia/<nome>`.
3. **JSON Schema: um gerador ou dois.** Gerar no Rust a partir do modelo interno (rápido, valida arquivo grande) e conferir no CI contra `z.toJSONSchema()`; ou gerar só o Zod e derivar o Schema na interface (menos código, validação mais lenta). Recomendo o primeiro.
4. **Origem das "versões" no Diff.** Dois arquivos escolhidos, mais a linha do tempo de pasta na Fase 3. Histórico git fica fora.
