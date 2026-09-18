# LiveKit phone worker

This is the private Python subprocess for `phoneCLI`, not an agent daemon. It
connects to LiveKit Cloud only for one explicitly authorized call, then exits.
The Rust CLI owns authorization, credentials, encrypted storage, and rendering.
The worker pins `livekit-agents==1.8.2`; `uv.lock` pins the transitive packages.

See the [phone quickstart](../../docs/phone.md) for prerequisites, credentials,
and calls. This README documents the worker contract; run calls through `phone`
or Switchboard for authorization, encrypted storage, and supervision.

## Local setup

Requires Python 3.12 or 3.13 and `uv`. From this directory:

```sh
uv sync --frozen
uv run --frozen --no-sync livekit-phone-worker check
```

`check` constructs GPT-Live using the real pinned SDK clients and
their HTTP context, then closes them. It does not contact a provider, run
inference, or dial.
No separate model-weight download command is needed. Silero VAD ships with the
SDK dependency. GPT-Live handles turn-taking directly. Setup downloads the
locked Python packages.

### Nix installation

Install `packages.<system>.phone` through your Nix configuration. The package
contains immutable worker source, its lock file, and Python 3.13. From the
repository root, prepare a private environment using that same package pin:

```sh
phone_package=$(nix build .#phone --no-link --print-out-paths)
worker_source=$(readlink "$phone_package/share/phone/livekit-worker")
phone_env="${XDG_DATA_HOME:-$HOME/.local/share}/phone/workers/$(basename "$phone_package")"
umask 077
UV_PROJECT_ENVIRONMENT="$phone_env" uv sync \
  --locked --no-dev --no-editable \
  --project "$worker_source" \
  --python "$phone_package/share/phone/python"
"$phone_env/bin/livekit-phone-worker" check
```

Set `worker_command` in the phone TOML to the absolute path of
`$phone_env/bin/livekit-phone-worker`, instead of `worker_project`. Calls use
that executable without a checkout or dependency installation. When the source
or interpreter changes, repeat setup and update the path. Keep the corresponding
Nix package generation installed so garbage collection retains the interpreter;
`nix build --no-link` alone does not retain it.

## Runtime configuration

The supervisor injects `LIVEKIT_URL`, `LIVEKIT_API_KEY`,
`LIVEKIT_API_SECRET`, and `LIVEKIT_SIP_TRUNK_ID`. Do not create `.env` files or
store credentials here. The URL must be a `wss` LiveKit Cloud project URL.

GPT-Live is the only voice engine. Every call requires `OPENAI_API_KEY` in the
worker environment and an account with model access.
`LIVEKIT_PHONE_REALTIME_MODEL` defaults to `gpt-live-1`,
`LIVEKIT_PHONE_BACKEND_MODEL` defaults to `gpt-5.6-luna`, and
`LIVEKIT_PHONE_VOICE` defaults to `marin`.
`LIVEKIT_PHONE_BACKEND_REASONING_EFFORT` optionally sets `none`, `minimal`,
`low`, `medium`, `high`, `xhigh`, or `max`; the backend must support that level.
When omitted, the API's model default applies. For example, use `gpt-6-astra`
with `xhigh` for deeper delegated reasoning, and `cedar` for a different voice.
GPT-Live handles audio and transcripts directly through the official OpenAI
endpoint, with the backend model handling delegated reasoning and local tools.
Answering-machine detection reuses those native transcripts and calls the same
backend model directly through OpenAI Responses, with `low` reasoning and
`store=false`. Choose a backend that supports Responses tool calls and `low`
reasoning. This separate greeting check does not change the configured reasoning
effort for delegated work. No LiveKit Inference services or credits are used;
LiveKit credentials and billing still apply to room/SIP transport. Keys belong
in 1Password and must be supplied by the supervising process, not written to
this source directory.

