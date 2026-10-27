"""Interaction mode (wire protocol 1.1) on ``/ws``.

A session asks for the mode with ``interaction=True`` (the defaults) or an object of settings
(``gateway/docs/interaction_mode.md``, "Session Configuration Reference"). When the gateway runs
it, ``ready`` carries ``protocol_version: "1.1"`` and the resolved settings under
``interaction``, and the session receives the events below. Each event keeps the gateway's
wire field names; unknown fields are kept too (``model_extra``), so a newer gateway never
breaks parsing.

Source of truth: ``gateway/src/core/interaction/events.rs`` (``WireEvent``) and
``gateway/src/handlers/ws/messages.rs`` (the client messages).
"""

from __future__ import annotations

from typing import Any, Literal, get_args

from pydantic import BaseModel, ConfigDict, Field, ValidationError

INTERACTION_PROTOCOL_VERSION = "1.1"
"""The ``protocol_version`` of a session that runs interaction mode."""

Effects = str | list[str]
"""``"none_committed"``, ``"unknown"``, or the labels of what was done."""

TaskAnswerOutcome = Literal["accept", "decline", "cancel"]
"""What a client may answer a ``task_question`` with."""

TASK_ANSWER_OUTCOMES: tuple[str, ...] = ("accept", "decline", "cancel")


class _Wire(BaseModel):
    model_config = ConfigDict(extra="allow")


class _Event(_Wire):
    t_ms: int | None = None
    """Milliseconds since the session started."""


class _TaskEvent(_Event):
    task_id: str
    task_ref: str = ""
    state: str = ""
    """``submitted``, ``working``, ``input_required``, ``completed``, ``failed``,
    ``cancelled``, ``rejected`` or ``unknown``."""
    cancelling: bool = False
    parent_id: str | None = None
    """The task whose orchestrator started this one; ``None`` for a caller's request."""
    agent: str | None = None
    """The catalog entry that runs it; ``None`` on the session's own background."""
    trace_id: str = ""
    """One per caller's request, shared by the tasks it starts (W3C trace id)."""


class TaskCreated(_TaskEvent):
    """A request became a task."""

    type: Literal["task_created"] = "task_created"
    revision: int = 1
    title: str = ""
    kind: str = ""
    """``lookup`` or ``action``."""
    side_effects: bool = False
    speculative: bool = False
    origin: str = ""
    """``front``, or ``provider_tool`` on a speech-to-speech session."""
    background: str = ""
    """``llm``, ``agent:<name>``, ``dag`` or ``<type>:<name>``."""


class AnswerStopped(_Wire):
    reason: str
    released_sentences: int = 0


class TaskProgress(_TaskEvent):
    """The task moved: it started working, was revised, made progress, changed its effects,
    had its answer stopped, or a cancel was requested."""

    type: Literal["task_progress"] = "task_progress"
    revision: int = 1
    seq: int = 0
    text: str | None = None
    fraction: float | None = None
    spoken: bool = False
    effects: Effects | None = None
    answer_stopped: AnswerStopped | None = None


class TaskQuestion(_TaskEvent):
    """The task needs the caller: answer it with ``answer_task(request_id, ...)``."""

    type: Literal["task_question"] = "task_question"
    request_id: str
    kind: str = ""
    """``confirm``, ``choose``, ``value``, ``form`` or ``auth_url``."""
    prompt: str = ""
    options: list[str] | None = None
    schema_: dict[str, Any] | None = Field(default=None, alias="schema")
    """A form's JSON Schema (wire field ``schema``)."""
    url: str | None = None
    """The sign-in link of an ``auth_url`` question."""
    sensitive: bool = False
    irreversible: bool = False
    channels: list[str] = Field(default_factory=list)
    """Where it may be answered: ``["client"]`` means on the screen only."""
    expires_at_ms: int = 0
    spoken: bool = False
    heard: str = ""


