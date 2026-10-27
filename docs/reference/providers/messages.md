# `messages` Provider

`messages` exposes `6` total tools, `6` curated and `0` raw inventory passthrough.

## Curated tools

- `messages.activity` [stable] Count sent and received messages in local Messages history
- `messages.attachments.read` [stable] Read a locally available Messages attachment, including images
- `messages.chats.list` [stable] List recent conversations in local macOS Messages history
- `messages.chats.read` [stable] Read one local Messages conversation without marking it read
- `messages.doctor` [stable] Check read-only access to the local Messages database
- `messages.search` [stable] Search message text in local macOS Messages history

## Raw surfaces

- `messages.cli.read`
- `messages.cli.write`

`messages` also ships `0` more raw command projections. The complete structured surface lives in [catalog.json](../catalog.json) and the deployed reference explorer.
