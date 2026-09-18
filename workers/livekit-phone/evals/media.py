"""Real-time PCM input and playout for the production agent's audio evals."""

from __future__ import annotations

import asyncio
import math
import sys
import time
import wave
from array import array
from dataclasses import dataclass
from pathlib import Path

from livekit import rtc
from livekit.agents.voice import io

SAMPLE_RATE = 24_000
FRAME_SECONDS = 0.02
_FRAME_SAMPLES = int(SAMPLE_RATE * FRAME_SECONDS)
_FRAME_BYTES = _FRAME_SAMPLES * 2


def _samples(pcm: bytes) -> array[int]:
    values = array("h", pcm)
    if sys.byteorder != "little":
        values.byteswap()
    return values


def _rms(pcm: bytes) -> float:
    values = _samples(pcm)
    return math.sqrt(sum(value * value for value in values) / len(values)) / 32768


@dataclass(frozen=True)
class Clip:
    name: str
    pcm: bytes

    def __post_init__(self) -> None:
        if len(self.pcm) % 2:
            raise ValueError("PCM must contain complete 16-bit samples")

    @property
    def duration(self) -> float:
        return len(self.pcm) / (2 * SAMPLE_RATE)


@dataclass(frozen=True)
class PlayedClip:
    name: str
    start: float
    end: float


class Timeline:
    """Mono PCM spans timestamped against one monotonic clock."""

    def __init__(self) -> None:
        self.origin = time.monotonic()
        self.recipient: list[tuple[float, bytes]] = []
        self.agent: list[tuple[float, bytes]] = []

    def elapsed(self) -> float:
        return time.monotonic() - self.origin

    def _track(self, track: str) -> list[tuple[float, bytes]]:
        if track == "recipient":
            return self.recipient
        if track == "agent":
            return self.agent
        raise ValueError(f"Unknown audio track: {track}")

    def activity(
        self, track: str, threshold: float = 0.005
    ) -> list[tuple[float, float]]:
        """Return audible intervals using 20 ms RMS windows, retaining gaps."""
        intervals: list[tuple[float, float]] = []
        for start, pcm in self._track(track):
            for offset in range(0, len(pcm), _FRAME_BYTES):
                chunk = pcm[offset : offset + _FRAME_BYTES]
                if _rms(chunk) <= threshold:
                    continue
                begin = start + offset / (2 * SAMPLE_RATE)
                end = begin + len(chunk) / (2 * SAMPLE_RATE)
                # Sub-sample timestamp rounding does not create a real audio gap.
                if intervals and begin <= intervals[-1][1] + 1 / SAMPLE_RATE:
                    intervals[-1] = (intervals[-1][0], max(end, intervals[-1][1]))
                else:
                    intervals.append((begin, end))
        return intervals

    def write_wav(self, path: Path) -> None:
        """Write recipient left and agent right, with silence in timing gaps."""
        tracks = (self.recipient, self.agent)
        length = max(
            (
                round(start * SAMPLE_RATE) + len(pcm) // 2
                for track in tracks
                for start, pcm in track
            ),
            default=0,
        )
        stereo = array("h", [0]) * (2 * length)
        for channel, track in enumerate(tracks):
            for start, pcm in track:
                offset = round(start * SAMPLE_RATE)
                values = _samples(pcm)
                stereo[2 * offset + channel : 2 * (offset + len(values)) : 2] = values
        if sys.byteorder != "little":
            stereo.byteswap()
        with wave.open(str(path), "wb") as recording:
            recording.setnchannels(2)
            recording.setsampwidth(2)
            recording.setframerate(SAMPLE_RATE)
            recording.writeframes(stereo.tobytes())


@dataclass
class _QueuedClip:
    clip: Clip
    done: asyncio.Future[PlayedClip]
    requested_at: float
    offset: int = 0
    start: float | None = None
    end: float = 0.0


