"""Fictional recipient speech and hold music, generated locally on macOS."""

from __future__ import annotations

import asyncio
import hashlib
import math
import struct
import wave
from pathlib import Path

from evals.media import SAMPLE_RATE, Clip

SPEECH = {
    "greeting": "Thanks for calling Example Shop. How can I help?",
    "human": "Hello?",
    "voicemail": (
        "You have reached the Example Shop message service. "
        "Record your message after the signal."
    ),
    "pickup": (
        "One moment while I see if Pat can take your call. "
        "Hello, Pat at Example Shop speaking. How may I help you?"
    ),
    "screening": (
        "Please tell me your name and why you are calling, "
        "so I can see if Pat is available."
    ),
    "screening_start": "Please tell me your name",
    "screening_end": "and why you are calling, so I can see if Pat is available.",
    "connecting": "One moment while I connect your call.",
    "live_pickup": "Hello, this is Pat. I just picked up. How may I help you?",
    "interrupt": "Sorry, what is the order number?",
    "question_start": "Before I look that up, could you tell me the customer's name",
    "question_middle": "and whether the damage was to the packaging",
    "question_end": "or to the lamp itself?",
    "backchannel": "Mm hmm.",
    "hold_start": "Please hold while I check with my supervisor.",
    "announcement": "Thanks for waiting. Your call is important to us.",
    "return": "Are you still there?",
    "transfer": "I am transferring you to the returns supervisor. Please hold.",
    "new_person": "This is Jordan, the returns supervisor. How can I help?",
    "photo": "Can you check with Casey now and get a photo?",
    "pending": (
        "I submitted the refund request. It is still pending approval. "
        "We will email a decision within two business days."
    ),
    "farewell": "That is all I can do today. Thank you. Goodbye.",
}


async def make_clips(directory: Path) -> dict[str, Clip]:
    await asyncio.to_thread(directory.mkdir, parents=True, exist_ok=True, mode=0o700)
    clips: dict[str, Clip] = {}
    for name, text in SPEECH.items():
        digest = hashlib.sha256(text.encode()).hexdigest()[:12]
        path = directory / f"{name}-{digest}.wav"
        if not path.exists():
            process = await asyncio.create_subprocess_exec(
                "/usr/bin/say",
                "--file-format=WAVE",
                "--data-format=LEI16@24000",
                "-v",
                "Samantha",
                "-r",
                "170",
                "-o",
                str(path),
                text,
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.PIPE,
            )
            async with asyncio.timeout(30):
                _, error = await process.communicate()
            if process.returncode:
                raise RuntimeError(
                    f"Speech fixture generation failed: {error.decode()}"
                )
            path.chmod(0o600)
        with wave.open(str(path), "rb") as audio:
            if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth()) != (
                SAMPLE_RATE,
                1,
                2,
            ):
                raise ValueError("Expected mono 24 kHz PCM16 fixture")
            clips[name] = Clip(name, audio.readframes(audio.getnframes()))
    clips["long_question"] = Clip(
        "long_question",
        clips["question_start"].pcm
        + bytes(SAMPLE_RATE)
        + clips["question_middle"].pcm
        + bytes(round(SAMPLE_RATE * 0.9) * 2)
        + clips["question_end"].pcm,
    )
    for name, first, pause, last in (
        ("paused_voicemail", "human", 2.0, "voicemail"),
        ("paused_screening", "screening_start", 1.5, "screening_end"),
        ("delayed_pickup", "connecting", 1.0, "live_pickup"),
    ):
        clips[name] = Clip(
            name,
            clips[first].pcm + bytes(round(SAMPLE_RATE * pause) * 2) + clips[last].pcm,
        )
    for name, pause in (("silent_answer", 4.0), ("boundary_greeting", 1.7)):
        clips[name] = Clip(
            name, bytes(round(SAMPLE_RATE * pause) * 2) + clips["human"].pcm
        )
    notes = (261.63, 329.63, 392.00, 329.63, 293.66, 349.23, 440.0, 349.23)
    music = bytearray()
    for sample in range(12 * SAMPLE_RATE):
        t = sample / SAMPLE_RATE
        local = t % 0.5
        envelope = min(local / 0.03, 1.0) * min((0.5 - local) / 0.1, 1.0)
        frequency = notes[int(t * 2) % len(notes)]
        value = (
            0.08
            * envelope
            * (
                math.sin(2 * math.pi * frequency * t)
                + 0.35 * math.sin(4 * math.pi * frequency * t)
            )
        )
        music.extend(struct.pack("<h", round(value * 32767)))
    clips["hold_music"] = Clip("hold_music", bytes(music))
    return clips
