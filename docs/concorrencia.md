# Dr. J: análise de concorrência

> Pesquisa web feita em 21/11/2026. Cada afirmação traz o número da fonte entre colchetes, ex. [12]; a lista completa está na seção 8. O que não consegui confirmar em fonte está marcado **(não confirmado)**.

## 1. Resumo executivo

1. **Existe lacuna real.** Não encontrei nenhuma ferramenta que reúna, no mesmo lugar, inferência sobre JSONL com presença por campo e evidência, uniões discriminadas detectadas por campo-tipo, diff estrutural com classificação breaking/compatível **a partir da estrutura inferida** e export Zod 4 + JSON Schema 2020-12 autovalidado contra o arquivo.
2. **O pilar mais diferenciado é o (c).** As ferramentas de diff de JSON comparam valores (jd, jsondiffpatch, Graphtage) [19][21][23]. As que classificam *breaking* exigem um schema escrito à mão (oasdiff, json-schema-diff, buf) [25][28][29]. Ninguém junta as duas coisas.
3. **O pilar (a) tem um análogo forte, mas preso ao MongoDB:** o Compass mostra o mix de tipos e a probabilidade de presença de cada campo, mas só para coleções Mongo [10].
4. **Dr. J é redundante em:** árvore de valores, busca e copy-path, que já estão em OK JSON, Dadroit, JSON Hero, jless, fx e no próprio Firefox, que abre JSONL desde a versão 155 [1][5][7][8][9]. Também é redundante na conversão de uma amostra só para tipo/schema (quicktype, JSON Crack, JSON Hero) [3][12].
5. **Recomendação:** concentrar a v1 nos pilares (a)+(b)+(c), com (d) derivado do mesmo modelo interno. Manter a árvore mínima. Trocar o nome, que tem risco alto de descoberta e algum risco de marca.

## 2. Tabela comparativa (4 pilares)

Legenda: ✅ faz · ◐ parcial · ❌ não faz · ? não confirmado

