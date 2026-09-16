"""Versioned, bounded messages on a private pipe; no credentials cross it."""

from __future__ import annotations

from typing import Annotated, Literal, TextIO

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    TypeAdapter,
    ValidationInfo,
    field_validator,
)

MAX_COMMAND_BYTES = 65_536


class Message(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, frozen=True)
    protocol_version: Literal[1] = 1

    @field_validator("protocol_version", mode="before")
    @classmethod
    def integer_version(cls, value: object) -> object:
        if type(value) is not int:
            raise ValueError("protocol version must be an integer")
        return value


class Start(Message):
    type: Literal["start"] = "start"
    call_id: str = Field(pattern=r"^[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}$")
    destination: str = Field(pattern=r"^\+[1-9][0-9]{7,14}$")
    task: str = Field(min_length=1, max_length=8_000)
    caller_name: str = Field(min_length=1, max_length=80)
    max_duration_seconds: int = Field(ge=30, le=3_600)

    @field_validator("task", "caller_name")
    @classmethod
    def meaningful_text(cls, value: str, info: ValidationInfo) -> str:
        if not value.strip() or "\x00" in value:
            raise ValueError("text must be nonempty and contain no NUL")
        limit = 80 if info.field_name == "caller_name" else 8_000
        if len(value.encode("utf-8")) > limit:
            raise ValueError("text exceeds its byte limit")
        return value

    @field_validator("caller_name")
    @classmethod
    def single_line_name(cls, value: str) -> str:
        if any(ord(char) < 32 for char in value):
            raise ValueError("caller name must be one line")
        return value


class Cancel(Message):
    type: Literal["cancel"] = "cancel"


Command = Annotated[Start | Cancel, Field(discriminator="type")]
COMMAND: TypeAdapter[Start | Cancel] = TypeAdapter(Command)


def parse_command(line: bytes) -> Start | Cancel:
    if not line or len(line) > MAX_COMMAND_BYTES:
        raise ValueError("invalid command size")
    return COMMAND.validate_json(line)


class Lifecycle(Message):
    type: Literal["ready", "dialing", "connected"]


class Transcript(Message):
    type: Literal["transcript"] = "transcript"
    speaker: Literal["agent", "recipient"]
    text: str
    timestamp_ms: int = Field(ge=0)
    interrupted: bool


class ApprovalRequired(Message):
    type: Literal["approval_required"] = "approval_required"
    reason: str


CompletionReason = Literal[
    "completed", "cancelled", "timeout", "approval_required", "failed"
]


class Completed(Message):
    type: Literal["completed"] = "completed"
    reason: CompletionReason
    remote_hangup_confirmed: bool
    summary: str | None = None


class Error(Message):
    type: Literal["error"] = "error"
    code: str
    message: str


class EventSink:
    def __init__(self, output: TextIO) -> None:
        self._output = output
        self.closed = False

    def emit(self, event: Message) -> None:
        if self.closed:
            return
        try:
            self._output.write(event.model_dump_json(exclude_none=True) + "\n")
            self._output.flush()
        except (BrokenPipeError, OSError):
            self.closed = True