The public CLI takes these model settings from `[livekit]` in its phone TOML.
It maps `PHONE_MODEL_API_KEY` (or standalone `OPENAI_API_KEY`) into the worker's
`OPENAI_API_KEY`. With Switchboard, configure the `phone_cli`
auth entry's `model_api_key` reference as well; inherited OpenAI environment
variables are not implicitly accepted. See the guide's [credential
injection](../../docs/phone.md#inject-credentials-from-1password) and
[Switchboard setup](../../docs/phone.md#through-switchboard).

For existing phone TOML, `voice_engine = "gpt_live"` remains accepted but is
unnecessary. Remove or replace `voice_engine = "pipeline"`, remove the retired
`stt_model`, `llm_model`, and `tts_model` settings, and supply the OpenAI key.
Replace a pipeline voice such as `Ashley` with a GPT-Live voice, or omit `voice`
to use Marin. Keep model and voice overrides under `[livekit]`.

The stored SIP trunk must already use TLS. The worker reads its configuration
and requires SRTP for each call; it never changes the trunk. Ordinary telephone
network segments are not end-to-end encrypted by this setting. Both provider
and LiveKit trunk settings must support the [secure trunking configuration].

## Private process protocol

Run `uv run --frozen --no-sync livekit-phone-worker`. Stdout contains one JSON
event per line. Wait for `ready`, then send one start command on stdin:

```json
{"protocol_version":1,"type":"start","call_id":"unique-local-id","destination":"+12025550100","task":"Ask for opening hours.","caller_name":"Caller","max_duration_seconds":600}
```

Keep stdin open during the call. A version 1 `cancel` command, EOF, SIGINT, or
SIGTERM requests cancellation. The maximum duration is 30 to 3600 seconds.
There is exactly one start command per process, and the worker never redials.
Identifiers must be unique per attempted call; the supervisor enforces this.

Events are `ready`, `dialing`, `connected`, `transcript`, `error`, and finally
`completed`. Every event carries
`protocol_version: 1`. Transcript events have `speaker` (`agent` or
`recipient`), `text`, `timestamp_ms` (Unix time at utterance creation), and
`interrupted`. Early media transcripts can arrive while dialing. Completed
events have `reason`, `remote_hangup_confirmed`, and an optional `summary`.
`completed` means the conversation ended normally, not that the requested
information was necessarily obtained. Voicemail and unavailable destinations
produce `failed` without leaving a message.

The subprocess emits committed SDK conversation messages, not token deltas.
A local transcript synchronizer truncates interrupted agent messages to the
SDK's estimate of the speech played. GPT-Live segments continuous speech into
turns, and transcripts can arrive after the audio. Nonverbal sounds such as
laughter may have no transcript. Transcriptions and alignment remain
machine-generated and can be imperfect. They are not audio recordings.

## Privacy and call lifetime

LiveKit session recording is explicitly disabled, and no audio files or
transcript files are written by this worker. Text is not published as a room
data stream. OpenTelemetry exporters and SDK logs are disabled so stdout is
the sole transcript output. The supervisor must retain those events encrypted.
LiveKit, OpenAI, and the phone carrier still process
the call. GPT-Live's pinned plugin omits the storage option, relying on the
Live API's documented `store: false` default. The plugin does not expose that
option publicly. This disables stored session recordings and forks; it does
not imply zero provider retention. See [GPT-Live session storage].

The agent introduces itself as the caller's AI assistant and discloses
transcription without asking a consent question. There is no in-call approval
tool. It declines disallowed alternatives and continues the approved task,
ending on recipient objections or when no permitted way forward remains.
The caller must establish the applicable legal
basis for transcription and retention before dialing.
It has no account, payment, booking, messaging, filesystem, or shell APIs.
The approved brief defines what it may accomplish through the conversation,
including refunds, support, reservations, appointments, and general questions.
Include the necessary facts, permitted choices, and any cost or time limits.
For example, a refund-only brief can forbid store credit and fees; a restaurant
brief can authorize a table within a time window but exclude deposits. These
are task-specific constraints, not hardcoded restrictions on every call.
The agent must not invent missing details or agree to commitments outside the
brief. It continues within those constraints without in-call approval pauses.
The voice conversation and menu choices are model-driven and require real-call
evaluation before relying on their behavior. GPT-Live produces its opening
natively. The worker observes that opening instead of issuing another greeting
after answering-machine detection releases queued audio. If no agent transcript
arrives within 20 seconds after detection, the worker stops. Its spoken wording
and objection handling are still model-driven.

Answering-machine detection is closed after its initial verdict so it cannot
suppress subsequent conversation turns. Keypad tools remain available even if
a menu appears after talking to a human. GPT-Live waits natively on hold,
during a transfer, or while someone checks information, since its backend
always speaks after a delegated tool result.
Transfers are not completed tasks. The SDK's IVR silence wakeups are disabled
so waiting does not
trigger a new response every five seconds. Automated menus get an explicit
first response. The SDK owns model retries; only a terminal session failure
produces a fatal diagnostic, using its typed error category without provider
error text. A recipient hangup before any committed agent speech is reported
as a failed call.

The worker sets a server-side maximum call duration, deletes its unique room
on every normal exit path, and distinguishes confirmed termination from an
ambiguous network failure. If a dial request times out, a later successful
room deletion is not treated as proof that a delayed request could not dial.
The supervisor must report `remote_hangup_confirmed: false` as unknown, never
as a confirmed hangup. It should allow at least 60 seconds for cleanup before
force-killing the subprocess. A killed process cannot perform cleanup; the
server-side duration limit is the remaining safeguard.

## Offline validation

```sh
uv run --frozen --no-sync ruff check src tests
uv run --frozen --no-sync ruff format --check src tests
uv run --frozen --no-sync mypy src tests
uv run --frozen --no-sync pytest -q
```

Tests exercise the real process protocol, bounded inputs, cancellation state,
SDK transcript types, secret-safe errors, model settings, and real client
construction and cleanup. They do not claim to prove credentials, model access,
phone routing, speech quality, prompt adherence, or carrier termination. The
public CLI's `doctor` is also offline and does not execute `worker_command`;
run the exact configured executable with `check`. Follow the onboarding guide's
[owned-number test](../../docs/phone.md#make-a-call)
for live validation with explicit authorization.

The implementation follows the official [Agents quickstart], [AMD guide], and
[recording controls]. Local source inspection additionally verified standalone
HTTP context requirements and transcript synchronization in the pinned SDK.

[Agents quickstart]: https://docs.livekit.io/agents/start/voice-ai/
[AMD guide]: https://docs.livekit.io/telephony/features/answering-machine-detection/
[recording controls]: https://docs.livekit.io/deploy/observability/insights/
[secure trunking configuration]: https://docs.livekit.io/telephony/features/secure-trunking/
[GPT-Live session storage]: https://developers.openai.com/api/docs/guides/live-conversations#store-and-fork-a-session
