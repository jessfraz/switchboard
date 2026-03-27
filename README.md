# Switchboard

Rust-first, model-agnostic local automation plane.

Stable CLI contract, explicit namespaces, safe writes, audit trail.

## What this is

`switchboard` is a local CLI that gives humans, scripts, and models a boring way to do useful work across:

- GitHub
- Google Workspace

It starts there.
It is explicitly being designed to grow into Slack, Ramp, iMessage, and other local or work tools later without changing the core contract.

The point is not to build another chat UI.
The point is not to build another agent shell.

The point is to build the trust layer those tools are usually missing:

- account isolation
- explicit authority
- draft-first writes
- approvals
- auditability
- undoable operations where that can be done honestly
- backend swapability without changing the command contract

Codex is the preferred operator today.
Claude should work too.
Humans and shell scripts should work just fine.

## What we are building first

V1 is intentionally narrow.

Start with:

- GitHub cloud-state workflows
- Google Workspace workflows
- full raw CLI coverage for both
- curated high-value tools layered on top
- local namespaces
- draft and approval flows
- audit logging
- honest undo for actions that can be compensated

Do not start with:

- Slack
- Ramp
- iMessage
- WhatsApp
- browser bridges
- "universal everything bus" nonsense

Those can come later if the core is good.
Right now the job is to ship something real, sharp, and trustworthy.

Future providers we are designing for:

- Slack
- Ramp
- iMessage
- other ugly but useful local or work integrations

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

If a long-running local service becomes useful later for approvals, auth refresh, caching, or background jobs, it should exist to support the CLI, not replace it.

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
[secret.github_personal_token]
kind = "onepassword_item"
account = "my.1password.com"
item = "GitHub Personal Access Token"
field = "token"

[secret.google_workspace_cli_client_id]
kind = "env"
name = "GOOGLE_WORKSPACE_CLI_CLIENT_ID"

[secret.google_workspace_cli_client_secret]
kind = "env"
name = "GOOGLE_WORKSPACE_CLI_CLIENT_SECRET"

[secret.google_personal_oauth]
kind = "onepassword_item"
account = "my.1password.com"
item = "gws cli"
field = "json"

[auth.github_personal]
provider = "github"
kind = "gh_cli"
account = "jessfraz"

[auth.github_personal_token]
provider = "github"
kind = "github_token"
account = "jessfraz"
token = "github_personal_token"

[auth.google_work]
provider = "google"
kind = "google_oauth"
account = "jess@company.com"
client_id = "google_workspace_cli_client_id"
client_secret = "google_workspace_cli_client_secret"

[auth.google_personal]
provider = "google"
kind = "google_oauth_file"
account = "jess@example.com"
credentials = "google_personal_oauth"

[namespace.github.personal]
provider = "github"
account = "jessfraz"
auth = "github_personal"
default_read = true

[namespace.google.work]
provider = "google"
account = "jess@company.com"
auth = "google_work"
default_read = true
state_dir = "/Users/jessfraz/.config/gws-work"

[namespace.google.personal]
provider = "google"
account = "jess@example.com"
auth = "google_personal"
default_read = false
state_dir = "/Users/jessfraz/.config/gws-personal"
```

```text
google.work
  |
  +--> auth.google_work
  |      |
  |      +--> secret.google_workspace_cli_client_id
  |      |      -> env GOOGLE_WORKSPACE_CLI_CLIENT_ID
  |      |
  |      +--> secret.google_workspace_cli_client_secret
  |             -> env GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
  |
  +--> state_dir /Users/jessfraz/.config/gws-work
         -> provider maps this to its CLI cache/config dir
