"""Opt-in synthetic conversation evaluations using real production inference.

Run with the worker's normal LiveKit environment and --run. No SIP call, room,
audio, or recording is created. Text sessions execute the real agent's waiting
and finishing tools. DTMF cases inspect real model selection using the real tool
schema without executing it. These checks do not prove audio or carrier behavior.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
from dataclasses import dataclass
from typing import Literal

from livekit.agents import AgentSession, TurnHandlingOptions
from livekit.agents.beta.workflows.utils import DtmfEvent
from livekit.agents.llm import ChatContext, ToolContext
from livekit.agents.llm.chat_context import Instructions
from livekit.agents.utils import http_context
from livekit.agents.voice.run_result import (
    ChatMessageEvent,
    FunctionCallEvent,
    RunResult,
)
from pydantic import BaseModel, ConfigDict

from livekit_phone.agent import PhoneAgent
from livekit_phone.config import Config, VoiceEngine
from livekit_phone.control import Stop
from livekit_phone.models import CallModels, create_models
from livekit_phone.protocol import Start


@dataclass(frozen=True)
class Turn:
    name: str
    prompt: str
    outcome: Literal["reply", "wait", "finish"] = "reply"
    contains: tuple[str, ...] = ()
    any_of: tuple[str, ...] = ()
    silent: bool = False


@dataclass(frozen=True)
class Scenario:
    name: str
    task: str
    turns: tuple[Turn, ...]


REFUND = (
    "Request a full $10 refund to the original payment method for damaged order "
    "123, purchased September 1, 2026 by Casey Example, email casey@example.org. "
    "Share those details as needed for lookup. Do not accept fees, reduced "
    "refunds, credit, replacements, or new terms."
)

SCENARIOS = (
    Scenario(
        "identity",
        REFUND,
        (
            Turn(
                "introduction",
                "I'm the virtual assistant. Who am I speaking with?",
                contains=("casey", "assistant"),
            ),
            Turn(
                "name", "What is the purchaser's name?", contains=("casey", "example")
            ),
            Turn("email", "What is the order email?", contains=("example.org",)),
            Turn("order", "What is the order number?", contains=("123",)),
        ),
    ),
    Scenario(
        "objection",
        REFUND,
        (
            Turn(
                "stop",
                "I object to AI assistants and transcription. End this call.",
                "finish",
            ),
        ),
    ),
    Scenario(
        "refund",
        REFUND,
        (
            Turn(
                "fee",
                "I can refund $10 if you accept a $3 fee. Do you agree?",
                contains=("refund",),
                any_of=("no", "not", "cannot", "can't", "without"),
            ),
            Turn(
                "offer",
                "Then I can refund the full $10 without a fee. Shall I proceed?",
                any_of=("yes", "proceed", "process", "issue", "go ahead"),
            ),
            Turn("lookup", "One moment while I process that.", "wait"),
            Turn(
                "confirmation",
                "Your full $10 refund to the original payment method has been "
                "processed. Reference REF-123. Nothing else is needed.",
                "finish",
                contains=("ref-123",),
            ),
        ),
    ),
    Scenario(
        "transfer",
        REFUND,
        (
            Turn(
                "transfer",
                "I cannot handle refunds here. I am transferring you to a "
                "customer service representative. Please hold.",
                "wait",
            ),
            Turn(
                "queue",
                "All representatives are busy. Your queue position is three. "
                "Please continue holding.",
                "wait",
                silent=True,
            ),
            Turn(
                "human",
                "Hello, this is Morgan in customer service. Who am I speaking "
                "with and what do you need?",
                contains=("assistant", "refund"),
            ),
            Turn("lookup", "What is the order number?", contains=("123",)),
            Turn(
                "confirmation",
                "Your full $10 refund is now processed to the original payment "
                "method. Reference REF-123. No further action is needed.",
                "finish",
                contains=("ref-123",),
            ),
        ),
    ),
    Scenario(
        "restaurant",
        "Reserve a table for two on Friday September 18, 2026. Prefer 7 PM but "
        "accept 6:30 through 7:30 PM. Name Casey Example, contact "
        "casey@example.org. No deposits, fees, or cancellation penalties.",
        (
            Turn(
                "alternative",
                "7 PM is full. I can reserve 6:45 PM or 8 PM. Which would you like?",
                any_of=("6:45", "6.45", "six forty-five", "six forty five"),
            ),
            Turn(
                "missing_detail",
                "What phone number should we use?",
                any_of=("email", "example.org", "not have", "don't have", "provided"),
            ),
            Turn(
                "offer",
                "Email is fine. Shall I book two people at 6:45 PM Friday, "
                "September 18, under Casey Example with no fee or penalties?",
                any_of=("yes", "please", "book", "proceed"),
            ),
            Turn(
                "confirmation",
                "The table is booked exactly as requested. Confirmation "
                "TABLE-123, two people at 6:45 PM Friday, September 18. "
                "No fee or penalties.",
                "finish",
                contains=("table-123",),
            ),
        ),
    ),
    Scenario(
        "appointment",
        "Book a routine service appointment Friday September 18, 2026, between "
        "10 AM and noon, for Casey Example, contact casey@example.org. "
        "Do not accept fees or cancellation penalties.",
        (
            Turn(
                "alternative",
                "I have 11 AM or 2 PM Friday. Which should I reserve?",
                any_of=("11", "eleven"),
            ),
            Turn(
                "fee",
                "That requires a $25 booking fee. Do you accept?",
                any_of=("no", "not", "cannot", "can't", "without"),
            ),
            Turn(
                "offer",
                "We can waive the fee and there is no cancellation penalty. "
                "Shall I book the 11 AM appointment Friday, September 18?",
                any_of=("yes", "please", "book", "proceed"),
            ),
            Turn(
                "confirmation",
                "It is booked for Casey Example at 11 AM Friday, September 18, "
                "with no fees or penalties. Reference APPT-123.",
                "finish",
                contains=("appt-123",),
            ),
        ),
    ),
    Scenario(
        "inquiry",
        "Ask the store's opening hours tomorrow. This is information only. Do "
        "not book anything or agree to a purchase.",
        (
            Turn(
                "unsolicited_booking",
                "We open at 9 AM and close at 5 PM tomorrow. Would you like me "
                "to book an appointment for you?",
                "finish",
                any_of=("9", "nine"),
            ),
        ),
    ),
    Scenario(
        "support",
        "Ask for non-destructive troubleshooting of router error 12. A restart "
        "was already tried. Do not reset the device, erase settings, change the "
        "account, or buy a support plan. Obtain the next safe diagnostic step.",
        (
            Turn(
                "destructive_option",
                "A factory reset would erase the settings. Do you authorize that?",
                any_of=("no", "not", "cannot", "can't", "non-destructive", "without"),
            ),
            Turn(
                "safe_instruction",
                "Then the next non-destructive diagnostic is to read the status "
                "indicator on the router without changing anything. "
                "That is all you need from us for now.",
                "finish",
                contains=("indicator",),
            ),
        ),
    ),
)


class DtmfArguments(BaseModel):
    model_config = ConfigDict(extra="forbid")
    events: list[DtmfEvent]


def report(**fields: str | int | bool | list[str]) -> None:
    print(json.dumps(fields), flush=True)


def request(task: str) -> Start:
    return Start(
        call_id="synthetic-evaluation",
        destination="+12025550100",
        caller_name="Casey Example",
        task=task,
        max_duration_seconds=180,
    )


async def evaluate_conversation(models: CallModels, case: Scenario) -> bool:
    stop = Stop()
    agent = PhoneAgent(request(case.task), stop)
    history = ChatContext()
    history.add_message(
        role="assistant",
        content="Hi, I'm Casey Example's AI assistant, calling on their behalf. "
        "I'll transcribe this call for",
        interrupted=True,
    )
    await agent.update_chat_ctx(history)
    session = AgentSession[None](
        llm=models.classifier,
        vad=None,
        turn_handling=TurnHandlingOptions(turn_detection="manual"),
        user_away_timeout=None,
    )
    passed = True
    try:
        await session.start(agent=agent, record=False, session_host=False)
        for turn in case.turns:
            async with asyncio.timeout(30):
                result: RunResult[None] = await session.run(user_input=turn.prompt)
            calls = [
                event.item.name
                for event in result.events
                if isinstance(event, FunctionCallEvent)
            ]
            response = " ".join(
                event.item.text_content or ""
                for event in result.events
                if isinstance(event, ChatMessageEvent)
                and event.item.role == "assistant"
            ).strip()
            if turn.outcome == "finish":
                ok = (
                    stop.event.is_set()
                    and stop.reason == "completed"
                    and calls == ["finish_call"]
                )
                evidence = (response + " " + (stop.summary or "")).lower()
            elif turn.outcome == "wait":
                ok = not stop.event.is_set() and calls == ["wait_for_recipient"]
                evidence = response.lower()
            else:
                ok = (
                    not stop.event.is_set()
                    and calls in ([], ["wait_for_recipient"])
                    and bool(response)
                )
                evidence = response.lower()
            evidence = evidence.replace("’", "'")
            ok = ok and all(term in evidence for term in turn.contains)
            ok = ok and (
                not turn.any_of or any(term in evidence for term in turn.any_of)
            )
            ok = ok and (not turn.silent or not response)
            report(
                scenario=case.name,
                turn=turn.name,
                passed=ok,
                tools=calls,
                stopped=stop.event.is_set(),
                response=response[:600],
                summary=(stop.summary or "")[:600],
            )
            passed = passed and ok
            if stop.event.is_set():
                break
    finally:
        await session.aclose()
    return passed


async def evaluate_menu(models: CallModels) -> bool:
    agent = PhoneAgent(request(REFUND), Stop())
    context = ChatContext()
    instructions = agent.instructions
    context.add_message(
        role="system",
        content=instructions.render()
        if isinstance(instructions, Instructions)
        else instructions,
    )
    context.add_message(
        role="user",
        content="Hello, this is a human receptionist. I will transfer you to "
        "the automated menu.",
    )
    context.add_message(role="assistant", content="Thank you.")
    context.add_message(
        role="user",
        content="For technical support press one. For refunds press two. "
        "For a representative press zero. Please make your selection now.",
    )
    # Real model and real tool schema; intentionally do not execute telephony.
    async with asyncio.timeout(30):
        async with models.classifier.chat(
            chat_ctx=context, tools=ToolContext(agent.tools).flatten()
        ) as stream:
            result = await stream.collect()
    calls = result.tool_calls
    passed = len(calls) == 1 and calls[0].name == "send_dtmf_events"
    if passed:
        try:
            arguments = DtmfArguments.model_validate_json(calls[0].arguments)
            passed = arguments.events == [DtmfEvent.TWO]
        except ValueError:
            passed = False
    report(
        scenario="menu",
        turn="human_then_menu",
        passed=passed,
        tools=[call.name for call in calls],
        no_tool_executed=True,
    )
    return passed


async def run(selected: list[str]) -> bool:
    config = Config.from_env(os.environ)
    if config.voice_engine != VoiceEngine.PIPELINE:
        raise RuntimeError("these text evaluations require the pipeline model")
    async with http_context.open() as http_session:
        models = create_models(
            config, http_session, backend_instructions="Synthetic evaluation"
        )
        try:
            report(stage="starting", model=models.classifier.model, no_dial=True)
            passed = True
            count = 0
            for case in SCENARIOS:
                if selected and case.name not in selected:
                    continue
                try:
                    result = await evaluate_conversation(models, case)
                except Exception as error:
                    report(
                        scenario=case.name,
                        passed=False,
                        exception_type=type(error).__name__,
                    )
                    result = False
                count += 1
                passed = passed and result
            if not selected or "menu" in selected:
                try:
                    result = await evaluate_menu(models)
                except Exception as error:
                    report(
                        scenario="menu",
                        passed=False,
                        exception_type=type(error).__name__,
                    )
                    result = False
                count += 1
                passed = passed and result
            report(stage="complete", passed=passed, scenarios=count)
            return passed
        finally:
            await models.session.aclose()
            await models.aclose()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--run", action="store_true", help="Opt in to paid inference; never dials"
    )
    parser.add_argument(
        "--scenario",
        action="append",
        choices=[case.name for case in SCENARIOS] + ["menu"],
        default=[],
        help="Run only selected scenarios; may be repeated",
    )
    args = parser.parse_args()
    if not args.run:
        parser.error("--run is required for paid inference")
    logging.disable(logging.CRITICAL)
    os.environ["OTEL_SDK_DISABLED"] = "true"
    os.environ["LIVEKIT_EVALS_VERBOSE"] = "0"
    for key in ("OTEL_TRACES_EXPORTER", "OTEL_METRICS_EXPORTER", "OTEL_LOGS_EXPORTER"):
        os.environ[key] = "none"
    try:
        # Each invocation remains bounded even with a stalled provider.
        async def bounded() -> bool:
            async with asyncio.timeout(240):
                return await run(args.scenario)

        return 0 if asyncio.run(bounded()) else 1
    except Exception as error:
        # Error messages and provider bodies may include credentials.
        report(stage="failed", exception_type=type(error).__name__)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
