# Phone calls

`phone` is a separate Rust binary in this workspace. It runs one approved call
from your computer and saves an encrypted local transcript. LiveKit Cloud
provides the media service and SIP bridge; a local Python subprocess runs
LiveKit Agents and uses LiveKit Inference for speech and reasoning. No VM,
public webhook, or always-running agent is required. Your computer must stay
awake and connected for the call.

This implementation has offline validation. Phone routing, voice quality, IVR
navigation, and carrier hangup behavior still need an authorized live test.
The current agent gathers information from businesses. It is instructed to
stop if someone objects to AI or transcription, or if the task would require
a booking, purchase, cancellation, payment, account change, or new authority.
Those conversational restrictions are model instructions, not a guarantee
that a model cannot say something incorrect.

## Architecture

```text
Switchboard: namespace, exact approval, audit, stable operation ID
    |
phone: validated request, call lifetime, encrypted transcript journal
    |
CallBackend / ActiveCall: provider-independent Rust traits and events
    |
LiveKitBackend: private, versioned NDJSON subprocess protocol
    |
local Python worker: LiveKit Agents, speech, interruption handling, IVR
    |
LiveKit Cloud + configured SIP carrier -> telephone
```

Replacing LiveKit means implementing `CallBackend` and `ActiveCall`. The core
call types, transcript storage, and Switchboard interface do not import its
SDK. The worker has no shell, filesystem, account, or payment tools. It receives
the call brief and scoped provider credentials, never the transcript decryption
identity or credentials from other Switchboard namespaces.

## Install and configure

From the repository root, install the Rust binary and the locked Python worker:

```sh
cargo install --locked --path crates/phone-cli
uv sync --locked --project workers/livekit-phone --python 3.13
uv run --locked --no-sync --project workers/livekit-phone livekit-phone-worker check
```

The final command constructs and closes the actual SDK clients without making
any network requests. For this development installation, set `worker_project`
explicitly to the worker checkout. The Nix development shell includes `uv` and
Python 3.13.

The Nix package includes the Rust binary, immutable worker source with its lock
file, and a Python 3.13 interpreter. Install `packages.<system>.phone` through
your Nix configuration. Then explicitly create a private worker environment
from that package, without depending on a checkout:

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

For this installation, configure `worker_command` with the absolute path to
`$phone_env/bin/livekit-phone-worker`, instead of `worker_project`. Setup
downloads the locked Python packages once. Calling uses that executable
directly and never installs dependencies. When the packaged worker source
or interpreter changes, run setup again and update `worker_command` to the new
environment. Use the package installed by your Nix configuration, including
its input pins, when preparing that environment.
The installed Nix package retains its Python interpreter across garbage
collection; avoid deleting older package generations while their worker
environments remain configured.

Before dialing, provision a LiveKit Cloud project, inference access/credits,
and an outbound SIP trunk with its caller ID. Configure the carrier and trunk
for TLS and SRTP. The worker refuses a trunk that does not select TLS and
requires SRTP per call. This protects the SIP leg, not every telephone-network
segment. See the [worker setup](../workers/livekit-phone/README.md).

Keep the LiveKit API key, API secret, and an age X25519 private identity in
1Password. Put only the matching public `age1...` recipient in the phone
configuration. No private identity is needed to make a call or encrypt notes.
Losing it makes the archived notes unreadable.

Create a local configuration outside the repository. Replace the placeholder
paths and values; paths in this file must be absolute, without `~` expansion:

```toml
backend = "livekit"
transcript_recipient = "age1..."
state_dir = "/absolute/path/to/private/phone/calls"
worker_project = "/absolute/path/to/switchboard/workers/livekit-phone"

[livekit]
url = "wss://your-project.livekit.cloud"
sip_trunk_id = "ST_your_outbound_trunk"
```

`phone` reads `~/.config/phone/config.toml` by default. `--config` or
`PHONE_CONFIG` selects another file. Standalone execution accepts
`PHONE_API_KEY` and `PHONE_API_SECRET` injected by your secret manager.
`phone --json doctor` validates local settings and encryption and reports
whether credentials and the worker project are present. It does not verify
provider credentials, balances, trunk connectivity, or installed Python
dependencies; use the offline worker check for the latter.

