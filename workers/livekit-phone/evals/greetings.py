"""Roomless AMD startup scenarios using the production greeting resolver."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from typing import TYPE_CHECKING

from livekit.agents import AMD, AMDCategory, AMDPredictionEvent

from evals.media import Clip, PacedInput, PacedOutput, PlayedClip, Timeline

if TYPE_CHECKING:
    from livekit_phone.runtime import Call

GREETING_SCENARIOS = ("voicemail", "pickup", "screening")


@dataclass
class GreetingResult:
    phase: str = "waiting_for_prediction"
    preliminary: str | None = None
    resolved: str | None = None
    gate_released_at: float | None = None
    agent_audio_seconds: float = 0.0
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
    # Load production helpers only after run.py selects its source snapshot.
    from livekit_phone.greeting import silence_greeting

    call.answered.set()
    detector = AMD(
        call.session,
        llm=call.models.classifier,
        stt=None,
        ivr_detection=False,
    )
    verdict: asyncio.Future[AMDPredictionEvent] = (
        asyncio.get_running_loop().create_future()
    )

    @detector.on("amd_prediction")
    def on_prediction(prediction: AMDPredictionEvent) -> None:
        if not verdict.done():
            verdict.set_result(prediction)

    async def resolve() -> AMDCategory:
        prediction = await verdict
        result.preliminary = prediction.category.value
        result.phase = "confirming_greeting"
        category = await call._resolve_greeting(prediction)
        result.resolved = category.value
        return category

    async def play() -> None:
        played = await source.play(clips[name])
        windows.append(played)
        print(f"Played {name} at {played.start:.1f}-{played.end:.1f}s", flush=True)

    playback: asyncio.Task[None] | None = None
    try:
        async with asyncio.timeout(75):
            accepted = False
            async with detector:
                try:
                    playback = asyncio.create_task(play())
                    resolution = asyncio.create_task(resolve())
                    terminal = asyncio.create_task(call.stop.event.wait())
                    try:
                        done, _ = await asyncio.wait(
                            (resolution, terminal), return_when=asyncio.FIRST_COMPLETED
                        )
                        # A terminal machine result sets Stop inside the resolver.
                        # Resolve it before treating the same event as session loss.
                        _require(
                            result, resolution in done, "session_ended_during_greeting"
                        )
                        category = await resolution
                        accepted = not call.stop.event.is_set()
                    finally:
                        resolution.cancel()
                        terminal.cancel()
                        await asyncio.gather(
                            resolution, terminal, return_exceptions=True
                        )
                finally:
                    if not accepted:
                        await silence_greeting(call.session)
            result.gate_released_at = timeline.elapsed()
            call.greeting = None
            await playback
            _require(
                result,
                not any(
                    start < result.gate_released_at
                    for start, _ in timeline.activity("agent")
                ),
                "audio_before_gate_release",
            )
            if name == "voicemail":
                result.phase = "checking_voicemail_silence"
                await asyncio.sleep(2)
                _require(
                    result,
                    category == AMDCategory.MACHINE_VM
                    and call.stop.event.is_set()
                    and call.stop.reason == "failed",
                    "voicemail_not_rejected",
                )
                _require(result, not timeline.activity("agent"), "voicemail_audio")
            else:
                result.phase = "waiting_for_interactive_opening"
                _require(
                    result,
                    accepted
                    and category
                    in (
                        AMDCategory.HUMAN,
                        AMDCategory.UNCERTAIN,
                        AMDCategory.MACHINE_IVR,
                    ),
                    "interactive_destination_rejected",
                )
                await call._open_conversation()
                _require(result, output.first_voice.is_set(), "opening_was_not_audible")
            result.passed = True
            result.phase = "finished"
    except TimeoutError:
        result.failure = f"{result.phase}_timeout"
        raise
    finally:
        if playback is not None:
            playback.cancel()
            await asyncio.gather(playback, return_exceptions=True)
        result.agent_audio_seconds = sum(
            end - start for start, end in timeline.activity("agent")
        )
