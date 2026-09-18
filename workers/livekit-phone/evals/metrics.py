"""Measurements from played PCM, independent of transcript arrival times."""

from __future__ import annotations

from dataclasses import dataclass

Interval = tuple[float, float]


def intersections(left: list[Interval], right: list[Interval]) -> list[Interval]:
    result: list[Interval] = []
    i = j = 0
    while i < len(left) and j < len(right):
        start = max(left[i][0], right[j][0])
        end = min(left[i][1], right[j][1])
        if end > start:
            result.append((start, end))
        if left[i][1] < right[j][1]:
            i += 1
        else:
            j += 1
    return result


def join_pauses(intervals: list[Interval], gap: float = 0.15) -> list[Interval]:
    result: list[Interval] = []
    for start, end in intervals:
        if result and start - result[-1][1] <= gap:
            result[-1] = (result[-1][0], end)
        else:
            result.append((start, end))
    return result


@dataclass(frozen=True)
class WindowMetrics:
    name: str
    agent_audio_seconds: float
    simultaneous_audio_seconds: float
    longest_overlap_seconds: float
    response_delay_seconds: float | None


def measure_window(
    name: str, window: Interval, recipient: list[Interval], agent: list[Interval]
) -> WindowMetrics:
    played_agent = intersections(agent, [window])
    overlap = intersections(intersections(recipient, [window]), agent)
    # A short intra-word gap does not make sustained overlapping speech harmless.
    bouts = join_pauses(overlap)
    next_response = next(
        (
            max(start, window[1])
            for start, end in join_pauses(agent, gap=0.3)
            if end > window[1]
        ),
        None,
    )
    return WindowMetrics(
        name=name,
        agent_audio_seconds=round(sum(end - start for start, end in played_agent), 3),
        simultaneous_audio_seconds=round(sum(end - start for start, end in overlap), 3),
        longest_overlap_seconds=round(max((e - s for s, e in bouts), default=0), 3),
        response_delay_seconds=(
            round(next_response - window[1], 3) if next_response is not None else None
        ),
    )
