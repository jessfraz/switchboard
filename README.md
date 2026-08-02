# Switchboard

`switchboard` is a local Rust CLI trust layer for humans, scripts, and LLMs.

It gives you a stable command contract across ugly real-world tools like GitHub, Google Workspace, and MyChart, with explicit namespaces, draft-first writes, approvals, audit logs, and raw provider passthrough when the curated layer is not enough.

## The Problem

Most agent setups are fine at reasoning and bad at authority boundaries.

The real pain is boring and operational:

- work and personal accounts get mixed together
- credentials end up smeared across prompts, shells, and random caches
- writes happen too easily and too opaquely
- audit trails are weak or nonexistent
- backend integrations churn, but your calling contract should not
- many useful CLIs do not support clean multi-login on one machine

That last one matters more than it sounds. Google Workspace CLI style tooling is a good example, upstream login and cache behavior are not really built around multiple isolated local identities. `switchboard` treats each namespace as its own authority domain, materializes credentials separately, and uses per-namespace state dirs so one login does not stomp another.

The product is not "AI does tasks."

The product is "automation with sane trust boundaries."

## The Solution

Keep the durable interface as a CLI.

Let models, humans, scripts, and cron all shell out to the same surface.

Make every command resolve to an explicit namespace, run through policy, produce structured output, and leave an audit trail.

```text
         Humans / Scripts / LLMs
                  |
                  v
            switchboard CLI
                  |
                  v
     namespace + policy + audit + undo
                  |
         +--------+--------+
         |                 |                 |
         v                 v                 v
      GitHub             Google           MyChart
```

That gives you a stable local contract even if the backend changes later from one CLI to another CLI or from a CLI to a direct API.

## Why CLI First

- CLIs compose well with humans, scripts, cron, and models
- CLIs are easy to inspect, test, replay, and debug
- CLIs survive ecosystem churn better than model-specific glue
- LLMs need exact commands and stable JSON more than they need another chat wrapper
- MCP can exist later as a shim, but it should not be the center of the design

## Status

This repo is real, but still in the "tighten the public surface" phase.

| Area | Status | Notes |
| --- | --- | --- |
| Source install | Ready | Build from source or use Nix today |
| Prebuilt release binaries | Wired, awaiting first tag | `cargo-dist` release pipeline is configured for checksummed archives plus shell and PowerShell installers |
| GitHub curated reads | Executable | Notifications, PR read/search, issue read, repository search |
| GitHub curated writes | Mixed | Comment tools plan cleanly, apply path is still evolving |
| Google curated reads | Mixed | Mail search/read and calendar list execute, drive search is planning-only |
| Google curated writes | Mixed | Mail draft and calendar create/delete execute, mail send is planning-only |
| MyChart raw CLI tools | Executable | Namespace-scoped `mychart-cli` passthrough, including inventory-backed raw leaf commands |
| Raw provider CLI tools | Executable | Namespace, policy, approval, and audit still apply |

This workspace also contains adjacent CLIs. The public polish and open-source hardening work is focused on `switchboard` first.

## Install Today

Today, the honest install paths are source or Nix. The release pipeline for prebuilt binaries is configured, but the first public tagged release still needs to be cut.

### From Source

```bash
cargo install --path crates/switchboard-cli
switchboard --help
```

If you do not want a global install yet:

```bash
cargo run -p switchboard-cli -- --help
```

### With Nix

```bash
nix build .#switchboard
./result/bin/switchboard --help
```

## 5-Minute Quickstart

Create a `switchboard.toml` in the repo root or at `$XDG_CONFIG_HOME/switchboard/config.toml`.

Minimal GitHub example:

```toml
[auth.github_personal]
provider = "github"
kind = "gh_cli"
account = "jessfraz"

[namespace.github.personal]
provider = "github"
account = "jessfraz"
auth = "github_personal"
default_read = true
```

Then try:

```bash
switchboard ns list
switchboard tools list
switchboard github.notifications.list --ns github.personal --json
```

