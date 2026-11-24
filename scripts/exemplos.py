#!/usr/bin/env python3
"""Gera os dois exemplos embutidos no app (app/src-tauri/exemplos/).

    python3 scripts/exemplos.py

- historia.json: um JSON numa linha só (o Jasão mostra JSON em várias linhas linha a linha;
  numa linha só ele vira uma árvore). O texto mora aqui, legível; o arquivo é gerado.
- vitrine.jsonl: registros de formatos diferentes para exercitar o app. Determinístico (sem
  `random`): rodar de novo dá o mesmo arquivo, byte a byte.

Tudo fictício. Os testes em app/src-tauri/src/exemplos.rs conferem os dois pelo `nucleo`.
"""

import base64
import json
from pathlib import Path

DESTINO = Path(__file__).resolve().parent.parent / "app" / "src-tauri" / "exemplos"


def linha(valor) -> str:
    return json.dumps(valor, ensure_ascii=False, separators=(",", ":"), allow_nan=False)


# ---- Exemplo 1: história e gramática do JSON. Fatos conferidos contra as RFCs citadas. ----

HISTORIA = {
    "titulo": "JSON: de onde veio e como se escreve",
    "sobre": "Exemplo embutido no Jasão. Este arquivo é um JSON que explica o próprio JSON: "
    "expanda os nós com → ou Enter, recolha com ←, e use Copiar caminho para levar o endereço de qualquer valor.",
    "nome": {
        "sigla": "JSON",
        "por_extenso": "JavaScript Object Notation",
    },
    "historia": [
        {
            "quando": "começo dos anos 2000",
            "marco": "Douglas Crockford especifica e divulga o formato",
            "detalhe": "Crockford diz que \"descobriu\" o JSON, não que o inventou: é um pedaço da sintaxe de literais do "
            "JavaScript, que já funcionava em todo navegador. O site json.org, com a gramática em diagramas, "
            "foi ao ar em 2002.",
        },
        {
            "quando": "julho de 2006",
            "marco": "RFC 4627",
            "detalhe": "Primeira RFC, informativa, escrita por Crockford. Registra o tipo de mídia application/json. "
            "O texto JSON precisava ser um objeto ou um array no topo.",
        },
        {
            "quando": "dezembro de 2009",
            "marco": "ECMAScript 5",
            "detalhe": "JSON.parse e JSON.stringify entram na própria linguagem; antes era comum usar eval ou a "
            "biblioteca json2.js.",
        },
        {
            "quando": "outubro de 2013",
            "marco": "ECMA-404, 1ª edição",
            "detalhe": "A Ecma publica a sintaxe do JSON como padrão próprio. O ECMA-404 trata só da gramática, "
            "sem regras de interoperabilidade.",
        },
        {
            "quando": "março de 2014",
            "marco": "RFC 7159",
            "detalhe": "Vira trilha de padrão no IETF (editor: Tim Bray). Qualquer valor pode estar no topo, "
            "não só objeto ou array. Substitui a RFC 4627.",
        },
        {
            "quando": "dezembro de 2017",
            "marco": "RFC 8259 (STD 90) e ECMA-404, 2ª edição",
            "detalhe": "A RFC 8259 é a referência atual e é Internet Standard. Exige UTF-8 entre sistemas que "
            "não fazem parte de um ecossistema fechado e proíbe acrescentar BOM. O ECMA-404 e a RFC "
            "descrevem a mesma gramática.",
        },
    ],
    "derivados": [
        {
            "nome": "JSON Lines / NDJSON",
            "documento": "jsonlines.org e a especificação NDJSON",
            "o_que_e": "Um valor JSON por linha, separados por \\n. Dá para ler em fluxo, acrescentar no fim e "
            "dividir o arquivo sem entender o conteúdo. É o formato de logs, exportações e datasets. "
            "Extensões comuns: .jsonl e .ndjson.",
        },
        {
            "nome": "JSON Schema",
            "documento": "json-schema.org (rascunhos do IETF; versão atual: 2020-12)",
            "o_que_e": "Vocabulário, escrito em JSON, para descrever e validar documentos JSON: tipos, campos "
            "obrigatórios, formatos, uniões (anyOf, oneOf).",
        },
        {
            "nome": "JSON Pointer",
            "documento": "RFC 6901 (abril de 2013)",
            "o_que_e": "Caminho até um valor dentro de um documento, como \"/autores/0/nome\". "
            "O til vira ~0 e a barra vira ~1 dentro do nome de um campo.",
        },
        {
            "nome": "JSON Patch",
            "documento": "RFC 6902 (abril de 2013)",
            "o_que_e": "Lista de operações (add, remove, replace, move, copy, test) endereçadas por JSON Pointer.",
        },
        {
            "nome": "I-JSON",
            "documento": "RFC 7493 (março de 2015)",
            "o_que_e": "Perfil mais restrito para interoperar: UTF-8, sem nomes duplicados num objeto, números "
            "que cabem num double IEEE 754.",
        },
        {
            "nome": "JSONPath",
            "documento": "RFC 9535 (fevereiro de 2024)",
            "o_que_e": "Linguagem de consulta que seleciona valores, como \"$.autores[*].nome\". Nasceu de um "
            "artigo de Stefan Gössner em 2007 e só ganhou RFC depois de anos de implementações divergentes.",
        },
    ],
    "gramatica": {
        "resumo": "Um texto JSON é um valor, com espaço em branco opcional em volta. A RFC 8259 fala em quatro "
        "tipos primitivos (string, número, booleano e nulo) e dois estruturados (objeto e array): seis tipos.",
        "tipos": [
            {"tipo": "objeto", "exemplo": {"chave": "valor", "outra": 2}, "regra": "Chaves entre { }, pares nome: valor separados por vírgula. O nome é sempre string."},
            {"tipo": "array", "exemplo": [1, "dois", None, [3]], "regra": "Colchetes [ ], valores separados por vírgula, de qualquer tipo misturado. A ordem importa."},
            {"tipo": "string", "exemplo": "olá, \"mundo\"\n", "regra": "Sempre entre aspas duplas. Aspas simples não valem."},
            {"tipo": "número", "exemplo": -12.5e3, "regra": "Decimal, com sinal de menos opcional, fração e expoente."},
            {"tipo": "booleano", "exemplo": True, "regra": "true ou false, em minúsculas."},
            {"tipo": "nulo", "exemplo": None, "regra": "null, em minúsculas."},
        ],
        "strings": {
            "obrigatorio_escapar": ["aspas duplas (\\\")", "barra invertida (\\\\)", "caracteres de controle U+0000 a U+001F"],
            "escapes": {"\\\"": "aspas", "\\\\": "barra invertida", "\\/": "barra (opcional)", "\\b": "backspace",
                        "\\f": "form feed", "\\n": "nova linha", "\\r": "retorno de carro", "\\t": "tabulação",
                        "\\uXXXX": "unidade UTF-16 em hexadecimal"},
            "fora_do_plano_basico": "Caractere acima de U+FFFF vira par substituto: a clave de sol U+1D11E se "
            "escreve \"\\uD834\\uDD1E\".",
            "exemplo_escrito": "aspas \" barra \\ tab\t quebra\n clave 𝄞 acento ç",
        },
        "numeros": {
            "forma": "-? (0 | [1-9][0-9]*) (.[0-9]+)? ([eE][+-]?[0-9]+)?",
            "validos": [0, 1.5, -1e-7, 6.02e23, 12345678901234567890],
            "invalidos": ["+1 (sem sinal de mais)", "01 (sem zero à esquerda)", ".5 e 5. (precisa de dígito dos dois lados do ponto)",
                          "0x1F (sem hexadecimal)", "NaN e Infinity (não existem)"],
            "observacao": "A gramática não limita tamanho nem precisão: -0 e 1E400 são JSON válidos, mas 1E400 não cabe num double. A vitrine tem esses casos.",
        },
        "espaco_em_branco": ["espaço", "tabulação", "nova linha (LF)", "retorno de carro (CR)"],
        "literais": ["true", "false", "null"],
    },
    "o_que_nao_tem": [
        {"falta": "comentários", "por_que": "Crockford conta que tirou os comentários porque começaram a ser usados como diretivas para o parser."},
        {"falta": "vírgula sobrando", "por_que": "[1, 2,] e {\"a\": 1,} são inválidos."},
        {"falta": "NaN, Infinity, undefined", "por_que": "Só números finitos escritos em decimal; não há valor indefinido."},
        {"falta": "datas", "por_que": "Não há tipo de data. A convenção é string no formato ISO 8601 / RFC 3339, como \"2024-02-29T13:45:00Z\"."},
        {"falta": "binário", "por_que": "Bytes viajam como string, em geral em Base64."},
        {"falta": "referências e ciclos", "por_que": "É uma árvore: um valor não aponta para outro."},
    ],
    "armadilhas": [
        {
            "nome": "chaves duplicadas",
            "regra": "A RFC 8259 diz que os nomes num objeto DEVERIAM (SHOULD) ser únicos, não que DEVEM. Com nome "
            "repetido, o comportamento é imprevisível: muitas bibliotecas ficam com o último, outras dão erro.",
            "exemplo": "{\"a\": 1, \"a\": 2}",
        },
        {
            "nome": "inteiros grandes",
            "regra": "Muitas implementações guardam número como double IEEE 754. A RFC 8259 diz que só os inteiros "
            "entre -(2^53)+1 e 2^53-1 têm interoperabilidade garantida.",
            "exemplo": 9007199254740993,
            "o_que_acontece": "Em JavaScript, JSON.parse devolve 9007199254740992. Por isso APIs costumam mandar IDs grandes como string.",
        },
        {
            "nome": "ordem das chaves",
            "regra": "Objeto é coleção não ordenada. Algumas bibliotecas preservam a ordem do arquivo, outras não; "
            "não dependa dela.",
        },
        {
            "nome": "codificação",
            "regra": "Entre sistemas abertos, UTF-8 sem BOM. Quem lê pode ignorar um BOM, mas quem escreve não deve pôr.",
        },
        {
            "nome": "U+2028 e U+2029",
            "regra": "Separadores de linha e parágrafo são válidos dentro de string JSON, mas eram proibidos em "
            "string JavaScript até o ES2019. JSON só virou subconjunto exato do JavaScript a partir daí.",
        },
        {
            "nome": "charset",
            "regra": "application/json não define parâmetro charset; a RFC 8259 diz que acrescentar um não tem efeito "
            "em quem segue o padrão.",
        },
    ],
    "fontes": [
        "https://www.rfc-editor.org/rfc/rfc8259",
        "https://www.rfc-editor.org/rfc/rfc7159",
        "https://www.rfc-editor.org/rfc/rfc4627",
        "https://ecma-international.org/publications-and-standards/standards/ecma-404/",
        "https://www.rfc-editor.org/rfc/rfc6901",
        "https://www.rfc-editor.org/rfc/rfc6902",
        "https://www.rfc-editor.org/rfc/rfc7493",
        "https://www.rfc-editor.org/rfc/rfc9535",
        "https://jsonlines.org",
        "https://json-schema.org",
        "https://www.json.org",
    ],
}

