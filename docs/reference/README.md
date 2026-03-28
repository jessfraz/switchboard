# Reference Catalog

Generated from the live `switchboard` tool registry, committed CLI inventories, and curated provider manifests.

## Snapshot

- Tools: `684`
- Curated: `15`
- Raw inventory passthrough: `669`
- Planning-only: `4`
- Undoable: `1`

## Providers

- [github](providers/github.md) for `193` tools (`7` curated, `186` raw)
- [google](providers/google.md) for `491` tools (`8` curated, `483` raw)

## Machine-readable Outputs

- [catalog.json](catalog.json)
- [support-matrix.md](support-matrix.md)
- [man page](man/switchboard.1)
- [shell completions](completions/)

## LLM Notes

- Prefer curated tools first, raw tools second.
- Prefer `--json` for reads and `--draft` before `--apply` for writes.
- The committed `catalog.json` is the fastest way to inspect the whole surface area without scraping help output.