class TaskAnswerReceived(_TaskEvent):
    """A question closed."""

    type: Literal["task_answer_received"] = "task_answer_received"
    request_id: str
    outcome: str = ""
    """``accept``, ``decline``, ``cancel``, ``timeout`` or ``superseded``."""
    reason: str | None = None
    correction: str | None = None
    by: str = ""
    """``voice``, ``client``, ``dtmf``, ``timeout``, ``session_end`` or ``policy``."""
    corroborated: bool = False
    value: Any = None
    value_redacted: bool = False


class TaskResult(_TaskEvent):
    """The task ended (or a replaced revision's call went through: ``superseded``)."""

    type: Literal["task_result"] = "task_result"
    revision: int = 1
    status: str = ""
    summary: str = ""
    """The answer of a completed task, otherwise what went wrong."""
    effects: Effects = "none_committed"
    spoken: str = ""
    """``full``, ``partial`` or ``none``."""
    streamed: bool = False
    superseded: bool = False


class TaskOutput(_TaskEvent):
    """A task completed: its whole result, published at once, before anything is said of it."""

    type: Literal["task_output"] = "task_output"
    revision: int = 1
    text: str = ""
    """The answer as the background wrote it."""
    data: Any = None
    """The backend's structured result, if it returned one."""
    artifacts: list[Any] = Field(default_factory=list)
    """Links and files the backend returned."""
    effects: Effects = "none_committed"


class TaskCancelled(_TaskEvent):
    """A cancel was confirmed."""

    type: Literal["task_cancelled"] = "task_cancelled"
    revision: int = 1
    by: str = ""
    reason: str = ""
    effects: Effects = "none_committed"


class FloorState(_Event):
    """Who is speaking changed (``events.floor``)."""

    type: Literal["floor_state"] = "floor_state"
    holder: str
    """``user``, ``assistant``, ``both`` or ``none``."""
    lane: str | None = None
    reason: str = ""


class AudioSegment(_Event):
    """An utterance's audio starts or ends in the stream (``events.segments``)."""

    type: Literal["audio_segment"] = "audio_segment"
    phase: str
    """``start`` or ``end``."""
    utterance_id: str
    lane: str = ""
    """``front``, ``main`` or ``backchannel``."""
    kind: str = ""
    task_id: str | None = None


class AgentAudioStopped(_Event):
    """Agent audio was cut: drop what is still queued for playback."""

    type: Literal["agent_audio_stopped"] = "agent_audio_stopped"
    utterance_ids: list[str] = Field(default_factory=list)
    reason: str = ""
    """``barge_in``, ``client_clear``, ``policy`` or ``provider``."""
    delivered_ms: list[int] = Field(default_factory=list)


class Refused(_Wire):
    path: str
    reason: str


class FailureNote(_Wire):
    """The line the caller hears because a service failed ("When Services Fail")."""

    line: str
    """The line's id in the pack, for example ``stall-1``."""
    kind: str
    """``stall``, ``hearing``, ``exit``, ``exit_follow_up`` or ``back``."""
    service: str
    """``recogniser``, ``front``, ``background`` or ``voice``."""
    text: str


class InteractionUpdated(_Event):
    """The reply to ``update_interaction``, or a capability was lost (``degraded``), or a
    failure line plays (``failure``)."""

    type: Literal["interaction_updated"] = "interaction_updated"
    applied: list[str] = Field(default_factory=list)
    refused: list[Refused] = Field(default_factory=list)
    degraded: list[str] = Field(default_factory=list)
    failure: FailureNote | None = None


class PendingTask(_Wire):
    """A task that was live when the session failed."""

    task_id: str
    task_ref: str = ""
    title: str = ""
    state: str = ""


class SessionFailed(_Event):
    """A service the call needs stayed down: the exit line was said, and the gateway closes
    the socket (close code 1011, reason ``session_failed``)."""

    type: Literal["session_failed"] = "session_failed"
    service: str
    """``recogniser``, ``front``, ``background`` or ``voice``."""
    reason: str = ""
    pending: list[PendingTask] = Field(default_factory=list)
    follow_up: bool = False
    """The exit line promised to get back to the caller."""


