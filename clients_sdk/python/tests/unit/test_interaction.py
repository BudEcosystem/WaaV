"""Interaction mode (protocol 1.1) on ``/ws``: config, ready echo, typed events, client
messages and the task list.

The event frames are the gateway's own wire examples (``gateway/src/core/interaction/events.rs``
tests), so a renamed field on either side fails here.
"""

from __future__ import annotations

import json
from typing import Any
from unittest.mock import AsyncMock, MagicMock

import pytest

from bud_waav import (
    AgentAudioStopped,
    AssistantTranscript,
    AudioSegment,
    BudClient,
    FloorState,
    InteractionUpdated,
    SessionFailed,
    TaskAnswerReceived,
    TaskCancelled,
    TaskCreated,
    TaskProgress,
    TaskQuestion,
    TaskOutput,
    TaskResult,
    TaskView,
    UnverifiedClaim,
    parse_interaction_event,
)
from bud_waav.pipelines.talk import TalkSession
from bud_waav.rest.client import RestClient
from bud_waav.types import ConversationConfig
from bud_waav.ws.session import WebSocketSession

# One frame per event, as the gateway sends it.
WIRE: dict[str, tuple[type, str]] = {
    "task_created": (TaskCreated, '{"type": "task_created", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "working", "cancelling": false, "title": "your table booking", "kind": "action", "side_effects": true, "speculative": true, "origin": "front", "background": "agent:support", "t_ms": 4180}'),
    "task_progress": (TaskProgress, '{"type": "task_progress", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 2, "state": "working", "cancelling": false, "seq": 4, "text": null, "fraction": null, "spoken": false, "effects": null, "answer_stopped": {"reason": "revised", "released_sentences": 1}, "t_ms": 8350}'),
    "task_question": (TaskQuestion, '{"type": "task_question", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "input_required", "cancelling": false, "request_id": "q_task_5b1e9c0d_2", "kind": "choose", "prompt": "Would you like 7 pm or 8 pm?", "options": ["7 pm", "8 pm"], "schema": null, "sensitive": false, "irreversible": false, "channels": ["voice", "client"], "expires_at_ms": 61000, "spoken": true, "heard": "pending", "t_ms": 31000}'),
    "task_answer_received": (TaskAnswerReceived, '{"type": "task_answer_received", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "working", "cancelling": false, "request_id": "q_task_5b1e9c0d_1", "outcome": "decline", "reason": null, "correction": "make it Tuesday", "by": "voice", "corroborated": false, "value": null, "value_redacted": false, "t_ms": 12400}'),
    "task_result": (TaskResult, '{"type": "task_result", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "completed", "status": "completed", "cancelling": false, "summary": "Booked: table for 4, Saturday 19:00, confirmation R-5512", "effects": ["table booking at Rosa\'s"], "spoken": "full", "streamed": true, "superseded": false, "t_ms": 13900}'),
    "task_output": (TaskOutput, '{"type": "task_output", "task_id": "task_9f2c", "task_ref": "t2", "revision": 1, "state": "completed", "text": "Booked AI 101 on Friday at 9:40.", "data": {"flight": "AI 101"}, "artifacts": [{"url": "https://air.example.com/t/AI101"}], "effects": "none_committed", "t_ms": 21400, "parent_id": "task_5b1e", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}'),
    "task_cancelled": (TaskCancelled, '{"type": "task_cancelled", "task_id": "task_77aa01fe", "task_ref": "t2", "revision": 1, "state": "cancelled", "cancelling": false, "by": "client", "reason": "client_cancelled", "effects": "none_committed", "t_ms": 15200}'),
    "floor_state": (FloorState, '{"type": "floor_state", "holder": "none", "lane": null, "reason": "stream_gap", "t_ms": 13240}'),
    "audio_segment": (AudioSegment, '{"type": "audio_segment", "phase": "start", "utterance_id": "u_41", "lane": "front", "kind": "ack", "task_id": null, "t_ms": 12100}'),
    "agent_audio_stopped": (AgentAudioStopped, '{"type": "agent_audio_stopped", "utterance_ids": ["u_44", "u_45"], "reason": "policy", "delivered_ms": [600, 0], "t_ms": 20400}'),
    "interaction_updated": (InteractionUpdated, '{"type": "interaction_updated", "applied": ["front.caller.verbosity"], "refused": [{"path": "regime", "reason": "not_mid_session"}]}'),
    "assistant_transcript": (AssistantTranscript, '{"type": "assistant_transcript", "turn_index": 3, "delta": "Booked: a table for 4 at Rosa\'s on Saturday at 7 pm.", "source": "background", "kind": "relay", "verbatim": true, "task_id": "task_5b1e9c0d", "utterance_id": "u_42", "sentence": {"attempt": 0, "seq": 0}, "heard": "full"}'),
    "session_failed": (SessionFailed, '{"type": "session_failed", "service": "background", "reason": "no answer for 30 s after the first stall line", "pending": [{"task_id": "task_5b1e9c0d", "task_ref": "t1", "title": "what time does the store open", "state": "working"}], "follow_up": false, "t_ms": 40210}'),
    "unverified_claim": (UnverifiedClaim, '{"type": "unverified_claim", "kind": "completion", "rule": "CompletionClaim", "text": "Your table is booked.", "corrected": true, "output_stopped": false, "t_ms": 9000}'),
}

