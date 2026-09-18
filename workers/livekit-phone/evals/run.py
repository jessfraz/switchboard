"""Run one paid, real-audio scenario through the production phone agent.

Run from the worker directory with ``uv run python -m evals.run --help``.
No telephone is dialed. Credentials come only from OPENAI_API_KEY.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import io
import json
import logging
import os
import sys
from dataclasses import asdict, dataclass
from pathlib import Path

from livekit.agents import ConversationItemAddedEvent
from livekit.agents.utils import http_context
from livekit.agents.voice import TranscriptSynchronizer

from evals.diagnostics import Diagnostics
from evals.fixtures import make_clips
from evals.greetings import GREETING_SCENARIOS, GreetingResult, run_greeting
from evals.media import Clip, PacedInput, PacedOutput, PlayedClip, Timeline
from evals.metrics import measure_window

TASK = (
    "Request a $48 refund for damaged lamp order 7842 for Casey Morgan. "
    "The lamp shade arrived cracked; the shipping box was intact. "
    "No photo is available. Do not purchase a replacement or accept store credit. "
    "If a decision requires later supervisor approval, obtain the next step and "
    "timing, and accurately report that the request remains pending."
)


def private_output(path: Path) -> Path:
    path = path.expanduser().resolve()
    if any((parent / ".git").exists() for parent in (path, *path.parents)):
        raise ValueError("Audio evaluation outputs must be outside Git checkouts")
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.chmod(0o700)
    return path


@dataclass(frozen=True)
class Utterance:
    speaker: str
    text: str
    received_at: float
    interrupted: bool


async def wait_voice(output: PacedOutput, since: float) -> None:
    async with asyncio.timeout(25):
        while output.last_voice_end <= since:
            output.voice_changed.clear()
            await output.voice_changed.wait()


async def wait_quiet(timeline: Timeline, output: PacedOutput) -> None:
    async with asyncio.timeout(25):
        while (remaining := 1.4 - (timeline.elapsed() - output.last_voice_end)) > 0:
            output.voice_changed.clear()
            try:
                await asyncio.wait_for(output.voice_changed.wait(), timeout=remaining)
            except TimeoutError:
                return


async def wait_answer(timeline: Timeline, output: PacedOutput, since: float) -> None:
    await wait_voice(output, since)
    await wait_quiet(timeline, output)


async def scenario(
    name: str,
    clips: dict[str, Clip],
    source: PacedInput,
    output: PacedOutput,
    timeline: Timeline,
    windows: list[PlayedClip],
) -> None:
    async def play(key: str) -> PlayedClip:
        played = await source.play(clips[key])
        windows.append(played)
        print(f"Played {key} at {played.start:.1f}-{played.end:.1f}s", flush=True)
        return played

    await play("greeting")
    async with asyncio.timeout(25):
        await output.first_voice.wait()
    if name == "turns":
        await asyncio.sleep(1.0)
        interruption = await play("interrupt")
        await wait_answer(timeline, output, interruption.end)
        question = await play("long_question")
        await wait_voice(output, question.end)
        await asyncio.sleep(0.8)
        acknowledgment = await play("backchannel")
        # A backchannel should not demand a new answer, so don't require one.
        await asyncio.sleep(3)
        await wait_quiet(timeline, output)
        windows.append(
            PlayedClip("after_backchannel", acknowledgment.end, timeline.elapsed())
        )
    else:
        await wait_answer(timeline, output, 0)
        if name == "hold":
            start = await play("hold_start")
            await asyncio.sleep(2)
            await play("hold_music")
            await play("announcement")
            await play("hold_music")
            windows.append(
                PlayedClip("hold_silence", start.end + 2, timeline.elapsed())
            )
            returned = await play("return")
            await wait_answer(timeline, output, returned.end)
            await play("transfer")
            await asyncio.sleep(2)
            await play("hold_music")
            representative = await play("new_person")
            await wait_answer(timeline, output, representative.end)
        else:
            photo = await play("photo")
            await wait_answer(timeline, output, photo.end)
    pending = await play("pending")
    await wait_answer(timeline, output, pending.end)
    await play("farewell")
    await asyncio.sleep(6)


async def run(args: argparse.Namespace) -> None:
    # Import after choosing the immutable source snapshot for an honest A/B run.
    sys.path.insert(0, str(args.source_root.resolve()))
    from livekit_phone.agent import PhoneAgent
    from livekit_phone.config import Config
    from livekit_phone.control import Stop, transcript_event
    from livekit_phone.protocol import EventSink, Start
    from livekit_phone.runtime import Call

    startup = args.scenario in GREETING_SCENARIOS
    if startup and not hasattr(Call, "_resolve_greeting"):
        raise ValueError(
            "Startup scenarios require a source snapshot with the production "
            "greeting resolver; older baseline sources are unsupported."
        )
    root = private_output(args.output)
    destination = root / f"{args.label}-{args.scenario}-{args.repeat}"
    destination.mkdir(mode=0o700)  # Never overwrite evidence from an earlier run.
    clips = await make_clips(private_output(root / "fixtures"))
    source_hashes = {
        name: hashlib.sha256(
            (args.source_root / "livekit_phone" / name).read_bytes()
        ).hexdigest()
        for name in ("agent.py", "models.py", "runtime.py")
    }
    greeting_module = args.source_root / "livekit_phone" / "greeting.py"
    if greeting_module.is_file():
        source_hashes["greeting.py"] = hashlib.sha256(
            greeting_module.read_bytes()
        ).hexdigest()
    greeting_result = GreetingResult() if startup else None
    timeline = Timeline()
    diagnostics = Diagnostics(timeline.elapsed)
    source, output = PacedInput(timeline), PacedOutput(timeline)
    windows: list[PlayedClip] = []
    transcript: list[Utterance] = []
    stop = Stop()
    request = Start(
        call_id="synthetic-evaluation",
        destination="+12025550100",
        caller_name="Casey Morgan",
        task=TASK,
        max_duration_seconds=180,
    )
    config = Config(
        url="wss://unused.livekit.cloud",
        api_key="unused",
        api_secret="unused",
        trunk_id="unused",
        openai_api_key=os.environ["OPENAI_API_KEY"],
    )
    failure: str | None = None
    completed = False
    async with http_context.open() as http:
        # Use the real call constructor, including its backend instructions and
        # event handlers. Starting the audio session here never dials its room.
        call = Call(request, config, stop, EventSink(io.StringIO()), http)
        models = call.models
        session = models.session
        session.on("function_tools_executed", diagnostics.on_tools)
        synchronizer = TranscriptSynchronizer(
            next_in_chain_audio=output, next_in_chain_text=None
        )
        session.input.audio = source
        session.output.audio = synchronizer.audio_output
        session.output.transcription = synchronizer.text_output

        @session.on("conversation_item_added")
        def on_item(event: ConversationItemAddedEvent) -> None:
            item = transcript_event(event)
            if item is not None:
                transcript.append(
                    Utterance(
                        item.speaker, item.text, timeline.elapsed(), item.interrupted
                    )
                )

        try:
            async with asyncio.timeout(180):
                phone_agent = PhoneAgent(request, stop)
                await session.start(phone_agent, record=False, session_host=False)
                diagnostics.attach(phone_agent)
                if greeting_result is not None:
                    await run_greeting(
                        args.scenario,
                        call,
                        clips,
                        source,
                        output,
                        timeline,
                        windows,
                        greeting_result,
                    )
                else:
                    playback = asyncio.create_task(
                        scenario(
                            args.scenario, clips, source, output, timeline, windows
                        )
                    )
                    terminal = asyncio.create_task(stop.event.wait())
                    try:
                        done, _ = await asyncio.wait(
                            (playback, terminal), return_when=asyncio.FIRST_COMPLETED
                        )
                        if playback in done:
                            await playback
                            try:
                                await asyncio.wait_for(terminal, timeout=10)
                            except TimeoutError:
                                failure = "MissingFinishCall"
                        elif not any(window.name == "pending" for window in windows):
                            failure = "PrematureFinishCall"
                        if stop.event.is_set() and stop.reason != "completed":
                            failure = "SessionFailed"
                    finally:
                        playback.cancel()
                        terminal.cancel()
                        await asyncio.gather(playback, terminal, return_exceptions=True)
        except Exception as error:
            # Keep failures explicit without copying provider request headers or keys.
            failure = type(error).__name__
            print(f"Scenario failed: {failure}", flush=True)
        finally:
            completed = stop.event.is_set() and stop.reason == "completed"
            if failure is not None:
                diagnostics.capture_pending_tasks()
            await session.aclose()
            await synchronizer.aclose()
            await source.aclose()
            await output.aclose()
            await models.aclose()
            await call.client.aclose()
            diagnostics.capture_history(session.history)
            diagnostics.detach()
            session.off("function_tools_executed", diagnostics.on_tools)
    timeline.write_wav(destination / "conversation.wav")
    recipient = timeline.activity("recipient")
    agent = timeline.activity("agent")
    report = {
        "label": args.label,
        "scenario": args.scenario,
        "source_root": str(args.source_root.resolve()),
        "source_sha256": source_hashes,
        "model": config.realtime_model,
        "backend": config.backend_model,
        "voice": config.voice,
        "failure": failure,
        "completed": completed,
        "summary": stop.summary,
        "greeting": asdict(greeting_result) if greeting_result is not None else None,
        "diagnostics": asdict(diagnostics.report),
        "duration_seconds": timeline.elapsed(),
        "windows": [asdict(window) for window in windows],
        "metrics": [
            asdict(
                measure_window(
                    window.name, (window.start, window.end), recipient, agent
                )
            )
            for window in windows
        ],
        "transcript": [asdict(item) for item in transcript],
        "recipient_activity": recipient,
        "agent_activity": agent,
    }
    (destination / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    (destination / "transcript.md").write_text(
        "\n\n".join(
            f"**{item.speaker}** ({item.received_at:.1f}s): {item.text}"
            for item in transcript
        )
        + "\n"
    )
    for path in destination.iterdir():
        path.chmod(0o600)
    print(
        json.dumps(
            {
                "label": args.label,
                "scenario": args.scenario,
                "repeat": args.repeat,
                "failure": failure,
                "completed": completed,
                "startup_passed": (
                    greeting_result.passed if greeting_result is not None else None
                ),
                "output": str(destination),
            }
        )
    )
    if failure:
        raise SystemExit(1)


def main() -> None:
    logging.disable(logging.CRITICAL)
    os.environ["OTEL_SDK_DISABLED"] = "true"
    for signal in ("TRACES", "METRICS", "LOGS"):
        os.environ[f"OTEL_{signal}_EXPORTER"] = "none"
    os.environ["LK_OPENAI_DEBUG"] = "0"
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=Path("src"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--label", required=True, choices=("baseline", "candidate"))
    parser.add_argument(
        "--scenario",
        choices=("turns", "hold", "outcome", *GREETING_SCENARIOS),
        required=True,
    )
    parser.add_argument("--repeat", type=int, default=1)
    args = parser.parse_args()
    # Private from creation, including subprocess-generated fixtures and SDK logs.
    os.umask(0o077)
    asyncio.run(run(args))


if __name__ == "__main__":
    main()