## Through Switchboard

Add a namespace and 1Password references to your existing Switchboard config:

```toml
[secret.phone_api_key]
kind = "onepassword_item"
account = "your-1password-account"
vault = "Private"
item = "Phone LiveKit"
field = "api_key"

[secret.phone_api_secret]
kind = "onepassword_item"
account = "your-1password-account"
vault = "Private"
item = "Phone LiveKit"
field = "api_secret"

[auth.phone_personal]
provider = "phone"
kind = "phone_cli"
account = "personal"
api_key = "phone_api_key"
api_secret = "phone_api_secret"

[namespace.phone.personal]
provider = "phone"
account = "personal"
auth = "phone_personal"
state_dir = "~/.local/share/switchboard/namespaces/phone.personal"
```

Save the phone TOML above as `config.toml` inside that namespace directory.
Switchboard forces call records into its `calls/` child directory regardless
of the configured `state_dir` inside the phone TOML. It clears inherited phone
and LiveKit configuration and uses only this namespace's settings and secrets.

```sh
switchboard phone.doctor --ns phone.personal --json
switchboard phone.call.run --ns phone.personal --draft \
  --destination +12125550100 --caller-name Example \
  --task 'Ask for opening hours' --max-duration-seconds 300
```

The example number is a fictional placeholder. Review the exact destination,
brief, and duration in the stored operation. Then, only when ready to dial:

```sh
switchboard op approve <operation-id> --apply
```

Calls require explicit operation approval even if the general write policy is
`allow`. There is no raw phone passthrough. The stored operation ID becomes the
call ID, and an existing call record prevents dialing that ID again, including
after a crash. An intentional second attempt needs a new reviewed operation.
Calls cannot be undone.

Switchboard executes the binary on `PATH`, or `SWITCHBOARD_PHONE_BIN`. A custom
wrapper must `exec` the actual binary so parent-process supervision can detect
Switchboard exiting. Phone integration currently targets macOS and Linux.

## Transcripts and outcomes

```sh
switchboard phone.transcripts.list --ns phone.personal --json
phone --config /absolute/path/to/namespace/config.toml transcripts show <call-id>
```

For the second command, inject `PHONE_TRANSCRIPT_IDENTITY` from 1Password and
set `PHONE_STATE_DIR` to the namespace's absolute `calls/` directory if the
TOML points elsewhere. Decryption writes JSON to stdout, so do not redirect it
to an unencrypted file or include it in logs. Transcript decryption is a
standalone CLI action; Switchboard's call result stores metadata only.

Each journal record is independently encrypted with age, finalized, synced,
and published atomically. The directory is private and files are owner-only.
Already-published records remain decryptable after an interrupted write. The
journal includes the intent, speaker-labelled transcript segments, lifecycle,
and outcome. No raw audio is retained. Transcript text and interruption
alignment are machine-generated and can contain errors.

Results contain `call_id`, `status`, `transcript_path`, and
`remote_hangup_confirmed`. A `completed` status means the conversation ended,
not that the task's answer was verified. Inspect the transcript for the result.
An unconfirmed remote hangup stays explicitly unknown. The CLI cancels on
interrupt, supervisor exit, deadline, or transcript storage failure, then
allows bounded cleanup. A server-side call-duration cap remains the fallback
if the machine or connection disappears. No automatic redial occurs.

The general Switchboard operation database still contains the approved brief,
destination, and audit metadata in its existing storage format. Only the phone
journal is encrypted by this feature. Keep its state directory private and
provide the minimum personal information the call requires.

LiveKit session recording is disabled in code. Disable project observability
before live use too, and confirm current retention settings for LiveKit,
inference providers, and the SIP carrier. Local encrypted notes do not prevent
those services from processing call content.

## Development

```sh
cargo test -p phone-cli
cargo test -p switchboard-providers phone::tests
cd workers/livekit-phone
uv run --locked --no-sync ruff check .
uv run --locked --no-sync ruff format --check .
uv run --locked --no-sync mypy src tests
uv run --locked --no-sync pytest -q
```

Offline checks use the actual SDK types and local process protocol. They do
not dial. The worker's pinned dependencies and wire protocol are documented
in its README; changes must keep the Rust adapter and Python consumer aligned.
