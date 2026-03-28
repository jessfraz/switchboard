# LLM Getting Started

This page is for model callers and agent authors, not for people looking for a marketing overview.

## Rules that matter

- Prefer `--json` for reads.
- Prefer curated tools before raw passthrough tools.
- Pass `--ns` explicitly for writes.
- Repeat `--ns` only for aggregate reads.
- Draft writes first. Use `--draft` or the equivalent planning mode before `--apply`.
- Treat `*.cli.read` and `*.cli.write` as escape hatches, not defaults.
- Do not assume a CLI is multi-login-safe on its own. Namespace `state_dir` exists because some provider CLIs absolutely are not.

## Mental model

- `switchboard` is the stable local contract.
- Provider CLIs and APIs are the unstable implementation detail.
- Namespaces pin provider, account label, auth, and optional state separation.
- Policy decides whether a write is allowed, denied, or needs approval.
- Audit records what was planned, approved, rejected, applied, or compensated.

## Discovery loop

```sh
switchboard tools list --json
switchboard tools describe github.repository.search --json
switchboard tools describe google.cli.read --json
```

## Read pattern

```sh
switchboard github.notifications.list --ns github.personal --json
switchboard google.mail.search --ns google.work --query 'from:finance newer_than:7d' --json
switchboard google.calendar.list --ns google.work --ns google.personal --json
```

## Write pattern

```sh
switchboard google.calendar.create \
  --ns google.work \
  --title "Vet visit" \
  --start "2026-04-01T09:00:00-07:00" \
  --end "2026-04-01T10:00:00-07:00" \
  --draft

switchboard op approve op_1234abcd --actor codex --note "looks right"
switchboard op apply op_1234abcd --json
```

## Raw passthrough pattern

```sh
switchboard google.cli.read --ns google.work --json -- calendar +agenda --format json --today
switchboard github.cli.write --ns github.personal --draft -- --repo owner/repo issue comment 123 --body "needs tests"
```

## Failure modes to avoid

- Do not assume current shell auth equals the right namespace.
- Do not collapse multiple account contexts into one namespace.
- Do not skip planning for writes just because the provider CLI supports a direct mutation.
- Do not scrape `--help` when `tools list --json` or `catalog.json` already gives structured metadata.
