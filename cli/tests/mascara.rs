//! Garantia da máscara pelo binário: nenhum comando imprime o valor de um campo sensível
//! (dados sintéticos); `--sem-mascara` mostra, `--anonimizar` troca por código.

use std::path::PathBuf;
use std::process::Command;

struct Arquivo(PathBuf);

impl Arquivo {
    fn novo(nome: &str, conteudo: &str) -> Self {
        let p = std::env::temp_dir().join(format!("jasao-mascara-{}-{nome}", std::process::id()));
        std::fs::write(&p, conteudo).unwrap();
        Arquivo(p)
    }
    fn p(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for Arquivo {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const SEGREDOS: [&str; 5] = ["S3NT1N3LA", "111.444.777", "4821", "4111", "8T0K3N"];

const DADOS: &str = concat!(
    r#"{"id":1,"nome":"ana","senha":"S3NT1N3LA-1","cvv":4821,"obs":"111.444.777-35","#,
    r#""pagamento":{"cartao":{"numero":"4111 1111 1111 1111"}}}"#,
    "\n",
    r#"{"id":2,"nome":"bia","senha":"S3NT1N3LA-2","cvv":4821,"obs":"ok","#,
    r#""pagamento":{"cartao":{"numero":"4111 1111 1111 1111"}}}"#,
    "\n",
    r#"{"id":3,"token":"8T0K3N" oops}"#,
    "\n",
);

/// stdout + stderr do comando.
fn jasao(args: &[&str]) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_jasao"))
        .args(args)
        .output()
        .unwrap();
    let s = |b: Vec<u8>| String::from_utf8(b).unwrap();
    s(o.stdout) + &s(o.stderr)
}

fn vaza(saida: &str) -> Option<&'static str> {
    SEGREDOS.iter().copied().find(|x| saida.contains(x))
}

#[test]
fn nenhum_comando_imprime_segredo() {
    let d = Arquivo::novo("dados.jsonl", DADOS);
    let outro = Arquivo::novo("outro.jsonl", &DADOS.replace("\"ana\"", "1"));
    let schema = Arquivo::novo(
        "schema.json",
        r#"{"properties":{"senha":{"type":"number"},"obs":{"enum":["x"]}}}"#,
    );
    let f = d.p();
    let comandos: Vec<Vec<&str>> = vec![
        vec!["janela", f, "0", "9"],
        vec!["buscar", f, "S3NT1N3LA"],
        vec!["buscar", f, "4111"],
        vec!["buscar", f, "a"],
        vec!["consultar", ".", f],
        vec!["consultar", ".senha, .cvv, .obs, .pagamento", f],
        vec!["consultar", ".", f, "--anonimizar"],
        vec!["consultar", ".", f, "--documento"],
        vec!["consultar", "error(.senha)", f],
        vec!["erros", f],
        vec!["erros", f, "--json"],
        vec!["inferir", f, "--json"],
        vec!["inferir", f],
        vec!["exportar", "--zod", "--literal", f],
        vec!["exportar", "--schema", "--literal", f],
        vec!["exportar", "--ts", "--literal", f],
        vec!["validar", f, "--schema", schema.p()],
        vec!["validar", outro.p(), "--como", f, "--literal"],
        vec!["diff", f, outro.p()],
        vec!["diff", f, outro.p(), "--json"],
    ];
    for c in &comandos {
        let s = jasao(c);
        assert_eq!(vaza(&s), None, "{c:?}:\n{s}");
    }
    // O crivo não é vazio: sem a máscara, o mesmo comando mostra o segredo.
    assert_eq!(
        vaza(&jasao(&["janela", f, "0", "9", "--sem-mascara"])),
        Some("S3NT1N3LA")
    );
    assert!(jasao(&["buscar", "--sem-mascara", f, "4111"]).contains("4111"));
    // Mascarado: mesmo comprimento, ainda JSON.
    let jan = jasao(&["janela", f, "0", "1"]);
    assert!(jan.contains(r#""senha":"***********","cvv":9999"#), "{jan}");
    // Anonimizado: código estável dentro da execução, diferente entre valores.
    let anon = jasao(&["consultar", ".senha, .cvv", f, "--anonimizar"]);
    let l: Vec<&str> = anon.lines().filter(|l| l.starts_with('"')).collect();
    assert!(
        l[0].starts_with("\"anon:") && l[0] != l[1] && l[1] != l[2] && l[1] == l[3],
        "{anon}"
    );
}