If you want Google plus clean multi-login separation, use distinct namespaces and state dirs:

```toml
[secret.google_workspace_cli_client_id]
kind = "env"
name = "GOOGLE_WORKSPACE_CLI_CLIENT_ID"

[secret.google_workspace_cli_client_secret]
kind = "env"
name = "GOOGLE_WORKSPACE_CLI_CLIENT_SECRET"

[auth.google_work]
provider = "google"
kind = "google_oauth"
account = "jess@company.com"
client_id = "google_workspace_cli_client_id"
client_secret = "google_workspace_cli_client_secret"

[auth.google_personal]
provider = "google"
kind = "google_oauth"
account = "jess@example.com"
client_id = "google_workspace_cli_client_id"
client_secret = "google_workspace_cli_client_secret"

[namespace.google.work]
provider = "google"
account = "jess@company.com"
auth = "google_work"
default_read = true
auth_scope_profile = "workspace_admin"
state_dir = "/Users/jessfraz/.config/gws-work"

[namespace.google.personal]
provider = "google"
account = "jess@example.com"
auth = "google_personal"
default_read = false
state_dir = "/Users/jessfraz/.config/gws-personal"
```

That `state_dir` split is not decorative. It is how `switchboard` makes multi-login-hostile CLI tooling behave like separate local authority domains instead of one cursed shared cache.
The optional `workspace_admin` auth scope profile extends bare `gws auth login` with user, organizational-unit, group, membership, and Groups Settings access for that namespace only. Other Google namespaces retain the standard scope set.

MyChart works the same way, except the upstream CLI wants a config file instead of a config dir. `switchboard` still treats the namespace state as isolated local authority, it just materializes `MYCHART_CONFIG` as a file path inside the namespace state directory and pins `MYCHART_ACCOUNT` so Epic does not wander off into the wrong patient account. For the normal case, `state_dir` is enough. You only need an explicit auth block if you want `switchboard` to inject extra `MYCHART_*` overrides from env, files, or 1Password. If you do add one, `switchboard` will pick up the default `mychart_<namespace>` auth ref automatically, so `namespace.mychart.ucla` naturally pairs with `auth.mychart_ucla`.

```toml
[namespace.mychart.ucla]
provider = "mychart"
account = "UCLA Health"
default_read = false
state_dir = "/Users/jessfraz/.config/mychart-ucla"
```

Schwab follows the same CLI-managed pattern as MyChart. `switchboard` scopes the Schwab CLI state file under the namespace `state_dir` by materializing `SCHWAB_CONFIG`, and you can optionally attach an explicit `schwab_cli` auth block when you want client id or secret overrides injected from env, files, or 1Password. If you omit `auth`, `switchboard` will automatically look for `auth.schwab_<namespace>`.

```toml
[secret.schwab_personal_client_id]
kind = "onepassword_item"
account = "my.1password.com"
item = "schwab cli"
field = "username"

[secret.schwab_personal_client_secret]
kind = "onepassword_item"
account = "my.1password.com"
item = "schwab cli"
field = "credential"

[auth.schwab_personal]
provider = "schwab"
kind = "schwab_cli"
account = "jessfraz"
client_id = "schwab_personal_client_id"
client_secret = "schwab_personal_client_secret"

[namespace.schwab.personal]
provider = "schwab"
account = "jessfraz"
auth = "schwab_personal"
default_read = true
state_dir = "/Users/jessfraz/.config/schwab-personal"
```

## How It Works

Every action follows the same rough shape:

1. Resolve the namespace.
2. Load auth and any namespace-scoped runtime state.
3. Validate policy.
4. Plan the action.
5. Persist an operation record for writes.
6. Require approval if policy says so.
7. Execute.
8. Audit the result.

The important bit is that writes are not just "run the command with more confidence."

They are first-class planned operations.

## Real Workflows

### GitHub Read

```bash
switchboard github.pull_request.read \
  --ns github.personal \
  --repo openai/codex \
  --number 1382 \
  --json
```