class PacedInput(io.AudioInput):
    """Emit continuous 20 ms PCM frames, including silence between clips."""

    def __init__(self, timeline: Timeline) -> None:
        super().__init__(label="eval recipient audio")
        self._timeline = timeline
        self._queue: asyncio.Queue[_QueuedClip] = asyncio.Queue()
        self._current: _QueuedClip | None = None
        self._next_at = time.monotonic()
        self._closed = asyncio.Event()

    async def play(self, clip: Clip) -> PlayedClip:
        if self._closed.is_set():
            raise RuntimeError("Audio input is closed")
        done: asyncio.Future[PlayedClip] = asyncio.get_running_loop().create_future()
        self._queue.put_nowait(
            _QueuedClip(clip=clip, done=done, requested_at=time.monotonic())
        )
        return await done

    async def __anext__(self) -> rtc.AudioFrame:
        delay = self._next_at - time.monotonic()
        if delay > 0:
            try:
                await asyncio.wait_for(self._closed.wait(), timeout=delay)
            except TimeoutError:
                pass
        if self._closed.is_set():
            raise StopAsyncIteration

        now = time.monotonic()
        # A frame clock absorbs timer jitter without gradually slowing the audio.
        # Missing a whole frame is starvation, so retain that gap rather than
        # sending old frames in a burst to catch up.
        rendered_at = self._next_at
        if now - rendered_at > FRAME_SECONDS:
            rendered_at = now
        start = rendered_at - self._timeline.origin
        if self._current and self._current.offset >= len(self._current.clip.pcm):
            current = self._current
            if not current.done.done():
                current.done.set_result(
                    PlayedClip(
                        current.clip.name,
                        current.start if current.start is not None else start,
                        current.end,
                    )
                )
            self._current = None

        while self._current is None and not self._queue.empty():
            current = self._queue.get_nowait()
            if current.done.cancelled():
                continue
            if not current.clip.pcm:
                current.done.set_result(PlayedClip(current.clip.name, start, start))
                continue
            self._current = current
            # A newly queued clip cannot begin before it was available.
            rendered_at = max(rendered_at, current.requested_at)
            start = rendered_at - self._timeline.origin

        self._next_at = rendered_at + FRAME_SECONDS
        pcm = bytes(_FRAME_BYTES)
        if self._current:
            current = self._current
            if current.start is None:
                current.start = start
            chunk = current.clip.pcm[current.offset : current.offset + _FRAME_BYTES]
            current.offset += len(chunk)
            current.end = start + len(chunk) / (2 * SAMPLE_RATE)
            pcm = chunk.ljust(_FRAME_BYTES, b"\0")
        self._timeline.recipient.append((start, pcm))
        return rtc.AudioFrame(
            data=pcm,
            sample_rate=SAMPLE_RATE,
            num_channels=1,
            samples_per_channel=_FRAME_SAMPLES,
        )

    async def aclose(self) -> None:
        self._closed.set()
        if self._current:
            self._current.done.cancel()
            self._current = None
        while not self._queue.empty():
            self._queue.get_nowait().done.cancel()


@dataclass
class _Segment:
    stopped: asyncio.Event
    played: float = 0.0
    interrupted: bool = False


@dataclass
class _Playing:
    segment: _Segment
    pcm: bytes
    started_at: float
    wall_time: float
    offset: float
    committed: int = 0


