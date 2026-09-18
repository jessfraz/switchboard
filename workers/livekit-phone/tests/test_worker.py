"""Offline contract tests using the real SDK's message types and the real CLI."""

from __future__ import annotations

import asyncio
import io
import json
import os
import subprocess
import sys

import aiohttp
import pytest
from livekit.agents import (
    APIStatusError,
    CloseEvent,
    ConversationItemAddedEvent,
    ErrorEvent,
    SpeechCreatedEvent,
    UserInputTranscribedEvent,
    UserStateChangedEvent,
)
from livekit.agents.llm import ChatMessage, DuplexRealtimeAdapter
from livekit.agents.llm.realtime import RealtimeModelError
from livekit.agents.voice.events import CloseReason
from livekit.agents.voice.speech_handle import SpeechHandle
from livekit.plugins.openai.realtime import GPTLiveModel, ResponsesDelegationOptions
from openai.types.shared_params import Reasoning

from livekit_phone.config import (
    BackendReasoningEffort,
    Config,
    ConfigurationError,
    VoiceEngine,
)
from livekit_phone.control import Stop, transcript_event
from livekit_phone.models import create_models, wait_for_voice_ready
from livekit_phone.protocol import (
    MAX_COMMAND_BYTES,
    Completed,
    CompletionReason,
    EventSink,
    Start,
    parse_command,
)
from livekit_phone.runtime import Call


def start_request() -> Start:
    return Start(
        call_id="test-call",
        destination="+12025550100",
        task="Ask for the store's opening hours.",
        caller_name="Caller",
        max_duration_seconds=600,
    )


def test_protocol_round_trip_and_rejects_unknown_fields() -> None:
    request = start_request()
    assert parse_command(request.model_dump_json().encode()) == request
    with pytest.raises(ValueError):
        parse_command(b'{"protocol_version":1,"type":"cancel","execute":"secret"}')
    with pytest.raises(ValueError):
        parse_command(b'{"protocol_version":2,"type":"cancel"}')


@pytest.mark.parametrize("number", ["911", "+0123456789", "+12025550100;rm", "*123"])
def test_invalid_destinations_never_reach_the_network(number: str) -> None:
    with pytest.raises(ValueError):
        Start.model_validate({**start_request().model_dump(), "destination": number})


def test_byte_limits_and_duration_are_enforced() -> None:
    with pytest.raises(ValueError):
        parse_command(b" " * (MAX_COMMAND_BYTES + 1))
    with pytest.raises(ValueError):
        Start.model_validate({**start_request().model_dump(), "task": "界" * 3_000})
    with pytest.raises(ValueError):
        Start.model_validate(
            {**start_request().model_dump(), "max_duration_seconds": True}
        )
    with pytest.raises(ValueError):
        Start.model_validate(
            {**start_request().model_dump(), "max_duration_seconds": 3_601}
        )


def test_maximum_escaped_task_fits_the_protocol_envelope() -> None:
    request = Start.model_validate(
        {**start_request().model_dump(), "task": "\x01" * 8_000}
    )
    encoded = request.model_dump_json().encode("utf-8")
    assert len(encoded) > 32_768
    assert parse_command(encoded) == request


def test_interrupted_transcript_uses_the_committed_sdk_item() -> None:
    event = ConversationItemAddedEvent(
        item=ChatMessage(
            role="assistant",
            content=["We have confirmed"],
            interrupted=True,
            created_at=1_750_000_000.125,
        )
    )
    transcript = transcript_event(event)
    assert transcript is not None
    assert transcript.text == "We have confirmed"
    assert transcript.interrupted
    assert transcript.speaker == "agent"
    assert transcript.timestamp_ms == 1_750_000_000_125


def test_recipient_transcript_and_nonspoken_items() -> None:
    event = ConversationItemAddedEvent(
        item=ChatMessage(role="user", content=["We close at five."], created_at=12.5)
    )
    transcript = transcript_event(event)
    assert transcript is not None and transcript.speaker == "recipient"
    assert not transcript.interrupted
    assert (
        transcript_event(
            ConversationItemAddedEvent(
                item=ChatMessage(role="system", content=["private"])
            )
        )
        is None
    )
    assert (
        transcript_event(
            ConversationItemAddedEvent(item=ChatMessage(role="assistant", content=[]))
        )
        is None
    )


