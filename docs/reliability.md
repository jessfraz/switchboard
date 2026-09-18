# Results, recovery, and readback

Switchboard reports what it observed. Successful process execution, complete
read coverage, and verified remote state are separate claims. These contracts
describe the code in this checkout; building it does not replace an installed
binary, activate a Nix configuration, or deploy a provider change.

## Reading results

JSON responses include `schema_version: 1`. Errors carry stable `code` and
`phase` fields, provider and namespace when known, `retryable`, and a structured
`next_action`. A provider exit code or retry delay is included when available.
Branch on these fields instead of matching human-readable error text.

An empty, complete result is different from a blocked or incomplete read.
Partial aggregate results preserve successful namespace results and report
failures separately. The CLI returns a nonzero exit status for incomplete
results while retaining their JSON on stdout. Planning-only tools cannot report
successful execution. Use `tools describe TOOL` to inspect execution support,
arguments, scope guidance, output shape, pagination, and any raw fallback.

Gmail search returns one page by default (`--max 20`, up to 500), with exact
message IDs and metadata. Its `coverage.status` is `complete`, `truncated`, or
`unknown`; `next_cursor` carries the continuation token. Use `--cursor` to
request the next page. Metadata failures remain visible beside successful
messages and make coverage unknown. A result-count estimate is not a promise
that all matching messages have been returned.

## Authentication

Set one `SWITCHBOARD_RUN_ID` for all commands in a task. `auth check --ns NS`
performs a provider read and compares its identity with the configured auth
account. Google uses Gmail `getProfile`, so it requires Gmail access. GitHub
uses `api user`. Other identity routes are explicitly unsupported.

Credentials are resolved from cache first, then existing noninteractive
1Password sessions. A cache miss can claim one desktop-unlock attempt per
provider/account/run, lasting at most 60 seconds. The claim is persisted before
unlocking. Concurrent processes wait for the attempt's cached result; a failed
or interrupted attempt cannot grant another unlock within that run. An overall
batch deadline can shorten the remaining wait. Explicit session-only,
service-account, and disabled-biometric choices are respected.

A structured authentication rejection can invalidate the values actually used
from the vault cache and trigger one re-resolution. A concurrent refresh is
preserved. Network failures do not authorize vault recovery. Wrong account,
browser consent, credential rejection, timeout, and exhausted recovery are
distinct failure cases. Browser consent is not launched by `auth check`.

`auth migration-preview --ns NS --verify` checks a proposed saved Google CLI
session using the existing state directory. It reports a proposed configuration
change without applying it. File presence alone does not verify identity.
See the README for the pinned gws version's admin-service login limitations.

## Writes and uncertainty

Writes are persisted as operations before execution. Approval and execution
state are separate. Applying an approved operation atomically claims it as
`executing`; only that claim's owner may invoke the provider.

| State | Meaning |
| --- | --- |
| `planned` | A saved plan, possibly awaiting approval. |
| `executing` | Execution was claimed; a crashed process may leave this state. |
| `uncertain` | The remote effect cannot be determined from the execution receipt. |
| `applied` | The command completed and its effect receipt was saved. |
| `verified` | Supported provider readback matched its defined postcondition. |
| `failed` | A failure was recorded that permits the operation's normal retry checks. |
| `compensated` | A supported undo operation completed. |

Do not reapply an executing or uncertain operation. Inspect it with `op show ID`
and use `op verify ID --json`. A timeout, decode failure after provider success,
or lost persistence after a write can leave a remote effect behind. Verification
can establish a supported postcondition; an unavailable or mismatching readback
does not prove that nothing happened. Uncertain operations remain protected
against blind reapplication.

Calendar creation derives its provider event ID from the saved operation ID,
which permits readback after losing the create response. This is a scoped
idempotency mechanism, not a general guarantee for arbitrary raw commands.
Raw passthrough operations without a defined readback contract remain explicitly
unverifiable.

## What verification proves

Readback receipts record a status (`verified`, `mismatch`, or `unavailable`), a
check time, a summary, and exact provider references. Supported Google paths
have different postconditions:

- Gmail label modification checks the exact message ID, every requested added
  label being present, and every requested removed label being absent. It does
  not require unrelated labels to disappear.
- Draft creation checks the returned draft/message identities, a `DRAFT` label,
  and no `SENT` label. It proves a saved, unsent draft, not byte-for-byte message
  body or attachment equivalence.
- Calendar creation checks event identity, non-cancelled status, requested title,
  start/end, optional description/location, requested attendees, and a Meet link
  when requested. Timestamp comparison is conservative string equality; an
  equivalent instant formatted differently can produce a mismatch.
- Drive upload readback compares the returned file identity and provider
  checksum with a subsequent provider read and rejects trashed files. It does
  not independently hash the original local file. If the upload response lacks
  a checksum, verification is unavailable.

## Bounded read batches

`read-batch` accepts 1 to 1,000 uniquely identified curated read requests.
Unrestricted raw commands and writes are rejected. Authentication runs serially
before bounded parallel reads. Input arguments use the normal typed argument
representation:

```json
{
  "items": [
    {
      "id": "recent-mail",
      "tool": "google.mail.search",
      "namespace": "google.personal",
      "args": [{"kind": "option", "name": "query", "value": "newer_than:1d"}]
    }
  ]
}
```

```sh
switchboard read-batch --input reads.json --checkpoint reads-state.json --json
switchboard read-batch --input reads.json --checkpoint reads-state.json --resume --json
```

The defaults are four concurrent requests, ten pages per item per invocation,
and a 120-second shared deadline. Their bounds are configurable. Recognized
provider rate-limit responses may receive one read retry when the reported
delay is at most 60 seconds and fits within the remaining deadline.

Checkpoints preserve results after each bounded wave and on completion. Resume
requires the same exact input and validates stored page identity and cursor
continuity. Completed items are not rerun. Repeated continuation tokens are
errors. Gmail partial metadata pages are retried at their original cursor;
failed retries preserve earlier evidence. Tools without a completeness contract
retain unknown coverage even after their single logical request finishes.

## Timings

Successful execution can include measured microseconds for credential
resolution, adapter work, executable lookup, capability probing, credential
materialization, provider execution, and decoding. Missing fields mean a phase
was not measured, not zero elapsed time. `execution_us` covers auth through
adapter completion, including a write's local claim; it excludes planning,
final persistence, audit, and readback. These are local elapsed-time
observations, not claims about remote server latency or process-start counts.
