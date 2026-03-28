# LLM Patterns

These are short, repeatable patterns for model callers.

## Review open GitHub pull requests

```sh
switchboard github.pull_request.search --ns github.personal --repo owner/repo --state open --json
switchboard github.pull_request.read --ns github.personal --repo owner/repo --number 123 --json
```

## Draft a GitHub comment without applying it

```sh
switchboard github.issue.comment \
  --ns github.personal \
  --repo owner/repo \
  --number 123 \
  --body "needs tests" \
  --draft
```

Note: `github.issue.comment` is planning-only today, so expect a clean plan and no apply path yet.

## Search Gmail across one namespace

```sh
switchboard google.mail.search \
  --ns google.work \
  --query 'from:finance newer_than:7d' \
  --json
```

## Draft a Gmail reply

```sh
switchboard google.mail.draft \
  --ns google.work \
  --thread-id THREAD_ID \
  --to boss@example.com \
  --subject "Re: Budget" \
  --body "Draft reply text" \
  --draft
```

## Create a calendar event with approval

```sh
switchboard google.calendar.create \
  --ns google.work \
  --title "Launch review" \
  --start "2026-04-10T15:00:00-07:00" \
  --end "2026-04-10T16:00:00-07:00" \
  --draft

switchboard op approve op_1234abcd --actor codex --note "approved by agent"
switchboard op apply op_1234abcd --json
```

## Use raw `gws` coverage in a multi-login-safe way

```sh
switchboard google.cli.read \
  --ns google.personal \
  --json \
  -- slides presentations get --presentation-id PRESENTATION_ID --format json
```

The point is not just forwarding argv. The point is forwarding argv through a namespace that isolates credentials and optional CLI state.