| Ferramenta | O que é | Plataforma | Preço / licença | Manutenção | (a) estrutura c/ presença + evidência | (b) JSONL multi-formato / união discriminada | (c) diff estrutural breaking × compatível | (d) Zod + JSON Schema validados no arquivo |
|---|---|---|---|---|---|---|---|---|
| **quicktype** [12][13][14] | gera tipos e schema a partir de amostras | CLI, web, VS Code | open source (Apache-2.0 **(não confirmado)**) | ativa: v26.0.0 [14] | ◐ infere opcional/nullable, mas não mostra contagem | ◐ infere uniões; o FAQ recomenda várias amostras [13]; discriminador **(não confirmado)** | ❌ | ◐ tem alvos `typescript-zod` e `schema` [12]; não valida contra o arquivo |
| **MongoDB Compass – Schema** [10][11] | análise de schema de coleção | desktop | gratuito | ativa (doc atual) | ✅ % de tipo e presença por campo; só Mongo | ◐ mostra o mix de tipos; não agrupa por discriminador | ❌ | ◐ exporta JSON Schema [11]; sem Zod |
| **JSON Hero + @jsonhero/schema-infer** [3][4][5] | viewer web + lib de inferência | web, VS Code | open source | lib v0.1.5; último push do repo visto em 08/2024 [6] **(data exata não confirmada)** | ◐ required/optional, formatos (date, uri, email, uuid, ip) [4]; sem contagem | ◐ `anyOf` para tipos conflitantes [4]; sem JSONL **(não confirmado)** | ❌ | ◐ 2020-12 [4]; sem Zod |
| **JSON Crack / ToDiagram** [15][16][17] | visualização em grafo | web, VS Code | README cita AGPLv3, outras fontes GPL-3.0 (**conflito**); ToDiagram pago ($9/usuário/mês segundo G2 **(não confirmado no site oficial)**) [18] | atividade em 2026 **(não confirmada)** | ❌ | ❌ | ❌ | ◐ gera TS/Go/Kotlin/Rust + JSON Schema; ToDiagram valida draft 7 [17] |
| **DuckDB `read_json`** [30][31] | SQL sobre JSON/NDJSON | CLI, libs | MIT **(não confirmado nesta pesquisa)** | ativa | ◐ amostra 20.480 registros por padrão, `-1` lê tudo [30]; sem presença por campo | ◐ `union_by_name`; STRUCT→MAP por limiar (equivale à detecção de mapa) [30] | ❌ | ❌ |
| **Hackolade** [32] | modelagem de dados | desktop | comercial | ativa **(não confirmado)** | ◐ engenharia reversa de JSON/NDJSON, data ISO 8601 [32] | ◐ polimorfismo (tipo simples × subdocumento) [32] | ? | ◐ exporta JSON Schema **(não confirmado)** |
| **mkschema** [33] | inferência de várias amostras | CLI Node/Python | ? | post recente; repositório **não localizado** | ◐ required só quando o campo aparece em todas as amostras; formato só se todas concordam [33] | ◐ `--ndjson` [33] | ❌ | ◐ JSON Schema; sem Zod |
| **GenSON / Skinfer / json-schema-inferrer** [34][35][36] | merge de schemas de várias amostras | Python / Java | open source (json-schema-inferrer Apache-2.0) | GenSON v1.4.0; Skinfer parado (último commit 2016) [35]; inferrer último push 01/2024 [36] | ◐ | ◐ Skinfer tem `--jsonlines` (draft-04) [35] | ❌ | ◐ só JSON Schema |
| **jd / jsondiffpatch / Graphtage / json-diff** [19][20][21][22][23] | diff de valores | CLI / lib | MIT, LGPL etc. | jd e jsondiffpatch ativos; json-diff pouco mantido | ❌ | ❌ | ◐ diff estrutural de **valores**, sem classificação breaking | ❌ |
| **oasdiff / json-schema-diff (Sentry) / json-schema-diff-validator / buf breaking** [25][26][27][28][29] | breaking change entre schemas | CLI | open source | oasdiff ativo; Sentry em WIP | ❌ | ❌ | ◐ classifica breaking, mas **exige schema existente** (OpenAPI, draft-07, proto) | ❌ |
| **OK JSON** [7][62] | viewer macOS | macOS | US$ 24,99 pagamento único, com teste grátis [62] (era Pro $17,99 em 21/11/2026) | versão vista na App Store em 24/11/2026 [7] | ❌ | ◐ abre JSONL; não infere | ❌ | ❌ |
| **Dadroit** [8][37] | viewer de arquivos grandes | desktop + web | pago; valor **não confirmado**: `https://dadroit.com/pricing` dá 404 em 24/11/2026; o $98/ano antes citado vinha de terceiro [37] | ? | ❌ | JSONL **(não confirmado)** | ❌ | ❌ |
| **jless / fx** [38][39] | viewer TUI | terminal | open source | jless v0.9.0 (data não confirmada); fx v36 com streaming | ❌ | ◐ fx faz streaming | ❌ | ❌ |
| **Firefox JSON viewer** [9][40] | viewer do navegador | browser | grátis | ativo; JSONL desde o Fx 155 [40] | ❌ | ◐ cada linha vira um nó; erro por linha [40] | ❌ | ❌ |
| **VisiData / lnav / Miller** [41][42][43] | exploração tabular de logs | terminal | open source | ativos | ◐ Miller tem verbo `summary` (saída **não confirmada**) | ◐ leem JSONL | ❌ | ❌ |
| **VS Code / JetBrains / jq** | editores e CLI | — | — | **não pesquisado**; conhecimento geral | ❌ | ❌ | ❌ | ❌ (VS Code valida contra schema externo, que é outra coisa) |

## 3. Concorrentes mais próximos

**1. quicktype** [12][13][14]
- *Faz melhor:* projeto maduro e muito ativo (v26), gera tipos para dezenas de linguagens e já tem saída Zod e JSON Schema.
- *Falta:* não mostra presença nem evidência por caminho, não faz diff entre versões, não valida o schema gerado contra a entrada e não é um app de inspeção.
- *Risco:* é o candidato natural a cobrir o pilar (d) "de graça". Se ganhar estatísticas de presença, encosta no pilar (a).

**2. MongoDB Compass – aba Schema** [10][11]
- *Faz melhor:* é a referência de UX do pilar (a), com % de tipo por campo e presença por campo [10].
- *Falta:* funciona só em coleção Mongo, sem arquivo solto nem JSONL. Não tem discriminador, diff ou Zod.
- *Uso para o Dr. J:* modelo de interface a copiar, sem concorrência direta.

**3. JSON Hero + @jsonhero/schema-infer** [3][4][6]
- *Faz melhor:* é web e open source, e a lib já entrega 2020-12 com formatos e `anyOf` [4].
- *Falta:* não lê JSONL **(não confirmado)**, não conta presença, não faz diff nem Zod. A manutenção parece baixa (último push visto em 2024 [6]).
- *Risco:* a lib é reaproveitável por qualquer concorrente.

