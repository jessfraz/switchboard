"""Allowlisted lifecycle metadata for private audio-evaluation reports."""

from __future__ import annotations

import asyncio
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from types import CoroutineType, FrameType, GeneratorType

from livekit.agents import Agent, FunctionToolsExecutedEvent
from livekit.agents.llm import ChatContext, FunctionCall, FunctionCallOutput
from livekit.plugins.openai.realtime.gpt_live_model import GPTLiveSession

_SESSION_EVENTS = frozenset(
    {
        "session.started",
        "session.updated",
        "session.output_audio.delta",
        "session.output_transcript.delta",
        "session.input_transcript.delta",
        "session.delegation.created",
        "response.event",
        "session.usage.updated",
        "session.closed",
        "session.input_audio.muted",
        "session.input_audio.unmuted",
        "session.instructions.appended",
        "session.thinking.appended",
        "session.commentary.appended",
        "error",
    }
)
_RESPONSE_EVENTS = frozenset(
    {
        "response.created",
        "response.queued",
        "response.in_progress",
        "response.completed",
        "response.failed",
        "response.incomplete",
        "response.output_item.added",
        "response.output_item.done",
        "response.function_call_arguments.delta",
        "response.function_call_arguments.done",
        "response.output_text.delta",
        "response.output_text.done",
        "response.content_part.added",
        "response.content_part.done",
        "response.reasoning_summary_part.added",
        "response.reasoning_summary_part.done",
        "response.reasoning_summary_text.delta",
        "response.reasoning_summary_text.done",
        "error",
    }
)
_NOISY_EVENTS = frozenset(
    {
        "session.output_audio.delta",
        "session.output_transcript.delta",
        "session.input_transcript.delta",
        "session.usage.updated",
        "response.function_call_arguments.delta",
        "response.output_text.delta",
        "response.reasoning_summary_text.delta",
    }
)
_FUNCTIONS = frozenset({"finish_call", "send_dtmf_events"})
_STATUSES = frozenset(
    {"in_progress", "completed", "incomplete", "failed", "cancelled", "queued"}
)


def _get(value: object, key: str) -> object:
    if isinstance(value, Mapping):
        result: object = value.get(key)
        return result
    return None


def _allowed(value: object, options: frozenset[str]) -> str:
    return value if isinstance(value, str) and value in options else "other"


@dataclass(frozen=True)
class LifecycleMetadata:
    received_at: float
    event: str
    backend_event: str | None = None
    delegation_target: str | None = None
    function: str | None = None
    status: str | None = None


@dataclass(frozen=True)
class ToolMetadata:
    kind: str
    name: str
    is_error: bool | None = None


@dataclass(frozen=True)
class StackLocation:
    file: str
    line: int


@dataclass
class DiagnosticReport:
    provider_events_attached: bool = False
    event_counts: dict[str, int] = field(default_factory=dict)
    lifecycle: list[LifecycleMetadata] = field(default_factory=list)
    executed_tools: list[ToolMetadata] = field(default_factory=list)
    history_tools: list[ToolMetadata] = field(default_factory=list)
    pending_tasks: list[list[StackLocation]] = field(default_factory=list)


class Diagnostics:
    def __init__(self, elapsed: Callable[[], float]) -> None:
        self.report = DiagnosticReport()
        self._elapsed = elapsed
        self._duplex: GPTLiveSession | None = None

    def attach(self, agent: Agent) -> None:
        duplex = agent.duplex_session
        if isinstance(duplex, GPTLiveSession):
            self._duplex = duplex
            duplex.on("openai_server_event_received", self.on_provider_event)
            self.report.provider_events_attached = True

    def detach(self) -> None:
        if self._duplex is not None:
            self._duplex.off("openai_server_event_received", self.on_provider_event)
            self._duplex = None

    def on_provider_event(self, raw: object) -> None:
        event = _allowed(_get(raw, "type"), _SESSION_EVENTS)
        backend_event = target = function = status = None
        if event == "session.delegation.created":
            target = _allowed(
                _get(_get(raw, "delegation"), "target"),
                frozenset({"client", "responses"}),
            )
        elif event == "response.event":
            backend = _get(raw, "event")
            backend_event = _allowed(_get(backend, "type"), _RESPONSE_EVENTS)
            item = _get(backend, "item")
            if _get(item, "type") == "function_call":
                function = _allowed(_get(item, "name"), _FUNCTIONS)
                status = _allowed(_get(item, "status"), _STATUSES)

        counted_event = backend_event or event
        counts = self.report.event_counts
        counts[counted_event] = counts.get(counted_event, 0) + 1
        if counted_event not in _NOISY_EVENTS:
            self.report.lifecycle.append(
                LifecycleMetadata(
                    self._elapsed(), event, backend_event, target, function, status
                )
            )

    def on_tools(self, event: FunctionToolsExecutedEvent) -> None:
        for call, result in event.zipped():
            self.report.executed_tools.append(
                ToolMetadata(
                    "executed", _allowed(call.name, _FUNCTIONS), result.is_error
                )
            )

    def capture_history(self, history: ChatContext) -> None:
        self.report.history_tools = [
            ToolMetadata(
                item.type,
                _allowed(item.name, _FUNCTIONS),
                item.is_error if isinstance(item, FunctionCallOutput) else None,
            )
            for item in history.items
            if isinstance(item, FunctionCall | FunctionCallOutput)
        ]

    def capture_pending_tasks(self) -> None:
        """Inspect await locations without retaining source lines, locals, or names."""
        stacks: list[list[StackLocation]] = []
        for task in asyncio.all_tasks():
            if task is asyncio.current_task() or task.done():
                continue
            locations: list[StackLocation] = []
            awaited: object = task.get_coro()
            for _ in range(30):
                frame: FrameType | None
                if isinstance(awaited, CoroutineType):
                    frame = awaited.cr_frame
                    awaited = awaited.cr_await
                elif isinstance(awaited, GeneratorType):
                    frame = awaited.gi_frame
                    awaited = awaited.gi_yieldfrom
                else:
                    break
                if frame is not None:
                    locations.append(
                        StackLocation(
                            Path(frame.f_code.co_filename).name, frame.f_lineno
                        )
                    )
            if locations:
                stacks.append(locations)
        self.report.pending_tasks = sorted(
            stacks,
            key=lambda stack: [(location.file, location.line) for location in stack],
        )
