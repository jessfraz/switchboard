"""One outbound call, with explicit server lifetime and best-effort cleanup."""

from __future__ import annotations

import asyncio
import contextlib
from datetime import timedelta

import aiohttp
from google.protobuf.duration_pb2 import Duration
from livekit import api, rtc
from livekit.agents import (
    AMD,
    AMDCategory,
    CloseEvent,
    ConversationItemAddedEvent,
    room_io,
)
from livekit.agents.voice import TranscriptSynchronizer

from livekit_phone.agent import PhoneAgent, call_instructions
from livekit_phone.config import Config, ConfigurationError, VoiceEngine
from livekit_phone.control import Stop, transcript_event
from livekit_phone.models import create_models
from livekit_phone.protocol import (
    Completed,
    CompletionReason,
    Error,
    EventSink,
    Lifecycle,
    Start,
)


class Call:
    def __init__(
        self,
        request: Start,
        config: Config,
        stop: Stop,
        output: EventSink,
        http_session: aiohttp.ClientSession,
    ) -> None:
        self.request = request
        self.config = config
        self.stop = stop
        self.output = output
        self.room_name = f"phone-{request.call_id}"
        self.recipient_identity = f"recipient-{request.call_id}"
        self.room = rtc.Room()
        self.models = create_models(
            config,
            http_session,
            backend_instructions=(
                call_instructions(request)
                + "\nYou are the backend handling delegated work for the voice model. "
                "Use require_approval immediately for any objection to consent or "
                "need for extra authorization. Use finish_call when the voice model "
                "has said goodbye and delegates ending the call. Return concise "
                "answers or instructions for a professional executive assistant, "
                "never start another task. Carefully check reasoning and calculations. "
                "Resolve ambiguity only within the authorized task; otherwise "
                "request clarification. Provide conclusions and relevant uncertainty, "
                "never internal deliberation or a spoken reasoning monologue."
            ),
        )
        self.session = self.models.session
        self.transcript_sync: TranscriptSynchronizer | None = None
        self.client = api.LiveKitAPI(
            config.url,
            config.api_key,
            config.api_secret,
            timeout=aiohttp.ClientTimeout(total=10),
            failover=False,
        )
        self.dial_started = False
        self.dial_resolved = False
        self.recipient_gone = False
        self.room_created = False
        self.answered = asyncio.Event()
        self.first_agent_utterance = asyncio.Event()
        self._register_events()

    def _register_events(self) -> None:
        @self.session.on("conversation_item_added")
        def on_item(event: ConversationItemAddedEvent) -> None:
            transcript = transcript_event(event)
            if transcript:
                self.output.emit(transcript)
                if transcript.speaker == "agent":
                    self.first_agent_utterance.set()
            if self.output.closed:
                self.stop.request("cancelled")

        @self.session.on("close")
        def on_close(event: CloseEvent) -> None:
            if not self.stop.event.is_set():
                reason: CompletionReason = "failed" if event.error else "completed"
                self.stop.request(reason)

        @self.room.on("participant_disconnected")
        def on_participant_disconnected(participant: rtc.RemoteParticipant) -> None:
            if participant.identity == self.recipient_identity:
                self.recipient_gone = True
                if self.answered.is_set():
                    self.stop.request("completed", "The recipient ended the call.")
                else:
                    self.stop.request("failed", "The destination did not answer.")

        @self.room.on("participant_connected")
        def on_participant_connected(participant: rtc.RemoteParticipant) -> None:
            # The first participant update can already carry "active" if the
            # remote endpoint answers before our RTC subscription is established.
            if (
                participant.identity == self.recipient_identity
                and participant.attributes.get("sip.callStatus") == "active"
            ):
                self.answered.set()

        @self.room.on("disconnected")
        def on_disconnected(reason: rtc.DisconnectReason.ValueType) -> None:
            if not self.stop.event.is_set():
                self.stop.request("failed", "The call connection was lost.")

        @self.room.on("participant_attributes_changed")
        def on_attributes(
            changed: dict[str, str], participant: rtc.Participant
        ) -> None:
            if (
                participant.identity == self.recipient_identity
                and changed.get("sip.callStatus") == "active"
            ):
                self.answered.set()

    async def connect_and_dial(self) -> None:
        # Refuse a plaintext trunk before any call. The stored trunk is inspected,
        # never changed, and per-call SRTP is required below.
        trunks = await self.client.sip.list_sip_outbound_trunk(
            api.ListSIPOutboundTrunkRequest(trunk_ids=[self.config.trunk_id])
        )
        if len(trunks.items) != 1 or trunks.items[0].transport != api.SIP_TRANSPORT_TLS:
            raise ConfigurationError("The selected outbound trunk must require TLS.")
        if self.stop.event.is_set():
            return

        await self.client.room.create_room(
            api.CreateRoomRequest(
                name=self.room_name,
                empty_timeout=30,
                departure_timeout=10,
                max_participants=2,
            )
        )
        self.room_created = True
        if self.stop.event.is_set():
            return
        token = (
            api.AccessToken(self.config.api_key, self.config.api_secret)
            .with_identity(f"agent-{self.request.call_id}")
            .with_ttl(timedelta(seconds=self.request.max_duration_seconds + 120))
            .with_grants(
                api.VideoGrants(
                    room_join=True,
                    room=self.room_name,
                    can_publish=True,
                    can_subscribe=True,
                    can_publish_data=True,
                )
            )
            .to_jwt()
        )
        async with asyncio.timeout(20):
            await self.room.connect(self.config.url, token)
        if self.stop.event.is_set():
            return
        await self.session.start(
            agent=PhoneAgent(
                self.request, self.stop, self.output, self.config.voice_engine
            ),
            room=self.room,
            room_options=room_io.RoomOptions(
                participant_identity=self.recipient_identity,
                text_input=False,
                text_output=False,
                video_input=False,
                close_on_disconnect=False,
            ),
            record=False,
            session_host=False,
        )
        audio = self.session.output.audio
        if audio is None:
            raise RuntimeError("the voice session did not create an audio output")
        # RoomIO normally synchronizes text by publishing it to the room. Keep
        # its played-text correction, but use no downstream text transport.
        self.transcript_sync = TranscriptSynchronizer(
            next_in_chain_audio=audio, next_in_chain_text=None
        )
        self.session.output.audio = self.transcript_sync.audio_output
        self.session.output.transcription = self.transcript_sync.text_output
        if self.stop.event.is_set():
            return
        async with AMD(
            self.session,
            llm=self.models.classifier,
            stt=None,
            participant_identity=self.recipient_identity,
            ivr_detection=True,
        ) as detector:
            self.output.emit(Lifecycle(type="dialing"))
            self.dial_started = True
            # Never retry this mutation. If its reply is lost, the provider may
            # still be dialing; cleanup reports the resulting uncertainty.
            await self.client.sip.create_sip_participant(
                api.CreateSIPParticipantRequest(
                    sip_trunk_id=self.config.trunk_id,
                    sip_call_to=self.request.destination,
                    room_name=self.room_name,
                    participant_identity=self.recipient_identity,
                    participant_name="Business recipient",
                    wait_until_answered=False,
                    ringing_timeout=Duration(seconds=45),
                    max_call_duration=Duration(
                        seconds=self.request.max_duration_seconds
                    ),
                    media_encryption=api.SIP_MEDIA_ENCRYPT_REQUIRE,
                    hide_phone_number=True,
                )
            )
            self.dial_resolved = True
            if self.stop.event.is_set():
                return
            participant = self.room.remote_participants.get(self.recipient_identity)
            if participant and participant.attributes.get("sip.callStatus") == "active":
                self.answered.set()
            async with asyncio.timeout(50):
                await self.answered.wait()
            self.output.emit(Lifecycle(type="connected"))
            prediction = await detector.execute()
            if prediction.category in (
                AMDCategory.MACHINE_VM,
                AMDCategory.MACHINE_UNAVAILABLE,
            ):
                self.output.emit(
                    Error(code="no_person_reached", message="No person was reached.")
                )
                self.stop.request("failed", "No person reached; no message was left.")
                return
            if prediction.category in (AMDCategory.HUMAN, AMDCategory.UNCERTAIN):
                await self._open_conversation()
            await self.stop.event.wait()

    async def _open_conversation(self) -> None:
        if self.config.voice_engine == VoiceEngine.GPT_LIVE:
            # AMD gates playout, not GPT-Live generation. A native opening may
            # already be queued or committed. Sending generate_reply here adds
            # another greeting instruction and restarts the introduction.
            # Observe the same conversation events we retain for transcripts;
            # they can arrive after the audio or before AMD returns.
            async with asyncio.timeout(20):
                await self.first_agent_utterance.wait()
        else:
            await self.session.say(
                f"Hi, I'm {self.request.caller_name}'s assistant, calling on their "
                "behalf. I'll transcribe this call for notes. Is that okay?"
            )

    async def cleanup(self) -> bool:
        confirmed = not self.dial_started or self.recipient_gone
        # Stop speech and flush final committed transcript events before the
        # terminal event. The server-side cap remains if cleanup cannot connect.
        with contextlib.suppress(Exception):
            async with asyncio.timeout(10):
                await self.session.aclose()
        if self.transcript_sync is not None:
            with contextlib.suppress(Exception):
                async with asyncio.timeout(5):
                    await self.transcript_sync.aclose()
        if self.room_created or self.dial_started:
            try:
                await self.client.room.delete_room(
                    api.DeleteRoomRequest(room=self.room_name)
                )
                if not self.dial_started or self.dial_resolved:
                    confirmed = True
            except api.TwirpError as error:
                if error.code == "not_found" and self.dial_resolved:
                    confirmed = True
            except Exception:
                pass
        with contextlib.suppress(Exception):
            async with asyncio.timeout(5):
                await self.room.disconnect()
        with contextlib.suppress(Exception):
            async with asyncio.timeout(5):
                await self.models.aclose()
        await self.client.aclose()
        return confirmed

    async def run(self) -> None:
        operation = asyncio.create_task(self.connect_and_dial())
        stopped = asyncio.create_task(self.stop.event.wait())
        try:
            done, _ = await asyncio.wait(
                (operation, stopped),
                timeout=self.request.max_duration_seconds,
                return_when=asyncio.FIRST_COMPLETED,
            )
            if not done:
                self.stop.request("timeout", "The maximum call duration was reached.")
            elif operation in done:
                await operation
        except ConfigurationError as error:
            self.output.emit(Error(code="configuration", message=str(error)))
            self.stop.request("failed")
        except Exception:
            self.output.emit(
                Error(code="call_failed", message="The call could not continue.")
            )
            self.stop.request("failed")
        finally:
            # Let a short in-flight dial RPC settle before deleting its room.
            # Cancellation cannot undo a SIP mutation whose reply was lost.
            if not operation.done() and self.dial_started and not self.dial_resolved:
                with contextlib.suppress(Exception):
                    async with asyncio.timeout(12):
                        await asyncio.shield(operation)
            operation.cancel()
            stopped.cancel()
            await asyncio.gather(operation, stopped, return_exceptions=True)
            confirmed = await self.cleanup()
            self.output.emit(
                Completed(
                    reason=self.stop.reason,
                    summary=self.stop.summary,
                    remote_hangup_confirmed=confirmed,
                )
            )
