# Phone calls

`phone` makes approved calls through LiveKit Cloud and saves encrypted local
transcripts. Keep your computer awake and online; no hosted agent or webhook is
needed. Rust's `CallBackend` / `ActiveCall` traits isolate the provider SDK.

## What you need

- macOS or Linux, Rust, `uv`, Python 3.12 or 3.13, `age`, and [1Password CLI].
- A [LiveKit project][LiveKit credentials], URL, API key/secret, and [Inference]
  credits. GPT-Live also needs Inference for answering-machine detection.
- An [outbound SIP trunk] and authorized caller ID. For Twilio, follow its
  [Elastic SIP Trunking setup][Twilio setup], enable Secure Trunking, and
  require TLS/SRTP in LiveKit using [secure trunking]. Use the LiveKit `ST_`
  trunk ID. Carrier billing is separate.
- An age X25519 public recipient, with its private identity backed up securely.
- For `gpt_live`, an [OpenAI API key] with access to `gpt-live-1` and the chosen
  backend. The [GPT-Live plugin] currently requires alpha access; funding an
  account alone does not grant it. OpenAI billing is separate too.

| Engine | Defaults | Runtime credentials |
| --- | --- | --- |
| `pipeline` | Deepgram Nova-3, Gemini 3.1 Flash Lite, Inworld TTS-2 / Ashley | LiveKit API key and secret; no separate model-provider keys |
| `gpt_live` | GPT-Live 1, delegated GPT-5.6 Luna, Marin voice | LiveKit credentials plus an OpenAI API key |

Replace example values; keep config and secrets outside the repository.

## Install and configure

From the repository root:

```sh
cargo install --locked --path crates/phone-cli
uv sync --locked --project workers/livekit-phone --python 3.13
uv run --locked --no-sync --project workers/livekit-phone livekit-phone-worker check
```

