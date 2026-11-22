//! Exemplos embutidos no binário, abertos quando o app começa sem arquivo.
//!
//! A interface os pede a `abrir` por um caminho sintético (`exemplo:<nome>`, como o `sample:` do
//! Leve). Nada vai para o disco: os bytes viram um `ArquivoLinhas::estatico` e daí seguem o
//! caminho de qualquer arquivo (índice, varredura, régua, árvore, texto).
//! Os arquivos são gerados por `scripts/exemplos.py`.

const PREFIXO: &str = "exemplo:";

const EXEMPLOS: [(&str, &[u8]); 2] = [
    ("historia", include_bytes!("../exemplos/historia.json")),
    ("vitrine", include_bytes!("../exemplos/vitrine.jsonl")),
];

/// Bytes do exemplo, se `caminho` for um caminho sintético conhecido.
// shortcut: um arquivo de verdade chamado literalmente `exemplo:historia`, passado relativo na
// linha de comando, abre o exemplo; o diálogo só devolve caminho absoluto.
pub fn embutido(caminho: &str) -> Option<&'static [u8]> {
    let nome = caminho.strip_prefix(PREFIXO)?;
    EXEMPLOS.iter().find(|(n, _)| *n == nome).map(|(_, b)| *b)
}

#[cfg(test)]
mod testes {
    use super::*;
    use nucleo::consulta::Consulta;
    use nucleo::resumo::{self, Conteudo};
    use nucleo::{ArquivoLinhas, Arvore, Busca};
    use serde_json::Value;
    use std::sync::atomic::AtomicBool;

    fn arquivo(nome: &str) -> ArquivoLinhas {
        ArquivoLinhas::estatico(embutido(&format!("exemplo:{nome}")).unwrap())
    }