**4. DuckDB** [30][31]
- *Faz melhor:* processa JSONL em escala, tem `union_by_name`, detecção de STRUCT×MAP e controle de amostragem [30].
- *Falta:* não tem interface de inspeção, evidência por caminho, diff com breaking ou export Zod.
- *Risco:* para um dev que já usa SQL, `DESCRIBE` + `GROUP BY` resolve na mão boa parte do pilar (a).

**5. oasdiff / json-schema-diff-validator** [25][26][28]
- *Faz melhor:* o oasdiff tem uma taxonomia madura de breaking changes, com mais de 100 checagens, níveis ERR/WARN/INFO e `--fail-on` para CI [25][26].
- *Falta:* precisa de um schema pronto (OpenAPI ou draft-07). Não infere nada de dados.
- *Uso para o Dr. J:* adotar a taxonomia deles, sem inventar uma própria.

## 4. Diferenciais defensáveis e riscos

**Diferenciais**
- **Diff de estrutura inferida × diff de valores.** Comparar duas versões de um JSONL por *forma* e dizer "isto quebra consumidor" sem exigir schema prévio. Não achei quem faça isso [19][21][25][28].
- **Uniões discriminadas automáticas.** Agrupar por um campo `tipo` e gerar `z.discriminatedUnion`. Os conversores JSON→Zod só geram `z.union` genérico [44][45]. O tema aparece em pesquisa acadêmica (Klessinger) [46], mas não encontrei produto que entregue isso.
- **Evidência por caminho** ("320 de 832", com exemplos) em vez de só um schema final. O Compass faz isso, mas só para Mongo [10].
- **Autovalidação do schema contra o próprio arquivo**, garantindo que o export não mente. Não encontrei em nenhum gerador pesquisado.
- **Smells** (número como texto, data não ISO, campo de valor único): não encontrei em ferramenta equivalente **(não confirmado como ausência total)**.
- **App desktop local e read-only, em pt-BR:** bom para dados sensíveis (LGPD), mas não é uma barreira técnica.

**Riscos**
- **quicktype** pode ganhar estatísticas de presença ou discriminadores; ele já tem o motor de uniões [12][13].
- **Firefox, JSON Hero e JSON Crack** já leem JSONL ou podem passar a ler [40], o que erode o "abrir JSONL bonito".
- **DuckDB + SQL** como alternativa "faça você mesmo" para quem é técnico [30].
- **Zod 4** gera JSON Schema 2020-12 nativamente com `z.toJSONSchema()` [47], então o export Schema vira commodity. O valor está na *inferência*, não na serialização.
- **Desempenho em arquivos grandes:** o DuckDB amostra 20.480 registros por padrão [30]. Dr. J precisa decidir entre amostra e varredura completa e deixar isso explícito na evidência ("baseado em N de M linhas").
- **Mercado de nicho:** quem sente a dor (integração com APIs e logs que mudam de forma) é um público técnico e pequeno.

## 5. Recomendação de escopo da v1

1. **Núcleo (obrigatório):** pilar (a) com presença, tipo, formato e evidência, e pilar (b) com uniões discriminadas, uniões dentro de objetos e objeto→record. É o que ninguém entrega junto em arquivo solto.
2. **Logo em seguida:** pilar (c), diff da estrutura inferida, usando uma taxonomia de breaking emprestada do oasdiff/buf (campo removido, tipo estreitado, opcional→obrigatório, variante removida = breaking; o inverso = compatível) [25][29].
3. **Pilar (d):** gerar Zod 4 a partir do modelo interno e derivar o JSON Schema via `z.toJSONSchema()` [47], sem manter dois geradores. A autovalidação contra o arquivo é o diferencial; os toggles literal/genérico e coerção vêm junto.
4. **Smells:** entram barato porque saem da mesma passada de inferência.
5. **Cortar ou minimizar:** árvore de valores, busca e copy-path só no básico, porque são commodity [5][7][9]. Não competir em viewer.
6. **Decidir cedo:** limite de tamanho e estratégia de amostragem, sempre explícitos na interface.

## 6. Nome "Dr. J": colisões

> Orientação geral a partir de busca pública. **Não é parecer jurídico de marca.** Antes de registrar, consultar a área responsável e buscas formais no INPI e no USPTO.