# ---- Exemplo 2: vitrine das capacidades. ----

# Sugestões que o guia mostra; os testes conferem que cada busca acha algo e cada jq compila e
# devolve resultado neste arquivo.
BUSCAS = [
    {"buscar": "Ganimedes", "acha": "os registros da cliente fictícia Ganimedes Lua (pedidos e mensagens)"},
    {"buscar": "ação", "acha": "texto com acento; a busca ignorando caixa também acha AÇÃO"},
    {"buscar": "🦜", "acha": "emoji dentro de strings"},
    {"buscar": "temperatura", "acha": "as leituras de sensor"},
    {"buscar": "PED-0042", "acha": "um pedido pelo código"},
    {"buscar": "\"tipo\":\"mensagem\"", "acha": "o texto cru de cada mensagem (a busca é sobre os bytes da linha)"},
    {"buscar": "fundo", "acha": "a chave no fim do aninhamento de 12 níveis"},
]
REGEX = [
    {"regex": "PED-00[0-9]{2}\\b", "acha": "códigos de pedido de PED-0000 a PED-0099"},
    {"regex": "\"nivel\":\"(erro|aviso)\"", "acha": "eventos de erro ou aviso"},
]
CONSULTAS = [
    {"jq": "select(.tipo == \"pedido\") | .total", "faz": "o total de cada pedido"},
    {"jq": "select(.tipo == \"evento\" and .nivel == \"erro\") | .mensagem", "faz": "só as mensagens de erro"},
    {"jq": "select(.tipo == \"leitura\") | {sensor, temperatura: .medidas.temperatura}", "faz": "monta um objeto novo por leitura"},
    {"jq": "select(.tipo == \"pedido\") | .itens | length", "faz": "quantos itens cada pedido tem"},
    {"jq": "select(.tipo == \"amostra\") | .nome", "faz": "lista as amostras deste arquivo"},
    {"jq": "select(.nome == \"inteiros grandes\") | .valor.alem_do_double", "faz": "mostra que o inteiro de 20 dígitos chega exato"},
    {"jq": "keys", "faz": "as chaves de cada registro, em ordem alfabética"},
]
CAMINHOS = [
    {"valor": "o \"fundo\" do aninhamento", "jq": ".valor.n1.n2.n3.n4.n5.n6.n7.n8.n9.n10.n11.fundo"},
    {"valor": "chave com espaço", "jq": ".valor[\"chave com espaço\"]"},
    {"valor": "chave vazia", "jq": ".valor[\"\"]"},
    {"valor": "chave com barra e til (escapes do JSON Pointer)", "pointer": "/valor/a~1b~0c"},
]