def test_cancellation_wins_over_a_later_hangup_notification() -> None:
    async def race() -> None:
        stop = Stop()
        stop.request("cancelled")
        stop.request("completed", "This result must not replace cancellation.")
        await stop.event.wait()
        assert stop.reason == "cancelled"
        assert stop.summary is None

    asyncio.run(race())


def test_credentials_are_not_in_config_representation() -> None:
    config = Config(
        url="wss://test.livekit.cloud",
        api_key="private-key-sentinel",
        api_secret="private-secret-sentinel",
        trunk_id="trunk",
        voice_engine=VoiceEngine.GPT_LIVE,
        openai_api_key="private-openai-sentinel",
    )
    assert "private-key-sentinel" not in repr(config)
    assert "private-secret-sentinel" not in repr(config)
    assert "private-openai-sentinel" not in repr(config)
    with pytest.raises(ConfigurationError):
        Config.from_env({})
    with pytest.raises(ConfigurationError):
        Config.from_env(
            {
                "LIVEKIT_URL": "wss://example.com",
                "LIVEKIT_API_KEY": "key",
                "LIVEKIT_API_SECRET": "secret",
                "LIVEKIT_SIP_TRUNK_ID": "trunk",
            }
        )


def test_gpt_live_configuration_requires_openai_credentials() -> None:
    env = {
        "LIVEKIT_URL": "wss://test.livekit.cloud",
        "LIVEKIT_API_KEY": "key",
        "LIVEKIT_API_SECRET": "secret",
        "LIVEKIT_SIP_TRUNK_ID": "trunk",
        "OPENAI_API_KEY": "private-openai-sentinel",
    }
    live = Config.from_env(env)
    assert live.voice_engine == VoiceEngine.GPT_LIVE
    assert live.openai_api_key == env["OPENAI_API_KEY"]
    assert live.voice == "marin"
    assert live.backend_model == "gpt-5.6-luna"
    assert live.backend_reasoning_effort is None
    env["LIVEKIT_PHONE_VOICE_ENGINE"] = "gpt_live"
    assert Config.from_env(env) == live
    env["LIVEKIT_PHONE_VOICE"] = "stone"
    env["LIVEKIT_PHONE_REALTIME_MODEL"] = "voice-model"
    env["LIVEKIT_PHONE_BACKEND_MODEL"] = "backend-model"
    env["LIVEKIT_PHONE_BACKEND_REASONING_EFFORT"] = "xhigh"
    changed = Config.from_env(env)
    assert changed.voice == "stone"
    assert changed.realtime_model == "voice-model"
    assert changed.backend_model == "backend-model"
    assert changed.backend_reasoning_effort == BackendReasoningEffort.XHIGH
    for unsupported in ("ultra", "private-secret-sentinel", ""):
        with pytest.raises(
            ConfigurationError, match="^Unsupported backend reasoning effort\\.$"
        ):
            Config.from_env(
                {**env, "LIVEKIT_PHONE_BACKEND_REASONING_EFFORT": unsupported}
            )
    for engine in (None, "gpt_live"):
        missing_key = {
            key: value for key, value in env.items() if key != "OPENAI_API_KEY"
        }
        if engine is None:
            del missing_key["LIVEKIT_PHONE_VOICE_ENGINE"]
        with pytest.raises(ConfigurationError, match="requires an OpenAI API key"):
            Config.from_env(missing_key)
    for unsupported in ("pipeline", "private-secret-sentinel", ""):
        with pytest.raises(
            ConfigurationError, match="^Unsupported phone voice engine\\.$"
        ):
            Config.from_env({**env, "LIVEKIT_PHONE_VOICE_ENGINE": unsupported})