| Colisão | O que é | Fonte |
|---|---|---|
| Julius Erving, "Dr. J" | apelido mundialmente associado ao jogador de basquete; a propriedade intelectual dele é gerida pela Authentic Brands Group desde 2016 | [48][49] |
| Marcas registradas em nome de Erving | encontrei THE JIM e STREET SMARTS; **registro "DR. J" não confirmado** | [50] |
| Dr. J Today, DrJ Men's Healthcare, AI Dr. J, Talk DRJ | apps nas lojas (saúde e outros) | [51][52][53][54] |
| drj.com | Disaster Recovery Journal, ativo em 2026 | [55] |
| npm `~drj` e `dr.js`; PyPI `drjson` | pacotes homônimos ou parecidos (`drjson` é o mais próximo do domínio do produto) | [56][57][58] |
| crates.io / GitHub "drj" | nada relevante encontrado **(não confirmado)** | — |

**Veredito: risco ALTO de descoberta e MODERADO de marca.**
- A busca por "Dr. J" é dominada pelo atleta e pelo DRJ, e o nome não comunica JSON.
- Há um gestor de PI ativo (ABG) ligado ao apelido, ainda que eu não tenha confirmado registro específico.
- O `drjson` no PyPI disputa o mesmo espaço semântico.

**Alternativas** (checagem rápida em busca web; nenhuma passou por INPI/USPTO):
1. **Jsonda** (JSON + sonda): nenhuma ferramenta ou pacote com esse nome exato encontrado [59].
2. **Laudo JSON** / **Laudo**: nenhuma ferramenta de dev JSON encontrada; existe o pacote npm `laud`, sem relação [60]. A palavra é genérica em pt-BR, o que dificulta registro, mas comunica bem o diagnóstico.
3. **Esquadro**: evoca medir e verificar forma. **Não verificado.**

Descartado: **Raio-J / JSON X-Ray**, porque existe o J-RAY, um visualizador JSON open source [61].

## 7. O que ficou não confirmado

- Preço e suporte a JSONL do Dadroit.
- Licença (AGPL × GPL) e atividade em 2026 do JSON Crack; preço do ToDiagram.
- Data exata do último commit do JSON Hero e se ele lê JSONL.
- Licença do quicktype; se ele detecta discriminador.
- Data do jless v0.9.0; saída do `summary` do Miller; preço e atividade do Hackolade.
- Recursos JSON do VS Code e do JetBrains, e o jq: não pesquisados.
- Existência de registro "DR. J" no USPTO/INPI; disponibilidade das 3 alternativas em INPI, USPTO, domínios e lojas.

## 8. Fontes

