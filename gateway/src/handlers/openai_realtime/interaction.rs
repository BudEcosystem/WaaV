//! Interaction mode on the `/v1/realtime` relay: regimes B and C over a Bud realtime
//! deployment whose vendor speaks OpenAI Realtime GA. The block is the operator's
//! `interaction.realtime_default` (budapp publishes none per deployment yet).
//!
//! The relay keeps forwarding frames; this part reads them on the way. The delegate function
//! is added to every `session.update` the vendor gets, its calls never reach the client, the
//! caller's and the vendor's words feed the session (`core::interaction::provider`), the
//! session's verbs go to the vendor as GA frames, and its events reach the client as `bud.*`.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tracing::warn;

use crate::core::interaction::actor::Msg;
use crate::core::interaction::config::{self, ParseCtx};
use crate::core::interaction::executor::{self, ExecHandle, Signal, Verb, Wire};
use crate::core::interaction::provider::{self, AudioOut, DELEGATE_TOOL, ProviderEvent};
use crate::core::interaction::types::HitlOutcome;
use crate::core::realtime::openai::protocol::OpenAiProtocol;
use crate::core::realtime::scaffold::{OutFrame, RealtimeProtocol};
use crate::core::realtime::{RealtimeConfig, RealtimeError, RealtimeResult};
use crate::handlers::realtime::interaction::plan;

use super::cascade::{bud_event, event};

/// The session's verbs as GA frames for the relay to send to the vendor.
struct RelayWire {
    protocol: OpenAiProtocol,
    tx: mpsc::UnboundedSender<String>,
}

