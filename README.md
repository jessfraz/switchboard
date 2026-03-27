# Switchboard

Rust-first, model-agnostic local automation plane.

## What this is

`switchboard` is a local-first Rust daemon and CLI that exposes trusted, auditable automation tools for:

- multiple Google accounts
- multiple Slack workspaces
- Ramp
- GitHub
- iMessage
- WhatsApp
- future internal/work tools
- future wrapper integrations for services that do not yet have proper tooling

The intent is **not** to build another chat UI or another agent shell.

The intent is to build a **boring, trustworthy backend** that any model can use through a CLI contract.

Codex is the preferred operator today.
Claude should work too.
Anything else that can execute a CLI and read structured output should work too.

## Core thesis

The stable interface should be a **CLI**, not a chat protocol.

Why:

- CLIs are durable
- CLIs compose well with shells, scripts, agents, cron, and humans
- CLIs are easy to inspect, test, replay, and debug
- CLIs do not care which model is calling them
- MCP can be added later as a thin compatibility shim if ever needed

So the architecture should optimize for:

- `switchboard` as the primary interface
- structured JSON output
- explicit namespaces
- safe writes
- auditability
- pluggable provider backends

Not for:

- model-specific SDK glue
- MCP-first design
- provider-specific prompt magic

## Why this exists

Most agent setups are fine at reasoning and bad at trust boundaries.

The real hard problems are:

- separating work and personal identities cleanly
- handling multiple accounts per provider
- using the same tools from Codex, Claude, or anything else
- keeping secrets out of prompt soup
- making writes safe and explicit
- producing a real audit trail
- supporting ugly services that have no real CLI or API yet
- replacing temporary hacks cleanly when native integrations arrive

This project solves that layer.

## Design goals

1. **Rust-native**

   - No JS daemon as the core control plane.
   - Lean on Rust for correctness, typing, concurrency, and fewer haunted dependency trees.

1. **Model-agnostic**

   - Any model that can call a CLI can use Switchboard.
   - Codex is preferred today.
   - Claude should work.
   - Future agents should work without architectural changes.

1. **CLI-first**

   - The CLI is the product surface.
   - The daemon backs the CLI.
   - JSON in, JSON out.
   - MCP is optional and non-core.

1. **Local-first**

   - Runs on the user’s machine or a user-owned box.
   - Secrets stay local by default.
   - No vendor relay required.

1. **Multi-account by design**

   - Multiple Google accounts.
   - Multiple Slack workspaces.
   - Multiple identities per provider.
   - No “sent from the wrong account” nonsense.

1. **Safe writes**

   - Every outbound or destructive action goes through explicit planning and approval.
   - Draft-first by default.
   - Dry-run first.

1. **Auditable**

   - Every tool call gets logged with actor, namespace, action, arguments hash, approval state, and result.

1. **Replaceable backends**

   - Start with whatever works.
   - Prefer native CLIs where available.
   - Fall back to direct APIs.
   - Fall back to browser or OS automation where necessary.
   - Replace temporary adapters later without breaking the user-facing CLI contract.

1. **Boring over clever**

   - Prefer explicit routing and typed tools over “AI figures it out.”
   - The model should not infer hidden authority.

## Non-goals

- building a new agent shell
- replacing Codex, Claude, or ChatGPT
- forcing everything through MCP
- pretending all services have clean APIs
- pretending iMessage or WhatsApp are normal stable developer platforms
- building a cloud sync product in v1

## Mental model

- **Model** = operator
- **Switchboard CLI** = stable control surface
- **Switchboard daemon** = local trust boundary and execution engine
- **Adapters** = provider-specific backends
- **Namespaces** = account isolation
- **Policies** = safety rules
- **Approvals** = human confirmation for writes

## High-level architecture

