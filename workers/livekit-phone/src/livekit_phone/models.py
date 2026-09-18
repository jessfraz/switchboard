"""Model selection and ownership for a single local call."""

from __future__ import annotations

import asyncio
from collections.abc import Mapping
from dataclasses import dataclass

import aiohttp
from livekit.agents import AgentSession, TurnHandlingOptions
from livekit.plugins.openai.realtime import GPTLiveModel, ResponsesDelegationOptions
from livekit.plugins.openai.realtime.gpt_live_model import GPTLiveSession
from openai.types.shared_params import Reasoning

from livekit_phone.config import Config


@dataclass
class CallModels:
    session: AgentSession[None]

    async def aclose(self) -> None:
        # AgentSession closes streams, but the model client is caller-owned.
        if self.session.llm is not None:
            await self.session.llm.aclose()


async def wait_for_voice_ready(duplex: GPTLiveSession) -> None:
    """Wait for the provider to accept this session before dialing a recipient."""
    ready = asyncio.Event()

    def on_event(event: object) -> None:
        if isinstance(event, Mapping) and event.get("type") == "session.started":
            ready.set()

    duplex.on("openai_server_event_received", on_event)
    try:
        # Subscribe before checking: the acknowledgement can arrive while the
        # agent starts. The public ID covers a session already acknowledged.
        if duplex.session_id is not None:
            return
        await ready.wait()
    finally:
        duplex.off("openai_server_event_received", on_event)


def create_models(
    config: Config,
    http_session: aiohttp.ClientSession,
    *,
    backend_instructions: str,
) -> CallModels:
    """Construct clients without opening network connections."""
    # Never honor ambient endpoint overrides when sending call audio or secrets.
    responses_options = ResponsesDelegationOptions(
        model=config.backend_model,
        instructions=backend_instructions,
    )
    if config.backend_reasoning_effort is not None:
        responses_options["reasoning"] = Reasoning(
            effort=config.backend_reasoning_effort.value
        )
    # The pinned plugin omits `store`; the Live API defaults it to false.
    # The pinned plugin has no public recording option.
    session = AgentSession[None](
        llm=GPTLiveModel(
            model=config.realtime_model,
            voice=config.voice,
            responses_options=responses_options,
            api_key=config.openai_api_key,
            base_url="https://api.openai.com/v1",
            http_session=http_session,
        ),
        turn_handling=TurnHandlingOptions(turn_detection="realtime_llm"),
        user_away_timeout=None,
        # Outbound SIP needs no microphone echo warmup. Make that SDK default
        # explicit so early interruptions also reach the model in audio evals.
        aec_warmup_duration=None,
    )
    return CallModels(session=session)