impl RelayWire {
    fn send(&self, wires: Vec<<OpenAiProtocol as RealtimeProtocol>::Wire>) -> RealtimeResult<()> {
        for wire in wires {
            match self.protocol.serialize(&wire)? {
                OutFrame::Text(text) => self
                    .tx
                    .send(text)
                    .map_err(|_| RealtimeError::NotConnected)?,
                OutFrame::Binary(_) => {}
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Wire for RelayWire {
    async fn verb(&mut self, verb: &Verb) -> RealtimeResult<()> {
        let wires = self.protocol.verb(&verb.realtime()).ok_or_else(|| {
            RealtimeError::InvalidConfiguration("no wire for this verb".to_owned())
        })?;
        self.send(wires)
    }

    async fn tool_result(
        &mut self,
        call_id: &str,
        output: &str,
        reply: bool,
    ) -> RealtimeResult<()> {
        self.send(self.protocol.format_tool_result(call_id, output))?;
        if reply {
            self.send(self.protocol.create_response(None))?;
        }
        Ok(())
    }

    async fn reply(&mut self) -> RealtimeResult<()> {
        self.send(self.protocol.create_response(None))
    }
}

/// What the relay loop reads besides its two sockets.
pub(super) struct Channels {
    /// GA frames for the vendor.
    pub to_vendor: mpsc::UnboundedReceiver<String>,
    /// `bud.*` events for the client.
    pub to_client: mpsc::UnboundedReceiver<String>,
}

/// One relay session's interaction mode.
pub(super) struct RelayIx {
    inbox: mpsc::Sender<Msg>,
    exec: ExecHandle,
    suffix: &'static str,
    /// The delegate calls' item and call ids: the client never sees them.
    hidden: HashSet<String>,
    pub(super) echo: Value,
    /// Frames for the client that the mode answers itself.
    client: mpsc::UnboundedSender<String>,
    /// The half-cascade (`speech.voice_owner: gateway`): the vendor writes its replies as
    /// text and its audio never reaches the client; the gateway's voice says the replies.
    half: bool,
}

/// The relay's sample rate (GA `audio/pcm`).
const RELAY_RATE: u32 = 24_000;

/// The session's own audio (chapter 10's failure lines, `kind` `failure`; a line of the
/// gateway's voice, `voice`) as one response of the gateway's: the client hears it as it hears
/// the vendor's, with `metadata.bud_source: gateway`.
fn gateway_response(pcm: &[u8], n: u64, kind: &str) -> Vec<String> {
    let (id, item) = (format!("resp_waav_{n}"), format!("item_waav_{n}"));
    let metadata = json!({"bud_source": "gateway", "bud_kind": kind});
    let response = |status: &str| {
        json!({"response": {"id": id, "object": "realtime.response", "status": status,
                            "output": [], "metadata": metadata}})
    };
    let mut frames = vec![event("response.created", response("in_progress"))];
    // 200 ms of PCM16 per delta.
    let chunk = (RELAY_RATE as usize / 5) * 2;
    for part in pcm.chunks(chunk) {
        let delta = json!({"response_id": id, "item_id": item, "output_index": 0,
                           "content_index": 0, "delta": BASE64.encode(part)});
        frames.push(event("response.output_audio.delta", delta));
    }
    let done = json!({"response_id": id, "item_id": item, "output_index": 0, "content_index": 0});
    frames.push(event("response.output_audio.done", done));
    frames.push(event("response.done", response("completed")));
    frames
}

/// The session's interaction mode, when the operator's block applies and the resolver says
/// so. Findings are logged: the client did not ask.
pub(super) fn prepare(
    vendor: &str,
    model: Option<&str>,
    stream_id: &str,
    owner: &str,
    state: &std::sync::Arc<crate::state::AppState>,
) -> Option<(RelayIx, Channels)> {
    let server = config::server();
    let block = server.realtime_default.clone().filter(|_| server.enabled)?;
    let ctx = ParseCtx {
        bud_mode: true,
        operator: true,
    };
    let (planned, findings) = plan(&block, ctx, &server, "v1_realtime_relay", vendor, model);
    for (code, message, _) in findings {
        warn!(code, %message, "interaction mode on the realtime relay");
    }
    let planned = planned?;
    let realtime = RealtimeConfig {
        api_key: "relay".to_owned(),
        ..RealtimeConfig::default()
    };
    let protocol = OpenAiProtocol::from_config(&realtime).ok()?;
    let (vendor_tx, to_vendor) = mpsc::unbounded_channel();
    let (client_tx, to_client) = mpsc::unbounded_channel();
    let (inbox, rx) = mpsc::channel(256);
    let (exec, pending) = executor::handle();
    let wire = RelayWire {
        protocol,
        tx: vendor_tx,
    };
    executor::start(&exec, pending, Box::new(wire), inbox.clone(), 24_000);
    let (suffix, echo, half) = (planned.suffix, planned.echo.clone(), planned.plan.half);
    // The half-cascade's voice (`plan` made sure the session has one).
    let voice = half
        .then(|| crate::handlers::realtime::interaction::voice_of(&planned.cfg, state))
        .flatten();
    let (events, lines, own) = (client_tx.clone(), client_tx.clone(), client_tx.clone());
    let out = Box::new(move |json: String| {
        if let Some(frame) = bud_frame(&json) {
            let _ = events.send(frame);
        }
    });
    // The failure lines and the voice's lines reach the client as responses of the gateway's
    // own, numbered in one series.
    let played = std::sync::Arc::new(AtomicU64::new(0));
    let (spoken, said) = (played.clone(), client_tx);
    let audio: AudioOut = Box::new(move |pcm| {
        let n = played.fetch_add(1, Ordering::Relaxed) + 1;
        for frame in gateway_response(&pcm, n, "failure") {
            let _ = lines.send(frame);
        }
    });
    let voice_audio: AudioOut = Box::new(move |pcm| {
        let n = spoken.fetch_add(1, Ordering::Relaxed) + 1;
        for frame in gateway_response(&pcm, n, "voice") {
            let _ = said.send(frame);
        }
    });
    let mut deps = planned.deps(
        exec.clone(),
        out,
        stream_id,
        owner,
        suffix.to_owned(),
        Some(audio),
        RELAY_RATE,
        voice,
    );
    deps.voice_audio = Some(voice_audio);
    provider::spawn(deps, inbox.clone(), rx);
    let ix = RelayIx {
        inbox,
        exec,
        suffix,
        hidden: HashSet::new(),
        echo,
        client: own,
        half,
    };
    Some((
        ix,
        Channels {
            to_vendor,
            to_client,
        },
    ))
}

/// A frame's `type`, read without copying the rest; `None` when it is not a JSON event.
fn kind_of(raw: &str) -> Option<Cow<'_, str>> {
    #[derive(serde::Deserialize)]
    struct Kind<'a> {
        #[serde(rename = "type", borrow)]
        kind: Cow<'a, str>,
    }
    serde_json::from_str::<Kind>(raw).ok().map(|k| k.kind)
}

#[derive(serde::Deserialize)]
struct Delta<'a> {
    #[serde(borrow)]
    delta: Cow<'a, str>,
}

pub(super) const ANSWER_REQUIRED: &str =
    "`request_id` and an `outcome` of accept, decline or cancel are required.";

/// A `bud.task.answer` event's request id and outcome; `None` when it cannot be read (an
/// answer it cannot read changes nothing).
pub(super) fn task_answer(event: &Value) -> Option<(String, HitlOutcome)> {
    let text = |key: &str| event.get(key).and_then(Value::as_str).map(str::to_owned);
    let outcome = match event.get("outcome").and_then(Value::as_str)? {
        "accept" => HitlOutcome::Accept {
            value: event.get("value").filter(|v| !v.is_null()).cloned(),
        },
        "decline" => HitlOutcome::Decline {
            correction: text("correction"),
        },
        "cancel" => HitlOutcome::Cancel,
        _ => return None,
    };
    Some((text("request_id")?, outcome))
}

/// The `bud.*` event that ends a relay session: the socket closes after it.
pub(super) const SESSION_FAILED: &str = "bud.session.failed";

/// A session event as a `bud.*` event. The relay also has `audio_segment` (the gateway's
/// lines, plan chapter 6) and `session_failed` (chapter 10).
fn bud_frame(json: &str) -> Option<String> {
    let Ok(Value::Object(mut body)) = serde_json::from_str::<Value>(json) else {
        return None;
    };
    let kind = body.remove("type")?;
    let name = match kind.as_str()? {
        "audio_segment" => "bud.audio.segment",
        "session_failed" => SESSION_FAILED,
        "agent_audio_stopped" => "bud.audio.stopped",
        kind => bud_event(kind)?,
    };
    Some(event(name, Value::Object(body)))
}

/// Whether a frame for the client is the relay session's last.
pub(super) fn is_session_failed(frame: &str) -> bool {
    kind_of(frame).is_some_and(|kind| kind == SESSION_FAILED)
}

/// The delegate function in GA form.
fn delegate_tool() -> Value {
    let (description, parameters) = provider::delegate_tool();
    json!({"type": "function", "name": DELEGATE_TOOL, "description": description, "parameters": parameters})
}

impl RelayIx {
    /// The delegate function and the regime's instructions added to a `session.update`'s
    /// session: to its tools when it names tools (or `all`), to its instructions when it
    /// names them (or `all`).
    fn steer(&self, session: &mut Map<String, Value>, all: bool) {
        if all || session.contains_key("tools") {
            let tools = session.entry("tools").or_insert_with(|| json!([]));
            if let Some(tools) = tools.as_array_mut()
                && !tools.iter().any(|t| t["name"] == DELEGATE_TOOL)
            {
                tools.push(delegate_tool());
            }
        }
        if all || session.contains_key("instructions") {
            let own = session.get("instructions").and_then(Value::as_str);
            // The session reads the numbers of the client's (or the deployment's) instructions
            // as given.
            if let Some(own) = own {
                self.tell(ProviderEvent::Instructions(own.to_owned()));
            }
            let instructions = provider::with_instructions(own, self.suffix);
            session.insert("instructions".into(), instructions.into());
        }
        // The half-cascade: the vendor writes its replies as text, whatever the client asks.
        if self.half && (all || session.contains_key("output_modalities")) {
            session.insert("output_modalities".into(), json!(["text"]));
        }
    }

