# Pinned Google Discovery inputs

These 17 public schemas were retrieved on 2026-10-13 for `gws 0.22.5`.
`sources.json` records each source URL and the SHA-256 of its canonical JSON
(sorted keys, compact separators, UTF-8, final newline). No schema fields were
removed. The documents contain public API descriptions, not account data.

The pinned CLI still builds its commands from Discovery data at runtime. Pinning
only the executable left inventory CI dependent on Google's current API methods
and wording. `switchboard-catalog-gen generate` and `check` now verify these
inputs, copy them into an isolated temporary configuration, and run Google help
without credentials or live network fallback. Exact inventory comparison and
manifest validation remain enabled. The temporary files are removed afterwards;
user configuration and discovery caches are never used or modified.

To refresh, download every URL listed in `sources.json`, canonicalize the JSON as
above, and update its byte count and SHA-256. Keep the service aliases and CLI
version aligned with the pinned `gws` release in `.github/workflows/ci.yml`.
Then run:

```sh
cargo run -p switchboard-providers --bin switchboard-catalog-gen -- generate --provider google
cargo run -p switchboard-cli --bin switchboard-docs-gen -- generate
cargo run -p switchboard-providers --bin switchboard-catalog-gen -- check
cargo run -p switchboard-cli --bin switchboard-docs-gen -- check
```

Review schema, inventory, and documentation changes together. New services need
an explicit snapshot entry; a missing or changed input fails before lookup.
The `workflow` service is built into `gws` and needs no Discovery document.