GUIA = {
    "tipo": "guia",
    "titulo": "Vitrine do Jasão",
    "sobre": "Exemplo embutido no Jasão: um JSON Lines (um registro por linha) feito para mostrar o que o app faz. "
    "Tudo aqui é fictício. Para ver o seu arquivo, use Abrir… no canto superior direito.",
    "experimente": {
        "painel_de_entrada": "Início (⌘0) mostra tamanho, registros, campos e as linhas inválidas propositais deste arquivo.",
        "arvore": "⌘1. Setas andam e abrem; ← volta ao pai. O registro 3 tem 12 níveis de aninhamento; o 4, uma lista de 1.000 números.",
        "texto": "⌘2. Uma linha por registro, como está no arquivo. Clicar num valor abre a árvore nele.",
        "regua": "A faixa à direita é o arquivo inteiro: cada cor é um valor de .tipo (veja a legenda) e as "
        "marcas vermelhas são as linhas inválidas. Arraste ou clique para pular.",
        "ir_para_linha": "Digite 1500 em Ir para linha.",
        "ver_formatado": "Selecione um registro e use Ver formatado para vê-lo indentado.",
        "copiar_caminho": "Selecione qualquer valor e use Copiar caminho.",
        "busca": BUSCAS,
        "busca_regex": REGEX,
        "consulta_jq": CONSULTAS,
        "caminhos": CAMINHOS,
    },
}

