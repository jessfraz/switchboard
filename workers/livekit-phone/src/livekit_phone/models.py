"""Model selection and ownership for a single local call."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass

import aiohttp
from livekit.agents import AgentSession, TurnHandlingOptions, inference, llm
from livekit.plugins.openai.realtime import GPTLiveModel, ResponsesDelegationOptions
from livekit.plugins.openai.responses import LLM as ResponsesLLM
from openai.types import Reasoning as ReasoningOptions
from openai.types.shared_params import Reasoning

from livekit_phone.config import Config, VoiceEngine


@dataclass
class CallModels:
    session: AgentSession[None]
    classifier: llm.LLM[None]

    async def aclose(self) -> None:
        # AgentSession closes streams, but these model clients are caller-owned.
        # GPT-Live uses a separate text model solely for answering-machine checks.
        models = [self.session.stt, self.session.llm, self.session.tts]
        if self.classifier is not self.session.llm:
            models.append(self.classifier)
        results = await asyncio.gather(
            *(model.aclose() for model in models if model is not None),
            return_exceptions=True,
        )
        for result in results:
            if isinstance(result, BaseException):
                raise result


def create_models(
    config: Config,
    http_session: aiohttp.ClientSession,
    *,
    backend_instructions: str,
) -> CallModels:
    """Construct clients without opening network connections."""
    # Never honor ambient endpoint overrides when sending call audio or secrets.
    if config.voice_engine == VoiceEngine.GPT_LIVE:
        # AMD returns verdicts via tools, which Astra supports through Responses.
        # Greeting checks need little reasoning, even with a deeper call backend.
        classifier: llm.LLM[None] = ResponsesLLM(
            model=config.backend_model,
            api_key=config.openai_api_key or "",
            base_url="https://api.openai.com/v1",
            use_websocket=False,
            reasoning=ReasoningOptions(effort="low"),
            store=False,
        )
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
                voice=config.selected_voice,
                responses_options=responses_options,
                api_key=config.openai_api_key,
                base_url="https://api.openai.com/v1",
                http_session=http_session,
            ),
            turn_handling=TurnHandlingOptions(turn_detection="realtime_llm"),
            user_away_timeout=None,
        )
    else:
        gateway = "https://agent-gateway.livekit.cloud/v1"
        classifier = inference.LLM(
            model=config.llm_model,
            base_url=gateway,
            api_key=config.api_key,
            api_secret=config.api_secret,
        )
        session = AgentSession[None](
            stt=inference.STT(
                model=config.stt_model,
                language="en",
                base_url=gateway,
                api_key=config.api_key,
                api_secret=config.api_secret,
                http_session=http_session,
            ),
            llm=classifier,
            tts=inference.TTS(
                model=config.tts_model,
                voice=config.selected_voice,
                base_url=gateway,
                api_key=config.api_key,
                api_secret=config.api_secret,
                http_session=http_session,
            ),
            turn_handling=TurnHandlingOptions(
                turn_detection=inference.TurnDetector(
                    version="v1",
                    base_url=gateway,
                    api_key=config.api_key,
                    api_secret=config.api_secret,
                    local_fallback=False,
                    http_session=http_session,
                ),
            ),
            user_away_timeout=None,
            use_tts_aligned_transcript=True,
        )
    return CallModels(session=session, classifier=classifier)
