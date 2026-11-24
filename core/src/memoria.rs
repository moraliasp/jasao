//! Teto de memória do processo, para a consulta jq (achado: `[range(1e9)]|length` passava de
//! 8 GB). `Contador` é um alocador global que conta o heap vivo; com `limitar`, alocação que
//! passaria do teto é recusada, o Rust aborta o processo ("memory allocation of N bytes failed")
//! e quem o criou vê a falha. Só o heap conta: o arquivo mapeado (mmap) fica de fora.
//! Portátil (sem `setrlimit`: no macOS `RLIMIT_AS` não vale e contaria o mmap do arquivo).
//! Quem usa declara `#[global_allocator] static A: nucleo::memoria::Contador = Contador;`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

/// Escrito no stderr ao recusar a 1ª alocação: o pai reconhece a falha por ele.
pub const MARCA: &str = "jasao: passou do teto de memória da consulta";

static USADO: AtomicUsize = AtomicUsize::new(0);
static TETO: AtomicUsize = AtomicUsize::new(usize::MAX);
static AVISOU: AtomicBool = AtomicBool::new(false);

pub struct Contador;

/// Liga o teto (bytes de heap vivo) para o resto do processo.
pub fn limitar(bytes: usize) {
    TETO.store(bytes, Relaxed);
}

fn reservar(n: usize) -> bool {
    let antes = USADO.fetch_add(n, Relaxed);
    if antes.saturating_add(n) <= TETO.load(Relaxed) {
        return true;
    }
    USADO.fetch_sub(n, Relaxed);
    if !AVISOU.swap(true, Relaxed) {
        avisar();
    }
    false
}

/// Escreve a MARCA direto no fd 2: sem alocar e sem o lock de `io::stderr()`, que outra thread
/// pode estar segurando (a CLI trava o stderr enquanto consulta; pegá-lo aqui seria deadlock).
#[cfg(unix)]
fn avisar() {
    use std::os::fd::FromRawFd;
    // SAFETY: o fd 2 é do processo e o `ManuallyDrop` não o fecha.
    let mut fd = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(2) });
    let _ = fd.write_all(MARCA.as_bytes());
    let _ = fd.write_all(b"\n");
}

#[cfg(not(unix))]
fn avisar() {
    let _ = std::io::stderr().write_all(MARCA.as_bytes());
    let _ = std::io::stderr().write_all(b"\n");
}

// SAFETY: repassa ao `System` com o mesmo `Layout`; só conta os bytes e recusa (ponteiro nulo,
// que o contrato permite) quando passaria do teto.
unsafe impl GlobalAlloc for Contador {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if !reservar(l.size()) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc(l) };
        if p.is_null() {
            USADO.fetch_sub(l.size(), Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if !reservar(l.size()) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc_zeroed(l) };
        if p.is_null() {
            USADO.fetch_sub(l.size(), Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        USADO.fetch_sub(l.size(), Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, novo: usize) -> *mut u8 {
        let velho = l.size();
        if novo > velho && !reservar(novo - velho) {
            return std::ptr::null_mut();
        }
        let q = unsafe { System.realloc(p, l, novo) };
        if q.is_null() {
            if novo > velho {
                USADO.fetch_sub(novo - velho, Relaxed);
            }
        } else if novo < velho {
            USADO.fetch_sub(velho - novo, Relaxed);
        }
        q
    }
}
