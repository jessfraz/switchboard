"""Local cancellation and transcript rules independent of network services."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass, field

from livekit.agents import ConversationItemAddedEvent
from livekit.agents.llm import ChatMessage

from livekit_phone.protocol import CompletionReason, Transcript


@dataclass
class Stop:
    event: asyncio.Event = field(default_factory=asyncio.Event)
    reason: CompletionReason = "failed"
    summary: str | None = None

    def request(self, reason: CompletionReason, summary: str | None = None) -> None:
        # Preserve the first terminal outcome when hangup, EOF and timeout race.
        if not self.event.is_set():
            self.reason = reason
            self.summary = summary
            self.event.set()


def transcript_event(event: ConversationItemAddedEvent) -> Transcript | None:
    item = event.item
    if (
        not isinstance(item, ChatMessage)
        or item.role not in ("user", "assistant")
        or not item.text_content
    ):
        return None
    # LiveKit commits assistant messages after playout and truncates their text
    # after interruption. Using these items avoids recording unspoken LLM deltas.
    return Transcript(
        speaker="agent" if item.role == "assistant" else "recipient",
        text=item.text_content,
        timestamp_ms=max(0, round(item.created_at * 1_000)),
        interrupted=item.interrupted,
    )
