"""The evaluator must measure played audio, preserving gaps and interruptions."""

from __future__ import annotations

import asyncio
import struct
import wave
from dataclasses import asdict
from pathlib import Path

import pytest
from livekit import rtc

from evals.diagnostics import Diagnostics
from evals.media import SAMPLE_RATE, PacedOutput, Timeline
from evals.metrics import measure_window
from evals.run import private_output


def test_diagnostics_keeps_numeric_timing_without_provider_content() -> None:
    diagnostics = Diagnostics(lambda: 12.5)
    diagnostics.on_provider_event(
        {"type": "session.started", "session": {"id": "private-session-id"}}
    )
    diagnostics.on_provider_event(
        {
            "type": "session.input_transcript.delta",
            "delta": "private-conversation-text",
            "event_id": "private-event-id",
        }
    )
    assert diagnostics.report.first_input_transcript_at == 12.5
    assert diagnostics.report.session_ready_at == 12.5
    assert "private-" not in str(asdict(diagnostics.report))


def test_stereo_recording_preserves_channels_and_silence(tmp_path: Path) -> None:
    timeline = Timeline()
    timeline.recipient = [(0.02, struct.pack("<h", 5000) * 480)]
    timeline.agent = [(0.04, struct.pack("<h", -6000) * 480)]
    path = tmp_path / "conversation.wav"
    timeline.write_wav(path)
    with wave.open(str(path), "rb") as audio:
        assert (audio.getnchannels(), audio.getframerate()) == (2, SAMPLE_RATE)
        pcm = struct.unpack(
            "<" + "h" * (audio.getnframes() * 2), audio.readframes(audio.getnframes())
        )
    assert set(pcm[:960]) == {0}
    assert set(pcm[960:1920:2]) == {5000}
    assert set(pcm[961:1920:2]) == {0}
    assert set(pcm[1920::2]) == {0}
    assert set(pcm[1921::2]) == {-6000}
    assert timeline.activity("recipient") == [(0.02, 0.04)]
    assert timeline.activity("agent") == [(0.04, 0.06)]


def test_interrupted_output_does_not_record_unplayed_tail() -> None:
    async def exercise() -> None:
        timeline = Timeline()
        output = PacedOutput(timeline)
        frame = rtc.AudioFrame(
            data=struct.pack("<h", 5000) * SAMPLE_RATE,
            sample_rate=SAMPLE_RATE,
            num_channels=1,
            samples_per_channel=SAMPLE_RATE,
        )
        playback = asyncio.create_task(output.capture_frame(frame))
        await asyncio.wait_for(output.first_voice.wait(), timeout=2)
        output.clear_buffer()
        await playback
        await asyncio.sleep(0)
        recorded = sum(len(pcm) // 2 for _, pcm in timeline.agent)
        assert 0 < recorded < SAMPLE_RATE
        assert output.last_voice_end <= timeline.elapsed()
        # A cleared segment must not poison subsequent output.
        short = rtc.AudioFrame(
            data=struct.pack("<h", 6000) * 480,
            sample_rate=SAMPLE_RATE,
            num_channels=1,
            samples_per_channel=480,
        )
        await output.capture_frame(short)
        output.flush()
        await asyncio.sleep(0)
        assert sum(len(pcm) // 2 for _, pcm in timeline.agent) == recorded + 480
        await output.aclose()

    asyncio.run(exercise())


def test_metrics_separate_silence_overlap_and_response() -> None:
    metric = measure_window(
        "question", (1, 4), [(1, 2), (3, 4)], [(1.5, 2.5), (3.5, 3.8), (4.4, 5)]
    )
    assert metric.agent_audio_seconds == 1.3
    assert metric.simultaneous_audio_seconds == 0.8
    assert metric.longest_overlap_seconds == 0.5
    assert metric.response_delay_seconds == 0.4


def test_response_already_playing_at_recipient_end_has_no_delay() -> None:
    metric = measure_window("question", (0, 1), [(0, 1)], [(0.5, 2), (4, 5)])
    assert metric.response_delay_seconds == 0
    assert metric.simultaneous_audio_seconds == 0.5


@pytest.mark.parametrize("worktree", [False, True])
def test_output_rejects_git_checkout_and_symlink(
    tmp_path: Path, worktree: bool
) -> None:
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    if worktree:
        (checkout / ".git").write_text("gitdir: /unused")
    else:
        (checkout / ".git").mkdir()
    alias = tmp_path / "alias"
    alias.symlink_to(checkout, target_is_directory=True)
    for root in (checkout, alias):
        with pytest.raises(ValueError, match="outside Git"):
            private_output(root / "nested" / "recordings")
    assert not (checkout / "nested").exists()
    output = private_output(tmp_path / "private")
    assert output.stat().st_mode & 0o777 == 0o700