    /// The deployment's defaults update with the delegate function and the instructions, or
    /// an update of its own when the deployment has none.
    pub(super) fn defaults(&self, update: Option<String>, event_id: &str) -> String {
        let mut update = update
            .and_then(|u| serde_json::from_str::<Value>(&u).ok())
            .unwrap_or_else(|| {
                json!({"type": "session.update", "event_id": event_id,
                       "session": {"type": "realtime"}})
            });
        if let Some(session) = update["session"].as_object_mut() {
            self.steer(session, true);
        }
        update.to_string()
    }

    /// A client event: `Ok(Some)` (possibly rewritten) to go on, `Ok(None)` when it was the
    /// mode's, `Err` with why when it was the mode's and malformed.
    pub(super) fn client<'a>(
        &mut self,
        raw: &'a str,
    ) -> Result<Option<Cow<'a, str>>, &'static str> {
        // Audio goes on as it came, without a copy or a full parse of its payload.
        if kind_of(raw).is_none_or(|kind| kind == "input_audio_buffer.append") {
            return Ok(Some(Cow::Borrowed(raw)));
        }
        let Ok(mut event) = serde_json::from_str::<Value>(raw) else {
            return Ok(Some(Cow::Borrowed(raw)));
        };
        let msg = match event["type"].as_str() {
            Some("bud.task.answer") => {
                let (request_id, outcome) = task_answer(&event).ok_or(ANSWER_REQUIRED)?;
                Msg::TaskAnswer {
                    request_id,
                    outcome,
                }
            }
            Some("bud.task.cancel") => Msg::TaskCancel {
                task_id: event["task_id"].as_str().unwrap_or_default().to_owned(),
            },
            Some("bud.task.seen") => Msg::TaskSeen {
                task_id: event["task_id"].as_str().unwrap_or_default().to_owned(),
            },
            // The block is the operator's: a relay session changes none of it (as a voice
            // agent's session changes only what its `session_overrides` list).
            Some("bud.interaction.update") => {
                let refused: Vec<Value> = event["interaction"]
                    .as_object()
                    .into_iter()
                    .flat_map(|patch| patch.keys())
                    .map(|path| json!({"path": path, "reason": "not_allowed"}))
                    .collect();
                let updated = json!({"applied": [], "refused": refused});
                let _ = self
                    .client
                    .send(super::cascade::event("bud.interaction.updated", updated));
                return Ok(None);
            }
            Some("session.update") => {
                if let Some(session) = event["session"].as_object_mut() {
                    self.steer(session, false);
                }
                return Ok(Some(Cow::Owned(event.to_string())));
            }
            // The half-cascade: a reply the client asks for is text too.
            Some("response.create")
                if self.half && event["response"].get("output_modalities").is_some() =>
            {
                event["response"]["output_modalities"] = json!(["text"]);
                return Ok(Some(Cow::Owned(event.to_string())));
            }
            _ => return Ok(Some(Cow::Borrowed(raw))),
        };
        if self.inbox.try_send(msg).is_err() {
            warn!("interaction session busy; a client message was dropped");
        }
        Ok(None)
    }