@pytest.mark.parametrize("effort", [None, BackendReasoningEffort.XHIGH])
@pytest.mark.parametrize("backend_model", ["gpt-6-astra", "gpt-5.6-luna"])
def test_real_sdk_models_use_openai_and_close_owned_clients(
    effort: BackendReasoningEffort | None,
    backend_model: str,
) -> None:
    async def construct() -> None:
        config = Config(
            url="wss://test.livekit.cloud",
            api_key="key",
            api_secret="offline-not-a-secret-at-least-32-characters",
            trunk_id="trunk",
            openai_api_key="offline-not-a-real-key",
            backend_model=backend_model,
            backend_reasoning_effort=effort,
        )
        async with aiohttp.ClientSession() as http_session:
            models = create_models(
                config, http_session, backend_instructions="Offline construction only."
            )
            model = models.session.llm
            try:
                assert isinstance(model, DuplexRealtimeAdapter)
                assert model.model == config.realtime_model
                assert model.provider == "api.openai.com"
                assert models.session.stt is None
                assert models.session.tts is None
                assert models.session.turn_detection == "realtime_llm"
                realtime_model = model.duplex_model
                assert isinstance(realtime_model, GPTLiveModel)
                expected = ResponsesDelegationOptions(
                    model=backend_model,
                    instructions="Offline construction only.",
                )
                if effort is not None:
                    expected["reasoning"] = Reasoning(effort=effort.value)
                assert realtime_model._opts.responses == expected
            finally:
                await models.session.aclose()
                await models.aclose()
            # Closing the models must not steal the shared HTTP context.
            assert not http_session.closed

    asyncio.run(construct())


@pytest.mark.parametrize(
    "outcome", ["already_ready", "acknowledged", "timeout", "cancelled"]
)
def test_voice_readiness_owns_only_its_pending_listener(outcome: str) -> None:
    async def wait() -> None:
        async with aiohttp.ClientSession() as http_session:
            model = GPTLiveModel(
                api_key="offline-not-a-real-key", http_session=http_session
            )
            duplex = model.session()
            # Cancel before yielding: the SDK starts connecting immediately.
            # Its real event emitter and decoder remain usable offline.
            duplex._main_atask.cancel()
            unrelated_events: list[object] = []
            duplex.on("openai_server_event_received", unrelated_events.append)
            started = {"type": "session.started", "session": {"id": "offline-ready"}}
            try:
                if outcome == "already_ready":
                    duplex._handle_event(started)
                    assert duplex.session_id == "offline-ready"
                    async with asyncio.timeout(0.05):
                        await wait_for_voice_ready(duplex)
                else:
                    waiting = asyncio.create_task(wait_for_voice_ready(duplex))
                    try:
                        await asyncio.sleep(0)
                        assert not waiting.done()
                        assert len(duplex._events["openai_server_event_received"]) == 2
                        duplex.emit(
                            "openai_server_event_received", {"type": "session.updated"}
                        )
                        await asyncio.sleep(0)
                        assert not waiting.done()
                        if outcome == "acknowledged":
                            # Match the SDK receive loop: raw event first, then
                            # its decoder publishes the session's public ID.
                            duplex.emit("openai_server_event_received", started)
                            duplex._handle_event(started)
                            await waiting
                        elif outcome == "timeout":
                            with pytest.raises(TimeoutError):
                                async with asyncio.timeout(0.05):
                                    await waiting
                        else:
                            waiting.cancel()
                            with pytest.raises(asyncio.CancelledError):
                                await waiting
                    finally:
                        waiting.cancel()
                        await asyncio.gather(waiting, return_exceptions=True)
                assert duplex._events["openai_server_event_received"] == {
                    unrelated_events.append
                }
            finally:
                duplex.off("openai_server_event_received", unrelated_events.append)
                await duplex.aclose()
                await model.aclose()
            assert not http_session.closed

    asyncio.run(wait())


