# Dr. J (nome provisório)

Visualizador de JSON/JSONL somente leitura. Plano em [docs/plano.md](docs/plano.md); decisões em [docs/decisoes.md](docs/decisoes.md).

| Pasta | O que é |
|---|---|
| `core/` | biblioteca Rust (pacote `nucleo`), sem Tauri: leitura e índices |
| `cli/` | linha de comando fina sobre o `core` |
| `app/` | casca Tauri 2 + Vite + TypeScript sem framework (`app/src-tauri` é o crate Rust) |

## Rodar

```bash
cargo run --release -p nucleo --example gerar -- /tmp/amostra.jsonl 100 1   # 100 MB, semente 1
cargo run --release -p cli -- linhas /tmp/amostra.jsonl
cargo run --release -p cli -- janela /tmp/amostra.jsonl 0 5
cd app && pnpm install && pnpm tauri dev -- -- /tmp/amostra.jsonl          # o caminho é opcional
```

## Testar

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # precisa de app/dist: rode `pnpm build` em app/ antes
cargo test --workspace
```

Fuzz (nightly + `cargo install cargo-fuzz`):

```bash
cd core/fuzz && cargo +nightly fuzz run indice_linhas --fuzz-dir . -- -max_total_time=30
```

## Medir

```bash
cargo bench -p nucleo --bench indice              # NUCLEO_BENCH_MB=1024 para 1 GB
```

## Trocar o nome

O nome provisório mora em dois arquivos, e só neles:

- `app/src-tauri/tauri.conf.json`: `productName` (nome do app e título da janela) e `identifier` (bundle);
- `cli/Cargo.toml`: `[[bin]] name` (nome do binário de linha de comando).

Pacotes e pastas têm nomes neutros e não mudam.