```

`switchboard` looks for config in this order:

1. `--config <path>`
1. `SWITCHBOARD_CONFIG`
1. `./switchboard.toml`
1. `$XDG_CONFIG_HOME/switchboard/config.toml`
1. `$HOME/.config/switchboard/config.toml`

Namespace `auth` values are references to concrete credential entries.
Namespace `state_dir` is where a CLI-backed adapter can keep per-account state without one login stomping another.
For the Google Workspace CLI specifically, that becomes `GOOGLE_WORKSPACE_CLI_CONFIG_DIR`.

That means `google.work` and `google.personal` can both use Google auth without pretending they are the same identity or sharing the same CLI cache.

`google_oauth` accepts `client_id`, `client_secret`, and optionally `refresh_token`.
`google_oauth_file` is for cases where a backend wants a full credential blob, for example a 1Password `json` field that later gets materialized into a temp file for a CLI.

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

- Google Workspace CLI today
- direct Google API later
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
- `github.cli.pr.view`
- `google.cli.calendar.+agenda`
- `github.cli.read`
- `github.cli.write`
- `google.cli.read`
- `google.cli.write`

All write tools should support `--plan`, `--draft`, or both.

Curated tools are the nice normalized layer.
Generated raw `provider.cli.<subcommand...>` tools are the broad coverage layer, every operation leaf from the committed CLI inventory becomes a real switchboard tool.
Generic `*.cli.read` and `*.cli.write` tools still exist as escape hatches when you want to shove arbitrary argv through without pretending it deserves a lovingly typed wrapper first.

The command catalog should come from two boring inputs:

- a generated committed inventory for the whole CLI
- a small handwritten manifest for the curated layer and any special cases

The handwritten manifest should only describe the stuff that actually needs judgment:

- tool name
- read vs write
- raw vs curated
- executable today vs planning-only
- whether undo is supported honestly through a compensating action
- whether a command can use built-in strategies like `raw_passthrough`, `summary_template`, declarative argv mapping, JSON projection, output summary templates, or effect extraction

Rust still owns the smart parts, auth materialization, any truly weird codecs, and compensation logic that cannot honestly be expressed as data.
The generated inventory is the broad map, the handwritten manifest is the semantic overlay, and neither is a replacement for typed code where it actually matters.
Adding a new provider should mostly be:

1. implement the runtime/materializer trait
1. generate and commit the full CLI inventory
1. write the small curated manifest
1. optionally add a little custom Rust only if the CLI output is especially cursed

You should be able to discover that surface without reading the source:

```text
switchboard tools list
switchboard tools describe google.cli.calendar.+agenda
switchboard tools describe google.cli.write
```

Raw passthrough should feel boring, not cursed.
Put `switchboard` flags before `--`.
Everything after `--` is forwarded to the provider CLI unchanged.

```text
switchboard google.cli.calendar.+agenda --ns google.work      --json -- --format json --today
switchboard github.cli.pr.view        --ns github.personal --json -- 1382 --repo openai/codex --json title,state
switchboard google.cli.read  --ns google.work      --json -- calendar +agenda --format json --today
switchboard github.cli.write --ns github.personal --draft -- pr comment 123 --body "needs tests"
```

For scripts, `--argv-json` should also work when building argv programmatically.

## Operation lifecycle

Every action follows the same shape:

1. resolve namespace
1. load auth and runtime context
1. validate policy
1. plan the action
1. persist an operation record for writes
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

Writes always create an operation record first.
What happens after that depends on policy, not vibes.
Approvals should be ergonomic enough that a human or agent can see pending work and move one stored operation from approval to execution without command-line calisthenics.

Write policy should be configurable:

- `allow` means `--apply` executes immediately
- `require_approval` means `--apply` stays planned until the stored operation is approved
- `deny` means writes fail closed

Example:

```toml
[policy]
write = "require_approval"
```

If you trust local agents and want them to write immediately on your own machine, `write = "allow"` should be a supported choice, not a moral failing.

## How a real request works

The user talks to the model.
The model talks to `switchboard`.
`switchboard` does not parse freeform natural language and pretend to be a mind reader.

Example user request:

> make me this calendar event on my personal account

What should happen:

1. the model decides this is a Google Calendar write
1. the model maps "personal account" to `google.personal`
1. the model extracts the event fields it already knows
1. the model calls `switchboard` in draft mode first
1. `switchboard` plans the write, checks policy, stores an operation record, and returns a preview plus operation id
1. the model shows the preview to the user
1. after approval, the model approves the stored operation
1. the model applies the stored operation by id
1. `switchboard` executes the provider adapter and records the result

```text
User
  |
  | "make me this calendar event on my personal account"
  v
