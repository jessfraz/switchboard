# LLM Patterns

These are prompt-shaped workflows for model callers, not just random command snippets.

The model does the reasoning.
`switchboard` provides typed, namespace-aware primitives.
Writes still go through draft, approval, and apply.

## Review open GitHub pull requests

Example user request:

> Review my open PRs and draft a comment on the riskiest one.

What should happen:

1. The model decides this is a `search or list -> read -> inspect -> draft comment` workflow.
2. The model searches or lists the relevant open pull requests.
3. The model reads the best candidate PRs.
4. The model inspects files, checks, or discussion as needed.
5. The model drafts a comment instead of posting it directly.

Example tool chain:

```sh
switchboard github.pull_request.search \
  --ns github.personal \
  --repo owner/repo \
  --state open \
  --json

switchboard github.pull_request.read \
  --ns github.personal \
  --repo owner/repo \
  --number 123 \
  --json

switchboard github.issue.comment \
  --ns github.personal \
  --repo owner/repo \
  --number 123 \
  --body "needs tests" \
  --draft \
  --json
```

Note: `github.issue.comment` is planning-only today, so expect a clean plan and no apply path yet.

## Latest email to calendar event

Example user request:

> You see the latest email from the car wash people, make me a calendar event with the details.

What should happen:

1. The model decides this is a `search -> read -> extract -> draft calendar event` workflow.
2. The model searches the allowed Gmail namespace or namespaces for the latest relevant message.
3. The model reads the winning message in the namespace it came from.
4. The model extracts structured event fields from the email.
5. The model chooses the target calendar namespace, or asks if that is ambiguous.
6. The model drafts the calendar event and shows the preview before applying it.

Example tool chain:

```sh
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

If the namespace is ambiguous, the model should ask instead of guessing.

## Similar email from sent history

Example user request:

> Look up all the past emails I sent to book the dogs at the dog hotel, make a new appointment for these dates by sending them a similar email.

What should happen:

1. The model decides this is a `search sent mail -> read history -> extract pattern -> draft similar email` workflow.
2. The model searches sent mail for prior booking conversations.
3. The model reads the best matching messages or threads.
4. The model extracts the reusable structure, recipient, subject pattern, and booking details they usually need.
5. The model fills in the new dates.
6. The model drafts a new email in the same namespace.
7. The model shows the draft and provenance before sending it.

Example tool chain:

```sh
switchboard google.mail.search \
  --ns google.personal \
  --query 'in:sent "dog hotel"' \
  --json

switchboard google.mail.read \
  --ns google.personal \
  --message-id 18c7f6... \
  --json

switchboard google.mail.draft \
  --ns google.personal \
  --to bookings@doghotel.example \
  --subject 'Boarding request for June 10 to June 14' \
  --body 'Hi, I would like to book...' \
  --draft \
  --json
```

Fuzzy reads are fine for finding candidates. They are not enough to auto-send mail without review.

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
