"""Process entry point. Stdout is exclusively the private NDJSON protocol."""

from __future__ import annotations

import asyncio
import contextlib
import logging
import os
import signal
import sys

from livekit_phone.protocol import (
    MAX_COMMAND_BYTES,
    Cancel,
    Completed,
    Error,
    EventSink,
    Lifecycle,
    Start,
    parse_command,
)


async def serve(output: EventSink) -> int:
    from livekit.agents.utils import http_context

    from livekit_phone.config import Config, ConfigurationError
    from livekit_phone.control import Stop
    from livekit_phone.runtime import Call

    stop = Stop()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop.request, "cancelled")
    reader = asyncio.StreamReader(limit=MAX_COMMAND_BYTES)
    transport, _ = await loop.connect_read_pipe(
        lambda: asyncio.StreamReaderProtocol(reader), sys.stdin.buffer
    )
    request: asyncio.Future[Start] = loop.create_future()

    async def receive() -> None:
        while not stop.event.is_set():
            try:
                line = await reader.readline()
                if not line:
                    stop.request("cancelled")
                    return
                command = parse_command(line)
                if isinstance(command, Cancel):
                    stop.request("cancelled")
                    return
                if request.done():
                    raise ValueError("only one start per worker")
                request.set_result(command)
            except (ValueError, OSError):
                output.emit(
                    Error(
                        code="invalid_command",
                        message="Invalid worker protocol message.",
                    )
                )
                stop.request("failed")
                return

    output.emit(Lifecycle(type="ready"))
    received = asyncio.create_task(receive())
    stopped = asyncio.create_task(stop.event.wait())
    try:
        await asyncio.wait((request, stopped), return_when=asyncio.FIRST_COMPLETED)
        if stop.event.is_set():
            output.emit(Completed(reason=stop.reason, remote_hangup_confirmed=True))
        else:
            try:
                config = Config.from_env(os.environ)
            except ConfigurationError as error:
                output.emit(Error(code="configuration", message=str(error)))
                output.emit(Completed(reason="failed", remote_hangup_confirmed=True))
                return 1
            except Exception:
                output.emit(
                    Error(
                        code="startup_failed",
                        message="The voice runtime could not start.",
                    )
                )
                output.emit(Completed(reason="failed", remote_hangup_confirmed=True))
                return 1
            async with http_context.open() as http_session:
                call = Call(request.result(), config, stop, output, http_session)
                await call.run()
        return 1 if stop.reason == "failed" else 0
    finally:
        received.cancel()
        stopped.cancel()
        await asyncio.gather(received, stopped, return_exceptions=True)
        transport.close()


async def check(output: EventSink) -> int:
    from livekit.agents.utils import http_context

    from livekit_phone.config import Config
    from livekit_phone.models import create_models

    async with http_context.open() as http_session:
        config = Config(
            url="wss://offline-check.livekit.cloud",
            api_key="offline-check",
            api_secret="offline-check-not-a-secret-at-least-32-characters",
            trunk_id="offline-check",
            openai_api_key="offline-check-not-a-secret",
        )
        models = create_models(
            config, http_session, backend_instructions="Offline construction only."
        )
        await models.session.aclose()
        await models.aclose()
    output.emit(
        Completed(
            reason="completed",
            remote_hangup_confirmed=True,
            summary="Offline SDK check passed. No connection or call was made.",
        )
    )
    return 0


def main() -> None:
    # SDK logs can contain transcripts, credentials embedded in failed URLs, or
    # provider replies. Emit only our fixed diagnostics; transcripts use the pipe.
    logging.disable(logging.CRITICAL)
    os.environ["OTEL_SDK_DISABLED"] = "true"
    os.environ["OTEL_TRACES_EXPORTER"] = "none"
    os.environ["OTEL_METRICS_EXPORTER"] = "none"
    os.environ["OTEL_LOGS_EXPORTER"] = "none"
    os.environ["LK_OPENAI_DEBUG"] = "0"
    output = EventSink(sys.stdout)
    if sys.argv[1:] not in ([], ["check"]):
        output.emit(Error(code="usage", message="Usage: livekit-phone-worker [check]"))
        raise SystemExit(2)
    try:
        result = asyncio.run(check(output) if sys.argv[1:] else serve(output))
    except (KeyboardInterrupt, BrokenPipeError):
        result = 1
    except Exception:
        output.emit(
            Error(
                code="worker_failed", message="The voice worker stopped unexpectedly."
            )
        )
        # No remote confirmation is possible after an unexpected top-level fault.
        output.emit(Completed(reason="failed", remote_hangup_confirmed=False))
        result = 1
    with contextlib.suppress(OSError):
        sys.stdout.flush()
    raise SystemExit(result)


if __name__ == "__main__":
    main()