class Sentence(_Wire):
    attempt: int = 0
    seq: int = 0


class AssistantTranscript(_Event):
    """The agent said a line (its words in ``delta``; an empty ``delta`` closes the line and
    says how much of it was ``heard``)."""

    type: Literal["assistant_transcript"] = "assistant_transcript"
    turn_index: int = 0
    delta: str = ""
    source: str = ""
    """``front``, ``background`` or ``gateway``."""
    kind: str = ""
    """``ack``, ``relay``, ``confirm``, ``question``, ``progress``, ... (``backchannel`` never
    has a transcript)."""
    verbatim: bool = False
    act: str | None = None
    task_id: str | None = None
    utterance_id: str = ""
    sentence: Sentence | None = None
    heard: str = ""
    """``full``, ``partial`` or ``none``."""


class UnverifiedClaim(_Event):
    """Speech-to-speech sessions (``/realtime``): the provider said something on its own that
    no task supports, or did not say a gateway line as asked."""

    type: Literal["unverified_claim"] = "unverified_claim"
    kind: str = ""
    rule: str = ""
    text: str = ""
    corrected: bool = False
    output_stopped: bool = False


InteractionEvent = (
    TaskCreated
    | TaskProgress
    | TaskQuestion
    | TaskAnswerReceived
    | TaskResult
    | TaskOutput
    | TaskCancelled
    | FloorState
    | AudioSegment
    | AgentAudioStopped
    | InteractionUpdated
    | SessionFailed
    | AssistantTranscript
    | UnverifiedClaim
)

INTERACTION_EVENTS: dict[str, type[_Event]] = {
    model.model_fields["type"].default: model for model in get_args(InteractionEvent)
}
"""Wire ``type`` -> event class."""


def parse_interaction_event(data: dict[str, Any]) -> InteractionEvent | None:
    """The typed event of a server frame, or ``None`` when it is not an interaction event.

    An ``assistant_transcript`` without the 1.1 field ``source`` is a plain voice-agent line
    (the mode is off) and is not one. A frame that does not fit its class is ``None`` too, so
    the caller can still pass it on raw.
    """
    model = INTERACTION_EVENTS.get(str(data.get("type")))
    if model is None or (model is AssistantTranscript and "source" not in data):
        return None
    try:
        return model.model_validate(data)  # type: ignore[return-value]
    except ValidationError:
        return None


# ---- the task list (GET /v1/tasks)


class TaskOutcome(_Wire):
    state: str
    summary: str = ""
    effects: Effects = "none_committed"


class TaskView(_Wire):
    """A task as ``GET /v1/tasks`` and ``GET /v1/tasks/{task_id}`` show it."""

    id: str
    task_ref: str = ""
    revision: int = 1
    title: str = ""
    state: str = ""
    cancelling: bool = False
    effects: Effects = "none_committed"
    open_question: str | None = None
    """The ``request_id`` of the open question, if any."""
    side_effects: bool = False
    started_at: int = 0
    stream_id: str | None = None
    """Only from ``get_task``."""
    result: TaskOutcome | None = None
    """Only from ``get_task``: ``None`` while the task runs."""


# ---- client messages


def task_answer_message(
    request_id: str,
    outcome: str = "accept",
    value: Any = None,
    correction: str | None = None,
) -> dict[str, Any]:
    """The ``task_answer`` message. ``value`` goes with ``accept`` (a chosen option, a value,
    or a form's fields as an object); ``correction`` with ``decline``."""
    if outcome not in TASK_ANSWER_OUTCOMES:
        raise ValueError(f"outcome must be one of {TASK_ANSWER_OUTCOMES}, not {outcome!r}")
    message: dict[str, Any] = {
        "type": "task_answer",
        "request_id": request_id,
        "outcome": outcome,
    }
    if value is not None:
        message["value"] = value
    if correction is not None:
        message["correction"] = correction
    return message
