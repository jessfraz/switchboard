"""Greeting freshness regressions using real SDK input events and owned state."""

from __future__ import annotations

import asyncio
import struct
from collections.abc import AsyncIterator

from livekit import rtc
from livekit.agents import AMD, Agent, AgentSession, UserInputTranscribedEvent
from livekit.plugins.openai.responses import LLM as ResponsesLLM

from evals.media import SAMPLE_RATE, PacedOutput, Timeline
from livekit_phone.greeting import Greeting, silence_greeting


def test_new_human_greeting_invalidates_pending_machine_verdict() -> None:
    greeting = Greeting()
    greeting.update_transcript(
        UserInputTranscribedEvent(
            item_id="announcement",
            transcript="Please leave a message after the tone.",
            is_final=True,
        )
    )
    pending_verdict = greeting.snapshot()
    assert greeting.is_current(pending_verdict)

    greeting.update_transcript(
        UserInputTranscribedEvent(
            item_id="person", transcript="Hello, this is Pat.", is_final=False
        )
    )
    assert not greeting.is_current(pending_verdict)
    current = greeting.snapshot()
    assert current.transcript == (
        "Please leave a message after the tone.\nHello, this is Pat."
    )
    assert greeting.is_current(current)


def test_partial_and_final_transcripts_replace_instead_of_repeating() -> None:
    greeting = Greeting()
    for text, final in (
        ("Please leave", False),
        ("Please leave a message.", False),
        ("Please leave a message.", True),
    ):
        greeting.update_transcript(
            UserInputTranscribedEvent(
                item_id="announcement", transcript=text, is_final=final
            )
        )
    greeting.update_transcript(
        UserInputTranscribedEvent(item_id="person", transcript="Hello?", is_final=True)
    )
    assert greeting.snapshot().transcript == "Please leave a message.\nHello?"


def test_speech_before_its_transcript_invalidates_a_pending_verdict() -> None:
    greeting = Greeting()
    pending_verdict = greeting.snapshot("Earlier mailbox announcement.")
    greeting.update_speaking(True)
    assert not greeting.is_current(pending_verdict)
    assert not greeting.is_current(greeting.snapshot())
    greeting.update_speaking(False)
    assert not greeting.is_current(pending_verdict)
    assert greeting.is_current(greeting.snapshot())


def test_fallback_is_used_only_until_greeting_input_is_available() -> None:
    greeting = Greeting()
    assert greeting.snapshot("Earlier greeting.").transcript == "Earlier greeting."
    greeting.update_transcript(
        UserInputTranscribedEvent(transcript="Hello", is_final=False)
    )
    greeting.update_transcript(
        UserInputTranscribedEvent(transcript="Hello, this is Pat.", is_final=True)
    )
    greeting.update_transcript(
        UserInputTranscribedEvent(transcript="Can I help?", is_final=True)
    )
    assert greeting.snapshot("Earlier greeting.").transcript == (
        "Hello, this is Pat.\nCan I help?"
    )


def test_wait_for_change_handles_speech_updates_and_already_changed_input() -> None:
    async def wait() -> None:
        greeting = Greeting()
        greeting.update_speaking(True)
        speaking = greeting.snapshot()
        waiting = asyncio.create_task(greeting.wait_for_change(speaking))
        await asyncio.sleep(0)
        assert not waiting.done()
        greeting.update_speaking(False)
        await asyncio.wait_for(waiting, timeout=1)
        # The change may arrive before confirmation starts awaiting it.
        await asyncio.wait_for(greeting.wait_for_change(speaking), timeout=1)

    asyncio.run(wait())


def test_aborted_greeting_discards_queued_audio_before_amd_releases_gate() -> None:
    async def exercise() -> None:
        timeline = Timeline()
        output = PacedOutput(timeline)
        session = AgentSession[None]()
        session.output.audio = output
        # No input reaches AMD, so this real client never issues a request.
        classifier = ResponsesLLM(
            model="unused-offline",
            api_key="unused-offline",
            base_url="http://127.0.0.1:1",
            store=False,
        )

        async def audio() -> AsyncIterator[rtc.AudioFrame]:
            yield rtc.AudioFrame(
                data=struct.pack("<h", 6000) * 2400,
                sample_rate=SAMPLE_RATE,
                num_channels=1,
                samples_per_channel=2400,
            )

        try:
            await session.start(
                Agent(instructions="Offline playout check"),
                record=False,
                session_host=False,
            )
            async with AMD(session, llm=classifier, stt=None, ivr_detection=False):
                speech = session.say("Synthetic greeting.", audio=audio())
                # Let the actual speech task retain its output sink and wait on
                # AMD authorization. Disabling the output flag alone leaks audio.
                await asyncio.sleep(0.03)
                assert not timeline.agent
                await asyncio.wait_for(silence_greeting(session), timeout=1)
                assert speech.interrupted
                assert not timeline.agent
            await asyncio.sleep(0.15)
            assert not session.output.audio_enabled
            assert not timeline.agent
        finally:
            await session.aclose()
            await output.aclose()
            await classifier.aclose()

    asyncio.run(exercise())
