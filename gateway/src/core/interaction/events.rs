//! Protocol 1.1 wire events of interaction mode, and the mapping from task events onto them.
//!
//! Field names and values are those of `docs/duplex-agent/providers/wire_events.md`
//! ("Server to client"). An event leaves as one pre-serialised text frame
//! (`MessageRoute::Json`).

use serde::Serialize;

use super::types::{
    Act, AnsweredBy, Effects, HitlKind, HitlOutcome, Kind, Lane, Lineage, StopReason, TaskEvent,
    TaskEventKind, TaskOutput, TaskState,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireEvent {
    /// Sent at adoption; a provisional run emits nothing.
    TaskCreated {
        task_id: String,
        task_ref: String,
        revision: u32,
        state: TaskState,
        cancelling: bool,
        title: String,
        /// `lookup` or `action`.
        kind: &'static str,
        side_effects: bool,
        speculative: bool,
        origin: &'static str,
        background: String,
        t_ms: u64,
        /// `parent_id`, `agent` and `trace_id`.
        #[serde(flatten)]
        lineage: Lineage,
    },
    TaskProgress {
        task_id: String,
        task_ref: String,
        revision: u32,
        state: TaskState,
        cancelling: bool,
        seq: u64,
        text: Option<String>,
        fraction: Option<f64>,
        spoken: bool,
        effects: Option<Effects>,
        answer_stopped: Option<AnswerStopped>,
        t_ms: u64,
        /// `parent_id`, `agent` and `trace_id`.
        #[serde(flatten)]
        lineage: Lineage,
    },
    TaskQuestion {
        task_id: String,
        task_ref: String,
        state: TaskState,
        cancelling: bool,
        request_id: String,
        kind: HitlKind,
        /// The exact rendered words.
        prompt: String,
        options: Option<Vec<String>>,
        schema: Option<serde_json::Value>,
        /// `auth_url`: the link the client shows. Never spoken.
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        sensitive: bool,
        irreversible: bool,
        channels: Vec<&'static str>,
        expires_at_ms: u64,
        spoken: bool,
        /// `verified`, `unverified` or `pending`.
        heard: &'static str,
        t_ms: u64,
        /// `parent_id`, `agent` and `trace_id`.
        #[serde(flatten)]
        lineage: Lineage,
    },
    TaskAnswerReceived {
        task_id: String,
        task_ref: String,
        state: TaskState,
        cancelling: bool,
        request_id: String,
        /// `accept`, `decline`, `cancel`, `timeout` or `superseded`.
        outcome: &'static str,
        reason: Option<String>,
        correction: Option<String>,
        by: AnsweredBy,
        corroborated: bool,
        value: Option<serde_json::Value>,
        value_redacted: bool,
        t_ms: u64,
        /// `parent_id`, `agent` and `trace_id`.
        #[serde(flatten)]
        lineage: Lineage,
    },
    TaskResult {
        task_id: String,
        task_ref: String,
        revision: u32,
        state: TaskState,
        /// The same value as `state`, kept for readers of the earlier draft.
        status: TaskState,
        cancelling: bool,
        summary: String,
        effects: Effects,
        /// `full`, `partial` or `none`.
        spoken: &'static str,
        streamed: bool,
        superseded: bool,
        t_ms: u64,
        /// `parent_id`, `agent` and `trace_id`.
        #[serde(flatten)]
        lineage: Lineage,
    },
    /// A task's result, published the moment it exists (chapter 8): the whole answer, what
    /// the backend returned besides it, and its effects. Independent of speech: `task_result`
    /// still follows once the spoken part is done.
    TaskOutput {
        task_id: String,
        task_ref: String,
        revision: u32,
        state: TaskState,
        /// The whole answer, as the background wrote it.
        text: String,
        /// A backend's structured result (a webhook's `data`, an A2A data part, an MCP
        /// tool's `structuredContent`), else `null`.
        data: Option<serde_json::Value>,
        /// Links and files the backend returned (A2A file and link parts, MCP resource links,
        /// a webhook's `artifacts`).
        artifacts: Vec<serde_json::Value>,
        effects: Effects,
        t_ms: u64,
        /// `parent_id`, `agent` and `trace_id`.
        #[serde(flatten)]
        lineage: Lineage,
    },
    /// Only for a confirmed cancel: `state` is `cancelled` and `cancelling` is `false`.
    TaskCancelled {
        task_id: String,
        task_ref: String,
        revision: u32,
        state: TaskState,
        cancelling: bool,
        by: String,
        reason: String,
        effects: Effects,
        t_ms: u64,
        /// `parent_id`, `agent` and `trace_id`.
        #[serde(flatten)]
        lineage: Lineage,
    },
    FloorState {
        /// `user`, `assistant`, `both` or `none`.
        holder: &'static str,
        lane: Option<Lane>,
        /// `user_speech`, `front_ack`, `relay`, `hold`, `idle` or `stream_gap`.
        reason: &'static str,
        t_ms: u64,
    },
    /// Where an utterance's audio starts and ends in the stream (`events.segments`).
    AudioSegment {
        /// `start` or `end`.
        phase: &'static str,
        utterance_id: String,
        /// `front`, `main` or `backchannel`.
        lane: &'static str,
        kind: Kind,
        task_id: Option<String>,
        t_ms: u64,
    },
    AgentAudioStopped {
        utterance_ids: Vec<String>,
        /// `barge_in`, `client_clear`, `policy` or `provider`.
        reason: &'static str,
        delivered_ms: Vec<u64>,
        t_ms: u64,
    },
    /// Nothing else went to the client for 5 s: the session is alive (a browser client cannot
    /// ping the socket, and a quiet wait would look like a dead one).
    Heartbeat {
        t_ms: u64,
    },
    InteractionUpdated {
        applied: Vec<String>,
        refused: Vec<Refused>,
        /// Warning codes that changed behaviour since the echo (for example `front_unavailable`).
        #[serde(skip_serializing_if = "Vec::is_empty")]
        degraded: Vec<String>,
        /// The versioned conversation state (chapter 9), when the session has one.
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<serde_json::Value>,
        /// The line the caller is hearing because a service failed (chapter 10).
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<FailureNote>,
    },
    /// A service the call needs stayed down (chapter 10): the exit line was said and the
    /// session ends. The same body, with the session's `stream_id`, is pushed to the
    /// operator's push address.
    SessionFailed {
        /// `recogniser`, `front`, `background` or `voice`.
        service: &'static str,
        /// Why, in words.
        reason: String,
        /// The tasks that were live, and whether each runs on (`tasks.session_end`).
        pending: Vec<Pending>,
        /// The exit line promised to get back to the caller.
        follow_up: bool,
        t_ms: u64,
    },
    /// The caller asked for a person and the operator's transfer was started (a `human`
    /// control): `via` is `sip` (the call is referred to `controls.human.transfer_to`) or
    /// `webhook` (the same body, with the session's `stream_id`, its state and history, is
    /// signed to `controls.human.url`; the session then ends).
    HumanRequested {
        via: &'static str,
        /// The tasks that were live: they follow `tasks.session_end`.
        pending: Vec<Pending>,
        t_ms: u64,
    },
    /// The operator answered a webhook hand-off to a person: `outcome` `connected` (the persona
    /// says it puts the caller through, then the session ends), `unavailable` or `no_answer`
    /// (within `controls.human.wait_s`): the caller hears so and the call goes on.
    HumanAnswered {
        outcome: &'static str,
        t_ms: u64,
    },
    /// A conversation control was applied, partly applied, not supported or refused.
    ControlApplied {
        kind: super::controls::ControlKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        source: super::controls::ControlSource,
        outcome: super::controls::Outcome,
        detail: String,
        state_version: u64,
        /// `to_screen`: the text sent to the caller's screen.
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// The mode-off shape `{turn_index, delta}` plus the 1.1 fields.
    AssistantTranscript {
        turn_index: u64,
        delta: String,
        /// `front`, `background` or `gateway`.
        source: &'static str,
        kind: Kind,
        verbatim: bool,
        /// Only on a line the front wrote.
        #[serde(skip_serializing_if = "Option::is_none")]
        act: Option<Act>,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        utterance_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sentence: Option<Sentence>,
        /// `full`, `partial` or `none`.
        heard: &'static str,
    },
    Error {
        message: String,
        code: String,
    },
    /// Regimes B and C: a sentence the speech-to-speech provider said on its own that no task
    /// of the session supports, or words of a gateway line it did not say as asked.
    UnverifiedClaim {
        /// `completion`, `outcome` or `entity`.
        kind: &'static str,
        /// `CompletionClaim`, `ActionPromise`, `ChannelPromise`, `EtaPromise`,
        /// `UnsourcedNumber` or `ExactWords`.
        rule: &'static str,
        /// The provider's words.
        text: String,
        /// The policy's correction was spoken.
        corrected: bool,
        /// The provider's output was stopped before the words played.
        output_stopped: bool,
        t_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnswerStopped {
    pub reason: StopReason,
    pub released_sentences: u32,
}

/// A failure line (chapter 10) on the wire: its id in the pack's `failure` group, its kind,
/// the service it is about and its words.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FailureNote {
    pub line: String,
    pub kind: &'static str,
    pub service: &'static str,
    pub text: String,
}

/// A task that was live when the session failed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pending {
    pub task_id: String,
    pub task_ref: String,
    pub title: String,
    pub state: TaskState,
}

impl Pending {
    /// The board's live tasks, as `session_failed` and `human_requested` list them.
    pub fn live(board: super::types::Board) -> Vec<Self> {
        let tasks = board.tasks.into_iter();
        tasks
            .filter(|task| task.state.is_live())
            .map(|task| Self {
                task_id: task.id.0,
                task_ref: task.task_ref.0,
                title: task.title,
                state: task.state,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Refused {
    pub path: String,
    pub reason: String,
}

impl Refused {
    /// The paths an `interaction_update` could not apply, from its findings: the path under
    /// `interaction.` and the finding's reason (else its code).
    pub fn of(warnings: Vec<super::config::ConfigWarning>) -> Vec<Refused> {
        warnings
            .into_iter()
            .map(|warning| {
                let detail = |key: &str| warning.detail[key].as_str();
                let path = detail("path").unwrap_or_default();
                Refused {
                    path: path.trim_start_matches("interaction.").to_owned(),
                    reason: detail("reason").unwrap_or(warning.code).to_owned(),
                }
            })
            .collect()
    }
}

/// One relayed sentence of a streamed answer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sentence {
    pub attempt: u32,
    pub seq: u32,
}

/// The client event for a task event. `spoken` says whether the policy also says it aloud.
/// `AnswerSentence` has no task event: it becomes one `assistant_transcript` when it plays.
///
/// A question's `prompt` is the background's own question, or the verb phrase of a
/// confirmation; the caller replaces it with the persona's rendered read-back.
pub fn task_event_to_wire(event: &TaskEvent, spoken: bool) -> Option<WireEvent> {
    let task_id = event.task.0.clone();
    let task_ref = event.task_ref.0.clone();
    let progress = |text: Option<String>,
                    effects: Option<Effects>,
                    answer_stopped: Option<AnswerStopped>,
                    cancelling: bool| WireEvent::TaskProgress {
        task_id: task_id.clone(),
        task_ref: task_ref.clone(),
        revision: event.revision,
        state: event.state,
        cancelling,
        seq: event.seq,
        text,
        fraction: None,
        spoken,
        effects,
        answer_stopped,
        t_ms: event.at,
        lineage: Lineage::clone(&event.lineage),
    };
    // `stale`: the outcome of a replaced revision, which is over while its task goes on.
    // `answered`: a finished answer, which always left as relayed sentences.
    let result = |summary: &str, effects: &Effects, answered: bool, stale: bool| {
        let state = if stale {
            TaskState::Completed
        } else {
            event.state
        };
        WireEvent::TaskResult {
            task_id: task_id.clone(),
            task_ref: task_ref.clone(),
            revision: event.revision,
            state,
            status: state,
            cancelling: event.cancelling,
            summary: summary.to_owned(),
            effects: effects.clone(),
            spoken: if spoken { "full" } else { "none" },
            streamed: answered,
            superseded: stale,
            t_ms: event.at,
            lineage: Lineage::clone(&event.lineage),
        }
    };
    Some(match &event.kind {
        TaskEventKind::Created {
            title,
            side_effects,
            speculative,
            backend,
        } => WireEvent::TaskCreated {
            task_id,
            task_ref,
            revision: event.revision,
            state: event.state,
            cancelling: event.cancelling,
            title: title.clone(),
            kind: if *side_effects { "action" } else { "lookup" },
            side_effects: *side_effects,
            speculative: *speculative,
            origin: "front",
            background: backend.clone(),
            t_ms: event.at,
            lineage: Lineage::clone(&event.lineage),
        },
        TaskEventKind::Working | TaskEventKind::Revised => {
            progress(None, None, None, event.cancelling)
        }
        TaskEventKind::Progress { text } => progress(text.clone(), None, None, event.cancelling),
        TaskEventKind::EffectsUpdated(effects) => {
            progress(None, Some(effects.clone()), None, event.cancelling)
        }
        TaskEventKind::AnswerStopped {
            released, reason, ..
        } => progress(
            None,
            None,
            Some(AnswerStopped {
                reason: *reason,
                released_sentences: *released,
            }),
            event.cancelling,
        ),
        TaskEventKind::CancelRequested => progress(None, None, None, true),
        TaskEventKind::CancelUnconfirmed { effects } => {
            progress(None, Some(effects.clone()), None, true)
        }
        TaskEventKind::AnswerSentence { .. } | TaskEventKind::AnswerDone => return None,
        TaskEventKind::InputRequired(request) => WireEvent::TaskQuestion {
            task_id,
            task_ref,
            state: event.state,
            cancelling: event.cancelling,
            request_id: request.id.0.clone(),
            kind: request.kind,
            prompt: request
                .question
                .clone()
                .unwrap_or_else(|| request.verb_phrase.clone()),
            options: (!request.options.is_empty()).then(|| request.options.clone()),
            schema: request.schema.as_deref().cloned(),
            url: request.url.as_deref().map(str::to_owned),
            sensitive: request.sensitive,
            irreversible: request.irreversible,
            // A form, a link and a sensitive value are answered on the screen only.
            channels: if request.sensitive || request.kind.on_screen() {
                vec!["client"]
            } else {
                vec!["voice", "client"]
            },
            expires_at_ms: request.expires_at,
            spoken,
            heard: "pending",
            t_ms: event.at,
            lineage: Lineage::clone(&event.lineage),
        },
        TaskEventKind::InputResolved {
            request,
            outcome,
            by,
            redacted,
        } => {
            let (name, correction, value) = match outcome {
                HitlOutcome::Accept { value } => ("accept", None, value.clone()),
                HitlOutcome::Decline { correction } => ("decline", correction.clone(), None),
                HitlOutcome::Cancel => ("cancel", None, None),
                HitlOutcome::Timeout => ("timeout", None, None),
                HitlOutcome::Superseded => ("superseded", None, None),
            };
            WireEvent::TaskAnswerReceived {
                task_id,
                task_ref,
                state: event.state,
                cancelling: event.cancelling,
                request_id: request.0.clone(),
                outcome: name,
                reason: None,
                correction,
                by: *by,
                corroborated: false,
                value,
                value_redacted: *redacted,
                t_ms: event.at,
                lineage: Lineage::clone(&event.lineage),
            }
        }
        TaskEventKind::Completed { text, effects } => result(text, effects, true, false),
        TaskEventKind::Failed { error, effects } => result(error, effects, false, false),
        TaskEventKind::Rejected { reason } => result(reason, &Effects::NoneCommitted, false, false),
        TaskEventKind::StaleOutcome { label } => {
            result(label, &Effects::Committed(vec![label.clone()]), false, true)
        }
        TaskEventKind::Cancelled { by, effects } => WireEvent::TaskCancelled {
            task_id,
            task_ref,
            revision: event.revision,
            state: TaskState::Cancelled,
            cancelling: false,
            by: (*by).to_owned(),
            reason: format!("{by}_cancelled"),
            effects: effects.clone(),
            t_ms: event.at,
            lineage: Lineage::clone(&event.lineage),
        },
    })
}

/// The `task_output` of a completed task; `None` for every other event.
pub fn task_output(event: &TaskEvent, output: Option<&TaskOutput>) -> Option<WireEvent> {
    let TaskEventKind::Completed { text, effects } = &event.kind else {
        return None;
    };
    let output = output.cloned().unwrap_or_default();
    Some(WireEvent::TaskOutput {
        task_id: event.task.0.clone(),
        task_ref: event.task_ref.0.clone(),
        revision: event.revision,
        state: event.state,
        text: text.clone(),
        data: output.data,
        artifacts: output.artifacts,
        effects: effects.clone(),
        t_ms: event.at,
        lineage: Lineage::clone(&event.lineage),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::types::{HitlId, HitlRequest, TaskId, TaskRef};
    use super::*;

    /// The event serialises to exactly the document's example.
    fn assert_wire(event: &WireEvent, example: &str) {
        let expected: Value = serde_json::from_str(example).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), expected);
    }

    fn task_event(
        seq: u64,
        revision: u32,
        at: u64,
        state: TaskState,
        kind: TaskEventKind,
    ) -> TaskEvent {
        TaskEvent {
            task: TaskId("task_5b1e9c0d".to_owned()),
            task_ref: TaskRef("t1".to_owned()),
            seq,
            revision,
            at,
            state,
            cancelling: false,
            lineage: Box::new(Lineage {
                follows: None,
                parent_id: Some(TaskId("task_parent".to_owned())),
                agent: Some("browser".to_owned()),
                trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".to_owned(),
            }),
            kind,
        }
    }

    fn rosa() -> Effects {
        Effects::Committed(vec!["table hold at Rosa's".to_owned()])
    }

    // The nine events of wire_events.md, "Server to client", one example each.

    #[test]
    fn task_created_example() {
        let event = task_event(
            0,
            1,
            4180,
            TaskState::Working,
            TaskEventKind::Created {
                title: "your table booking".to_owned(),
                side_effects: true,
                speculative: true,
                backend: "agent:support".to_owned(),
            },
        );
        assert_wire(
            &task_event_to_wire(&event, false).unwrap(),
            r#"{"type": "task_created", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "working", "cancelling": false, "title": "your table booking", "kind": "action", "side_effects": true, "speculative": true, "origin": "front", "background": "agent:support", "t_ms": 4180, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );
    }

    #[test]
    fn task_progress_examples() {
        let progress = task_event(
            3,
            1,
            7020,
            TaskState::Working,
            TaskEventKind::Progress {
                text: Some("Checking availability at Rosa's".to_owned()),
            },
        );
        assert_wire(
            &task_event_to_wire(&progress, true).unwrap(),
            r#"{"type": "task_progress", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "working", "cancelling": false, "seq": 3, "text": "Checking availability at Rosa's", "fraction": null, "spoken": true, "effects": null, "answer_stopped": null, "t_ms": 7020, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );

        let stopped = task_event(
            4,
            2,
            8350,
            TaskState::Working,
            TaskEventKind::AnswerStopped {
                attempt: 0,
                released: 1,
                reason: StopReason::Revised,
            },
        );
        assert_wire(
            &task_event_to_wire(&stopped, false).unwrap(),
            r#"{"type": "task_progress", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 2, "state": "working", "cancelling": false, "seq": 4, "text": null, "fraction": null, "spoken": false, "effects": null, "answer_stopped": {"reason": "revised", "released_sentences": 1}, "t_ms": 8350, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );

        // An unconfirmed cancel stays a progress event: state unchanged, `cancelling: true`.
        let unconfirmed = task_event(
            6,
            2,
            25200,
            TaskState::Working,
            TaskEventKind::CancelUnconfirmed {
                effects: Effects::Unknown,
            },
        );
        assert_wire(
            &task_event_to_wire(&unconfirmed, true).unwrap(),
            r#"{"type": "task_progress", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 2, "state": "working", "cancelling": true, "seq": 6, "text": null, "fraction": null, "spoken": true, "effects": "unknown", "answer_stopped": null, "t_ms": 25200, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );
    }

    #[test]
    fn task_question_examples() {
        let request = HitlRequest {
            id: HitlId("q_task_5b1e9c0d_1".to_owned()),
            task: TaskId("task_5b1e9c0d".to_owned()),
            revision: 1,
            kind: HitlKind::Confirm,
            op: None,
            verb_phrase: "book a table".to_owned(),
            params: vec![("party".to_owned(), json!(4))],
            question: None,
            options: Vec::new(),
            irreversible: false,
            expires_at: 39500,
            ..Default::default()
        };
        let event = task_event(
            5,
            1,
            9500,
            TaskState::InputRequired,
            TaskEventKind::InputRequired(request),
        );
        // The caller replaces the prompt of a confirmation with the persona's read-back.
        let mut wire = task_event_to_wire(&event, true).unwrap();
        match &mut wire {
            WireEvent::TaskQuestion { prompt, .. } => {
                assert_eq!(prompt, "book a table");
                *prompt = "Shall I book a table for 4 at Rosa's on Saturday at 7 pm?".to_owned();
            }
            other => panic!("expected a task_question, got {other:?}"),
        }
        assert_wire(
            &wire,
            r#"{"type": "task_question", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "input_required", "cancelling": false, "request_id": "q_task_5b1e9c0d_1", "kind": "confirm", "prompt": "Shall I book a table for 4 at Rosa's on Saturday at 7 pm?", "options": null, "schema": null, "sensitive": false, "irreversible": false, "channels": ["voice", "client"], "expires_at_ms": 39500, "spoken": true, "heard": "pending", "t_ms": 9500, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );

        let choose = HitlRequest {
            id: HitlId("q_task_5b1e9c0d_2".to_owned()),
            task: TaskId("task_5b1e9c0d".to_owned()),
            revision: 1,
            kind: HitlKind::Choose,
            op: None,
            verb_phrase: String::new(),
            params: Vec::new(),
            question: Some("Would you like 7 pm or 8 pm?".to_owned()),
            options: vec!["7 pm".to_owned(), "8 pm".to_owned()],
            irreversible: false,
            expires_at: 61000,
            ..Default::default()
        };
        let event = task_event(
            7,
            1,
            31000,
            TaskState::InputRequired,
            TaskEventKind::InputRequired(choose),
        );
        assert_wire(
            &task_event_to_wire(&event, true).unwrap(),
            r#"{"type": "task_question", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "input_required", "cancelling": false, "request_id": "q_task_5b1e9c0d_2", "kind": "choose", "prompt": "Would you like 7 pm or 8 pm?", "options": ["7 pm", "8 pm"], "schema": null, "sensitive": false, "irreversible": false, "channels": ["voice", "client"], "expires_at_ms": 61000, "spoken": true, "heard": "pending", "t_ms": 31000, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );
    }

    #[test]
    fn a_screen_question_and_a_segment_on_the_wire() {
        let link = HitlRequest {
            id: HitlId("q_task_5b1e9c0d_3".to_owned()),
            kind: HitlKind::AuthUrl,
            question: Some("Sign in to continue.".to_owned()),
            url: Some("https://id.example.com/x".into()),
            sensitive: true,
            expires_at: 61000,
            ..Default::default()
        };
        let event = task_event(
            9,
            1,
            31000,
            TaskState::InputRequired,
            TaskEventKind::InputRequired(link),
        );
        let wire = serde_json::to_value(task_event_to_wire(&event, true).unwrap()).unwrap();
        assert_eq!(wire["kind"], "auth_url");
        assert_eq!(wire["url"], "https://id.example.com/x");
        assert_eq!(
            (&wire["sensitive"], &wire["channels"]),
            (&json!(true), &json!(["client"]))
        );
        let redacted = task_event(
            10,
            1,
            33000,
            TaskState::Working,
            TaskEventKind::InputResolved {
                request: HitlId("q_task_5b1e9c0d_3".to_owned()),
                outcome: HitlOutcome::Accept { value: None },
                by: AnsweredBy::Client,
                redacted: true,
            },
        );
        let wire = serde_json::to_value(task_event_to_wire(&redacted, false).unwrap()).unwrap();
        assert_eq!(
            (&wire["value"], &wire["value_redacted"]),
            (&Value::Null, &json!(true))
        );
        assert_wire(
            &WireEvent::AudioSegment {
                phase: "start",
                utterance_id: "u_41".to_owned(),
                lane: "front",
                kind: Kind::Ack,
                task_id: None,
                t_ms: 12100,
            },
            r#"{"type": "audio_segment", "phase": "start", "utterance_id": "u_41", "lane": "front", "kind": "ack", "task_id": null, "t_ms": 12100}"#,
        );
        let clip = serde_json::to_value(Kind::Clip).unwrap();
        assert_eq!(clip, "backchannel");
    }

    #[test]
    fn task_answer_received_examples() {
        // The document's first example, from a regime where a provider corroborates the answer.
        assert_wire(
            &WireEvent::TaskAnswerReceived {
                task_id: "task_5b1e9c0d".to_owned(),
                task_ref: "t1".to_owned(),
                state: TaskState::Working,
                cancelling: false,
                request_id: "q_task_5b1e9c0d_1".to_owned(),
                outcome: "accept",
                reason: None,
                correction: None,
                by: AnsweredBy::Voice,
                corroborated: true,
                value: None,
                value_redacted: false,
                t_ms: 11240,
                lineage: Lineage::default(),
            },
            r#"{"type": "task_answer_received", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "working", "cancelling": false, "request_id": "q_task_5b1e9c0d_1", "outcome": "accept", "reason": null, "correction": null, "by": "voice", "corroborated": true, "value": null, "value_redacted": false, "t_ms": 11240, "parent_id": null, "agent": null, "trace_id": ""}"#,
        );

        let declined = task_event(
            8,
            1,
            12400,
            TaskState::Working,
            TaskEventKind::InputResolved {
                request: HitlId("q_task_5b1e9c0d_1".to_owned()),
                outcome: HitlOutcome::Decline {
                    correction: Some("make it Tuesday".to_owned()),
                },
                by: AnsweredBy::Voice,
                redacted: false,
            },
        );
        assert_wire(
            &task_event_to_wire(&declined, false).unwrap(),
            r#"{"type": "task_answer_received", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "working", "cancelling": false, "request_id": "q_task_5b1e9c0d_1", "outcome": "decline", "reason": null, "correction": "make it Tuesday", "by": "voice", "corroborated": false, "value": null, "value_redacted": false, "t_ms": 12400, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );
    }

    #[test]
    fn task_result_examples() {
        let completed = task_event(
            9,
            1,
            13900,
            TaskState::Completed,
            TaskEventKind::Completed {
                text: "Booked: table for 4, Saturday 19:00, confirmation R-5512".to_owned(),
                effects: Effects::Committed(vec!["table booking at Rosa's".to_owned()]),
            },
        );
        assert_wire(
            &task_event_to_wire(&completed, true).unwrap(),
            r#"{"type": "task_result", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "completed", "status": "completed", "cancelling": false, "summary": "Booked: table for 4, Saturday 19:00, confirmation R-5512", "effects": ["table booking at Rosa's"], "spoken": "full", "streamed": true, "superseded": false, "t_ms": 13900, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );

        // The `unknown` state of the document's second example.
        assert_wire(
            &WireEvent::TaskResult {
                task_id: "task_77aa01fe".to_owned(),
                task_ref: "t2".to_owned(),
                revision: 1,
                state: TaskState::Unknown,
                status: TaskState::Unknown,
                cancelling: false,
                summary: "The booking service stopped answering; the outcome is not known"
                    .to_owned(),
                effects: Effects::Unknown,
                spoken: "full",
                streamed: false,
                superseded: false,
                t_ms: 64000,
                lineage: Lineage::default(),
            },
            r#"{"type": "task_result", "task_id": "task_77aa01fe", "task_ref": "t2", "revision": 1, "state": "unknown", "status": "unknown", "cancelling": false, "summary": "The booking service stopped answering; the outcome is not known", "effects": "unknown", "spoken": "full", "streamed": false, "superseded": false, "t_ms": 64000, "parent_id": null, "agent": null, "trace_id": ""}"#,
        );

        let failed = task_event(
            2,
            1,
            500,
            TaskState::Failed,
            TaskEventKind::Failed {
                error: "the search timed out".to_owned(),
                effects: Effects::NoneCommitted,
            },
        );
        let rejected = task_event(
            1,
            1,
            10,
            TaskState::Rejected,
            TaskEventKind::Rejected {
                reason: "too many tasks".to_owned(),
            },
        );
        for (event, state) in [(failed, "failed"), (rejected, "rejected")] {
            let wire = serde_json::to_value(task_event_to_wire(&event, false).unwrap()).unwrap();
            assert_eq!(wire["type"], "task_result");
            assert_eq!(wire["state"], state);
            assert_eq!(wire["status"], state);
            assert_eq!(wire["effects"], "none_committed");
            assert_eq!(wire["spoken"], "none");
        }
    }

    #[test]
    fn task_cancelled_examples() {
        // The registry reports the task as still cancelling; the wire event never does.
        let mut event = task_event(
            10,
            2,
            15000,
            TaskState::Cancelled,
            TaskEventKind::Cancelled {
                by: "user",
                effects: rosa(),
            },
        );
        event.cancelling = true;
        assert_wire(
            &task_event_to_wire(&event, true).unwrap(),
            r#"{"type": "task_cancelled", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 2, "state": "cancelled", "cancelling": false, "by": "user", "reason": "user_cancelled", "effects": ["table hold at Rosa's"], "t_ms": 15000, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );

        let mut event = task_event(
            3,
            1,
            15200,
            TaskState::Cancelled,
            TaskEventKind::Cancelled {
                by: "client",
                effects: Effects::NoneCommitted,
            },
        );
        event.task = TaskId("task_77aa01fe".to_owned());
        event.task_ref = TaskRef("t2".to_owned());
        assert_wire(
            &task_event_to_wire(&event, false).unwrap(),
            r#"{"type": "task_cancelled", "task_id": "task_77aa01fe", "task_ref": "t2", "revision": 1, "state": "cancelled", "cancelling": false, "by": "client", "reason": "client_cancelled", "effects": "none_committed", "t_ms": 15200, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );
    }

    #[test]
    fn floor_state_examples() {
        assert_wire(
            &WireEvent::FloorState {
                holder: "assistant",
                lane: Some(Lane::Front),
                reason: "front_ack",
                t_ms: 11010,
            },
            r#"{"type": "floor_state", "holder": "assistant", "lane": "front", "reason": "front_ack", "t_ms": 11010}"#,
        );
        assert_wire(
            &WireEvent::FloorState {
                holder: "none",
                lane: None,
                reason: "stream_gap",
                t_ms: 13240,
            },
            r#"{"type": "floor_state", "holder": "none", "lane": null, "reason": "stream_gap", "t_ms": 13240}"#,
        );
    }

    #[test]
    fn agent_audio_stopped_examples() {
        assert_wire(
            &WireEvent::AgentAudioStopped {
                utterance_ids: vec!["u_41".to_owned()],
                reason: "barge_in",
                delivered_ms: vec![1840],
                t_ms: 12620,
            },
            r#"{"type": "agent_audio_stopped", "utterance_ids": ["u_41"], "reason": "barge_in", "delivered_ms": [1840], "t_ms": 12620}"#,
        );
        assert_wire(
            &WireEvent::AgentAudioStopped {
                utterance_ids: vec!["u_44".to_owned(), "u_45".to_owned()],
                reason: "policy",
                delivered_ms: vec![600, 0],
                t_ms: 20400,
            },
            r#"{"type": "agent_audio_stopped", "utterance_ids": ["u_44", "u_45"], "reason": "policy", "delivered_ms": [600, 0], "t_ms": 20400}"#,
        );
    }

    #[test]
    fn interaction_updated_examples() {
        assert_wire(
            &WireEvent::InteractionUpdated {
                applied: vec!["front.caller.verbosity".to_owned()],
                refused: vec![Refused {
                    path: "regime".to_owned(),
                    reason: "not_mid_session".to_owned(),
                }],
                degraded: Vec::new(),
                state: None,
                failure: None,
            },
            r#"{"type": "interaction_updated", "applied": ["front.caller.verbosity"], "refused": [{"path": "regime", "reason": "not_mid_session"}]}"#,
        );
        assert_wire(
            &WireEvent::InteractionUpdated {
                applied: Vec::new(),
                refused: Vec::new(),
                degraded: vec!["front_unavailable".to_owned()],
                state: Some(json!({"version": 2, "language": "en"})),
                failure: None,
            },
            r#"{"type": "interaction_updated", "applied": [], "refused": [], "degraded": ["front_unavailable"], "state": {"version": 2, "language": "en"}}"#,
        );
        // Chapter 10: the failure line the caller hears, and the end of a failed session.
        assert_wire(
            &WireEvent::InteractionUpdated {
                applied: Vec::new(),
                refused: Vec::new(),
                degraded: vec!["voice_unavailable".to_owned()],
                state: None,
                failure: Some(FailureNote {
                    line: "stall-1".to_owned(),
                    kind: "stall",
                    service: "voice",
                    text: "Sorry, my systems are acting up. Bear with me a moment.".to_owned(),
                }),
            },
            r#"{"type": "interaction_updated", "applied": [], "refused": [], "degraded": ["voice_unavailable"], "failure": {"line": "stall-1", "kind": "stall", "service": "voice", "text": "Sorry, my systems are acting up. Bear with me a moment."}}"#,
        );
        assert_wire(
            &WireEvent::HumanAnswered {
                outcome: "no_answer",
                t_ms: 61000,
            },
            r#"{"type": "human_answered", "outcome": "no_answer", "t_ms": 61000}"#,
        );
        assert_wire(
            &WireEvent::HumanRequested {
                via: "webhook",
                pending: Vec::new(),
                t_ms: 52100,
            },
            r#"{"type": "human_requested", "via": "webhook", "pending": [], "t_ms": 52100}"#,
        );
        assert_wire(
            &WireEvent::SessionFailed {
                service: "background",
                reason: "no answer for 30 s after the first stall line".to_owned(),
                pending: vec![Pending {
                    task_id: "task_5b1e9c0d".to_owned(),
                    task_ref: "t1".to_owned(),
                    title: "what time does the store open".to_owned(),
                    state: TaskState::Working,
                }],
                follow_up: false,
                t_ms: 40210,
            },
            r#"{"type": "session_failed", "service": "background", "reason": "no answer for 30 s after the first stall line", "pending": [{"task_id": "task_5b1e9c0d", "task_ref": "t1", "title": "what time does the store open", "state": "working"}], "follow_up": false, "t_ms": 40210}"#,
        );
        assert_wire(
            &WireEvent::ControlApplied {
                kind: super::super::controls::ControlKind::Rate,
                value: Some("slower".to_owned()),
                source: super::super::controls::ControlSource::Caller,
                outcome: super::super::controls::Outcome::Applied,
                detail: "rate 0.85".to_owned(),
                state_version: 3,
                text: None,
            },
            r#"{"type": "control_applied", "kind": "rate", "value": "slower", "source": "caller", "outcome": "applied", "detail": "rate 0.85", "state_version": 3}"#,
        );
    }

    // The two existing messages that interaction mode extends.

    #[test]
    fn assistant_transcript_examples() {
        assert_wire(
            &WireEvent::AssistantTranscript {
                turn_index: 3,
                delta: "Booked: a table for 4 at Rosa's on Saturday at 7 pm.".to_owned(),
                source: "background",
                kind: Kind::Relay,
                verbatim: true,
                act: None,
                task_id: Some("task_5b1e9c0d".to_owned()),
                utterance_id: "u_42".to_owned(),
                sentence: Some(Sentence { attempt: 0, seq: 0 }),
                heard: "full",
            },
            r#"{"type": "assistant_transcript", "turn_index": 3, "delta": "Booked: a table for 4 at Rosa's on Saturday at 7 pm.", "source": "background", "kind": "relay", "verbatim": true, "task_id": "task_5b1e9c0d", "utterance_id": "u_42", "sentence": {"attempt": 0, "seq": 0}, "heard": "full"}"#,
        );
        assert_wire(
            &WireEvent::AssistantTranscript {
                turn_index: 4,
                delta: "Still on it.".to_owned(),
                source: "front",
                kind: Kind::Progress,
                verbatim: false,
                act: Some(Act::Progress),
                task_id: Some("task_5b1e9c0d".to_owned()),
                utterance_id: "u_43".to_owned(),
                sentence: None,
                heard: "full",
            },
            r#"{"type": "assistant_transcript", "turn_index": 4, "delta": "Still on it.", "source": "front", "kind": "progress", "verbatim": false, "act": "progress", "task_id": "task_5b1e9c0d", "utterance_id": "u_43", "heard": "full"}"#,
        );
    }

    #[test]
    fn error_example() {
        assert_wire(
            &WireEvent::Error {
                message: "That question was already answered.".to_owned(),
                code: "question_closed".to_owned(),
            },
            r#"{"type": "error", "message": "That question was already answered.", "code": "question_closed"}"#,
        );
    }

    #[test]
    fn task_events_without_a_wire_event_and_plain_progress() {
        let sentence = task_event(
            2,
            1,
            900,
            TaskState::Working,
            TaskEventKind::AnswerSentence {
                attempt: 0,
                seq: 0,
                text: "Two flights are under four hundred dollars.".to_owned(),
                first_after_e_ms: Some(640),
            },
        );
        assert_eq!(task_event_to_wire(&sentence, true), None);

        for kind in [
            TaskEventKind::Working,
            TaskEventKind::Revised,
            TaskEventKind::CancelRequested,
            TaskEventKind::EffectsUpdated(rosa()),
        ] {
            let cancel = kind == TaskEventKind::CancelRequested;
            let effects = matches!(kind, TaskEventKind::EffectsUpdated(_));
            let event = task_event(1, 1, 100, TaskState::Working, kind);
            let wire = serde_json::to_value(task_event_to_wire(&event, false).unwrap()).unwrap();
            assert_eq!(wire["type"], "task_progress");
            assert_eq!(wire["state"], "working");
            assert_eq!(wire["cancelling"], cancel);
            assert_eq!(wire["effects"].is_null(), !effects);
        }
    }
    #[test]
    fn task_output_example() {
        let completed = TaskEventKind::Completed {
            text: "Booked AI 101 on Friday at 9:40.".to_owned(),
            effects: rosa(),
        };
        let event = task_event(4, 1, 13_900, TaskState::Completed, completed);
        let output = TaskOutput {
            data: Some(json!({ "flight": "AI 101", "departs": "09:40" })),
            artifacts: vec![json!({ "url": "https://air.example/t/AI101", "name": "e-ticket" })],
        };
        assert_wire(
            &task_output(&event, Some(&output)).expect("an output"),
            r#"{"type": "task_output", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "completed", "text": "Booked AI 101 on Friday at 9:40.", "data": {"flight": "AI 101", "departs": "09:40"}, "artifacts": [{"url": "https://air.example/t/AI101", "name": "e-ticket"}], "effects": ["table hold at Rosa's"], "t_ms": 13900, "parent_id": "task_parent", "agent": "browser", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"}"#,
        );
        // A text-only result has no data and no artifacts; only a completed task has one.
        let plain = serde_json::to_value(task_output(&event, None).unwrap()).unwrap();
        assert_eq!(
            (&plain["data"], &plain["artifacts"]),
            (&Value::Null, &json!([]))
        );
        let failed = TaskEventKind::Failed {
            error: "no seats".to_owned(),
            effects: Effects::NoneCommitted,
        };
        let failed = task_event(4, 1, 13_900, TaskState::Failed, failed);
        assert_eq!(task_output(&failed, Some(&output)), None);
        // The end of an orchestrator's answer is not on the wire.
        let done = task_event(3, 1, 9_000, TaskState::Working, TaskEventKind::AnswerDone);
        assert_eq!(task_event_to_wire(&done, false), None);
    }
}
