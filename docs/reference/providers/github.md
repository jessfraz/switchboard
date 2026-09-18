# `github` Provider

`github` exposes `201` total tools, `10` curated and `191` raw inventory passthrough.

## Curated tools

- `github.ci.status` [stable] Read CI status for an exact commit
- `github.issue.comment` [planning_only] Draft or send an issue comment
- `github.issue.context` [stable] Read bounded issue context
- `github.issue.read` [stable] Read an issue
- `github.notifications.list` [stable] List notifications for a GitHub namespace
- `github.pull_request.comment` [planning_only] Draft or send a pull request comment
- `github.pull_request.context` [stable] Read bounded pull request context
- `github.pull_request.read` [stable] Read a pull request
- `github.pull_request.search` [stable] Search pull requests
- `github.repository.search` [stable] Search repositories

## Raw surfaces

- `github.cli.read`
- `github.cli.write`

`github` also ships `189` more raw command projections. The complete structured surface lives in [catalog.json](../catalog.json) and the deployed reference explorer.