READY_1_1 = {
    "type": "ready",
    "protocol_version": "1.1",
    "stream_id": "call-1",
    "interaction": {
        "enabled": True,
        "protocol_version": "1.1",
        "regime": "cascade",
        "front": {"persona": "default"},
        "tasks": {"backend": "llm", "speculative_start": "auto", "tools": ["book_table"],
                  "reconnect_grace_s": 60, "push": False},
        "events": {"floor": True, "segments": True},
        "hitl": {"channels": ["voice", "client"]},
        "degraded": [],
    },
}


async def _receive(session: WebSocketSession, *frames: Any) -> None:
    """Feed frames through the session's own receive loop."""

    async def stream():
        for frame in frames:
            yield frame if isinstance(frame, str) else json.dumps(frame)

    session._ws = stream()
    session._connected = True
    await session._receive_loop()


async def _sent_by(session: WebSocketSession, call) -> list[dict[str, Any]]:
    """What `call` writes to a live socket."""
    sent: list[str] = []
    session._ws = MagicMock()
    session._ws.send = AsyncMock(side_effect=sent.append)
    session._connected = True
    await call()
    return [json.loads(frame) for frame in sent]


class TestConfig:
    @pytest.mark.asyncio
    @pytest.mark.parametrize("value", [True, False, {"policy": {"ack_due_ms": 250}}])
    async def test_interaction_is_sent_as_given(self, value):
        session = WebSocketSession(url="ws://x/ws", interaction=value)
        [config] = await _sent_by(session, session._send_config)
        assert config["interaction"] == value

    @pytest.mark.asyncio
    async def test_no_interaction_key_without_the_value(self):
        session = WebSocketSession(url="ws://x/ws")
        [config] = await _sent_by(session, session._send_config)
        assert "interaction" not in config

    @pytest.mark.asyncio
    async def test_tool_schema_keys_are_not_renamed(self):
        tool = {"name": "book_table", "url": "https://t.example/book", "side_effects": True,
                "parameters": {"type": "object", "properties": {"party_size": {"type": "integer"}},
                               "required": ["party_size"]}}
        block = {"tasks": {"tools": [tool]}, "hitl": {"channels": ["voice", "client"]}}
        session = WebSocketSession(url="ws://x/ws", interaction=block)
        [config] = await _sent_by(session, session._send_config)
        assert config["interaction"] == block

    @pytest.mark.asyncio
    async def test_agent_and_talk_carry_it_to_the_wire(self):
        bud = BudClient(base_url="http://127.0.0.1:3001")
        talk = bud.agent(llm={"base_url": "http://llm/v1", "model": "m"}, interaction=True)
        [config] = await _sent_by(talk._session, talk._session._send_config)
        assert config["interaction"] is True
        assert config["conversation_config"]["model"] == "m"
        talk = bud.talk.create(interaction={"events": {"floor": False}})
        assert talk._session.interaction == {"events": {"floor": False}}


class TestReady:
    @pytest.mark.asyncio
    async def test_protocol_1_1_and_the_echo(self):
        session = WebSocketSession(url="ws://x/ws")
        warnings: list[Any] = []
        session.on("warning", warnings.append)
        await _receive(session, READY_1_1)
        assert session.protocol_version == "1.1"
        assert session.interaction_settings == READY_1_1["interaction"]
        assert warnings == []  # same major: no mismatch

    @pytest.mark.asyncio
    async def test_a_plain_session_has_no_echo(self):
        session = WebSocketSession(url="ws://x/ws")
        await _receive(session, {"type": "ready", "protocol_version": "1.0", "stream_id": "s"})
        assert session.protocol_version == "1.0"
        assert session.interaction_settings is None