NOMES = ["Ganimedes Lua", "Calisto Bruma", "Europa Vento", "Io Cometa", "Titã Neblina", "Tritão Orvalho", "Fobos Areia", "Deimos Sereno"]
PRODUTOS = [("SKU-ABACAXI", "Abacaxi de plástico", "12.90"), ("SKU-BULE", "Bule que não ferve", "48.00"),
            ("SKU-CORDA", "Corda de 7 metros e meio", "19.75"), ("SKU-DADO", "Dado de 13 faces", "6.50"),
            ("SKU-ESCADA", "Escada só de subida", "230.00"), ("SKU-FARO", "Farol de mesa", "89.99")]
SENSORES = ["estufa-norte", "estufa-sul", "porão", "telhado", "câmara-fria"]
NIVEIS = ["info", "info", "info", "aviso", "info", "erro", "info", "debug"]
SERVICOS = ["fila", "cobranca", "catalogo", "entrega"]
FRASES = ["pedido recebido", "estoque reservado", "pagamento confirmado", "nota emitida", "rota calculada",
          "tentativa de conexão falhou", "tempo de resposta acima do normal", "cache aquecido"]
CANAIS = ["e-mail", "chat", "telefone"]
TEXTOS = ["Olá! Meu abacaxi de plástico chegou amassado 🦜", "A escada só sobe. Isso é esperado?",
          "Ação rápida, obrigado!", "Dá para trocar o dado de 13 faces por um de 12?",
          "Recebi dois bules. Devolvo um?", "Quando chega a corda de 7 metros e meio?"]


# Valores inteligentes: PNG 16×16 gerado por código (xadrez em degradê) e JWT fictício, sem
# segredo; a assinatura é texto qualquer, o Jasão nunca a confere.
PNG = (
    "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAABK0lEQVR42l3PoW6GMBSG4f+OdgGViEoEEjGJQCIqmzeViEkEEoFEVCImEUhEZUVl/bpMDbXskBz15jzie319//zdO6WnGIqj/O+flJMSKInykiD3ZEN2ZAnySQ7kRH6A2BMN0REliCcxEBPxAa6ey3A5LgmukytwJa7XG4fi0BwNx/+nD46ZY+Xwst9gV+yavWGXYJ/ZV3Yv+w02xabZGjYJtpltZfOy32BRLJqlYZFgmVlWFi/7Y/TUMxkmxyRHTydTYEpMDzD2jIbRMUownoyBMTE+gO2xBuuwEtgTG7AJ+wBDz2AYHIMEw8kQGBLDPbpTdJquoZOju5lupfOy36BVtJq2oZWgnWlXWi/7DWpFrakbagnqmXql9rLfoFJUmqqhkqCaqVYqL/sv4fhHn5VuItAAAAAASUVORK5CYII="
)
_b64 = lambda o: base64.urlsafe_b64encode(json.dumps(o, separators=(",", ":"), ensure_ascii=False).encode()).decode().rstrip("=")
JWT = ".".join([_b64({"alg": "HS256", "typ": "JWT"}),
                _b64({"sub": "cliente-ficticio-42", "nome": "Ganimedes Lua", "iat": 1700000000, "exp": 1900000000}),
                "assinatura-ficticia-nao-conferida"])


