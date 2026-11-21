//! JSONL sintético e determinístico para testes e benchmarks.
//!
//! Três formatos de registro, distinguidos pelo campo `tipo`: `evento`, `pedido` e `sensor`.
//! Mesma semente e mesmo tamanho geram os mesmos bytes. Tudo inventado: nomes fictícios e
//! IPs da faixa de documentação 192.0.2.0/24.

use std::io::{self, Write};

const NOMES: &[&str] = &[
    "Ana Exemplo",
    "Bruno Teste",
    "Carla Fictícia",
    "Davi Modelo",
    "Érica Amostra",
    "Fábio Ensaio",
];
const NIVEIS: &[&str] = &["info", "aviso", "erro"];
const MENSAGENS: &[&str] = &[
    "conexão aceita",
    "tempo esgotado ao ler resposta",
    "cache renovado",
    "requisição com \\\"aspas\\\" no corpo",
    "fila acima do limite",
];

/// Gerador splitmix64: poucas linhas, determinístico, sem dependência.
struct Rng(u64);

impl Rng {
    fn proximo(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn ate(&mut self, n: u64) -> u64 {
        self.proximo() % n
    }

    fn escolher<'a>(&mut self, lista: &[&'a str]) -> &'a str {
        lista[self.ate(lista.len() as u64) as usize]
    }
}

/// Escreve linhas até passar de `bytes_alvo` bytes; devolve quantas linhas escreveu.
pub fn gerar(saida: &mut impl Write, bytes_alvo: u64, semente: u64) -> io::Result<u64> {
    let mut rng = Rng(semente);
    let mut escritos = 0u64;
    let mut id = 0u64;
    let mut linha = String::with_capacity(512);
    while escritos < bytes_alvo {
        id += 1;
        linha.clear();
        registro(&mut rng, id, &mut linha);
        linha.push('\n');
        saida.write_all(linha.as_bytes())?;
        escritos += linha.len() as u64;
    }
    Ok(id)
}

fn registro(rng: &mut Rng, id: u64, s: &mut String) {
    use std::fmt::Write as _;
    let quando = format!(
        "2026-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        1 + rng.ate(12),
        1 + rng.ate(28),
        rng.ate(24),
        rng.ate(60),
        rng.ate(60)
    );
    // `write!` em String não falha.
    let _ = match rng.ate(3) {
        0 => write!(
            s,
            r#"{{"tipo":"evento","id":{id},"quando":"{quando}","origem":"192.0.2.{}","nivel":"{}","mensagem":"{}"}}"#,
            1 + rng.ate(254),
            rng.escolher(NIVEIS),
            rng.escolher(MENSAGENS),
        ),
        1 => {
            let _ = write!(
                s,
                r#"{{"tipo":"pedido","id":{id},"quando":"{quando}","cliente":"{}","itens":["#,
                rng.escolher(NOMES)
            );
            let mut total = 0u64;
            for i in 0..1 + rng.ate(4) {
                let (qtd, centavos) = (1 + rng.ate(5), 100 + rng.ate(99_900));
                total += qtd * centavos;
                let sep = if i == 0 { "" } else { "," };
                let _ = write!(
                    s,
                    r#"{sep}{{"sku":"SKU-{:05}","qtd":{qtd},"preco":"{}.{:02}"}}"#,
                    rng.ate(100_000),
                    centavos / 100,
                    centavos % 100
                );
            }
            write!(s, r#"],"total":"{}.{:02}"}}"#, total / 100, total % 100)
        }
        _ => write!(
            s,
            r#"{{"tipo":"sensor","id":{id},"quando":"{quando}","leituras":{{"temperatura":{}.{},"umidade":{}}},"ativo":{},"nota":null}}"#,
            rng.ate(40),
            rng.ate(10),
            rng.ate(100),
            rng.ate(2) == 0,
        ),
    };
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn deterministico_e_com_tres_tipos() {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let n = gerar(&mut a, 20_000, 7).unwrap();
        gerar(&mut b, 20_000, 7).unwrap();
        assert_eq!(a, b);
        assert!(a.len() >= 20_000);
        let texto = std::str::from_utf8(&a).unwrap();
        assert_eq!(texto.lines().count() as u64, n);
        for tipo in ["evento", "pedido", "sensor"] {
            assert!(
                texto.contains(&format!(r#""tipo":"{tipo}""#)),
                "falta {tipo}"
            );
        }
        let mut c = Vec::new();
        gerar(&mut c, 20_000, 8).unwrap();
        assert_ne!(a, c);
    }
}