Model
  |
  | decides:
  | - tool = google.calendar.create
  | - namespace = google.personal
  | - mode = --draft first
  v
switchboard
  |
  +--> resolve namespace: google.personal
  +--> check provider: google
  +--> build draft plan
  +--> store operation record
  +--> require approval because policy says so
  +--> audit planned action
  v
draft preview returned to model
  |
  v
User approves
  |
  v
Model approves and applies stored operation
  |
  +--> switchboard op approve <operation-id>
  +--> switchboard op apply <operation-id>
  +--> execute stored plan through google adapter
  +--> create event
  +--> audit result
  v
done
```

Example draft call:

```bash
switchboard google.calendar.create \
  --ns google.personal \
  --title 'Dinner with Sam' \
  --start '2026-03-30T19:00:00-07:00' \
  --end '2026-03-30T21:00:00-07:00' \
  --draft \
  --json
```

Example draft result:

```json
{
  "status": "planned",
  "tool": "google.calendar.create",
  "namespace": "google.personal",
  "summary": "Draft calendar event \"Dinner with Sam\" starting at 2026-03-30T19:00:00-07:00 for google.personal",
  "backend": "cli",
  "operation_id": "op_7f0b6e6c0cf54d7f8d1baf1d0d7a4abc",
  "approval_required": true,
  "approval_reason": "google.calendar.create requires approval by configured write policy"
}
```

Then, after the user approves, the model approves and applies the stored operation:

```bash
switchboard op approve op_7f0b6e6c0cf54d7f8d1baf1d0d7a4abc \
  --actor codex \
  --note 'user approved dinner plan'

switchboard op apply op_7f0b6e6c0cf54d7f8d1baf1d0d7a4abc \
  --json

switchboard op list --pending

switchboard op approve op_7f0b6e6c0cf54d7f8d1baf1d0d7a4abc \
  --actor codex \
  --apply \
  --json
```

If policy is set to `allow`, the first `google.calendar.create --apply` call can execute immediately and still emit the same kind of operation receipt.

Fail-closed rule:

- if the model cannot confidently map the request to exactly one namespace, it should ask
- if the event fields are incomplete, it should ask
- if the request is a write, it should draft first instead of firing blindly

Aggregate read rule:

- one provider read executes against one namespace
- one `switchboard` read request may declare multiple namespaces and fan out into isolated per-namespace reads

Example user request:

> tell me everything on my agenda today

That should mean:

1. the model recognizes this as an aggregate calendar read
1. the model resolves all allowed calendar namespaces, for example `google.work` and `google.personal`
1. the model calls `switchboard` once with `google.calendar.list` and repeats `--ns` for each calendar namespace
1. `switchboard` returns structured results for each namespace
1. the model merges, sorts, and summarizes the events for the user

```text
User
  |
  | "tell me everything on my agenda today"
  v
Model
  |
  | decides:
  | - intent = aggregate calendar read
  | - namespaces = google.work + google.personal
  v
switchboard
  |
  | google.calendar.list \
  |   --ns google.work \
  |   --ns google.personal
  v
fan out inside switchboard
  |
  +--> google.calendar.list (google.work)
  |
  +--> google.calendar.list (google.personal)
  v
structured per-namespace results

Model
  |
  +--> merge results
  +--> sort by start time
  +--> summarize for user
  v
agenda for today
```

Example aggregate read call:

```bash
switchboard google.calendar.list \
  --ns google.work \
  --ns google.personal \
  --json
