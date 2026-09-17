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
from livekit.agents import CloseEvent, ConversationItemAddedEvent, ErrorEvent, inference
from livekit.agents.llm import ChatMessage, DuplexRealtimeAdapter
from livekit.agents.tts import TTSError
from livekit.agents.voice.events import CloseReason
from livekit.plugins.openai.realtime import GPTLiveModel, ResponsesDelegationOptions
from openai.types.shared_params import Reasoning

from livekit_phone.config import (
    BackendReasoningEffort,
    Config,
    ConfigurationError,
    VoiceEngine,
)
from livekit_phone.control import Stop, transcript_event
from livekit_phone.models import create_models
from livekit_phone.protocol import (
    MAX_COMMAND_BYTES,
    Completed,
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


def test_engine_selection_requires_only_its_own_credentials() -> None:
    env = {
        "LIVEKIT_URL": "wss://test.livekit.cloud",
        "LIVEKIT_API_KEY": "key",
        "LIVEKIT_API_SECRET": "secret",
        "LIVEKIT_SIP_TRUNK_ID": "trunk",
        "OPENAI_API_KEY": "private-openai-sentinel",
    }
    pipeline = Config.from_env(env)
    assert pipeline.voice_engine == VoiceEngine.PIPELINE
    assert pipeline.openai_api_key is None
    assert pipeline.selected_voice == "Ashley"
    env["LIVEKIT_PHONE_VOICE_ENGINE"] = "gpt_live"
    live = Config.from_env(env)
    assert live.voice_engine == VoiceEngine.GPT_LIVE
    assert live.openai_api_key == env["OPENAI_API_KEY"]
    assert live.selected_voice == "marin"
    assert live.backend_model == "gpt-5.6-luna"
    assert live.backend_reasoning_effort is None
    env["LIVEKIT_PHONE_VOICE"] = "stone"
    env["LIVEKIT_PHONE_REALTIME_MODEL"] = "voice-model"
    env["LIVEKIT_PHONE_BACKEND_MODEL"] = "backend-model"
    env["LIVEKIT_PHONE_BACKEND_REASONING_EFFORT"] = "xhigh"
    changed = Config.from_env(env)
    assert changed.selected_voice == "stone"
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
    del env["OPENAI_API_KEY"]
    with pytest.raises(ConfigurationError, match="requires an OpenAI API key"):
        Config.from_env(env)
    env["LIVEKIT_PHONE_VOICE_ENGINE"] = "private-secret-sentinel"
    with pytest.raises(ConfigurationError, match="^Unsupported phone voice engine\\.$"):
        Config.from_env(env)


@pytest.mark.parametrize(
    ("engine", "effort"),
    [
        (VoiceEngine.PIPELINE, None),
        (VoiceEngine.GPT_LIVE, None),
        (VoiceEngine.GPT_LIVE, BackendReasoningEffort.XHIGH),
    ],
)
def test_real_sdk_models_select_the_engine_and_close_owned_clients(
    engine: VoiceEngine,
    effort: BackendReasoningEffort | None,
) -> None:
    async def construct() -> None:
        config = Config(
            url="wss://test.livekit.cloud",
            api_key="key",
            api_secret="offline-not-a-secret-at-least-32-characters",
            trunk_id="trunk",
            voice_engine=engine,
            openai_api_key="offline-not-a-real-key",
            backend_model="gpt-6-astra",
            backend_reasoning_effort=effort,
        )
        async with aiohttp.ClientSession() as http_session:
            models = create_models(
                config, http_session, backend_instructions="Offline construction only."
            )
            model = models.session.llm
            try:
                if engine == VoiceEngine.GPT_LIVE:
                    assert isinstance(model, DuplexRealtimeAdapter)
                    assert model.model == config.realtime_model
                    assert model.provider == "api.openai.com"
                    assert models.session.stt is None
                    assert models.session.tts is None
                    realtime_model = model.duplex_model
                    assert isinstance(realtime_model, GPTLiveModel)
                    expected = ResponsesDelegationOptions(
                        model="gpt-6-astra",
                        instructions="Offline construction only.",
                    )
                    if effort is not None:
                        expected["reasoning"] = Reasoning(effort=effort.value)
                    assert realtime_model._opts.responses == expected
                else:
                    assert isinstance(model, inference.LLM)
                    assert model is models.classifier
                    assert model.model == config.llm_model
                    assert models.session.stt is not None
                    assert models.session.tts is not None
            finally:
                await models.session.aclose()
                await models.aclose()
            # Closing the models must not steal the shared HTTP context.
            assert not http_session.closed
            assert models.classifier._client.is_closed()

    asyncio.run(construct())


@pytest.mark.parametrize("opening_precedes_detection", [False, True])
def test_gpt_live_opening_observes_native_speech_without_starting_another_reply(
    opening_precedes_detection: bool,
) -> None:
    async def open_conversation() -> None:
        config = Config(
            url="wss://test.livekit.cloud",
            api_key="key",
            api_secret="offline-not-a-secret-at-least-32-characters",
            trunk_id="trunk",
            voice_engine=VoiceEngine.GPT_LIVE,
            openai_api_key="offline-not-a-real-key",
        )
        output = io.StringIO()
        async with aiohttp.ClientSession() as http_session:
            call = Call(
                start_request(), config, Stop(), EventSink(output), http_session
            )
            recipient = ConversationItemAddedEvent(
                item=ChatMessage(role="user", content=["Hello."])
            )
            greeting = ConversationItemAddedEvent(
                item=ChatMessage(
                    role="assistant",
                    content=[
                        "Hi, I'm Caller's AI assistant, calling on their behalf. "
                        "I'll transcribe this call for notes."
                    ],
                )
            )
            # Drive the actual registered SDK event path, preserving the first
            # recipient utterance. No model or provider connection is started.
            call.session.emit("conversation_item_added", recipient)
            assert not call.first_agent_utterance.is_set()
            try:
                if opening_precedes_detection:
                    call.session.emit("conversation_item_added", greeting)
                    await call._open_conversation()
                else:
                    opening = asyncio.create_task(call._open_conversation())
                    await asyncio.sleep(0)
                    assert not opening.done()
                    call.session.emit("conversation_item_added", greeting)
                    await opening
                assert call.first_agent_utterance.is_set()
                # This real, unstarted session rejects say/generate_reply. The
                # production opening path must observe speech without either.
                assert [
                    json.loads(line)["speaker"]
                    for line in output.getvalue().splitlines()
                ] == [
                    "recipient",
                    "agent",
                ]
            finally:
                assert await call.cleanup()

    asyncio.run(open_conversation())


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
def test_voice_errors_allow_sdk_recovery_until_the_session_closes(
    recoverable: bool,
) -> None:
    async def report_error() -> None:
        config = Config(
            url="wss://test.livekit.cloud",
            api_key="key",
            api_secret="offline-not-a-secret-at-least-32-characters",
            trunk_id="trunk",
        )
        output = io.StringIO()
        stop = Stop()
        async with aiohttp.ClientSession() as http_session:
            call = Call(start_request(), config, stop, EventSink(output), http_session)
            try:
                # Use the real registered session event path. SDK/provider error
                # payloads must never be copied into our transcript protocol.
                error = TTSError(
                    timestamp=0.0,
                    label="private-model-sentinel",
                    error=RuntimeError("private-credential-sentinel"),
                    recoverable=recoverable,
                )
                call.session.emit(
                    "error",
                    ErrorEvent(source=call.session.tts, error=error),
                )
                assert not stop.event.is_set()
                assert output.getvalue() == ""
                if not recoverable:
                    call.session.emit(
                        "close", CloseEvent(error=error, reason=CloseReason.ERROR)
                    )
                    assert json.loads(output.getvalue()) == {
                        "protocol_version": 1,
                        "type": "error",
                        "code": "tts_error",
                        "message": "The voice session ended after model errors.",
                    }
                    assert stop.event.is_set()
                    assert stop.reason == "failed"
                    assert stop.summary == "The voice session ended (tts_error)."
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


def test_offline_check_constructs_both_actual_voice_engines() -> None:
    result = worker(["check"], "")
    assert result.returncode == 0, result.stderr
    event = json.loads(result.stdout)
    assert event["reason"] == "completed"
    assert "Offline SDK check passed" in event["summary"]