    fn tell(&self, event: ProviderEvent) {
        if self.inbox.try_send(Msg::Provider(event)).is_err() {
            warn!("interaction session busy; a vendor event was dropped");
        }
    }

    /// A vendor event: what the session reads in it, and `Some` (possibly rewritten) for the
    /// client, `None` for a frame of the delegate function's.
    pub(super) fn vendor<'a>(&mut self, raw: &'a str) -> Option<Cow<'a, str>> {
        let Some(kind) = kind_of(raw) else {
            return Some(Cow::Borrowed(raw));
        };
        if kind == "response.output_audio.delta" {
            // The commonest frame: read its length only. The half-cascade's vendor audio never
            // reaches the caller.
            if self.half || !self.exec.gate().pass() {
                return None;
            }
            let delta = serde_json::from_str::<Delta>(raw).ok();
            let bytes = delta.map_or(0, |d| d.delta.len() * 3 / 4);
            self.exec.signal(Signal::Audio(bytes));
            return Some(Cow::Borrowed(raw));
        }
        let Ok(mut event) = serde_json::from_str::<Value>(raw) else {
            return Some(Cow::Borrowed(raw));
        };
        let kind = event["type"].as_str().unwrap_or_default().to_owned();
        let item = &event["item"];
        let ours = item["name"] == DELEGATE_TOOL
            || [&item["id"], &item["call_id"], &event["item_id"]]
                .iter()
                .any(|id| id.as_str().is_some_and(|id| self.hidden.contains(id)));
        match kind.as_str() {
            // The caller's turns, for the health watcher (chapter 10).
            "input_audio_buffer.speech_started" => self.tell(ProviderEvent::CallerSpeaking(true)),
            "input_audio_buffer.speech_stopped" | "input_audio_buffer.committed" => {
                self.tell(ProviderEvent::CallerSpeaking(false));
            }
            "error" => {
                let message = event["error"]["message"].as_str().unwrap_or("vendor error");
                self.tell(ProviderEvent::Failed(message.to_owned()));
            }
            "response.output_audio_transcript.delta" | "response.output_text.delta" => {
                let delta = event["delta"].as_str().unwrap_or_default();
                self.tell(ProviderEvent::Saying(delta.to_owned()));
            }
            "conversation.item.input_audio_transcription.completed" => {
                let words = event["transcript"].as_str().unwrap_or_default();
                self.tell(ProviderEvent::Heard(words.to_owned()));
            }
            "response.output_audio_transcript.done" | "response.output_text.done" => {
                let words = event["transcript"]
                    .as_str()
                    .or_else(|| event["text"].as_str())
                    .unwrap_or_default();
                self.tell(ProviderEvent::Said(words.to_owned()));
            }
            "response.created" => self.exec.signal(Signal::Responding),
            "response.output_item.added" | "response.output_item.done" if ours => {
                for id in [&item["id"], &item["call_id"]] {
                    if let Some(id) = id.as_str() {
                        self.hidden.insert(id.to_owned());
                    }
                }
                return None;
            }
            "response.function_call_arguments.delta" if ours => return None,
            "response.function_call_arguments.done" if ours => {
                let call_id = event["call_id"].as_str().unwrap_or_default().to_owned();
                let call = ProviderEvent::Call {
                    call_id: call_id.clone(),
                    name: DELEGATE_TOOL.to_owned(),
                    arguments: event["arguments"].as_str().unwrap_or_default().to_owned(),
                };
                // The client never sees this call: one the session cannot take is answered.
                if self.inbox.try_send(Msg::Provider(call)).is_err() {
                    warn!("interaction session busy; a delegate call was refused");
                    let output = json!({"status": "refused", "reason": "busy; try again"});
                    let epoch = self.exec.epoch();
                    self.exec
                        .tool_result(call_id, epoch, output.to_string(), true);
                }
                return None;
            }
            "conversation.item.created" | "conversation.item.added" | "conversation.item.done"
                if ours =>
            {
                return None;
            }
            "response.done" => {
                self.tell(ProviderEvent::ResponseDone);
                self.exec.signal(Signal::ResponseDone);
                if let Some(output) = event["response"]["output"].as_array_mut() {
                    let hidden = &self.hidden;
                    output.retain(|item| {
                        item["name"] != DELEGATE_TOOL
                            && !item["call_id"]
                                .as_str()
                                .is_some_and(|id| hidden.contains(id))
                    });
                }
                return Some(Cow::Owned(event.to_string()));
            }
            _ => {}
        }
        Some(Cow::Borrowed(raw))
    }

    /// The session is over.
    pub(super) fn shutdown(self) {
        // The session holds a sender of its own inbox and ends only at this message: it must
        // arrive, however full the inbox is.
        let inbox = self.inbox;
        tokio::spawn(async move {
            let _ = inbox.send(Msg::Shutdown { clean: true }).await;
        });
        self.exec.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix() -> RelayIx {
        let (inbox, _rx) = mpsc::channel(8);
        let (exec, _pending) = executor::handle();
        RelayIx {
            inbox,
            exec,
            suffix: provider::instructions(provider::Regime::Delegating),
            hidden: HashSet::new(),
            echo: Value::Null,
            client: mpsc::unbounded_channel().0,
            half: false,
        }
    }

    /// The half-cascade on the relay: every session the vendor gets, and every reply the client
    /// asks for, is text; the vendor's audio never reaches the client; its words are read.
    #[test]
    fn the_relays_half_cascade_asks_for_text_and_drops_the_vendors_audio() {
        let (inbox, mut rx) = mpsc::channel(8);
        let mut half = RelayIx {
            inbox,
            half: true,
            ..ix()
        };
        let defaults: Value = serde_json::from_str(&half.defaults(None, "e1")).unwrap();
        assert_eq!(defaults["session"]["output_modalities"], json!(["text"]));
        let update = json!({"type": "session.update", "session": {"output_modalities": ["audio"]}});
        let sent: Value =
            serde_json::from_str(&half.client(&update.to_string()).unwrap().unwrap()).unwrap();
        assert_eq!(sent["session"]["output_modalities"], json!(["text"]));
        // An update that does not name them keeps the session's.
        let voice = json!({"type": "session.update", "session": {"audio": {}}});
        let sent: Value =
            serde_json::from_str(&half.client(&voice.to_string()).unwrap().unwrap()).unwrap();
        assert!(sent["session"].get("output_modalities").is_none());
        let reply =
            json!({"type": "response.create", "response": {"output_modalities": ["audio"]}});
        let sent: Value =
            serde_json::from_str(&half.client(&reply.to_string()).unwrap().unwrap()).unwrap();
        assert_eq!(sent["response"]["output_modalities"], json!(["text"]));
        let audio = json!({"type": "response.output_audio.delta", "delta": "AAAA"});
        assert_eq!(half.vendor(&audio.to_string()), None);
        let text = json!({"type": "response.output_text.done", "text": "We open at nine."});
        assert!(half.vendor(&text.to_string()).is_some());
        assert!(matches!(
            rx.try_recv(),
            Ok(Msg::Provider(ProviderEvent::Said(words))) if words == "We open at nine."
        ));
        // The numbers of the client's instructions are the provider's to say.
        let told = json!({"type": "session.update", "session": {"instructions": "We open at 9."}});
        half.client(&told.to_string()).unwrap();
        assert!(matches!(
            rx.try_recv(),
            Ok(Msg::Provider(ProviderEvent::Instructions(text))) if text == "We open at 9."
        ));
        // Without the half-cascade nothing of it changes.
        let mut plain = ix();
        let (update, reply) = (update.to_string(), reply.to_string());
        let sent = plain.client(&update).unwrap().unwrap();
        assert!(sent.contains(r#""output_modalities":["audio"]"#), "{sent}");
        assert_eq!(
            plain.client(&reply).unwrap(),
            Some(Cow::Borrowed(reply.as_str()))
        );
        let voice_line: Value =
            serde_json::from_str(&gateway_response(&[0u8; 4], 1, "voice")[0]).unwrap();
        assert_eq!(voice_line["response"]["metadata"]["bud_kind"], "voice");
    }

    #[test]
    fn the_delegate_functions_frames_never_reach_the_client() {
        let mut ix = ix();
        let added = json!({"type": "response.output_item.added", "response_id": "r1",
                           "item": {"id": "item_1", "type": "function_call", "call_id": "call_1",
                                    "name": DELEGATE_TOOL}});
        assert_eq!(ix.vendor(&added.to_string()), None);
        let delta = json!({"type": "response.function_call_arguments.delta", "item_id": "item_1", "delta": "{"});
        assert_eq!(ix.vendor(&delta.to_string()), None);
        let done = json!({"type": "response.function_call_arguments.done", "item_id": "item_1",
                          "call_id": "call_1", "arguments": "{\"request\":\"x\"}"});
        assert_eq!(ix.vendor(&done.to_string()), None);
        let output = json!({"type": "conversation.item.added",
                            "item": {"id": "item_2", "type": "function_call_output", "call_id": "call_1"}});
        assert_eq!(ix.vendor(&output.to_string()), None);
        let response = json!({"type": "response.done", "response": {"id": "r1", "output": [
            {"type": "function_call", "name": DELEGATE_TOOL, "call_id": "call_1"},
            {"type": "function_call", "name": "client_tool", "call_id": "call_2"}]}});
        let forwarded: Value =
            serde_json::from_str(&ix.vendor(&response.to_string()).unwrap()).unwrap();
        assert_eq!(forwarded["response"]["output"].as_array().unwrap().len(), 1);
        assert_eq!(forwarded["response"]["output"][0]["name"], "client_tool");
        // A client function's call goes on as before.
        let theirs = json!({"type": "response.output_item.added",
                            "item": {"id": "item_3", "type": "function_call", "call_id": "call_3", "name": "lookup"}});
        assert!(ix.vendor(&theirs.to_string()).is_some());
    }

    #[test]
    fn every_session_update_keeps_the_delegate_function_and_the_instructions() {
        let mut ix = ix();
        let update = json!({"type": "session.update", "session": {
            "instructions": "Be brief.", "tools": [{"type": "function", "name": "lookup"}]}});
        let sent: Value =
            serde_json::from_str(&ix.client(&update.to_string()).unwrap().unwrap()).unwrap();
        let tools: Vec<&str> = sent["session"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(tools, ["lookup", DELEGATE_TOOL]);
        let instructions = sent["session"]["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("Be brief.\n\n") && instructions.contains(DELEGATE_TOOL));
        // An update of the voice alone changes nothing else.
        let voice =
            json!({"type": "session.update", "session": {"audio": {"output": {"voice": "x"}}}});
        let sent: Value =
            serde_json::from_str(&ix.client(&voice.to_string()).unwrap().unwrap()).unwrap();
        assert!(
            sent["session"].get("tools").is_none() && sent["session"].get("instructions").is_none()
        );
        // The deployment has no defaults: the relay sends its own update.
        let own: Value = serde_json::from_str(&ix.defaults(None, "evt_1")).unwrap();
        assert_eq!(own["type"], "session.update");
        assert_eq!(own["session"]["tools"][0]["name"], DELEGATE_TOOL);
        // The client's interaction messages are the mode's.
        let cancel = json!({"type": "bud.task.cancel", "task_id": "task_1"});
        assert_eq!(ix.client(&cancel.to_string()), Ok(None));
        let seen = json!({"type": "bud.task.seen", "task_id": "task_1"});
        assert_eq!(ix.client(&seen.to_string()), Ok(None));
    }

    #[tokio::test]
    async fn an_answer_it_cannot_read_is_refused_and_cancels_nothing() {
        let (inbox, mut rx) = mpsc::channel(8);
        let mut ix = RelayIx { inbox, ..ix() };
        let typo = json!({"type": "bud.task.answer", "request_id": "q1", "outcome": "approve"});
        assert_eq!(ix.client(&typo.to_string()), Err(ANSWER_REQUIRED));
        let unnamed = json!({"type": "bud.task.answer", "outcome": "accept"});
        assert_eq!(ix.client(&unnamed.to_string()), Err(ANSWER_REQUIRED));
        assert!(rx.try_recv().is_err(), "nothing reached the session");
        let cancel = json!({"type": "bud.task.answer", "request_id": "q1", "outcome": "cancel"});
        assert_eq!(ix.client(&cancel.to_string()), Ok(None));
        assert!(matches!(
            rx.try_recv(),
            Ok(Msg::TaskAnswer {
                outcome: HitlOutcome::Cancel,
                ..
            })
        ));
    }

    #[test]
    fn audio_and_unreadable_frames_go_on_uncopied() {
        let mut ix = ix();
        for raw in [
            r#"{"type": "response.output_audio.delta", "item_id": "i", "delta": "AAAA"}"#,
            "not json",
        ] {
            assert!(matches!(ix.vendor(raw), Some(Cow::Borrowed(same)) if same == raw));
        }
        let append = r#"{"type": "input_audio_buffer.append", "audio": "AAAA"}"#;
        assert!(matches!(ix.client(append), Ok(Some(Cow::Borrowed(same))) if same == append));
    }

    #[tokio::test]
    async fn the_end_reaches_a_session_whose_inbox_is_full() {
        let (inbox, mut rx) = mpsc::channel(1);
        inbox
            .try_send(Msg::TaskCancel {
                task_id: "t".into(),
            })
            .unwrap();
        let ix = RelayIx { inbox, ..ix() };
        ix.shutdown();
        assert!(matches!(rx.recv().await, Some(Msg::TaskCancel { .. })));
        assert!(matches!(
            rx.recv().await,
            Some(Msg::Shutdown { clean: true })
        ));
    }

    #[test]
    fn a_session_event_leaves_as_its_bud_event() {
        let frame = bud_frame(r#"{"type": "unverified_claim", "kind": "completion", "text": "x"}"#)
            .unwrap();
        let frame: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(frame["type"], "bud.claim.unverified");
        assert_eq!(frame["kind"], "completion");
        assert!(frame["event_id"].is_string());
        assert!(bud_frame(r#"{"type": "assistant_transcript"}"#).is_none());
    }

    /// The relay's own events: the gateway's lines as `bud.audio.segment`, the end as
    /// `bud.session.failed` (the socket closes after it), a failure line as one response of
    /// the gateway's, a settings change refused (the block is the operator's), and the
    /// vendor's turns and errors read for the health watcher.
    #[tokio::test]
    async fn the_relay_says_its_failure_lines_as_gateway_responses_and_ends_after_session_failed() {
        let segment =
            bud_frame(r#"{"type":"audio_segment","phase":"start","utterance_id":"g_1"}"#).unwrap();
        assert!(
            segment.contains(r#""type":"bud.audio.segment""#),
            "{segment}"
        );
        let failed = bud_frame(r#"{"type":"session_failed","service":"voice"}"#).unwrap();
        assert!(is_session_failed(&failed));
        assert!(!is_session_failed(&segment));

        let frames: Vec<Value> = gateway_response(&vec![0u8; 19_200], 3, "failure")
            .iter()
            .map(|f| serde_json::from_str(f).unwrap())
            .collect();
        let kinds: Vec<&str> = frames.iter().map(|f| f["type"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "response.created",
                "response.output_audio.delta",
                "response.output_audio.delta",
                "response.output_audio.done",
                "response.done"
            ]
        );
        assert_eq!(frames[0]["response"]["metadata"]["bud_source"], "gateway");
        assert_eq!(frames[4]["response"]["id"], "resp_waav_3");

        let (inbox, mut rx) = mpsc::channel(8);
        let (client, mut answers) = mpsc::unbounded_channel();
        let mut ix = RelayIx {
            inbox,
            client,
            ..ix()
        };
        let update = json!({"type": "bud.interaction.update", "interaction": {"policy": {}}});
        assert_eq!(ix.client(&update.to_string()), Ok(None));
        let answer: Value = serde_json::from_str(&answers.try_recv().unwrap()).unwrap();
        assert_eq!(answer["type"], "bud.interaction.updated");
        assert_eq!(
            answer["refused"][0],
            json!({"path": "policy", "reason": "not_allowed"})
        );
        for (frame, event) in [
            (
                json!({"type": "input_audio_buffer.speech_started"}),
                ProviderEvent::CallerSpeaking(true),
            ),
            (
                json!({"type": "input_audio_buffer.committed"}),
                ProviderEvent::CallerSpeaking(false),
            ),
            (
                json!({"type": "error", "error": {"message": "overloaded"}}),
                ProviderEvent::Failed("overloaded".into()),
            ),
            (
                json!({"type": "response.output_text.delta", "delta": "Su"}),
                ProviderEvent::Saying("Su".into()),
            ),
        ] {
            assert!(
                ix.vendor(&frame.to_string()).is_some(),
                "{frame}: forwarded"
            );
            assert!(
                matches!(rx.try_recv(), Ok(Msg::Provider(got)) if got == event),
                "{frame}"
            );
        }
    }
}