@pytest.mark.parametrize(
    "event_kind", ["transcript", "speaking", "pending", "committed"]
)
def test_native_conversation_suppresses_silent_line_prompt(event_kind: str) -> None:
    async def open_conversation() -> None:
        config = Config(
            url="wss://test.livekit.cloud",
            api_key="key",
            api_secret="offline-not-a-secret-at-least-32-characters",
            trunk_id="trunk",
            openai_api_key="offline-not-a-real-key",
        )
        output = io.StringIO()
        async with aiohttp.ClientSession() as http_session:
            call = Call(
                start_request(), config, Stop(), EventSink(output), http_session
            )
            opening = asyncio.create_task(call._open_conversation())
            try:
                await asyncio.sleep(0)
                assert not opening.done()
                # The real, unstarted SDK rejects generate_reply. Any initial
                # native activity must suppress that extra conversation turn.
                if event_kind == "transcript":
                    call.session.emit(
                        "user_input_transcribed",
                        UserInputTranscribedEvent(transcript="Hello", is_final=False),
                    )
                elif event_kind == "speaking":
                    call.session.emit(
                        "user_state_changed",
                        UserStateChangedEvent(
                            old_state="listening", new_state="speaking"
                        ),
                    )
                elif event_kind == "pending":
                    call.session.emit(
                        "speech_created",
                        SpeechCreatedEvent(
                            speech_handle=SpeechHandle.create(),
                            user_initiated=False,
                            source="generate_reply",
                        ),
                    )
                else:
                    call.session.emit(
                        "conversation_item_added",
                        ConversationItemAddedEvent(
                            item=ChatMessage(
                                role="assistant",
                                content=["Hi, I'm Caller's assistant."],
                            )
                        ),
                    )
                await asyncio.wait_for(opening, timeout=0.25)
                assert call.first_agent_utterance.is_set() == (
                    event_kind == "committed"
                )
            finally:
                opening.cancel()
                await asyncio.gather(opening, return_exceptions=True)
                assert await call.cleanup()

    asyncio.run(open_conversation())


@pytest.mark.parametrize(
    ("message", "cancelled", "expected_reason"),
    [
        (
            ChatMessage(role="assistant", content=["Oh, sorry, voicemail. Bye."]),
            False,
            "completed",
        ),
        (
            ChatMessage(role="assistant", content=[" OH! Sorry,  voicemail.\nBYE! "]),
            False,
            "completed",
        ),
        (
            ChatMessage(role="user", content=["Oh, sorry, voicemail. Bye."]),
            False,
            None,
        ),
        (
            ChatMessage(
                role="assistant",
                content=["Oh, sorry, voicemail. Bye."],
                interrupted=True,
            ),
            False,
            None,
        ),
        (
            ChatMessage(role="assistant", content=['"Oh, sorry, voicemail. Bye."']),
            False,
            None,
        ),
        (
            ChatMessage(
                role="assistant",
                content=["Oh, sorry, voicemail. Bye. Actually, hello again."],
            ),
            False,
            None,
        ),
        (
            ChatMessage(
                role="assistant", content=["Sorry, I thought it was voicemail."]
            ),
            False,
            None,
        ),
        (
            ChatMessage(role="assistant", content=["Oh, sorry, voicemail. Bye."]),
            True,
            "cancelled",
        ),
    ],
)
def test_only_delivered_voicemail_signoff_ends_call(
    message: ChatMessage,
    cancelled: bool,
    expected_reason: CompletionReason | None,
) -> None:
    async def deliver() -> None:
        config = Config(
            url="wss://test.livekit.cloud",
            api_key="key",
            api_secret="offline-not-a-secret-at-least-32-characters",
            trunk_id="trunk",
            openai_api_key="offline-not-a-real-key",
        )
        stop = Stop()
        if cancelled:
            stop.request("cancelled")
        async with aiohttp.ClientSession() as http_session:
            call = Call(
                start_request(), config, stop, EventSink(io.StringIO()), http_session
            )
            try:
                # This is the production SDK event path after audio playout.
                # It must not depend on a second model deciding to invoke a tool.
                call.session.emit(
                    "conversation_item_added", ConversationItemAddedEvent(item=message)
                )
                assert stop.event.is_set() == (expected_reason is not None)
                if expected_reason is not None:
                    assert stop.reason == expected_reason
                if expected_reason == "completed":
                    assert stop.summary == (
                        "No person was reached; voicemail was recognized "
                        "and the call ended."
                    )
                else:
                    assert stop.summary is None
            finally:
                assert await call.cleanup()

    asyncio.run(deliver())


