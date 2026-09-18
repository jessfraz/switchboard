"""Natural openings and voicemail hangup through the production voice session."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from typing import TYPE_CHECKING

from evals.media import Clip, PacedInput, PacedOutput, PlayedClip, Timeline, wait_answer

if TYPE_CHECKING:
    from livekit_phone.runtime import Call

GREETING_SCENARIOS = (
    "human",
    "voicemail",
    "pickup",
    "screening",
    "paused_voicemail",
    "paused_screening",
    "delayed_pickup",
    "silent_answer",
    "boundary_greeting",
)


@dataclass
class GreetingResult:
    phase: str = "playing_greeting"
    first_agent_audio_at: float | None = None
    response_delay_seconds: float | None = None
    agent_audio_seconds: float = 0.0
    ended_call: bool = False
    failure: str | None = None
    passed: bool = False


class GreetingCheckFailed(RuntimeError):
    """The observed startup violated this scenario's expected behavior."""


def _require(result: GreetingResult, condition: bool, failure: str) -> None:
    if not condition:
        result.failure = failure
        raise GreetingCheckFailed(failure)


async def run_greeting(
    name: str,
    call: Call,
    clips: dict[str, Clip],
    source: PacedInput,
    output: PacedOutput,
    timeline: Timeline,
    windows: list[PlayedClip],
    result: GreetingResult,
) -> None:
    call.answered.set()
    opening = asyncio.create_task(call._open_conversation())
    try:
        async with asyncio.timeout(30):
            played = await source.play(clips[name])
            windows.append(played)
            print(f"Played {name} at {played.start:.1f}-{played.end:.1f}s", flush=True)
            if name in ("voicemail", "paused_voicemail"):
                result.phase = "waiting_for_voicemail_hangup"
                # A short introduction or sign-off is permitted. Hanging up is
                # the observable contract; saying goodbye alone is insufficient.
                await asyncio.wait_for(call.stop.event.wait(), timeout=8)
                _require(result, call.stop.reason == "completed", "session_failed")
                _require(result, output.first_voice.is_set(), "missing_signoff_audio")
            else:
                result.phase = "waiting_for_interactive_opening"
                await wait_answer(timeline, output, played.end)
                await opening
                _require(result, not call.stop.event.is_set(), "premature_hangup")
                if name == "human":
                    first = timeline.activity("agent")[0][0]
                    result.response_delay_seconds = max(0.0, first - played.end)
                    _require(
                        result,
                        result.response_delay_seconds <= 1.5,
                        "human_opening_exceeded_1_5_seconds",
                    )
            result.passed = True
            result.phase = "finished"
    except TimeoutError:
        result.failure = f"{result.phase}_timeout"
        raise
    finally:
        opening.cancel()
        await asyncio.gather(opening, return_exceptions=True)
        activity = timeline.activity("agent")
        result.first_agent_audio_at = activity[0][0] if activity else None
        result.agent_audio_seconds = sum(end - start for start, end in activity)
        result.ended_call = call.stop.event.is_set()