class TestEvents:
    @pytest.mark.parametrize("name", sorted(WIRE))
    def test_every_event_parses_to_its_class(self, name):
        cls, frame = WIRE[name]
        event = parse_interaction_event(json.loads(frame))
        assert isinstance(event, cls)
        assert event.type == name

    def test_fields(self):
        question = parse_interaction_event(json.loads(WIRE["task_question"][1]))
        assert isinstance(question, TaskQuestion)
        assert (question.request_id, question.kind, question.options) == (
            "q_task_5b1e9c0d_2", "choose", ["7 pm", "8 pm"])
        assert question.channels == ["voice", "client"] and question.schema_ is None
        result = parse_interaction_event(json.loads(WIRE["task_result"][1]))
        assert isinstance(result, TaskResult)
        assert result.effects == ["table booking at Rosa's"] and result.streamed
        output = parse_interaction_event(json.loads(WIRE["task_output"][1]))
        assert isinstance(output, TaskOutput)
        assert (output.text, output.data, output.agent) == (
            "Booked AI 101 on Friday at 9:40.", {"flight": "AI 101"}, "browser")
        assert output.artifacts == [{"url": "https://air.example.com/t/AI101"}]
        cancelled = parse_interaction_event(json.loads(WIRE["task_cancelled"][1]))
        assert isinstance(cancelled, TaskCancelled) and cancelled.effects == "none_committed"
        progress = parse_interaction_event(json.loads(WIRE["task_progress"][1]))
        assert isinstance(progress, TaskProgress)
        assert progress.answer_stopped is not None and progress.answer_stopped.reason == "revised"
        updated = parse_interaction_event(json.loads(WIRE["interaction_updated"][1]))
        assert isinstance(updated, InteractionUpdated)
        assert [(r.path, r.reason) for r in updated.refused] == [("regime", "not_mid_session")]
        assert updated.degraded == [] and updated.failure is None
        failing = parse_interaction_event({
            "type": "interaction_updated", "applied": [], "refused": [],
            "degraded": ["background_failing"],
            "failure": {"line": "stall-1", "kind": "stall", "service": "background",
                        "text": "Sorry, my systems are acting up. Bear with me a moment."}})
        assert isinstance(failing, InteractionUpdated) and failing.failure is not None
        assert (failing.failure.line, failing.failure.kind, failing.failure.service) == (
            "stall-1", "stall", "background")
        failed = parse_interaction_event(json.loads(WIRE["session_failed"][1]))
        assert isinstance(failed, SessionFailed)
        assert (failed.service, failed.follow_up) == ("background", False)
        assert [(p.task_id, p.state) for p in failed.pending] == [("task_5b1e9c0d", "working")]

    def test_a_child_task_names_its_parent_agent_and_trace(self):
        frame = json.loads(WIRE["task_created"][1])
        frame.update(parent_id="task_parent", agent="browser",
                     trace_id="4bf92f3577b34da6a3ce929d0e0e4736")
        child = parse_interaction_event(frame)
        assert isinstance(child, TaskCreated)
        assert (child.parent_id, child.agent, child.trace_id) == (
            "task_parent", "browser", "4bf92f3577b34da6a3ce929d0e0e4736")
        assert child.model_extra == {}
        top = parse_interaction_event(json.loads(WIRE["task_created"][1]))
        assert (top.parent_id, top.agent, top.trace_id) == (None, None, "")

    def test_a_form_schema_and_an_unknown_field_are_kept(self):
        frame = json.loads(WIRE["task_question"][1])
        frame.update(kind="form", schema={"type": "object"}, url=None, new_field=7)
        event = parse_interaction_event(frame)
        assert isinstance(event, TaskQuestion)
        assert event.schema_ == {"type": "object"}
        assert event.model_extra == {"new_field": 7}

    def test_not_interaction_events(self):
        # A voice agent's line without the mode has no `source`: it stays a plain message.
        assert parse_interaction_event({"type": "assistant_transcript", "turn_index": 1, "delta": "Hi"}) is None
        assert parse_interaction_event({"type": "stt_result", "transcript": "x"}) is None
        # A frame that does not fit its class (no request_id) is not typed.
        assert parse_interaction_event({"type": "task_question", "task_id": "t"}) is None

    @pytest.mark.asyncio
    async def test_the_session_emits_and_queues_typed_events(self):
        session = WebSocketSession(url="ws://x/ws")
        seen: list[Any] = []
        others: list[Any] = []
        session.on("interaction", seen.append)
        session.on("server_message", others.append)
        plain_line = {"type": "assistant_transcript", "turn_index": 1, "delta": "Hi"}
        await _receive(session, *(frame for _, frame in WIRE.values()), plain_line)
        assert [type(event) for event in seen] == [cls for cls, _ in WIRE.values()]
        assert others == [plain_line]
        queue = session._get_message_queue()
        queued = [queue.get_nowait() for _ in range(queue.qsize())]
        interaction = [item for item in queued if item["type"] == "interaction"]
        assert len(interaction) == len(WIRE)
        assert interaction[0]["event"].type == "task_created"
        assert interaction[0]["data"]["task_id"] == "task_5b1e9c0d"