def test_events_are_valid_ndjson_without_null_summary() -> None:
    output = io.StringIO()
    EventSink(output).emit(Completed(reason="cancelled", remote_hangup_confirmed=True))
    assert json.loads(output.getvalue()) == {
        "protocol_version": 1,
        "type": "completed",
        "reason": "cancelled",
        "remote_hangup_confirmed": True,
    }


@pytest.mark.parametrize("recoverable", [False, True])
@pytest.mark.parametrize("status_code", [None, 400])
def test_voice_errors_allow_sdk_recovery_until_the_session_closes(
    recoverable: bool,
    status_code: int | None,
) -> None:
    async def report_error() -> None:
        config = Config(
            url="wss://test.livekit.cloud",
            api_key="key",
            api_secret="offline-not-a-secret-at-least-32-characters",
            trunk_id="trunk",
            openai_api_key="offline-not-a-real-key",
        )
        output = io.StringIO()
        stop = Stop()
        async with aiohttp.ClientSession() as http_session:
            call = Call(start_request(), config, stop, EventSink(output), http_session)
            try:
                # Use the real registered session event path. SDK/provider error
                # payloads must never be copied into our transcript protocol.
                error = RealtimeModelError(
                    timestamp=0.0,
                    label="private-model-sentinel",
                    error=(
                        RuntimeError("private-credential-sentinel")
                        if status_code is None
                        else APIStatusError(
                            "private-credential-sentinel",
                            status_code=status_code,
                            request_id="private-request-sentinel",
                            body="private-body-sentinel",
                        )
                    ),
                    recoverable=recoverable,
                )
                call.session.emit(
                    "error",
                    ErrorEvent(source=call.session.llm, error=error),
                )
                assert not stop.event.is_set()
                assert output.getvalue() == ""
                if not recoverable:
                    call.session.emit(
                        "close", CloseEvent(error=error, reason=CloseReason.ERROR)
                    )
                    detail = (
                        "realtime_model_error"
                        if status_code is None
                        else f"realtime_model_error, HTTP {status_code}"
                    )
                    summary = f"The voice session ended ({detail})."
                    assert json.loads(output.getvalue()) == {
                        "protocol_version": 1,
                        "type": "error",
                        "code": "realtime_model_error",
                        "message": summary,
                    }
                    assert stop.event.is_set()
                    assert stop.reason == "failed"
                    assert stop.summary == summary
            finally:
                assert await call.cleanup()

    asyncio.run(report_error())


def worker(arguments: list[str], input_text: str) -> subprocess.CompletedProcess[str]:
    # Remove ambient credentials so no test can accidentally reach a project.
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("LIVEKIT_", "OPENAI_", "PHONE_"))
    }
    return subprocess.run(
        [sys.executable, "-m", "livekit_phone", *arguments],
        input=input_text,
        capture_output=True,
        text=True,
        env=env,
        timeout=30,
        check=False,
    )


def test_eof_before_start_exits_without_remote_state() -> None:
    result = worker([], "")
    assert result.returncode == 0, result.stderr
    events = [json.loads(line) for line in result.stdout.splitlines()]
    assert events == [
        {"protocol_version": 1, "type": "ready"},
        {
            "protocol_version": 1,
            "type": "completed",
            "reason": "cancelled",
            "remote_hangup_confirmed": True,
        },
    ]


def test_invalid_input_is_sanitized_in_real_process() -> None:
    result = worker([], "private-secret-sentinel\n")
    assert result.returncode == 1
    assert "private-secret-sentinel" not in result.stdout + result.stderr
    events = [json.loads(line) for line in result.stdout.splitlines()]
    assert events[-1]["reason"] == "failed"
    assert events[-1]["remote_hangup_confirmed"]
    assert events[1]["code"] == "invalid_command"


def test_offline_check_constructs_actual_gpt_live_models() -> None:
    result = worker(["check"], "")
    assert result.returncode == 0, result.stderr
    event = json.loads(result.stdout)
    assert event["reason"] == "completed"
    assert "Offline SDK check passed" in event["summary"]