```

Important:

- aggregate fan-out is fine for reads
- writes should still resolve to one namespace unless the user explicitly asks for multiple writes

## Compound workflow examples

These are the kinds of requests the model should be able to handle by composing multiple `switchboard` calls.

The rule stays the same:

- the model does the reasoning
- `switchboard` provides typed, namespace-aware primitives
- every read and write stays auditable
- writes still end in draft, then approval, then apply

### Latest email to calendar event

Example user request:

> you see the latest email from the car wash people, make me a calendar event with the details

What should happen:

1. the model decides this is a `search -> read -> draft calendar event` workflow
1. the model searches the allowed Gmail namespace or namespaces for the latest relevant message
1. the model reads the winning message in the namespace it came from
1. the model extracts structured event fields from the email
1. the model chooses the target calendar namespace, or asks if that is ambiguous
1. the model calls `google.calendar.create` in draft mode
1. the model shows the event preview before applying it

```text
User
  |
  | "you see the latest email from the car wash people,
  |  make me a calendar event with the details"
  v
Model
  |
  | decides:
  | - search mail
  | - read latest matching message
  | - extract event fields
  | - draft calendar event
  v
switchboard
  |
  +--> google.mail.search --ns google.personal ...
  |
  +--> google.mail.read --ns google.personal --message-id <id>
  |
  +--> google.calendar.create --ns google.personal --draft ...
  v
draft preview returned to model
  |
  v
User approves
  |
  v
Model calls apply
  |
  +--> google.calendar.create --ns google.personal --apply ...
  v
done
```

Example tool chain:

```bash
switchboard google.mail.search \
  --ns google.personal \
  --query 'from:carwash newer_than:30d' \
  --json

switchboard google.mail.read \
  --ns google.personal \
  --message-id 18c7f6... \
  --json

switchboard google.calendar.create \
  --ns google.personal \
  --title 'Car wash appointment' \
  --start '2026-03-31T10:00:00-07:00' \
  --end '2026-03-31T11:00:00-07:00' \
  --draft \
  --json
```

### Prior booking emails to similar draft

Example user request:

> look up all the past emails i sent to book the dogs at the dog hotel, make a new appointment for these dates by sending them a similar email

What should happen:

1. the model decides this is a `search sent mail -> read history -> draft similar email` workflow
1. the model searches sent mail for prior booking conversations
1. the model reads the best matching messages or threads
1. the model extracts recurring structure, recipient, subject pattern, and the booking details they usually need
1. the model fills in the new dates
1. the model drafts a new email in the same namespace
1. the model shows the draft and provenance before sending it

```text
User
  |
  | "look up all the past emails i sent to book the dogs at
  |  the dog hotel, make a new appointment for these dates"
  v
Model
  |
  | decides:
  | - search sent mail
  | - read prior booking emails
  | - extract reusable pattern
  | - draft similar outbound email
  v
switchboard
  |
  +--> google.mail.search --ns google.personal --query 'in:sent dog hotel'
  |
  +--> google.mail.read --ns google.personal --message-id <id>
  |
  +--> google.mail.draft --ns google.personal --to ... --subject ... --body ...
  v
draft preview returned to model
  |
  v
User approves
  |
  v
Model calls send
  |
  +--> google.mail.send --ns google.personal ...
  v
done
```

Important:

- fuzzy reads are fine for finding candidates
- fuzzy reads are not enough to auto-send mail without review
- if the target namespace is ambiguous, the model should ask
- if the extracted dates or recipient are uncertain, the model should ask
- search and read results should preserve stable IDs and source namespace so later calls can chain correctly

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

1. `gh` today
1. direct GitHub APIs later if they win on stability or coverage

Example commands:

```bash
switchboard github.notifications.list --ns github.personal --json

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

1. Google Workspace CLI today
1. direct Google APIs later if they win on stability or ergonomics

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

