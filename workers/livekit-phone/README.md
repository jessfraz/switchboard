# LiveKit phone worker

This is the private Python subprocess for `phoneCLI`, not an agent daemon. It
connects to LiveKit Cloud only for one explicitly authorized call, then exits.
The Rust CLI owns authorization, credentials, encrypted storage, and rendering.
The worker pins `livekit-agents==1.8.2`; `uv.lock` pins the transitive packages.

## Local setup

Requires Python 3.12 or 3.13 and `uv`. From this directory:

```sh
uv sync --frozen
uv run --frozen --no-sync livekit-phone-worker check
```

`check` constructs the real pinned SDK's inference clients and their HTTP
context, then closes them. It does not contact LiveKit, run inference, or dial.
No separate model-weight download command is needed. Silero VAD ships with the
SDK dependency, and the turn detector uses LiveKit Inference without a local
fallback model. Setup downloads the locked Python packages.

The Nix `phone` package also contains this source, its lock file, and Python
3.13. Its [installation instructions](../../docs/phone.md#install-and-configure)
use a private, versioned uv environment and an explicit `worker_command`, so
the installed CLI does not require a source checkout.

## Runtime configuration

The supervisor injects `LIVEKIT_URL`, `LIVEKIT_API_KEY`,
`LIVEKIT_API_SECRET`, and `LIVEKIT_SIP_TRUNK_ID`. Do not create `.env` files or
store credentials here. The URL must be a `wss` LiveKit Cloud project URL.

Optional model settings are `LIVEKIT_PHONE_STT_MODEL` (default
`deepgram/nova-3`), `LIVEKIT_PHONE_LLM_MODEL` (default
`google/gemini-3.1-flash-lite`), `LIVEKIT_PHONE_TTS_MODEL` (default
`inworld/inworld-tts-2`), and `LIVEKIT_PHONE_VOICE` (default `Ashley`). All
inference uses the LiveKit gateway. No independent model-provider key is used.
These models require access and sufficient credits in the configured project.

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

Events are `ready`, `dialing`, `connected`, `transcript`,
`approval_required`, `error`, and finally `completed`. Every event carries
`protocol_version: 1`. Transcript events have `speaker` (`agent` or
`recipient`), `text`, `timestamp_ms` (Unix time at utterance creation), and
`interrupted`. Early media transcripts can arrive while dialing. Completed
events have `reason`, `remote_hangup_confirmed`, and an optional `summary`.
`completed` means the conversation ended normally, not that the requested
information was necessarily obtained. Voicemail and unavailable destinations
produce `failed` without leaving a message.

The subprocess emits committed SDK conversation messages, not token deltas.
A local transcript synchronizer truncates interrupted agent messages to the
SDK's estimate of the speech played. Transcriptions and alignment remain
machine-generated and can be imperfect. They are not audio recordings.

## Privacy and call lifetime

Cloud session recording is explicitly disabled, and no audio files or
transcript files are written by this worker. Text is not published as a room
data stream. OpenTelemetry exporters and SDK logs are disabled so stdout is
the sole transcript output. The supervisor must retain those events encrypted.
LiveKit, its inference providers, and the phone carrier still process the call.

The agent discloses AI use and transcription, asks permission before continuing
with a person, and stops on an objection or a need for additional authorization.
It has no account, payment, booking, messaging, filesystem, or shell tools.
The voice conversation and menu choices are model-driven and require real-call
evaluation before relying on their behavior.

The worker sets a server-side maximum call duration, deletes its unique room
on every normal exit path, and distinguishes confirmed termination from an
ambiguous network failure. If a dial request times out, a later successful
room deletion is not treated as proof that a delayed request could not dial.
The supervisor must report `remote_hangup_confirmed: false` as unknown, never
as a confirmed hangup. It should allow at least 45 seconds for cleanup before
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
SDK transcript types, secret-safe errors, and inference construction. They do
not claim to prove phone routing, speech quality, prompt adherence, or carrier
termination. Those require an explicitly authorized live test.

The implementation follows the official [Agents quickstart], [AMD guide], and
[recording controls]. Local source inspection additionally verified standalone
HTTP context requirements and transcript synchronization in the pinned SDK.

[Agents quickstart]: https://docs.livekit.io/agents/start/voice-ai/
[AMD guide]: https://docs.livekit.io/telephony/features/answering-machine-detection/
[recording controls]: https://docs.livekit.io/deploy/observability/insights/
[secure trunking configuration]: https://docs.livekit.io/telephony/features/secure-trunking/