def aninhado(n: int):
    no = {"fundo": "você chegou ao fim: 12 níveis abaixo da raiz"}
    for i in range(n - 1, 0, -1):
        no = {f"n{i}": no}
    return no


AMOSTRAS = [
    {"tipo": "amostra", "nome": "todos os tipos", "valor": {
        "objeto": {"a": 1}, "array": [1, "dois", 3.0], "string": "texto", "numero_inteiro": 42,
        "numero_decimal": -0.125, "verdadeiro": True, "falso": False, "nulo": None,
        "objeto_vazio": {}, "array_vazio": [], "string_vazia": ""}},
    {"tipo": "amostra", "nome": "aninhamento fundo", "valor": aninhado(12)},
    {"tipo": "amostra", "nome": "array longo", "valor": [i * i % 997 for i in range(1000)]},
    {"tipo": "amostra", "nome": "unicode e escapes", "valor": {
        "acentos": "ação, maçã, pão, coração, avô, Ação em caixa alta: AÇÃO",
        "emoji": "🦜 papagaio, 👩🏽‍🚀 astronauta (sequência com junção), 🇧🇷 bandeira",
        "outras_escritas": "日本語 · العربية · Ελληνικά · русский · עברית",
        "escapes": "aspas \" barra \\ tab\t nova linha\n retorno\r",
        "controle_escapado": "\u0001\u001f",
        "fora_do_plano_basico": "𝄞 (clave de sol, U+1D11E)",
        "separadores_js": "linha parágrafo fim"}},
    {"tipo": "amostra", "nome": "números extremos", "valor": {
        "zero_negativo": "-0", "minimo_positivo_double": "5e-324", "maior_double": "1.7976931348623157e308",
        "alem_do_double": "1e400", "muitas_casas": "3.14159265358979323846264338327950288419716939937510"}},
    {"tipo": "amostra", "nome": "inteiros grandes", "valor": {
        "maior_seguro_js": 9007199254740991, "primeiro_inseguro_js": 9007199254740993,
        "alem_do_double": 12345678901234567890, "quatrocentos_digitos": "9" * 400}},
    {"tipo": "amostra", "nome": "string longa", "valor": " ".join(
        f"Parágrafo {i}: o Jasão nunca põe o arquivo inteiro na tela; pede ao Rust só a janela que cabe." for i in range(1, 121))},
    {"tipo": "amostra", "nome": "chaves difíceis", "valor": {
        "chave com espaço": 1, "": "chave vazia", "a/b~c": "barra e til", "123": "chave que parece número",
        "ação": "chave com acento", "aspas\"dentro": True}},
    {"tipo": "amostra", "nome": "valores inteligentes", "valor": {
        "criado_em": 1700000000, "id": 1700000000, "atualizado_ms": 1763900000000,
        "ultima_visita": "2026-11-20T09:30:00-03:00",
        "cor_marca": "#2f81f7", "cor_fundo": "rgb(246, 248, 250)", "cor_alerta": "rgba(207, 34, 46, 0.5)",
        "site": "https://exemplo.test/produtos?id=42",
        "logo": f"data:image/png;base64,{PNG}", "miniatura": PNG, "token": JWT,
        "payload_texto": json.dumps({"evento": "compra", "itens": [{"sku": "PED-0042", "qtd": 2}],
                                     "total": 12345678901234567890}, separators=(",", ":"))}},
]


def numero_cru(texto: str) -> str:
    # Números extremos vão como estão no texto, sem passar pelo float do Python.
    return texto