```text
                ┌────────────────────────────────────┐
                │   Codex / Claude / other models    │
                │    or humans / scripts / cron      │
                └────────────────┬───────────────────┘
                                 │
                                 │ shell out to CLI
                                 v
                ┌────────────────────────────────────┐
                │            switchboard             │
                │      human + agent-facing CLI      │
                │      JSON output, stable verbs     │
                └────────────────┬───────────────────┘
                                 │
                                 v
                ┌────────────────────────────────────┐
                │          switchboardd              │
                │------------------------------------│
                │ router                             │
                │ policy engine                      │
                │ approvals                          │
                │ scheduler                          │
                │ audit log                          │
                │ namespace resolver                 │
                │ adapter registry                   │
                └───────────────┬────────────────────┘
                                │
             ┌──────────────────┼──────────────────┬───────────────────┐
             │                  │                  │                   │
             v                  v                  v                   v
    ┌────────────────┐ ┌────────────────┐ ┌────────────────┐ ┌────────────────┐
    │ native CLI     │ │ direct API     │ │ browser bridge │ │ local OS node  │
    │ adapters       │ │ adapters       │ │ adapters       │ │ adapters       │
    └────────────────┘ └────────────────┘ └────────────────┘ └────────────────┘
```

## Integration strategy

Every service should be integrated through one of four backend types.

### 1. Native CLI adapter

Use this whenever possible.

Examples:

- `gh`
- future Google Workspace CLI
- future Slack CLI if one exists
- internal company CLIs

Best properties:

- easiest to debug
- easiest to test manually
- naturally model-agnostic
- often already solves auth

### 2. Direct API adapter

Use when there is a sane API and no good CLI.

Examples:

- Ramp
- GitHub cloud-state actions if `gh` is not enough
- Google APIs when CLI support is incomplete
- Slack API for advanced message/thread features

### 3. Browser bridge adapter

Use when the service is useful but has no CLI/API worth trusting.

Examples:

- WhatsApp personal
- random SaaS admin panels
- weird internal vendor dashboards
- consumer tools with no public automation surface

Important:

- treat this as temporary or second-class
- explicit approval for writes
- brittle by definition

### 4. Local OS node adapter

Use when integration depends on a local signed-in device.

Examples:

- iMessage
- local notifications
- local app control
- AppleScript / Shortcuts based automations

Important:

- separate failure model
- not portable
- best-effort only

## Contract stability rule

The user-facing command contract should stay stable even if the backend changes.

Example:

```bash
switchboard google.mail.search --ns google.work --query 'from:billing'
```

That command should survive backend changes like:

- direct Google API today
- Google Workspace CLI tomorrow
- official Google tool later

Same for WhatsApp, Slack, or anything else.

The adapter is replaceable.
The command contract is the product.

## Why not MCP-first

MCP can still exist as a tiny shim if needed later.

But it should not be the center of the design.

Reasons:

- CLI is simpler
- CLI is more durable
- CLI works with any model
- CLI works with scripts and humans too
- MCP lock-in is not worth architecting around
- if a model ecosystem changes, the CLI survives

So:

- build CLI first
- build daemon second
- add MCP only if it becomes tactically useful
- never make MCP the core abstraction

## Account model

Every provider identity is referenced through an explicit namespace.

Examples:

- `google.work`
- `google.personal`
- `google.ops`
- `slack.zoo`
- `slack.community`
- `slack.customer-success`
- `ramp.zoo`
- `github.jess`
- `imessage.personal`
- `whatsapp.personal`

Rules:

1. Every tool invocation must resolve to exactly one namespace.
1. If ambiguous, fail closed.
1. Reads may support default namespaces.
1. Writes should require explicit namespace unless a strict safe default exists.
1. Cross-account operations must be explicit.

## Tool naming

Tool names should be boring and predictable.

Pattern:

- `provider.resource.action`

Examples:

- `google.mail.search`
- `google.mail.read`
- `google.mail.draft`
- `google.mail.send`
- `google.calendar.list`
- `google.calendar.create`
- `slack.message.search`
- `slack.thread.read`
- `slack.message.draft`
- `slack.message.send`
- `ramp.transaction.search`
- `github.notifications.list`
- `github.pr.comment`
- `imessage.message.send`
- `whatsapp.message.send`

All write tools should support plan or draft mode.

## Operation lifecycle

Every action follows the same shape:

1. resolve namespace
1. load auth context
1. choose adapter backend
1. validate policy
1. generate plan
1. show preview or draft
1. require approval if needed
1. execute
1. audit
1. return structured result

The important bit is step 3:
the same logical tool may execute through a native CLI, direct API, browser bridge, or local OS node.