class PacedOutput(io.AudioOutput):
    """Play captured PCM against the clock and retain only audio actually played."""

    def __init__(self, timeline: Timeline) -> None:
        super().__init__(
            label="eval agent audio",
            capabilities=io.AudioOutputCapabilities(pause=False),
            sample_rate=SAMPLE_RATE,
        )
        self._timeline = timeline
        self._segment: _Segment | None = None
        self._playing: _Playing | None = None
        self._flushed: list[_Segment] = []
        self._lock = asyncio.Lock()
        self._closed = False
        self._last_voice_end = 0.0
        self._next_at: float | None = None
        self.first_voice = asyncio.Event()
        self.voice_changed = asyncio.Event()

    @property
    def last_voice_end(self) -> float:
        return self._last_voice_end

    def _commit_played(self) -> None:
        playing = self._playing
        if playing is None:
            return
        samples = min(
            len(playing.pcm) // 2,
            max(0, int((time.monotonic() - playing.started_at) * SAMPLE_RATE)),
        )
        if samples <= playing.committed:
            return
        previous = playing.committed
        for offset in range(previous, samples, _FRAME_SAMPLES):
            end = min(offset + _FRAME_SAMPLES, samples)
            pcm = playing.pcm[2 * offset : 2 * end]
            start = playing.started_at - self._timeline.origin + offset / SAMPLE_RATE
            duration = (end - offset) / SAMPLE_RATE
            self._timeline.agent.append((start, pcm))
            if _rms(pcm) > 0.005:
                self._last_voice_end = start + duration
                self.first_voice.set()
                self.voice_changed.set()
        duration = (samples - previous) / SAMPLE_RATE
        playing.segment.played += duration
        playing.committed = samples
        self.on_playback_progressed(
            started_at=playing.wall_time + previous / SAMPLE_RATE,
            offset=playing.offset + previous / SAMPLE_RATE,
            duration=duration,
        )

    async def capture_frame(self, frame: rtc.AudioFrame) -> None:
        if frame.sample_rate != SAMPLE_RATE or frame.num_channels != 1:
            raise ValueError("Eval output requires 24 kHz mono PCM")
        if not frame.samples_per_channel:
            return
        async with self._lock:
            if self._closed:
                raise RuntimeError("Audio output is closed")
            await super().capture_frame(frame)
            now = time.monotonic()
            if self._segment is None:
                self._segment = _Segment(stopped=asyncio.Event())
                self._next_at = now
                self.on_playback_started(created_at=time.time())
            segment = self._segment
            started_at = self._next_at if self._next_at is not None else now
            # Preserve the device clock across timer jitter. A whole missing
            # frame is an underrun, retained as silence before playback resumes.
            if now - started_at > FRAME_SECONDS:
                started_at = now
            pcm = frame.data.tobytes()
            playing = _Playing(
                segment=segment,
                pcm=pcm,
                started_at=started_at,
                wall_time=time.time() - (now - started_at),
                offset=segment.played,
            )
            self._playing = playing
            samples = len(pcm) // 2
            self._next_at = started_at + samples / SAMPLE_RATE
            try:
                # The complete captured frame is available to the virtual sink.
                # Its sample clock keeps playing during scheduler delays; only
                # elapsed samples are committed if it is interrupted.
                while playing.committed < samples and not segment.stopped.is_set():
                    next_sample = min(
                        ((playing.committed // _FRAME_SAMPLES) + 1) * _FRAME_SAMPLES,
                        samples,
                    )
                    deadline = started_at + next_sample / SAMPLE_RATE
                    if (delay := deadline - time.monotonic()) > 0:
                        try:
                            await asyncio.wait_for(
                                segment.stopped.wait(), timeout=delay
                            )
                        except TimeoutError:
                            pass
                    self._commit_played()
            except asyncio.CancelledError:
                self._commit_played()
                segment.interrupted = True
                segment.stopped.set()
                raise
            finally:
                self._playing = None

    def _finish(self, segment: _Segment) -> None:
        self._flushed.remove(segment)
        self.on_playback_finished(
            playback_position=segment.played, interrupted=segment.interrupted
        )

    def flush(self) -> None:
        super().flush()
        if self._segment is not None:
            segment, self._segment = self._segment, None
            self._flushed.append(segment)
            # Wrappers update their counters and end transcript input after flush
            # returns, so playback completion must be delivered on the next tick.
            asyncio.get_running_loop().call_soon(self._finish, segment)

    def clear_buffer(self) -> None:
        self._commit_played()
        self._playing = None
        if self._segment:
            self._segment.interrupted = True
            self._segment.stopped.set()
            self.flush()
        for segment in self._flushed:
            segment.interrupted = True
            segment.stopped.set()

    async def aclose(self) -> None:
        self._closed = True
        self.clear_buffer()
        async with self._lock:
            pass
        await asyncio.sleep(0)
