# Messages

`messages` reads local macOS Messages history. There are no send, draft, delete,
mark-read, raw SQL, or other write commands. Reads preserve unread state.

## Install

```sh
nix build .#messages
./result/bin/messages --help
```

Or build from source with `cargo build -p messages-cli`.

The database defaults to `~/Library/Messages/chat.db`. `--db PATH` or
`MESSAGES_DB` selects another database. `messages doctor` checks read access,
schema compatibility, and existing Contacts authorization. If macOS denies
database access, the application running the command needs permission in
System Settings > Privacy & Security > Full Disk Access. The CLI does not
change permissions or trigger Contacts consent.

## Read history

```sh
messages chats
messages chats --with '+15555550123' --unread
messages chats --with 'Alex'
messages read CHAT_ID --limit 50
messages read CHAT_ID --unread
messages search 'dinner' --since 2026-10-01 --until 2026-11-01
messages search 'dinner' --chat CHAT_ID --json
```

`--with` accepts phone numbers, email addresses, and contact names. Repeat it
to require every participant. Contacts names are available only with an existing
authorization; otherwise handles still work. Phone punctuation is normalized,
but country codes are never guessed.

Chats are sorted by latest activity, and date filters apply to that last
activity. Reading a conversation selects its newest page and displays it in
chronological order. Search is a case-insensitive substring match over plain
and decoded attributed-body text, sorted newest first. Reactions and system
events retain their kind and associated-message reference.

All list commands accept `--limit` (1–500) and `--cursor`. Default limits are
20 chats, 50 messages, and 100 activity buckets. Repeat the same command and
filters with the returned cursor. Each search invocation scans at most 5,000
candidates; a page can contain no matches and still have a continuation.
`unsupported_bodies` reports text that could not be decoded. Missing text
must not be treated as a confirmed empty message or a complete search.

Dates accept `YYYY-MM-DD` (midnight UTC) or RFC 3339 timestamps. `--since` is
inclusive and `--until` is exclusive. JSON timestamps and activity buckets are
UTC. Human output is the default; `--json` emits typed data with stable chat
and message GUIDs and an opaque `next_cursor`.

## Attachments and images

```sh
messages attachment ATTACHMENT_ID
messages attachment ATTACHMENT_ID --json
messages attachment ATTACHMENT_ID --raw > attachment.png
```

Message reads include attachment metadata and local availability. The attachment
command handles images and other files: `--json` includes MIME type and base64
content, and `--raw` emits original bytes. Reads are capped at 16 MiB and confined
to regular files beneath the database's `Attachments` directory. Symlinks and
special files are rejected. No cloud downloads are initiated. Shell redirection
is optional; the CLI itself has no file-writing interface.

## Activity

```sh
messages activity
messages activity --chat CHAT_ID --since 2026-10-01
messages activity --interval day --json
messages activity --interval week
messages activity --interval month --limit 12
```

Counts include ordinary sent and received messages, excluding reactions and
system events. Messages linked to multiple conversations count once in overall
totals. Weeks start on Monday. Buckets are chronological and paginated; totals
cover the full requested range on every page.
Messages without a timestamp appear in a final `undated` bucket when no date
filter excludes them.

## Switchboard

```toml
[namespace.messages.local]
provider = "messages"
account = "local"
default_read = true
```

No credentials or auth block are needed. The namespace refers to the current
macOS user's local database, not an Apple account login.

```sh
switchboard messages.chats.list --ns messages.local
switchboard messages.chats.read --ns messages.local --chat-id CHAT_ID
switchboard messages.search --ns messages.local --query dinner
switchboard messages.attachments.read --ns messages.local --attachment-id ATTACHMENT_ID
switchboard messages.activity --ns messages.local --interval month
switchboard messages.doctor --ns messages.local
```

Switchboard calls the same read-only library directly. There is no Messages
raw passthrough or write tool. Coverage is limited to the history present on
this machine; it does not establish that all iCloud history is downloaded.