For Nix, use the [packaged worker setup](../workers/livekit-phone/README.md#nix-installation).
The worker's `check` is offline. It verifies SDK construction and cleanup,
not credentials, balances, model access, or phone routing.

Generate an X25519 identity with [age-keygen] (not `-pq`) and back it up:

```sh
umask 077
phone_key_dir=$(mktemp -d)
age-keygen -o "$phone_key_dir/identity.txt"
age-keygen -y "$phone_key_dir/identity.txt"
op document create "$phone_key_dir/identity.txt" \
  --vault Private --title 'Phone Transcript Identity'
op document get 'Phone Transcript Identity' --vault Private | age-keygen -y
```

Verify both public recipients match, then remove the temporary file/directory.
Keep the vault backup; losing it makes notes unreadable. Configure only the
complete public `age1...` recipient.

Save this as `~/.config/phone/config.toml`, replacing paths and values. Paths
must be absolute, without `~` expansion. Keep the state directory private
(mode `0700`); `--config` or `PHONE_CONFIG` selects another config file.

```toml
backend = "livekit"
caller_name = "Alex"
max_duration_seconds = 600
transcript_recipient = "age1...your-complete-public-recipient"
state_dir = "/absolute/path/to/private/phone/calls"
worker_project = "/absolute/path/to/switchboard/workers/livekit-phone"

[livekit]
url = "wss://your-project.livekit.cloud"
sip_trunk_id = "ST_your_outbound_trunk"
voice_engine = "pipeline"
```

For the Nix installation, use `worker_command` pointing to the prepared worker
executable instead of `worker_project`. For GPT-Live with [Astra] reasoning,
replace the `[livekit]` section with:

```toml
[livekit]
url = "wss://your-project.livekit.cloud"
sip_trunk_id = "ST_your_outbound_trunk"
voice_engine = "gpt_live"
realtime_model = "gpt-live-1"
backend_model = "gpt-6-astra"
backend_reasoning_effort = "xhigh"
voice = "cedar"
```

Reasoning effort is optional and must be supported by the backend. Higher
levels can increase latency and cost; the call deadline still applies.
See the [worker README] for model overrides and defaults.

## Inject credentials from 1Password

Use concealed credential fields and keep the transcript identity separate.
Put your vault references in `~/.config/phone/runtime.env`:

```dotenv
PHONE_API_KEY="op://Private/Phone Runtime/livekit_api_key"
PHONE_API_SECRET="op://Private/Phone Runtime/livekit_api_secret"
# Uncomment for gpt_live:
# PHONE_MODEL_API_KEY="op://Private/OpenAI/api_key"
```

This file contains [secret references], not key values. Avoid shell tracing
and never put plaintext keys in command arguments. Then check local config:

```sh
op run --env-file "$HOME/.config/phone/runtime.env" -- phone --json doctor
```

`doctor` validates config, encryption, and credential presence. It does not
contact providers or run the worker; with `worker_command`, it does not even
check executable existence. Run that exact executable with `check` as well.
Standalone `phone` also accepts `LIVEKIT_API_KEY` / `LIVEKIT_API_SECRET` and
`OPENAI_API_KEY` fallbacks. Only GPT-Live workers receive the OpenAI key.

## Through Switchboard

Merge these entries into `~/.config/switchboard/config.toml`:

```toml
[secret]
phone_api_key = { kind = "env", name = "PHONE_API_KEY" }
phone_api_secret = { kind = "env", name = "PHONE_API_SECRET" }
phone_model_api_key = { kind = "env", name = "PHONE_MODEL_API_KEY" }

[auth.phone_personal]
provider = "phone"
kind = "phone_cli"
account = "personal"
api_key = "phone_api_key"
api_secret = "phone_api_secret"
# Uncomment for gpt_live:
# model_api_key = "phone_model_api_key"

[namespace.phone.personal]
provider = "phone"
account = "personal"
auth = "phone_personal"
state_dir = "/absolute/path/to/private/switchboard/phone.personal"
```

Save phone TOML as `config.toml` in the namespace directory; journals go into
`calls/`. GPT-Live requires `model_api_key`. There is no raw phone command.
Using `env` avoids the whole-item cache of `onepassword_item`; never include
decryption identities in items resolved through that cache.

```sh
op run --env-file "$HOME/.config/phone/runtime.env" -- \
  switchboard phone.doctor --ns phone.personal --json
```

## Make a call

Use a number you control. Replace the fictional number below, review the
brief and duration, then call it:

```sh
op run --env-file "$HOME/.config/phone/runtime.env" -- \
  phone call +12125550101 'Do a short audio check, then finish.'
```

The `call` command authorizes and starts that call without another approval
flag. Your configured `caller_name` supplies whose assistant is calling;
`--caller-name` overrides it for one call. `max_duration_seconds` defaults to
600 and includes hold time, with an allowed range of 30 to 3600 seconds.
`--max-duration-seconds` overrides that setting for one call. Model, voice,
trunk, worker, and transcript settings come from the same phone configuration.

With credentials already supplied in the environment, the command is simply
`phone call NUMBER 'PROMPT'`. Standalone `phone` does not resolve Switchboard's
1Password references or automatically select its namespace configuration. Use
the credential injection above for standalone calls, or continue through
Switchboard with its existing configuration and authentication.

With Switchboard, use `--approve-and-apply` to plan, record your approval, and
apply the exact operation in one command. It uses the namespace's existing
credentials:

```sh
switchboard phone.call.run --ns phone.personal --approve-and-apply \
  --destination +12125550101 --caller-name Example \
  --task 'Do a short audio check, then finish.' --max-duration-seconds 180
```

For an `env` credential configuration, wrap this command in `op run` as above.
For `onepassword_item` references, Switchboard resolves the configured vault
fields itself. No separate credential export is needed.

Omit `--approve-and-apply` to save a pending operation for review, then use
`switchboard op approve OPERATION_ID --apply` when ready. `--approve-and-apply`
is an explicit authorization for that command's exact destination, brief, caller,
and duration. It cannot be combined with planning/draft/apply/dry-run modes or
multiple namespaces, and it does not override a policy that denies writes.
The result retains the operation ID and its plan, approval, and execution audit
events. `--apply` by itself still cannot bypass required approval.

Check the opening, audio, interruptions, transcript completeness, and actual
hangup. Confirm `remote_hangup_confirmed`; false means unknown. Calls cannot be
undone and never automatically redial. Another attempt needs a new
call/operation ID. The supervised `phone run` command still requires
`--approve` and explicit request values; it does not use the new caller or
duration defaults. For sensitive standalone briefs, use
`phone run --approve --request-stdin` to avoid process arguments.

## Transcripts and privacy

```sh
phone transcripts list
op run --env-file "$HOME/.config/phone/runtime.env" -- \
  switchboard phone.transcripts.list --ns phone.personal --json
```

Decrypt using the vault backup (Bash or Zsh, with shell tracing disabled):

```sh
set -o pipefail
PHONE_TRANSCRIPT_IDENTITY="$(
  op document get 'Phone Transcript Identity' --vault Private |
    sed -n '/^AGE-SECRET-KEY-1/p'
)" phone transcripts show 'replace-with-call-id'
```

The identity variable accepts only the private-key line, not file comments.
For Switchboard calls, add `--config /absolute/path/to/namespace/config.toml`
and set `PHONE_STATE_DIR` to its `calls/` directory. Decryption prints plaintext
JSON; keep it out of logs. Journals are encrypted, but Switchboard's operation
database still contains the brief, destination, and audit metadata.

The agent announces AI assistance and transcription without a consent question.
There is no in-call approval tool. It answers routine questions, declines
disallowed alternatives, and continues pursuing the approved task. It ends if
the recipient objects or no permitted way forward remains.
The brief defines the outcome and constraints for refunds, support,
reservations, appointments, or general questions. Supply the relevant facts,
acceptable alternatives, and any cost or scheduling limits. A request to ask
about availability is not authorization to book; an explicit booking request
lets the agent confirm an option within its stated constraints. It declines
commitments outside the brief instead of stopping for caller approval.
Menus, conversational AI, people, hold queues, and transfers can occur in any
order. Keypad navigation remains available throughout; hold and transfer
announcements do not mean the task is complete. The agent waits silently and
resumes when addressed. The maximum duration still bounds the whole call.
These are model-driven conversation rules, not guaranteed outcomes.
Establish the legal basis for transcription before dialing. Voice behavior
remains model-driven.

No raw audio is saved locally. Keep project observability and carrier recording
off; the worker disables LiveKit recording and relies on GPT-Live's
[default `store: false`][session storage]. Providers still process call data;
this is neither zero retention nor end-to-end PSTN encryption. Local journals
do not expire automatically. See [OpenAI data controls] and the [worker README].

[1Password CLI]: https://www.1password.dev/cli/get-started
[LiveKit credentials]: https://docs.livekit.io/reference/telephony/connectors-api/#constructor-parameters
[Inference]: https://docs.livekit.io/agents/models/inference/
[outbound SIP trunk]: https://docs.livekit.io/telephony/making-calls/outbound-trunk/
[Twilio setup]: https://docs.livekit.io/telephony/start/providers/twilio/
[secure trunking]: https://docs.livekit.io/telephony/features/secure-trunking/
[OpenAI API key]: https://developers.openai.com/api/docs/quickstart
[GPT-Live plugin]: https://docs.livekit.io/agents/models/realtime/plugins/gpt-live/
[age-keygen]: https://github.com/FiloSottile/age/blob/main/doc/age-keygen.1.ronn
[Astra]: https://developers.openai.com/api/docs/models/gpt-6-astra
[secret references]: https://www.1password.dev/cli/secrets-environment-variables
[session storage]: https://developers.openai.com/api/docs/guides/live-conversations#store-and-fork-a-session
[OpenAI data controls]: https://developers.openai.com/api/docs/guides/your-data
[worker README]: ../workers/livekit-phone/README.md