def registro(i: int):
    """Registro i (base 0) da parte gerada. Blocos com mistura diferente para a régua ter desenho."""
    bloco = i // 150
    sorteio = (i * 7 + bloco * 3) % 10
    if bloco == 3 and sorteio < 7:  # rajada de leituras de sensor
        tipo = "leitura"
    elif bloco == 6 and sorteio < 6:  # dia de muitas mensagens
        tipo = "mensagem"
    else:
        tipo = ["evento", "evento", "evento", "evento", "pedido", "pedido", "pedido", "leitura", "leitura", "mensagem"][sorteio]
    momento = f"2031-03-{1 + i // 96:02d}T{(i % 96) // 4:02d}:{(i % 4) * 15:02d}:00Z"
    if tipo == "evento":
        nivel = NIVEIS[i % len(NIVEIS)]
        r = {"tipo": tipo, "momento": momento, "nivel": nivel, "servico": SERVICOS[i % 4], "mensagem": FRASES[i % len(FRASES)]}
        if nivel == "erro":
            r["erro"] = {"codigo": 500 + i % 4, "tentativas": [{"n": k, "ms": 100 * (k + 1)} for k in range(1 + i % 3)]}
        return r
    if tipo == "pedido":
        itens = [{"sku": PRODUTOS[(i + k) % 6][0], "nome": PRODUTOS[(i + k) % 6][1], "qtd": 1 + (i + k) % 3,
                  "preco": PRODUTOS[(i + k) % 6][2]} for k in range(1 + i % 4)]
        total = sum(int(it["preco"].replace(".", "")) * it["qtd"] for it in itens)
        return {"tipo": tipo, "codigo": f"PED-{i:04d}", "momento": momento, "cliente": NOMES[i % len(NOMES)],
                "itens": itens, "total": f"{total // 100}.{total % 100:02d}", "pago": i % 5 != 0,
                "entrega": None if i % 6 == 0 else {"prazo_dias": 2 + i % 9, "rastreio": f"RASTRO-{i:05d}"}}
    if tipo == "leitura":
        t = 180 + (i * 37) % 120  # décimos de grau
        return {"tipo": tipo, "momento": momento, "sensor": SENSORES[i % len(SENSORES)],
                "medidas": {"temperatura": f"{t // 10}.{t % 10}", "umidade": 40 + (i * 13) % 50,
                            "bateria": None if i % 11 == 0 else round(1 - (i % 100) / 100, 2)},
                "ok": t < 290}
    return {"tipo": "mensagem", "momento": momento, "de": NOMES[i % len(NOMES)], "canal": CANAIS[i % 3],
            "texto": TEXTOS[i % len(TEXTOS)], "lida": i % 4 == 0, "anexos": [] if i % 3 else [f"foto-{i}.png"]}


# Linhas inválidas de propósito, cada uma com um motivo diferente, inseridas antes do registro de
# mesmo índice. O teste confere a quantidade.
INVALIDAS = {
    40: '{"tipo":"evento","nivel":"info",}',                      # vírgula sobrando
    310: "{'tipo': 'evento', 'nivel': 'info'}",                     # aspas simples
    555: '{"tipo":"leitura","medidas":{"temperatura":21,5}}',       # vírgula decimal
    800: '{"tipo":"pedido","codigo":"PED-TRUNCADO","itens":[{"sku"',  # linha cortada no meio
    1001: '{"tipo":"evento"} // comentário não existe em JSON',     # texto depois do valor
    1240: "",                                                        # linha vazia
}
GERADOS = 1400


def vitrine() -> list[str]:
    saida = [linha(GUIA)]
    for a in AMOSTRAS:
        texto = linha(a)
        if a["nome"] == "números extremos":
            # Troca as strings por números crus, para o arquivo ter os números de verdade.
            for k, v in a["valor"].items():
                texto = texto.replace(f'"{k}":"{v}"', f'"{k}":{numero_cru(v)}')
        if a["nome"] == "inteiros grandes":
            texto = texto.replace('"9' + "9" * 399 + '"', "9" * 400)
        saida.append(texto)
    # Chave duplicada: fiel ao byte, a árvore mostra as duas.
    saida.append('{"tipo":"amostra","nome":"chave duplicada","valor":{"a":1,"a":2},"nota":"A RFC 8259 não proíbe; o Jasão mostra as duas como estão."}')
    for i in range(GERADOS):
        if i in INVALIDAS:
            saida.append(INVALIDAS[i])
        saida.append(linha(registro(i)))
    return saida


def main() -> None:
    DESTINO.mkdir(parents=True, exist_ok=True)
    (DESTINO / "historia.json").write_text(linha(HISTORIA) + "\n", encoding="utf-8")
    (DESTINO / "vitrine.jsonl").write_text("\n".join(vitrine()) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