## Core components

### `switchboard`

Primary CLI interface.

Responsibilities:

- all human and model-facing commands
- JSON output
- bootstrap auth
- inspect namespaces
- inspect approvals
- inspect audit records
- direct execution against daemon

### `switchboardd`

Long-running local daemon.

Responsibilities:

- local RPC
- adapter registry
- namespace resolution
- policy evaluation
- approval queue
- scheduling
- audit persistence
- caching
- retries and backoff

### `switchboard-types`

Shared request/response types.

### `switchboard-policy`

Allow/deny/approval rules.

### `switchboard-store`

SQLite persistence.

### `switchboard-auth`

Credential loading and refresh.

### `switchboard-audit`

Audit event recording.

### Adapter crates

Examples:

- `switchboard-google`
- `switchboard-slack`
- `switchboard-ramp`
- `switchboard-github`
- `switchboard-imessage-node`
- `switchboard-whatsapp-node`

### Backend driver crates

Examples:

- `switchboard-driver-cli`
- `switchboard-driver-http`
- `switchboard-driver-browser`
- `switchboard-driver-localos`

These are reusable execution primitives, not provider-specific tools.

## Suggested Cargo workspace

```text
switchboard/
├─ Cargo.toml
├─ crates/
│  ├─ switchboard-types/
│  ├─ switchboard-core/
│  ├─ switchboard-cli/
│  ├─ switchboard-daemon/
│  ├─ switchboard-store/
│  ├─ switchboard-auth/
│  ├─ switchboard-policy/
│  ├─ switchboard-audit/
│  ├─ switchboard-scheduler/
│  ├─ switchboard-driver-cli/
│  ├─ switchboard-driver-http/
│  ├─ switchboard-driver-browser/
│  ├─ switchboard-driver-localos/
│  ├─ switchboard-google/
│  ├─ switchboard-slack/
│  ├─ switchboard-ramp/
│  ├─ switchboard-github/
│  ├─ switchboard-imessage-node/
│  └─ switchboard-whatsapp-node/
└─ docs/
```

## Internal interfaces

```rust
pub trait Adapter: Send + Sync {
    fn provider(&self) -> ProviderKind;
    async fn capabilities(&self, ns: &Namespace) -> Result<Vec<Capability>>;
    async fn plan(&self, ctx: &ToolContext, req: ToolRequest) -> Result<PlannedAction>;
    async fn execute(&self, ctx: &ExecutionContext, plan: PlannedAction) -> Result<ToolResult>;
}

pub trait Driver: Send + Sync {
    fn kind(&self) -> DriverKind;
    async fn invoke(&self, req: DriverRequest) -> Result<DriverResponse>;
}

pub trait PolicyEngine: Send + Sync {
    async fn evaluate(&self, ctx: &PolicyContext, action: &PlannedAction) -> Result<PolicyDecision>;
}

pub trait AuditSink: Send + Sync {
    async fn record(&self, event: AuditEvent) -> Result<()>;
}
```

Important split:

- **Adapters** know provider semantics
- **Drivers** know how to execute via CLI, HTTP, browser, or local OS

That lets you swap backend strategy without rewriting the provider surface.

## Provider strategy

### Google

Namespaces:

- `google.work`
- `google.personal`
- `google.ops`

Preferred backend order:

1. native CLI when it is good enough
1. direct API when needed
1. never bake auth assumptions into the core

Support:

- Gmail
- Calendar
- Drive
- Docs

### Slack

Namespaces:

- `slack.zoo`
- `slack.community`
- `slack.partner`

Preferred backend order:

1. native CLI if it ever exists
1. direct API
1. browser fallback only if absolutely necessary

Support:

- channel search
- message search
- thread read
- draft reply
- post reply

### Ramp

Namespaces:

- `ramp.zoo`

Preferred backend order:

1. direct API
1. browser fallback only for missing admin workflows

Start read-only.

### GitHub

Namespaces:

- `github.jess`

Preferred backend order:

1. `gh` CLI for obvious flows
1. direct API for anything `gh` does poorly

Focus on cloud state, not local git plumbing.

### iMessage

Namespaces:

- `imessage.personal`

Preferred backend order:

1. local OS node
1. AppleScript / Shortcuts / local bridge

This is a node integration, not a cloud API.

### WhatsApp

Namespaces:

- `whatsapp.personal`

Preferred backend order:

1. business API if applicable
1. browser bridge for personal use

This should be treated as unstable and approval-heavy.

## Services with no proper tools yet

This matters a lot.

Switchboard should explicitly support “temporary ugly integrations” without polluting the long-term architecture.

Examples:

- browser-only SaaS tools
- internal dashboards
- admin panels
- consumer messaging products
- vendor portals

These should be integrated through a clear adapter status model:

- `native`
- `api`
- `bridge`
- `experimental`

And every tool should be able to report its backend mode.

Example output:

```json
{
  "tool": "whatsapp.message.send",
  "namespace": "whatsapp.personal",
  "backend": "bridge",
  "status": "experimental",
  "approval_required": true
}
```

That way the system tells the truth instead of pretending all integrations are equally trustworthy.

## Security model

### Principles

- least privilege
- separate credentials per namespace
- no raw tokens in prompts
- no silent writes
- local secret storage
- explicit approvals for outbound or destructive actions
- log redaction

### Approval defaults

Reads:

- usually allowed
- still logged

Writes:

- draft or preview first
- approval required by default

Experimental / bridge integrations:

- stricter approval
- maybe human confirmation every time

## Execution model

- Tokio runtime
- one daemon
- bounded concurrency
- retries only when safe
- idempotency keys for writes
- structured JSON output for all commands
- machine-readable errors
- human-readable summaries as optional formatting layer

## CLI examples

```bash
switchboard ns list --json

switchboard google.mail.search \
  --ns google.work \
  --query 'from:billing newer_than:7d' \
  --json

switchboard slack.thread.read \
  --ns slack.zoo \
  --channel C123 \
  --thread-ts 1712345678.1234 \
  --json

switchboard github.notifications.list \
  --ns github.jess \
  --json

switchboard google.calendar.create \
  --ns google.personal \
  --title 'Dinner' \
  --start '2026-03-30T19:00:00-07:00' \
  --end '2026-03-30T21:00:00-07:00' \
  --draft \
  --json

switchboard whatsapp.message.send \
  --ns whatsapp.personal \
  --to '+15551234567' \
  --body 'Running 10 min late' \
  --draft \
  --json
```

## Model usage guidance

Codex may be the main operator today, but the tool should not know or care.

Expected use patterns:

- Codex shells out to `switchboard`
- Claude shells out to `switchboard`
- shell scripts shell out to `switchboard`
- cron shells out to `switchboard`
- humans shell out to `switchboard`

The JSON contract is the integration surface.

## Suggested AGENTS.md

```markdown
- Use switchboard via CLI, not through a model-specific protocol.
- Prefer explicit namespaces for all writes.
- Prefer draft/plan mode before apply.
- Never send outbound messages without approval.
- Treat personal and work namespaces as separate security domains.
- Prefer native CLI-backed adapters when available.
- Use direct API adapters where CLI support is missing.
- Use bridge adapters only when necessary, and treat them as less reliable.
- Never print secrets or raw tokens.
- Summarize side effects before apply.
```

## Phase plan

### Phase 0

- workspace skeleton
- CLI
- daemon
- namespace config
- policy engine
- approval queue
- audit log
- JSON request/response types

### Phase 1

- driver layer
  - CLI driver
  - HTTP driver
  - browser driver
  - local OS driver

### Phase 2

- GitHub
- Google
- Slack
- Ramp read-only

### Phase 3

- safe write flows
- approvals
- draft/send split
- adapter status reporting

### Phase 4

- iMessage node
- WhatsApp bridge
- scheduler
- recurring summaries / life-admin flows

### Phase 5

- optional MCP shim if useful
- tiny local approval UI if needed
- richer scheduling and replay tools

## Summary

Build a Rust daemon and CLI, not an MCP-centric platform.
Make the CLI the stable contract.
Keep it model-agnostic.
Prefer native CLIs when possible.
Support direct APIs when needed.
Support browser or OS bridges for missing tools.
Replace ugly adapters later without breaking commands.
Make namespaces explicit.
Make writes safe.
Make everything auditable.