1. Firefox JSON viewer – https://firefox-source-docs.mozilla.org/devtools-user/json_viewer/
2. *(reservado)*
3. JSON Hero – https://jsonhero.io/
4. @jsonhero/schema-infer README – https://unpkg.com/browse/@jsonhero/schema-infer@0.1.5/README.md · https://npmjs.com/@jsonhero/schema-infer
5. JSON Hero README – https://raw.githubusercontent.com/triggerdotdev/jsonhero-web/main/README.md
6. JSON Hero, metadados do repo – https://awesome.ecosyste.ms/projects/github.com%2Ftriggerdotdev%2Fjsonhero-web
7. OK JSON (App Store) – https://apps.apple.com/app/id1576121509
8. Dadroit – https://sourceforge.net/software/product/Dadroit-JSON-Viewer/
9. Firefox JSON viewer (MDN) – https://developer.mozilla.org/en-US/docs/Tools/JSON_viewer
10. MongoDB Compass Schema – https://www.mongodb.com/docs/compass/current/schema/
11. Compass export de schema – https://mongodb.com/docs/compass/current/schema/export
12. quicktype README – https://raw.githubusercontent.com/glideapps/quicktype/master/README.md · https://quicktype.io/llms.txt
13. quicktype FAQ – https://raw.githubusercontent.com/glideapps/quicktype/HEAD/FAQ.md
14. quicktype v26.0.0 – https://newreleases.io/project/github/glideapps/quicktype/release/v26.0.0
15. JSON Crack – https://bestofjs.org/projects/json-crack
16. ToDiagram preços – https://todiagram.com/pricing
17. ToDiagram validação de schema – https://todiagram.com/blog/validate-json-schema-on-diagrams
18. G2 JSON Crack – https://www.g2.com/products/json-crack/pricing
19. jd – https://github.com/josephburnett/jd
20. json-diff – https://npmjs.com/package/json-diff · https://andreyvit.github.io/json-diff
21. jsondiffpatch – https://cdn.jsdelivr.net/npm/jsondiffpatch@0.7.6/README.md
22. jsondiffpatch (Snyk) – https://snyk.io/advisor/npm-package/jsondiffpatch
23. Graphtage – https://blog.trailofbits.com/2020/08/28/graphtage/ · https://trailofbits.com/tools/graphtage/
24. *(reservado)*
25. oasdiff breaking changes – https://github.com/tufin/oasdiff/blob/v1.10.16/BREAKING-CHANGES.md
26. oasdiff (CodeRabbit) – https://docs.coderabbit.ai/tools/oasdiff
27. json-schema-diff (npm) – https://www.npmjs.com/package/json-schema-diff · api-schema-diff – https://pypi.org/project/api-schema-diff/1.0.1/
28. json-schema-diff (Sentry) – https://github.com/getsentry/json-schema-diff · json-schema-diff-validator – https://npmjs.com/package/json-schema-diff-validator
29. buf breaking rules – https://buf.build/docs/breaking/rules/
30. DuckDB JSON format settings – https://duckdb.org/docs/current/data/json/format_settings.html
31. DuckDB loading JSON – https://duckdb.org/docs/data/json/loading_json
32. Hackolade JSON – https://hackolade.com/help/JSONdocument1.html
33. mkschema – https://dev.to/_06a3df6b50aec966668fb/most-json-to-schema-tools-over-fit-one-example-mkschema-merges-many-samples-51lg
34. GenSON – https://pypi.org/project/genson/
35. Skinfer – https://www.zyte.com/blog/skinfer-a-tool-for-inferring-json-schemas/ · https://github.com/scrapinghub/skinfer
36. json-schema-inferrer – https://awesome.ecosyste.ms/projects/github.com%2Fsaasquatch%2Fjson-schema-inferrer
37. Dadroit preço (terceiro) – https://jsonbolt-web.pages.dev/blog/json-viewer-comparison.html · https://www.g2.com/products/dadroit-json-viewer/pricing
38. jless – https://docs.rs/crate/jless/latest · https://terminaltrove.com/feed/jless/
39. fx v36 – https://newreleases.io/project/github/antonmedv/fx/release/36.0.0
40. Firefox suporte a JSONL (bug 2055774) – https://bugzilla.mozilla.org/show_bug.cgi?id=2055774
41. VisiData formatos – https://visidata.org/formats
42. lnav formatos – https://docs.lnav.org/en/latest/formats.html
43. Miller – https://miller.readthedocs.io/en/main/manpage/
44. JSON→Zod (flaviocopes) – https://flaviocopes.com/tools/json-to-zod/
45. JSON→Zod, escolhas de algoritmo – https://dev.to/jsontots/how-to-convert-a-json-sample-to-a-zod-schema-and-the-4-algorithm-choices-behind-a-working-673
46. Klessinger, tagged unions – https://arxiv.org/pdf/2306.07085 · survey BENEVOL2024 – https://ceur-ws.org/Vol-3941/BENEVOL2024_TECH_paper14.pdf
47. Zod 4 JSON Schema – https://zod.dev/json-schema · zod-to-json-schema descontinuado – https://npmjs.com/package/zod-to-json-schema
48. Dr. J (Wikipedia) – https://www.wikipedia.com/wiki/Dr._J
49. ABG + Dr. J – https://us.fashionnetwork.com/news/authentic-brands-enters-partnership-with-dr-j-julius-erving,736717.html
50. Marcas de Julius Erving – https://trademark.justia.com/owners/erving-julius-2320333
51. Dr. J Today – https://apps.apple.com/us/app/dr-j-today/id1490440019
52. DrJ Men's Healthcare – https://apps.apple.com/app/id6451014534
53. AI Dr. J – https://mwm.ai/apps/ai-dr-j/6756600573
54. Talk DRJ – https://www.appbrain.com/appstore/talk-drj/ios-1560708693
55. Disaster Recovery Journal – https://drj.com/about-us/
56. npm ~drj – https://npmjs.com/~drj
57. npm dr.js – https://www.npmjs.com/package/dr.js
58. PyPI drjson – https://socket.dev/pypi/package/drjson
59. Busca "jsonda" sem resultado exato (busca web em 21/11/2026; sem URL de resultado positivo)
60. npm laud – https://pkgstats.com/pkg:laud
61. J-RAY – https://github.com/MauryDevIta/J-Ray
62. OK JSON, site oficial (preço conferido em 24/11/2026) – https://okjson.app

Outras fontes consultadas: a2z.tools inferrer (existência só parcialmente confirmada) – https://a2z.tools/json-schema-inferrer · jddf-infer – https://docs.rs/crate/jddf-infer/0.1.0 · datamodel-code-generator – https://pydantic.dev/docs/validation/latest/integrations/dev-tools/datamodel_code_generator/ · Transform Tool VS Code – https://dev.to/osonwa/convert-api-json-responses-to-typescript-types-and-zod-schemas-in-vscode-3d3p