switchboard google.mail.draft \
  --ns google.work \
  --to finance@company.com \
  --subject 'Budget review follow-up' \
  --body-text 'Can we move this to Thursday?' \
  --draft \
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
- writes follow configured policy
- `require_approval` is the safe default
- `allow` is valid for users who explicitly trust local agents with writes
- `deny` is valid for locked-down environments

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

Useful commands:

```text
switchboard audit list
switchboard audit list --operation-id op_1234abcd
switchboard audit show audit_1234abcd
switchboard audit show op_1234abcd
```

## Undo model

Undo should be honest.

That means:

- the audit log stays append-only
- undo is a new compensating write, not a log rewrite
- only some actions are meaningfully undoable
- the system should say "not undoable" when that is the truth

For example:

- create a calendar event, undo by deleting that event later
- create a draft email, undo by deleting that draft later
- send an email, probably not truly undoable, so do not lie

```text
create event
   |
   +--> append audit record
   |
   +--> store operation effect
   |      - event ref
   |      - undoable = true
   |      - undo summary
   v
later, user wants undo
   |
   +--> plan compensating delete
   +--> require approval if needed
   +--> execute
   +--> append compensation audit record
```

The important distinction is:

- audit answers "what happened?"
- operation effects answer "can this be safely compensated later?"

An undoable write receipt should look more like this:

```json
{
  "status": "executed",
  "tool": "google.calendar.create",
  "namespace": "google.personal",
  "operation_id": "op_7f0b6e6c0cf54d7f8d1baf1d0d7a4abc",
  "effect": {
    "undoable": true,
    "undo_summary": "Delete calendar event \"Dinner with Sam\" from google.personal",
    "refs": [
      {
        "provider": "google_workspace",
        "namespace": "google.personal",
        "kind": "event",
        "id": "event-123",
        "parent_id": "primary"
      }
    ]
  }
}
```

And the command should stay boring:

```text
switchboard op undo op_7f0b6e6c0cf54d7f8d1baf1d0d7a4abc --apply --json
```

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

## V1 success criteria

V1 is successful if it can do these reliably:

1. list what needs attention on GitHub for your account
1. search and read work email safely
1. draft a GitHub comment and require approval before posting
1. draft an email or calendar event and require approval before sending or creating
1. keep personal and work namespaces from bleeding into each other
1. produce an audit trail for everything that matters

If it cannot do those things cleanly, adding more providers is just expanding the blast radius.

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

## MyChart Pages Callback

`mychart-cli` can use a tiny GitHub Pages callback site so Epic production has a real `https` redirect URI without
turning this repo into some sad little auth server.

The static callback page lives at:

- `/pages/index.html`
- `/pages/mychart-callback/index.html`

After GitHub Pages is enabled for this repo, a project site under `jessfraz/switchboard` will serve the callback at:

```text
https://jessfraz.github.io/switchboard/mychart-callback/
```

That page only does three things:

1. reads the OAuth callback URL
1. scrubs the code from the browser address bar
1. gives you a copyable `mychart auth exchange-url 'https://...'` command

Typical MyChart production flow:

```text
mychart connect add --name ucla \
  --base-url https://arrprox.mednet.ucla.edu/FHIRPRD/api/FHIR/R4 \
  --client-id <production-client-id> \
  --redirect-uri https://jessfraz.github.io/switchboard/mychart-callback/

mychart --account ucla auth authorize-url --scope openid --scope fhirUser --scope patient/*.read
```

Finish the browser login, let Epic redirect to the GitHub Pages callback, then run the command the page gives you:

```text
mychart --account ucla auth exchange-url 'https://jessfraz.github.io/switchboard/mychart-callback/?code=...&state=...'
```

## Summary

Build a Rust-first local automation layer for GitHub and Google Workspace.
Make the CLI the stable contract.
Keep namespaces explicit.
Make writes draft-first and approval-heavy.
Audit everything important.
Ship the smallest version that is actually useful.
