# LLM Patterns

Use the smallest bounded context that answers the task. Familiar commands need
no discovery call. The examples below use explicit namespaces and compact JSON;
coverage and failures determine whether further reads are needed.

## Review a pull request

```sh
switchboard github.pull_request.context --ns github.personal \
  --repo owner/repo --number 123 --include discussion,files,checks \
  --limit 20 --body-limit 4000 --json
```

The context includes the current head SHA, requested discussion, file metadata,
and checks. File metadata is not a source diff. Retrieve the relevant diff or
source before making code-level review claims. Use `--fields context.title,context.files`
for a narrow follow-up view. Issue context supports discussion and linked PRs:

```sh
switchboard github.issue.context --ns github.personal \
  --repo owner/repo --number 123 --include discussion,linked-prs --json
```

Issue linked PRs are closing references. PR linked PRs are recent cross
references; the returned relation label makes that distinction explicit.
Optional-section failures retain the core issue/PR and mark coverage unknown.
Draft a comment with `github.issue.comment --draft`; that tool remains
planning-only, so its plan does not imply an executable posting path.

## Extract details from email

```sh
switchboard google.mail.search --ns google.personal \
  --query 'from:appointments@example.invalid newer_than:30d' \
  --hydrate --max 5 --body-limit 4000 --json
```

Search returns decoded text and reply references in one invocation. For a known
conversation, use its thread ID:

```sh
switchboard google.mail.thread --ns google.personal --thread-id THREAD_ID \
  --max-messages 20 --max-attachments 20 --body-limit 4000 --json
```

Thread context includes bounded attachment metadata, not attachment contents.
Use `google.mail.read --ns google.personal --message-id ID --json` for an individual message's full
text and HTML. Hydrated search does not fetch label metadata, so `--hydrate`
and `--labels` cannot be combined. Preserve exact message/thread references
when drafting a reply or extracting an appointment.

## Search several queries

```sh
switchboard read-batch --tool google.mail.search --ns google.personal \
  --args-json '{"query":"in:sent to:bookings@example.invalid","hydrate":true,"max":3}' \
  --args-json '{"query":"from:bookings@example.invalid newer_than:30d","hydrate":true,"max":3}' \
  --fields messages.subject,messages.body_text --json
```

The batch creates its private checkpoint automatically. Resume with the returned
`resume_argv`; do not reconstruct successful requests. Mixed tools can use
`--input -` with `{"items":[{"id":"mail","tool":"google.mail.search",
"namespace":"google.personal","args":{"query":"newer_than:1d","max":20}}]}`.
The existing typed-argument array and explicit checkpoint paths remain supported.

## Monitor CI without repeated full dumps

```sh
switchboard github.ci.status --ns github.personal --repo owner/repo \
  --commit 0123456789abcdef0123456789abcdef01234567 --wait 30 --json

switchboard github.ci.status --ns github.personal --repo owner/repo \
  --commit 0123456789abcdef0123456789abcdef01234567 \
  --wait 30 --cursor CURSOR_FROM_PREVIOUS_RESULT --json
```

Each snapshot reconciles Actions runs, check runs, and commit statuses for that
exact commit. Without a cursor it waits for completion; with one it waits for a
change. Waiting is bounded to 60 seconds, including authentication. No runs,
partial reads, and truncated lists are not green. A timeout preserves the last
snapshot and reports any failed read separately.

## Keep output small without losing receipts

```sh
switchboard google.mail.search --ns google.personal --query 'newer_than:1d' \
  --fields messages.subject,messages.from --json
switchboard google.mail.read --ns google.personal --message-id ID --full --json
```

Field paths are relative to `fields`; array paths apply to each row. Projection
changes only the returned view, never durable operations or batch checkpoints.
Human output abbreviates long strings/lists with explicit markers. JSON retains
full payloads unless fields were selected. `--full` restores diagnostic details
and pretty JSON, but does not expand a provider's read bounds.

## Raw provider access

```sh
switchboard google.cli.read --ns google.personal --json -- \
  calendar +agenda --format json --today
```

Everything after `--` belongs to the provider. Native help there follows normal
execution policy and authentication; Switchboard help before it is local.

## Timings

Execution JSON reports measured wall-clock microseconds. `auth_us` measures
credential resolution, `adapter_us` the adapter call, and `execution_us` auth
through adapter completion. These overlap; do not add them together. Provider
backend phases distinguish locating, probing, materializing credentials,
executing, and decoding. Missing phases are unmeasured, not zero.
