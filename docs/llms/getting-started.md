# LLM Getting Started

Run familiar commands directly. Prefer curated tools, explicit namespaces, and
`--json`. JSON is compact by default; `--full` pretty-prints the complete view.
Use `--fields` to select paths inside result fields while keeping references,
coverage, failures, and write receipts visible.

## Discover only what is missing

```sh
switchboard ns list --provider google --json
switchboard ns show google.work --json
switchboard tools list --provider google --search mail --json
switchboard google.mail.search --help --json
```

Namespace discovery reads configuration only: no operation store, secret
resolution, provider subprocess, or auth probe. `ns list --account` matches the
exact auth identity, not the display label; combine it with `--provider` and
`--default-read` as needed. Multiple matches remain multiple choices, not an
automatic route. `ns show` reports safe auth-source, policy, transport, and config
provenance metadata. `--full` never exposes secrets or their locators.

Tool listings default to eight executable matches, with configured namespaces and
runnable examples. Use `--limit` to change the limit or `--full` for the complete
catalog, including planning-only tools. Describe/help returns arguments and
examples; add `--full` for output schemas, scopes, and native fallback details.
Discovery does not authenticate. Use `doctor --ns NS --json` to investigate an
actual setup failure, not as a preamble to every task.

## Read task context in one invocation

```sh
switchboard google.mail.search --ns google.personal \
  --query 'from:appointments@example.invalid newer_than:30d' \
  --hydrate --max 5 --body-limit 4000 --json

switchboard google.mail.thread --ns google.personal --thread-id THREAD_ID \
  --max-messages 20 --body-limit 4000 --json

switchboard github.pull_request.context --ns github.personal \
  --repo owner/repo --number 123 --include discussion,files,checks --json

switchboard github.issue.context --ns github.personal \
  --repo owner/repo --number 123 --include discussion,linked-prs --json
```

These commands consolidate caller round trips; provider requests can still fan
out internally. Check `coverage` and failures before calling a result complete.
Increase explicit bounds or fetch a specific item if text or lists were cut.
`--full` changes presentation, not provider limits.

## Batch independent reads without scratch files

```sh
switchboard read-batch --tool google.mail.search --ns google.personal \
  --args-json '{"query":"from:appointments@example.invalid","hydrate":true,"max":5}' \
  --args-json '{"query":"from:receipts@example.invalid","hydrate":true,"max":5}' \
  --fields messages.subject,messages.body_text --json

switchboard read-batch --resume /absolute/path/from/checkpoint-field.json --json
```

The response includes a private checkpoint path and exact `resume_argv`.
Pagination follows cursors within page/time limits; successful pages survive a
failure. `--input -` accepts mixed-tool JSON from stdin. See
[bounded reads and result contracts](../reliability.md).

## Watch the requested commit

```sh
switchboard github.ci.status --ns github.personal --repo owner/repo \
  --commit 0123456789abcdef0123456789abcdef01234567 --wait 30 --json
```

Pass the returned `fields.ci.cursor` as `--cursor` to wait for a change. Unchanged
snapshots keep status/counts but omit repeated run/check arrays. The full 40-digit
SHA is required; absent, unknown, and truncated checks never become success.

## Writes and authentication

Run ordinary `switchboard` commands without credential or biometric environment
prefixes. Switchboard resolves cached credentials first, keeps namespaces
isolated, and automatically coordinates bounded authentication recovery across
commands. Resolve necessary identity checks serially before parallel reads.
`SWITCHBOARD_RUN_ID` is optional for automation that needs one recovery budget
for an entire task. Do not set `SWITCHBOARD_OP_BIN=/usr/bin/false` unless the user
explicitly requests cache-only access; it disables normal 1Password recovery.

Draft writes before execution. When the user already authorized the exact
mutation, `--approve-and-apply` persists the plan, approves that operation, and
applies it in one invocation. It does not grant permission to perform a write.
Email remains draft-only unless the user explicitly authorizes sending.

Bare Google `auth login` requests the full user-OAuth automation defaults,
avoiding incomplete service-scope discovery in gws 0.22.5. Both scope profiles
use the same defaults for Workspace domains. Known `gmail.com` and
`googlemail.com` accounts receive only the standard set.

| Accounts | Default capabilities |
| --- | --- |
| All | Drive and activity history, Sheets, Gmail and filter settings, Calendar, Docs, Slides, Tasks, Contacts and Other Contacts, Apps Script projects/deployments/processes/metrics, Cloud Platform |
| Workspace | Chat messages/spaces/memberships/emojis/read state/notification settings/sections/availability and administration; Meet spaces/settings/conference records; organization directory; Directory users/groups/OUs/domains/custom schemas/roles/resources/devices/printers; offboarding transfers; licenses; Groups settings/archive migration; Drive labels; Chrome policy/profiles/reports/telemetry/security insights; Cloud Identity groups/devices/policies/shared-drive OUs; Vault; Admin Reports |

The existing full Drive scope also covers the Forms API, including responses.
Gmail filter creation needs `gmail.settings.basic` in addition to
`gmail.modify`; both use the existing Gmail API.

Enable each service's API in the OAuth client's Google Cloud project before
using it. Chat writes also need Chat app configuration; Apps Script needs its
user API access setting enabled. Organization roles, service editions, and
licenses still apply, including Vault and Chrome Enterprise features. Scopes
grant neither administrator roles nor API enablement.

Keep and Alert Center require a separate delegated administrator/service-account
setup. Their scopes, Chat app-only/import scopes, and Meet Media preview scopes
are excluded from ordinary user login. See the official
[Chat authorization](https://developers.google.com/workspace/chat/authenticate-authorize),
[Meet authorization](https://developers.google.com/workspace/meet/api/guides/authenticate-authorize),
[Keep setup](https://developers.google.com/workspace/keep/api/guides), and
[Alert Center setup](https://developers.google.com/workspace/admin/alertcenter/guides)
guides.

Explicit login flags such as `--readonly`, `--services`, or `--scopes` keep their
native meaning. Existing sessions gain no additional permissions until the user
logs in again and grants consent. Login verifies the configured auth account;
never use a different namespace's login to bypass an access failure.

```sh
switchboard google.calendar.create --ns google.personal \
  --title 'Appointment' --start '2026-10-01T09:00:00-05:00' \
  --end '2026-10-01T10:00:00-05:00' --draft --json

switchboard op approve OPERATION_ID --actor codex
switchboard op apply OPERATION_ID --json
```

Retain operation IDs. Preserve partial JSON even on a nonzero exit. Inspect and
verify uncertain writes before retrying; a missing receipt does not prove the
remote write failed. Use `*.cli.read`/`*.cli.write` only when curated coverage is
insufficient, retaining the same namespace and approval boundaries.
