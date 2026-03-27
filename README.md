# Switchboard

Rust-first, model-agnostic local automation plane.

Stable CLI contract, explicit namespaces, safe writes, audit trail.

## What this is

`switchboard` is a local CLI, with an optional local daemon, that gives humans, scripts, and models a boring way to do useful work across:

- GitHub
- Google Workspace

The point is not to build another chat UI.
The point is not to build another agent shell.

The point is to build the trust layer those tools are usually missing:

- account isolation
- explicit authority
- draft-first writes
- approvals
- auditability
- backend swapability without changing the command contract

Codex is the preferred operator today.
Claude should work too.
Humans and shell scripts should work just fine.

## What we are building first

V1 is intentionally narrow.

Start with:

- GitHub cloud-state workflows
- Google Workspace workflows
- local namespaces
- draft and approval flows
- audit logging

Do not start with:

- Slack
- Ramp
- iMessage
- WhatsApp
- browser bridges
- "universal everything bus" nonsense

Those can come later if the core is good.
Right now the job is to ship something real, sharp, and trustworthy.

## Core thesis

The durable interface should be a CLI, not a chat protocol.

Why:

- CLIs compose with humans, scripts, cron, and models
- CLIs are easy to inspect, test, replay, and debug
- CLIs do not care which model is calling them
- CLIs survive ecosystem churn better than model-specific glue
- MCP can exist later as a shim, it should not be the center of the design

```text
           Models / Humans / Scripts / Cron
                        |
                        | shell out
                        v
              +----------------------+
              |     switchboard      |
              |   stable CLI contract|
              +----------------------+
                        |
                        v
              +----------------------+
              |   local trust layer  |
              | ns + policy + audit  |
              +----------------------+
                        |
             +----------+----------+
             |                     |
             v                     v
      GitHub backend         Google backend
```

If a model vendor changes strategy next month, fine.
The CLI survives.

## The real problem

Most agent setups are fine at reasoning and bad at authority boundaries.

The actual hard parts are:

- separating work and personal identities
- handling multiple accounts cleanly
- keeping credentials out of prompt soup
- making writes explicit
- generating a real audit trail
- swapping ugly temporary integrations out later without breaking the surface

The product is not "AI does tasks."
The product is "automation with sane trust boundaries."

## Non-goals

- building a new agent shell
- replacing Codex, Claude, or ChatGPT
- forcing everything through MCP
- shipping browser automation in v1
- pretending every provider has a clean API
- building cloud sync in v1

## Mental model

- `switchboard` CLI = product surface
- local core = trust boundary
- namespaces = account isolation
- policies = authority rules
- approvals = human confirmation for risky writes
- adapters = provider-specific behavior

```text
           one command
               |
               v
    +------------------------+
    | switchboard CLI        |
    +------------------------+
               |
               v
    +------------------------+
    | switchboard core       |
    |------------------------|
    | namespace resolver     |
    | policy engine          |
    | planner                |
    | audit recorder         |
    +------------------------+
          |             |
          |             +----------------------+
          |                                    |
          v                                    v
 +-------------------+                +------------------+
 | github adapter    |                | google adapter   |
 +-------------------+                +------------------+
     |        |                           |         |
     |        +--> GitHub API             |         +--> Google APIs
     |
     +--> gh
```

A long-running daemon is useful for approvals, auth refresh, caching, and background jobs.
It is not the product.
The product is still the CLI.

## Core invariants

1. Every command resolves to exactly one namespace.
1. If namespace resolution is ambiguous, fail closed.
1. Reads may use defaults.
1. Writes should require explicit namespace unless a very strict safe default exists.
1. Writes are plan or draft first.
1. Outbound or destructive writes require approval by default.
1. Every action gets audited.
1. The user-facing command contract stays stable even if the backend changes.

## Namespaces

Every provider identity is referenced through an explicit namespace.

Examples:

- `github.personal`
- `google.work`
- `google.personal`

