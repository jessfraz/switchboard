# Contributing

Thanks for improving `switchboard`.

## Ground rules

- Keep the trust boundary local. New features should preserve explicit namespaces, local policy, approval, and audit.
- Prefer first-principles fixes over wrappers around broken behavior.
- Do not hand-maintain reference tables that can be generated from live code or inventories.
- If a change affects provider coverage, update the catalog outputs instead of editing docs by hand.

## Local workflow

1. Install the toolchain with Rust or Nix.
2. Run inventory and docs generation checks before opening a PR.
3. Run formatting, clippy, and the targeted tests for your change.

Useful commands:

```sh
cargo run -p switchboard-providers --bin switchboard-catalog-gen -- check
cargo run -p switchboard-cli --bin switchboard-docs-gen -- check
cargo fmt
cargo clippy --all --benches --tests --examples --all-features
cargo test -p switchboard-cli
```

If you change the provider manifests, inventories, or tool metadata, regenerate the committed docs:

```sh
cargo run -p switchboard-cli --bin switchboard-docs-gen -- generate
```

## What to document

- New curated tools should explain the problem they solve, not just the flags they take.
- If you add a provider capability, make sure the README or provider docs explain when to use the curated tool versus the raw passthrough surface.
- If you change the CLI contract, update the LLM docs too. Models are part of the audience here, not an afterthought.

## Pull requests

- Keep PRs tightly scoped.
- Include the problem, the approach, and the commands you ran.
- Call out any follow-up work or deliberately deferred tradeoffs.

## Release-facing changes

If you touch install paths, release packaging, or docs generation, check these files too:

- `dist-workspace.toml`
- `.github/workflows/release.yml`
- `.github/workflows/pages.yml`
- `README.md`
- `docs/reference/`
- `pages/`
