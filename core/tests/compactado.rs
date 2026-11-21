//! Entrada gzip e zstd: abre pela mesma função do cru, com cache, limites e cancelamento.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use nucleo::{ArquivoLinhas, Opcoes};
use tempfile::TempDir;

const JSONL: &[u8] = b"{\"tipo\":\"pedido\",\"id\":1}\n{\"tipo\":\"pedido\",\"id\":2}\n";

fn gz(dados: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(dados).unwrap();
    e.finish().unwrap()
}

fn zst(dados: &[u8]) -> Vec<u8> {
    zstd::encode_all(dados, 3).unwrap()
}

struct Caso {
    pasta: TempDir,
}

impl Caso {
    fn novo() -> Self {
        Self {
            pasta: tempfile::tempdir().unwrap(),
        }
    }
    fn cache(&self) -> PathBuf {
        self.pasta.path().join("cache")
    }
    fn op(&self) -> Opcoes<'static> {
        Opcoes {
            dir_cache: Some(self.cache()),
            ..Opcoes::default()
        }
    }
    fn escrever(&self, nome: &str, dados: &[u8]) -> PathBuf {
        let p = self.pasta.path().join(nome);
        fs::write(&p, dados).unwrap();
        p
    }
    /// Arquivos no cache (completos e parciais).
    fn no_cache(&self) -> Vec<PathBuf> {
        match fs::read_dir(self.cache()) {
            Ok(it) => it.map(|e| e.unwrap().path()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn abrir(p: &Path, op: &Opcoes) -> ArquivoLinhas {
    ArquivoLinhas::abrir_com(p, op).unwrap()
}

#[test]
fn gzip_e_zstd_validos() {
    let c = Caso::novo();
    for (nome, dados) in [("a.jsonl.gz", gz(JSONL)), ("a.jsonl.zst", zst(JSONL))] {
        let a = abrir(&c.escrever(nome, &dados), &c.op());
        assert_eq!(a.bytes(), JSONL, "{nome}");
        assert_eq!(a.linhas(), 2);
    }
    assert_eq!(c.no_cache().len(), 2);
}

#[test]
fn varios_membros_e_quadros() {
    let c = Caso::novo();
    let (a, b) = JSONL.split_at(10);
    let gzs = [gz(a), gz(b)].concat();
    let zsts = [zst(a), zst(b), zst(b"")].concat();
    for (nome, dados) in [("m.gz", gzs), ("m.zst", zsts)] {
        let arq = abrir(&c.escrever(nome, &dados), &c.op());
        assert_eq!(arq.bytes(), JSONL, "{nome}");
    }
}

#[test]
fn truncado_no_meio_da_erro_claro_e_nao_deixa_cache() {
    let c = Caso::novo();
    let grande = gerado(2 << 20);
    for (nome, dados) in [("t.gz", gz(&grande)), ("t.zst", zst(&grande))] {
        let meio = &dados[..dados.len() / 2];
        let e = ArquivoLinhas::abrir_com(c.escrever(nome, meio), &c.op())
            .err()
            .unwrap_or_else(|| panic!("{nome} truncado abriu"));
        assert_eq!(e.kind(), ErrorKind::InvalidData, "{nome}: {e}");
        assert!(e.to_string().contains("truncado"), "{e}");
    }
    assert!(c.no_cache().is_empty(), "{:?}", c.no_cache());
}

#[test]
fn extensao_mentirosa() {
    let c = Caso::novo();
    // Cru com nome de gz: abre como cru, sem tocar no cache.
    let cru = abrir(&c.escrever("cru.gz", JSONL), &c.op());
    assert_eq!(cru.bytes(), JSONL);
    assert!(c.no_cache().is_empty());
    // gzip e zstd com nome de jsonl: descompactam.
    let g = abrir(&c.escrever("g.jsonl", &gz(JSONL)), &c.op());
    let z = abrir(&c.escrever("z.txt", &zst(JSONL)), &c.op());
    assert_eq!((g.bytes(), z.bytes()), (JSONL, JSONL));
}

#[test]
fn bomba_pequena_para_com_mensagem() {
    let c = Caso::novo();
    let zeros = vec![b'\n'; 40 << 20];
    let bomba = c.escrever("bomba.zst", &zst(&zeros));
    let op = Opcoes {
        max_razao: 100,
        ..c.op()
    };
    let e = ArquivoLinhas::abrir_com(&bomba, &op)
        .err()
        .expect("bomba abriu");
    assert_eq!(e.kind(), ErrorKind::FileTooLarge);
    assert!(e.to_string().contains("bomba"), "{e}");
    // Teto absoluto de saída também vale.
    let op = Opcoes {
        max_saida: 1000,
        ..c.op()
    };
    let e = ArquivoLinhas::abrir_com(c.escrever("b.gz", &gz(&[b'x'; 5000])), &op)
        .err()
        .expect("passou do teto");
    assert_eq!(e.kind(), ErrorKind::FileTooLarge);
    assert!(c.no_cache().is_empty(), "{:?}", c.no_cache());
}

fn gerado(bytes: u64) -> Vec<u8> {
    let mut v = Vec::new();
    nucleo::gerador::gerar(&mut v, bytes, 7).unwrap();
    v
}

#[test]
fn cancelar_apaga_o_parcial() {
    let c = Caso::novo();
    // Compactado com vários MiB, para o progresso chegar a ser chamado no meio.
    let dados = gerado(64 << 20);
    let p = c.escrever("grande.jsonl.gz", &gz(&dados));
    assert!(fs::metadata(&p).unwrap().len() > 3 << 20);

    let parar = AtomicBool::new(false);
    let vistos = std::cell::Cell::new(0);
    let progresso = |lidos: u64, total: u64| {
        assert!(lidos <= total);
        vistos.set(vistos.get() + 1);
        parar.store(true, Ordering::Relaxed);
    };
    let op = Opcoes {
        cancelar: Some(&parar),
        progresso: Some(&progresso),
        ..c.op()
    };
    let e = ArquivoLinhas::abrir_com(&p, &op)
        .err()
        .expect("não cancelou");
    assert_eq!(e.kind(), ErrorKind::Interrupted);
    assert_eq!(vistos.get(), 1);
    assert!(c.no_cache().is_empty(), "{:?}", c.no_cache());

    // Sem cancelar, abre inteiro e o progresso termina em total/total.
    let ultimo = std::cell::Cell::new((0, 1));
    let progresso = |l, t| ultimo.set((l, t));
    let op = Opcoes {
        progresso: Some(&progresso),
        ..c.op()
    };
    assert_eq!(abrir(&p, &op).bytes(), dados);
    let (l, t) = ultimo.get();
    assert_eq!(l, t);
}

#[test]
fn reaproveita_e_invalida_o_cache() {
    let c = Caso::novo();
    let p = c.escrever("r.gz", &gz(JSONL));
    abrir(&p, &c.op());
    let [entrada] = c.no_cache().try_into().unwrap();

    // Reaproveita: se o cache fosse refeito, o conteúdo mexido sumiria.
    fs::write(&entrada, b"{\"do\":\"cache\"}\n").unwrap();
    assert_eq!(abrir(&p, &c.op()).bytes(), b"{\"do\":\"cache\"}\n");

    // Original mudou (tamanho e data): descompacta de novo, em outra entrada.
    let novo = b"{\"tipo\":\"novo\",\"id\":3}\n";
    fs::write(&p, gz(novo)).unwrap();
    let f = fs::File::options().write(true).open(&p).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    assert_eq!(abrir(&p, &c.op()).bytes(), novo);
    assert_eq!(c.no_cache().len(), 2);
}

#[test]
fn teto_do_cache_tira_o_mais_antigo() {
    let c = Caso::novo();
    let a = c.escrever("a.gz", &gz(&[b'a'; 600]));
    let b = c.escrever("b.gz", &gz(&[b'b'; 600]));
    let op = Opcoes {
        teto_cache: 1000,
        ..c.op()
    };
    abrir(&a, &op);
    let [antiga] = c.no_cache().try_into().unwrap();
    abrir(&b, &op);
    let [nova] = c.no_cache().try_into().unwrap();
    assert_ne!(antiga, nova);
    assert_eq!(fs::read(nova).unwrap(), [b'b'; 600]);
}

#[test]
fn compactado_vazio_abre_sem_linhas() {
    let c = Caso::novo();
    let a = abrir(&c.escrever("v.gz", &gz(b"")), &c.op());
    assert_eq!(a.linhas(), 0);
}
