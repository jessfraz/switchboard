# Reference Catalog

Generated from the live `switchboard` tool registry, committed CLI inventories, and curated provider manifests.

## Snapshot

- Tools: `768`
- Curated: `15`
- Raw inventory passthrough: `753`
- Planning-only: `4`
- Undoable: `1`

## Providers

- [github](providers/github.md) for `198` tools (`7` curated, `191` raw)
- [google](providers/google.md) for `506` tools (`8` curated, `498` raw)
- [mychart](providers/mychart.md) for `32` tools (`0` curated, `32` raw)
- [schwab](providers/schwab.md) for `32` tools (`0` curated, `32` raw)

## Machine-readable Outputs

- [catalog.json](catalog.json)
- [support-matrix.md](support-matrix.md)
- [man page](man/switchboard.1)
- [shell completions](completions/)

## LLM Notes

- Prefer curated tools first, raw tools second.
- Prefer `--json` for reads and `--draft` before `--apply` for writes.
- The committed `catalog.json` is the fastest way to inspect the whole surface area without scraping help output.