class TestClientMessages:
    @pytest.mark.asyncio
    async def test_wire_of_each_message(self):
        session = WebSocketSession(url="ws://x/ws")

        async def calls():
            await session.answer_task("q1")
            await session.answer_task("q2", "accept", value="8 pm")
            await session.answer_task("q3", "decline", correction="make it four")
            await session.answer_task("q4", "cancel")
            await session.cancel_task("task_1")
            await session.mark_task_seen("task_2")
            await session.update_interaction({"policy": {"hold_after_silence_ms": 5000}})
            await session.report_playback("u_41", 1320)

        assert await _sent_by(session, calls) == [
            {"type": "task_answer", "request_id": "q1", "outcome": "accept"},
            {"type": "task_answer", "request_id": "q2", "outcome": "accept", "value": "8 pm"},
            {"type": "task_answer", "request_id": "q3", "outcome": "decline", "correction": "make it four"},
            {"type": "task_answer", "request_id": "q4", "outcome": "cancel"},
            {"type": "task_cancel", "task_id": "task_1"},
            {"type": "task_seen", "task_id": "task_2"},
            {"type": "interaction_update", "patch": {"policy": {"hold_after_silence_ms": 5000}}},
            {"type": "playback_position", "utterance_id": "u_41", "played_ms": 1320},
        ]

    @pytest.mark.asyncio
    async def test_a_timeout_is_not_a_client_answer(self):
        session = WebSocketSession(url="ws://x/ws")
        with pytest.raises(ValueError):
            await session.answer_task("q1", "timeout")

    @pytest.mark.asyncio
    async def test_offline_messages_are_queued_like_other_controls(self):
        session = WebSocketSession(url="ws://x/ws")
        await session.cancel_task("task_1")
        assert session._control_queue.size == 1

    @pytest.mark.asyncio
    async def test_talk_session_delegates(self):
        talk = TalkSession(url="ws://x/ws", conversation_config=ConversationConfig(base_url="b", model="m"),
                           interaction=True)

        async def calls():
            await talk.answer_task("q1", "accept", value={"party": 2})
            await talk.cancel_task("task_1")
            await talk.update_interaction({"events": {"floor": False}})
            await talk.report_playback("u_1", 250)

        sent = await _sent_by(talk._session, calls)
        assert [m["type"] for m in sent] == [
            "task_answer", "task_cancel", "interaction_update", "playback_position"]
        assert sent[0]["value"] == {"party": 2}


class TestTalkStream:
    @pytest.mark.asyncio
    async def test_talk_yields_interaction_events(self):
        talk = TalkSession(url="ws://x/ws", interaction=True)
        heard: list[Any] = []
        talk.on("interaction", heard.append)
        await _receive(talk._session, READY_1_1, WIRE["task_created"][1], WIRE["task_result"][1])
        assert [event.type for event in heard] == ["task_created", "task_result"]
        assert talk.interaction_settings == READY_1_1["interaction"]
        talk._session._connected = False
        events = [event async for event in talk]
        kinds = [(e.type, e.interaction.type if e.interaction else None) for e in events]
        assert kinds == [("interaction", "task_created"), ("interaction", "task_result")]


class TestTaskList:
    @pytest.mark.asyncio
    async def test_list_and_get(self):
        rest = RestClient(base_url="http://127.0.0.1:3001")
        listed = {"tasks": [{"id": "task_1", "task_ref": "t1", "revision": 1, "title": "your table booking",
                             "state": "input_required", "cancelling": False, "effects": "none_committed",
                             "open_question": "q_task_1_1", "side_effects": True, "started_at": 4180}]}
        one = {**listed["tasks"][0], "state": "completed", "effects": ["booked a table for 2"],
               "open_question": None, "stream_id": "call-1",
               "result": {"state": "completed", "summary": "Booked.", "effects": ["booked a table for 2"]}}
        rest.get = AsyncMock(side_effect=[listed, {"tasks": []}, one])
        tasks = await rest.list_tasks(stream_id="call-1")
        assert [(t.id, t.state, t.open_question) for t in tasks] == [("task_1", "input_required", "q_task_1_1")]
        assert await rest.list_tasks() == []
        task = await rest.get_task("task_1", stream_id="call-1")
        assert isinstance(task, TaskView)
        assert task.result is not None and task.result.summary == "Booked."
        assert task.stream_id == "call-1" and task.effects == ["booked a table for 2"]
        assert [c.args + (c.kwargs.get("params"),) for c in rest.get.call_args_list] == [
            ("/v1/tasks", {"stream_id": "call-1"}),
            ("/v1/tasks", None),
            ("/v1/tasks/task_1", {"stream_id": "call-1"}),
        ]

    @pytest.mark.asyncio
    async def test_the_client_delegates(self):
        bud = BudClient(base_url="http://127.0.0.1:3001")
        bud._rest_client.get = AsyncMock(return_value={"tasks": []})
        assert await bud.list_tasks(stream_id="s") == []
