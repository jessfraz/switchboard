"""Runtime configuration, supplied by the supervising CLI without secret files."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import StrEnum
from urllib.parse import urlsplit


class ConfigurationError(Exception):
    """A safe, fixed diagnostic for missing or incompatible configuration."""


class VoiceEngine(StrEnum):
    PIPELINE = "pipeline"
    GPT_LIVE = "gpt_live"


class BackendReasoningEffort(StrEnum):
    NONE = "none"
    MINIMAL = "minimal"
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    XHIGH = "xhigh"
    MAX = "max"


@dataclass(frozen=True)
class Config:
    url: str
    api_key: str = field(repr=False)
    api_secret: str = field(repr=False)
    trunk_id: str
    voice_engine: VoiceEngine = VoiceEngine.PIPELINE
    openai_api_key: str | None = field(default=None, repr=False)
    realtime_model: str = "gpt-live-1"
    backend_model: str = "gpt-5.6-luna"
    backend_reasoning_effort: BackendReasoningEffort | None = None
    stt_model: str = "deepgram/nova-3"
    llm_model: str = "openai/gpt-5.5"
    tts_model: str = "inworld/inworld-tts-2"
    voice: str | None = None

    def __post_init__(self) -> None:
        if self.voice_engine == VoiceEngine.GPT_LIVE and not (
            self.openai_api_key and self.openai_api_key.strip()
        ):
            raise ConfigurationError("GPT-Live requires an OpenAI API key.")

    @property
    def selected_voice(self) -> str:
        return self.voice or (
            "marin" if self.voice_engine == VoiceEngine.GPT_LIVE else "Ashley"
        )

    @classmethod
    def from_env(cls, env: Mapping[str, str]) -> Config:
        names = (
            "LIVEKIT_URL",
            "LIVEKIT_API_KEY",
            "LIVEKIT_API_SECRET",
            "LIVEKIT_SIP_TRUNK_ID",
        )
        if any(not env.get(name, "").strip() for name in names):
            raise ConfigurationError("Required LiveKit configuration is missing.")
        parts = urlsplit(env["LIVEKIT_URL"])
        if (
            parts.scheme != "wss"
            or not parts.hostname
            or not parts.hostname.endswith(".livekit.cloud")
            or parts.username
            or parts.password
            or parts.query
            or parts.fragment
            or parts.path not in ("", "/")
        ):
            raise ConfigurationError("A secure LiveKit Cloud project URL is required.")
        try:
            engine = VoiceEngine(env.get("LIVEKIT_PHONE_VOICE_ENGINE", "pipeline"))
        except ValueError:
            raise ConfigurationError("Unsupported phone voice engine.") from None
        effort = env.get("LIVEKIT_PHONE_BACKEND_REASONING_EFFORT")
        try:
            reasoning_effort = (
                BackendReasoningEffort(effort) if effort is not None else None
            )
        except ValueError:
            raise ConfigurationError("Unsupported backend reasoning effort.") from None
        return cls(
            url=env["LIVEKIT_URL"],
            api_key=env["LIVEKIT_API_KEY"],
            api_secret=env["LIVEKIT_API_SECRET"],
            trunk_id=env["LIVEKIT_SIP_TRUNK_ID"],
            voice_engine=engine,
            openai_api_key=(
                env.get("OPENAI_API_KEY") if engine == VoiceEngine.GPT_LIVE else None
            ),
            realtime_model=env.get("LIVEKIT_PHONE_REALTIME_MODEL", "gpt-live-1"),
            backend_model=env.get("LIVEKIT_PHONE_BACKEND_MODEL", "gpt-5.6-luna"),
            backend_reasoning_effort=reasoning_effort,
            stt_model=env.get("LIVEKIT_PHONE_STT_MODEL", "deepgram/nova-3"),
            llm_model=env.get("LIVEKIT_PHONE_LLM_MODEL", "openai/gpt-5.5"),
            tts_model=env.get("LIVEKIT_PHONE_TTS_MODEL", "inworld/inworld-tts-2"),
            voice=env.get("LIVEKIT_PHONE_VOICE"),
        )