```text
          same machine, different authority domains

   +------------------+   +------------------+   +--------------------+
   | github.personal  |   | google.work      |   | google.personal    |
   | personal account |   | work mailbox     |   | personal mailbox   |
   +------------------+   +------------------+   +--------------------+

   A command must land in exactly one box.
```

Example config:

```toml
[namespace.github.personal]
provider = "github"
account = "jessfraz"
auth = "gh"
default_read = true

[namespace.google.work]
provider = "google"
account = "jess@company.com"
auth = "oauth"
default_read = true

[namespace.google.personal]
provider = "google"
account = "jess@example.com"
auth = "oauth"
default_read = false
```

The model should never infer hidden authority.
If it wants to write, it should say where.

GitHub may start single-account while Google starts multi-account.
That is fine, keep the namespace contract consistent anyway.

## Contract stability rule

The command contract is the product.
Backends are replaceable.

Example:

```bash
switchboard google.mail.search --ns google.work --query 'from:billing newer_than:7d'
```

That command should survive backend changes like:

- Google API today
- Google Workspace CLI tomorrow
- some better official tool later

Same story for GitHub.

```text
 user command stays the same
             |
             v
 switchboard google.mail.search --ns google.work ...
             |
             +--> backend A today
             +--> backend B later
             +--> backend C after that
```

## Tool naming

Tool names should be boring and predictable.

Pattern:

- `provider.resource.action`

Examples:

- `github.notifications.list`
- `github.pull_request.read`
- `github.issue.read`
- `github.issue.comment`
- `github.pull_request.comment`
- `google.mail.search`
- `google.mail.read`
- `google.mail.draft`
- `google.mail.send`
- `google.calendar.list`
- `google.calendar.create`
- `google.drive.search`

All write tools should support `--plan`, `--draft`, or both.

## Operation lifecycle

Every action follows the same shape:

1. resolve namespace
1. load auth context
1. validate policy
1. plan the action
1. preview or draft if needed
1. require approval if needed
1. execute
1. audit
1. return structured output

```text
READ PATH
---------
request -> resolve ns -> policy -> execute -> audit -> result

WRITE PATH
----------
request -> resolve ns -> plan -> preview -> approval -> execute -> audit -> result
```

The important bit is that planning and approval are first-class.
The model does not get silent write authority just because it sounds confident.

## Initial provider surface

### GitHub

Start with cloud-state workflows, not local git plumbing.

Initial read flows:

- notifications list and inspect
- pull request read
- pull request files and checks
- issue list and read
- repository and PR search

Initial write flows:

- draft PR comment
- submit PR comment after approval
- draft issue comment
- submit issue comment after approval

Preferred backend order:

1. `gh` for obvious flows
1. GitHub API when `gh` is awkward or incomplete

Example commands:

```bash
switchboard github.notifications.list --ns github.work --json

switchboard github.pull_request.read \
  --ns github.personal \
  --repo owner/repo \
  --number 123 \
  --json

switchboard github.pull_request.comment \
  --ns github.personal \
  --repo owner/repo \
  --number 123 \
  --body 'I think this needs a regression test.' \
  --draft \
  --json
```

### Google Workspace

Start with the boring, high-value stuff:

- Gmail
- Calendar
- Drive

Docs can come after the core flows are solid.

Initial read flows:

- mail search
- mail read
- calendar list
- drive file search

Initial write flows:

- draft email
- send email after approval
- draft calendar event
- create calendar event after approval

Preferred backend order:

1. direct Google APIs or official workspace tooling
1. a provider-specific CLI if it becomes the better option later

Example commands:

```bash
switchboard google.mail.search \
  --ns google.work \
  --query 'from:finance newer_than:7d has:attachment' \
  --json

switchboard google.mail.read \
  --ns google.work \
  --message-id 18c7f6... \
  --json

switchboard google.calendar.create \
  --ns google.work \
  --title 'Budget review' \
  --start '2026-03-30T15:00:00-07:00' \
  --end '2026-03-30T15:30:00-07:00' \
  --draft \
  --json
```

## Security model

Principles:

- least privilege
- separate credentials per namespace
- no raw tokens in prompts
- no silent writes
- local secret storage
- approval by default for risky writes
- log redaction

Approval defaults:

- reads are usually allowed and still logged
- writes are previewed first
- outbound or destructive actions require approval by default

## Audit model

Every tool call should emit an audit record.

Minimum useful fields:

- timestamp
- actor
- namespace
- tool
- arguments hash
- plan summary
- approval state
- backend used
- result status

Example:

```json
{
  "timestamp": "2026-03-26T19:15:00-07:00",
  "actor": "codex",
  "namespace": "github.personal",
  "tool": "github.pull_request.comment",
  "arguments_hash": "sha256:...",
  "plan_summary": "Draft comment on owner/repo#123",
  "approval_state": "approved",
  "backend": "gh",
  "result": "ok"
}
```

If the system does something important, there should be a receipt.

## Output contract

Commands should return structured JSON by default or with `--json`.

Human-readable text can exist as a formatting layer, but JSON is the integration surface.

Example:

```json
{
  "tool": "google.mail.search",
  "namespace": "google.work",
  "items": [
    {
      "id": "18c7f6...",
      "from": "finance@company.com",
      "subject": "Q2 budget review",
      "received_at": "2026-03-26T08:12:11-07:00"
    }
  ]
}
```

## Suggested workspace

Do not over-split the codebase on day one.
Start with a small number of crates and split when the seams become real.

```text
switchboard/
├─ Cargo.toml
├─ crates/
│  ├─ switchboard-cli/
│  ├─ switchboard-core/
│  ├─ switchboard-store/
│  ├─ switchboard-github/
│  ├─ switchboard-google/
│  └─ switchboard-daemon/
└─ docs/
```

Possible responsibilities:

- `switchboard-cli`: argument parsing, human-facing entrypoint, JSON formatting
- `switchboard-core`: namespace resolution, policy, planning, approval hooks, shared types
- `switchboard-store`: SQLite persistence, audit, config
- `switchboard-github`: GitHub adapter
- `switchboard-google`: Google Workspace adapter
- `switchboard-daemon`: optional local RPC, approval queue, background jobs

Keep the architecture honest.
If a crate exists, it should solve a real boundary, not just look tidy in a tree view.

## V1 success criteria

V1 is successful if it can do these reliably:

1. list what needs attention on GitHub for a work account
1. list what needs attention on GitHub for a personal account
1. search and read work email safely
1. draft a GitHub comment and require approval before posting
1. draft an email or calendar event and require approval before sending or creating
1. keep personal and work namespaces from bleeding into each other
1. produce an audit trail for everything that matters

If it cannot do those things cleanly, adding more providers is just expanding the blast radius.

## Rollout plan

### Phase 0, core contract

- CLI skeleton
- namespace config
- auth loading
- SQLite store
- audit log
- shared JSON request and response types
- policy and approval hooks

### Phase 1, GitHub reads

- notifications
- pull request read flows
- issue read flows
- repository and PR search

### Phase 2, Google Workspace reads

- Gmail search and read
- Calendar list
- Drive search

### Phase 3, safe writes

- GitHub comment draft and apply
- Gmail draft and send
- Calendar draft and create
- approval UX that is not annoying

### Phase 4, optional daemon extras

- background jobs
- auth refresh
- caching
- local approval UI if needed
- optional MCP shim if it earns its keep

### Phase 5, new providers

Only after v1 is good:

- Slack
- Ramp
- iMessage
- WhatsApp
- browser-backed internal tools

## Later, if this works

The architecture should make these possible later:

- Slack
- Ramp
- local OS automations
- ugly internal vendor tools
- browser bridges for systems with no sane API

But those are follow-ons, not the current mission.

```text
now:
  GitHub + Google Workspace + trust boundaries

later:
  everything else, if deserved
```

## Summary

Build a Rust-first local automation layer for GitHub and Google Workspace.
Make the CLI the stable contract.
Keep namespaces explicit.
Make writes draft-first and approval-heavy.
Audit everything important.
Ship the smallest version that is actually useful.
