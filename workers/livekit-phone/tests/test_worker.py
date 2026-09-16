"""Offline contract tests using the real SDK's message types and the real CLI."""

from __future__ import annotations

import asyncio
import io
import json
import os
import subprocess
import sys

import pytest
from livekit.agents import ConversationItemAddedEvent
from livekit.agents.llm import ChatMessage

from livekit_phone.config import Config, ConfigurationError
from livekit_phone.control import Stop, transcript_event
from livekit_phone.protocol import (
    MAX_COMMAND_BYTES,
    Completed,
    EventSink,
    Start,
    parse_command,
)


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
    )
    assert "private-key-sentinel" not in repr(config)
    assert "private-secret-sentinel" not in repr(config)
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


def test_events_are_valid_ndjson_without_null_summary() -> None:
    output = io.StringIO()
    EventSink(output).emit(Completed(reason="cancelled", remote_hangup_confirmed=True))
    assert json.loads(output.getvalue()) == {
        "protocol_version": 1,
        "type": "completed",
        "reason": "cancelled",
        "remote_hangup_confirmed": True,
    }


def worker(arguments: list[str], input_text: str) -> subprocess.CompletedProcess[str]:
    # Remove ambient credentials so no test can accidentally reach a project.
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("LIVEKIT_")
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


def test_offline_check_constructs_actual_livekit_inference_classes() -> None:
    result = worker(["check"], "")
    assert result.returncode == 0, result.stderr
    event = json.loads(result.stdout)
    assert event["reason"] == "completed"
    assert "Offline SDK check passed" in event["summary"]
