#!/usr/bin/env bash
# Gates locais: os mesmos passos de .github/workflows/ci.yml, para quando o Actions não roda.
#   scripts/gates.sh             passos do job `rust` (é o que o pre-push chama)
#   scripts/gates.sh --completo  + build do app (job `app`) + fuzz curto (job `fuzz`)
# FUZZ_SEG=10 encurta o fuzz (padrão 60 s por alvo, como no CI).
# Ao mudar o ci.yml, mude aqui também.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

passo() { printf '\n== %s\n' "$*"; "$@"; }

(cd app && passo pnpm install --frozen-lockfile && passo pnpm build)
passo cargo fmt --all --check
passo cargo clippy --workspace --all-targets -- -D warnings
passo cargo test --workspace
passo cargo bench -p nucleo --no-run

if [ "${1:-}" = "--completo" ]; then
  (cd app && passo pnpm tauri build --no-bundle)
  cargo +nightly fuzz --version >/dev/null 2>&1 || {
    echo "fuzz: falta o nightly ou o cargo-fuzz (rustup toolchain install nightly; cargo install cargo-fuzz --locked)" >&2
    exit 1
  }
  seg="${FUZZ_SEG:-60}"
  cd core/fuzz
  passo cargo +nightly fuzz run indice_linhas --fuzz-dir . -- -max_total_time="$seg"
  passo cargo +nightly fuzz run compactado --fuzz-dir . -- -max_total_time="$seg"
  passo cargo +nightly fuzz run busca --fuzz-dir . -- -max_total_time="$seg" -max_len=4096
  passo cargo +nightly fuzz run erros --fuzz-dir . -- -max_total_time="$seg" -max_len=4096
fi

printf '\nGATES OK\n'
