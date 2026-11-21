# Contributing to Jasão

Thanks for your interest. Jasão is a small app maintained by Moralia; issues and pull requests in English or Portuguese are welcome.

- **Bugs and ideas:** open an [issue](https://github.com/moraliasp/jasao/issues/new/choose). For security problems, follow [SECURITY.md](SECURITY.md) instead.
- **Pull requests:** keep them focused, one change per PR. Before opening one, run:

  ```bash
  cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
  ```

- **Sample files:** never commit real data. Fixtures are invented (documentation IPs such as 192.0.2.x, fictional names).
- **Design decisions** are recorded in [`docs/decisoes.md`](docs/decisoes.md) (Portuguese). Read the relevant entry before changing something listed there.
- Be kind. The [GitHub Community Guidelines](https://docs.github.com/site-policy/github-terms/github-community-guidelines) apply.
