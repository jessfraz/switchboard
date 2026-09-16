"""Runtime configuration, supplied by the supervising CLI without secret files."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from urllib.parse import urlsplit


class ConfigurationError(Exception):
    """A safe, fixed diagnostic for missing or incompatible configuration."""


@dataclass(frozen=True)
class Config:
    url: str
    api_key: str = field(repr=False)
    api_secret: str = field(repr=False)
    trunk_id: str
    stt_model: str = "deepgram/nova-3"
    llm_model: str = "google/gemini-3.1-flash-lite"
    tts_model: str = "inworld/inworld-tts-2"
    voice: str = "Ashley"

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
        return cls(
            url=env["LIVEKIT_URL"],
            api_key=env["LIVEKIT_API_KEY"],
            api_secret=env["LIVEKIT_API_SECRET"],
            trunk_id=env["LIVEKIT_SIP_TRUNK_ID"],
            stt_model=env.get("LIVEKIT_PHONE_STT_MODEL", "deepgram/nova-3"),
            llm_model=env.get(
                "LIVEKIT_PHONE_LLM_MODEL", "google/gemini-3.1-flash-lite"
            ),
            tts_model=env.get("LIVEKIT_PHONE_TTS_MODEL", "inworld/inworld-tts-2"),
            voice=env.get("LIVEKIT_PHONE_VOICE", "Ashley"),
        )
