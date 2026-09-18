"""Runtime configuration, supplied by the supervising CLI without secret files."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import StrEnum
from urllib.parse import urlsplit


class ConfigurationError(Exception):
    """A safe, fixed diagnostic for missing or incompatible configuration."""


class VoiceEngine(StrEnum):
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
    openai_api_key: str = field(repr=False)
    voice_engine: VoiceEngine = VoiceEngine.GPT_LIVE
    realtime_model: str = "gpt-live-1"
    backend_model: str = "gpt-5.6-luna"
    backend_reasoning_effort: BackendReasoningEffort | None = None
    voice: str = "marin"

    def __post_init__(self) -> None:
        if not self.openai_api_key.strip():
            raise ConfigurationError("GPT-Live requires an OpenAI API key.")

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
            engine = VoiceEngine(env.get("LIVEKIT_PHONE_VOICE_ENGINE", "gpt_live"))
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
            openai_api_key=env.get("OPENAI_API_KEY", ""),
            realtime_model=env.get("LIVEKIT_PHONE_REALTIME_MODEL", "gpt-live-1"),
            backend_model=env.get("LIVEKIT_PHONE_BACKEND_MODEL", "gpt-5.6-luna"),
            backend_reasoning_effort=reasoning_effort,
            voice=env.get("LIVEKIT_PHONE_VOICE") or "marin",
        )