### Google Calendar Draft, Approve, Apply

Draft first:

```bash
switchboard google.calendar.create \
  --ns google.work \
  --title 'Budget review' \
  --start '2026-03-30T15:00:00-07:00' \
  --end '2026-03-30T15:30:00-07:00' \
  --draft \
  --json
```

Then approve and apply:

```bash
switchboard op approve op_1234abcd --actor codex --note 'user approved'
switchboard op apply op_1234abcd --json
```

### Aggregate Read Across Namespaces

```bash
switchboard google.calendar.list \
  --ns google.work \
  --ns google.personal \
  --json
```

Reads can fan out across namespaces. Writes stay single-namespace unless the caller explicitly asks otherwise.

## Raw CLI Coverage

The curated tools are the nice typed layer, not the limit.

`switchboard` also generates raw provider tool surfaces from committed CLI inventories, so provider breadth does not depend on hand-writing every wrapper first.

Examples:

```bash
switchboard google.cli.read --ns google.work --json -- calendar +agenda --format json --today
switchboard github.cli.write --ns github.personal --draft -- --repo owner/repo issue comment 123 --body 'needs tests'
```

Everything before `--` belongs to `switchboard`.

Everything after `--` is forwarded to the provider CLI unchanged.

Namespace auth, policy, approval, and audit still apply.

## LLM Usage Rules

If you are driving `switchboard` from an LLM:

- prefer `--json`
- pass `--ns` explicitly for writes
- draft or plan writes first
- repeat `--ns` only for aggregate reads
- treat raw tools as escape hatches, not defaults
- if namespace resolution is ambiguous, ask instead of guessing

This repo is being shaped for both humans and LLMs on purpose. The docs should work as rendered pages and as plain Markdown chunks.

## Prompt-Shaped Workflows

These are the kinds of user requests `switchboard` is meant to support well:

- "You see the latest email from the car wash people, make me a calendar event with the details."
  The model should search mail, read the winning message, extract the event fields, and draft the calendar event before applying it.
- "Look up all the past emails I sent to book the dogs at the dog hotel, make a new appointment for these dates by sending them a similar email."
  The model should search sent mail, read the relevant history, extract the reusable pattern, and draft the outbound message before sending it.
- "Review my open PRs and draft a comment on the riskiest one."
  The model should search or list PRs, read the candidate PRs, inspect the risky one, and draft the comment instead of posting it directly.

The model does the reasoning. `switchboard` provides typed, namespace-aware primitives with audit, policy, and approval around them.

The longer versions of these prompt workflows live in [docs/llms/patterns.md](docs/llms/patterns.md).

## Command Discovery

You should be able to discover the surface without reading Rust:

```bash
switchboard tools list
switchboard tools describe github.notifications.list
switchboard tools describe google.cli.read
switchboard audit list
switchboard op list --pending
```

## Config Resolution

`switchboard` looks for config in this order:

1. `--config <path>`
2. `SWITCHBOARD_CONFIG`
3. `./switchboard.toml`
4. `$XDG_CONFIG_HOME/switchboard/config.toml`
5. `$HOME/.config/switchboard/config.toml`

## Non-Goals

- building another chat shell
- replacing Codex, Claude, or ChatGPT
- forcing everything through MCP
- pretending every provider has a clean API
- shipping a fake "universal everything bus" before the core is trustworthy

## Docs

- LLM operator guide: [docs/llms/getting-started.md](docs/llms/getting-started.md)
- LLM workflow patterns: [docs/llms/patterns.md](docs/llms/patterns.md)
- Generated reference catalog: [docs/reference/README.md](docs/reference/README.md)
- Structured tool catalog JSON: [docs/reference/catalog.json](docs/reference/catalog.json)
- Prebuilt binaries and package-manager installs still need the first public release tag

The repo should eventually be the best place to understand:

- the problem
- the solution
- how the trust boundary works
- how to install it
- how to call it from both shells and LLMs