    /// Linha e valor da amostra (`Value::Null` se o serde_json não a entender, como 1e400).
    fn amostra(nome: &str) -> (usize, Value) {
        let a = arquivo("vitrine");
        let marca = format!(r#""tipo":"amostra","nome":"{nome}""#);
        let i = (0..a.linhas())
            .find(|&i| linha_de(&a, i).contains(&marca))
            .unwrap_or_else(|| panic!("falta a amostra {nome}"));
        (
            i,
            serde_json::from_slice(a.janela(i, 1)[0]).unwrap_or_default(),
        )
    }

    fn consultar(filtro: &str, dados: &[u8]) -> (Vec<String>, u64) {
        let c = Consulta::nova(filtro).unwrap_or_else(|e| panic!("{filtro}: {e}"));
        let mut saida = Vec::new();
        let r = c.executar_jsonl(dados, &AtomicBool::new(false), |l| {
            saida.extend(l.resultados.into_iter().map(|r| r.json))
        });
        (saida, r.erros)
    }

    #[test]
    fn so_caminho_sintetico_conhecido() {
        assert!(embutido("exemplo:historia").is_some());
        assert!(embutido("exemplo:vitrine").is_some());
        assert!(embutido("exemplo:outro").is_none());
        assert!(embutido("/tmp/exemplo:historia").is_none());
        assert!(embutido("historia").is_none());
    }

    #[test]
    fn historia_e_um_json_unico_valido() {
        let a = arquivo("historia");
        assert_eq!(resumo::conteudo(&a), Conteudo::JsonUnico);
        let v: Value = serde_json::from_slice(a.bytes()).unwrap();
        for chave in [
            "historia",
            "derivados",
            "gramatica",
            "o_que_nao_tem",
            "armadilhas",
            "fontes",
        ] {
            assert!(v.get(chave).is_some(), "falta {chave}");
        }
        assert_eq!(v["gramatica"]["tipos"].as_array().unwrap().len(), 6);
        let texto = String::from_utf8_lossy(a.bytes());
        for rfc in [
            "RFC 4627", "RFC 7159", "RFC 8259", "ECMA-404", "RFC 6901", "RFC 9535",
        ] {
            assert!(texto.contains(rfc), "falta {rfc}");
        }
        // A árvore abre o documento como um registro só, sem erro.
        let arvore = Arvore::new(a);
        assert_eq!(arvore.registros(), 1);
        assert!(arvore.janela(0, 1)[0].erro.is_none());
    }

    #[test]
    fn vitrine_tem_registros_e_as_invalidas_propositais() {
        let a = arquivo("vitrine");
        assert_eq!(resumo::conteudo(&a), Conteudo::Objetos);
        let marcas = nucleo::regua::marcas(a.linhas());
        let p = resumo::varrer(&a, &AtomicBool::new(false), &marcas, |_| {});
        assert!(p.fim);
        // 5 inválidas com motivos diferentes + 1 linha vazia (scripts/exemplos.py, INVALIDAS).
        assert_eq!(p.invalidas, 6);
        assert!(
            a.linhas() > 1_000,
            "régua e Ir para linha precisam de volume"
        );
        // Motivo de cada inválida pelo `nucleo::erros` (o do `jasao erros`): um diferente por linha.
        let mut pilha = Vec::new();
        let mut motivos: Vec<&str> = (0..a.linhas())
            .filter_map(|i| nucleo::erros::validar_linha(a.janela(i, 1)[0], &mut pilha).err())
            .map(|(_, m)| m.codigo())
            .collect();
        motivos.sort();
        motivos.dedup();
        assert_eq!(motivos.len(), 5, "{motivos:?}");
    }

    #[test]
    fn vitrine_colore_a_regua_por_tipo() {
        let a = arquivo("vitrine");
        let inf = nucleo::estrutura::inferir(a.bytes(), &AtomicBool::new(false), |_| {});
        let ds = inf.resumo.discriminadores();
        let d = nucleo::regua::escolher(&ds).expect("sem discriminador");
        assert_eq!(d.campo, ".tipo");
        let legenda = nucleo::regua::legenda(d);
        // As 4 cores e ainda "outros" (guia e amostras).
        assert_eq!(legenda.variantes.len(), 4);
        assert!(legenda.outros > 0);
        assert!(resumo::campos(&inf.resumo).unwrap() > 10);
    }

    #[test]
    fn vitrine_tem_cada_capacidade_da_arvore_e_do_texto() {
        // Todos os tipos, e vazios.
        let (_, r) = amostra("todos os tipos");
        let tipos: Vec<&str> = r["valor"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| match v {
                Value::Object(_) => "objeto",
                Value::Array(_) => "lista",
                Value::String(_) => "texto",
                Value::Number(_) => "numero",
                Value::Bool(_) => "booleano",
                Value::Null => "nulo",
            })
            .collect();
        for t in ["objeto", "lista", "texto", "numero", "booleano", "nulo"] {
            assert!(tipos.contains(&t), "falta {t}");
        }
        // Array longo.
        assert!(amostra("array longo").1["valor"].as_array().unwrap().len() >= 1_000);
        // Unicode e escapes.
        let u = amostra("unicode e escapes").1["valor"].to_string();
        for s in ["🦜", "日本語", "\\n", "\\\"", "\\u0001", "𝄞", "\u{2028}"] {
            assert!(u.contains(s), "falta {s}");
        }
        // String longa: passa do teto de colunas do Texto, que corta e mostra "+N bytes".
        let a = arquivo("vitrine");
        let (i, _) = amostra("string longa");
        assert!(a.janela(i, 1)[0].len() > nucleo::texto::MAX_COLUNAS);
        // Números extremos, como estão no arquivo (o parser do teste não precisa entendê-los).
        let (i, _) = amostra("números extremos");
        let linha = String::from_utf8_lossy(a.janela(i, 1)[0]).into_owned();
        for n in [":-0,", "5e-324", "1.7976931348623157e308", "1e400"] {
            assert!(linha.contains(n), "falta {n}");
        }
        // Chave duplicada fica como está (fiel ao byte).
        assert!(String::from_utf8_lossy(a.bytes()).contains(r#"{"a":1,"a":2}"#));
        // Aninhamento: revelar o "fundo" abre 12 níveis abaixo do registro.
        let (i, _) = amostra("aninhamento fundo");
        let byte = linha_de(&a, i).find("\"fundo\"").unwrap() + 1;
        let mut arvore = Arvore::new(arquivo("vitrine"));
        let v = arvore.revelar(i, byte).unwrap();
        let l = &arvore.janela(v, 1)[0];
        assert_eq!(l.prof, 13);
        assert_eq!(
            arvore.caminho(v).unwrap().jq(),
            ".valor.n1.n2.n3.n4.n5.n6.n7.n8.n9.n10.n11.fundo"
        );
    }

    fn linha_de(a: &ArquivoLinhas, i: usize) -> String {
        String::from_utf8_lossy(a.janela(i, 1)[0]).into_owned()
    }

    #[test]
    fn sugestoes_do_guia_funcionam() {
        let a = arquivo("vitrine");
        let guia: Value = serde_json::from_slice(a.janela(0, 1)[0]).unwrap();
        assert_eq!(guia["tipo"], "guia");
        let e = &guia["experimente"];
        let nada = AtomicBool::new(false);
        let achados = |t: &str, regex, caixa| {
            Busca::nova(t, regex, caixa)
                .unwrap()
                .executar(a.bytes(), &nada, |_| {})
                .achados
        };
        for b in e["busca"].as_array().unwrap() {
            let t = b["buscar"].as_str().unwrap();
            assert!(achados(t, false, false) > 0, "busca sem achado: {t}");
        }
        assert!(achados("ação", false, true) > achados("ação", false, false));
        for b in e["busca_regex"].as_array().unwrap() {
            let t = b["regex"].as_str().unwrap();
            assert!(achados(t, true, false) > 0, "regex sem achado: {t}");
        }
        for c in e["consulta_jq"].as_array().unwrap() {
            let f = c["jq"].as_str().unwrap();
            let (saida, erros) = consultar(f, a.bytes());
            assert!(!saida.is_empty(), "jq sem resultado: {f}");
            // Só as 5 inválidas não vazias falham (linha vazia é pulada).
            assert_eq!(erros, 5, "{f}");
        }
        let (saida, _) = consultar(
            r#"select(.nome == "inteiros grandes") | .valor.alem_do_double"#,
            a.bytes(),
        );
        assert_eq!(saida, ["12345678901234567890"]);
        // Caminhos sugeridos levam a um valor.
        for c in e["caminhos"].as_array().unwrap() {
            let Some(jq) = c["jq"].as_str() else {
                continue;
            };
            let (saida, _) = consultar(
                &format!("select(.tipo == \"amostra\") | ({jq})? // empty"),
                a.bytes(),
            );
            assert_eq!(saida.len(), 1, "{jq}");
        }
        let (_, r) = amostra("chaves difíceis");
        let p = e["caminhos"][3]["pointer"].as_str().unwrap();
        assert_eq!(r.pointer(p), Some(&Value::from("barra e til")));
    }
}
