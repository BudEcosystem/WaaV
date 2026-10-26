//! The interaction actor against test doubles, on a paused clock.
//!
//! Each test builds one session the way `handlers/ws/interaction.rs` does (a `VoiceManager` with
//! the metered pump, the speech worker, the actor, the chained callbacks) and then plays the
//! caller: the analyser's events and the speech-to-text results go in through the actor's inbox
//! and the provider callbacks, and the test reads what the client and the loudspeaker would get.
//!
//! The clock is tokio's paused clock, which moves only when every task waits. The doubles' HTTP
//! goes over loopback sockets, and a hop on a socket takes real time in which every task waits
//! too, so each rig paces the clock (`ix_mocks::pace_paused_clock`): a hop costs a millisecond
//! or two of virtual time, not the whole wait of whichever timer is next.
//!
//! One thing the tests keep clear of: the VoiceManager's own speech-final timers. A final piece
//! that is not a speech-final arms them, and one of them compares a real clock with virtual
//! sleeps. A test that sends such a piece ends the segment within 600 virtual milliseconds.

#[path = "mock_providers/budgateway.rs"]
mod budgateway;
#[path = "mock_providers/interaction.rs"]
mod ix_mocks;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use serial_test::serial;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use budgateway::Reply;
use ix_mocks::{
    BackendDouble, FrontDouble, FrontScript, Hit, LlmDouble, Round, SttDouble, ToolDouble,
    TtsDouble,
};
use waav_gateway::auth::SessionCredential;
use waav_gateway::core::agent::{AgentBrain, AgentSessionConfig};
use waav_gateway::core::audio::PumpControl;
use waav_gateway::core::conversation::{ConversationConfig, resolve_llm_legs};
use waav_gateway::core::interaction::actor::{self, ActorDeps, Background, InteractionHandle, Msg};
use waav_gateway::core::interaction::background::TextBackend;
use waav_gateway::core::interaction::bud_agent::AgentBackground;
use waav_gateway::core::interaction::config::{
    self, BackendKind, BackendSpec, FrontModelServer, ParseCtx, ServerInteraction, Tri,
};
use waav_gateway::core::interaction::controls::{Control, ControlSource};
use waav_gateway::core::interaction::persona::Persona;
use waav_gateway::core::interaction::remote::{self, CallbackRefused, Remote};
use waav_gateway::core::interaction::types::{DuplexEvent, HitlOutcome, Ms, Say, TaskState};
use waav_gateway::core::interaction::{clips, speech, tasks, tenant};
use waav_gateway::core::stt::{STTConfig, STTResult};
use waav_gateway::core::tts::{AudioData, TTSConfig};
use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
use waav_gateway::handlers::ws::messages::MessageRoute;
use waav_gateway::livekit::{LiveKitOperation, OperationQueue};

const TOOL_SECRET: &str = "s3cret-for-tools";
/// The playout lead the `/ws` pump runs with.
const LEAD_MS: u64 = 150;

#[derive(Default)]
struct Opts {
    /// The session's `interaction` block; `true` when `None`.
    interaction: Option<Value>,
    /// The operator's backend catalog; a block that names an entry runs on it.
    catalog: Vec<BackendSpec>,
    request_response: bool,
    /// The front model's URL.
    front: Option<String>,
    /// The server's default block: the operator's own settings and tools.
    operator: Option<Value>,
    /// The session's background is a Bud agent on a stand-in Bud gateway: what its entry
    /// holds besides the two legs. `interaction` is then the entry's block.
    agent: Option<Value>,
    /// A LiveKit session: the room's operation queue.
    room: Option<OperationQueue>,
    /// The session's background answers in text on its own, as a text-output DAG does.
    graph: Option<Arc<Graph>>,
    /// The session is a new connection of this caller's earlier one: its owner and its
    /// `stream_id`.
    back: Option<(String, String)>,
    /// Caller memory: the server's key, and the caller the session's verified token names.
    memory: Option<(&'static str, &'static str)>,
    /// The server's `interaction.clips_dir`.
    clips_dir: Option<String>,
    /// The speech-to-text's language (`multi`: it hears any).
    stt_language: Option<&'static str>,
}

/// A text backend that answers each request with the next scripted reply: pieces of text
/// `wait` milliseconds apart, or an error.
#[derive(Default)]
struct Graph {
    replies: Mutex<std::collections::VecDeque<Result<Vec<&'static str>, &'static str>>>,
    wait: u64,
    requests: Mutex<Vec<String>>,
    cancelled: Mutex<usize>,
}

#[async_trait::async_trait]
impl TextBackend for Graph {
    fn label(&self) -> &str {
        "dag"
    }

    fn side_effects(&self) -> bool {
        false
    }

    async fn run(
        &self,
        request: String,
        out: mpsc::Sender<String>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), String> {
        self.requests.lock().push(request);
        let reply = self.replies.lock().pop_front().unwrap_or(Ok(Vec::new()));
        for text in reply.map_err(str::to_owned)? {
            tokio::select! {
                _ = cancel.cancelled() => {
                    *self.cancelled.lock() += 1;
                    return Err("cancelled".to_owned());
                }
                _ = tokio::time::sleep(Duration::from_millis(self.wait)) => {}
            }
            let _ = out.send(text.to_owned()).await;
        }
        Ok(())
    }
}

/// One session with the actor attached, and everything it sends.
struct Rig {
    handle: InteractionHandle,
    actor: JoinHandle<()>,
    ctl: Arc<PumpControl>,
    tts: Arc<TtsDouble>,
    stt: Arc<SttDouble>,
    llm: LlmDouble,
    /// The stand-in Bud gateway of an agent session.
    bud: Option<budgateway::Gateway>,
    /// Every client event with the time it was sent.
    wire: Arc<Mutex<Vec<(Ms, Value)>>>,
    /// Every 20 ms frame that reached the client: the time and its loudest sample.
    frames: Arc<Mutex<Vec<(Ms, i16)>>>,
    owner: String,
    stream_id: String,
    _vm: Arc<VoiceManager>,
    /// The front model double of a test that brings none: it reads each turn in context.
    _front: Option<FrontDouble>,
}

impl Rig {
    async fn start(opts: Opts) -> Rig {
        // The doubles listen on loopback.
        unsafe {
            std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
        }
        ix_mocks::pace_paused_clock();
        let tts = TtsDouble::register();
        let stt = SttDouble::register(opts.request_response);
        let llm = LlmDouble::start().await;
        // Interaction mode decides every turn with the front model (chapter 9).
        let own_front = match &opts.front {
            Some(_) => None,
            None => Some(
                FrontDouble::start(FrontScript {
                    decide: Some(ix_mocks::reads_the_turn),
                    ..FrontScript::default()
                })
                .await,
            ),
        };
        let front_url = (opts.front.clone()).or_else(|| own_front.as_ref().map(|f| f.url.clone()));

        let mut stt_config = STTConfig {
            provider: stt.provider.clone(),
            api_key: "test".to_owned(),
            ..Default::default()
        };
        if let Some(language) = opts.stt_language {
            stt_config.language = language.to_owned();
        }
        let tts_config = TTSConfig {
            provider: tts.provider.clone(),
            ..Default::default()
        };
        let vm = VoiceManager::new(VoiceManagerConfig::new(stt_config, tts_config), None);
        let vm = Arc::new(vm.expect("voice manager"));
        vm.start().await.expect("voice manager start");

        // The metered pump, set before the egress callback.
        let (pump_tx, pump_rx) = mpsc::unbounded_channel();
        let ctl = Arc::new(PumpControl::new(pump_tx.clone(), LEAD_MS));
        vm.set_metered_playback(Some(ctl.clone()));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let (sink, clock) = (frames.clone(), ctl.clone());
        vm.on_tts_audio(move |audio: AudioData| {
            let (samples, _) = audio.data.as_chunks::<2>();
            let peak = samples
                .iter()
                .map(|s| i16::from_le_bytes(*s).saturating_abs());
            sink.lock().push((clock.now_ms(), peak.max().unwrap_or(0)));
            Box::pin(async {})
        })
        .await
        .expect("egress callback");
        let done = ctl.clone();
        vm.on_tts_complete(move || {
            done.synth_done();
            Box::pin(async {})
        })
        .await
        .expect("completion callback");

        let server = Arc::new(ServerInteraction {
            enabled: true,
            front_model: front_url.map(|url| FrontModelServer {
                url,
                model: None,
                api_key: None,
                command: None,
                licence: None,
            }),
            tool_secret: Some(TOOL_SECRET.to_owned()),
            allow_client_tools: true,
            default: opts.operator,
            agent_default: None,
            task_redis_url: None,
            task_backends: opts.catalog.clone(),
            allow_client_backends: true,
            allow_resume_key: true,
            allow_client_push: true,
            caller_memory_key: opts.memory.map(|(key, _)| key.to_owned()),
            clips_dir: opts.clips_dir.clone(),
            ..Default::default()
        });
        let block = opts.interaction.unwrap_or(json!(true));
        let ctx = ParseCtx {
            operator: opts.agent.is_some(),
            bud_mode: opts.agent.is_some(),
        };
        let (cfg, warnings) = config::parse(&block, &server, ctx);
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut cfg = cfg.expect("interaction mode is on");
        // As `handlers/ws/interaction.rs` does once the caller's token is verified.
        cfg.front.caller = opts.memory.map(|(_, sub)| sub.to_owned());
        let conversation = ConversationConfig {
            base_url: llm.base_url.clone(),
            model: "mock".to_owned(),
            api_key: Some("test-key".to_owned()),
            ..Default::default()
        };

        let (out, mut out_rx) = mpsc::channel(256);
        let own = (
            format!("owner-of-{}", stt.provider),
            format!("stream-of-{}", stt.provider),
        );
        let (owner, stream_id) = opts.back.unwrap_or(own);
        // As `handlers/ws/interaction.rs` does: the language this caller asked for before.
        if let Some((key, sub)) = opts.memory {
            cfg.front.language_wish = tenant::language_wish(key, &owner, sub).await;
        }
        // As `handlers/ws/interaction.rs` prepares the session's background.
        let named = cfg.tasks.backend.clone();
        let remote = opts
            .catalog
            .iter()
            .find(|b| Some(&b.name) == named.as_ref());
        let (background, bud) = match (opts.agent, opts.graph) {
            (None, None) if remote.is_some() => {
                let spec = remote.cloned().expect("a catalog entry");
                if spec.kind != BackendKind::OpenaiResponses {
                    cfg.tasks.tools.clear();
                }
                cfg.tasks.speculative_start = Tri::Off;
                let (secret, tools) = (server.tool_secret.clone(), cfg.tasks.tools.clone());
                let prompt = ["", "You answer callers of the Larkspur shop."];
                (
                    Background::Remote(Remote::new(spec, secret, prompt, tools)),
                    None,
                )
            }
            (None, Some(graph)) => {
                cfg.tasks.tools.clear();
                cfg.tasks.speculative_start = Tri::Off;
                (Background::Text(graph), None)
            }
            (Some(extra), _) => {
                let (base, bud) = budgateway::gateway().await;
                cfg.tasks.tools.clear();
                cfg.tasks.speculative_start = Tri::Off;
                let session = AgentSessionConfig {
                    session_id: stream_id.clone(),
                    prompt_name: "support".to_owned(),
                    version: 2,
                    entry: agent_entry(extra),
                    conversation_id: "conv_voice_s".to_owned(),
                    variables: None,
                    text_only: false,
                };
                let brain = Arc::new(AgentBrain::new(base));
                let credential = SessionCredential::new("bud-caller-key");
                let marker = cfg.tasks.answer_marker.clone();
                let agent = AgentBackground::new(brain, session, credential, marker);
                (Background::Agent(agent), Some(bud))
            }
            (None, None) => {
                let (fast, reasoning) = resolve_llm_legs(&conversation, None).expect("llm legs");
                (Background::Llm(fast, reasoning), None)
            }
        };
        // A caller comes back with the session's resume key, else its stream_id.
        let comes_back = tasks::back_id(&stream_id, cfg.tasks.resume_key.as_deref());
        let (handle, actor) = actor::spawn(ActorDeps {
            vm: vm.clone(),
            ctl: ctl.clone(),
            pump_rx,
            speech: speech::spawn(vm.clone(), ctl.clone(), pump_tx),
            out,
            background,
            system_prompt: Some("You answer callers of the Larkspur shop.".to_owned()),
            credential: None,
            attribution: None,
            server,
            cfg,
            stream_id: stream_id.clone(),
            refs_used: tasks::store().refs_used(&owner, &stream_id).await,
            owner: owner.clone(),
            request_response: vm.stt_is_request_response().await,
            provider_turns: false,
            stt_end_turn_after_ms: None,
            warm: true,
            plain_path: true,
            room: opts.room,
        });
        // As `handlers/ws/interaction.rs` does: the session waits for what the tasks of an
        // earlier connection of its caller came to.
        tokio::spawn(actor::follow(owner.clone(), comes_back, handle.clone()));

        let wire = Arc::new(Mutex::new(Vec::new()));
        let (seen, clock) = (wire.clone(), ctl.clone());
        tokio::spawn(async move {
            while let Some(route) = out_rx.recv().await {
                let event = match route {
                    MessageRoute::Json(text) => serde_json::from_str(&text).expect("a JSON event"),
                    // The session failed (chapter 10): the actor closes the socket.
                    MessageRoute::CloseWith { code, reason } => {
                        json!({ "type": "close", "code": code, "reason": reason })
                    }
                    route => panic!("the actor sends only pre-serialised events: {route:?}"),
                };
                seen.lock().push((clock.now_ms(), event));
            }
        });

        // The provider callbacks: interims may be dropped, everything else waits for room.
        let inbox = handle.clone();
        vm.on_stt_result(move |result| {
            let inbox = inbox.clone();
            Box::pin(async move {
                if result.is_final {
                    inbox.send(Msg::Stt(Box::new(result))).await;
                } else {
                    inbox.try_send(Msg::Stt(Box::new(result)));
                }
            })
        })
        .await
        .expect("STT result callback");
        let inbox = handle.clone();
        vm.on_stt_error(move |error| {
            let inbox = inbox.clone();
            Box::pin(async move {
                inbox.send(Msg::SttError(error.to_string())).await;
            })
        })
        .await
        .expect("STT error callback");
        let inbox = handle.clone();
        vm.on_tts_error(move |error| {
            let inbox = inbox.clone();
            Box::pin(async move {
                inbox.send(Msg::TtsError(error.to_string())).await;
            })
        })
        .await
        .expect("TTS error callback");

        Rig {
            handle,
            actor,
            ctl,
            tts,
            stt,
            llm,
            bud,
            wire,
            frames,
            owner,
            stream_id,
            _vm: vm,
            _front: own_front,
        }
    }

    /// A session with these tools of the operator's (the server signs only its own); each is
    /// `(name, phrase, irreversible)` on `url`.
    async fn with_tools(url: &str, tools: &[(&str, &str, bool)]) -> Rig {
        let tools: Vec<Value> = tools
            .iter()
            .map(|(name, phrase, irreversible)| {
                json!({
                    "name": name, "description": format!("Use it to {phrase}."), "url": url,
                    "parameters": { "type": "object", "properties": {} },
                    "side_effects": true, "irreversible": irreversible, "confirm_phrase": phrase,
                })
            })
            .collect();
        let operator = Some(json!({ "tasks": { "tools": tools } }));
        Rig::start(Opts {
            operator,
            ..Opts::default()
        })
        .await
    }

    fn now(&self) -> Ms {
        self.ctl.now_ms()
    }

    async fn run(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    async fn send(&self, msg: Msg) {
        assert!(self.handle.send(msg).await, "the actor is gone");
    }

    async fn duplex(&self, event: DuplexEvent) {
        self.send(Msg::Duplex(event)).await;
    }

    /// The session is ready; the warm-up lines are synthesised before the caller speaks.
    async fn ready(&self) {
        self.send(Msg::Ready).await;
        self.run(2_000).await;
    }

    /// The caller starts to speak: the analyser's onset. Returns its time.
    async fn onset(&self) -> Ms {
        let at = self.now();
        self.duplex(DuplexEvent::Onset {
            at,
            level_db: -20.0,
            reflex_ducked: false,
        })
        .await;
        at
    }

    /// The caller stopped `voiced_ms` of speech 200 ms ago: the analyser's offset and its
    /// first silence report.
    async fn offset(&self, voiced_ms: u32) {
        let at = self.now();
        self.duplex(DuplexEvent::Offset { at, voiced_ms }).await;
        self.silence(200).await;
    }

    async fn silence(&self, ms: u32) {
        let at = self.now();
        self.duplex(DuplexEvent::Silence { at, ms }).await;
    }

    async fn smart_turn(&self, p: f32) {
        let at = self.now();
        self.duplex(DuplexEvent::SmartTurn { at, p }).await;
    }

    /// The caller speaks for `ms` and stops. Returns the onset time.
    async fn speech(&self, ms: u32) -> Ms {
        let onset = self.onset().await;
        self.run(u64::from(ms) + 200).await;
        self.offset(ms).await;
        onset
    }

    /// One caller turn on streaming speech-to-text: speech, Smart Turn's verdict that it is
    /// over, then the vendor's speech-final, which is E.
    async fn turn(&self, text: &str) {
        self.speech(600).await;
        self.smart_turn(0.9).await;
        self.stt.speech_final(text).await;
    }

    fn wire(&self) -> Vec<(Ms, Value)> {
        self.wire.lock().clone()
    }

    /// The client events of one type, in order.
    fn events(&self, kind: &str) -> Vec<Value> {
        let of_kind = |(_, event): (Ms, Value)| (event["type"] == kind).then_some(event);
        self.wire().into_iter().filter_map(of_kind).collect()
    }

    /// Every agent line that was put on the wire with its words, in order.
    fn lines(&self) -> Vec<Value> {
        let worded = |event: &Value| event["delta"] != "";
        let transcripts = self.events("assistant_transcript");
        transcripts.into_iter().filter(worded).collect()
    }

    fn said(&self) -> Vec<String> {
        let delta = |line: Value| line["delta"].as_str().unwrap_or_default().to_owned();
        self.lines().into_iter().map(delta).collect()
    }

    /// The line with these words; the test fails when the agent has not said it.
    fn line(&self, words: &str) -> Value {
        let found = self.lines().into_iter().find(|line| line["delta"] == words);
        found.unwrap_or_else(|| panic!("the agent never said {words:?}\n{}", self.timeline()))
    }

    /// How much of the utterance with this id the caller has heard, by its latest transcript.
    fn heard(&self, utterance_id: &Value) -> String {
        let transcripts = self.events("assistant_transcript");
        let of_utterance = |event: &&Value| &event["utterance_id"] == utterance_id;
        let last = transcripts.iter().rev().find(of_utterance);
        last.map_or("", |event| event["heard"].as_str().unwrap_or_default())
            .to_owned()
    }

    /// Whether the agent said these words and the caller heard all of them.
    fn heard_in_full(&self, words: &str) -> bool {
        let said = self.lines().into_iter().find(|line| line["delta"] == words);
        said.is_some_and(|line| self.heard(&line["utterance_id"]) == "full")
    }

    /// When the first event that matches was sent; the test fails when there is none.
    fn time_of(&self, what: &str, matches: impl Fn(&Value) -> bool) -> Ms {
        let found = self.wire().into_iter().find(|(_, event)| matches(event));
        let (at, _) = found.unwrap_or_else(|| panic!("no event: {what}\n{}", self.timeline()));
        at
    }

    /// Lets the session run until `done`, checking every 10 ms; fails after `limit_ms`.
    async fn until(&self, what: &str, limit_ms: u64, done: impl Fn(&Rig) -> bool) {
        let deadline = self.now() + limit_ms;
        while !done(self) {
            assert!(
                self.now() < deadline,
                "not within {limit_ms} ms: {what}\n{}",
                self.timeline()
            );
            self.run(10).await;
        }
    }

    async fn until_heard(&self, words: &str) {
        let what = format!("the caller hears {words:?}");
        self.until(&what, 30_000, |rig| rig.heard_in_full(words))
            .await;
    }

    async fn until_event(&self, kind: &str, count: usize) {
        let what = format!("{count} {kind} event(s)");
        self.until(&what, 30_000, |rig| rig.events(kind).len() >= count)
            .await;
    }

    /// Everything the session sent and synthesised, for a failed assertion.
    fn timeline(&self) -> String {
        let event = |(at, event): (Ms, Value)| format!("{at:>7} {event}");
        let mut lines: Vec<String> = self.wire().into_iter().map(event).collect();
        lines.push(format!("synthesised: {:?}", self.tts.spoken()));
        lines.push(format!("frames: {}", self.frames.lock().len()));
        lines.join("\n")
    }

    /// The frames that reached the client in `(after, until]`.
    fn frames_in(&self, after: Ms, until: Ms) -> Vec<(Ms, i16)> {
        let within = |(at, _): &&(Ms, i16)| after < *at && *at <= until;
        self.frames.lock().iter().filter(within).copied().collect()
    }
}

/// A Bud agent's entry: the two legs it cannot do without, and `extra`.
fn agent_entry(extra: Value) -> Arc<bud_auth::VoiceAgentEntry> {
    let mut entry = json!({
        "prompt_id": "c0de", "version": 2,
        "stt": {"endpoint_id": "s"}, "tts": {"endpoint_id": "t"},
    });
    let extra = extra.as_object().expect("an object").clone();
    entry.as_object_mut().expect("an object").extend(extra);
    Arc::new(bud_auth::parse_voice_agent_blob(&entry.to_string()).expect("an entry"))
}

/// A plain answer in two sentences, the first released `first_ms` after the request.
fn answer(first_ms: u64) -> Round {
    Round::text(
        first_ms,
        20,
        &["The store opens at nine. ", "It closes ", "at six."],
    )
}

fn is_ack(event: &Value) -> bool {
    event["type"] == "assistant_transcript" && event["kind"] == "ack" && event["delta"] != ""
}

fn says(words: &'static str) -> impl Fn(&Value) -> bool {
    move |event| event["type"] == "assistant_transcript" && event["delta"] == words
}

/// The words the default persona may withdraw an answer with.
fn withdrawals() -> HashSet<String> {
    let persona = Persona::load("default");
    let bank = Say::Bank {
        group: "withdrawal",
        intent: None,
    };
    (0..16).filter_map(|n| persona.resolve(&bank, n)).collect()
}

// ---- session start

#[tokio::test(start_paused = true)]
#[serial]
async fn warm_up_lines_are_synthesised_and_never_played() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;

    // The warm-up lines first (the first turn's acknowledgement is one of them), then the
    // listening sounds for the clip bank, then the lines said when a service fails (chapter
    // 10). Live L1/L3: with the warm-up lines last the first acknowledgements were
    // synthesised while the caller waited.
    let persona = Persona::load("default");
    let mut expected = persona.warm_lines();
    expected.extend(persona.backchannel_lines().to_vec());
    expected.extend(persona.failure_lines().into_iter().map(|line| line.text));
    assert_eq!(rig.tts.spoken(), expected);
    assert!(
        rig.frames.lock().is_empty(),
        "a warm-up line or a clip was played"
    );
    assert!(rig.wire().is_empty(), "{}", rig.timeline());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_reply_does_not_wait_for_the_warm_up() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(900));
    // The caller speaks at once: the warm-up has barely begun.
    rig.send(Msg::Ready).await;
    rig.turn("what time does the store open").await;
    let e = rig.now();
    rig.until("the acknowledgement", 2_000, |rig| {
        rig.wire().iter().any(|(_, event)| is_ack(event))
    })
    .await;

    // Due at E + 300 ms, behind at most the one warm-up line in synthesis.
    let acknowledged = rig.time_of("the acknowledgement", is_ack);
    assert!(acknowledged <= e + 450, "E {e}, heard at {acknowledged}");
    assert!(
        rig.tts.spoken().len() < 6,
        "the warm-up went on over the turn"
    );

    // Once the call is quiet again the rest is synthesised.
    rig.until_heard("It closes at six.").await;
    rig.run(3_000).await;
    let spoken = rig.tts.spoken();
    for line in Persona::load("default").warm_lines() {
        assert!(spoken.contains(&line), "never warmed: {line}");
    }
}

// ---- lookups

/// A thinking pause: the vendor's speech-final comes in it, but Smart Turn is sure the caller
/// has not finished (or has not judged the pause yet). Nothing is said and no task starts in
/// it; the caller goes on, and the turn is one request with all the words.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_thinking_pause_is_held_and_the_turn_goes_on() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(300));
    rig.ready().await;
    rig.speech(1_500).await;
    rig.smart_turn(0.02).await;
    rig.stt.speech_final("I want to know when the store").await;
    rig.run(600).await;
    assert!(rig.lines().is_empty(), "{}", rig.timeline());
    // The caller goes on before the hold ends; the final of this stretch comes before Smart
    // Turn's verdict on its pause, which is sure.
    rig.speech(800).await;
    rig.stt.speech_final("opens on Sunday").await;
    rig.run(300).await;
    assert!(rig.events("task_created").is_empty(), "{}", rig.timeline());
    rig.smart_turn(0.9).await;
    let e = rig.now();
    rig.until_event("task_created", 1).await;
    let created = &rig.events("task_created")[0];
    assert_eq!(
        created["title"],
        "I want to know when the store opens on Sunday"
    );
    let requests = rig.llm.requests();
    assert_eq!(
        ix_mocks::messages(&requests[requests.len() - 1], "user"),
        ["I want to know when the store opens on Sunday"]
    );
    rig.until_heard("It closes at six.").await;
    let first = rig.time_of("the first line", |event| {
        event["type"] == "assistant_transcript" && event["delta"] != ""
    });
    assert!(first >= e, "E {e}, first line {first}");
    assert_eq!(rig.events("task_created").len(), 1);
}

/// Smart Turn took a pause for the end, the answer is still with a slow synthesiser, and the
/// caller goes on within 1.5 s (the Candor pauses of the experience run, fin-experience): none
/// of it is heard over them. It is taken back and said once their turn has ended.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_answer_not_yet_heard_waits_when_the_caller_goes_on() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(100));
    rig.llm
        .push(Round::text(100, 20, &["On Sunday it opens at ten."]));
    rig.ready().await;
    rig.tts
        .synth_ms
        .store(2_000, std::sync::atomic::Ordering::SeqCst);
    rig.turn("When does the store open").await;
    let e = rig.now();
    rig.until_event("task_created", 1).await;
    rig.run(600u64.saturating_sub(rig.now() - e)).await;
    let went_on = rig.speech(1_200).await;
    assert!(went_on <= e + 1_500, "E {e}, went on {went_on}");
    rig.smart_turn(0.9).await;
    rig.stt.speech_final("and on Sunday").await;
    let e2 = rig.now();
    rig.until_heard("The store opens at nine.").await;
    let stopped = rig.events("agent_audio_stopped");
    assert!(
        stopped.iter().any(|event| event["reason"] == "policy"),
        "{}",
        rig.timeline()
    );
    let first = rig.time_of("the first answer", says("The store opens at nine."));
    assert!(
        first >= e2,
        "E2 {e2}, first answer {first}\n{}",
        rig.timeline()
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_lookup_is_acknowledged_then_relayed_sentence_by_sentence() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(900));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    let e = rig.now();
    rig.until_heard("It closes at six.").await;
    rig.until_event("task_result", 1).await;

    // The run started at E and was adopted: one request, the caller's words as its one turn.
    let requests = rig.llm.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        ix_mocks::messages(&requests[0], "user"),
        ["what time does the store open"]
    );
    let system = ix_mocks::messages(&requests[0], "system").join("\n");
    assert!(
        system.ends_with("You answer callers of the Larkspur shop."),
        "{system}"
    );
    assert_ne!(system, "You answer callers of the Larkspur shop.");

    let created = &rig.events("task_created")[0];
    assert_eq!(created["speculative"], true);
    assert_eq!(created["state"], "working");
    assert_eq!(created["title"], "what time does the store open");

    // The acknowledgement at E + 300 ms, then the answer in the model's own words.
    let ack = rig.lines()[0].clone();
    assert!(is_ack(&ack), "{}", rig.timeline());
    assert_eq!(
        (&ack["source"], &ack["verbatim"]),
        (&json!("gateway"), &json!(false))
    );
    let acknowledged = rig.time_of("the acknowledgement", is_ack);
    assert!(
        (e + 300..=e + 400).contains(&acknowledged),
        "E {e}, {acknowledged}"
    );

    let first = rig.line("The store opens at nine.");
    let second = rig.line("It closes at six.");
    assert_eq!(rig.said().len(), 3, "{}", rig.timeline());
    for (seq, sentence) in [&first, &second].into_iter().enumerate() {
        assert_eq!(sentence["source"], "background");
        assert_eq!(sentence["kind"], "relay");
        assert_eq!(sentence["verbatim"], true);
        assert_eq!(sentence["task_id"], created["task_id"]);
        assert_eq!(sentence["sentence"], json!({ "attempt": 0, "seq": seq }));
        assert_eq!(sentence["heard"], "none", "nothing is heard when it starts");
    }
    // Two sentences are two utterances, and each is reported heard on its own.
    assert_ne!(first["utterance_id"], second["utterance_id"]);
    for utterance in [&ack, &first, &second] {
        let id = &utterance["utterance_id"];
        assert_eq!(rig.heard(id), "full", "{id}\n{}", rig.timeline());
    }
    let order = [
        rig.time_of("task_created", |event| event["type"] == "task_created"),
        acknowledged,
        rig.time_of("sentence 0", says("The store opens at nine.")),
        rig.time_of("sentence 1", says("It closes at six.")),
    ];
    assert!(order.is_sorted(), "{order:?}");

    let result = &rig.events("task_result")[0];
    assert_eq!(result["state"], "completed");
    assert_eq!(result["effects"], "none_committed");
    assert_eq!(result["streamed"], true);
    assert_eq!(
        result["summary"],
        "The store opens at nine. It closes at six."
    );

    // The floor went from the caller to the agent and back to nobody.
    let floor = rig.events("floor_state");
    let holders: Vec<&str> = floor.iter().filter_map(|f| f["holder"].as_str()).collect();
    assert_eq!(holders.first(), Some(&"user"));
    assert_eq!(holders.last(), Some(&"none"));
    assert!(floor.iter().any(|f| f["reason"] == "front_ack"));
    assert!(
        floor
            .iter()
            .any(|f| f["reason"] == "relay" && f["lane"] == "main")
    );
    assert_eq!(
        tasks::store().list(&rig.owner, None)[0].state,
        TaskState::Completed
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn say_that_again_repeats_the_last_line_the_caller_heard() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.respond_with(|_| Some(answer(900)));
    rig.ready().await;

    // Nothing was said yet: the agent says so (a control, chapter 9).
    rig.turn("say that again").await;
    rig.until("a reply", 5_000, |rig| {
        rig.lines()
            .first()
            .is_some_and(|line| rig.heard(&line["utterance_id"]) == "full")
    })
    .await;
    assert_eq!(rig.said()[0], "Sorry, I don't have that to say again.");

    // Long enough that "say that again" is not taken for the echo of "could you say that
    // again?" (the transcript filter looks 5 s back from the caller's onset).
    rig.run(1_500).await;
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;
    rig.run(500).await;
    rig.turn("sorry, say that again").await;
    rig.until("the repeat", 5_000, |rig| rig.said().len() == 5)
        .await;

    // The last line heard in full, said again by the gateway; no task and no run for it.
    let repeat = rig.lines()[4].clone();
    assert_eq!(repeat["delta"], "It closes at six.");
    assert_eq!(
        (&repeat["kind"], &repeat["source"]),
        (&json!("answer"), &json!("gateway"))
    );
    assert_eq!(repeat["verbatim"], false);
    assert_eq!(rig.events("task_created").len(), 1, "no task for a control");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_fast_answer_needs_no_acknowledgement() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    // Three answers whose first sentence is ready before the acknowledgement is due.
    for turn in 1..=3 {
        rig.llm.push(answer(100));
        rig.turn("what time does the store open").await;
        rig.until_event("task_result", turn).await;
        rig.until("the answer is heard", 20_000, |rig| {
            let last = rig.lines().pop();
            rig.said().len() == 2 * turn
                && last.is_some_and(|last| rig.heard(&last["utterance_id"]) == "full")
        })
        .await;
        rig.run(500).await;
    }
    // Now the answers are known to be fast: the acknowledgement waits until E + 700 ms, and
    // an answer at E + 520 ms comes first.
    rig.llm.push(answer(500));
    rig.turn("and when does it close").await;
    rig.until_event("task_result", 4).await;
    rig.run(8_000).await;

    assert!(
        !rig.wire().iter().any(|(_, event)| is_ack(event)),
        "{}",
        rig.timeline()
    );
    assert_eq!(rig.said().len(), 8, "four answers of two sentences");
    let requests = rig.llm.requests();
    assert_eq!(requests.len(), 4);
    // What the caller said and heard so far is the history of the next run.
    assert_eq!(
        ix_mocks::messages(&requests[1], "user"),
        ["what time does the store open"; 2]
    );
    assert_eq!(
        ix_mocks::messages(&requests[1], "assistant"),
        ["The store opens at nine.", "It closes at six."]
    );
}

/// A run's tool calls stay in the call's history, right after the request they served: a later
/// run sees what was done for a request like its own (live D1/D2: with only the lines the
/// caller heard after the first request, "One moment." and "To confirm:", the 3B background
/// wrote those again and looked nothing up).
#[tokio::test(start_paused = true)]
#[serial]
async fn a_later_run_sees_the_tool_calls_made_for_an_earlier_request() {
    let tool = ToolDouble::start(json!({ "available": true })).await;
    let tools = json!([{
        "name": "check_availability", "description": "Use it to look up free tables.",
        "url": tool.url,
        "parameters": { "type": "object", "properties": { "day": { "type": "string" } } },
    }]);
    let rig = Rig::start(Opts {
        operator: Some(json!({ "tasks": { "tools": tools } })),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        let last = request["messages"].as_array()?.last()?.clone();
        Some(if last["role"] == "tool" {
            Round::text(50, 20, &["ANSWER:\n", "There are free tables."])
        } else {
            Round::tool(200, "check_availability", json!({ "day": "saturday" }))
        })
    });
    rig.ready().await;
    for (turn, words) in [
        (1, "are there tables on saturday"),
        (2, "what about sunday"),
    ] {
        rig.turn(words).await;
        rig.until_event("task_result", turn).await;
        rig.until_heard("There are free tables.").await;
        rig.run(6_000).await;
    }
    let requests = rig.llm.requests();
    assert_eq!(requests.len(), 4, "{}", rig.timeline());
    let history = requests[2]["messages"]
        .as_array()
        .expect("messages")
        .clone();
    let first = (history.iter())
        .position(|message| message["content"] == "are there tables on saturday")
        .expect("the first request");
    let call = &history[first + 1];
    assert_eq!(call["role"], "assistant");
    assert_eq!(
        call["tool_calls"][0]["function"]["name"],
        "check_availability"
    );
    let result = &history[first + 2];
    assert_eq!(result["role"], "tool");
    assert_eq!(result["tool_call_id"], call["tool_calls"][0]["id"]);
    assert_eq!(
        history.last().expect("request")["content"],
        "what about sunday"
    );
    assert_eq!(tool.calls().len(), 2);
}

/// Live D1: after a lookup's result the 3B background booked the table it had found, unasked. A
/// run whose first calls only looked something up is offered no tool that changes something in
/// its later rounds; the caller's next request is a run of its own, offered every tool again.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_lookup_run_is_offered_no_tool_that_changes_something_after_its_lookup() {
    let tool = ToolDouble::start(json!({ "available": true })).await;
    let lookup = |name: &str, side_effects: bool| {
        json!({
            "name": name, "description": "Use it for tables.", "url": tool.url,
            "parameters": { "type": "object", "properties": { "day": { "type": "string" } } },
            "side_effects": side_effects, "confirm_phrase": "book a table",
        })
    };
    let tools = json!([
        lookup("check_availability", false),
        lookup("book_table", true)
    ]);
    let rig = Rig::start(Opts {
        operator: Some(json!({ "tasks": { "tools": tools } })),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        let last = request["messages"].as_array()?.last()?.clone();
        Some(if last["role"] == "tool" {
            Round::text(50, 20, &["ANSWER:\n", "There are free tables."])
        } else {
            Round::tool(200, "check_availability", json!({ "day": "saturday" }))
        })
    });
    rig.ready().await;
    for (turn, words) in [(1, "are there tables on saturday"), (2, "and on sunday")] {
        rig.turn(words).await;
        rig.until_event("task_result", turn).await;
        rig.until_heard("There are free tables.").await;
        rig.run(6_000).await;
    }
    let offered = |request: &Value| -> Vec<String> {
        let tools = request["tools"].as_array().cloned().unwrap_or_default();
        let name = |tool: &Value| tool["function"]["name"].as_str().map(str::to_owned);
        tools.iter().filter_map(name).collect()
    };
    let requests = rig.llm.requests();
    assert_eq!(requests.len(), 4, "{}", rig.timeline());
    let booking = |request: &Value| offered(request).contains(&"book_table".to_owned());
    let lookup = |request: &Value| offered(request).contains(&"check_availability".to_owned());
    // Each run's first round has both; the round after its lookup only the lookup.
    assert!(booking(&requests[0]) && booking(&requests[2]));
    assert!(!booking(&requests[1]) && !booking(&requests[3]));
    assert!(lookup(&requests[1]) && lookup(&requests[3]));
    assert_eq!(tool.calls().len(), 2);
}

// ---- confirmations

/// The background books a table: the first run asks for two, the corrected run for four, and
/// once the tool has answered the model says so.
fn booking(request: &Value) -> Option<Round> {
    let user = ix_mocks::messages(request, "user").join("\n");
    Some(if !ix_mocks::messages(request, "tool").is_empty() {
        Round::text(50, 20, &["ANSWER:\n", "Your table for four ", "is booked."])
    } else if user.contains("make it four") {
        Round::tool(600, "book_table", json!({ "party": 4, "day": "Saturday" }))
    } else {
        Round::tool(600, "book_table", json!({ "party": 2, "day": "Saturday" }))
    })
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_booking_is_confirmed_corrected_and_called_once() {
    let tool = ToolDouble::start(json!({ "effect": "booked a table for 4 on Saturday" })).await;
    let rig = Rig::with_tools(&tool.url, &[("book_table", "book a table", true)]).await;
    rig.llm.respond_with(booking);
    rig.ready().await;
    rig.turn("can you book a table for two on saturday").await;

    // The tool call is held and the caller is asked: lead-in, read-back, question.
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "confirm");
    assert_eq!(question["irreversible"], true);
    assert_eq!(
        question["prompt"],
        "To confirm: book a table, Saturday, 2. Do you want me to do that?"
    );
    assert!(tool.calls().is_empty());

    // The caller corrects it while the read-back plays: the agent stops at the first word.
    rig.until("the read-back plays", 10_000, |rig| {
        rig.said()
            .contains(&"book a table, Saturday, 2.".to_owned())
    })
    .await;
    rig.run(400).await;
    rig.onset().await;
    rig.stt.interim("actually").await;
    rig.until_event("agent_audio_stopped", 1).await;
    let stopped = rig.events("agent_audio_stopped");
    assert_eq!(stopped.len(), 1, "{}", rig.timeline());
    assert_eq!(stopped[0]["reason"], "barge_in");
    rig.run(700).await;
    rig.offset(700).await;
    rig.stt.speech_final("actually make it four").await;

    // Revision 2: the first question is superseded and a new one reads back four.
    rig.until_event("task_question", 2).await;
    let second = rig.events("task_question")[1].clone();
    assert_eq!(
        second["prompt"],
        "To confirm: book a table, Saturday, 4. Do you want me to do that?"
    );
    assert_ne!(second["request_id"], question["request_id"]);
    let superseded = rig.events("task_answer_received")[0].clone();
    assert_eq!(superseded["request_id"], question["request_id"]);
    assert_eq!(superseded["outcome"], "superseded");
    assert!(rig.said().contains(&"Okay, I'll change that.".to_owned()));
    assert!(tool.calls().is_empty(), "the stale call must not go out");

    // "Okay" does not bind an irreversible action: the agent asks once more.
    rig.until_heard("Do you want me to do that?").await;
    rig.run(300).await;
    rig.turn("okay").await;
    rig.until_heard("Just to be sure: do you want me to book a table?")
        .await;
    assert!(tool.calls().is_empty(), "a weak yes released the call");

    // A clear yes after the read-back was heard does.
    rig.run(300).await;
    rig.turn("yes").await;
    rig.until_event("task_result", 1).await;
    rig.until_heard("Your table for four is booked.").await;

    let accepted = rig.events("task_answer_received")[1].clone();
    assert_eq!(accepted["request_id"], second["request_id"]);
    assert_eq!(
        (&accepted["outcome"], &accepted["by"]),
        (&json!("accept"), &json!("voice"))
    );

    // One call, with the corrected arguments, signed, under the operation of revision 2.
    let calls = tool.calls();
    assert_eq!(calls.len(), 1, "{}", rig.timeline());
    let task_id = question["task_id"].as_str().expect("task id");
    let operation = format!("op_{task_id}_2_1");
    assert_eq!(
        calls[0].body,
        json!({
            "tool": "book_table",
            "arguments": { "party": 4, "day": "Saturday" },
            "operation_id": operation,
        })
    );
    assert_eq!(calls[0].headers["idempotency-key"], operation);
    assert!(calls[0].signed_with(TOOL_SECRET), "{:?}", calls[0].headers);
    assert!(!calls[0].signed_with("another secret"));

    let result = &rig.events("task_result")[0];
    assert_eq!(result["state"], "completed");
    assert_eq!(result["revision"], 2);
    assert_eq!(
        result["effects"],
        json!(["booked a table for 4 on Saturday"])
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_cancel_before_any_commit_says_it_is_stopped() {
    let tool = ToolDouble::start(json!({ "effect": "booked a table" })).await;
    let rig = Rig::with_tools(&tool.url, &[("book_table", "book a table", true)]).await;
    // The model asks for the tool at once, before an acknowledgement is due.
    rig.llm
        .push(Round::tool(100, "book_table", json!({ "party": 2 })));
    rig.ready().await;
    rig.turn("can you book a table for two").await;
    rig.until_heard("Do you want me to do that?").await;
    assert!(
        !rig.wire().iter().any(|(_, event)| is_ack(event)),
        "an acknowledgement over the question\n{}",
        rig.timeline()
    );

    rig.run(300).await;
    rig.turn("cancel that").await;
    rig.until_event("task_cancelled", 1).await;
    rig.until_heard("It's stopped.").await;

    let cancelled = &rig.events("task_cancelled")[0];
    assert_eq!(cancelled["effects"], "none_committed");
    assert_eq!(cancelled["by"], "user");
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(rig.events("task_answer_received")[0]["outcome"], "cancel");
    assert!(rig.heard_in_full("Okay, I'll leave it."));
    assert!(tool.calls().is_empty(), "nothing was confirmed");
    assert!(rig.events("task_result").is_empty());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_cancel_after_a_commit_names_what_was_done() {
    let tool = ToolDouble::start(json!({ "effect": "held a table for two" })).await;
    let tools = [
        ("hold_table", "hold a table", false),
        ("charge_card", "charge your card", true),
    ];
    let rig = Rig::with_tools(&tool.url, &tools).await;
    // Two steps: the hold, then the charge.
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(600, "hold_table", json!({ "party": 2 })),
            1 => Round::tool(600, "charge_card", json!({ "amount": 20 })),
            _ => Round::text(50, 20, &["ANSWER:\n", "All done."]),
        })
    });
    rig.ready().await;
    rig.turn("can you book a table for two and pay the deposit")
        .await;

    // The client's screen answers the first question.
    rig.until_event("task_question", 1).await;
    let hold = rig.events("task_question")[0].clone();
    rig.send(Msg::TaskAnswer {
        request_id: hold["request_id"].as_str().expect("request id").to_owned(),
        outcome: HitlOutcome::Accept { value: None },
    })
    .await;
    rig.until_event("task_question", 2).await;
    let answered = &rig.events("task_answer_received")[0];
    assert_eq!(
        (&answered["outcome"], &answered["by"]),
        (&json!("accept"), &json!("client"))
    );
    let progress = rig.events("task_progress");
    let committed = json!(["held a table for two"]);
    assert!(progress.iter().any(|event| event["effects"] == committed));

    // The caller cancels at the second question: the hold is not hidden.
    rig.until_heard("Do you want me to do that?").await;
    rig.run(300).await;
    rig.turn("cancel that").await;
    rig.until_event("task_cancelled", 1).await;
    // No undo is offered: no tool declares one, and nothing would act on the answer.
    let truth = "I've stopped it, but before it stopped it had already held a table for two.";
    rig.until_heard(truth).await;

    let cancelled = &rig.events("task_cancelled")[0];
    assert_eq!(cancelled["effects"], committed);
    let calls = tool.calls();
    assert_eq!(calls.len(), 1, "only the hold went out");
    assert_eq!(calls[0].body["tool"], "hold_table");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_tool_that_fails_after_a_commit_ends_the_task_with_what_was_done() {
    let tool = ToolDouble::start(json!({ "effect": "held a table for two" })).await;
    let tools = [
        ("hold_table", "hold a table", false),
        ("charge_card", "charge your card", true),
    ];
    let rig = Rig::with_tools(&tool.url, &tools).await;
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(600, "hold_table", json!({ "party": 2 })),
            1 => Round::tool(600, "charge_card", json!({ "amount": 20 })),
            // A model that would tell the caller to try again: it is never asked.
            _ => Round::text(50, 20, &["ANSWER:\n", "Please try again later."]),
        })
    });
    rig.ready().await;
    rig.turn("can you book a table for two and pay the deposit")
        .await;
    let accept = |n: usize| {
        let question = rig.events("task_question")[n].clone();
        Msg::TaskAnswer {
            request_id: question["request_id"].as_str().expect("id").to_owned(),
            outcome: HitlOutcome::Accept { value: None },
        }
    };
    rig.until_event("task_question", 1).await;
    rig.send(accept(0)).await;
    // The hold went through. The payment endpoint then answers HTTP 500.
    rig.until_event("task_question", 2).await;
    tool.fail(true);
    rig.send(accept(1)).await;
    rig.until_event("task_result", 1).await;

    let effects = json!([
        "held a table for two",
        "tried to charge your card (unconfirmed)"
    ]);
    let result = &rig.events("task_result")[0];
    assert_eq!(
        (&result["state"], &result["effects"]),
        (&json!("failed"), &effects)
    );
    let truth = "It didn't finish, but it had already held a table for two and tried to charge \
                 your card (unconfirmed).";
    rig.until_heard(truth).await;
    assert_eq!(tool.calls().len(), 2, "the failed call is not repeated");
    let rounds = rig.llm.requests().len();
    assert_eq!(rounds, 2, "the model is not asked to word the outcome");
    let said = rig.lines();
    assert!(
        said.iter()
            .all(|line| line["delta"] != "Please try again later.")
    );
}

// ---- the caller speaks over the agent

/// A long first sentence, then a pause of `stall_ms` before the rest.
fn slow_answer(stall_ms: u64) -> Round {
    Round {
        tokens: vec![
            (600, LONG_SENTENCE.to_owned() + " "),
            (20, "The ".to_owned()),
            (stall_ms, "tracking number ends in four two.".to_owned()),
        ],
        ..Round::default()
    }
}

const LONG_SENTENCE: &str =
    "Your order shipped on Monday and it should arrive at your door by Thursday afternoon.";

#[tokio::test(start_paused = true)]
#[serial]
async fn a_barge_in_cuts_the_speech_and_the_task_still_completes() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(slow_answer(6_000));
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&LONG_SENTENCE.to_owned())
    })
    .await;
    let cut_line = rig.line(LONG_SENTENCE);
    rig.run(1_000).await;

    // The caller talks over it; no words come from the speech-to-text.
    let onset = rig.onset().await;
    rig.run(300).await;
    // Ducked by 15 dB within the ramp, well before anything is decided.
    let ducked = rig.frames_in(onset + 100, onset + 300);
    assert!(!ducked.is_empty(), "{}", rig.timeline());
    let quiet = f64::from(ix_mocks::TTS_AMPLITUDE) * 10f64.powf(-10.0 / 20.0);
    assert!(
        ducked.iter().all(|(_, peak)| f64::from(*peak) < quiet),
        "{ducked:?}"
    );
    assert!(rig.events("agent_audio_stopped").is_empty());

    // Still talking at 900 ms: an interruption. The agent stops.
    rig.run(700).await;
    let stopped = rig.events("agent_audio_stopped");
    assert_eq!(stopped.len(), 1, "{}", rig.timeline());
    assert_eq!(stopped[0]["reason"], "barge_in");
    assert_eq!(
        stopped[0]["utterance_ids"],
        json!([cut_line["utterance_id"]])
    );
    let cut = rig.time_of("the cut", |event| event["type"] == "agent_audio_stopped");
    assert!(
        (onset + 900..=onset + 920).contains(&cut),
        "onset {onset}, cut {cut}"
    );
    assert_eq!(rig.heard(&cut_line["utterance_id"]), "partial");
    assert!(
        rig.events("task_result").is_empty(),
        "the task is still running"
    );

    // The caller stops without having said anything the speech-to-text caught.
    rig.run(200).await;
    rig.offset(1_200).await;
    rig.run(500).await;
    rig.silence(700).await;
    rig.run(800).await;
    rig.silence(1_500).await;

    // The task went on: its last sentence is relayed and it completes.
    rig.until_event("task_result", 1).await;
    rig.until_heard("The tracking number ends in four two.")
        .await;
    assert_eq!(rig.events("task_result")[0]["state"], "completed");
    let resumed = rig.time_of("the rest", says("The tracking number ends in four two."));
    assert!(
        rig.frames_in(cut + 20, resumed - 1).is_empty(),
        "audio after the cut"
    );
    assert_eq!(rig.events("agent_audio_stopped").len(), 1);

    // The next run is told what the caller heard: of the cut sentence, only its start.
    rig.llm.push(answer(900));
    rig.run(500).await;
    rig.turn("and what time does the store open").await;
    rig.until_event("task_created", 2).await;
    let heard = ix_mocks::messages(&rig.llm.requests()[1], "assistant");
    assert_eq!(heard.len(), 3, "{heard:?}");
    assert_eq!(heard[0], rig.said()[0], "the acknowledgement");
    // The cut is marked (chapter 9's history).
    let cut = heard[1]
        .strip_suffix(" [cut off by the caller]")
        .expect("the cut marker");
    assert!(
        LONG_SENTENCE.starts_with(cut) && cut.len() < LONG_SENTENCE.len() / 2,
        "{heard:?}"
    );
    assert!(heard[1].starts_with("Your order"), "{heard:?}");
    assert_eq!(heard[2], "The tracking number ends in four two.");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn scratch_that_follows_the_cut_of_an_amended_answer() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.respond_with(|request| {
        let user = ix_mocks::messages(request, "user").join("\n");
        Some(if user.contains("tomorrow") {
            Round::text(
                200,
                20,
                &["Tomorrow it arrives by noon. ", "See ", "you then."],
            )
        } else {
            slow_answer(60_000)
        })
    });
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&LONG_SENTENCE.to_owned())
    })
    .await;
    rig.run(1_000).await;

    // "Actually" is enough to stop the agent; the rest of the turn amends the request.
    rig.onset().await;
    rig.stt.interim("actually").await;
    rig.until_event("agent_audio_stopped", 1).await;
    assert_eq!(
        rig.events("agent_audio_stopped").len(),
        1,
        "{}",
        rig.timeline()
    );
    rig.run(700).await;
    rig.offset(700).await;
    rig.stt
        .speech_final("actually I meant the one for tomorrow")
        .await;
    rig.until_heard("Tomorrow it arrives by noon.").await;
    rig.until_event("task_result", 1).await;

    // The first thing the agent says after the cut takes the half-heard answer back.
    let said = rig.said();
    let cut_at = said
        .iter()
        .position(|line| line == LONG_SENTENCE)
        .expect("the answer");
    assert!(withdrawals().contains(&said[cut_at + 1]), "{said:?}");
    assert!(withdrawals().contains("Scratch that."));
    let withdrawal = rig.line(&said[cut_at + 1]);
    assert_eq!(withdrawal["kind"], "notice");
    let cut = rig.time_of("the cut", |event| event["type"] == "agent_audio_stopped");
    let withdrawn = rig.time_of("the withdrawal", |event| event == &withdrawal);
    assert!(cut < withdrawn);
    // Then the change is acknowledged and the new answer relayed.
    let update_ack = rig.line(&said[cut_at + 2]);
    assert_eq!(update_ack["kind"], "ack");
    assert_eq!(
        said[cut_at + 3..],
        ["Tomorrow it arrives by noon.", "See you then."]
    );
    assert!(!said.iter().any(|line| line.contains("tracking number")));

    // One task, revised: the old answer is stopped and the new run has both turns.
    let progress = rig.events("task_progress");
    let stopped = json!({ "reason": "revised", "released_sentences": 1 });
    assert!(
        progress
            .iter()
            .any(|event| event["answer_stopped"] == stopped)
    );
    assert_eq!(rig.events("task_created").len(), 1);
    let result = &rig.events("task_result")[0];
    assert_eq!(
        (&result["state"], &result["revision"]),
        (&json!("completed"), &json!(2))
    );
    // The run of the revision (a provisional run started for the amending turn, before the
    // front named it an update, was discarded).
    let requests = rig.llm.requests();
    assert!(requests.len() >= 2, "{}", requests.len());
    assert_eq!(
        ix_mocks::messages(&requests[requests.len() - 1], "user"),
        ["where is my order\nCorrection: actually I meant the one for tomorrow"]
    );
}

// ---- provider failures

#[tokio::test(start_paused = true)]
#[serial]
async fn a_tts_error_is_said_once_and_a_second_one_only_reported() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    let sorry = "Sorry, I couldn't get that done just now.";

    // The acknowledgement fails: the caller is told, and the answer is still relayed.
    rig.llm.push(answer(2_500));
    rig.tts.fail_next(1);
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;
    let lines = rig.lines();
    assert!(is_ack(&lines[0]), "{}", rig.timeline());
    // Its words went to the client although no audio came of them.
    assert_eq!(rig.heard(&lines[0]["utterance_id"]), "none");
    assert_eq!(lines[1]["delta"], sorry);
    assert!(rig.heard_in_full(sorry));
    assert!(rig.events("error").is_empty(), "{}", rig.timeline());
    rig.run(1_000).await;

    // The acknowledgement and the apology both fail: no third try, the client gets an error.
    let before = rig.wire().len();
    rig.llm.push(answer(2_500));
    rig.tts.fail_next(2);
    rig.turn("and when does it close").await;
    rig.until_event("error", 1).await;
    rig.until_event("task_result", 2).await;
    rig.run(6_000).await;
    let errors = rig.events("error");
    assert_eq!(errors.len(), 1, "{}", rig.timeline());
    assert_eq!(errors[0]["code"], "interaction_tts_failed");
    let after: Vec<Value> = rig
        .wire()
        .split_off(before)
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    let lines: Vec<&Value> = after
        .iter()
        .filter(|e| e["delta"].is_string() && e["delta"] != "")
        .collect();
    assert!(
        is_ack(lines[0]) && lines[1]["delta"] == sorry,
        "{}",
        rig.timeline()
    );
    // The synthesis works again: the answer is spoken.
    assert_eq!(lines[2]["delta"], "The store opens at nine.");
    assert_eq!(rig.heard(&lines[2]["utterance_id"]), "full");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_refused_speak_fails_the_utterance_too() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    rig.llm.push(answer(2_500));
    // The provider refuses the speak itself: the speech worker reports it.
    rig.tts.refuse_next(1);
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;
    assert!(rig.heard_in_full("Sorry, I couldn't get that done just now."));
    assert!(rig.events("error").is_empty(), "{}", rig.timeline());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_stt_error_ends_the_turn_and_the_floor_listens_again() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    rig.onset().await;
    rig.run(400).await;
    let floor = rig.events("floor_state");
    assert_eq!(floor.last().expect("a floor state")["holder"], "user");

    rig.stt.fail("the socket closed").await;
    rig.run(1_000).await;
    let floor = rig.events("floor_state");
    let last = floor.last().expect("a floor state");
    assert_eq!(
        (&last["holder"], &last["reason"]),
        (&json!("none"), &json!("idle"))
    );
    assert!(rig.events("task_created").is_empty());
    assert!(rig.said().is_empty(), "{}", rig.timeline());
    assert!(rig.llm.requests().is_empty());

    // The next turn is taken as usual.
    rig.offset(400).await;
    rig.run(500).await;
    rig.llm.push(answer(900));
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;
}

#[tokio::test(start_paused = true)]
#[serial]
async fn request_response_stt_is_uploaded_at_the_end_of_speech() {
    let rig = Rig::start(Opts {
        request_response: true,
        ..Opts::default()
    })
    .await;
    rig.ready().await;

    // A cough: too little speech to upload. The buffer is dropped at the silence ceiling.
    rig.speech(100).await;
    rig.run(1_300).await;
    rig.silence(1_500).await;
    rig.run(100).await;
    assert_eq!((rig.stt.taken(), rig.stt.uploaded()), (1, 0));

    // Speech, then Smart Turn is sure: the upload starts. It fails, so the turn ends empty.
    rig.stt.script_upload(Err("the transcription timed out"));
    rig.speech(600).await;
    rig.smart_turn(0.9).await;
    rig.run(1_000).await;
    assert_eq!((rig.stt.taken(), rig.stt.uploaded()), (2, 1));
    assert!(rig.events("task_created").is_empty(), "{}", rig.timeline());
    let floor = rig.events("floor_state");
    assert_eq!(floor.last().expect("a floor state")["holder"], "none");

    // The next upload returns the words: its final is E.
    rig.stt.script_upload(Ok("what time does the store open"));
    rig.llm.push(answer(900));
    rig.speech(600).await;
    rig.smart_turn(0.9).await;
    rig.until_heard("It closes at six.").await;
    assert_eq!((rig.stt.taken(), rig.stt.uploaded()), (3, 2));
    assert_eq!(rig.events("task_created").len(), 1);
    assert_eq!(
        ix_mocks::messages(&rig.llm.requests()[0], "user"),
        ["what time does the store open"]
    );

    // The client's `audio_end` ends a turn through the same flush, so the speech-to-text
    // goes on hearing. With nothing buffered the turn ends at once, without words.
    rig.send(Msg::AudioEnd).await;
    rig.run(200).await;
    assert_eq!((rig.stt.taken(), rig.stt.uploaded()), (4, 2));
    assert_eq!(rig.events("task_created").len(), 1);
    // With speech buffered it is uploaded, and its final is a turn like any other.
    rig.stt.script_upload(Ok("what time does the store open"));
    rig.llm.push(answer(900));
    rig.send(Msg::AudioEnd).await;
    rig.until_event("task_created", 2).await;
    assert_eq!((rig.stt.taken(), rig.stt.uploaded()), (5, 3));
}

// ---- streaming speech-to-text: eager end of turn and the claim

#[tokio::test(start_paused = true)]
#[serial]
async fn an_eager_run_is_adopted_when_the_final_has_the_same_words() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(900));
    rig.ready().await;

    rig.onset().await;
    rig.run(400).await;
    rig.stt.interim("what time does the store open").await;
    rig.run(400).await;
    rig.offset(600).await;
    // Smart Turn is sure before the vendor's final: the run starts on the interim words.
    rig.smart_turn(0.9).await;
    rig.run(250).await;
    assert_eq!(rig.llm.requests().len(), 1, "the eager run did not start");
    assert!(
        rig.events("task_created").is_empty(),
        "a provisional run is not shown"
    );

    rig.stt.speech_final("What time does the store open?").await;
    rig.run(50).await;
    let created = rig.events("task_created");
    assert_eq!(created.len(), 1, "{}", rig.timeline());
    assert_eq!(created[0]["speculative"], true);
    rig.until_heard("It closes at six.").await;
    rig.until_event("task_result", 1).await;

    // Adopted, not restarted.
    assert_eq!(rig.llm.requests().len(), 1);
    assert_eq!(rig.said().len(), 3, "{}", rig.timeline());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_eager_run_is_discarded_when_the_final_has_other_words() {
    let rig = Rig::start(Opts::default()).await;
    // The eager run would say something else, late.
    rig.llm.push(Round::text(
        5_000,
        20,
        &["It opens at ten on weekdays. ", "Bye."],
    ));
    rig.llm.push(answer(900));
    rig.ready().await;

    rig.onset().await;
    rig.run(400).await;
    rig.stt.interim("what time does the store open").await;
    rig.run(400).await;
    rig.offset(600).await;
    rig.smart_turn(0.9).await;
    rig.run(250).await;
    assert_eq!(rig.llm.requests().len(), 1);

    rig.stt
        .speech_final("what time does the store open on sunday")
        .await;
    rig.until_heard("It closes at six.").await;
    rig.until_event("task_result", 1).await;
    rig.run(6_000).await;

    // A second run on the final's words; nothing of the first was ever shown or said.
    let requests = rig.llm.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        ix_mocks::messages(&requests[1], "user"),
        ["what time does the store open on sunday"]
    );
    assert_eq!(rig.events("task_created").len(), 1);
    assert_eq!(rig.events("task_result").len(), 1);
    assert!(!rig.said().iter().any(|line| line.contains("weekdays")));
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_claimed_segment_ends_the_turn_before_the_vendor_does() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(900));
    rig.ready().await;

    rig.onset().await;
    rig.run(600).await;
    // The vendor finalises the words and does not say that the speech has ended.
    rig.stt.final_piece("where is my order").await;
    rig.run(200).await;
    rig.offset(600).await;
    assert!(rig.events("task_created").is_empty());
    // 200 ms of silence and Smart Turn at 0.75: the gateway claims the segment itself.
    rig.smart_turn(0.75).await;
    let claimed = rig.now();
    rig.run(50).await;

    // E came with the claim, not with the VoiceManager's own timer 400 ms later.
    let created = rig.time_of("task_created", |event| event["type"] == "task_created");
    assert!(
        created <= claimed + 10,
        "claimed at {claimed}, E at {created}"
    );
    rig.until_heard("It closes at six.").await;
    assert_eq!(
        ix_mocks::messages(&rig.llm.requests()[0], "user"),
        ["where is my order"]
    );
    assert_eq!(rig.events("task_created").len(), 1);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn the_agents_own_line_coming_back_creates_no_task() {
    let rig = Rig::start(Opts::default()).await;
    let line = "The store opens at nine tomorrow morning and closes at six.";
    rig.llm.push(Round::text(900, 20, &[line]));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&line.to_owned())
    })
    .await;

    // The loudspeaker leaks into the microphone: an onset and the agent's words as interims.
    rig.run(1_500).await;
    rig.onset().await;
    rig.stt.interim("the store opens at nine tomorrow").await;
    rig.run(600).await;
    assert!(
        rig.events("agent_audio_stopped").is_empty(),
        "the echo cut the agent"
    );
    rig.until_heard(line).await;
    rig.offset(900).await;
    rig.stt
        .speech_final("the store opens at nine tomorrow morning and closes at six")
        .await;
    rig.run(3_000).await;

    assert_eq!(rig.events("task_created").len(), 1, "{}", rig.timeline());
    assert_eq!(rig.llm.requests().len(), 1);
    assert_eq!(rig.said().len(), 2, "the acknowledgement and the answer");
    let floor = rig.events("floor_state");
    assert_eq!(floor.last().expect("a floor state")["holder"], "none");
}

/// The analyser's echo defence kept the agent's own audio out of the onsets, and the
/// speech-to-text still hears it. Two words are too few for the transcript filter.
#[tokio::test(start_paused = true)]
#[serial]
async fn echo_the_analyser_heard_ducks_nothing_and_a_caller_over_it_still_cuts() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(slow_answer(6_000));
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&LONG_SENTENCE.to_owned())
    })
    .await;
    rig.run(1_000).await;

    let heard = rig.now();
    rig.duplex(DuplexEvent::Echo { at: heard }).await;
    rig.stt.interim("your order").await;
    rig.run(600).await;
    let frames = rig.frames_in(heard + 20, heard + 600);
    assert!(frames.len() >= 20, "{}", rig.timeline());
    assert!(
        frames
            .iter()
            .all(|(_, peak)| *peak == ix_mocks::TTS_AMPLITUDE),
        "the agent was ducked or paused for its own words: {frames:?}"
    );
    rig.stt.speech_final("your order").await;
    rig.run(300).await;
    assert_eq!(rig.events("task_created").len(), 1, "{}", rig.timeline());
    assert!(rig.events("agent_audio_stopped").is_empty());

    // The caller speaks over it: an onset the defence let through, and a transcript with
    // the agent's words and the caller's.
    rig.duplex(DuplexEvent::Echo { at: rig.now() }).await;
    let onset = rig.onset().await;
    rig.run(350).await;
    rig.stt
        .interim("shipped on Monday and it should stop")
        .await;
    rig.until_event("agent_audio_stopped", 1).await;
    let stopped = rig.events("agent_audio_stopped");
    assert_eq!(stopped.len(), 1, "{}", rig.timeline());
    assert_eq!(stopped[0]["reason"], "barge_in");
    let cut = rig.time_of("the cut", |event| event["type"] == "agent_audio_stopped");
    assert!(cut <= onset + 400, "onset {onset}, cut {cut}");
}

/// A LiveKit session: what the client gets on the socket, the room gets on its data topic.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_room_gets_every_event_on_its_data_topic() {
    let (queue, mut operations) = OperationQueue::new(256);
    let rig = Rig::start(Opts {
        room: Some(queue),
        ..Opts::default()
    })
    .await;
    let line = "The store opens at nine tomorrow morning.";
    rig.llm.push(Round::text(900, 20, &[line]));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_heard(line).await;
    rig.run(500).await;

    let mut published = Vec::new();
    while let Ok(queued) = operations.try_recv() {
        match queued.operation {
            LiveKitOperation::SendDataMessage { topic, data, .. } => {
                assert_eq!(topic, "waav.interaction");
                published.push(data);
            }
            other => panic!("only data messages go to the room: {other:?}"),
        }
    }
    let sent: Vec<Value> = rig.wire().into_iter().map(|(_, event)| event).collect();
    assert_eq!(published, sent, "{}", rig.timeline());
    for kind in [
        "task_created",
        "assistant_transcript",
        "floor_state",
        "task_result",
    ] {
        assert!(
            published.iter().any(|event| event["type"] == kind),
            "{kind}"
        );
    }
}

#[tokio::test(start_paused = true)]
#[serial]
async fn typed_words_that_repeat_the_agents_line_are_still_a_request() {
    let rig = Rig::start(Opts::default()).await;
    let line = "The store opens at nine tomorrow morning and closes at six.";
    rig.llm.push(Round::text(900, 20, &[line]));
    rig.llm.push(Round::text(900, 20, &["Yes, every day."]));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_heard(line).await;

    // No audio came with it: it cannot be the loudspeaker leaking into the microphone.
    rig.run(500).await;
    let typed = "does the store open at nine tomorrow morning and close at six";
    rig.send(Msg::TypedTurn(typed.to_owned())).await;
    rig.until_event("task_created", 2).await;
    rig.until_heard("Yes, every day.").await;
}

// ---- the model front

fn model_front(url: &str) -> Opts {
    Opts {
        interaction: Some(json!({ "front": { "profile": "model" } })),
        front: Some(url.to_owned()),
        ..Opts::default()
    }
}

#[tokio::test(start_paused = true)]
#[serial]
async fn the_model_front_decides_and_words_the_acknowledgement() {
    let front = FrontDouble::start(FrontScript {
        reply: r#"delegate","say":"Let me check that."}"#.to_owned(),
        act_ms: 80,
        line_ms: 60,
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(model_front(&front.url)).await;
    rig.llm.push(answer(900));
    rig.ready().await;
    assert_eq!(front.requests(), 1, "the warm-up request");
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;

    let decisions = front.decisions();
    assert_eq!(decisions.len(), 1);
    assert!(decisions[0]["grammar"].is_string());
    let observation = decisions[0]["messages"][3]["content"]
        .as_str()
        .expect("user turn");
    assert!(
        observation.contains("what time does the store open"),
        "{observation}"
    );

    // The acknowledgement is the model's line, and says which act it belongs to.
    let ack = rig.lines()[0].clone();
    assert_eq!(ack["delta"], "Let me check that.");
    assert_eq!(ack["kind"], "ack");
    assert_eq!(
        (&ack["source"], &ack["act"]),
        (&json!("front"), &json!("delegate"))
    );
    assert_eq!(rig.events("task_created").len(), 1);
    assert!(rig.events("interaction_updated").is_empty());
}

/// Live D1: "Are you still there?" a few seconds after the acknowledgement, while the lookup
/// runs, was read as a `wait`. The front is shown how long the agent has said nothing.
#[tokio::test(start_paused = true)]
#[serial]
async fn the_front_is_shown_how_long_the_agent_has_said_nothing_while_a_task_runs() {
    let front = FrontDouble::start(FrontScript {
        reply: r#"delegate","say":"Let me check that."}"#.to_owned(),
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(model_front(&front.url)).await;
    rig.llm.push(answer(20_000));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_heard("Let me check that.").await;
    rig.run(3_000).await;
    rig.turn("are you still there").await;
    rig.run(500).await;
    let decisions = front.decisions();
    assert_eq!(decisions.len(), 2, "{}", rig.timeline());
    let shown = |decision: &Value| {
        decision["messages"][3]["content"]
            .as_str()
            .map(str::to_owned)
    };
    let (first, second) = (shown(&decisions[0]).unwrap(), shown(&decisions[1]).unwrap());
    assert!(!first.contains("nothing said"), "{first}");
    assert!(
        second.contains("working, just started, nothing said about it for a few seconds"),
        "{second}"
    );
}

/// Live MA5 (r6-verify, 3 of 3): the caller went on over the acknowledgement with the rest of
/// the request ("And my budget is ...") and the 2B, shown the silence, read it as a new
/// request. After a line the caller cut, the agent's silence is not shown.
#[tokio::test(start_paused = true)]
#[serial]
async fn the_silence_after_a_line_the_caller_cut_is_not_shown() {
    let front = FrontDouble::start(FrontScript {
        reply: r#"delegate","say":"Let me check that."}"#.to_owned(),
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(model_front(&front.url)).await;
    // The first sentence is relayed while the run goes on (its rest 8 s later).
    rig.llm.push(slow_answer(8_000));
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&LONG_SENTENCE.to_owned())
    })
    .await;
    rig.run(1_000).await;
    // The caller talks over it until it is cut, then the recogniser's final comes late.
    rig.speech(1_200).await;
    rig.run(2_000).await;
    rig.smart_turn(0.9).await;
    rig.stt.speech_final("and the one from last week").await;
    rig.run(500).await;
    let stopped = rig.events("agent_audio_stopped");
    assert_eq!(stopped.len(), 1, "{}", rig.timeline());
    assert_eq!(stopped[0]["reason"], "barge_in");
    let decisions = front.decisions();
    assert_eq!(decisions.len(), 2, "{}", rig.timeline());
    let second = decisions[1]["messages"][3]["content"].as_str().unwrap();
    assert!(second.contains("EVENT: task t1"), "{second}");
    assert!(second.contains(" working, "), "{second}");
    assert!(!second.contains("nothing said"), "{second}");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn the_interims_slot_is_warmed_only_while_the_caller_is_quiet() {
    // Two slots, warmed one after the other in 2.5 s each (the loaded GPU of live MA3). Read
    // beside the first turn's decision, the second warm-up took it past its budget: begun
    // while the caller speaks, it is left to the next agent line's priming.
    let script = || FrontScript {
        reply: r#"delegate","say":"Let me check that."}"#.to_owned(),
        slots: 2,
        warm_ms: 2_500,
        ..FrontScript::default()
    };
    let front = FrontDouble::start(script()).await;
    let rig = Rig::start(model_front(&front.url)).await;
    rig.llm.push(answer(900));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;
    rig.run(5_000).await;
    assert_eq!(front.warmed(), 1, "only the decisions' slot was warmed");
    assert_eq!(rig.events("task_created").len(), 1);
    // A caller who is quiet when the first slot is warm: the second is warmed too.
    let quiet = FrontDouble::start(script()).await;
    let rig = Rig::start(model_front(&quiet.url)).await;
    rig.ready().await;
    rig.run(4_000).await;
    assert_eq!(quiet.warmed(), 2, "both slots");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_late_model_front_is_not_replaced_by_a_guess() {
    // The act comes after 2 s; the front's budget is 1.5 s.
    let front = FrontDouble::start(FrontScript {
        reply: r#"wait","why":"incomplete","say":""}"#.to_owned(),
        act_ms: 2_000,
        line_ms: 0,
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(model_front(&front.url)).await;
    rig.llm.respond_with(|_| Some(answer(900)));
    rig.ready().await;

    // No rule decides in its place (chapter 9): the caller is asked to say it again.
    rig.turn("what time does the store open").await;
    rig.until_heard("Sorry, I didn't catch that. Could you say it again?")
        .await;
    assert!(rig.events("task_created").is_empty(), "{}", rig.timeline());
    assert!(
        rig.events("interaction_updated").is_empty(),
        "one miss is not a failure"
    );
    // Three misses in a row: the front is unavailable, and said so once.
    for _ in 2..=3 {
        rig.run(3_000).await;
        rig.turn("what time does the store open").await;
        rig.run(2_000).await;
    }
    let updated = rig.events("interaction_updated");
    assert_eq!(updated.len(), 1, "{}", rig.timeline());
    assert_eq!(updated[0]["degraded"], json!(["front_unavailable"]));
    assert!(rig.events("task_created").is_empty());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn without_the_front_interaction_mode_decides_nothing_and_ends() {
    let front = FrontDouble::refusing().await;
    let rig = Rig::start(model_front(&front.url)).await;
    rig.llm.push(answer(900));
    rig.ready().await;

    // The warm-up could not connect.
    let updated = rig.events("interaction_updated");
    assert_eq!(updated.len(), 1, "{}", rig.timeline());
    assert_eq!(updated[0]["degraded"], json!(["front_unavailable"]));
    assert_eq!(updated[0]["applied"], json!([]));

    // No new decisions while it is away.
    rig.turn("what time does the store open").await;
    rig.run(3_000).await;
    assert!(rig.events("task_created").is_empty(), "{}", rig.timeline());
    // Not back within the supervisor's restart window: interaction mode ends.
    rig.run(160_000).await;
    let warnings = rig.events("config_warning");
    assert!(
        warnings.iter().any(|w| w["code"] == "interaction_ended"),
        "{}",
        rig.timeline()
    );
}

// ---- the client's own messages, and the end of the session

#[tokio::test(start_paused = true)]
#[serial]
async fn client_messages_reach_the_machines() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.respond_with(|_| Some(slow_answer(60_000)));
    rig.ready().await;

    // A typed turn is a turn.
    rig.send(Msg::TypedTurn("where is my order".to_owned()))
        .await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&LONG_SENTENCE.to_owned())
    })
    .await;
    rig.run(1_000).await;

    // `clear` stops the speech, not the task.
    rig.send(Msg::ClientClear).await;
    rig.run(50).await;
    let stopped = rig.events("agent_audio_stopped");
    assert_eq!(stopped.len(), 1, "{}", rig.timeline());
    assert_eq!(stopped[0]["reason"], "client_clear");
    let cut = rig.now();
    rig.run(500).await;
    assert!(
        rig.frames_in(cut, cut + 500).is_empty(),
        "audio after the clear"
    );
    assert!(rig.events("task_cancelled").is_empty());

    // A blank `speak` is not speech: it holds nothing up and nobody apologises for it.
    rig.send(Msg::ClientSpeak("  ".to_owned())).await;
    // `speak` is said as it is.
    rig.send(Msg::ClientSpeak("Please stay on the line.".to_owned()))
        .await;
    rig.until_heard("Please stay on the line.").await;
    let apology = |line: &String| line.starts_with("Sorry");
    assert!(!rig.said().iter().any(apology), "{}", rig.timeline());
    assert_eq!(rig.line("Please stay on the line.")["kind"], "notice");

    // Answers and cancels that name nothing get an error with a code.
    rig.send(Msg::TaskCancel {
        task_id: "task_nobody".to_owned(),
    })
    .await;
    rig.send(Msg::TaskAnswer {
        request_id: "q_nobody_1".to_owned(),
        outcome: HitlOutcome::Accept { value: None },
    })
    .await;
    rig.run(50).await;
    let codes: Vec<Value> = rig
        .events("error")
        .iter()
        .map(|e| e["code"].clone())
        .collect();
    assert_eq!(codes, ["task_unknown", "unknown_request_id"]);

    // A settings update: policy values apply, the task backend may not change.
    rig.send(Msg::Update(json!({
        "policy": { "ack_due_ms": 100 },
        "tasks": { "speculative_start": "off" },
    })))
    .await;
    rig.run(50).await;
    let updated = &rig.events("interaction_updated")[0];
    assert_eq!(updated["applied"], json!(["policy.ack_due_ms"]));
    assert_eq!(
        updated["refused"],
        json!([{ "path": "tasks.speculative_start", "reason": "not_mid_session" }])
    );

    // The client cancels the task by its id.
    let task_id = rig.events("task_created")[0]["task_id"].clone();
    rig.send(Msg::TaskCancel {
        task_id: task_id.as_str().expect("task id").to_owned(),
    })
    .await;
    rig.until_event("task_cancelled", 1).await;
    assert_eq!(rig.events("task_cancelled")[0]["by"], "client");
    rig.until_heard("It's stopped.").await;

    // The update holds: the next acknowledgement is due at E + 100 ms.
    rig.run(1_000).await;
    rig.send(Msg::TypedTurn("where is my other order".to_owned()))
        .await;
    let e = rig.now();
    rig.until_event("task_created", 2).await;
    rig.until("the acknowledgement", 2_000, |rig| {
        rig.lines().iter().filter(|line| is_ack(line)).count() == 2
    })
    .await;
    let second_ack = rig
        .wire()
        .into_iter()
        .filter(|(_, event)| is_ack(event))
        .nth(1);
    let (acknowledged, _) = second_ack.expect("the second acknowledgement");
    assert!(
        (e + 100..=e + 200).contains(&acknowledged),
        "E {e}, {acknowledged}"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn shutdown_stops_speech_and_cancels_the_tasks() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(slow_answer(60_000));
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&LONG_SENTENCE.to_owned())
    })
    .await;
    rig.run(500).await;

    rig.handle.shutdown().await;
    let ended = rig.now();
    tokio::time::timeout(Duration::from_secs(5), rig.actor)
        .await
        .expect("the actor ends")
        .expect("without a panic");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let wire = rig.wire.lock().clone();
    let of = |kind: &str| {
        wire.iter()
            .find(|(_, event)| event["type"] == kind)
            .cloned()
    };
    let (_, stopped) = of("agent_audio_stopped").expect("the speech was stopped");
    assert_eq!(stopped["reason"], "policy");
    let (_, cancelled) = of("task_cancelled").expect("the task was cancelled");
    assert_eq!(
        (&cancelled["by"], &cancelled["effects"]),
        (&json!("system"), &json!("none_committed"))
    );
    let late = |(at, _): &&(Ms, i16)| *at > ended;
    assert_eq!(
        rig.frames.lock().iter().filter(late).count(),
        0,
        "audio after the end"
    );
    // The store keeps the task for `GET /v1/tasks`, as it ended.
    let stored = tasks::store().list(&rig.owner, None);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].state, TaskState::Cancelled);
    assert!(!rig.handle.send(Msg::Ready).await, "the inbox is closed");
}

// ---- tasks that outlive the connection

const HELD: &str = "Your table for two is held.";

/// The operator's block of a session whose tool takes three seconds, with a push address.
fn holding(tool: &ToolDouble, hook: &ToolDouble, grace_s: u32) -> Value {
    json!({ "tasks": {
        "tools": [{
            "name": "hold_table", "description": "Use it to hold a table.", "url": tool.url,
            "parameters": { "type": "object", "properties": {} },
            "side_effects": true, "confirm_phrase": "hold a table",
        }],
        "push_url": hook.url,
        "reconnect_grace_s": grace_s,
    } })
}

/// A session whose confirmed tool call is on the wire, and the task's id. The tool answers
/// after three seconds; the background then words its answer.
async fn call_on_the_wire(tool: &ToolDouble, hook: &ToolDouble, grace_s: u32) -> (Rig, String) {
    tool.delay(3_000);
    let rig = Rig::start(Opts {
        operator: Some(holding(tool, hook, grace_s)),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(600, "hold_table", json!({ "party": 2 })),
            _ => Round::text(50, 20, &["ANSWER:\n", HELD]),
        })
    });
    rig.ready().await;
    rig.turn("can you hold a table for two").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    rig.send(Msg::TaskAnswer {
        request_id: question["request_id"].as_str().expect("an id").to_owned(),
        outcome: HitlOutcome::Accept { value: None },
    })
    .await;
    rig.until("the call is on the wire", 5_000, |_| {
        tool.calls().len() == 1
    })
    .await;
    let task_id = question["task_id"].as_str().expect("a task id").to_owned();
    (rig, task_id)
}

/// Waits, on the paused clock, until the push address has been called `count` times.
async fn pushed(hook: &ToolDouble, count: usize, limit_ms: u64) -> Vec<ix_mocks::ToolCall> {
    let arrived = async {
        while hook.calls().len() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let waited = tokio::time::timeout(Duration::from_millis(limit_ms), arrived).await;
    assert!(waited.is_ok(), "no push #{count} within {limit_ms} ms");
    hook.calls()
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_call_on_the_wire_at_a_clean_close_finishes_and_its_result_is_pushed() {
    let tool = ToolDouble::start(json!({ "effect": "held a table for two" })).await;
    let hook = ToolDouble::start(json!({})).await;
    let (rig, task_id) = call_on_the_wire(&tool, &hook, 60).await;

    // The client closes: nobody comes back, so the result goes out as soon as there is one.
    rig.handle.shutdown().await;
    assert!(hook.calls().is_empty(), "the call has not reported yet");
    let stored = tasks::store()
        .get(&rig.owner, &task_id)
        .await
        .expect("stored");
    assert_eq!(
        (stored.view.state, stored.result),
        (TaskState::Working, None)
    );

    let calls = pushed(&hook, 1, 10_000).await;
    let push = &calls[0];
    assert!(push.signed_with(TOOL_SECRET), "signed like a tool call");
    assert_eq!(push.headers.get("idempotency-key"), Some(&task_id));
    assert_eq!(
        push.body,
        json!({
            "type": "task_result",
            "task_id": task_id,
            "task_ref": "t1",
            "revision": 1,
            "state": "completed",
            "status": "completed",
            "cancelling": false,
            "summary": HELD,
            "effects": ["held a table for two"],
            "spoken": "none",
            "streamed": true,
            "superseded": false,
            "stream_id": rig.stream_id,
            // A caller's request on the session's own tool: top level, no catalog agent.
            "parent_id": null,
            "agent": null,
            "trace_id": push.body["trace_id"],
            // The short spoken update next to the full result: the narrator's line.
            "update": "Your table for two is held.",
        })
    );
    let trace = push.body["trace_id"].as_str().unwrap_or_default();
    assert!(
        trace.len() == 32 && trace.bytes().all(|b| b.is_ascii_hexdigit()),
        "a W3C trace id: {trace}"
    );
    // The result is kept with the task, and the run did end.
    let stored = tasks::store()
        .get(&rig.owner, &task_id)
        .await
        .expect("stored");
    assert_eq!(stored.view.state, TaskState::Completed);
    let result = stored.result.expect("a result");
    assert_eq!(
        (result.state, result.summary.as_str()),
        (TaskState::Completed, HELD)
    );
    tokio::time::timeout(Duration::from_secs(5), rig.actor)
        .await
        .expect("the actor ends with its last task")
        .expect("without a panic");
    // A clean close leaves nothing to come back for.
    assert_eq!(tasks::store().claim(&rig.owner, &rig.stream_id).await, None);
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert_eq!(hook.calls().len(), 1, "pushed once");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_caller_who_is_back_within_the_grace_hears_the_result_and_nothing_is_pushed() {
    let tool = ToolDouble::start(json!({ "effect": "held a table for two" })).await;
    let hook = ToolDouble::start(json!({})).await;
    let (rig, task_id) = call_on_the_wire(&tool, &hook, 60).await;

    // The connection is lost while the call is out, and the caller is back on a new one
    // before the call has even reported.
    rig.handle.dropped().await;
    let back = Rig::start(Opts {
        operator: Some(holding(&tool, &hook, 60)),
        back: Some((rig.owner.clone(), rig.stream_id.clone())),
        ..Opts::default()
    })
    .await;
    back.ready().await;
    back.until_event("task_result", 1).await;
    back.until_heard(HELD).await;

    let result = &back.events("task_result")[0];
    assert_eq!(result["task_id"], task_id.as_str(), "{result}");
    assert_eq!(
        (&result["state"], &result["summary"], &result["effects"]),
        (
            &json!("completed"),
            &json!(HELD),
            &json!(["held a table for two"])
        ),
        "{result}"
    );
    // The answer is the background's own words, said as such.
    let line = back.line(HELD);
    assert_eq!(
        (&line["source"], &line["verbatim"], &line["task_id"]),
        (&json!("background"), &json!(true), &json!(task_id)),
        "{line}"
    );
    // The new connection lists the task with its own.
    let listed = tasks::store().list(&rig.owner, Some(&rig.stream_id));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, TaskState::Completed);

    // The caller has it: nothing is pushed, at the end of the grace or after.
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert!(hook.calls().is_empty(), "{:?}", hook.calls());
    assert_eq!(back.events("task_result").len(), 1, "told once");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_result_nobody_came_back_for_is_pushed_when_the_grace_is_over() {
    let tool = ToolDouble::start(json!({ "effect": "held a table for two" })).await;
    let hook = ToolDouble::start(json!({})).await;
    let (rig, task_id) = call_on_the_wire(&tool, &hook, 20).await;

    rig.handle.dropped().await;
    let dropped_at = tokio::time::Instant::now();
    // The task ends within seconds; the caller still has the rest of the grace.
    tokio::time::sleep(Duration::from_secs(15)).await;
    let stored = tasks::store()
        .get(&rig.owner, &task_id)
        .await
        .expect("stored");
    assert_eq!(stored.view.state, TaskState::Completed);
    assert!(hook.calls().is_empty(), "the grace is not over");

    let calls = pushed(&hook, 1, 10_000).await;
    let waited = dropped_at.elapsed();
    assert!(waited >= Duration::from_secs(20), "pushed after {waited:?}");
    assert!(calls[0].signed_with(TOOL_SECRET));
    assert_eq!(calls[0].body["task_id"], task_id.as_str());
    assert_eq!(calls[0].body["summary"], HELD);
    // Who comes after that gets nothing a second time.
    assert_eq!(tasks::store().claim(&rig.owner, &rig.stream_id).await, None);
}

/// A session whose lookup (no tool call) answers eight seconds after the caller's request,
/// with the operator's push address when `hook` is given.
async fn lookup_running(hook: Option<&ToolDouble>, grace_s: u32) -> (Rig, String) {
    let mut tasks = json!({ "reconnect_grace_s": grace_s });
    if let Some(hook) = hook {
        tasks["push_url"] = json!(hook.url);
    }
    let rig = Rig::start(Opts {
        operator: Some(json!({ "tasks": tasks })),
        ..Opts::default()
    })
    .await;
    rig.llm.push(answer(8_000));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_event("task_created", 1).await;
    let task_id = rig.events("task_created")[0]["task_id"]
        .as_str()
        .expect("a task id")
        .to_owned();
    (rig, task_id)
}

const OPENS: &str = "The store opens at nine. It closes at six.";

#[tokio::test(start_paused = true)]
#[serial]
async fn a_lookup_still_running_at_the_hang_up_finishes_and_its_answer_is_pushed() {
    let hook = ToolDouble::start(json!({})).await;
    let (rig, task_id) = lookup_running(Some(&hook), 60).await;
    rig.handle.shutdown().await;
    let calls = pushed(&hook, 1, 30_000).await;
    assert!(calls[0].signed_with(TOOL_SECRET));
    assert_eq!(
        (
            &calls[0].body["task_id"],
            &calls[0].body["state"],
            &calls[0].body["summary"],
            &calls[0].body["effects"],
        ),
        (
            &json!(task_id),
            &json!("completed"),
            &json!(OPENS),
            &json!("none_committed"),
        ),
        "{}",
        calls[0].body
    );
    tokio::time::timeout(Duration::from_secs(5), rig.actor)
        .await
        .expect("the actor ends with its last task")
        .expect("without a panic");

    // With nobody to read it (no push address, no grace), a lookup stops with its session.
    let (rig, task_id) = lookup_running(None, 0).await;
    rig.handle.shutdown().await;
    let stored = tasks::store()
        .get(&rig.owner, &task_id)
        .await
        .expect("kept");
    assert_eq!(stored.view.state, TaskState::Cancelled);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_lookup_that_ends_after_a_lost_connection_reaches_the_caller_who_is_back() {
    let (rig, task_id) = lookup_running(None, 60).await;
    rig.handle.dropped().await;
    let back = Rig::start(Opts {
        back: Some((rig.owner.clone(), rig.stream_id.clone())),
        ..Opts::default()
    })
    .await;
    back.ready().await;
    back.until_event("task_result", 1).await;
    back.until_heard(OPENS).await;
    let result = &back.events("task_result")[0];
    assert_eq!(
        (&result["task_id"], &result["state"], &result["summary"]),
        (&json!(task_id), &json!("completed"), &json!(OPENS)),
        "{result}"
    );
}

// ---- unclear turns, another language, repeated requests, sensitive values, the speech policy

/// A final result as a recogniser that reports its confidence and the language sends it.
fn heard(text: &str, confidence: f32, language: Option<&str>) -> Msg {
    let mut result = STTResult::new(text.to_owned(), true, true, confidence);
    result.vendor_confidence = Some(confidence);
    result.detected_language = language.map(str::to_owned);
    Msg::Stt(Box::new(result))
}

#[tokio::test(start_paused = true)]
#[serial]
async fn words_the_recogniser_is_unsure_of_are_asked_again() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(900));
    rig.ready().await;
    rig.speech(600).await;
    rig.send(heard("I need a flight to the twenty", 0.31, None))
        .await;
    rig.run(3_000).await;

    let lines = rig.lines();
    assert_eq!(lines.len(), 1, "{}", rig.timeline());
    assert_eq!(lines[0]["kind"], "question");
    assert!(rig.events("task_created").is_empty());
    assert!(
        rig.llm.requests().is_empty(),
        "no run, not even a provisional one"
    );

    // The same request from a recogniser that is sure of it is a task.
    rig.speech(600).await;
    rig.send(heard(
        "I need a flight to Denver on the twentieth",
        0.93,
        None,
    ))
    .await;
    rig.until_event("task_result", 1).await;
    assert_eq!(rig.events("task_created").len(), 1);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_caller_who_keeps_speaking_another_language_is_stepped_down() {
    const SPANISH: &str = "quiero una cita para el lunes";
    let rig = Rig::start(Opts::default()).await;
    rig.llm
        .push(Round::text(300, 20, &["Claro. ", "Para ", "el lunes."]));
    rig.ready().await;
    for _ in 0..2 {
        rig.speech(600).await;
        rig.send(heard(SPANISH, 0.9, Some("es"))).await;
        rig.run(4_000).await;
    }
    assert_eq!(
        rig.said(),
        [
            "Sorry, I didn't catch that. Could you say it again?",
            "Sorry, I can only help in English on this line."
        ],
        "{}",
        rig.timeline()
    );
    let warnings = rig.events("config_warning");
    assert_eq!(warnings.len(), 1, "{}", rig.timeline());
    assert_eq!(warnings[0]["code"], "interaction_language_step_down");
    assert!(rig.events("task_created").is_empty());
    assert!(rig.llm.requests().is_empty());

    // The plain path: the turn goes to the background as it is, with none of the call so far,
    // and nothing but its answer is spoken.
    rig.speech(600).await;
    rig.send(heard(SPANISH, 0.9, Some("es"))).await;
    rig.until_event("task_result", 1).await;
    rig.until_heard("Para el lunes.").await;
    let requests = rig.llm.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(ix_mocks::messages(&requests[0], "user"), [SPANISH]);
    assert!(ix_mocks::messages(&requests[0], "assistant").is_empty());
    assert_eq!(rig.said()[2..], ["Claro.", "Para el lunes."]);
}

/// The same with no language field from the recogniser: the front reads the words as another
/// language (`wait` `foreign`; owner, 24 Oct: no word list does), so the caller is asked again
/// and then stepped down; a turn it reads otherwise ends the run.
#[tokio::test(start_paused = true)]
#[serial]
async fn the_fronts_reading_of_another_language_asks_again_then_steps_down() {
    const SPANISH: &str = "quiero una cita para el lunes";
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(300));
    rig.ready().await;
    // A turn the front reads otherwise between two in another language: no step-down yet.
    for text in [SPANISH, "mm-hm", SPANISH] {
        rig.speech(600).await;
        rig.send(heard(text, 0.9, None)).await;
        rig.run(4_000).await;
    }
    let again = "Sorry, I didn't catch that. Could you say it again?";
    assert_eq!(rig.said(), [again, again], "{}", rig.timeline());
    assert!(
        rig.events("config_warning").is_empty(),
        "{}",
        rig.timeline()
    );
    rig.speech(600).await;
    rig.send(heard(SPANISH, 0.9, None)).await;
    rig.run(4_000).await;
    assert_eq!(
        rig.said()[2..],
        ["Sorry, I can only help in English on this line."],
        "{}",
        rig.timeline()
    );
    let warnings = rig.events("config_warning");
    assert_eq!(warnings.len(), 1, "{}", rig.timeline());
    assert_eq!(warnings[0]["code"], "interaction_language_step_down");
    // A speculative run may have started on the words before the front's reading (as for any
    // turn the front decides); it is discarded unseen: no task.
    assert!(rig.events("task_created").is_empty());
}

/// A newer turn stops the language reading of the turn it supersedes with its decision: left
/// running it held a slot of the server beside the newer turn's decision (live G1, r14: the
/// superseded turn's reading ran 0.4-1.1 s on). The newer turn is read and answered.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_newer_turn_stops_the_language_reading_of_the_turn_it_supersedes() {
    let front = FrontDouble::start(FrontScript {
        decide: Some(ix_mocks::reads_the_turn),
        // Within the front's budget (1.5 s): the newer turn comes while it is out.
        language_ms: 1_000,
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(Opts {
        front: Some(front.url.clone()),
        ..Opts::default()
    })
    .await;
    // Speculative runs start on both turns' words before their readings.
    rig.llm.respond_with(|_| Some(answer(300)));
    rig.ready().await;
    rig.send(Msg::TypedTurn("what time does the store open".to_owned()))
        .await;
    rig.run(500).await;
    assert_eq!(front.languages_read(), (1, 0), "{}", rig.timeline());
    rig.send(Msg::TypedTurn("what time does the store close".to_owned()))
        .await;
    rig.until_heard("It closes at six.").await;
    rig.run(3_000).await;
    assert_eq!(front.languages_read(), (2, 1), "{}", rig.timeline());
    assert_eq!(rig.events("task_created").len(), 1, "{}", rig.timeline());
}

/// The front reads the language only of words the recogniser named none for: one that names
/// it read it already (its model's reading), and the reading held every acknowledgement 0.6 s
/// (live L1, r14-verify).
#[tokio::test(start_paused = true)]
#[serial]
async fn the_front_reads_the_language_only_of_words_the_recogniser_named_none_for() {
    let front = FrontDouble::start(FrontScript {
        decide: Some(ix_mocks::reads_the_turn),
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(Opts {
        front: Some(front.url.clone()),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|_| Some(answer(300)));
    rig.ready().await;
    rig.speech(600).await;
    rig.send(heard("what time does the store open", 0.9, Some("en")))
        .await;
    rig.until_heard("It closes at six.").await;
    assert_eq!(front.languages_read(), (0, 0), "{}", rig.timeline());
    rig.run(1_000).await;
    rig.send(Msg::TypedTurn("what time does the store close".to_owned()))
        .await;
    rig.run(3_000).await;
    assert_eq!(front.languages_read(), (1, 1), "{}", rig.timeline());
    assert_eq!(rig.events("task_created").len(), 2, "{}", rig.timeline());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_request_that_nearly_repeats_a_running_task_is_asked_about() {
    const FOR_FOUR: &str = "book a table for four at Rosa's on Saturday";
    // A session whose tool can book: only there is a repeated request asked about.
    let tool = ToolDouble::start(json!({ "status": "booked" })).await;
    let rig = Rig::with_tools(&tool.url, &[("book_table", "book a table", false)]).await;
    // By the request, not by order: a run started before the turn was settled asks too.
    rig.llm.respond_with(|request| {
        let asked = ix_mocks::messages(request, "user");
        let second = asked.last().is_some_and(|last| last.contains("for four"));
        Some(if second {
            Round::text(300, 20, &["A second table is booked."])
        } else {
            Round::text(60_000, 20, &["It is booked."])
        })
    });
    rig.ready().await;
    rig.turn("book a table for two at Rosa's on Saturday").await;
    rig.run(3_000).await;
    rig.turn(FOR_FOUR).await;
    let which = "Do you want me to change it, or is this a second one?";
    rig.until_heard(which).await;
    assert_eq!(rig.line(which)["kind"], "question");
    assert_eq!(rig.events("task_created").len(), 1, "{}", rig.timeline());

    rig.run(500).await;
    rig.turn("a second one please").await;
    rig.until_event("task_created", 2).await;
    assert_eq!(rig.events("task_created")[1]["title"], FOR_FOUR);
    rig.until_heard("A second table is booked.").await;
    let requests = rig.llm.requests();
    let asked = ix_mocks::messages(requests.last().unwrap(), "user");
    assert_eq!(asked.last().map(String::as_str), Some(FOR_FOUR));
    // It runs from where the request was said: the question and the answer that settled it
    // are not in its history, and the request is said once (live D2: the 3B background read
    // the request after the question as asked again and booked nothing).
    assert_eq!(
        asked.iter().filter(|said| *said == FOR_FOUR).count(),
        1,
        "{asked:?}"
    );
    assert!(
        !asked.iter().any(|said| said.contains("second one")),
        "{asked:?}"
    );
    let heard = ix_mocks::messages(requests.last().unwrap(), "assistant");
    assert!(!heard.iter().any(|line| line == which), "{heard:?}");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_card_number_is_declined_and_never_reaches_the_background() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(answer(300));
    rig.ready().await;
    rig.turn("my card number is 4111 1111 1111 1111").await;
    rig.until_heard("I can't take your card number on this call for security reasons.")
        .await;
    assert!(rig.events("task_created").is_empty());

    rig.run(1_000).await;
    rig.turn("what time does the store open").await;
    rig.until_event("task_result", 1).await;
    let sent = serde_json::to_string(&rig.llm.requests()).unwrap();
    assert!(!sent.contains("4111"), "{sent}");
    assert!(sent.contains("what time does the store open"));
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_cut_holding_line_stays_out_of_the_next_tasks_history() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(Round::text(120_000, 20, &["It shipped."]));
    rig.llm.push(answer(300));
    rig.ready().await;
    rig.turn("where is my order").await;
    let holds = |rig: &Rig| rig.lines().iter().any(|line| line["kind"] == "hold");
    rig.until("the holding line plays", 12_000, holds).await;
    let hold = rig.lines().into_iter().find(|line| line["kind"] == "hold");
    let hold = hold.expect("a holding line");
    rig.run(450).await;

    // The line is cut after a word or two, and the caller asks for something else.
    rig.send(Msg::ClientClear).await;
    rig.run(200).await;
    assert_eq!(
        rig.heard(&hold["utterance_id"]),
        "partial",
        "{}",
        rig.timeline()
    );
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;

    // What the second task's run was told of the call: both requests and the acknowledgement
    // the caller heard in full, not the start of the holding line. The first run's silence
    // played a stall line (chapter 10), noted in the history.
    let requests = rig.llm.requests();
    let second = requests.last().unwrap();
    assert_eq!(
        ix_mocks::messages(second, "user"),
        [
            "where is my order",
            "[the background failed; the agent said: Sorry, my systems are acting up. Bear with \
             me a moment.]",
            "what time does the store open"
        ]
    );
    assert_eq!(
        ix_mocks::messages(second, "assistant"),
        [rig.said()[0].clone()]
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_link_and_a_number_in_the_answer_are_never_spoken() {
    let rig = Rig::start(Opts::default()).await;
    let tokens = [
        "It opens at nine. ",
        "For more, visit www.larkspur-shop.org today. ",
        "Or call 1 800 555 0100 now. ",
        "HEARD: okay. ",
        "It closes at six.",
    ];
    rig.llm.push(Round::text(300, 20, &tokens));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_event("task_result", 1).await;
    rig.until_heard("It closes at six.").await;
    let relayed: Vec<Value> = rig
        .lines()
        .into_iter()
        .filter(|l| l["kind"] == "relay")
        .collect();
    let words: Vec<&str> = relayed
        .iter()
        .filter_map(|line| line["delta"].as_str())
        .collect();
    assert_eq!(words, ["It opens at nine.", "It closes at six."]);
    assert_eq!(
        rig.events("task_result")[0]["summary"],
        "It opens at nine. It closes at six."
    );
}

/// Live P1 (r15-verify): the background said its own prompt's first line to the caller. A
/// sentence that repeats the system prompt the gateway sent it, the operator's or the persona's
/// preamble, is never spoken and is not part of the result.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_sentence_that_repeats_the_backgrounds_prompt_is_never_spoken() {
    let rig = Rig::start(Opts::default()).await;
    let tokens = [
        "It opens at nine. ",
        // The operator's prompt's first line (the rig's whole prompt): who it is.
        "You answer callers of the Larkspur shop. ",
        "It closes at six.",
    ];
    rig.llm.push(Round::text(300, 20, &tokens));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_event("task_result", 1).await;
    rig.until_heard("It closes at six.").await;
    let relayed: Vec<String> = rig
        .lines()
        .into_iter()
        .filter(|l| l["kind"] == "relay")
        .filter_map(|line| line["delta"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(relayed, ["It opens at nine.", "It closes at six."]);
    assert_eq!(
        rig.events("task_result")[0]["summary"],
        "It opens at nine. It closes at six."
    );
}

// ---- the session's background is a Bud agent

use budgateway::{completed, created, delta, tool};

/// A voice-agent session: the entry's `interaction` block and the rest of the entry.
fn agent_session(interaction: Option<Value>, entry: Value) -> Opts {
    Opts {
        interaction,
        agent: Some(entry),
        ..Opts::default()
    }
}

impl Rig {
    fn bud(&self) -> &budgateway::Gateway {
        self.bud.as_ref().expect("an agent session")
    }

    /// The next run of the agent answers with these frames, each after its pause in ms.
    fn agent_run(&self, frames: Vec<(u64, Value)>) {
        self.bud().replies.lock().push_back(Reply::Sse(frames));
    }
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_agents_answer_is_relayed_when_its_run_completes() {
    let entry = json!({
        "fillers": {"use_tool_status_messages": true},
        "tool_status_messages": {"lookup_order": ["Let me look that order up."]},
    });
    let rig = Rig::start(agent_session(None, entry)).await;
    rig.agent_run(vec![
        (200, created("resp_1")),
        (0, delta("Let me check that for you. ")),
        (100, tool("mcp_1", "lookup_order", false)),
        (2_000, tool("mcp_1", "lookup_order", true)),
        (100, delta("Your order shipped on Monday. ")),
        (0, delta("It arrives Friday.")),
        (2_000, completed()),
    ]);
    assert_eq!(
        rig.handle.echo()["tasks"],
        json!({
            "backend": "agent:support", "speculative_start": "off", "tools": [],
            "reconnect_grace_s": 60, "push": false,
            "session_end": "auto", "orphan_max_s": 1800,
            // No orchestrator, no agents, no limit unless one is set.
            "orchestrator": false, "agents": [],
            "limits": {"max_live": null, "max_live_tenant": null},
            // An update is offered at the end of an answer.
            "announce": "ask",
            // A finished task's agent keeps its context for 30 minutes; no resume key.
            "follow_up_ttl_s": 1800, "resume": false,
        })
    );
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until_event("task_result", 1).await;
    rig.until_heard("It arrives Friday.").await;

    // One run of the agent, as the agent engine would have asked for it.
    let bodies = rig.bud().bodies.lock().clone();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert_eq!(
        bodies[0]["prompt"],
        json!({"id": "support", "version": "2"})
    );
    assert_eq!(bodies[0]["input"], "where is my order");
    assert_eq!(bodies[0]["conversation"], "conv_voice_s");
    assert_eq!(bodies[0]["bud_channel"], "voice");
    assert!(bodies[0].get("bud_truncate").is_none());
    assert_eq!(rig.bud().auth.lock()[0], "Bearer bud-caller-key");
    assert!(rig.bud().cancels.lock().is_empty());

    // A task like any other, by the agent: never started before the turn was settled.
    let created = &rig.events("task_created")[0];
    assert_eq!(created["background"], "agent:support");
    assert_eq!(created["speculative"], false);
    assert_eq!(created["side_effects"], true, "an agent with tools");
    // The caller is not left in silence, and the tool's phrase is the progress.
    assert!(rig.wire().iter().any(|(_, event)| is_ack(event)));
    let progress = rig.events("task_progress");
    let phrase = |event: &Value| event["text"] == "Let me look that order up.";
    assert!(progress.iter().any(phrase), "{}", rig.timeline());

    // What the agent wrote before its tool was a draft. The answer could be told from one
    // only when the run completed, two seconds after its last word.
    let said = rig.said();
    assert!(!said.iter().any(|line| line.contains("Let me check")));
    let ran = |event: &Value| event["effects"] == json!(["used lookup order"]);
    let tool_done = rig.time_of("the tool's effect", ran);
    let first = rig.time_of("the answer", says("Your order shipped on Monday."));
    assert!(first >= tool_done + 2_100, "{}", rig.timeline());
    let result = &rig.events("task_result")[0];
    assert_eq!(result["state"], "completed");
    assert_eq!(result["effects"], json!(["used lookup order"]));
    assert_eq!(
        result["summary"],
        "Your order shipped on Monday. It arrives Friday."
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_declared_marker_streams_an_agents_answer_and_a_cut_is_told_to_its_next_run() {
    let block = json!({ "background": { "answer_marker": "ANSWER:" } });
    let rig = Rig::start(agent_session(
        Some(block),
        json!({"allow_preemptive": true}),
    ))
    .await;
    rig.agent_run(vec![
        (200, created("resp_1")),
        (300, delta("Looking at the order. ANSWER: ")),
        (0, delta(&(LONG_SENTENCE.to_owned() + " "))),
        (20, delta("The ")),
        (6_000, delta("tracking number ends in four two.")),
        (0, completed()),
    ]);
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&LONG_SENTENCE.to_owned())
    })
    .await;
    // The agent is still writing: the sentence after the marker did not wait for it.
    assert!(rig.events("task_result").is_empty(), "{}", rig.timeline());
    assert_eq!(rig.events("task_created")[0]["side_effects"], false);
    let cut_line = rig.line(LONG_SENTENCE);
    rig.run(1_000).await;

    // The caller talks over it: the speech stops, the run does not.
    rig.onset().await;
    rig.run(1_000).await;
    assert_eq!(rig.events("agent_audio_stopped").len(), 1);
    assert_eq!(rig.heard(&cut_line["utterance_id"]), "partial");
    rig.run(200).await;
    rig.offset(1_200).await;
    rig.run(500).await;
    rig.silence(700).await;
    rig.run(800).await;
    rig.silence(1_500).await;
    rig.until_event("task_result", 1).await;
    rig.until_heard("The tracking number ends in four two.")
        .await;
    assert_eq!(rig.events("task_result")[0]["state"], "completed");
    assert!(rig.bud().cancels.lock().is_empty(), "the work went on");
    assert!(!rig.said().iter().any(|line| line.contains("Looking at")));

    // The next run tells budprompt what of that answer the caller heard: the start of the
    // cut sentence, and the sentence that came after the cut.
    rig.agent_run(vec![
        (100, created("resp_2")),
        (0, delta("ANSWER: We open at nine.")),
        (0, completed()),
    ]);
    rig.run(500).await;
    rig.turn("and what time does the store open").await;
    rig.until_heard("We open at nine.").await;
    let second = rig.bud().bodies.lock()[1].clone();
    assert_eq!(second["input"], "and what time does the store open");
    assert_eq!(second["bud_truncate"]["response_id"], "resp_1");
    let heard = second["bud_truncate"]["output_text"]
        .as_str()
        .expect("the heard text")
        .to_owned();
    let (start, rest) = heard
        .split_once(" The tracking number ends in four two.")
        .unwrap_or_else(|| panic!("the sentence after the cut: {heard:?}"));
    assert!(rest.is_empty(), "{heard:?}");
    assert!(
        LONG_SENTENCE.starts_with(start) && start.len() < LONG_SENTENCE.len() / 2,
        "{heard:?}"
    );
    assert!(start.starts_with("Your order"), "{heard:?}");

    // That answer was heard to its end: the run after it has nothing to shorten.
    rig.agent_run(vec![
        (100, created("resp_3")),
        (0, delta("ANSWER: Until six.")),
        (0, completed()),
    ]);
    rig.run(6_500).await;
    rig.turn("and when does the store close").await;
    rig.until_heard("Until six.").await;
    assert!(rig.bud().bodies.lock()[2].get("bud_truncate").is_none());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_second_request_waits_for_the_agents_first_run() {
    let rig = Rig::start(agent_session(None, json!({}))).await;
    rig.agent_run(vec![
        (100, created("resp_1")),
        (5_000, delta("Your order shipped on Monday.")),
        (0, completed()),
    ]);
    rig.agent_run(vec![
        (100, created("resp_2")),
        (0, delta("The store opens at nine.")),
        (0, completed()),
    ]);
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until_event("task_created", 1).await;
    rig.run(1_500).await;

    // budprompt takes one run of a conversation at a time: the second task is queued.
    let typed = "what time does the store open on sunday".to_owned();
    rig.send(Msg::TypedTurn(typed.clone())).await;
    rig.until_event("task_created", 2).await;
    let queued = &rig.events("task_created")[1];
    assert_eq!(queued["state"], "submitted", "{}", rig.timeline());
    rig.run(500).await;
    assert_eq!(
        rig.bud().bodies.lock().len(),
        1,
        "one run on the conversation"
    );

    rig.until_event("task_result", 2).await;
    rig.until_heard("The store opens at nine.").await;
    let bodies = rig.bud().bodies.lock().clone();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[1]["input"], typed);
    let first_done = rig.time_of("the first result", |event| event["type"] == "task_result");
    let second_started = rig.time_of("the second task starts", |event| {
        event["type"] == "task_progress"
            && event["task_id"] == queued["task_id"]
            && event["state"] == "working"
    });
    assert!(second_started >= first_done, "{}", rig.timeline());
    assert!(rig.heard_in_full("Your order shipped on Monday."));
    assert!(rig.bud().cancels.lock().is_empty());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_cancel_while_an_agents_tool_runs_says_what_it_had_started() {
    let rig = Rig::start(agent_session(None, json!({}))).await;
    rig.agent_run(vec![
        (100, created("resp_1")),
        (100, tool("mcp_1", "refund_order", false)),
        (60_000, completed()),
    ]);
    rig.ready().await;
    rig.turn("please refund my last order").await;
    rig.until_event("task_created", 1).await;
    rig.run(1_500).await;
    rig.turn("cancel that").await;
    rig.until_event("task_cancelled", 1).await;

    // The gateway cannot see what budprompt's tool did: it never says that nothing happened.
    let cancelled = &rig.events("task_cancelled")[0];
    assert_eq!(cancelled["effects"], json!(["started refund order"]));
    assert_eq!(cancelled["by"], "user");
    rig.until_heard("I've stopped it, but before it stopped it had already started refund order.")
        .await;
    assert!(!rig.said().contains(&"It's stopped.".to_owned()));
    // The run is cancelled at budprompt, under the agent's name.
    rig.until("the run is cancelled at the Bud gateway", 5_000, |rig| {
        !rig.bud().cancels.lock().is_empty()
    })
    .await;
    assert_eq!(
        *rig.bud().cancels.lock(),
        [("resp_1".to_owned(), "support".to_owned())]
    );
    assert!(rig.events("task_result").is_empty());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_agent_run_held_for_approval_is_said_so() {
    let rig = Rig::start(agent_session(None, json!({}))).await;
    let held = json!({"error": {
        "code": "approval_required_foreground",
        "message": "a manager must approve refunds",
    }});
    rig.bud().replies.lock().push_back(Reply::Status(403, held));
    rig.ready().await;
    rig.turn("please refund my last order").await;
    rig.until_event("task_result", 1).await;
    let result = &rig.events("task_result")[0];
    assert_eq!(result["state"], "failed");
    assert_eq!(
        result["summary"],
        "approval_required: a manager must approve refunds"
    );
    assert_eq!(result["effects"], "none_committed");
    rig.until_heard("That needs someone's approval first, so I haven't done it.")
        .await;
    assert!(
        !rig.said().iter().any(|line| line.contains("couldn't")),
        "{}",
        rig.timeline()
    );
}

// ---------------------------------------------------------------------------------------------
// A background that answers in text on its own (a text-output DAG)
// ---------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
#[serial]
async fn a_text_backend_runs_one_task_per_request_and_its_text_is_relayed() {
    let graph = Arc::new(Graph {
        wait: 900,
        ..Graph::default()
    });
    let first = "The store opens at nine. It closes at six.";
    graph.replies.lock().push_back(Ok(vec![first]));
    graph
        .replies
        .lock()
        .push_back(Ok(vec!["Yes.", "Every day of the week."]));
    let rig = Rig::start(Opts {
        graph: Some(graph.clone()),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    let echo = rig.handle.echo();
    assert_eq!(echo["tasks"]["backend"], "dag");
    assert_eq!(echo["tasks"]["speculative_start"], "off");

    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;
    rig.until_event("task_result", 1).await;
    let wire: Vec<Value> = rig.wire().into_iter().map(|(_, event)| event).collect();
    let of = |kind: &str| -> Vec<&Value> { wire.iter().filter(|e| e["type"] == kind).collect() };
    let created = of("task_created");
    assert_eq!(created.len(), 1, "{}", rig.timeline());
    assert_eq!(created[0]["background"], "dag");
    assert_eq!(created[0]["speculative"], false);
    assert_eq!(created[0]["side_effects"], false);
    // Nothing ran before the turn was settled, and the graph got the caller's words.
    assert_eq!(*graph.requests.lock(), ["what time does the store open"]);
    // Each sentence of the text is a line of its own, word for word, by the background.
    let relayed: Vec<&str> = of("assistant_transcript")
        .into_iter()
        .filter(|e| e["kind"] == "relay" && e["delta"] != "")
        .inspect(|e| assert!(e["source"] == "background" && e["verbatim"] == true))
        .map(|e| e["delta"].as_str().unwrap())
        .collect();
    assert_eq!(relayed, ["The store opens at nine.", "It closes at six."]);
    let result = of("task_result")[0];
    assert_eq!(result["state"], "completed");
    assert_eq!(result["summary"], first);

    // Two pieces of text from the graph are two stretches of speech of one answer.
    rig.turn("is that every day").await;
    rig.until_heard("Every day of the week.").await;
    rig.until_event("task_result", 2).await;
    let results = rig.wire();
    let second = results
        .iter()
        .filter(|(_, e)| e["type"] == "task_result")
        .nth(1)
        .unwrap();
    assert_eq!(second.1["summary"], "Yes. Every day of the week.");
    assert_eq!(graph.requests.lock().len(), 2);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_text_backend_that_fails_or_is_cancelled_says_so() {
    let graph = Arc::new(Graph {
        wait: 20_000,
        ..Graph::default()
    });
    graph.replies.lock().push_back(Err("DAG node llm failed"));
    graph.replies.lock().push_back(Ok(vec!["Too late."]));
    let rig = Rig::start(Opts {
        graph: Some(graph.clone()),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_event("task_result", 1).await;
    let failed = rig.wire();
    let result = &failed
        .iter()
        .find(|(_, e)| e["type"] == "task_result")
        .unwrap()
        .1;
    assert_eq!(result["state"], "failed", "{}", rig.timeline());
    rig.run(4_000).await;

    // A run that is cancelled by the client stops the graph.
    rig.turn("and what time does it close").await;
    rig.until_event("task_created", 2).await;
    let created = rig.wire();
    let task = created
        .iter()
        .filter(|(_, e)| e["type"] == "task_created")
        .nth(1)
        .unwrap()
        .1["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    rig.handle.send(Msg::TaskCancel { task_id: task }).await;
    rig.until_event("task_cancelled", 1).await;
    rig.run(500).await;
    let wire = rig.wire();
    let cancelled = &wire
        .iter()
        .find(|(_, e)| e["type"] == "task_cancelled")
        .unwrap()
        .1;
    assert_eq!(cancelled["by"], "client");
    assert_eq!(cancelled["effects"], "none_committed");
    assert!(!wire.iter().any(|(_, e)| e["delta"] == "Too late."));
}

// ---------------------------------------------------------------------------------------------
// The operator's catalog: a webhook, an A2A agent, an MCP task tool, a Responses model
// ---------------------------------------------------------------------------------------------

const HOOK_SECRET: &str = "s3cret-of-the-hook";

/// A catalog entry, with the secret its `secret_env` would have named.
fn backend(entry: Value, secret: Option<&str>) -> BackendSpec {
    let mut spec: BackendSpec = serde_json::from_value(entry).expect("a catalog entry");
    spec.secret = secret.map(str::to_owned);
    spec
}

/// A session whose block names the one entry of the catalog. `operator`: the server's block.
async fn rig_on(spec: BackendSpec, operator: Option<Value>) -> Rig {
    let interaction = Some(json!({ "tasks": { "default_backend": spec.name } }));
    Rig::start(Opts {
        interaction,
        operator,
        catalog: vec![spec],
        ..Opts::default()
    })
    .await
}

/// A webhook's signed callback about `task`.
fn call_back(task: &str, secret: &str, event: Value) -> Result<(), CallbackRefused> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let (now, body) = (
        now.expect("a clock").as_secs().to_string(),
        event.to_string(),
    );
    let signature = ix_mocks::sign(secret, &now, &body);
    let sender = remote::CallbackFrom {
        timestamp: Some(&now),
        signature: Some(&signature),
        ..remote::CallbackFrom::default()
    };
    remote::callback(task, sender, &body)
}

impl Rig {
    /// The id of the `n`th task, once it exists.
    async fn task(&self, n: usize) -> String {
        self.until_event("task_created", n).await;
        let created = &self.events("task_created")[n - 1];
        created["task_id"].as_str().expect("a task id").to_owned()
    }

    async fn answer(&self, question: &Value, value: &str) {
        let request_id = question["request_id"]
            .as_str()
            .expect("request id")
            .to_owned();
        let value = Some(json!(value));
        let outcome = HitlOutcome::Accept { value };
        self.send(Msg::TaskAnswer {
            request_id,
            outcome,
        })
        .await;
    }
}

/// A JSON-RPC result as a backend double returns it.
fn rpc_result(result: Value) -> (u16, String) {
    (
        200,
        json!({ "jsonrpc": "2.0", "id": "1", "result": result }).to_string(),
    )
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_webhook_backend_reports_progress_asks_the_caller_and_answers_by_callback() {
    let hook = BackendDouble::start(|hit: &Hit| {
        let reply = match hit.call.body["type"].as_str() {
            Some("task") => json!({ "type": "progress", "text": "Looking up your order." }),
            Some("answer") => json!([{ "type": "effect", "label": "reopened the order" }]),
            _ => json!({}),
        };
        (200, reply.to_string())
    })
    .await;
    let entry = json!({ "name": "crm", "type": "webhook", "url": hook.url });
    let rig = rig_on(backend(entry, Some(HOOK_SECRET)), None).await;
    rig.ready().await;
    let echo = rig.handle.echo();
    assert_eq!(echo["tasks"]["backend"], "webhook:crm");
    assert_eq!(echo["tasks"]["speculative_start"], "off");

    rig.turn("where is my order").await;
    let task = rig.task(1).await;
    let created = rig.events("task_created")[0].clone();
    assert_eq!(created["background"], "webhook:crm");
    assert_eq!(
        (&created["side_effects"], &created["speculative"]),
        (&json!(true), &json!(false))
    );
    // The task goes out signed like a tool call, with where to report.
    rig.until("the webhook has the task", 5_000, |_| {
        !hook.hits().is_empty()
    })
    .await;
    let sent = hook.hits()[0].call.clone();
    assert!(sent.signed_with(HOOK_SECRET), "{sent:?}");
    assert_eq!(sent.headers["idempotency-key"], format!("{task}:1:task"));
    assert_eq!(sent.body["request"], "where is my order");
    assert_eq!(
        (&sent.body["task_id"], &sent.body["revision"]),
        (&json!(task), &json!(1))
    );
    assert_eq!(sent.body["callback"], format!("/v1/tasks/{task}/callback"));
    let told = |rig: &Rig| {
        let progress = rig.events("task_progress");
        progress
            .iter()
            .any(|event| event["text"] == "Looking up your order.")
    };
    rig.until("the webhook's progress is passed on", 5_000, told)
        .await;

    // Only the webhook's own signature reports on the task, and only on its live revision.
    let result = json!({ "type": "result", "text": "Not this." });
    assert_eq!(
        call_back(&task, "another-secret", result.clone()),
        Err(CallbackRefused::Unsigned)
    );
    assert_eq!(
        call_back("task_0", HOOK_SECRET, result),
        Err(CallbackRefused::UnknownTask)
    );
    let old = json!({ "type": "result", "text": "Not this.", "revision": 7 });
    assert_eq!(
        call_back(&task, HOOK_SECRET, old),
        Err(CallbackRefused::Stale)
    );

    // It asks the caller through the callback; the answer goes back to it.
    let question = json!({ "type": "question", "request_id": "q1", "kind": "value",
                           "prompt": "What is the order number?" });
    call_back(&task, HOOK_SECRET, question).expect("a signed callback");
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "value", "{}", rig.timeline());
    rig.until_heard("What is the order number?").await;
    rig.answer(&question, "A 17").await;
    rig.until("the webhook has the answer", 5_000, |_| {
        hook.hits().len() == 2
    })
    .await;
    let answer = hook.hits()[1].call.clone();
    assert!(answer.signed_with(HOOK_SECRET));
    let said = (
        &answer.body["type"],
        &answer.body["request_id"],
        &answer.body["outcome"],
    );
    assert_eq!(said, (&json!("answer"), &json!("q1"), &json!("accept")));
    assert_eq!(answer.body["value"], "A 17");

    // The result comes by callback and is relayed word for word, with what the webhook did.
    let result = json!({ "type": "result", "text": "Order A 17 ships on Friday." });
    call_back(&task, HOOK_SECRET, result).expect("a signed callback");
    rig.until_heard("Order A 17 ships on Friday.").await;
    rig.until_event("task_result", 1).await;
    let result = &rig.events("task_result")[0];
    assert_eq!(result["state"], "completed", "{}", rig.timeline());
    assert_eq!(result["summary"], "Order A 17 ships on Friday.");
    assert_eq!(result["effects"], json!(["reopened the order"]));
    // The run is over: nobody listens for that task any more.
    let late = json!({ "type": "progress", "text": "Still here." });
    assert_eq!(
        call_back(&task, HOOK_SECRET, late),
        Err(CallbackRefused::UnknownTask)
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_cancelled_webhook_task_is_stopped_by_the_webhook_and_says_what_it_did() {
    // The webhook confirms the cancel of the first task it sees and says nothing to any other.
    let confirms = Arc::new(Mutex::new(None::<String>));
    let first = confirms.clone();
    let hook = BackendDouble::start(move |hit: &Hit| {
        let body = &hit.call.body;
        let id = body["task_id"].as_str().unwrap_or_default().to_owned();
        let confirmed = first.lock().get_or_insert(id.clone()) == &id;
        let reply = match body["type"].as_str() {
            Some("cancel") if confirmed => {
                json!([{ "type": "effect", "label": "held the table" }, { "type": "cancelled" }])
            }
            _ => json!({}),
        };
        (200, reply.to_string())
    })
    .await;
    let entry = json!({ "name": "crm", "type": "webhook", "url": hook.url });
    let rig = rig_on(backend(entry, Some(HOOK_SECRET)), None).await;
    rig.ready().await;

    // The webhook confirms the cancel and says what it had done by then.
    rig.turn("can you book a table for two at seven").await;
    let task = rig.task(1).await;
    rig.until("the webhook has the task", 5_000, |_| {
        !hook.hits().is_empty()
    })
    .await;
    rig.send(Msg::TaskCancel {
        task_id: task.clone(),
    })
    .await;
    rig.until_event("task_cancelled", 1).await;
    let cancelled = rig.events("task_cancelled")[0].clone();
    assert_eq!(
        cancelled["effects"],
        json!(["held the table"]),
        "{}",
        rig.timeline()
    );
    let truth = "I've stopped it, but before it stopped it had already held the table.";
    rig.until_heard(truth).await;
    let cancel = hook.hits()[1].call.clone();
    assert!(cancel.signed_with(HOOK_SECRET));
    assert_eq!(
        (&cancel.body["type"], &cancel.body["task_id"]),
        (&json!("cancel"), &json!(task))
    );
    rig.run(6_000).await;

    // A webhook that does not confirm leaves what it did unknown; the caller is told so.
    rig.turn("can you send the brochure to my address").await;
    let second = rig.task(2).await;
    rig.until("the webhook has the second task", 5_000, |_| {
        hook.hits().len() == 3
    })
    .await;
    rig.send(Msg::TaskCancel { task_id: second }).await;
    rig.until_event("task_cancelled", 2).await;
    let cancelled = rig.events("task_cancelled")[1].clone();
    assert_eq!(cancelled["effects"], "unknown", "{}", rig.timeline());
    rig.until_heard("I've stopped it, but I can't confirm whether it had already gone through.")
        .await;
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_a2a_agent_runs_the_task_asks_for_input_and_is_cancelled_by_name() {
    // The agent's task goes working, asks, and completes after the answer; a second task
    // only ever works.
    let polls = Arc::new(Mutex::new(0u32));
    let seen = polls.clone();
    let agent = BackendDouble::start(move |hit: &Hit| {
        let (method, params) = (hit.call.body["method"].as_str(), &hit.call.body["params"]);
        let status = |state: &str, text: &str| {
            let message = json!({ "role": "ROLE_AGENT", "parts": [{ "text": text }] });
            json!({ "state": state, "message": message })
        };
        let task =
            |id: &str, status: Value| json!({ "id": id, "contextId": "ctx-1", "status": status });
        let working = status("TASK_STATE_WORKING", "Checking the calendar.");
        match method {
            Some("SendMessage") if params["message"]["taskId"] == "tk-1" => {
                *seen.lock() = 10;
                rpc_result(json!({ "task": task("tk-1", working) }))
            }
            Some("SendMessage") if params["message"]["contextId"] == "ctx-1" => {
                rpc_result(json!({ "task": task("tk-2", working) }))
            }
            Some("SendMessage") => rpc_result(json!({ "task": task("tk-1", working) })),
            Some("GetTask") if params["id"] == "tk-2" => rpc_result(task("tk-2", working)),
            Some("GetTask") => {
                let mut polls = seen.lock();
                *polls += 1;
                match *polls {
                    1 => rpc_result(task("tk-1", working)),
                    2..10 => {
                        let asks = status("TASK_STATE_INPUT_REQUIRED", "Which day do you mean?");
                        rpc_result(task("tk-1", asks))
                    }
                    _ => {
                        let mut done = task("tk-1", json!({ "state": "TASK_STATE_COMPLETED" }));
                        let parts = json!([{ "text": "Tuesday at ten is free." }]);
                        done["artifacts"] = json!([{ "artifactId": "a1", "parts": parts }]);
                        rpc_result(done)
                    }
                }
            }
            Some("CancelTask") => {
                rpc_result(task("tk-2", json!({ "state": "TASK_STATE_CANCELED" })))
            }
            _ => (404, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "calendar", "type": "a2a", "url": agent.url, "bearer_env": "X" });
    let rig = rig_on(backend(entry, Some("agent-token")), None).await;
    rig.ready().await;

    rig.turn("is there a free slot next week").await;
    rig.until_event("task_question", 1).await;
    let created = rig.events("task_created")[0].clone();
    assert_eq!(created["background"], "a2a:calendar");
    // The session's start read the agent's card first (a GET; none here).
    let hits = agent.hits();
    let first = hits
        .iter()
        .find(|hit| hit.method != "GET")
        .expect("a call")
        .call
        .clone();
    assert_eq!(first.headers["a2a-version"], "1.0");
    assert_eq!(first.headers["authorization"], "Bearer agent-token");
    assert_eq!(first.body["method"], "SendMessage");
    let message = &first.body["params"]["message"];
    assert_eq!(message["role"], "ROLE_USER");
    assert_eq!(
        message["parts"][0]["text"],
        "is there a free slot next week"
    );
    assert_eq!(
        first.body["params"]["configuration"]["returnImmediately"],
        true
    );
    // What the agent does is not seen here: the task's effects are unknown from the start.
    let progress = rig.events("task_progress");
    assert!(
        progress.iter().any(|event| event["effects"] == "unknown"),
        "{}",
        rig.timeline()
    );
    assert!(
        progress
            .iter()
            .any(|event| event["text"] == "Checking the calendar.")
    );

    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "value");
    rig.until_heard("Which day do you mean?").await;
    rig.answer(&question, "Tuesday").await;
    rig.until_heard("Tuesday at ten is free.").await;
    rig.until_event("task_result", 1).await;
    let result = &rig.events("task_result")[0];
    assert_eq!(result["state"], "completed", "{}", rig.timeline());
    assert_eq!(result["summary"], "Tuesday at ten is free.");
    assert_eq!(result["effects"], "unknown");
    let answer = agent
        .bodies()
        .into_iter()
        .find(|body| body["params"]["message"]["taskId"] == "tk-1");
    let answer = answer.expect("the answer goes to the task");
    assert_eq!(answer["params"]["message"]["parts"][0]["text"], "Tuesday");
    rig.run(6_000).await;

    // The session's next task continues the agent's context, and a cancel names the task.
    rig.turn("and please put my name down for the first one")
        .await;
    let second = rig.task(2).await;
    let polled = |_: &Rig| {
        agent
            .bodies()
            .iter()
            .any(|body| body["params"]["id"] == "tk-2")
    };
    rig.until("the agent's second task is read", 5_000, polled)
        .await;
    rig.send(Msg::TaskCancel { task_id: second }).await;
    rig.until_event("task_cancelled", 1).await;
    let cancelled = rig.events("task_cancelled")[0].clone();
    assert_eq!(cancelled["effects"], "unknown", "{}", rig.timeline());
    let cancel = agent
        .bodies()
        .into_iter()
        .find(|body| body["method"] == "CancelTask");
    assert_eq!(cancel.expect("a cancel")["params"]["id"], "tk-2");
    rig.until_heard("I've stopped it, but I can't confirm whether it had already gone through.")
        .await;
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_mcp_task_tool_runs_the_task_elicits_a_choice_and_is_cancelled() {
    let polls = Arc::new(Mutex::new(0u32));
    let seen = polls.clone();
    let server = BackendDouble::start(move |hit: &Hit| {
        let (method, params) = (hit.call.body["method"].as_str(), &hit.call.body["params"]);
        let task = |id: &str, status: &str, more: Value| {
            let mut task = json!({
                "taskId": id, "status": status, "createdAt": "2026-10-14T08:00:00Z",
                "lastUpdatedAt": "2026-10-14T08:00:00Z", "ttlMs": 600_000, "pollIntervalMs": 300,
            });
            for (key, value) in more.as_object().into_iter().flatten() {
                task[key] = value.clone();
            }
            task
        };
        let complete = |mut result: Value| {
            result["resultType"] = json!("complete");
            rpc_result(result)
        };
        match method {
            Some("tools/call") => {
                let second = params["arguments"]["question"].as_str().is_some_and(|q| q.contains("red"));
                let id = if second { "mt-2" } else { "mt-1" };
                let mut created = task(id, "working", json!({ "statusMessage": "Searching the stock." }));
                created["resultType"] = json!("task");
                rpc_result(created)
            }
            Some("tasks/get") if params["taskId"] == "mt-2" => {
                let cancelled = *seen.lock() >= 100;
                let status = if cancelled { "cancelled" } else { "working" };
                complete(task("mt-2", status, json!({})))
            }
            Some("tasks/get") => {
                let mut polls = seen.lock();
                *polls += 1;
                match *polls {
                    1 => complete(task("mt-1", "working", json!({ "statusMessage": "Searching the stock." }))),
                    2..10 => {
                        let schema = json!({ "type": "object", "properties": {
                            "size": { "type": "string", "enum": ["small", "large"] } } });
                        let ask = json!({ "method": "elicitation/create", "params": {
                            "mode": "form", "message": "Which size?", "requestedSchema": schema } });
                        complete(task("mt-1", "input_required", json!({ "inputRequests": { "pick": ask } })))
                    }
                    _ => {
                        let content = json!([{ "type": "text", "text": "A large one is in stock." }]);
                        complete(task("mt-1", "completed", json!({ "result": { "content": content } })))
                    }
                }
            }
            Some("tasks/update") => {
                *seen.lock() = 10;
                complete(json!({}))
            }
            Some("tasks/cancel") => {
                *seen.lock() = 100;
                complete(json!({}))
            }
            _ => (404, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "stock", "type": "mcp", "url": server.url, "tool": "find_item",
                        "argument": "question", "side_effects": "none" });
    let rig = rig_on(backend(entry, None), None).await;
    rig.ready().await;

    rig.turn("do you have the blue kettle").await;
    rig.until_event("task_question", 1).await;
    let created = rig.events("task_created")[0].clone();
    assert_eq!(
        (&created["background"], &created["side_effects"]),
        (&json!("mcp:stock"), &json!(false))
    );
    // The server refused `initialize` (404): each request stands alone.
    assert_eq!(server.hits()[0].call.body["method"], "initialize");
    let calls = server.hits().into_iter();
    let mut calls = calls.filter(|hit| hit.call.body["method"] == "tools/call");
    let call = calls.next().expect("the call").call;
    assert!(!call.headers.contains_key("mcp-session-id"));
    assert_eq!(call.headers["mcp-protocol-version"], "2026-07-28");
    assert_eq!(
        (&call.headers["mcp-method"], &call.headers["mcp-name"]),
        (&"tools/call".to_owned(), &"find_item".to_owned())
    );
    let params = &call.body["params"];
    assert_eq!(params["name"], "find_item");
    assert_eq!(
        params["arguments"],
        json!({ "question": "do you have the blue kettle" })
    );
    let meta = &params["_meta"];
    assert_eq!(
        meta["io.modelcontextprotocol/protocolVersion"],
        "2026-07-28"
    );
    let capabilities = &meta["io.modelcontextprotocol/clientCapabilities"];
    assert!(capabilities["extensions"]["io.modelcontextprotocol/tasks"].is_object());

    // The form's one field with its two values is a choice for the caller.
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "choose", "{}", rig.timeline());
    assert_eq!(question["options"], json!(["small", "large"]));
    rig.answer(&question, "large").await;
    rig.until_heard("A large one is in stock.").await;
    rig.until_event("task_result", 1).await;
    let result = &rig.events("task_result")[0];
    assert_eq!(
        (&result["state"], &result["effects"]),
        (&json!("completed"), &json!("none_committed"))
    );
    let update = server
        .bodies()
        .into_iter()
        .find(|body| body["method"] == "tasks/update");
    let update = update.expect("the answer is sent");
    assert_eq!(update["params"]["taskId"], "mt-1");
    let accepted = json!({ "pick": { "action": "accept", "content": { "size": "large" } } });
    assert_eq!(update["params"]["inputResponses"], accepted);
    rig.run(6_000).await;

    // A cancel is asked of the server, which says later that the task stopped.
    rig.turn("do you have the red kettle").await;
    let second = rig.task(2).await;
    let polled = |_: &Rig| {
        server
            .bodies()
            .iter()
            .any(|body| body["params"]["taskId"] == "mt-2")
    };
    rig.until("the second task is read", 5_000, polled).await;
    rig.send(Msg::TaskCancel { task_id: second }).await;
    rig.until_event("task_cancelled", 1).await;
    assert_eq!(rig.events("task_cancelled")[0]["effects"], "none_committed");
    rig.until_heard("It's stopped.").await;
    let cancelled = |_: &Rig| {
        server
            .bodies()
            .iter()
            .any(|body| body["method"] == "tasks/cancel")
    };
    rig.until("the server is told", 5_000, cancelled).await;
}

/// Server-sent events of a Responses stream.
fn events(all: &[Value]) -> String {
    let event = |event: &Value| {
        format!(
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap()
        )
    };
    all.iter().map(event).collect()
}

/// A Responses object with one message.
fn response(id: &str, status: &str, text: &str) -> Value {
    let content = json!([{ "type": "output_text", "text": text, "annotations": [] }]);
    let message = json!({ "type": "message", "role": "assistant", "content": content });
    let output = if text.is_empty() {
        json!([])
    } else {
        json!([message])
    };
    json!({ "id": id, "object": "response", "status": status, "output": output })
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_responses_model_streams_its_answer_and_a_broken_stream_is_picked_up() {
    let model = BackendDouble::start(|hit: &Hit| {
        let delta = |n: u64, text: &str| {
            json!({ "type": "response.output_text.delta", "sequence_number": n, "delta": text })
        };
        let whole = "The shop opens at nine. It closes at six.";
        if hit.method == "POST" {
            // The stream breaks after the first sentence.
            let created = json!({ "type": "response.created", "sequence_number": 0,
                                  "response": response("resp_1", "in_progress", "") });
            return (200, events(&[created, delta(1, "The shop opens "), delta(2, "at nine. ")]));
        }
        assert_eq!(hit.path, "/responses/resp_1?stream=true&starting_after=2");
        let done = json!({ "type": "response.completed", "sequence_number": 5,
                           "response": response("resp_1", "completed", whole) });
        (200, events(&[delta(3, "It closes "), delta(4, "at six."), done]))
    })
    .await;
    let entry = json!({ "name": "research", "type": "openai_responses", "url": model.url,
                        "model": "gpt-test", "api_key_env": "X" });
    let rig = rig_on(backend(entry, Some("sk-test")), None).await;
    rig.ready().await;
    assert_eq!(
        rig.handle.echo()["tasks"]["backend"],
        "openai_responses:research"
    );

    rig.turn("what time does the shop open").await;
    rig.until_heard("It closes at six.").await;
    rig.until_event("task_result", 1).await;
    let result = &rig.events("task_result")[0];
    assert_eq!(result["state"], "completed", "{}", rig.timeline());
    assert_eq!(
        result["summary"],
        "The shop opens at nine. It closes at six."
    );
    assert_eq!(rig.events("task_created")[0]["speculative"], false);
    let hits = model.hits();
    assert_eq!(hits.len(), 2, "one request and one resume");
    let request = &hits[0].call;
    assert_eq!(
        (hits[0].method.as_str(), hits[0].path.as_str()),
        ("POST", "/responses")
    );
    assert_eq!(request.headers["authorization"], "Bearer sk-test");
    let body = &request.body;
    assert_eq!(
        (&body["model"], &body["background"], &body["stream"]),
        (&json!("gpt-test"), &json!(true), &json!(true))
    );
    assert!(
        body["instructions"]
            .as_str()
            .unwrap()
            .ends_with("You answer callers of the Larkspur shop.")
    );
    let input = body["input"].as_array().expect("input items");
    assert_eq!(
        input.last().unwrap(),
        &json!({ "role": "user", "content": "what time does the shop open" })
    );
    assert!(body.get("tools").is_none());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_polled_responses_model_asks_the_caller_calls_a_tool_and_is_cancelled() {
    let tool = ToolDouble::start(json!({ "status": "in the oven" })).await;
    // Round one asks the caller, round two looks the order up, round three answers; a
    // fourth response never ends.
    let model = BackendDouble::start(|hit: &Hit| {
        let call = |id: &str, name: &str, arguments: Value| {
            let call = json!({ "type": "function_call", "id": format!("fc_{id}"), "call_id": id,
                               "name": name, "arguments": arguments.to_string() });
            let mut response = response("resp", "completed", "");
            response["output"] = json!([call]);
            response
        };
        let path = hit.path.as_str();
        let reply = match (hit.method.as_str(), path) {
            ("POST", "/responses") => {
                let outputs = hit.call.body["input"].as_array().map_or(0, |input| {
                    input
                        .iter()
                        .filter(|item| item["type"] == "function_call_output")
                        .count()
                });
                let fresh = hit.call.body["input"].as_array().is_some_and(|input| {
                    input
                        .iter()
                        .any(|item| item["content"] == "where is my other order")
                });
                let id = if fresh {
                    "resp_9".to_owned()
                } else {
                    format!("resp_{}", outputs + 1)
                };
                response(&id, "queued", "")
            }
            ("GET", "/responses/resp_1") => {
                let ask = json!({ "kind": "value", "question": "What is the order number?" });
                call("c1", "ask_caller", ask)
            }
            ("GET", "/responses/resp_2") => call("c2", "order_status", json!({ "order": "A 17" })),
            ("GET", "/responses/resp_3") => {
                response("resp_3", "completed", "ANSWER:\nYour pizza is in the oven.")
            }
            ("GET", "/responses/resp_9") => response("resp_9", "in_progress", ""),
            ("POST", "/responses/resp_9/cancel") => response("resp_9", "cancelled", ""),
            _ => return (404, "{}".to_owned()),
        };
        (200, reply.to_string())
    })
    .await;
    let entry = json!({ "name": "research", "type": "openai_responses", "url": model.url,
                        "model": "gpt-test", "mode": "poll" });
    let tools = json!([{ "name": "order_status", "description": "Look an order up.", "url": tool.url,
                         "parameters": { "type": "object", "properties": { "order": { "type": "string" } } } }]);
    let rig = rig_on(
        backend(entry, None),
        Some(json!({ "tasks": { "tools": tools } })),
    )
    .await;
    rig.ready().await;

    rig.turn("where is my pizza").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "value", "{}", rig.timeline());
    rig.answer(&question, "A 17").await;
    rig.until_heard("Your pizza is in the oven.").await;
    rig.until_event("task_result", 1).await;
    assert_eq!(
        rig.events("task_result")[0]["summary"],
        "Your pizza is in the oven."
    );
    // The session's own tool was called by the gateway, with the model's arguments.
    let calls = tool.calls();
    assert_eq!(calls.len(), 1, "{}", rig.timeline());
    assert_eq!(calls[0].body["arguments"], json!({ "order": "A 17" }));
    // Each round carries the model's call and its result back, and the tools it may call.
    let posts: Vec<Value> = model
        .hits()
        .into_iter()
        .filter(|hit| hit.method == "POST")
        .map(|hit| hit.call.body)
        .collect();
    assert_eq!(posts.len(), 3);
    assert_eq!(
        (&posts[0]["background"], &posts[0]["stream"]),
        (&json!(true), &json!(false))
    );
    let names: Vec<&str> = posts[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["order_status", "ask_caller"]);
    assert!(
        posts[0]["instructions"]
            .as_str()
            .unwrap()
            .contains("ANSWER:")
    );
    let last = posts[2]["input"].as_array().unwrap();
    let output = &last[last.len() - 1];
    assert_eq!(
        (&output["type"], &output["call_id"]),
        (&json!("function_call_output"), &json!("c2"))
    );
    assert_eq!(
        output["output"],
        json!({ "status": "in the oven" }).to_string()
    );
    let answered = last
        .iter()
        .find(|item| item["call_id"] == "c1" && item["type"] == "function_call_output");
    assert_eq!(
        answered.unwrap()["output"],
        json!({ "answer": "A 17" }).to_string()
    );
    rig.run(6_000).await;

    // A cancel stops the background response on the server.
    rig.turn("where is my other order").await;
    let second = rig.task(2).await;
    let polled = |_: &Rig| {
        model
            .hits()
            .iter()
            .any(|hit| hit.path == "/responses/resp_9")
    };
    rig.until("the response is read", 5_000, polled).await;
    rig.send(Msg::TaskCancel { task_id: second }).await;
    rig.until_event("task_cancelled", 1).await;
    assert_eq!(rig.events("task_cancelled")[0]["effects"], "none_committed");
    let stopped = |_: &Rig| {
        model
            .hits()
            .iter()
            .any(|hit| hit.path == "/responses/resp_9/cancel")
    };
    rig.until("the server is told", 5_000, stopped).await;
}

// ---- Release 2: listening sounds, the client's playback, screen questions, personas

/// The `audio_segment` events of one lane and phase.
fn segments(rig: &Rig, lane: &str, phase: &str) -> Vec<Value> {
    let of = |event: &Value| event["lane"] == lane && event["phase"] == phase;
    rig.events("audio_segment").into_iter().filter(of).collect()
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_long_turn_gets_a_listening_sound_that_stops_when_the_caller_goes_on() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    assert_eq!(rig.handle.echo()["duplex"]["backchannel"], "auto");
    let clip = rig.tts.spoken().into_iter().any(|text| text == "mhm");
    assert!(
        clip,
        "the clip is rendered at start: {:?}",
        rig.tts.spoken()
    );
    // 1.6 s of speech, a pause Smart Turn says is not the end.
    rig.onset().await;
    rig.run(1_800).await;
    rig.offset(1_600).await;
    let frames_before = rig.frames.lock().len();
    rig.smart_turn(0.3).await;
    rig.until("a listening sound", 2_000, |rig| {
        !segments(rig, "backchannel", "start").is_empty()
    })
    .await;
    let start = segments(&rig, "backchannel", "start")[0].clone();
    assert_eq!(start["kind"], "backchannel");
    rig.run(100).await;
    assert!(
        rig.frames.lock().len() > frames_before,
        "its audio reached the client"
    );
    // The caller goes on: it stops at once, and it is no line of the agent's.
    rig.onset().await;
    rig.until("it stops", 500, |rig| {
        !segments(rig, "backchannel", "end").is_empty()
    })
    .await;
    assert!(rig.lines().is_empty(), "{}", rig.timeline());
    assert!(rig.events("task_created").is_empty());
    // Not again within 6 s.
    rig.run(800).await;
    rig.offset(800).await;
    rig.smart_turn(0.3).await;
    rig.run(300).await;
    assert_eq!(segments(&rig, "backchannel", "start").len(), 1);
}

/// A caller who has just stopped the agent ("wait a second") hears no listening sound at the
/// pause after it: the gap between listening sounds counts from the agent's own words too.
#[tokio::test(start_paused = true)]
#[serial]
async fn no_listening_sound_soon_after_the_agent_spoke() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.push(slow_answer(6_000));
    rig.ready().await;
    rig.turn("where is my order").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&LONG_SENTENCE.to_owned())
    })
    .await;
    rig.run(1_000).await;
    rig.onset().await;
    rig.stt.interim("wait a second").await;
    rig.until_event("agent_audio_stopped", 1).await;
    assert_eq!(
        rig.events("agent_audio_stopped").len(),
        1,
        "{}",
        rig.timeline()
    );
    rig.run(750).await;
    rig.offset(800).await;
    rig.smart_turn(0.3).await;
    rig.run(300).await;
    assert!(
        segments(&rig, "backchannel", "start").is_empty(),
        "{}",
        rig.timeline()
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_phone_line_or_a_session_that_turns_them_off_gets_no_listening_sound() {
    for phone in [true, false] {
        let interaction = (!phone).then(|| json!({ "duplex": { "backchannel": false } }));
        let rig = Rig::start(Opts {
            interaction,
            ..Opts::default()
        })
        .await;
        if phone {
            rig.ctl.set_phone_leg();
        }
        rig.ready().await;
        rig.onset().await;
        rig.run(1_400).await;
        rig.offset(1_200).await;
        rig.smart_turn(0.3).await;
        rig.run(1_000).await;
        assert!(
            segments(&rig, "backchannel", "start").is_empty(),
            "phone {phone}: {}",
            rig.timeline()
        );
    }
}

#[tokio::test(start_paused = true)]
#[serial]
async fn what_the_client_reports_playing_is_what_the_caller_heard() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    let (first, second) = (
        "The first line, which runs for a while.",
        "The second line.",
    );
    rig.send(Msg::ClientSpeak(first.to_owned())).await;
    rig.send(Msg::ClientSpeak(second.to_owned())).await;
    let id = |rig: &Rig, words: &str| rig.line(words)["utterance_id"].clone();
    rig.until("the first line starts", 5_000, |rig| {
        rig.said().contains(&first.to_owned())
    })
    .await;
    let first_id = id(&rig, first);
    let starts = segments(&rig, "front", "start");
    assert!(
        starts
            .iter()
            .any(|segment| segment["utterance_id"] == first_id)
    );
    // The client plays 300 ms of the first line and stops there, saying so twice a second,
    // while the pump plays both lines out.
    for _ in 0..12 {
        let at = PlaybackPosition(first_id.as_str().unwrap().to_owned(), 300);
        rig.send(at.into()).await;
        rig.run(500).await;
    }
    let second_id = id(&rig, second);
    assert_eq!(rig.heard(&first_id), "none", "{}", rig.timeline());
    assert_eq!(rig.heard(&second_id), "none");
    assert!(
        !segments(&rig, "front", "end").is_empty(),
        "the pump played them out"
    );

    // "Say that again" is the line the client stopped in.
    rig.send(Msg::TypedTurn("sorry, say that again".to_owned()))
        .await;
    rig.until("the repeat", 5_000, |rig| rig.said().len() == 3)
        .await;
    assert_eq!(rig.said()[2], first);
    let repeat_id = rig.lines()[2]["utterance_id"].clone();
    // The client plays the repeat to its end: the first line was heard in part, the second
    // not at all, the repeat in full.
    for _ in 0..8 {
        let at = PlaybackPosition(repeat_id.as_str().unwrap().to_owned(), 60_000);
        rig.send(at.into()).await;
        rig.run(500).await;
    }
    assert_eq!(rig.heard(&first_id), "partial", "{}", rig.timeline());
    assert_eq!(rig.heard(&second_id), "none");
    assert_eq!(rig.heard(&repeat_id), "full");
}

/// The client's `playback_position`.
struct PlaybackPosition(String, u64);

impl From<PlaybackPosition> for Msg {
    fn from(PlaybackPosition(utterance_id, played_ms): PlaybackPosition) -> Msg {
        Msg::PlaybackPosition {
            utterance_id,
            played_ms,
        }
    }
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_form_is_asked_on_the_screen_and_its_answer_goes_back_to_the_backend() {
    let hook = BackendDouble::start(|hit: &Hit| {
        let reply = match hit.call.body["type"].as_str() {
            Some("task") => json!({
                "type": "question", "request_id": "f1", "kind": "form",
                "prompt": "Where should the flowers go?",
                "schema": { "type": "object",
                            "properties": { "street": { "type": "string" } } }
            }),
            Some("answer") => {
                json!({ "type": "result", "text": "They go to High Street on Friday." })
            }
            _ => json!({}),
        };
        (200, reply.to_string())
    })
    .await;
    let entry = json!({ "name": "florist", "type": "webhook", "url": hook.url });
    let rig = rig_on(backend(entry, Some(HOOK_SECRET)), None).await;
    rig.ready().await;
    rig.turn("send flowers to my mother").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "form", "{}", rig.timeline());
    assert_eq!(question["channels"], json!(["client"]));
    assert_eq!(question["schema"]["properties"]["street"]["type"], "string");
    rig.until_heard("I'm putting a secure form on your screen for the details.")
        .await;
    assert_eq!(hook.hits().len(), 1, "{}", rig.timeline());
    let request_id = question["request_id"].as_str().unwrap().to_owned();
    let value = Some(json!({ "street": "High Street" }));
    rig.send(Msg::TaskAnswer {
        request_id,
        outcome: HitlOutcome::Accept { value },
    })
    .await;
    rig.until("the webhook has the form", 5_000, |_| {
        hook.hits().len() == 2
    })
    .await;
    let answer = hook.hits()[1].call.body.clone();
    assert_eq!(answer["value"], json!({ "street": "High Street" }));
    rig.until_heard("They go to High Street on Friday.").await;
    let received = &rig.events("task_answer_received")[0];
    assert_eq!(
        (&received["by"], &received["value_redacted"]),
        (&json!("client"), &json!(false))
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn the_operators_persona_speaks_and_the_callers_fields_reach_the_background() {
    let persona = json!({
        "id": "lumen-lumi", "name": "Lumi", "org": "Lumen Air", "role": "booking assistant",
        "banks": { "ack": [{ "text": "Lumi here, checking." }] }
    });
    let rig = Rig::start(Opts {
        operator: Some(json!({ "front": { "persona": persona } })),
        interaction: Some(json!({ "front": { "caller": {
            "preferred_name": "Sam", "verbosity": "brief" } } })),
        ..Opts::default()
    })
    .await;
    assert_eq!(rig.handle.echo()["front"]["persona"], "lumen-lumi");
    rig.llm.push(answer(1_500));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_heard("Lumi here, checking.").await;
    rig.until_heard("It closes at six.").await;
    let request = rig.llm.requests()[0].clone();
    let system = request["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(system.contains("the voice of Lumen Air"), "{system}");
    assert!(
        system.ends_with(
            "CALLER CONTEXT (data)\n- The caller's name: Sam.\n\
             - The caller prefers brief answers: at most 20 words."
        ),
        "{system}"
    );
}

/// Release 4, DTMF on SIP: a caller on a phone line (the room found a SIP participant) whose
/// session takes keypad answers types the card number the background asks for, ending it with
/// `#`, and confirms the payment with 1. Spoken digits are not the value; the value reaches
/// the background and is redacted on the wire.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_caller_on_a_phone_line_types_a_card_number_and_confirms_with_keys() {
    let tool = ToolDouble::start(json!({ "effect": "paid the bill" })).await;
    let tools = json!([{
        "name": "pay_bill", "description": "Use it to pay the bill.", "url": tool.url,
        "parameters": { "type": "object", "properties": { "card": { "type": "string" } } },
        "side_effects": true, "irreversible": true, "confirm_phrase": "pay the bill",
    }]);
    let rig = Rig::start(Opts {
        operator: Some(
            json!({ "tasks": { "tools": tools }, "hitl": { "channels": ["voice", "dtmf"] } }),
        ),
        ..Opts::default()
    })
    .await;
    rig.ctl.set_phone_leg();
    // The room's recording: paused while the number is typed, and only then.
    let recording: Arc<Mutex<Vec<bool>>> = Arc::default();
    let seen = recording.clone();
    rig.send(Msg::RecordingPause(actor::RecordingPause(Arc::new(
        move |paused| {
            seen.lock().push(paused);
            let done: actor::Referral = Box::pin(async {});
            done
        },
    ))))
    .await;
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(
                300,
                "ask_caller",
                json!({ "kind": "value", "question": "What is your card number?" }),
            ),
            1 => Round::tool(300, "pay_bill", json!({ "card": "4111" })),
            _ => Round::text(50, 20, &["ANSWER:\n", "The bill is paid."]),
        })
    });
    rig.ready().await;
    rig.turn("please pay my bill").await;

    rig.until_heard(
        "For your security, please type your card number on your keypad, then press the pound key.",
    )
    .await;
    // Said aloud, the number is not taken.
    rig.run(300).await;
    rig.turn("it is 4111").await;
    rig.until_heard("Please don't say that out loud. Use your keypad instead.")
        .await;
    assert!(rig.events("task_answer_received").is_empty());
    for key in ['9', '*', '4', '1', '1', '1', '#'] {
        rig.send(Msg::Dtmf(key)).await;
        rig.run(150).await;
    }
    rig.until_event("task_answer_received", 1).await;
    let typed = rig.events("task_answer_received")[0].clone();
    assert_eq!(
        (&typed["outcome"], &typed["by"]),
        (&json!("accept"), &json!("dtmf"))
    );
    assert_eq!(
        (&typed["value"], &typed["value_redacted"]),
        (&Value::Null, &json!(true))
    );
    let sent = serde_json::to_string(&rig.llm.requests()).unwrap();
    assert!(sent.contains("4111") && !sent.contains("94111"), "{sent}");
    rig.until("the recording again", 2_000, |_| {
        recording.lock().len() == 2
    })
    .await;
    assert_eq!(*recording.lock(), [true, false]);

    // The payment is confirmed on the keypad: the hint follows the question.
    rig.until_event("task_question", 2).await;
    rig.until_heard("You can also press 1 for yes, or 2 for no.")
        .await;
    assert!(tool.calls().is_empty());
    rig.send(Msg::Dtmf('1')).await;
    rig.until_event("task_result", 1).await;
    let accepted = rig.events("task_answer_received")[1].clone();
    assert_eq!(
        (&accepted["outcome"], &accepted["by"]),
        (&json!("accept"), &json!("dtmf"))
    );
    assert_eq!(tool.calls().len(), 1, "{}", rig.timeline());
    rig.until_heard("The bill is paid.").await;
    // A confirmation by key is no sensitive value: the recording went on.
    assert_eq!(*recording.lock(), [true, false]);
}

// ---- voice experience: listening-sound intensity and fade, the capture window of a
// sensitive value, the caller's pace, the heartbeat

/// The persona's `medium` lowered to `low` by the session: one listening sound in a caller
/// turn, and one that must yield fades out over 40 ms instead of stopping dead.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_low_intensity_makes_one_listening_sound_a_turn_and_it_fades_when_it_yields() {
    let interaction = json!({ "duplex": { "backchannel": { "intensity": "low" } } });
    let rig = Rig::start(Opts {
        interaction: Some(interaction),
        ..Opts::default()
    })
    .await;
    // A long clip: "mhm" is 570 ms, so half of it is still to play when it yields (the
    // others, 760 ms and more, fail the clip check).
    rig.tts
        .ms_per_char
        .store(190, std::sync::atomic::Ordering::SeqCst);
    rig.ready().await;
    assert_eq!(rig.handle.echo()["duplex"]["backchannel_intensity"], "low");
    // 0.8 s of speech is too little for `low` (2.1 s).
    rig.onset().await;
    rig.run(1_000).await;
    rig.offset(800).await;
    rig.smart_turn(0.3).await;
    rig.run(300).await;
    assert!(segments(&rig, "backchannel", "start").is_empty());
    // 2.4 s in all: the first eligible pause plays.
    rig.onset().await;
    rig.run(1_800).await;
    rig.offset(1_600).await;
    rig.smart_turn(0.3).await;
    rig.until("a listening sound", 2_000, |rig| {
        !segments(rig, "backchannel", "start").is_empty()
    })
    .await;
    rig.run(300).await;
    let clip_peak = rig.frames.lock().last().map(|(_, peak)| *peak).unwrap();
    // The caller goes on: the clip fades over two frames and no more of it goes out.
    let yielded = rig.onset().await;
    rig.run(600).await;
    let tail = rig.frames_in(yielded, yielded + 600);
    assert_eq!(tail.len(), 2, "two faded frames: {tail:?}");
    assert!(
        tail[0].1 <= clip_peak && tail[1].1 < clip_peak / 2,
        "{clip_peak} then {tail:?}"
    );
    // Later pauses of the same turn get none: `low` makes one a turn.
    for _ in 0..2 {
        rig.run(5_000).await;
        rig.offset(1_500).await;
        rig.smart_turn(0.3).await;
        rig.run(300).await;
        rig.onset().await;
    }
    assert_eq!(segments(&rig, "backchannel", "start").len(), 1);
}

// ---- clips per language and voice (chapter 5): every listening sound is checked before it
// enters the bank; a recorded clip replaces the rendered one

/// The caller talks for 1.6 s and pauses; Smart Turn says the turn goes on.
async fn long_pause(rig: &Rig) {
    rig.onset().await;
    rig.run(1_800).await;
    rig.offset(1_600).await;
    rig.smart_turn(0.3).await;
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_clip_that_fails_its_check_is_replaced_by_the_fallback_word() {
    // The persona's sounds are "mhm" and "I see"; the pack's fallback word is "okay".
    let persona = json!({
        "id": "lumen-lumi", "name": "Lumi", "org": "Lumen Air", "role": "booking assistant",
        "backchannel_clips": ["mhm", "i-see"]
    });
    let rig = Rig::start(Opts {
        operator: Some(json!({ "front": { "persona": persona } })),
        ..Opts::default()
    })
    .await;
    // This voice renders "mhm" as silence.
    rig.tts.silence("mhm");
    rig.ready().await;
    // After the warm-up lines.
    let spoken = rig.tts.spoken();
    let warm = spoken
        .iter()
        .position(|line| line == "mhm")
        .expect("the clips");
    assert_eq!(spoken[warm..warm + 2], ["mhm", "I see"], "{spoken:?}");
    assert!(
        spoken[warm + 2..].contains(&"okay".to_owned()),
        "{spoken:?}"
    );
    assert_eq!(rig.ctl.clips(), 2, "\"I see\" and the fallback \"okay\"");
    assert!(rig.events("interaction_updated").is_empty());
    long_pause(&rig).await;
    rig.until("a listening sound", 2_000, |rig| {
        !segments(rig, "backchannel", "start").is_empty()
    })
    .await;
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_voice_with_no_usable_clip_makes_no_listening_sounds() {
    // A recogniser that answers once per upload checks each clip's words. Heard as a
    // listening sound, every clip passes.
    let rig = Rig::start(Opts {
        request_response: true,
        ..Opts::default()
    })
    .await;
    rig.stt.hear_in_clips("Mm-hmm.");
    rig.ready().await;
    assert_eq!((rig.stt.clips_heard(), rig.ctl.clips()), (3, 3));
    assert!(rig.events("interaction_updated").is_empty());
    // A voice that spells every sound out: no clip passes, the fallback "okay" among them.
    let rig = Rig::start(Opts {
        request_response: true,
        ..Opts::default()
    })
    .await;
    rig.stt.hear_in_clips("M. H. M.");
    rig.ready().await;
    assert_eq!((rig.stt.clips_heard(), rig.ctl.clips()), (3, 0));
    // Said once.
    let updated = rig.events("interaction_updated");
    assert_eq!(updated.len(), 1, "{updated:?}");
    assert_eq!(updated[0]["degraded"], json!(["backchannel_unavailable"]));
    // A streaming session whose voice renders every sound as silence makes none either.
    let rig = Rig::start(Opts::default()).await;
    for text in ["mhm", "okay", "I see"] {
        rig.tts.silence(text);
    }
    rig.ready().await;
    assert_eq!(rig.ctl.clips(), 0);
    long_pause(&rig).await;
    rig.run(1_000).await;
    assert!(segments(&rig, "backchannel", "start").is_empty());
    assert_eq!(rig.events("interaction_updated").len(), 1);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_recorded_clip_replaces_the_rendered_one() {
    let dir = std::env::temp_dir().join(format!("waav-ix-clips-{}", std::process::id()));
    let rig = Rig::start(Opts {
        clips_dir: Some(dir.display().to_string()),
        ..Opts::default()
    })
    .await;
    // The operator's recordings for this provider's default voice in English: a 300 ms
    // "mhm", louder than the voice, and an "okay" too long to pass (900 ms).
    let at = dir.join(&rig.tts.provider).join("default").join("en");
    std::fs::create_dir_all(&at).unwrap();
    for (clip, ms) in [("mhm", 300), ("okay", 900)] {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav = hound::WavWriter::create(at.join(format!("{clip}.wav")), spec).unwrap();
        // A 400 Hz tone.
        for i in 0..16 * ms {
            let phase = std::f32::consts::TAU * 400.0 * i as f32 / 16_000.0;
            wav.write_sample((12_000.0 * phase.sin()) as i16).unwrap();
        }
        wav.finalize().unwrap();
    }
    rig.ready().await;
    std::fs::remove_dir_all(&dir).unwrap();
    let spoken = rig.tts.spoken();
    assert!(!spoken.contains(&"mhm".to_owned()), "{spoken:?}");
    let rendered = |text: &str| spoken.iter().filter(|said| *said == text).count();
    // The refused recording is rendered.
    assert_eq!((rendered("okay"), rendered("I see")), (1, 1), "{spoken:?}");
    assert_eq!(rig.ctl.clips(), 3);
    // The first listening sound is the recording, 3 dB down: 12,000 is about 8,500, the
    // voice's 8,000 about 5,700.
    let before = rig.frames.lock().len();
    long_pause(&rig).await;
    rig.until("a listening sound", 2_000, |rig| {
        !segments(rig, "backchannel", "start").is_empty()
    })
    .await;
    rig.run(200).await;
    let peak = rig.frames.lock()[before..]
        .iter()
        .map(|(_, peak)| *peak)
        .max();
    assert!(
        peak.is_some_and(|peak| (8_000..9_000).contains(&peak)),
        "{peak:?}"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn listening_sounds_turned_on_mid_session_are_made_then() {
    let rig = Rig::start(Opts {
        interaction: Some(json!({ "duplex": { "backchannel": false } })),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    assert_eq!(rig.ctl.clips(), 0);
    let patch = json!({ "duplex": { "backchannel": true } });
    rig.send(Msg::Update(patch)).await;
    rig.run(2_000).await;
    assert_eq!(rig.ctl.clips(), 3);
    long_pause(&rig).await;
    rig.until("a listening sound", 2_000, |rig| {
        !segments(rig, "backchannel", "start").is_empty()
    })
    .await;
}

/// Plan chapter 4's capture window: while a sensitive value is asked on the screen the
/// recogniser hears silence and its words reach nobody; the caller who speaks anyway is
/// pointed at the screen once; the answer from the screen closes the window.
#[tokio::test(start_paused = true)]
#[serial]
async fn the_recogniser_hears_silence_while_a_sensitive_value_is_on_the_screen() {
    // `ask_caller` comes with the session's tools.
    let tool = ToolDouble::start(json!({ "effect": "saved the card" })).await;
    let tools = json!([{
        "name": "save_card", "description": "Use it to save a card.", "url": tool.url,
        "parameters": { "type": "object", "properties": { "card": { "type": "string" } } },
    }]);
    let rig = Rig::start(Opts {
        operator: Some(json!({ "tasks": { "tools": tools } })),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(
                300,
                "ask_caller",
                json!({ "kind": "value", "question": "What is your card number?" }),
            ),
            _ => Round::text(50, 20, &["ANSWER:\n", "The card is on file now."]),
        })
    });
    rig.ready().await;
    rig.turn("please update my card").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(
        (&question["sensitive"], &question["channels"]),
        (&json!(true), &json!(["client"]))
    );
    rig.until_heard("I'm putting a secure form on your screen for your card number.")
        .await;
    assert!(rig._vm.stt_muted(), "{}", rig.timeline());
    // What the caller's microphone sends reaches the recogniser as silence.
    let before = rig.stt.audio().len();
    let _ = rig
        ._vm
        .receive_audio(bytes::Bytes::from(vec![7u8; 640]))
        .await;
    let sent = rig.stt.audio();
    assert_eq!(sent.len(), before + 640);
    assert!(sent[before..].iter().all(|byte| *byte == 0));
    // Said aloud anyway: no word of it is kept, and the caller is pointed at the screen.
    rig.run(300).await;
    rig.turn("it is 4111 1111 1111 1111").await;
    rig.until_heard("Please don't say that out loud. Use your screen instead.")
        .await;
    assert!(rig.events("task_answer_received").is_empty());
    // The screen answers: the window closes and the recogniser hears the caller again.
    let request_id = question["request_id"].as_str().unwrap().to_owned();
    let value = Some(json!("4000 0000 0000 0002"));
    rig.send(Msg::TaskAnswer {
        request_id,
        outcome: HitlOutcome::Accept { value },
    })
    .await;
    rig.until_event("task_answer_received", 1).await;
    rig.run(50).await;
    assert!(!rig._vm.stt_muted());
    rig.until_heard("The card is on file now.").await;
    let sent = serde_json::to_string(&rig.llm.requests()).unwrap();
    assert!(
        sent.contains("4000 0000 0000 0002") && !sent.contains("4111"),
        "{sent}"
    );
    assert_eq!(
        rig.said()
            .iter()
            .filter(|line| line.starts_with("Please don't say"))
            .count(),
        1
    );
}

/// `front.caller.pace` sets the provider's speaking rate before the first line and again
/// when it changes: the same line is longer at `slower` than at `faster`.
#[tokio::test(start_paused = true)]
#[serial]
async fn the_callers_pace_changes_the_speaking_rate_mid_session() {
    let interaction = json!({ "front": { "caller": { "pace": "slower" } } });
    let rig = Rig::start(Opts {
        interaction: Some(interaction),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    assert_eq!(rig.tts.rates(), [0.85]);
    let line = "This line is the same at both paces.";
    let spoken = |rig: &Rig, phase: &str| -> Vec<Value> {
        let of = |event: &Value| event["phase"] == phase && event["lane"] != "backchannel";
        rig.events("audio_segment").into_iter().filter(of).collect()
    };
    let length = |rig: &Rig, n: usize| {
        let (start, end) = (
            spoken(rig, "start")[n]["t_ms"].as_u64().unwrap(),
            spoken(rig, "end")[n]["t_ms"].as_u64().unwrap(),
        );
        rig.frames_in(start - 1, end).len()
    };
    rig.send(Msg::ClientSpeak(line.to_owned())).await;
    rig.until("the line ends", 10_000, |rig| {
        !spoken(rig, "end").is_empty()
    })
    .await;
    rig.send(Msg::Update(
        json!({ "front": { "caller": { "pace": "faster" } } }),
    ))
    .await;
    rig.run(100).await;
    assert_eq!(rig.tts.rates(), [0.85, 1.15]);
    rig.send(Msg::ClientSpeak(line.to_owned())).await;
    rig.until("the line ends again", 10_000, |rig| {
        spoken(rig, "end").len() == 2
    })
    .await;
    let (slow, fast) = (length(&rig, 0), length(&rig, 1));
    // 1.15 / 0.85 = 1.35.
    assert!(
        slow * 100 >= fast * 125 && slow * 100 <= fast * 145,
        "{slow} frames slower, {fast} faster"
    );
    // Back to the session's own rate.
    rig.send(Msg::Update(
        json!({ "front": { "caller": { "pace": "normal" } } }),
    ))
    .await;
    rig.run(100).await;
    assert_eq!(rig.tts.rates(), [0.85, 1.15, 1.0]);
}

/// A quiet session tells the client it is alive every 5 s, once `ready` went out.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_quiet_session_sends_a_heartbeat_every_five_seconds_after_ready() {
    let rig = Rig::start(Opts::default()).await;
    rig.run(7_000).await;
    assert!(rig.events("heartbeat").is_empty(), "not before ready");
    rig.send(Msg::Ready).await;
    let ready = rig.now();
    rig.run(16_000).await;
    let beats: Vec<Ms> = rig
        .wire()
        .into_iter()
        .filter(|(_, event)| event["type"] == "heartbeat")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(beats.len(), 3, "{}", rig.timeline());
    assert!(
        (ready + 5_000..ready + 5_100).contains(&beats[0]),
        "{beats:?}"
    );
    assert!(beats.windows(2).all(|pair| pair[1] - pair[0] >= 5_000));
    // Anything else the session sends restarts the count.
    rig.send(Msg::ClientSpeak("Still here.".to_owned())).await;
    rig.run(3_000).await;
    assert_eq!(rig.events("heartbeat").len(), 3);
}

// ---- questions on the screen from every background, and what the background remembers

/// `ask_caller` with `kind: "form"`: the model names the fields, the client fills them in on
/// its screen, and the run gets them as an object.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_form_the_model_asks_for_is_filled_in_on_the_screen() {
    let tool = ToolDouble::start(json!({ "effect": "moved the parcel" })).await;
    let tools = json!([{
        "name": "move_parcel", "description": "Send a parcel to another address.", "url": tool.url,
        "parameters": { "type": "object", "properties": { "street": { "type": "string" } } },
    }]);
    let rig = Rig::start(Opts {
        operator: Some(json!({ "tasks": { "tools": tools } })),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(
                300,
                "ask_caller",
                json!({ "kind": "form", "question": "Where should it go?",
                        "fields": ["street", "city"] }),
            ),
            _ => Round::text(50, 20, &["ANSWER:\n", "It goes to Harbour Road now."]),
        })
    });
    rig.ready().await;
    rig.turn("please send my parcel somewhere else").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "form", "{}", rig.timeline());
    assert_eq!(question["channels"], json!(["client"]));
    assert_eq!(question["sensitive"], false);
    assert_eq!(question["schema"]["required"], json!(["street", "city"]));
    rig.until_heard("I'm putting a secure form on your screen for the details.")
        .await;
    let request_id = question["request_id"].as_str().unwrap().to_owned();
    let value = Some(json!({ "street": "Harbour Road", "city": "Leith" }));
    rig.send(Msg::TaskAnswer {
        request_id,
        outcome: HitlOutcome::Accept { value },
    })
    .await;
    rig.until_heard("It goes to Harbour Road now.").await;
    let requests = rig.llm.requests();
    let results = ix_mocks::messages(requests.last().unwrap(), "tool");
    let result = &results[0];
    assert!(
        result.contains("Harbour Road") && result.contains("Leith"),
        "{result}"
    );
    let received = &rig.events("task_answer_received")[0];
    assert_eq!(
        (&received["by"], &received["value"]["city"]),
        (&json!("client"), &json!("Leith"))
    );
}

/// An MCP form elicitation with several fields is a form on the client's screen; the client's
/// object goes back as the elicitation's content.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_mcp_form_of_several_fields_is_filled_in_on_the_screen() {
    let answered = Arc::new(Mutex::new(false));
    let seen = answered.clone();
    let server = BackendDouble::start(move |hit: &Hit| {
        let (method, params) = (hit.call.body["method"].as_str(), &hit.call.body["params"]);
        let task = |status: &str, more: Value| {
            let mut task = json!({ "taskId": "mf-1", "status": status, "pollIntervalMs": 300,
                                   "createdAt": "2026-10-15T08:00:00Z", "ttlMs": 600_000 });
            for (key, value) in more.as_object().into_iter().flatten() {
                task[key] = value.clone();
            }
            json!({ "jsonrpc": "2.0", "id": hit.call.body["id"], "result": task }).to_string()
        };
        match method {
            Some("tools/call") => (200, task("working", json!({ "resultType": "task" }))),
            Some("tasks/update") => {
                *seen.lock() = true;
                (200, task("working", json!({})))
            }
            Some("tasks/get") if *seen.lock() => {
                let content = json!([{ "type": "text", "text": "It goes to Harbour Road." }]);
                (200, task("completed", json!({ "result": { "content": content } })))
            }
            Some("tasks/get") => {
                let schema = json!({ "type": "object", "required": ["street", "city"],
                    "properties": { "street": { "type": "string" }, "city": { "type": "string" } } });
                let ask = json!({ "method": "elicitation/create", "params": {
                    "mode": "form", "message": "Where should it go?", "requestedSchema": schema } });
                let _ = params;
                (200, task("input_required", json!({ "inputRequests": { "address": ask } })))
            }
            _ => (404, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "parcels", "type": "mcp", "url": server.url, "tool": "move",
                        "side_effects": "none" });
    let rig = rig_on(backend(entry, None), None).await;
    rig.ready().await;
    rig.turn("please send my parcel somewhere else").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(
        (&question["kind"], &question["channels"]),
        (&json!("form"), &json!(["client"])),
        "{}",
        rig.timeline()
    );
    assert_eq!(question["schema"]["required"], json!(["street", "city"]));
    rig.until_heard("I'm putting a secure form on your screen for the details.")
        .await;
    let request_id = question["request_id"].as_str().unwrap().to_owned();
    let fields = json!({ "street": "Harbour Road", "city": "Leith" });
    rig.send(Msg::TaskAnswer {
        request_id,
        outcome: HitlOutcome::Accept {
            value: Some(fields.clone()),
        },
    })
    .await;
    rig.until_heard("It goes to Harbour Road.").await;
    let update = server
        .bodies()
        .into_iter()
        .find(|body| body["method"] == "tasks/update")
        .expect("the answer is sent");
    assert_eq!(
        update["params"]["inputResponses"]["address"],
        json!({ "action": "accept", "content": fields })
    );
}

const SIGN_IN: &str = "Please sign in at https://id.example.com/authorize?state=7 to see it.";

/// An A2A agent that needs a sign-in sends a link: it is a question on the client's screen,
/// the caller hears that it was sent once the client's connection has it, and the agent goes
/// on when the client says the caller signed in.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_a2a_sign_in_is_a_link_on_the_screen_and_the_agent_goes_on_after_it() {
    let signed = Arc::new(Mutex::new(false));
    let seen = signed.clone();
    let agent = BackendDouble::start(move |hit: &Hit| {
        let (method, params) = (hit.call.body["method"].as_str(), &hit.call.body["params"]);
        let task = |status: Value| json!({ "id": "tk-a", "contextId": "ctx-a", "status": status });
        let says = |state: &str, text: &str| {
            json!({ "state": state, "message": { "role": "ROLE_AGENT", "parts": [{ "text": text }] } })
        };
        let signed_in = *seen.lock();
        match method {
            Some("SendMessage") if params["message"]["taskId"] == "tk-a" => {
                *seen.lock() = true;
                rpc_result(json!({ "task": task(says("TASK_STATE_WORKING", "Reading it.")) }))
            }
            Some("SendMessage") => {
                rpc_result(json!({ "task": task(says("TASK_STATE_AUTH_REQUIRED", SIGN_IN)) }))
            }
            Some("GetTask") if signed_in => {
                let mut done = task(json!({ "state": "TASK_STATE_COMPLETED" }));
                let parts = json!([{ "text": "Your statement shows two payments." }]);
                done["artifacts"] = json!([{ "artifactId": "a1", "parts": parts }]);
                rpc_result(done)
            }
            Some("GetTask") => rpc_result(task(says("TASK_STATE_AUTH_REQUIRED", SIGN_IN))),
            _ => (404, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "bank", "type": "a2a", "url": agent.url, "bearer_env": "X" });
    let rig = rig_on(backend(entry, Some("agent-token")), None).await;
    rig.ready().await;
    rig.turn("what payments are on my statement").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "auth_url", "{}", rig.timeline());
    assert_eq!(
        (
            &question["url"],
            &question["channels"],
            &question["sensitive"]
        ),
        (
            &json!("https://id.example.com/authorize?state=7"),
            &json!(["client"]),
            &json!(true)
        )
    );
    let before = "To do that I need you to sign in. I'm sending a secure link to your screen.";
    assert_eq!(question["prompt"], before);
    rig.until_heard("I've sent it.").await;
    let said = rig.said();
    let at = |line: &str| said.iter().position(|said| said == line);
    assert!(at(before) < at("I've sent it."), "{said:?}");
    let request_id = question["request_id"].as_str().unwrap().to_owned();
    rig.send(Msg::TaskAnswer {
        request_id,
        outcome: HitlOutcome::Accept {
            value: Some(json!(true)),
        },
    })
    .await;
    rig.until_heard("Your statement shows two payments.").await;
    let resumed = agent
        .bodies()
        .into_iter()
        .find(|body| body["params"]["message"]["taskId"] == "tk-a")
        .expect("the agent is told");
    assert_eq!(
        resumed["params"]["message"]["parts"][0]["text"],
        "Signed in."
    );
    let received = &rig.events("task_answer_received")[0];
    assert_eq!(
        (
            &received["by"],
            &received["value"],
            &received["value_redacted"]
        ),
        (&json!("client"), &Value::Null, &json!(true))
    );
}

const MEMORY_KEY: &str = "memory-key-of-the-actor-test";

/// After a call whose caller the operator's token named, the background model writes what is
/// worth keeping about the caller: only facts the memory may keep, added to what it had.
#[tokio::test(start_paused = true)]
#[serial]
async fn after_a_call_with_a_known_caller_the_background_adds_to_its_memory() {
    let rig = Rig::start(Opts {
        memory: Some((MEMORY_KEY, "caller-9")),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        let system = request["messages"][0]["content"]
            .as_str()
            .unwrap_or_default();
        Some(if system.starts_with("You keep short notes") {
            Round::text(
                50,
                10,
                &["- Prefers aisle seats\n", "- Card ends in 4417\n"],
            )
        } else {
            Round::text(300, 20, &["An aisle seat is free on that flight."])
        })
    });
    let owner = rig.owner.clone();
    tenant::remember(
        MEMORY_KEY,
        &owner,
        "caller-9",
        vec!["Travels with a dog".to_owned()],
    )
    .await
    .unwrap();
    rig.ready().await;
    rig.turn("I always sit on the aisle, is one free").await;
    rig.until_heard("An aisle seat is free on that flight.")
        .await;
    rig.handle.shutdown().await;
    let mut kept = Vec::new();
    for _ in 0..100 {
        kept = tenant::all_facts(MEMORY_KEY, &owner, "caller-9")
            .await
            .unwrap();
        if kept.len() > 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(kept, ["Prefers aisle seats", "Travels with a dog"]);
    let requests = rig.llm.requests();
    let asked = requests.last().unwrap()["messages"].to_string();
    assert!(
        asked.contains("Known facts:\\n(none)")
            && asked.contains("Caller: I always sit on the aisle, is one free")
            && asked.contains("Agent: An aisle seat is free on that flight."),
        "{asked}"
    );
}

// ---------------------------------------------------------------------------------------------
// Many agents: an orchestrator on the LLM tier with the catalog's agents as its tools
// ---------------------------------------------------------------------------------------------

/// A webhook agent of the catalog, by name, with a description and anything else of `extra`.
/// Its calls are not confirmed unless `extra` says so: these tests are about orchestration
/// (confirmations: `an_orchestrator_calls_an_mcp_servers_tools_...`).
fn agent_entry_of(name: &str, url: &str, extra: Value) -> BackendSpec {
    let mut entry = json!({ "name": name, "type": "webhook", "url": url, "confirm": false });
    if let (Some(entry), Some(extra)) = (entry.as_object_mut(), extra.as_object()) {
        entry.extend(extra.clone());
    }
    backend(entry, Some(HOOK_SECRET))
}

/// An orchestrator session (`tasks.orchestrator` on the LLM double) with these agents.
async fn orchestrated(catalog: Vec<BackendSpec>, agents: &[&str]) -> Rig {
    let interaction = json!({ "tasks": { "orchestrator": "mock", "agents": agents } });
    Rig::start(Opts {
        interaction: Some(interaction),
        catalog,
        ..Opts::default()
    })
    .await
}

/// How many "Task update" messages a request of the orchestrator carries.
fn task_updates(request: &Value) -> usize {
    let users = ix_mocks::messages(request, "user");
    users
        .iter()
        .filter(|text| text.starts_with("Task update"))
        .count()
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrator_starts_two_agents_in_one_round_and_hears_both_results() {
    // Both agents take the task and report later by callback.
    let taken = |_: &Hit| (200, "[]".to_owned());
    let browser = BackendDouble::start(taken).await;
    let coder = BackendDouble::start(taken).await;
    let catalog = vec![
        agent_entry_of(
            "browser",
            &browser.url,
            json!({ "description": "Operates websites in a browser: bookings, forms, purchases." }),
        ),
        agent_entry_of(
            "coder",
            &coder.url,
            json!({ "description": "Works on code in the caller's repository.", "side_effects": "none" }),
        ),
    ];
    let rig = orchestrated(catalog, &["browser", "coder"]).await;
    // One round calls both agents; each child's end is a later message.
    rig.llm.respond_with(|request| {
        let (tools, updates) = (
            ix_mocks::messages(request, "tool").len(),
            task_updates(request),
        );
        Some(match (tools, updates) {
            (0, _) => Round {
                tool_calls: vec![
                    (
                        "browser".to_owned(),
                        json!({ "request": "Book a flight to Delhi on Friday" }),
                    ),
                    (
                        "coder".to_owned(),
                        json!({ "request": "Fix the failing test" }),
                    ),
                ],
                end_ms: 400,
                ..Round::default()
            },
            (_, 0) => Round::text(50, 20, &["ANSWER:\n", "Both are under way."]),
            (_, 1) => Round::text(50, 20, &["ANSWER:\n", "Your flight is booked for 9:40."]),
            _ => Round::text(50, 20, &["ANSWER:\n", "And the test is fixed."]),
        })
    });
    rig.ready().await;
    let echo = rig.handle.echo();
    assert_eq!(echo["tasks"]["orchestrator"], true);
    assert_eq!(echo["tasks"]["agents"], json!(["browser", "coder"]));

    rig.turn("book me a flight to Delhi on Friday and fix the failing test")
        .await;
    rig.until_event("task_created", 3).await;
    let created = rig.events("task_created");
    let (request, flight, test) = (&created[0], &created[1], &created[2]);
    assert_eq!(request["parent_id"], Value::Null);
    let trace = request["trace_id"].as_str().expect("a trace").to_owned();
    assert_eq!(trace.len(), 32);
    for (child, agent) in [(flight, "browser"), (test, "coder")] {
        assert_eq!(child["parent_id"], request["task_id"]);
        assert_eq!(child["agent"], agent);
        assert_eq!(child["trace_id"], trace);
        assert_eq!(child["background"], format!("webhook:{agent}"));
    }
    // The orchestrator was given the agents as tools, with the catalog's descriptions.
    let first = rig.llm.requests()[0].clone();
    let tools = first["tools"].to_string();
    assert!(tools.contains("Operates websites in a browser"), "{tools}");
    assert!(tools.contains("\"coder\""), "{tools}");

    // Both agents have their task before either has reported: they run at once. Each hears
    // only its request, in its request's trace.
    rig.until("both agents have their task", 5_000, |_| {
        !browser.hits().is_empty() && !coder.hits().is_empty()
    })
    .await;
    for (agent, words) in [
        (&browser, "Book a flight to Delhi on Friday"),
        (&coder, "Fix the failing test"),
    ] {
        let sent = agent.hits()[0].call.clone();
        assert_eq!(sent.body["request"], words);
        assert_eq!(sent.body["history"], json!([]));
        let traceparent = sent.headers["traceparent"].clone();
        assert!(
            traceparent.starts_with(&format!("00-{trace}-")) && traceparent.ends_with("-01"),
            "{traceparent}"
        );
    }
    rig.until_heard("Both are under way.").await;

    let id = |event: &Value| event["task_id"].as_str().expect("an id").to_owned();
    let ticket = json!({ "url": "https://air.example/t/AI101", "name": "e-ticket" });
    let booked = json!({ "type": "result", "text": "Booked AI 101 on Friday at 9:40.",
                         "data": { "flight": "AI 101" }, "artifacts": [ticket] });
    call_back(&id(flight), HOOK_SECRET, booked).expect("taken");
    // The result is published at once, whole, in its place in the tree.
    rig.until_event("task_output", 1).await;
    let output = rig.events("task_output")[0].clone();
    assert_eq!(output["task_id"], flight["task_id"]);
    assert_eq!(output["text"], "Booked AI 101 on Friday at 9:40.");
    assert_eq!(
        (&output["data"], &output["artifacts"][0]["name"]),
        (&json!({ "flight": "AI 101" }), &json!("e-ticket"))
    );
    assert_eq!(
        (&output["parent_id"], &output["agent"], &output["trace_id"]),
        (&request["task_id"], &json!("browser"), &json!(trace))
    );
    // It is said as an update (the front's line; the client has a screen), not in the
    // orchestrator's words.
    let flight_update = "Booked AI 101 on Friday at 9:40. The details are on your screen.";
    rig.until_heard(flight_update).await;
    let published = rig.time_of("the output", |event| event["type"] == "task_output");
    let said = rig.time_of("the update", |event| event["delta"] == flight_update);
    assert!(published < said, "{published} < {said}");
    let fixed = json!({ "type": "result", "text": "Fixed test_parse; 42 tests pass." });
    call_back(&id(test), HOOK_SECRET, fixed).expect("taken");
    // The narrator's prompt has the result sanitised ("test_parse" -> "test parse").
    rig.until_heard("Fixed test parse; 42 tests pass. The details are on your screen.")
        .await;
    rig.until_event("task_result", 3).await;
    assert_eq!(
        rig.events("task_output").len(),
        3,
        "both children and the request"
    );

    // The orchestrator heard each result as a later message of its own run.
    let last = rig.llm.requests().last().cloned().expect("a request");
    let users = ix_mocks::messages(&last, "user").join("\n");
    assert!(
        users.contains("task t2 on browser is done. Its result: Booked AI 101 on Friday at 9:40."),
        "{users}"
    );
    assert!(users.contains("task t3 on coder is done"), "{users}");
    // A child's own words are never said, nor the orchestrator's after a result: each result
    // is an update (the front's line).
    let not_updates: Vec<String> = rig
        .lines()
        .into_iter()
        .filter(|line| line["kind"] != "update")
        .map(|line| line["delta"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        not_updates
            .iter()
            .all(|line| !line.contains("AI 101") && !line.contains("9:40")),
        "{not_updates:?}"
    );
    let results = rig.events("task_result");
    let parent = results
        .iter()
        .find(|result| result["task_id"] == request["task_id"])
        .expect("the request's result");
    assert_eq!(parent["state"], "completed");
    assert_eq!(
        parent["summary"],
        "Both are under way. Your flight is booked for 9:40. And the test is fixed."
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrator_acts_on_its_own_request_and_not_on_the_callers_other_live_ones() {
    // Both agents take the task and never report: the first request stays live.
    let taken = |_: &Hit| (200, "[]".to_owned());
    let browser = BackendDouble::start(taken).await;
    let coder = BackendDouble::start(taken).await;
    let catalog = vec![
        agent_entry_of("browser", &browser.url, json!({})),
        agent_entry_of("coder", &coder.url, json!({})),
    ];
    let rig = orchestrated(catalog, &["browser", "coder"]).await;
    rig.llm.respond_with(|request| {
        let said = ix_mocks::messages(request, "user");
        let last = said.last().cloned().unwrap_or_default();
        let agent = if last.contains("flight") {
            "browser"
        } else {
            "coder"
        };
        Some(if ix_mocks::messages(request, "tool").is_empty() {
            Round {
                tool_calls: vec![(agent.to_owned(), json!({ "request": last }))],
                end_ms: 200,
                ..Round::default()
            }
        } else {
            Round::text(50, 20, &["ANSWER:\n", "That is under way."])
        })
    });
    rig.ready().await;
    rig.turn("book me a flight to Delhi on Friday").await;
    rig.until_event("task_created", 2).await;
    rig.turn("fix the failing test").await;
    rig.until_event("task_created", 4).await;

    // The second request's run was not shown the first, which is still running: live, a
    // model shown it booked the flight a second time.
    let second = (rig.llm.requests().into_iter())
        .find(|request| {
            ix_mocks::messages(request, "user")
                .last()
                .map(String::as_str)
                == Some("fix the failing test")
        })
        .expect("the second request's run");
    let said = ix_mocks::messages(&second, "user");
    assert!(said.iter().all(|text| !text.contains("flight")), "{said:?}");
    let created = rig.events("task_created");
    let agents: Vec<&Value> = created.iter().map(|event| &event["agent"]).collect();
    assert_eq!(
        agents,
        [
            &Value::Null,
            &json!("browser"),
            &Value::Null,
            &json!("coder")
        ]
    );
    assert_eq!(browser.hits().len(), 1, "the flight is booked once");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_failing_agent_does_not_affect_another_agent() {
    // One agent never answers; its fallback does. Both change nothing, so a failed task is
    // tried once more.
    let flaky = BackendDouble::start(|_: &Hit| (500, "{}".to_owned())).await;
    let done = json!({ "type": "result", "text": "Found three trains." }).to_string();
    let backup = BackendDouble::start(move |_: &Hit| (200, done.clone())).await;
    let catalog = vec![
        agent_entry_of(
            "flaky-finder",
            &flaky.url,
            json!({ "side_effects": "none", "fallback": "backup-finder" }),
        ),
        agent_entry_of(
            "backup-finder",
            &backup.url,
            json!({ "side_effects": "none" }),
        ),
    ];
    let rig = orchestrated(catalog, &["flaky-finder"]).await;
    rig.llm.respond_with(|request| {
        // The earlier requests' rounds are in the history of the later ones.
        let (tools, updates) = (
            ix_mocks::run_messages(request, "tool").len(),
            task_updates(request),
        );
        Some(match (tools, updates) {
            (0, _) => {
                let words = ix_mocks::messages(request, "user")
                    .pop()
                    .unwrap_or_default();
                Round::tool(300, "flaky-finder", json!({ "request": words }))
            }
            (_, 0) => Round::text(50, 20, &["ANSWER:\n", "I'm on it."]),
            _ => Round::text(50, 20, &["ANSWER:\n", "That's all I have."]),
        })
    });
    rig.ready().await;

    // Two tasks fail, each after a second try: the caller is told, and the entry's breaker
    // opens.
    for (n, request) in [
        "find me a train to Pune tomorrow",
        "look up the bus times to Goa",
    ]
    .into_iter()
    .enumerate()
    {
        rig.turn(request).await;
        rig.until("the failed task's end", 30_000, |rig| {
            let failed = |result: &&Value| result["state"] == "failed";
            rig.events("task_result").iter().filter(failed).count() > n
        })
        .await;
        rig.run(3_000).await;
    }
    assert_eq!(flaky.hits().len(), 4, "two tasks, each tried twice");
    assert!(
        rig.said()
            .iter()
            .any(|line| line == "Sorry, I couldn't get that done just now.")
    );
    assert!(remote::is_open("flaky-finder"));

    // The next task goes to the fallback, which is not touched by the other's failures.
    rig.turn("check the ferry to Mumbai on Sunday").await;
    rig.until("the fallback's result", 30_000, |rig| {
        let created = rig.events("task_created");
        created
            .iter()
            .any(|event| event["agent"] == "backup-finder")
            && rig
                .events("task_result")
                .iter()
                .any(|r| r["summary"] == "Found three trains.")
    })
    .await;
    assert_eq!(flaky.hits().len(), 4, "the failing agent gets no new task");
    assert_eq!(backup.hits().len(), 1);
    assert!(!remote::is_open("backup-finder"));
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_request_beyond_the_sessions_limit_waits_and_the_caller_hears_it_is_queued() {
    let interaction = json!({ "tasks": { "limits": { "max_live": 1 } } });
    let rig = Rig::start(Opts {
        interaction: Some(interaction),
        ..Opts::default()
    })
    .await;
    // Each answer takes a while.
    rig.llm.respond_with(|request| {
        let asked = ix_mocks::messages(request, "user")
            .pop()
            .unwrap_or_default();
        let answer = if asked.contains("weather") {
            "It is sunny."
        } else {
            "The shop opens at nine."
        };
        Some(Round::text(4_000, 20, &[answer]))
    });
    rig.ready().await;
    rig.turn("when does the shop open").await;
    rig.until_event("task_created", 1).await;
    rig.run(1_000).await;
    rig.turn("what is the weather like today").await;
    rig.until_event("task_created", 2).await;
    let created = rig.events("task_created");
    assert_eq!(created[1]["state"], "submitted");
    assert_eq!(rig.events("task_result").len(), 0, "nothing is refused");
    rig.until("the caller hears it is queued", 10_000, |rig| {
        let queued = |line: &String| line.starts_with("I'll get to that right after");
        rig.said().iter().any(queued)
    })
    .await;
    rig.until_heard("It is sunny.").await;
    let results = rig.events("task_result");
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result["state"] == "completed"));
}

/// An orchestrator session with one agent, `browser`, where the caller asks for a flight and
/// then a general question; the flight is booked while the long answer to the question plays.
/// `extra` is laid over the session's block.
async fn booked_while_answering(extra: Value) -> (Rig, BackendDouble, Value) {
    let (rig, browser, flight) = flight_while_answering(extra, None).await;
    let id = flight["task_id"].as_str().expect("an id");
    let booked = json!({ "type": "result", "text": "Booked AI 101 on Friday at 9:40." });
    call_back(id, HOOK_SECRET, booked).expect("taken");
    rig.until("the flight's output", 10_000, |rig| {
        let outputs = rig.events("task_output");
        outputs
            .iter()
            .any(|output| output["task_id"] == flight["task_id"])
    })
    .await;
    (rig, browser, flight)
}

/// The start of [`booked_while_answering`]: the flight's task runs on `browser` and the long
/// answer to the question has begun to play. `front`: a model front at that address.
async fn flight_while_answering(
    extra: Value,
    front: Option<String>,
) -> (Rig, BackendDouble, Value) {
    let taken = |_: &Hit| (200, "[]".to_owned());
    let browser = BackendDouble::start(taken).await;
    let catalog = vec![agent_entry_of("browser", &browser.url, json!({}))];
    let mut interaction = json!({ "tasks": { "orchestrator": "mock", "agents": ["browser"] },
                                  "hitl": { "channels": ["voice"] } });
    if let (Some(block), Some(extra)) = (interaction.as_object_mut(), extra.as_object()) {
        block.extend(extra.clone());
    }
    let rig = Rig::start(Opts {
        interaction: Some(interaction),
        catalog,
        front,
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        let said = ix_mocks::messages(request, "user");
        let last = said.last().cloned().unwrap_or_default();
        let tools = ix_mocks::messages(request, "tool").len();
        Some(if last.starts_with("Task update") {
            Round::text(50, 20, &["ANSWER:\n", "Your flight is booked."])
        } else if last.contains("capital") {
            Round::text(
                50,
                20,
                &[
                    "ANSWER:\n",
                    "The capital of Peru is Lima. ",
                    "It sits on the coast below the Andes, and it is the biggest city of the \
                     country by far, with about ten million people.",
                ],
            )
        } else if tools == 0 {
            let request = json!({ "request": "Book the morning flight to Delhi on Friday" });
            Round::tool(300, "browser", request)
        } else {
            Round::text(50, 20, &["ANSWER:\n", "The booking is under way."])
        })
    });
    rig.ready().await;
    rig.turn("book me the morning flight to Delhi on Friday")
        .await;
    rig.until_heard("The booking is under way.").await;
    rig.turn("what is the capital of Peru").await;
    rig.until("the answer plays", 30_000, |rig| {
        rig.said()
            .iter()
            .any(|line| line == "The capital of Peru is Lima.")
    })
    .await;
    let created = rig.events("task_created");
    let flight = created.iter().find(|event| event["agent"] == "browser");
    let flight = flight.cloned().expect("the flight's task");
    (rig, browser, flight)
}

const FLIGHT_UPDATE: &str = "Booked AI 101 on Friday at 9:40.";
const LONG_ANSWER: &str = "It sits on the coast below the Andes, and it is the biggest city of \
    the country by far, with about ten million people.";
const FLIGHT_OFFER: &str = "I've got the answer on your request to book the morning flight to Delhi on Friday. Want it now?";

#[tokio::test(start_paused = true)]
#[serial]
async fn an_update_waits_for_the_end_of_the_answer_then_is_offered() {
    let (rig, _browser, flight) = booked_while_answering(json!({})).await;
    // Published while the agent answers the question; the answer is not cut.
    assert!(!rig.heard_in_full(LONG_ANSWER), "{}", rig.timeline());
    rig.until_heard(FLIGHT_OFFER).await;
    assert!(rig.heard_in_full(LONG_ANSWER));
    let id_of = |words: &str| rig.line(words)["utterance_id"].clone();
    let (answer, offer) = (id_of(LONG_ANSWER), id_of(FLIGHT_OFFER));
    let at = |id: &Value, phase: &str| {
        rig.time_of(phase, |event| {
            event["type"] == "audio_segment"
                && event["utterance_id"] == *id
                && event["phase"] == phase
        })
    };
    let published = rig.time_of("the output", |event| {
        event["type"] == "task_output" && event["task_id"] == flight["task_id"]
    });
    assert!(published < at(&answer, "end"), "{}", rig.timeline());
    assert!(
        at(&offer, "start") >= at(&answer, "end"),
        "{}",
        rig.timeline()
    );
    assert_eq!(rig.line(FLIGHT_OFFER)["kind"], "update");
    assert_eq!(rig.line(FLIGHT_OFFER)["task_id"], flight["task_id"]);

    // A yes plays it: the front's line (no screen).
    rig.turn("yes please").await;
    let update = FLIGHT_UPDATE;
    rig.until_heard(update).await;
    assert_eq!(rig.line(update)["kind"], "update");
    // The orchestrator's words about it are not said, and the offer is made once.
    rig.run(10_000).await;
    let said = rig.said();
    assert!(
        !said.iter().any(|line| line == "Your flight is booked."),
        "{said:?}"
    );
    assert_eq!(said.iter().filter(|line| *line == FLIGHT_OFFER).count(), 1);
    // "Tell me more" finds the whole result in the call's history.
    let last = answering(&rig).last().cloned().expect("a request");
    let users = ix_mocks::messages(&last, "user").join("\n");
    assert!(
        !users.contains("Result of an earlier task"),
        "a later run's, not this one's"
    );
    rig.turn("tell me more about the flight").await;
    rig.until("a run for it", 10_000, |rig| {
        let last = answering(rig).last().cloned().unwrap_or_default();
        ix_mocks::messages(&last, "user")
            .last()
            .is_some_and(|said| said.contains("tell me more"))
    })
    .await;
    let last = answering(&rig).last().cloned().expect("a request");
    let users = ix_mocks::messages(&last, "user").join("\n");
    assert!(
        users.contains(
            "Book the morning flight to Delhi on Friday: Booked AI 101 on Friday at 9:40."
        ),
        "{users}"
    );
}

/// `tasks.announce` changed during the session (`interaction_update`): a result that comes
/// after it is kept for the screen, neither offered nor said.
#[tokio::test(start_paused = true)]
#[serial]
async fn how_updates_reach_the_caller_changes_during_the_session() {
    let (rig, _browser, flight) = flight_while_answering(json!({}), None).await;
    rig.send(Msg::Update(
        json!({ "tasks": { "announce": "screen", "default_backend": "x" } }),
    ))
    .await;
    rig.until("the change is applied", 5_000, |rig| {
        rig.events("interaction_updated")
            .iter()
            .any(|event| event["applied"] == json!(["tasks.announce"]))
    })
    .await;
    let updated = rig.events("interaction_updated");
    let updated = updated
        .iter()
        .rev()
        .find(|e| e["applied"] == json!(["tasks.announce"]));
    let updated = updated.expect("the change");
    assert_eq!(updated["state"]["updates"], "screen");
    assert_eq!(updated["refused"][0]["path"], "tasks.default_backend");
    let id = flight["task_id"].as_str().expect("an id");
    let booked = json!({ "type": "result", "text": "Booked AI 101 on Friday at 9:40." });
    call_back(id, HOOK_SECRET, booked).expect("taken");
    rig.until_heard(LONG_ANSWER).await;
    rig.run(15_000).await;
    let said = rig.said();
    assert!(
        said.iter()
            .all(|line| line != FLIGHT_OFFER && !line.contains("flight to Delhi")),
        "{said:?}"
    );
    assert!(
        rig.events("task_output")
            .iter()
            .any(|output| output["task_id"] == flight["task_id"]),
        "the result is still published for the screen"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_result_the_client_showed_is_not_said() {
    let (rig, _browser, flight) = booked_while_answering(json!({})).await;
    let task_id = flight["task_id"].as_str().expect("an id").to_owned();
    rig.send(Msg::TaskSeen { task_id }).await;
    rig.until_heard(LONG_ANSWER).await;
    rig.run(15_000).await;
    let said = rig.said();
    assert!(
        said.iter().all(|line| !line.contains("flight to Delhi")),
        "{said:?}"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn the_front_words_an_update_and_a_number_the_result_lacks_is_never_said() {
    let line = "Your Friday flight to Delhi is booked, AI 101 at 9:40.";
    let front = FrontDouble::start(FrontScript {
        reply: r#"delegate","say":"Let me check that."}"#.to_owned(),
        act_ms: 40,
        line_ms: 20,
        narration: Some(line.to_owned()),
        narration_ms: 150,
        ..FrontScript::default()
    })
    .await;
    let taken = |_: &Hit| (200, "[]".to_owned());
    let (browser, coder) = (
        BackendDouble::start(taken).await,
        BackendDouble::start(taken).await,
    );
    let catalog = vec![
        agent_entry_of("browser", &browser.url, json!({})),
        agent_entry_of("coder", &coder.url, json!({})),
    ];
    let interaction = json!({ "front": { "profile": "model" },
                              "tasks": { "orchestrator": "mock", "agents": ["browser", "coder"] },
                              "hitl": { "channels": ["voice"] } });
    let rig = Rig::start(Opts {
        interaction: Some(interaction),
        catalog,
        front: Some(front.url.clone()),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        let tools = ix_mocks::messages(request, "tool").len();
        Some(if tools == 0 {
            Round {
                tool_calls: vec![
                    (
                        "browser".to_owned(),
                        json!({ "request": "Book the flight to Delhi" }),
                    ),
                    (
                        "coder".to_owned(),
                        json!({ "request": "Fix the failing test" }),
                    ),
                ],
                end_ms: 300,
                ..Round::default()
            }
        } else {
            Round::text(50, 20, &["ANSWER:\n", "Both are under way."])
        })
    });
    rig.ready().await;
    rig.turn("book the flight to Delhi and fix the failing test")
        .await;
    rig.until_heard("Both are under way.").await;
    // An idle conversation: the update is said without an offer, in the front's words.
    rig.run(5_000).await;
    let created = rig.events("task_created");
    let task_of = |agent: &str| {
        let event = created
            .iter()
            .find(|event| event["agent"] == agent)
            .expect("a child");
        event["task_id"].as_str().expect("an id").to_owned()
    };
    let booked = json!({ "type": "result", "text": "Booked AI 101 on Friday at 9:40." });
    call_back(&task_of("browser"), HOOK_SECRET, booked).expect("taken");
    rig.until_heard(line).await;
    let asked = front.narrations();
    let given = asked[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        given.contains("Task: \u{ab}Book the flight to Delhi\u{bb}")
            && given.contains("Result: \u{ab}Booked AI 101 on Friday at 9:40.\u{bb}"),
        "{given}"
    );
    assert!(
        given.contains("Agent: \u{ab}Both are under way.\u{bb}"),
        "{given}"
    );

    // A line with a number the result does not have is never said; there is no template to
    // say instead (chapter 9): the update waits for its next slot, and is asked again.
    front.script(FrontScript {
        reply: r#"delegate","say":"Let me check that."}"#.to_owned(),
        narration: Some("Your test is fixed and 45 tests pass now.".to_owned()),
        narration_ms: 150,
        ..FrontScript::default()
    });
    rig.run(5_000).await;
    let fixed = json!({ "type": "result", "text": "Fixed it; 42 tests pass." });
    call_back(&task_of("coder"), HOOK_SECRET, fixed).expect("taken");
    rig.until("the narrator is asked again", 30_000, |_| {
        front.narrations().len() >= 3
    })
    .await;
    rig.run(5_000).await;
    assert!(
        rig.said().iter().all(|said| !said.contains("45")),
        "{:?}",
        rig.said()
    );
    assert!(
        rig.said()
            .iter()
            .all(|said| !said.contains("Your request to")),
        "no template: {:?}",
        rig.said()
    );
}

// ---- deadline questions, follow-ups, a hesitant request, resume keys, data scoping

/// Whether the question of the browser's was heard in full.
fn question_heard(rig: &Rig) -> bool {
    let lines = rig.lines();
    let asked: Vec<&Value> = lines
        .iter()
        .filter(|line| line["kind"] == "question")
        .collect();
    asked
        .iter()
        .any(|line| rig.heard(&line["utterance_id"]) == "full")
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_urgent_question_cuts_at_a_clause_and_the_answer_resumes() {
    let (rig, browser, flight) = flight_while_answering(json!({}), None).await;
    // The airline needs the seat within 7 s, while six seconds of the answer are still to
    // play: it cannot wait for the slot after it.
    let id = flight["task_id"].as_str().expect("an id");
    let seat = json!({ "type": "question", "request_id": "seat", "kind": "value",
                       "prompt": "Window or aisle?", "expires_in_ms": 7000 });
    call_back(id, HOOK_SECRET, seat).expect("taken");
    let asked_at = rig.now();
    // On the screen at once, with its deadline.
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["prompt"], "Window or aisle?");
    let left = question["expires_at_ms"].as_u64().unwrap() - question["t_ms"].as_u64().unwrap();
    assert_eq!(left, 7_000);
    // Its lead-in and its words go to synthesis while the answer still plays, before the cut
    // (live MA2: the question's synthesis began only at the cut).
    let cut = |rig: &Rig| {
        let stopped = rig.events("agent_audio_stopped");
        stopped.iter().any(|stopped| stopped["reason"] == "policy")
    };
    rig.until("the question is rendered ahead", 3_000, |_| {
        rig.tts
            .spoken()
            .iter()
            .any(|text| text == "Window or aisle?")
    })
    .await;
    assert!(!cut(&rig), "rendered before the cut: {}", rig.timeline());
    let spoken = rig.tts.spoken();
    let lead_in = spoken.iter().position(|text| text == "Quick question.");
    let asked = spoken.iter().position(|text| text == "Window or aisle?");
    assert!(lead_in.is_some() && lead_in < asked, "{spoken:?}");
    rig.until("the answer is cut for it", 3_000, cut).await;
    rig.until_heard("Quick question.").await;
    rig.until("the question is heard", 5_000, question_heard)
        .await;
    assert!(!rig.heard_in_full(LONG_ANSWER), "{}", rig.timeline());
    // The caller answers once it has reached their ear.
    rig.run(800).await;
    rig.turn("window").await;
    rig.until("the airline has the answer", 7_000, |_| {
        browser
            .hits()
            .iter()
            .any(|hit| hit.call.body["type"] == "answer")
    })
    .await;
    assert!(rig.now() < asked_at + 7_000, "in time: {}", rig.timeline());
    let answer = browser.hits().last().cloned().expect("a hit").call.body;
    assert_eq!(
        (&answer["outcome"], &answer["value"]),
        (&json!("accept"), &json!("window"))
    );
    // The interrupted answer resumes with "Anyway," and is heard in full.
    rig.until_heard("Anyway,").await;
    rig.until_heard(LONG_ANSWER).await;
    let anyway = rig.time_of("anyway", |event| event["delta"] == "Anyway,");
    let resumed = rig
        .wire()
        .into_iter()
        .filter(|(_, event)| event["delta"] == LONG_ANSWER);
    let resumed = resumed
        .map(|(at, _)| at)
        .max()
        .expect("the rest of the answer");
    assert!(anyway < resumed, "{}", rig.timeline());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_follow_up_action_continues_the_same_agent_context() {
    let (rig, browser, flight) = booked_while_answering(json!({})).await;
    // The offer is not taken: the update is said once the call is idle.
    rig.until_heard(FLIGHT_OFFER).await;
    rig.until_heard(FLIGHT_UPDATE).await;
    // About the finished flight (the update the caller heard last): a request of its own
    // on the same agent, with its handle.
    rig.turn("can it be a window seat").await;
    rig.until("the browser has the follow-up", 10_000, |_| {
        browser
            .hits()
            .iter()
            .any(|hit| hit.call.body["follows"].is_string())
    })
    .await;
    let sent = browser.hits().last().cloned().expect("a hit").call.body;
    assert_eq!(sent["request"], "can it be a window seat");
    assert_eq!(sent["follows"], flight["task_id"]);
    assert_eq!(sent["history"], json!([]));
    let created = rig.events("task_created");
    let follow = created.last().cloned().expect("a task");
    assert_eq!(
        (&follow["agent"], &follow["parent_id"], &follow["follows"]),
        (&json!("browser"), &Value::Null, &flight["task_id"])
    );
    // Its answer is the caller's: said as an answer is.
    let id = follow["task_id"].as_str().expect("an id");
    let back = json!({ "type": "result", "text": "Your seat is now 14A, by the window." });
    call_back(id, HOOK_SECRET, back).expect("taken");
    rig.until_heard("Your seat is now 14A, by the window.")
        .await;
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_follow_up_question_is_answered_from_the_stored_result() {
    // A model front that reads the turn: the question about the flight is its follow-up.
    let front = FrontDouble::start(FrontScript {
        decide: Some(ix_mocks::reads_the_turn),
        answer: Some("It leaves on Friday at 9:40.".to_owned()),
        ..FrontScript::default()
    })
    .await;
    let (rig, _browser, flight) = flight_while_answering(json!({}), Some(front.url.clone())).await;
    let id = flight["task_id"].as_str().expect("an id");
    let booked = json!({ "type": "result", "text": "Booked AI 101 on Friday at 9:40." });
    call_back(id, HOOK_SECRET, booked).expect("taken");
    rig.until_heard(FLIGHT_OFFER).await;
    rig.until_heard(FLIGHT_UPDATE).await;
    let tasks_before = rig.events("task_created").len();
    rig.turn("when does the flight leave").await;
    rig.until_heard("It leaves on Friday at 9:40.").await;
    rig.run(3_000).await;
    assert_eq!(
        rig.events("task_created").len(),
        tasks_before,
        "no new task"
    );
    assert_eq!(rig.line("It leaves on Friday at 9:40.")["kind"], "answer");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn the_line_said_while_the_front_answers_from_a_result_is_primed_after_the_answer() {
    // A front server with a slot for the interims: the answer from the stored result and the
    // priming of the line said meanwhile share it. Primed first, the whole prompt was read
    // again ahead of the answer, which missed its budget (live MA3, 4 of 4).
    let front = FrontDouble::start(FrontScript {
        decide: Some(ix_mocks::reads_the_turn),
        answer: Some("It leaves on Friday at 9:40.".to_owned()),
        answer_ms: 800,
        slots: 2,
        ..FrontScript::default()
    })
    .await;
    let (rig, _browser, flight) = flight_while_answering(json!({}), Some(front.url.clone())).await;
    let id = flight["task_id"].as_str().expect("an id");
    let booked = json!({ "type": "result", "text": "Booked AI 101 on Friday at 9:40." });
    call_back(id, HOOK_SECRET, booked).expect("taken");
    rig.until_heard(FLIGHT_OFFER).await;
    rig.until_heard(FLIGHT_UPDATE).await;
    let before = front.requests();
    rig.turn("when does the flight leave").await;
    rig.until_heard("It leaves on Friday at 9:40.").await;
    rig.run(2_000).await;
    // A priming: the decision's prompt with nothing to generate.
    let primed = |request: &Value| request["max_tokens"] == 1 && request["stream"] == false;
    let during = front.during_answer();
    assert!(
        !during.iter().any(primed),
        "a line was primed while the answer was worded: {during:?}"
    );
    // The opener said meanwhile is primed once the answer is back.
    let bodies = front.bodies();
    let asked = |request: &Value| {
        (request["messages"][0]["content"].as_str())
            .is_some_and(|system| system.starts_with("You answer a caller's question"))
    };
    let answer = (bodies.iter().skip(before).position(asked)).expect("the answer was asked");
    assert!(
        bodies.iter().skip(before + answer + 1).any(primed),
        "the line is primed after the answer"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_caller_with_a_resume_key_gets_a_pending_update_on_a_new_stream_id() {
    let block = json!({ "tasks": { "resume_key": "caller-7-device-a", "reconnect_grace_s": 60 } });
    let rig = Rig::start(Opts {
        interaction: Some(block.clone()),
        ..Opts::default()
    })
    .await;
    rig.llm.push(answer(8_000));
    rig.ready().await;
    rig.turn("what time does the store open").await;
    rig.until_event("task_created", 1).await;
    let task_id = rig.events("task_created")[0]["task_id"].clone();
    // The caller hangs up, and calls back from another device: another stream_id.
    rig.handle.shutdown().await;
    let back = Rig::start(Opts {
        interaction: Some(block),
        back: Some((rig.owner.clone(), "stream-after-the-call-back".to_owned())),
        ..Opts::default()
    })
    .await;
    back.ready().await;
    back.until_event("task_result", 1).await;
    back.until_heard(OPENS).await;
    let result = &back.events("task_result")[0];
    assert_eq!(
        (&result["task_id"], &result["state"]),
        (&task_id, &json!("completed"))
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_value_given_on_the_screen_goes_only_to_the_agent_that_asked() {
    let passport = "P7654321";
    let form = json!({ "type": "question", "request_id": "f1", "kind": "form",
        "prompt": "Your passport details?",
        "schema": { "type": "object", "properties": { "passport_number": { "type": "string" } } } });
    let browser = BackendDouble::start(move |hit: &Hit| {
        let reply = match hit.call.body["type"].as_str() {
            Some("task") => form.clone(),
            _ => json!([]),
        };
        (200, reply.to_string())
    })
    .await;
    let coder = BackendDouble::start(|_: &Hit| (200, "[]".to_owned())).await;
    let catalog = vec![
        agent_entry_of("browser", &browser.url, json!({})),
        agent_entry_of("coder", &coder.url, json!({})),
    ];
    let rig = orchestrated(catalog, &["browser", "coder"]).await;
    rig.llm.respond_with(|request| {
        let tools = ix_mocks::messages(request, "tool").len();
        Some(if tools == 0 {
            Round {
                tool_calls: vec![
                    (
                        "browser".to_owned(),
                        json!({ "request": "Book the flight" }),
                    ),
                    (
                        "coder".to_owned(),
                        json!({ "request": "Fix the failing test" }),
                    ),
                ],
                end_ms: 300,
                ..Round::default()
            }
        } else {
            Round::text(50, 20, &["ANSWER:\n", "Both are under way."])
        })
    });
    rig.ready().await;
    rig.turn("book the flight and fix the failing test").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    let request_id = question["request_id"].as_str().expect("an id").to_owned();
    let value = Some(json!({ "passport_number": passport }));
    rig.send(Msg::TaskAnswer {
        request_id,
        outcome: HitlOutcome::Accept { value },
    })
    .await;
    rig.until("the browser has the form", 5_000, |_| {
        browser
            .hits()
            .iter()
            .any(|hit| hit.call.body["type"] == "answer")
    })
    .await;
    rig.run(3_000).await;
    let answer = browser.hits().last().cloned().expect("a hit").call.body;
    assert_eq!(answer["value"]["passport_number"], passport);
    // Nobody else hears it: not the other agent, not the orchestrator.
    assert!(
        coder
            .hits()
            .iter()
            .all(|hit| !hit.call.raw.contains(passport))
    );
    assert!(
        rig.llm
            .requests()
            .iter()
            .all(|request| !request.to_string().contains(passport))
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_hesitant_request_stays_one_task() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm
        .respond_with(|_| Some(Round::text(4_000, 20, &["Here are three flats."])));
    rig.ready().await;
    rig.turn("I need a pet friendly apartment in Miami").await;
    rig.until_event("task_created", 1).await;
    // The caller goes on at once, after a pause the recogniser took for the end.
    rig.turn("and my budget is about fifteen hundred a month")
        .await;
    rig.until_heard("Here are three flats.").await;
    assert_eq!(rig.events("task_created").len(), 1, "{}", rig.timeline());
    let last = rig.llm.requests().last().cloned().expect("a request");
    let users = ix_mocks::messages(&last, "user").join("\n");
    assert!(
        users.contains("I need a pet friendly apartment in Miami and my budget is about fifteen hundred a month"),
        "{users}"
    );
}

/// Owner, 26 Oct (live PR2, PR8): words the front delegates right after a request started
/// are read apart against it; about it, the running task takes them and no second one
/// starts. A new request then is a task of its own.
#[tokio::test(start_paused = true)]
#[serial]
async fn words_about_a_request_just_started_update_it_and_a_new_one_is_its_own_task() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.respond_with(|request| {
        let users = ix_mocks::messages(request, "user").join(" ");
        Some(if users.contains("peru") {
            Round::text(300, 20, &["Lima is the capital of Peru."])
        } else {
            Round::text(4_000, 20, &["Your parcel arrives tomorrow."])
        })
    });
    rig.ready().await;
    rig.turn("where is my parcel").await;
    rig.until_event("task_created", 1).await;
    rig.run(2_000).await;
    rig.turn("can you check its delivery status?").await;
    rig.until_heard("Your parcel arrives tomorrow.").await;
    assert_eq!(rig.events("task_created").len(), 1, "{}", rig.timeline());
    // The running task took the words: its second revision.
    let revised = rig.events("task_progress");
    assert!(
        revised.iter().any(|event| event["revision"] == 2),
        "{}",
        rig.timeline()
    );
    let last = rig.llm.requests().last().cloned().expect("a request");
    let users = ix_mocks::messages(&last, "user").join("\n");
    assert!(users.contains("delivery status"), "{users}");
    rig.turn("what's the capital of peru").await;
    rig.until_heard("Lima is the capital of Peru.").await;
    assert_eq!(rig.events("task_created").len(), 2, "{}", rig.timeline());
}

/// The 2B front's CT1 misreading: every ask to hear something again is the number. Test
/// data, never a gateway path.
fn every_again_is_the_number(observation: &str) -> String {
    let user = observation
        .lines()
        .find(|line| line.starts_with("USER"))
        .unwrap_or_default();
    if user.contains("again") {
        return r#"control","kind":"repeat","value":"number","say":""}"#.to_owned();
    }
    ix_mocks::reads_the_turn(observation)
}

/// Owner, 26 Oct (live CT1): a repeat of the number the front names while the line heard
/// holds a number is read apart: a plain "say that again" hears the whole line again, an ask
/// for the number only the number.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_repeat_the_front_named_the_number_of_is_read_apart_from_the_whole_line() {
    let front = FrontDouble::start(FrontScript {
        decide: Some(every_again_is_the_number),
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(Opts {
        front: Some(front.url.clone()),
        ..Opts::default()
    })
    .await;
    let line = "Your code is 4 4 1 7, valid today.";
    rig.llm
        .respond_with(move |_| Some(Round::text(300, 20, &[line])));
    rig.ready().await;
    rig.turn("what is my code").await;
    rig.until_heard(line).await;
    rig.run(1_000).await;
    rig.turn("sorry, say that again").await;
    rig.until("the line again", 10_000, |rig| {
        rig.said().iter().filter(|said| *said == line).count() == 2
    })
    .await;
    rig.run(3_000).await;
    rig.turn("what was the number again?").await;
    rig.until_heard("4 4 1 7").await;
    let kinds: Vec<String> = controls_applied(&rig)
        .into_iter()
        .map(|c| format!("{}:{}", c.0, c.1))
        .collect();
    assert_eq!(kinds, ["repeat:", "repeat:number"]);
    let readings = front.bodies().into_iter().filter(|body| {
        let system = body["messages"][0]["content"].as_str().unwrap_or_default();
        system.starts_with("On a phone call the agent just said a line with a number")
    });
    assert_eq!(readings.count(), 2);
}

// ---------------------------------------------------------------------------------------------
// A standalone agent: what A2A agents and MCP servers say of themselves
// ---------------------------------------------------------------------------------------------

/// Server-sent events of these JSON-RPC results, as one reply.
fn rpc_events(results: &[Value]) -> (u16, String) {
    let event = |result: &Value| {
        let reply = json!({ "jsonrpc": "2.0", "id": "1", "result": result });
        format!("data: {reply}\n\n")
    };
    (200, results.iter().map(event).collect())
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrator_calls_an_mcp_servers_tools_and_the_caller_confirms_the_destructive_one() {
    let server = BackendDouble::start(|hit: &Hit| {
        let params = &hit.call.body["params"];
        let text = |text: &str| json!({ "content": [{ "type": "text", "text": text }] });
        match hit.call.body["method"].as_str() {
            Some("tools/list") => rpc_result(json!({ "tools": [
                { "name": "search_notes", "description": "Searches the caller's notes for words.",
                  "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } },
                                   "required": ["query"] },
                  "annotations": { "readOnlyHint": true } },
                { "name": "delete_note", "description": "Deletes one of the caller's notes by its title.",
                  "inputSchema": { "type": "object", "properties": { "title": { "type": "string" } },
                                   "required": ["title"] },
                  "annotations": { "destructiveHint": true } },
                { "name": "add_reminder", "description": "Adds a reminder.",
                  "inputSchema": { "type": "object", "required": ["what", "day", "time"],
                    "properties": { "what": { "type": "string" }, "day": { "type": "string" },
                                    "time": { "type": "string" }, "priority": { "type": "string" } } },
                  "annotations": { "readOnlyHint": false, "destructiveHint": false } },
                { "name": "not_allowed", "description": "Not for the orchestrator." },
            ] })),
            Some("tools/call") => match params["name"].as_str() {
                Some("search_notes") => rpc_result(text("One note mentions the dentist.")),
                Some("delete_note") => rpc_result(text("Deleted the note.")),
                // Its progress comes on the reply, before its result.
                _ => {
                    let token = params["_meta"]["progressToken"].clone();
                    let progress = json!({ "jsonrpc": "2.0", "method": "notifications/progress",
                        "params": { "progressToken": token, "progress": 1, "total": 2,
                                    "message": "Saving the reminder." } });
                    let done = json!({ "jsonrpc": "2.0", "id": "1",
                                       "result": text("The reminder is set.") });
                    (200, format!("data: {progress}\n\ndata: {done}\n\n"))
                }
            },
            _ => (404, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "notes", "type": "mcp", "url": server.url,
                        "tools": ["search_notes", "delete_note", "add_reminder"] });
    let rig = orchestrated(vec![backend(entry, None)], &["notes"]).await;
    let reminder =
        json!({ "what": "call mom", "day": "Friday", "time": "6 pm", "priority": "high" });
    let asked = reminder.clone();
    rig.llm.respond_with(move |request| {
        let (tools, updates) = (
            ix_mocks::messages(request, "tool").len(),
            task_updates(request),
        );
        Some(match (tools, updates) {
            (0, _) => Round {
                tool_calls: vec![
                    ("search_notes".to_owned(), json!({ "query": "dentist" })),
                    ("delete_note".to_owned(), json!({ "title": "old list" })),
                    ("add_reminder".to_owned(), asked.clone()),
                ],
                end_ms: 400,
                ..Round::default()
            },
            (_, 0) => Round::text(50, 20, &["ANSWER:\n", "On it."]),
            _ => Round::text(50, 20, &["ANSWER:\n", "Done."]),
        })
    });
    rig.ready().await;
    rig.turn("find my dentist note, delete the old list and remind me to call mom")
        .await;

    // The server's tools are the orchestrator's, as the server describes them; the one the
    // entry does not allow is not.
    rig.until_event("task_question", 1).await;
    let first = rig.llm.requests()[0].clone();
    let tools = first["tools"].to_string();
    assert!(
        tools.contains("Deletes one of the caller's notes by its title."),
        "{tools}"
    );
    assert!(
        tools.contains("\"priority\""),
        "the server's input schema: {tools}"
    );
    assert!(!tools.contains("not_allowed"), "{tools}");
    // The destructive call is read back from its exact arguments and nothing is sent yet.
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "confirm");
    assert_eq!(
        question["prompt"],
        "To confirm: delete note, old list. Do you want me to do that?"
    );
    let called = |name: &str| {
        let calls = server.bodies().into_iter();
        let mut calls = calls.filter(|body| body["method"] == "tools/call");
        calls.find(|body| body["params"]["name"] == name)
    };
    assert!(called("delete_note").is_none());
    // The read-only tool was not asked about: it went out at once.
    let search = called("search_notes").expect("the search went out");
    assert_eq!(search["params"]["arguments"], json!({ "query": "dentist" }));

    rig.answer(&question, "yes").await;
    rig.until("the three calls reached the server", 5_000, |_| {
        called("delete_note").is_some() && called("add_reminder").is_some()
    })
    .await;
    let delete = called("delete_note").expect("sent after the yes");
    assert_eq!(
        delete["params"]["arguments"],
        json!({ "title": "old list" })
    );
    let add = called("add_reminder").expect("sent");
    assert_eq!(add["params"]["arguments"], reminder);

    // Each call is a child task of the request, on the server's entry, in its trace.
    rig.until_event("task_result", 4).await;
    let created = rig.events("task_created");
    let request = &created[0];
    for (child, (agent, changes)) in created[1..].iter().zip([
        ("search_notes", false),
        ("delete_note", true),
        ("add_reminder", true),
    ]) {
        assert_eq!(child["agent"], agent, "{}", rig.timeline());
        assert_eq!(child["parent_id"], request["task_id"]);
        assert_eq!(child["trace_id"], request["trace_id"]);
        assert_eq!(child["background"], "mcp:notes");
        assert_eq!(child["side_effects"], changes, "{agent}");
    }
    // The reminder's progress notification is its task's progress; what a call that changes
    // things did is known.
    let progress = rig.events("task_progress");
    assert!(
        progress
            .iter()
            .any(|event| event["text"] == "Saving the reminder."),
        "{}",
        rig.timeline()
    );
    let results = rig.events("task_result");
    let deleted = results
        .iter()
        .find(|result| result["task_id"] == created[2]["task_id"]);
    let deleted = deleted.expect("the delete's result");
    assert_eq!(deleted["state"], "completed");
    assert_ne!(deleted["effects"], "unknown", "{deleted}");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrator_refused_a_destructive_call_never_sends_it() {
    let server = BackendDouble::start(|hit: &Hit| match hit.call.body["method"].as_str() {
        Some("tools/list") => rpc_result(json!({ "tools": [
            { "name": "delete_note", "description": "Deletes a note.",
              "inputSchema": { "type": "object", "properties": { "title": { "type": "string" } } } },
            { "name": "archive_note", "description": "Archives a note.",
              "inputSchema": { "type": "object", "properties": { "title": { "type": "string" } } } },
        ] })),
        _ => rpc_result(json!({ "content": [{ "type": "text", "text": "Done." }] })),
    })
    .await;
    // The operator says archiving needs no confirmation, whatever the server says.
    let entry = json!({ "name": "notes", "type": "mcp", "url": server.url, "tools": ["*"],
                        "confirm": { "archive_note": false } });
    let rig = orchestrated(vec![backend(entry, None)], &["notes"]).await;
    rig.llm.respond_with(|request| {
        Some(if ix_mocks::messages(request, "tool").is_empty() {
            Round {
                tool_calls: vec![
                    ("delete_note".to_owned(), json!({ "title": "taxes" })),
                    ("archive_note".to_owned(), json!({ "title": "taxes" })),
                ],
                end_ms: 400,
                ..Round::default()
            }
        } else {
            Round::text(50, 20, &["ANSWER:\n", "Archived, not deleted."])
        })
    });
    rig.ready().await;
    rig.turn("get rid of my taxes note").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    rig.send(Msg::TaskAnswer {
        request_id: question["request_id"].as_str().expect("an id").to_owned(),
        outcome: HitlOutcome::Decline { correction: None },
    })
    .await;
    rig.until_heard("Archived, not deleted.").await;
    let names: Vec<Value> = server
        .bodies()
        .into_iter()
        .filter(|body| body["method"] == "tools/call")
        .map(|body| body["params"]["name"].clone())
        .collect();
    assert_eq!(names, [json!("archive_note")], "{}", rig.timeline());
    // The orchestrator was told the call was declined.
    let last = rig.llm.requests().last().cloned().expect("a request");
    assert!(last.to_string().contains("declined"), "{last}");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrators_a2a_agent_is_described_by_its_card_and_streams_its_task() {
    let agent = BackendDouble::start(|hit: &Hit| {
        if hit.method == "GET" {
            let card = json!({
                "name": "Trips", "description": "Plans trips.",
                "skills": [{ "id": "hotels", "name": "Hotels", "description": "Finds and books hotels." }],
                "capabilities": { "streaming": true },
                "securitySchemes": { "key": { "type": "apiKey", "in": "header", "name": "X-Agent-Key" } },
            });
            return if hit.path.ends_with("/.well-known/agent-card.json") {
                (200, card.to_string())
            } else {
                (404, "{}".to_owned())
            };
        }
        let message = |text: &str| json!({ "role": "ROLE_AGENT", "parts": [{ "text": text }] });
        let status = |state: &str, text: &str| {
            json!({ "statusUpdate": { "taskId": "tk-1", "contextId": "ctx-1",
                    "status": { "state": state, "message": message(text) } } })
        };
        let params = &hit.call.body["params"];
        match hit.call.body["method"].as_str() {
            Some("SendStreamingMessage") if params["message"]["taskId"] == "tk-1" => rpc_events(&[
                status("TASK_STATE_WORKING", "Booking it."),
                json!({ "artifactUpdate": { "taskId": "tk-1", "artifact": { "artifactId": "a1",
                        "parts": [{ "text": "Booked the Sea View in Goa," }] } } }),
                json!({ "artifactUpdate": { "taskId": "tk-1", "append": true, "artifact": {
                        "artifactId": "a1", "parts": [{ "text": "two nights." }] } } }),
                json!({ "statusUpdate": { "taskId": "tk-1", "contextId": "ctx-1", "final": true,
                        "status": { "state": "TASK_STATE_COMPLETED" } } }),
            ]),
            Some("SendStreamingMessage") => rpc_events(&[
                json!({ "task": { "id": "tk-1", "contextId": "ctx-1",
                                  "status": { "state": "TASK_STATE_SUBMITTED" } } }),
                status("TASK_STATE_WORKING", "Looking at hotels."),
                status("TASK_STATE_INPUT_REQUIRED", "What is your budget per night?"),
            ]),
            _ => (404, "{}".to_owned()),
        }
    })
    .await;
    // No description: the card's is the tool's. Nothing it does changes things outside it.
    let entry = json!({ "name": "trips", "type": "a2a", "url": agent.url, "bearer_env": "X",
                        "side_effects": "none" });
    let rig = orchestrated(vec![backend(entry, Some("agent-key"))], &["trips"]).await;
    rig.llm.respond_with(|request| {
        let (tools, updates) = (
            ix_mocks::messages(request, "tool").len(),
            task_updates(request),
        );
        Some(match (tools, updates) {
            (0, _) => Round::tool(400, "trips", json!({ "request": "Find a hotel in Goa" })),
            // The same call again (live, gpt-oss-20b did): the same child.
            (1, 0) => Round::tool(400, "trips", json!({ "request": "Find a hotel in Goa" })),
            (_, 0) => Round::text(50, 20, &["ANSWER:\n", "I've asked the trips agent."]),
            _ => Round::text(50, 20, &["ANSWER:\n", "Your hotel is booked."]),
        })
    });
    rig.ready().await;
    rig.turn("find me a hotel in goa").await;

    rig.until_event("task_question", 1).await;
    let first = answering(&rig)[0].clone();
    let tools = first["tools"].to_string();
    assert!(
        tools.contains("Plans trips. Skills: Hotels: Finds and books hotels."),
        "{tools}"
    );
    // The card says it streams and takes its key in a header of its own.
    let sent = agent.hits().into_iter().find(|hit| hit.method == "POST");
    let sent = sent.expect("the task went out").call;
    assert_eq!(sent.body["method"], "SendStreamingMessage");
    assert_eq!(sent.headers["x-agent-key"], "agent-key");
    assert!(!sent.headers.contains_key("authorization"));
    // Its progress and its question came as they streamed.
    let progress = rig.events("task_progress");
    assert!(
        progress
            .iter()
            .any(|event| event["text"] == "Looking at hotels."),
        "{}",
        rig.timeline()
    );
    let question = rig.events("task_question")[0].clone();
    assert_eq!(question["kind"], "value");
    assert_eq!(question["prompt"], "What is your budget per night?");
    rig.answer(&question, "two hundred dollars").await;

    rig.until_event("task_output", 1).await;
    let answered = agent
        .bodies()
        .into_iter()
        .find(|body| body["params"]["message"]["taskId"] == "tk-1");
    let answered = answered.expect("the answer went to the task");
    assert_eq!(
        answered["params"]["message"]["parts"][0]["text"],
        "two hundred dollars"
    );
    let output = rig.events("task_output")[0].clone();
    assert_eq!(output["text"], "Booked the Sea View in Goa, two nights.");
    let children = rig.events("task_created");
    let children: Vec<&Value> = children.iter().filter(|e| e["agent"] == "trips").collect();
    assert_eq!(children.len(), 1, "{}", rig.timeline());
    rig.until("the orchestrator hears the result", 10_000, |rig| {
        answering(rig).len() > 2
    })
    .await;
    let tool_results = ix_mocks::messages(&answering(&rig)[2], "tool");
    assert_eq!(tool_results.len(), 2);
    assert_eq!(tool_results[0], tool_results[1], "the same task_ref twice");
    assert_eq!(output["agent"], "trips");
    // The orchestrator heard the child's result; the caller hears it as an update.
    rig.until_heard("Booked the Sea View in Goa, two nights. The details are on your screen.")
        .await;
    let last = answering(&rig).last().cloned().expect("a request");
    let users = ix_mocks::messages(&last, "user").join("\n");
    assert!(
        users.contains("Booked the Sea View in Goa, two nights."),
        "{users}"
    );
}

// ---- conversation controls (plan chapter 9)

/// The client events of a control, in order: (kind, value, source, outcome).
/// Whether a request is an orchestrator's control round (r7-fix: one beside the first round of
/// each orchestrator run).
fn control_round(request: &Value) -> bool {
    (ix_mocks::messages(request, "user").last())
        .is_some_and(|said| said.contains("Only check whether your instructions"))
}

/// The orchestrator's requests without its control rounds.
fn answering(rig: &Rig) -> Vec<Value> {
    let requests = rig.llm.requests().into_iter();
    requests.filter(|request| !control_round(request)).collect()
}

fn controls_applied(rig: &Rig) -> Vec<(String, String, String, String)> {
    let field = |event: &Value, key: &str| event[key].as_str().unwrap_or_default().to_owned();
    rig.events("control_applied")
        .iter()
        .map(|e| {
            (
                field(e, "kind"),
                field(e, "value"),
                field(e, "source"),
                field(e, "outcome"),
            )
        })
        .collect()
}

/// The state of the latest `interaction_updated`.
fn latest_state(rig: &Rig) -> Value {
    let updated = rig.events("interaction_updated");
    updated
        .last()
        .map_or(Value::Null, |event| event["state"].clone())
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_spoken_rate_control_changes_the_rate_within_the_bounds_and_reports_the_state() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    rig.turn("a bit slower please").await;
    rig.until_event("control_applied", 1).await;
    let applied = rig.events("control_applied")[0].clone();
    assert_eq!(
        controls_applied(&rig)[0],
        (
            "rate".into(),
            "slower".into(),
            "caller".into(),
            "applied".into()
        )
    );
    assert_eq!(applied["state_version"], 1);
    rig.until("the rate is set", 2_000, |rig| rig.tts.rates() == [0.85])
        .await;
    let state = latest_state(&rig);
    assert_eq!(
        (state["rate"].as_f64(), &state["version"]),
        (Some(0.85f32 as f64), &json!(1))
    );
    assert!(
        rig.events("task_created").is_empty(),
        "a control is never a task"
    );
    // Down to the operator's bound (0.7), then said honestly.
    rig.run(1_000).await;
    rig.turn("slower please").await;
    rig.until_event("control_applied", 2).await;
    rig.run(1_000).await;
    rig.turn("even slower please").await;
    rig.until_event("control_applied", 3).await;
    assert_eq!(controls_applied(&rig)[2].3, "unsupported");
    rig.until("the honest line", 5_000, |rig| {
        rig.said()
            .iter()
            .any(|line| line.contains("Sorry, I can't change my speed"))
    })
    .await;
    assert_eq!(rig.tts.rates(), [0.85, 0.7]);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn repeat_replays_the_last_heard_line_and_repeat_the_number_says_only_the_number() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.respond_with(|_| {
        Some(Round::text(
            300,
            20,
            &["Your code is 4 4 1 7, valid today."],
        ))
    });
    rig.ready().await;
    rig.turn("what is my code").await;
    rig.until_heard("Your code is 4 4 1 7, valid today.").await;
    rig.run(1_000).await;
    rig.turn("sorry, say that again").await;
    rig.until("the line again", 10_000, |rig| {
        rig.said()
            .iter()
            .filter(|line| *line == "Your code is 4 4 1 7, valid today.")
            .count()
            == 2
    })
    .await;
    rig.run(3_000).await;
    rig.turn("what was the number?").await;
    rig.until_heard("4 4 1 7").await;
    let kinds: Vec<String> = controls_applied(&rig)
        .into_iter()
        .map(|c| format!("{}:{}", c.0, c.1))
        .collect();
    assert_eq!(kinds, ["repeat:", "repeat:number"]);
    assert_eq!(
        rig.events("task_created").len(),
        1,
        "only the request was a task"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_stop_cuts_and_the_history_has_cut_markers_and_control_notes() {
    let rig = Rig::start(Opts::default()).await;
    let long = "The store opens at nine in the morning on weekdays and stays open late.";
    rig.llm.respond_with(move |request| {
        let users = ix_mocks::messages(request, "user").join(" ");
        Some(if users.contains("close") {
            answer(300)
        } else {
            Round::text(300, 20, &[long])
        })
    });
    rig.ready().await;
    rig.turn("when does the store open").await;
    rig.until("the answer plays", 10_000, |rig| {
        rig.said().contains(&long.to_owned())
    })
    .await;
    rig.run(600).await;
    // Over the agent: the front reads "stop talking" in context, on the interim result.
    rig.onset().await;
    rig.stt.interim("stop talking").await;
    rig.until_event("agent_audio_stopped", 1).await;
    assert_eq!(rig.events("agent_audio_stopped")[0]["reason"], "barge_in");
    rig.run(500).await;
    rig.offset(500).await;
    rig.stt.speech_final("stop talking").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].0, "stop");
    assert!(rig.events("task_created").len() == 1);
    // The next request's history: what the caller heard, where they cut it, the control.
    rig.run(1_000).await;
    rig.turn("what time does it close").await;
    rig.until_heard("It closes at six.").await;
    let requests = rig.llm.requests();
    let last = &requests[requests.len() - 1];
    let assistant = ix_mocks::messages(last, "assistant");
    let users = ix_mocks::messages(last, "user");
    assert!(
        assistant
            .iter()
            .any(|line| line.ends_with("[cut off by the caller]")
                && long.starts_with(line.trim_end_matches(" [cut off by the caller]"))),
        "{assistant:?}"
    );
    assert!(
        !assistant.iter().any(|line| line == long),
        "the unheard rest is left out"
    );
    assert!(
        users
            .iter()
            .any(|line| line == "[the caller asked the agent to stop talking]"),
        "{users:?}"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn after_a_spoken_stop_over_a_client_line_the_next_client_line_plays() {
    let rig = Rig::start(Opts::default()).await;
    let long = "This is a long notice that keeps on going for a good while so it can be cut.";
    rig.ready().await;
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.until("the line plays", 10_000, |rig| {
        rig.said().contains(&long.to_owned())
    })
    .await;
    rig.run(600).await;
    rig.onset().await;
    rig.stt.interim("stop talking").await;
    rig.until_event("agent_audio_stopped", 1).await;
    rig.run(500).await;
    rig.offset(500).await;
    rig.smart_turn(0.9).await;
    rig.stt.speech_final("stop talking").await;
    rig.until_event("control_applied", 1).await;
    rig.run(2_000).await;
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.until("the next client line plays", 10_000, |rig| {
        rig.said().iter().filter(|line| *line == long).count() == 2
    })
    .await;
}

/// Over the agent an interim takes the floor only on a sure reading (live L6: the interim
/// "aha" of an "uh-huh" was named `control` and cut the agent): a control without a stop cue
/// is read on to its kind, and only a stop cuts; any other act cuts once the next
/// interim names it too.
#[tokio::test(start_paused = true)]
#[serial]
async fn over_the_agent_only_a_sure_reading_of_an_interim_cuts() {
    let rig = Rig::start(Opts::default()).await;
    let long = "This is a long notice that keeps on going for a good while so it can be cut.";
    rig.ready().await;
    let stopped = |rig: &Rig| {
        rig.events("agent_audio_stopped")
            .iter()
            .filter(|event| event["reason"] == "barge_in")
            .count()
    };
    // "go on" is a control (resume) with no stop cue: its kind is read, and it does not cut.
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.until("the line plays", 10_000, |rig| {
        rig.said().contains(&long.to_owned())
    })
    .await;
    rig.run(600).await;
    rig.onset().await;
    rig.run(100).await;
    rig.stt.interim("go on").await;
    rig.run(400).await;
    assert_eq!(stopped(&rig), 0, "{}", rig.timeline());
    rig.offset(400).await;
    rig.smart_turn(0.9).await;
    rig.stt.speech_final("go on").await;
    rig.run(2_000).await;
    assert_eq!(stopped(&rig), 0, "{}", rig.timeline());
    rig.send(Msg::ClientClear).await;
    rig.run(1_000).await;

    // A request over the agent: the first interim's `delegate` does not cut, the second one's
    // does, before the barge-in's own 900 ms deadline.
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.run(1_500).await;
    let onset = rig.onset().await;
    rig.run(100).await;
    rig.stt.interim("what time does").await;
    rig.run(150).await;
    assert_eq!(stopped(&rig), 0, "{}", rig.timeline());
    rig.stt.interim("what time does the store open").await;
    rig.until("the cut", 1_000, |rig| stopped(rig) == 1).await;
    let cut = rig.time_of("the cut", |event| {
        event["type"] == "agent_audio_stopped" && event["reason"] == "barge_in"
    });
    assert!(cut < onset + 800, "onset {onset}, cut {cut}");
    rig.run(200).await;
    rig.offset(900).await;
    rig.smart_turn(0.9).await;
    rig.stt.speech_final("what time does the store open").await;
    rig.run(3_000).await;

    // "top talking" (no word list reads it): read on, its kind is a stop, and it cuts.
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.run(1_500).await;
    rig.onset().await;
    rig.run(100).await;
    rig.stt.interim("top talking").await;
    rig.run(300).await;
    assert_eq!(stopped(&rig), 2, "{}", rig.timeline());
    rig.offset(300).await;
    rig.smart_turn(0.9).await;
    rig.stt.speech_final("top talking").await;
    rig.run(3_000).await;

    // A hold cuts as any other act: not at its first interim, at the next that reads it too.
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.run(1_500).await;
    rig.onset().await;
    rig.run(100).await;
    rig.stt.interim("hold on").await;
    rig.run(300).await;
    assert_eq!(stopped(&rig), 2, "{}", rig.timeline());
    rig.stt.interim("hold on a moment").await;
    rig.until("the hold's cut", 1_000, |rig| stopped(rig) == 3)
        .await;
}

/// A front double that reads "wait a second" as a hold only when the observation says the
/// caller talks over the agent, else as a `wait` (the 2B front, live S1 r14); the rest as
/// [`ix_mocks::reads_the_turn`].
fn holds_only_over_the_agent(observation: &str) -> String {
    if !observation.contains("wait a second") {
        return ix_mocks::reads_the_turn(observation);
    }
    if observation.contains("USER (interrupting you): \u{ab}wait a second") {
        r#"control","kind":"hold","say":""}"#.to_owned()
    } else {
        r#"wait","why":"incomplete","say":""}"#.to_owned()
    }
}

/// r15 (live S1): a turn decided while the agent's line still plays is shown to the front as
/// said over the agent, so "wait a second" whose one interim did not cut is the turn's hold and
/// pauses the agent; the same words while the agent is quiet are shown as a plain turn.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_turn_decided_while_the_agent_speaks_is_shown_as_said_over_it() {
    let front = FrontDouble::start(FrontScript {
        decide: Some(holds_only_over_the_agent),
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(Opts {
        front: Some(front.url.clone()),
        ..Opts::default()
    })
    .await;
    let long = "This is a long notice that keeps on going for a good while so it can be cut.";
    rig.ready().await;
    // While the agent is quiet the words are a plain turn: no control.
    rig.onset().await;
    rig.run(600).await;
    rig.offset(300).await;
    rig.smart_turn(0.97).await;
    rig.stt.speech_final("wait a second").await;
    rig.run(3_000).await;
    assert!(controls_applied(&rig).is_empty(), "{}", rig.timeline());
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.until("the line plays", 10_000, |rig| {
        rig.said().contains(&long.to_owned())
    })
    .await;
    rig.run(600).await;
    rig.onset().await;
    rig.run(100).await;
    // One interim's hold does not cut; the turn ends before a second one.
    rig.stt.interim("wait a second").await;
    rig.run(300).await;
    assert!(
        rig.events("agent_audio_stopped").is_empty(),
        "{}",
        rig.timeline()
    );
    rig.offset(300).await;
    rig.smart_turn(0.97).await;
    rig.stt.speech_final("wait a second").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].0, "hold");
    rig.until_event("agent_audio_stopped", 1).await;
}

/// A stop whose interims did not cut (live S1: the first interim "zob" was no sure reading and
/// the turn ended before the cue's own) is the turn's control: it cuts the line that plays on.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_stop_whose_interims_did_not_cut_cuts_at_the_end_of_the_turn() {
    let rig = Rig::start(Opts::default()).await;
    let long = "This is a long notice that keeps on going for a good while so it can be cut.";
    rig.ready().await;
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.until("the line plays", 10_000, |rig| {
        rig.said().contains(&long.to_owned())
    })
    .await;
    rig.run(600).await;
    rig.onset().await;
    rig.run(100).await;
    rig.stt.interim("zob").await;
    rig.run(200).await;
    rig.offset(300).await;
    rig.smart_turn(0.97).await;
    rig.stt.speech_final("stop talking").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].0, "stop");
    rig.until_event("agent_audio_stopped", 1).await;
    assert_eq!(rig.events("agent_audio_stopped")[0]["reason"], "barge_in");
    // At the decision's act, before its whole line was read and the control applied.
    let cut = rig.time_of("the cut", |event| event["type"] == "agent_audio_stopped");
    let applied = rig.time_of("the stop", |event| event["type"] == "control_applied");
    assert!(cut <= applied, "cut {cut}, applied {applied}");
}

/// An interim still out when its turn ends is kept until the turn's decision has its act (live
/// S1: the cue's interim was dropped at the end of the turn and the cut waited ~0.7 s for the
/// decision's act): its sure stop cuts the agent first, once. The decision still runs and
/// settles the turn, also when it reads the words otherwise.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_interim_still_out_at_the_end_of_its_turn_cuts_before_the_decision() {
    // Every decision names its act 300 ms after it was asked.
    let front = FrontDouble::start(FrontScript {
        decide: Some(ix_mocks::reads_the_turn),
        act_ms: 300,
        line_ms: 40,
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(Opts {
        front: Some(front.url.clone()),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|_| Some(answer(300)));
    let long = "This is a long notice that keeps on going for a good while so it can be cut.";
    let cuts = |rig: &Rig| {
        let cut = |event: &&Value| event["reason"] == "barge_in";
        rig.events("agent_audio_stopped").iter().filter(cut).count()
    };
    rig.ready().await;
    for (words, cut_by) in [
        ("stop talking", "stop"),
        ("stop by the store at six", "task"),
    ] {
        let before = cuts(&rig);
        rig.send(Msg::ClientSpeak(long.to_owned())).await;
        rig.run(1_500).await;
        rig.onset().await;
        rig.run(100).await;
        rig.stt.interim("stop talking").await;
        let asked = rig.now();
        // The turn ends 250 ms later, before the interim has its act.
        rig.run(250).await;
        rig.offset(300).await;
        rig.smart_turn(0.97).await;
        rig.stt.speech_final(words).await;
        let ended = rig.now();
        rig.until("the cut", 1_000, |rig| cuts(rig) == before + 1)
            .await;
        let mut at = rig.wire().into_iter().filter(|(_, event)| {
            event["type"] == "agent_audio_stopped" && event["reason"] == "barge_in"
        });
        let cut = at.next_back().map(|(at, _)| at).unwrap_or_default();
        assert!(
            cut < asked + 300 + 100 && cut < ended + 300,
            "{words}: interim asked {asked}, turn ended {ended}, cut {cut}\n{}",
            rig.timeline()
        );
        rig.run(2_000).await;
        assert_eq!(
            cuts(&rig),
            before + 1,
            "{words}: one cut\n{}",
            rig.timeline()
        );
        match cut_by {
            "stop" => assert_eq!(
                controls_applied(&rig)
                    .iter()
                    .map(|c| c.0.as_str())
                    .collect::<Vec<_>>(),
                ["stop"]
            ),
            _ => {
                assert_eq!(controls_applied(&rig).len(), 1, "no second stop");
                rig.until_event("task_created", 1).await;
            }
        }
        rig.send(Msg::ClientClear).await;
        rig.run(1_000).await;
    }
}

/// A turn whose barge-in already cut the agent leaves alone the line the client starts after
/// it: its interim, back after the end of the turn, does not cut that line (live S1, r8-verify:
/// the cue's interim came 1.3 s late and cut the next cue's line before the caller spoke).
#[tokio::test(start_paused = true)]
#[serial]
async fn a_late_interim_of_a_turn_that_already_cut_leaves_the_next_line_alone() {
    // Every request names its act 1,400 ms after it was asked.
    let front = FrontDouble::start(FrontScript {
        decide: Some(ix_mocks::reads_the_turn),
        act_ms: 1_400,
        line_ms: 40,
        ..FrontScript::default()
    })
    .await;
    let rig = Rig::start(Opts {
        front: Some(front.url.clone()),
        ..Opts::default()
    })
    .await;
    let long = "This is a long notice that keeps on going for a good while so it can be cut.";
    let cuts = |rig: &Rig| {
        let cut = |event: &&Value| event["reason"] == "barge_in";
        rig.events("agent_audio_stopped").iter().filter(cut).count()
    };
    rig.ready().await;
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.run(1_500).await;
    rig.onset().await;
    rig.run(100).await;
    rig.stt.interim("stop talking").await;
    // The barge-in's own deadline cuts the line while the caller still speaks.
    rig.until("the deadline's cut", 1_000, |rig| cuts(rig) == 1)
        .await;
    rig.offset(900).await;
    rig.smart_turn(0.97).await;
    rig.stt.speech_final("stop talking").await;
    // The client's next line, before the interim (asked 1,400 ms before its act) is back.
    rig.send(Msg::ClientSpeak(long.to_owned())).await;
    rig.run(3_000).await;
    assert_eq!(
        cuts(&rig),
        1,
        "the next line is not cut\n{}",
        rig.timeline()
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_language_without_a_pack_is_reported_unsupported_and_said_honestly() {
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "languages": ["es", "zh"] } })),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    rig.turn("can you talk to me in chinese").await;
    rig.until_event("control_applied", 1).await;
    let applied = rig.events("control_applied")[0].clone();
    assert_eq!(
        (&applied["kind"], &applied["value"]),
        (&json!("language"), &json!("zh"))
    );
    assert_eq!(applied["outcome"], "unsupported");
    rig.until("the honest line", 5_000, |rig| {
        rig.said()
            .contains(&"Sorry, I can only speak English on this line.".to_owned())
    })
    .await;
    assert_eq!(latest_state(&rig)["language"], "en");
    assert!(rig.tts.voices().is_empty(), "nothing pretends to switch");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_language_switch_answers_in_it_and_tells_the_orchestrator() {
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "languages": ["es"], "answers_only": true } })),
        stt_language: Some("multi"),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|_| Some(answer(300)));
    rig.ready().await;
    rig.send(Msg::Voices(vec![("es".to_owned(), "voice-es".to_owned())]))
        .await;
    rig.turn("talk to me in spanish please").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].3, "partial");
    rig.until("the voice is switched", 2_000, |rig| {
        rig.tts.voices() == ["voice-es"]
    })
    .await;
    assert_eq!(latest_state(&rig)["language"], "es");
    // A Spanish turn is the conversation's own now, and the background is told the state.
    rig.run(2_000).await;
    rig.turn("a que hora abre la tienda").await;
    rig.until_event("task_created", 1).await;
    rig.until_heard("It closes at six.").await;
    let requests = rig.llm.requests();
    let system = ix_mocks::messages(&requests[requests.len() - 1], "system").join("\n");
    assert!(
        system.contains("every answer is in Spanish (es)"),
        "{system}"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_language_switch_asked_for_before_the_voice_list_came_waits_for_it() {
    // Live CT2: Deepgram's voice list took 10 s, and "talk to me in Spanish" came first.
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "languages": ["es"], "answers_only": true } })),
        stt_language: Some("multi"),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    rig.send(Msg::VoicesComing).await;
    rig.turn("talk to me in spanish please").await;
    rig.run(3_000).await;
    assert!(
        rig.events("control_applied").is_empty(),
        "not decided before the list"
    );
    rig.send(Msg::Voices(vec![("es".to_owned(), "voice-es".to_owned())]))
        .await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].3, "partial");
    rig.until("the voice is switched", 2_000, |rig| {
        rig.tts.voices() == ["voice-es"]
    })
    .await;
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_language_switch_opens_a_recogniser_in_it_and_swaps_it_in_at_a_pause() {
    // A recogniser in one language (Deepgram nova-3 `en`): a second one is opened in Spanish
    // before the switch is applied, and hears the caller from their next pause.
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "languages": ["es"], "answers_only": true } })),
        stt_language: Some("en"),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|_| Some(answer(300)));
    rig.ready().await;
    rig.send(Msg::Voices(vec![("es".to_owned(), "voice-es".to_owned())]))
        .await;
    assert_eq!(rig.stt.languages().len(), 1);
    rig.turn("talk to me in spanish please").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].3, "partial");
    let languages = rig.stt.languages();
    assert_eq!(languages.len(), 2, "{languages:?}");
    assert!(languages[1].starts_with("es"), "{languages:?}");
    rig.until("the voice is switched", 2_000, |rig| {
        rig.tts.voices() == ["voice-es"]
    })
    .await;
    // The new recogniser's words are the caller's turn; the one it replaced is disconnected.
    rig.run(2_000).await;
    assert_eq!(rig.stt.disconnects(), 1);
    rig.turn("a que hora abre la tienda").await;
    rig.until_event("task_created", 1).await;
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_callers_language_wish_is_kept_and_applied_at_their_next_call() {
    let opts = |stream: &str| Opts {
        operator: Some(json!({ "controls": { "languages": ["es"], "answers_only": true } })),
        stt_language: Some("multi"),
        memory: Some(("memory-key", "caller-with-a-wish")),
        back: Some(("owner-of-the-wish".to_owned(), stream.to_owned())),
        ..Opts::default()
    };
    let voices = || Msg::Voices(vec![("es".to_owned(), "voice-es".to_owned())]);
    let first = Rig::start(opts("first-call")).await;
    first.ready().await;
    first.send(voices()).await;
    first.turn("talk to me in spanish please").await;
    first.until_event("control_applied", 1).await;
    // Kept off the actor.
    first.run(1_000).await;
    let wish = tenant::language_wish("memory-key", "owner-of-the-wish", "caller-with-a-wish");
    assert_eq!(wish.await.as_deref(), Some("es"));

    let second = Rig::start(opts("second-call")).await;
    second.send(voices()).await;
    second.ready().await;
    second.until_event("control_applied", 1).await;
    assert_eq!(
        controls_applied(&second)[0],
        (
            "language".into(),
            "es".into(),
            "memory".into(),
            "partial".into()
        )
    );
    assert_eq!(latest_state(&second)["language"], "es");
    second.llm.respond_with(|_| Some(answer(300)));
    second.run(3_000).await;
    second.turn("a que hora abre la tienda").await;
    second.until_event("task_created", 1).await;
    let requests = second.llm.requests();
    let users = ix_mocks::messages(&requests[requests.len() - 1], "user");
    assert!(
        users.iter().any(|line| line
            == "[the caller asked for Spanish on an earlier call; language switched to es (partly)]"),
        "{users:?}"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_repeat_slower_and_louder_changes_that_line_only() {
    let rig = Rig::start(Opts::default()).await;
    rig.llm.respond_with(|_| {
        Some(Round::text(
            300,
            20,
            &["Your code is 4 4 1 7, valid today."],
        ))
    });
    rig.ready().await;
    let line = "Your code is 4 4 1 7, valid today.";
    rig.turn("what is my code").await;
    rig.until_heard(line).await;
    let first = rig.line(line);
    rig.run(1_000).await;
    rig.turn("sorry, say that again, slower").await;
    rig.until("the line again", 10_000, |rig| {
        rig.said().iter().filter(|said| *said == line).count() == 2
    })
    .await;
    // Slower for that line, then the session's own rate again.
    rig.until("the rate is back", 2_000, |rig| {
        rig.tts.rates() == [0.85, 1.0]
    })
    .await;
    rig.until("the slower line is heard", 20_000, |rig| {
        rig.lines()
            .iter()
            .filter(|said| said["delta"] == line)
            .all(|said| rig.heard(&said["utterance_id"]) == "full")
    })
    .await;
    let state = latest_state(&rig);
    assert_eq!(
        (state["rate"].as_f64(), state["volume_db"].as_f64()),
        (Some(1.0), Some(0.0))
    );
    rig.run(1_000).await;
    // Louder: that line's frames 3 dB up, the next line's not.
    rig.turn("say that again, louder").await;
    rig.until("the line a third time", 10_000, |rig| {
        rig.said().iter().filter(|said| *said == line).count() == 3
    })
    .await;
    rig.run(4_000).await;
    rig.send(Msg::ClientSpeak("That is all for now.".to_owned()))
        .await;
    rig.until_heard("That is all for now.").await;
    let kinds: Vec<String> = controls_applied(&rig)
        .into_iter()
        .map(|c| format!("{}:{}:{}", c.0, c.1, c.3))
        .collect();
    assert_eq!(kinds, ["repeat:slower:applied", "repeat:louder:applied"]);
    let starts: Vec<Ms> = rig
        .lines()
        .iter()
        .filter(|said| said["delta"] == line || said["delta"] == "That is all for now.")
        .map(|said| {
            let id = said["utterance_id"].clone();
            rig.time_of("its start", |event| event["utterance_id"] == id)
        })
        .collect();
    assert_eq!(starts.len(), 4, "{}", rig.timeline());
    let peak = |from: Ms, to: Ms| {
        let frames = rig.frames_in(from, to);
        frames.iter().map(|(_, peak)| *peak).max().unwrap_or(0)
    };
    let (plain, louder, after) = (
        peak(starts[0], starts[0] + 500),
        peak(starts[2], starts[2] + 500),
        peak(starts[3], starts[3] + 500),
    );
    assert!(plain > 0 && first["delta"] == line);
    let ratio = f64::from(louder) / f64::from(plain);
    assert!(
        (1.35..1.48).contains(&ratio),
        "3 dB louder: {plain} -> {louder}"
    );
    assert_eq!(after, plain, "the next line at the session's volume");
}

/// A provider's voice list as `/voices` gives it.
fn listed_voice(id: &str, gender: &str, accent: &str) -> waav_gateway::handlers::voices::Voice {
    serde_json::from_value(json!({
        "id": id, "sample": "", "name": id, "accent": accent, "gender": gender,
        "language": "English",
    }))
    .expect("a voice")
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_voice_switch_renders_the_listening_sounds_and_failure_lines_again_in_it() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    rig.run(3_000).await;
    let failure_line = "Sorry, my systems are acting up. Bear with me a moment.";
    let renders = |rig: &Rig| {
        rig.tts
            .spoken()
            .iter()
            .filter(|t| *t == failure_line)
            .count()
    };
    assert_eq!(renders(&rig), 1, "{:?}", rig.tts.spoken());
    rig.send(Msg::VoiceCatalog(vec![
        listed_voice("voice-ava", "Female", "American"),
        listed_voice("voice-ben", "Male", "British"),
        listed_voice("voice-cal", "Male", "American"),
    ]))
    .await;
    rig.turn("can you use a male voice instead").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(
        controls_applied(&rig)[0],
        (
            "voice".into(),
            "male".into(),
            "caller".into(),
            "applied".into()
        )
    );
    assert_eq!(latest_state(&rig)["voice"], "voice-ben");
    rig.until_heard("Okay, I'll speak in this voice now.").await;
    assert_eq!(rig.tts.voices(), ["voice-ben"]);
    // The lines said when a service fails are made again, in the new voice.
    rig.until("the failure lines again", 10_000, |rig| renders(rig) == 2)
        .await;
    // Nothing of that kind in the list: said, and the voice stays.
    rig.run(1_000).await;
    rig.turn("a british voice please").await;
    rig.until_event("control_applied", 2).await;
    assert_eq!(controls_applied(&rig)[1].3, "unsupported");
    assert_eq!(latest_state(&rig)["voice"], "voice-ben");
}

/// A failure line whose render failed (silent audio, refused by the line checks) is made
/// again when the voice changes; a render in flight at the switch leaves nothing in the pump.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_voice_switch_renders_a_failed_failure_line_again_and_drops_the_old_voices_audio() {
    let rig = Rig::start(Opts::default()).await;
    let failure_line = "Sorry, my systems are acting up. Bear with me a moment.";
    rig.tts.silence(failure_line);
    let renders = |rig: &Rig| {
        rig.tts
            .spoken()
            .iter()
            .filter(|t| *t == failure_line)
            .count()
    };
    rig.ready().await;
    rig.run(3_000).await;
    assert_eq!(renders(&rig), 1, "{:?}", rig.tts.spoken());
    // The next renders take long: one is in flight at the switch.
    rig.tts
        .synth_ms
        .store(1_500, std::sync::atomic::Ordering::SeqCst);
    rig.send(Msg::VoiceCatalog(vec![
        listed_voice("voice-ava", "Female", "American"),
        listed_voice("voice-ben", "Male", "British"),
    ]))
    .await;
    rig.turn("can you use a male voice instead").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(latest_state(&rig)["voice"], "voice-ben");
    rig.until("the failed line again", 30_000, |rig| renders(rig) == 2)
        .await;
    rig.tts
        .synth_ms
        .store(40, std::sync::atomic::Ordering::SeqCst);
    rig.run(20_000).await;
    assert_eq!(rig.ctl.captured_clips(), 0, "{:?}", rig.tts.spoken());
}

/// An orchestrator's language switch waits for the voice list as the caller's does, and its
/// tool gets the switch's result when it is applied.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrators_language_switch_waits_for_the_voice_list_and_its_tool_hears_how_it_went()
{
    let taken = |_: &Hit| (200, "[]".to_owned());
    let browser = BackendDouble::start(taken).await;
    let catalog = vec![agent_entry_of(
        "browser",
        &browser.url,
        json!({ "description": "Operates websites in a browser." }),
    )];
    let rig = Rig::start(Opts {
        interaction: Some(json!({ "tasks": { "orchestrator": "mock", "agents": ["browser"] } })),
        catalog,
        operator: Some(json!({ "controls": { "languages": ["es"], "answers_only": true } })),
        stt_language: Some("multi"),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round {
                tool_calls: vec![(
                    "conversation_control".to_owned(),
                    json!({ "kind": "language", "value": "es", "asked_by": "speaks Spanish" }),
                )],
                end_ms: 200,
                ..Round::default()
            },
            _ => Round::text(50, 20, &["ANSWER:\n", "It opens at nine."]),
        })
    });
    rig.ready().await;
    rig.send(Msg::VoicesComing).await;
    // The words the switch quotes (the front takes "in Spanish, please" as its own control).
    rig.turn("my mother speaks spanish, when does the store open")
        .await;
    rig.run(3_000).await;
    assert!(
        rig.events("control_applied").is_empty(),
        "not decided before the list: {}",
        rig.timeline()
    );
    let answered = |rig: &Rig| {
        let requests = rig.llm.requests();
        requests
            .iter()
            .any(|request| !ix_mocks::messages(request, "tool").is_empty())
    };
    assert!(!answered(&rig), "the tool waits for the switch");
    rig.send(Msg::Voices(vec![("es".to_owned(), "voice-es".to_owned())]))
        .await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(
        controls_applied(&rig)[0],
        (
            "language".into(),
            "es".into(),
            "agent".into(),
            "partial".into()
        )
    );
    rig.until("the tool's result", 5_000, answered).await;
    let requests = rig.llm.requests();
    let tool = ix_mocks::messages(&requests[requests.len() - 1], "tool").join("");
    assert!(
        tool.contains("control_applied") && tool.contains("partial"),
        "{tool}"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn without_a_transfer_the_caller_hears_that_no_person_is_available() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    rig.turn("let me talk to a person").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].3, "unsupported");
    rig.until_heard("Sorry, there's no one I can put you through to on this line.")
        .await;
    assert!(rig.events("human_requested").is_empty());
    assert!(rig.events("close").is_empty(), "the call goes on");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_person_is_asked_for_with_a_signed_request_and_the_session_ends() {
    // The operator's reply says a person took the call.
    let reply = json!({ "type": "human_connected" }).to_string();
    let operator = BackendDouble::start(move |_| (200, reply.clone())).await;
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "human": { "url": operator.url } } })),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    rig.turn("can I speak to a human please").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].3, "applied");
    let requested = rig.events("human_requested");
    assert_eq!(requested[0]["via"], "webhook");
    rig.until_heard("Of course, I'm finding a person for you. One moment.")
        .await;
    rig.until_heard("Of course, I'm putting you through to a person now.")
        .await;
    assert_eq!(rig.events("human_answered")[0]["outcome"], "connected");
    rig.until_event("close", 1).await;
    let close = rig.events("close")[0].clone();
    assert_eq!(
        (&close["code"], &close["reason"]),
        (&json!(1000), &json!("transferred"))
    );
    rig.until("the operator is told", 5_000, |_| {
        !operator.hits().is_empty()
    })
    .await;
    let hit = operator.hits()[0].clone();
    assert!(
        hit.call.signed_with(TOOL_SECRET),
        "signed with the server's secret"
    );
    let body = &hit.call.body;
    assert_eq!(
        (&body["type"], &body["via"], &body["stream_id"]),
        (
            &json!("human_requested"),
            &json!("webhook"),
            &json!(rig.stream_id)
        )
    );
    assert_eq!(body["state"]["language"], "en");
    let task = body["task_id"].as_str().expect("the hand-off's id");
    assert!(task.starts_with("human_"), "{task}");
    assert_eq!(body["callback"], format!("/v1/tasks/{task}/callback"));
    let history = body["history"].as_array().expect("the history");
    assert!(
        history
            .iter()
            .any(|turn| turn["content"] == "can I speak to a human please"),
        "{history:?}"
    );
}

/// The operator takes the request and answers later on the callback route: the persona's
/// line comes when the person answers. Nobody within `controls.human.wait_s`, or
/// `human_unavailable`: an honest line, and the call goes on.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_person_answers_on_the_callback_route_and_no_answer_is_said_honestly() {
    let operator = BackendDouble::start(|_| (200, "{}".to_owned())).await;
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "human": { "url": operator.url, "wait_s": 20 } } })),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    rig.turn("can I speak to a human please").await;
    rig.until_heard("Of course, I'm finding a person for you. One moment.")
        .await;
    rig.until("the operator is asked", 5_000, |_| {
        !operator.hits().is_empty()
    })
    .await;
    let task = operator.hits()[0].call.body["task_id"]
        .as_str()
        .expect("the hand-off's id")
        .to_owned();
    // Not a person yet: other events are not an answer.
    call_back(
        &task,
        TOOL_SECRET,
        json!({ "type": "progress", "text": "ringing" }),
    )
    .expect("taken");
    rig.run(5_000).await;
    assert!(rig.events("human_answered").is_empty());
    call_back(&task, TOOL_SECRET, json!({ "type": "human_connected" })).expect("taken");
    rig.until_heard("Of course, I'm putting you through to a person now.")
        .await;
    rig.until_event("close", 1).await;

    // A second session: nobody answers within the 20 s.
    let operator = BackendDouble::start(|_| (200, "{}".to_owned())).await;
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "human": { "url": operator.url, "wait_s": 20 } } })),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    rig.turn("can I speak to a human please").await;
    rig.until_heard("Of course, I'm finding a person for you. One moment.")
        .await;
    rig.run(15_000).await;
    assert!(rig.events("human_answered").is_empty(), "still waiting");
    rig.until_heard("Sorry, nobody can take your call right now, but I am still here to help.")
        .await;
    assert_eq!(rig.events("human_answered")[0]["outcome"], "no_answer");
    rig.run(3_000).await;
    assert!(rig.events("close").is_empty(), "the call goes on");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_sip_caller_is_referred_once_the_line_is_said() {
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "human": { "transfer_to": "+15551230000" } } })),
        ..Opts::default()
    })
    .await;
    let referred: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = referred.clone();
    rig.send(Msg::SipTransfer(actor::SipTransfer(Arc::new(move |to| {
        seen.lock().push(to);
        let referral: actor::Referral = Box::pin(async {});
        referral
    }))))
    .await;
    rig.ready().await;
    // Not a phone caller yet: no transfer for them.
    rig.turn("let me talk to a person").await;
    rig.until_event("control_applied", 1).await;
    assert_eq!(controls_applied(&rig)[0].3, "unsupported");
    rig.run(3_000).await;
    rig.ctl.set_phone_leg();
    rig.turn("let me talk to a person").await;
    rig.until_event("control_applied", 2).await;
    assert_eq!(controls_applied(&rig)[1].3, "applied");
    assert_eq!(rig.events("human_requested")[0]["via"], "sip");
    assert!(referred.lock().is_empty(), "not before the line is said");
    rig.until_heard("Of course, I'm putting you through to a person now.")
        .await;
    rig.until("the call is referred", 2_000, |_| {
        *referred.lock() == ["+15551230000"]
    })
    .await;
    assert!(
        rig.events("close").is_empty(),
        "the room's leg ends the call"
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_client_control_and_an_operator_bound_on_it() {
    let rig = Rig::start(Opts {
        operator: Some(json!({ "controls": { "allowed": ["stop", "verbosity"] } })),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    let control = |kind: &str, value: &str| {
        let raw = json!({ "kind": kind, "value": value });
        Msg::Control(Control::from_json(&raw, ControlSource::Client).expect("a control"))
    };
    rig.send(control("verbosity", "brief")).await;
    rig.send(control("volume", "louder")).await;
    rig.until_event("control_applied", 2).await;
    assert_eq!(
        controls_applied(&rig),
        [
            (
                "verbosity".into(),
                "brief".into(),
                "client".into(),
                "applied".into()
            ),
            (
                "volume".into(),
                "louder".into(),
                "client".into(),
                "refused".into()
            ),
        ]
    );
    assert_eq!(latest_state(&rig)["verbosity"], "brief");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrator_issues_a_conversation_control() {
    let taken = |_: &Hit| (200, "[]".to_owned());
    let browser = BackendDouble::start(taken).await;
    let catalog = vec![agent_entry_of(
        "browser",
        &browser.url,
        json!({ "description": "Operates websites in a browser." }),
    )];
    let rig = orchestrated(catalog, &["browser"]).await;
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round {
                tool_calls: vec![(
                    "conversation_control".to_owned(),
                    json!({
                        "kind": "verbosity",
                        "value": "brief",
                        "asked_by": "keep your answers short",
                    }),
                )],
                end_ms: 200,
                ..Round::default()
            },
            _ => Round::text(50, 20, &["ANSWER:\n", "It opens at nine."]),
        })
    });
    rig.ready().await;
    rig.turn("keep your answers short, when does the store open")
        .await;
    rig.until_heard("It opens at nine.").await;
    // The control round asks for it too (the responder's first round): applied once.
    assert_eq!(
        controls_applied(&rig),
        [(
            "verbosity".into(),
            "brief".into(),
            "agent".into(),
            "applied".into()
        )]
    );
    let requests = rig.llm.requests();
    let after = (requests.iter())
        .find(|request| !ix_mocks::messages(request, "tool").is_empty())
        .expect("the round after the tool");
    let tool = ix_mocks::messages(after, "tool").join("");
    assert!(
        tool.contains("control_applied") && tool.contains("applied"),
        "{tool}"
    );
}

/// A control no one asked for is never applied (r7-verify; live MA7: after starting the
/// planner, gpt-oss-20b's next round called `updates tell` on "Find me a hotel in Goa", and
/// once the control round `verbosity brief`): its `asked_by` must quote the operator's prompt
/// or the caller's words, and the tool says why not.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrators_control_that_no_one_asked_for_is_not_applied() {
    let taken = |_: &Hit| (200, "[]".to_owned());
    let browser = BackendDouble::start(taken).await;
    let catalog = vec![agent_entry_of(
        "browser",
        &browser.url,
        json!({ "description": "Operates websites in a browser." }),
    )];
    let rig = orchestrated(catalog, &["browser"]).await;
    rig.llm.respond_with(|request| {
        Some(if control_round(request) {
            Round::text(50, 20, &["NONE"])
        } else if ix_mocks::messages(request, "tool").is_empty() {
            let unasked = json!({ "kind": "verbosity", "value": "brief", "asked_by": "be quick" });
            Round::tool(100, "conversation_control", unasked)
        } else {
            Round::text(50, 20, &["ANSWER:\n", "The Sea View has a room."])
        })
    });
    rig.ready().await;
    rig.turn("find me a hotel in goa for two nights").await;
    rig.until_heard("The Sea View has a room.").await;
    rig.run(1_000).await;
    assert!(controls_applied(&rig).is_empty(), "{}", rig.timeline());
    assert_ne!(latest_state(&rig)["verbosity"], "brief");
    let requests = answering(&rig);
    let tool = ix_mocks::messages(requests.last().expect("a request"), "tool").join("");
    assert!(tool.contains("Not applied"), "{tool}");
}

/// The control round (chapter 9, r7-fix): beside its first round an orchestrator is asked
/// apart, with the operator's prompt and conversation_control alone, whether the call itself
/// should change; what it asks for there is applied though the answer round did not ask, when
/// its `asked_by` quotes the operator's prompt or the caller's words (r7-verify).
#[tokio::test(start_paused = true)]
#[serial]
async fn an_orchestrators_control_round_applies_a_change_someone_named() {
    let taken = |_: &Hit| (200, "[]".to_owned());
    let browser = BackendDouble::start(taken).await;
    let catalog = vec![agent_entry_of(
        "browser",
        &browser.url,
        json!({ "description": "Operates websites in a browser." }),
    )];
    let rig = orchestrated(catalog, &["browser"]).await;
    rig.llm.respond_with(move |request| {
        let said = ix_mocks::messages(request, "user").join("");
        Some(if !control_round(request) {
            Round::text(50, 20, &["ANSWER:\n", "Lima."])
        } else if said.contains("chile") {
            // Named by no one: a quote neither was shown, no quote at all; and brief again.
            let control = |kind: &str, value: &str, asked_by: Option<&str>| {
                let args = json!({ "kind": kind, "value": value, "asked_by": asked_by });
                ("conversation_control".to_owned(), args)
            };
            Round {
                tool_calls: vec![
                    control("verbosity", "detailed", Some("tell me everything")),
                    control("rate", "slower", None),
                    control("verbosity", "brief", Some("keep your answers brief")),
                ],
                end_ms: 100,
                ..Round::default()
            }
        } else {
            // r11-fix: the round's own frame, and the words in their order within the
            // caller's sentence (gpt-oss-20b quoted CT3's instruction as "verbosity brief").
            let quote = "The caller said: Keep answers BRIEF";
            let args = json!({ "kind": "verbosity", "value": "brief", "asked_by": quote });
            Round::tool(100, "conversation_control", args)
        })
    });
    rig.ready().await;
    rig.turn("keep your answers brief, what is the capital of peru")
        .await;
    rig.until_heard("Lima.").await;
    rig.run(1_000).await;
    assert_eq!(
        controls_applied(&rig),
        [(
            "verbosity".into(),
            "brief".into(),
            "agent".into(),
            "applied".into()
        )],
        "{}",
        rig.timeline()
    );
    assert_eq!(latest_state(&rig)["verbosity"], "brief");
    let requests = rig.llm.requests();
    let tools = |request: &Value| -> Vec<String> {
        let tools = request["tools"].as_array().cloned().unwrap_or_default();
        let name = |tool: &Value| {
            tool["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        };
        tools.iter().map(name).collect()
    };
    let (asked, answered): (Vec<&Value>, Vec<&Value>) =
        requests.iter().partition(|request| control_round(request));
    assert_eq!(asked.len(), 1, "one control round per run");
    assert_eq!(tools(asked[0]), ["conversation_control"]);
    let said = ix_mocks::messages(asked[0], "user").join("");
    assert!(said.contains("what is the capital of peru"), "{said}");
    // It is asked for the quote, and shown no caller context.
    let parameters = &asked[0]["tools"][0]["function"]["parameters"];
    assert_eq!(parameters["required"], json!(["kind", "value", "asked_by"]));
    // Not the persona's voice contract: the operator's prompt and the context only.
    let system = ix_mocks::messages(asked[0], "system").join("");
    let answer_system = ix_mocks::messages(answered[0], "system").join("");
    assert!(!system.contains("Voice contract") && answer_system.contains("Voice contract"));
    assert!(tools(answered[0]).contains(&"conversation_control".to_owned()));

    // The next run's control round asks for changes no one named, and for brief again (the
    // call is brief already): nothing is applied or reported.
    rig.turn("and what is the capital of chile").await;
    rig.until("a second control round", 10_000, |rig| {
        let requests = rig.llm.requests();
        requests
            .iter()
            .filter(|request| control_round(request))
            .count()
            == 2
    })
    .await;
    rig.run(3_000).await;
    assert_eq!(controls_applied(&rig).len(), 1, "{}", rig.timeline());
}

/// A webhook and an A2A agent ask for controls on their own wires (chapter 9): each is
/// applied by the one control executor, and its `control_applied` goes back on that wire.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_agent_asks_for_a_control_on_its_wire_and_hears_how_it_went() {
    let hook = BackendDouble::start(|hit: &Hit| {
        let reply = match hit.call.body["type"].as_str() {
            Some("task") => json!([
                { "type": "control", "kind": "verbosity", "value": "brief", "request_id": "c-1" },
                { "type": "progress", "text": "Looking it up." },
            ]),
            Some("control_applied") => json!({ "type": "result", "text": "It opens at nine." }),
            _ => json!({}),
        };
        (200, reply.to_string())
    })
    .await;
    let entry = json!({ "name": "crm", "type": "webhook", "url": hook.url });
    let rig = rig_on(backend(entry, Some(HOOK_SECRET)), None).await;
    rig.ready().await;
    rig.turn("when does the store open").await;
    rig.until_heard("It opens at nine.").await;
    assert_eq!(
        controls_applied(&rig),
        [(
            "verbosity".into(),
            "brief".into(),
            "agent".into(),
            "applied".into()
        )],
        "{}",
        rig.timeline()
    );
    let applied = hook.hits().into_iter().map(|hit| hit.call);
    let mut applied = applied.filter(|call| call.body["type"] == "control_applied");
    let applied = applied
        .next()
        .expect("the webhook hears how its control went");
    assert!(applied.signed_with(HOOK_SECRET));
    assert_eq!(
        (
            &applied.body["request_id"],
            &applied.body["outcome"],
            &applied.body["kind"]
        ),
        (&json!("c-1"), &json!("applied"), &json!("verbosity"))
    );
    let key = &applied.headers["idempotency-key"];
    assert!(key.ends_with(":control_applied:c-1"), "{key}");
    drop(rig);

    // An A2A agent puts the control in a data part of its status message; it is applied
    // once however often the task is read, and the answer is a message to the task.
    let agent = BackendDouble::start(|hit: &Hit| {
        let (method, params) = (hit.call.body["method"].as_str(), &hit.call.body["params"]);
        let control = json!({ "data": { "type": "control", "kind": "verbosity", "value": "detailed" } });
        let working = json!({ "state": "TASK_STATE_WORKING", "message": {
            "messageId": "m-1", "role": "ROLE_AGENT", "parts": [{ "text": "Looking." }, control] } });
        let task = |status: Value| json!({ "id": "tk-c", "contextId": "c", "status": status });
        let applied = params["message"]["parts"][0]["data"]["type"] == "control_applied";
        match method {
            Some("SendMessage") if applied => {
                let parts = json!([{ "text": "It opens at nine, and closes at six." }]);
                let mut done = task(json!({ "state": "TASK_STATE_COMPLETED" }));
                done["artifacts"] = json!([{ "artifactId": "a", "parts": parts }]);
                rpc_result(json!({ "task": done }))
            }
            Some("SendMessage" | "GetTask") => rpc_result(json!({ "task": task(working) })),
            _ => (404, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "store", "type": "a2a", "url": agent.url, "bearer_env": "X" });
    let rig = rig_on(backend(entry, Some("agent-token")), None).await;
    rig.ready().await;
    rig.turn("what are the store hours").await;
    rig.until_heard("It opens at nine, and closes at six.")
        .await;
    assert_eq!(
        controls_applied(&rig),
        [(
            "verbosity".into(),
            "detailed".into(),
            "agent".into(),
            "applied".into()
        )],
        "{}",
        rig.timeline()
    );
    let told: Vec<Value> = agent
        .bodies()
        .into_iter()
        .filter(|body| body["params"]["message"]["parts"][0]["data"]["type"] == "control_applied")
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    let message = &told[0]["params"]["message"];
    assert_eq!(message["taskId"], "tk-c");
    assert_eq!(message["parts"][0]["data"]["outcome"], "applied");
}

/// An MCP server with a session, reached with an OAuth client's token: the gateway fetches
/// the token (client credentials), runs `initialize` once and sends the agreed version, the
/// session and the token with each request; a token the server refuses is fetched again. An
/// elicitation the server writes on the reply of a call that is no MCP task is the caller's
/// question, and its answer goes back as the JSON-RPC response.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_mcp_server_is_initialized_with_an_oauth_token_and_asks_the_caller_on_a_reply() {
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let issued = Arc::new(Mutex::new(0u32));
    let count = issued.clone();
    let auth = BackendDouble::start(move |_: &Hit| {
        let mut issued = count.lock();
        *issued += 1;
        let token = format!("tok-{issued}");
        (
            200,
            json!({ "access_token": token, "token_type": "Bearer", "expires_in": 3600 })
                .to_string(),
        )
    })
    .await;
    let server = MockServer::start().await;
    let rpc = |result: Value| json!({ "jsonrpc": "2.0", "id": "1", "result": result });
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "initialize" })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Mcp-Session-Id", "sess-9")
                .set_body_json(rpc(json!({ "protocolVersion": "2026-07-28",
                                          "capabilities": { "tools": {} } }))),
        )
        .mount(&server)
        .await;
    // The first token is refused once: the gateway fetches another and tries again.
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "tools/call" })))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let schema = json!({ "type": "object", "properties": {
        "size": { "type": "string", "enum": ["small", "large"] } } });
    let ask = json!({ "jsonrpc": "2.0", "id": 7, "method": "elicitation/create", "params": {
        "mode": "form", "message": "Which size?", "requestedSchema": schema } });
    let done = rpc(json!({ "content": [{ "type": "text", "text": "A large one is in stock." }] }));
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "tools/call" })))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("data: {ask}\n\ndata: {done}\n\n"),
            "text/event-stream",
        ))
        .with_priority(2)
        .mount(&server)
        .await;
    // `notifications/initialized` and the answer to the elicitation.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202))
        .with_priority(10)
        .mount(&server)
        .await;
    let entry = json!({ "name": "stock", "type": "mcp", "url": format!("{}/mcp", server.uri()),
                        "tool": "find_item", "side_effects": "none",
                        "oauth": { "token_url": format!("{}/token", auth.url), "client_id": "waav",
                                   "client_secret_env": "STOCK_CLIENT_SECRET",
                                   "scope": "stock.read" } });
    let mut spec = backend(entry, None);
    if let Some(oauth) = spec.oauth.as_mut() {
        oauth.client_secret = Some("s3cret".to_owned());
    }
    let rig = rig_on(spec, None).await;
    rig.ready().await;
    rig.turn("do you have the blue kettle").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(
        (&question["kind"], &question["options"]),
        (&json!("choose"), &json!(["small", "large"])),
        "{}",
        rig.timeline()
    );
    rig.answer(&question, "large").await;
    rig.until_heard("A large one is in stock.").await;
    rig.until_event("task_result", 1).await;
    assert_eq!(rig.events("task_result")[0]["state"], "completed");

    let received = server.received_requests().await.expect("recorded");
    // The server's GET stream, opened after `initialize`, is not one of the calls.
    let parsed: Vec<(Value, std::collections::HashMap<String, String>)> = received
        .iter()
        .filter(|request| request.method.as_str() != "GET")
        .map(|request| {
            let headers = request.headers.iter();
            let headers = headers.map(|(name, value)| {
                let value = value.to_str().unwrap_or_default().to_owned();
                (name.as_str().to_lowercase(), value)
            });
            (
                serde_json::from_slice(&request.body).unwrap_or_default(),
                headers.collect(),
            )
        })
        .collect();
    let methods: Vec<Value> = parsed
        .iter()
        .map(|(body, _)| body["method"].clone())
        .collect();
    assert_eq!(
        methods,
        [
            json!("initialize"),
            json!("notifications/initialized"),
            json!("tools/call"),
            json!("tools/call"),
            Value::Null
        ],
        "{parsed:?}"
    );
    let (init, init_headers) = &parsed[0];
    assert_eq!(init["params"]["protocolVersion"], "2026-07-28");
    assert!(init["params"]["capabilities"]["elicitation"].is_object());
    assert!(!init_headers.contains_key("mcp-session-id"));
    assert_eq!(init_headers["authorization"], "Bearer tok-1");
    let (_, refused) = &parsed[2];
    assert_eq!(refused["authorization"], "Bearer tok-1");
    let (_, called) = &parsed[3];
    assert_eq!(
        (
            &called["authorization"],
            &called["mcp-session-id"],
            &called["mcp-protocol-version"]
        ),
        (
            &"Bearer tok-2".to_owned(),
            &"sess-9".to_owned(),
            &"2026-07-28".to_owned()
        )
    );
    let (answer, answer_headers) = &parsed[4];
    assert_eq!(
        answer,
        &json!({ "jsonrpc": "2.0", "id": 7,
                 "result": { "action": "accept", "content": { "size": "large" } } })
    );
    assert!(!answer_headers.contains_key("mcp-method"));
    assert_eq!(answer_headers["mcp-session-id"], "sess-9");
    // Two tokens, each fetched with the client's credentials.
    let fetched = auth.hits();
    assert_eq!(fetched.len(), 2);
    let basic = format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "waav:s3cret")
    );
    assert_eq!(fetched[0].call.headers["authorization"], basic);
    assert_eq!(
        fetched[0].call.headers["content-type"],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(
        fetched[0].call.raw,
        "grant_type=client_credentials&scope=stock.read"
    );
}

/// A request a server writes on a reply no session waits on (its tool list, read at an
/// orchestrator's start) has nobody to answer it: it is refused with the JSON-RPC error,
/// and the list is read.
#[tokio::test]
#[serial]
async fn an_elicitation_on_a_reply_no_session_waits_on_is_refused() {
    // The double listens on loopback.
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    let tool = json!({ "name": "find_item", "description": "Finds an item.",
                       "inputSchema": { "type": "object", "properties": {} } });
    let server = BackendDouble::start(move |hit: &Hit| match hit.call.body["method"].as_str() {
        Some("tools/list") => {
            let ask = json!({ "jsonrpc": "2.0", "id": "e-1", "method": "elicitation/create",
                "params": { "mode": "form", "message": "Who are you?",
                            "requestedSchema": { "type": "object", "properties": {
                                "name": { "type": "string" } } } } });
            let listed = json!({ "jsonrpc": "2.0", "id": "1", "result": { "tools": [tool] } });
            (200, format!("data: {ask}\n\ndata: {listed}\n\n"))
        }
        _ => (202, String::new()),
    })
    .await;
    let entry = json!({ "name": "stock", "type": "mcp", "url": server.url, "tools": ["*"] });
    let spec = backend(entry, None);
    remote::discover(&[&spec]).await;
    let tools = remote::agent_tools(&[&spec]);
    assert_eq!(tools.len(), 1, "the list is read");
    let refusal = server
        .bodies()
        .into_iter()
        .find(|body| body["id"] == "e-1")
        .expect("the request is answered");
    assert_eq!(refusal["error"]["code"], -32601);
    assert!(refusal.get("result").is_none());
}

/// An A2A agent whose card says it pushes and has an extended card: the session's start
/// reads the extended card with the entry's credential (its security scheme is then the
/// one used), the task's first message carries a push configuration for the task's callback
/// route, and the agent's pushes are the task's events; a push with another token is
/// refused, and the task is read only now and then meanwhile.
#[tokio::test(start_paused = true)]
#[serial]
async fn an_a2a_agent_pushes_its_tasks_updates_and_its_extended_card_is_read() {
    let agent = BackendDouble::start(|hit: &Hit| {
        if hit.method == "GET" {
            let card = json!({ "name": "Calendar", "description": "Public.",
                "capabilities": { "pushNotifications": true, "extendedAgentCard": true } });
            return (200, card.to_string());
        }
        let task = json!({ "id": "tk-p", "contextId": "c",
                           "status": { "state": "TASK_STATE_WORKING" } });
        match hit.call.body["method"].as_str() {
            Some("GetExtendedAgentCard")
                if hit.call.headers.get("authorization").map(String::as_str)
                    == Some("Bearer agent-token") =>
            {
                rpc_result(json!({ "name": "Calendar", "description": "Private.",
                    "capabilities": { "pushNotifications": true },
                    "securitySchemes": { "key": { "apiKeySecurityScheme": {
                        "location": "header", "name": "X-Agent-Key" } } } }))
            }
            Some("SendMessage") => rpc_result(json!({ "task": task })),
            Some("GetTask") => rpc_result(task),
            _ => (401, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "calendar", "type": "a2a", "url": agent.url, "bearer_env": "X",
                        "push_base_url": "https://gateway.example.com/" });
    let rig = rig_on(backend(entry, Some("agent-token")), None).await;
    rig.ready().await;
    rig.turn("is there a free slot next week").await;
    let task_id = rig.task(1).await;
    rig.until("the agent has the task", 5_000, |_| {
        agent
            .bodies()
            .iter()
            .any(|body| body["method"] == "SendMessage")
    })
    .await;
    let hits = agent.hits();
    let sent = hits
        .iter()
        .find(|hit| hit.call.body["method"] == "SendMessage");
    let sent = sent.expect("the task").call.clone();
    // The extended card's scheme: the key in its own header.
    assert_eq!(sent.headers["x-agent-key"], "agent-token");
    assert!(!sent.headers.contains_key("authorization"));
    let push = sent.body["params"]["configuration"]["pushNotificationConfig"].clone();
    assert_eq!(
        push["url"],
        format!("https://gateway.example.com/v1/tasks/{task_id}/callback")
    );
    let token = push["token"].as_str().expect("a token").to_owned();
    assert_eq!(token.len(), 64);

    let pushed = |token: &str, body: Value| {
        let sender = remote::CallbackFrom {
            token: Some(token),
            ..remote::CallbackFrom::default()
        };
        remote::callback(&task_id, sender, &body.to_string())
    };
    let working = json!({ "statusUpdate": { "taskId": "tk-p", "status": {
        "state": "TASK_STATE_WORKING", "message": { "parts": [{ "text": "Checking the calendar." }] } } } });
    assert_eq!(
        pushed("not-the-token", working.clone()),
        Err(CallbackRefused::Unsigned)
    );
    assert_eq!(pushed(&token, working), Ok(()));
    rig.until("the push is the task's progress", 5_000, |rig| {
        rig.events("task_progress")
            .iter()
            .any(|event| event["text"] == "Checking the calendar.")
    })
    .await;
    let parts = json!([{ "text": "Tuesday at ten is free." }]);
    let done = json!({ "task": { "id": "tk-p", "contextId": "c",
        "status": { "state": "TASK_STATE_COMPLETED" },
        "artifacts": [{ "artifactId": "a1", "parts": parts }] } });
    assert_eq!(pushed(&token, done), Ok(()));
    rig.until_heard("Tuesday at ten is free.").await;
    rig.until_event("task_result", 1).await;
    assert_eq!(rig.events("task_result")[0]["state"], "completed");
    let reads = agent
        .bodies()
        .iter()
        .filter(|body| body["method"] == "GetTask")
        .count();
    assert!(reads <= 3, "{reads} reads of a task that pushes");
    // The run is over: its token no longer reaches anything.
    assert_eq!(
        pushed(&token, json!({ "task": {} })),
        Err(CallbackRefused::UnknownTask)
    );
}

// ---- when services fail (plan chapter 10)

/// What names each failure line the caller heard (`interaction_updated.failure`), in order.
fn failure_notes(rig: &Rig) -> Vec<Value> {
    let updates = rig.events("interaction_updated").into_iter();
    updates
        .filter_map(|event| Some(event["failure"].clone()).filter(Value::is_object))
        .collect()
}

fn failure_kinds(rig: &Rig) -> Vec<String> {
    let kind = |note: Value| note["kind"].as_str().unwrap_or_default().to_owned();
    failure_notes(rig).into_iter().map(kind).collect()
}

/// When each failure line started to play (`audio_segment` of kind `failure`).
fn failure_starts(rig: &Rig) -> Vec<Ms> {
    let start = |event: &Value| {
        event["type"] == "audio_segment" && event["kind"] == "failure" && event["phase"] == "start"
    };
    rig.wire()
        .into_iter()
        .filter(|(_, event)| start(event))
        .map(|(at, _)| at)
        .collect()
}

/// The loudest frame the client got in `(after, after + ms]`.
fn peak(rig: &Rig, after: Ms, ms: Ms) -> i16 {
    let frames = rig.frames_in(after, after + ms);
    frames.iter().map(|(_, peak)| *peak).max().unwrap_or(0)
}

/// Sample `i` of a 400 Hz tone at 16 kHz (it survives the resampling to the session's rate).
fn tone(amplitude: i16, i: usize) -> i16 {
    let phase = std::f32::consts::TAU * 400.0 * i as f32 / 16_000.0;
    (f32::from(amplitude) * phase.sin()) as i16
}

/// Whether the loudest frame is about `amplitude` (a tone resampled to the session's rate).
fn about(peak: i16, amplitude: i16) -> bool {
    (f32::from(peak) - f32::from(amplitude)).abs() <= f32::from(amplitude) * 0.06
}

/// When the first failure line started; the test fails with the timeline when none did.
fn first_failure(rig: &Rig) -> Ms {
    let starts = failure_starts(rig);
    *starts
        .first()
        .unwrap_or_else(|| panic!("no failure line played\n{}", rig.timeline()))
}

/// The server's default set for one test: removed when the test ends, however it ends.
struct DefaultSetFor;

impl DefaultSetFor {
    /// Every line of the default persona, as a tone at `amplitude` of `ms`.
    fn install(amplitude: i16, ms: usize) -> DefaultSetFor {
        let tone: Vec<u8> = (0..16 * ms)
            .flat_map(|i| tone(amplitude, i).to_le_bytes())
            .collect();
        let audio = AudioData {
            data: tone,
            sample_rate: 16_000,
            format: "pcm".to_owned(),
            duration_ms: None,
        };
        let lines = Persona::load("default").failure_lines();
        clips::install_default_set(Some(clips::DefaultSet {
            voice: clips::Voice {
                provider: "server-default".to_owned(),
                voice: "default".to_owned(),
                language: "en".to_owned(),
            },
            lines: lines
                .into_iter()
                .map(|line| (line, audio.clone()))
                .collect(),
        }));
        DefaultSetFor
    }
}

impl Drop for DefaultSetFor {
    fn drop(&mut self) {
        clips::install_default_set(None);
    }
}

const STALL_1: &str = "Sorry, my systems are acting up. Bear with me a moment.";

#[tokio::test(start_paused = true)]
#[serial]
async fn a_stalled_answer_gets_a_stall_line_then_the_answer() {
    let rig = Rig::start(Opts::default()).await;
    // Nothing was rendered in this voice before, and the server has no default set.
    assert_eq!(rig.handle.echo()["failure"]["lines"], "none");
    assert_eq!(rig.handle.echo()["failure"]["stall_after_ms"], 2_500);
    rig.ready().await;
    // Made once at session start in the session's voice, never played there.
    let spoken = rig.tts.spoken();
    for line in Persona::load("default").failure_lines() {
        assert!(spoken.contains(&line.text), "never rendered: {}", line.text);
    }
    assert!(failure_starts(&rig).is_empty());

    // The background's first token comes 5 s after the request: a stall line at 2.5 s, the
    // back line once it answers, then the answer.
    rig.llm.push(answer(5_000));
    rig.turn("what time does the store open").await;
    let e = rig.now();
    rig.until_heard("It closes at six.").await;
    assert_eq!(failure_kinds(&rig), ["stall", "back"], "{}", rig.timeline());
    let notes = failure_notes(&rig);
    assert_eq!(
        (&notes[0]["line"], &notes[0]["service"], &notes[0]["text"]),
        (&json!("stall-1"), &json!("background"), &json!(STALL_1))
    );
    assert_eq!(notes[1]["line"], "back");
    let starts = failure_starts(&rig);
    assert!(
        starts[0] >= e + 2_500 && starts[0] < e + 3_000,
        "E {e}, the stall at {}\n{}",
        starts[0],
        rig.timeline()
    );
    let answered = rig.time_of("the answer", says("The store opens at nine."));
    assert!(
        starts[1] < answered,
        "the back line comes before the answer"
    );
    // The stall line is heard at speech level, not as a listening sound.
    assert_eq!(peak(&rig, starts[0], 500), ix_mocks::TTS_AMPLITUDE);
    // While it lasted the client saw what is degraded; once it was over, nothing.
    let updates = rig.events("interaction_updated");
    assert_eq!(
        notes[0],
        updates.iter().find(|u| u["failure"].is_object()).unwrap()["failure"]
    );
    let stalled = updates
        .iter()
        .position(|u| u["degraded"] == json!(["background_stalled"]));
    assert!(stalled.is_some(), "{updates:?}");
    assert!(updates.last().unwrap()["degraded"].is_null(), "{updates:?}");
    // The history has what was said, as notes.
    rig.llm.push(answer(300));
    rig.turn("and when does it close").await;
    rig.until_event("task_result", 2).await;
    let requests = rig.llm.requests();
    let users = ix_mocks::messages(requests.last().unwrap(), "user").join("\n");
    assert!(
        users.contains(&format!(
            "[the background failed; the agent said: {STALL_1}]"
        )) && users
            .contains("[the background is back; the agent said: Thanks for waiting, I'm back.]"),
        "{users}"
    );
}

/// The first-token deadline holds for every background, not only the session's own LLM: a DAG
/// whose first text comes 5 s after the request gets a stall line; a webhook that takes the
/// task at once and answers by callback 6 s later is working, and gets none.
#[tokio::test(start_paused = true)]
#[serial]
async fn every_background_has_a_first_token_deadline() {
    let graph = Arc::new(Graph {
        wait: 5_000,
        ..Graph::default()
    });
    graph
        .replies
        .lock()
        .push_back(Ok(vec!["The store opens at nine.", "It closes at six."]));
    let rig = Rig::start(Opts {
        graph: Some(graph.clone()),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    rig.turn("what time does the store open").await;
    let e = rig.now();
    rig.until_heard("It closes at six.").await;
    assert_eq!(failure_kinds(&rig), ["stall", "back"], "{}", rig.timeline());
    assert_eq!(failure_notes(&rig)[0]["service"], "background");
    let starts = failure_starts(&rig);
    assert!(
        starts[0] >= e + 2_500 && starts[0] < e + 3_000,
        "E {e}, the stall at {}\n{}",
        starts[0],
        rig.timeline()
    );
    drop(rig);

    let hook = BackendDouble::start(|_: &Hit| (200, json!({}).to_string())).await;
    let entry = json!({ "name": "crm", "type": "webhook", "url": hook.url });
    let rig = rig_on(backend(entry, Some(HOOK_SECRET)), None).await;
    rig.ready().await;
    rig.turn("where is my order").await;
    let task = rig.task(1).await;
    rig.until("the webhook has the task", 5_000, |_| {
        !hook.hits().is_empty()
    })
    .await;
    rig.run(6_000).await;
    let result = json!({ "type": "result", "text": "It ships on Monday." });
    call_back(&task, HOOK_SECRET, result).expect("the callback is taken");
    rig.until_heard("It ships on Monday.").await;
    assert!(failure_kinds(&rig).is_empty(), "{}", rig.timeline());
}

#[tokio::test(start_paused = true)]
#[serial]
async fn stall_lines_are_spaced_rotated_and_capped() {
    let hook = ToolDouble::start(json!({})).await;
    let rig = Rig::start(Opts {
        operator: Some(json!({ "tasks": { "push_url": hook.url } })),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    // The background never answers in the call.
    rig.llm.push(answer(120_000));
    rig.turn("what time does the store open").await;
    rig.until("the session fails", 60_000, |rig| {
        !rig.events("session_failed").is_empty()
    })
    .await;
    let notes = failure_notes(&rig);
    let ids: Vec<&str> = notes
        .iter()
        .map(|note| note["line"].as_str().unwrap())
        .collect();
    // Three stall lines, each another, then the exit line.
    assert_eq!(
        ids,
        ["stall-1", "stall-2", "stall-3", "exit-follow-up"],
        "{}",
        rig.timeline()
    );
    let starts = failure_starts(&rig);
    assert!(
        starts[1] >= starts[0] + 8_000 && starts[2] >= starts[1] + 8_000,
        "{starts:?}"
    );
    assert!(starts[3] >= starts[0] + 30_000, "{starts:?}");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn an_outage_past_max_wait_plays_the_exit_line_and_ends_the_session() {
    let hook = ToolDouble::start(json!({})).await;
    let rig = Rig::start(Opts {
        operator: Some(
            json!({ "tasks": { "push_url": hook.url }, "failure": { "max_wait_s": 5 } }),
        ),
        ..Opts::default()
    })
    .await;
    assert_eq!(rig.handle.echo()["failure"]["follow_up"], true);
    rig.ready().await;
    rig.llm.push(answer(120_000));
    rig.turn("what time does the store open").await;
    rig.until("the socket is closed", 30_000, |rig| {
        !rig.events("close").is_empty()
    })
    .await;
    assert_eq!(
        failure_kinds(&rig),
        ["stall", "exit_follow_up"],
        "{}",
        rig.timeline()
    );
    // What was pending, and why; then the socket closes.
    let failed = &rig.events("session_failed")[0];
    let task = &rig.events("task_created")[0];
    assert_eq!(failed["service"], "background");
    assert_eq!(failed["follow_up"], true);
    assert_eq!(failed["pending"][0]["task_id"], task["task_id"]);
    assert_eq!(failed["pending"][0]["state"], "working");
    let at = |kind: &str| rig.time_of(kind, |event| event["type"] == kind);
    let exit_started = failure_starts(&rig)[1];
    assert!(exit_started <= at("session_failed") && at("session_failed") <= at("close"));
    assert_eq!(rig.events("close")[0]["code"], 1011);
    // The exit line played out before the session ended.
    let failure = |phase: &str| {
        let of = |event: &Value| event["kind"] == "failure" && event["phase"] == phase;
        rig.events("audio_segment")
            .into_iter()
            .filter(of)
            .collect::<Vec<_>>()
    };
    let exit_id = failure("start")[1]["utterance_id"].clone();
    let exit_ended = rig.time_of("the exit line's end", |event| {
        event["type"] == "audio_segment"
            && event["phase"] == "end"
            && event["utterance_id"] == exit_id
    });
    assert!(exit_ended >= exit_started + 4_000, "{}", rig.timeline());
    assert!(at("session_failed") >= exit_ended, "{}", rig.timeline());
    // The signed post-call push carries the same.
    let calls = pushed(&hook, 1, 10_000).await;
    let push = &calls[0];
    assert!(push.signed_with(TOOL_SECRET));
    assert_eq!(push.body["type"], "session_failed");
    assert_eq!(push.body["stream_id"], json!(rig.stream_id));
    assert_eq!(push.body["pending"], failed["pending"]);
    assert!(push.body.get("t_ms").is_none());
    let key = format!("{}:session_failed", rig.stream_id);
    assert_eq!(push.headers.get("idempotency-key"), Some(&key));
    // The task runs on: its result has a reader (the push address).
    let stored = tasks::store()
        .get(&rig.owner, task["task_id"].as_str().unwrap())
        .await;
    assert_eq!(stored.expect("stored").view.state, TaskState::Working);
    assert_eq!(rig.events("session_failed").len(), 1);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn the_exit_line_promises_a_follow_up_only_when_there_is_one() {
    let rig = Rig::start(Opts {
        operator: Some(json!({ "failure": { "max_wait_s": 5 } })),
        ..Opts::default()
    })
    .await;
    assert_eq!(rig.handle.echo()["failure"]["follow_up"], false);
    rig.ready().await;
    rig.llm.push(answer(120_000));
    rig.turn("what time does the store open").await;
    rig.until("the session fails", 30_000, |rig| {
        !rig.events("session_failed").is_empty()
    })
    .await;
    let notes = failure_notes(&rig);
    let exit = notes.last().unwrap();
    assert_eq!(exit["line"], "exit", "{}", rig.timeline());
    assert!(
        exit["text"]
            .as_str()
            .unwrap()
            .ends_with("Please call back a little later.")
    );
    assert_eq!(rig.events("session_failed")[0]["follow_up"], false);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_failing_voice_still_plays_its_recorded_stall_line() {
    let dir = std::env::temp_dir().join(format!("waav-ix-failure-{}", std::process::id()));
    let rig = Rig::start(Opts {
        clips_dir: Some(dir.display().to_string()),
        ..Opts::default()
    })
    .await;
    // The operator recorded the first stall line in this voice, louder than the voice.
    let at = dir.join(&rig.tts.provider).join("default/en/failure");
    std::fs::create_dir_all(&at).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = hound::WavWriter::create(at.join("stall-1.wav"), spec).unwrap();
    for i in 0..16 * 1_200 {
        wav.write_sample(tone(12_000, i)).unwrap();
    }
    wav.finalize().unwrap();
    rig.ready().await;
    let spoken = rig.tts.spoken();
    assert!(
        !spoken.contains(&STALL_1.to_owned()),
        "the recording is not rendered"
    );
    // The rest were rendered, and kept under the directory for the next session.
    let cached = std::fs::read_dir(at.join("cache"))
        .map(|dir| dir.count())
        .unwrap_or(0);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(cached, 8, "{spoken:?}");

    // From now on every synthesis fails: the acknowledgement, the apology, the answer.
    rig.tts.fail_next(1_000);
    rig.llm.push(answer(400));
    rig.turn("what time does the store open").await;
    let e = rig.now();
    rig.until("a failure line", 10_000, |rig| {
        !failure_starts(rig).is_empty()
    })
    .await;
    let notes = failure_notes(&rig);
    assert_eq!(
        (&notes[0]["kind"], &notes[0]["service"]),
        (&json!("stall"), &json!("voice"))
    );
    let start = failure_starts(&rig)[0];
    assert!(
        start >= e + 2_500,
        "a single error that the next try mends says nothing"
    );
    // The caller hears the recording, pre-made: no synthesis was asked for it.
    rig.run(600).await;
    assert!(about(peak(&rig, start, 500), 12_000), "{}", rig.timeline());
    assert!(!rig.tts.spoken().contains(&STALL_1.to_owned()));
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_line_from_the_speech_cache_is_no_sign_that_the_voice_is_back() {
    let rig = Rig::start(Opts {
        operator: Some(json!({ "failure": { "max_wait_s": 5 } })),
        ..Opts::default()
    })
    .await;
    rig.ready().await;
    // The vendor is down: every synthesis fails, but the answer's sentences were said before
    // and are in the speech cache (live SF3 with ElevenLabs behind a dead proxy).
    rig.tts.fail_next(1_000);
    rig.tts.cache("The store opens at nine.");
    rig.tts.cache("It closes at six.");
    rig.llm.push(answer(4_000));
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;
    rig.until("the session fails", 30_000, |rig| {
        !rig.events("session_failed").is_empty()
    })
    .await;
    assert_eq!(rig.events("session_failed")[0]["service"], "voice");
    let kinds = failure_kinds(&rig);
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("stall"),
        "{}",
        rig.timeline()
    );
    assert!(!kinds.iter().any(|kind| kind == "back"), "{kinds:?}");
    assert_eq!(kinds.last().map(String::as_str), Some("exit"), "{kinds:?}");
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_failing_recogniser_plays_the_hearing_line() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    // The caller speaks for 3.6 s and the streaming recogniser gives nothing.
    let onset = rig.onset().await;
    rig.run(3_600).await;
    assert!(failure_starts(&rig).is_empty(), "never over the caller");
    rig.offset(3_600).await;
    // The turn ends with no words: the speech-final that will not come.
    rig.send(Msg::SttEmpty).await;
    rig.run(1_000).await;
    let notes = failure_notes(&rig);
    assert_eq!(
        (&notes[0]["kind"], &notes[0]["service"], &notes[0]["line"]),
        (&json!("hearing"), &json!("recogniser"), &json!("hearing-1")),
        "{}",
        rig.timeline()
    );
    assert!(first_failure(&rig) >= onset + 3_600);
    // The recogniser answers again: the back line, and the turn goes on.
    rig.llm.push(answer(300));
    rig.turn("what time does the store open").await;
    rig.until_heard("It closes at six.").await;
    assert_eq!(
        failure_kinds(&rig),
        ["hearing", "back"],
        "{}",
        rig.timeline()
    );
}

#[tokio::test(start_paused = true)]
#[serial]
async fn failure_lines_never_start_over_the_caller() {
    let rig = Rig::start(Opts::default()).await;
    rig.ready().await;
    rig.llm.push(answer(8_000));
    rig.turn("what time does the store open").await;
    // The caller goes on from E + 1.5 s to E + 3.5 s: the stall due at E + 2.5 s waits.
    rig.run(1_500).await;
    let onset = rig.onset().await;
    rig.run(2_000).await;
    assert!(failure_starts(&rig).is_empty(), "{}", rig.timeline());
    rig.offset(2_000).await;
    // A sound with no words: the turn ends with nothing said.
    rig.send(Msg::SttEmpty).await;
    rig.until("a stall line", 5_000, |rig| !failure_starts(rig).is_empty())
        .await;
    let start = first_failure(&rig);
    assert!(start >= onset + 2_000);
    // A caller who speaks over a stall line fades it out at once.
    rig.run(300).await;
    rig.onset().await;
    let cut = rig.now();
    rig.run(400).await;
    assert!(peak(&rig, cut + 200, 200) == 0, "{}", rig.timeline());
    assert!(peak(&rig, start, 300) > 0);
}

#[tokio::test(start_paused = true)]
#[serial]
async fn a_silent_or_long_failure_line_is_replaced_by_the_default_set() {
    let _set = DefaultSetFor::install(20_000, 1_500);
    let rig = Rig::start(Opts::default()).await;
    assert_eq!(rig.handle.echo()["failure"]["lines"], "default");
    // The session's voice renders the first stall line as silence, and one exit line too long.
    rig.tts.silence(STALL_1);
    rig.ready().await;
    rig.llm.push(answer(5_000));
    rig.turn("what time does the store open").await;
    rig.until("a stall line", 5_000, |rig| !failure_starts(rig).is_empty())
        .await;
    let start = failure_starts(&rig)[0];
    rig.run(600).await;
    // The default set's line, not the session voice's silence.
    assert!(about(peak(&rig, start, 500), 20_000), "{}", rig.timeline());
}

// ---- r3-proto: A2A bindings and OAuth from the card, MCP's GET stream and control wire,
// a client's own push

/// A session's own A2A backend: its card is read at the start (no `push_base_url` needed) and
/// says it is reached over HTTP+JSON with an OAuth 2.0 client-credentials token from the token
/// endpoint the card names.
#[tokio::test]
#[serial]
async fn a_default_a2a_backends_card_names_its_binding_and_its_oauth_token_endpoint() {
    let agent = BackendDouble::start(|hit: &Hit| {
        let host = hit.call.headers.get("host").cloned().unwrap_or_default();
        let task = |state: &str| json!({ "id": "tk-r", "contextId": "c-r", "status": { "state": state } });
        match (hit.method.as_str(), hit.path.as_str()) {
            ("GET", "/guide/.well-known/agent-card.json") => {
                let card = json!({ "name": "Guide", "description": "Knows the museum.",
                    "supportedInterfaces": [
                        { "url": format!("http://{host}/rest"), "protocolBinding": "HTTP+JSON" }],
                    "securitySchemes": { "o": { "oauth2SecurityScheme": { "flows": {
                        "clientCredentials": { "tokenUrl": format!("http://{host}/token"),
                                               "scopes": { "guide": "" } } } } } } });
                (200, card.to_string())
            }
            ("POST", "/token") => (
                200,
                json!({ "access_token": "tok-guide", "expires_in": 3600 }).to_string(),
            ),
            ("POST", "/rest/message:send") => (200, json!({ "task": task("TASK_STATE_WORKING") }).to_string()),
            ("GET", "/rest/tasks/tk-r") => {
                let mut done = task("TASK_STATE_COMPLETED");
                done["artifacts"] = json!([{ "artifactId": "a", "parts": [{ "text": "The museum opens at ten." }] }]);
                (200, done.to_string())
            }
            _ => (404, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "guide", "type": "a2a", "url": format!("{}/guide", agent.url),
                        "side_effects": "none",
                        "oauth": { "client_id": "waav", "client_secret_env": "GUIDE_SECRET" } });
    let mut spec = backend(entry, None);
    if let Some(oauth) = spec.oauth.as_mut() {
        oauth.client_secret = Some("s3cret".to_owned());
    }
    let rig = rig_on(spec, None).await;
    rig.ready().await;
    rig.turn("when does the museum open").await;
    rig.until_heard("The museum opens at ten.").await;
    rig.until_event("task_result", 1).await;
    assert_eq!(rig.events("task_result")[0]["state"], "completed");
    let hits = agent.hits();
    let paths: Vec<(String, String)> = hits
        .iter()
        .map(|hit| (hit.method.clone(), hit.path.clone()))
        .collect();
    let at = |method: &str, path: &str| {
        hits.iter()
            .find(|hit| hit.method == method && hit.path == path)
            .unwrap_or_else(|| panic!("no {method} {path} in {paths:?}"))
    };
    // The client credentials grant, with the client's own credentials.
    let token = at("POST", "/token");
    let basic = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "waav:s3cret");
    assert_eq!(
        token.call.headers["authorization"],
        format!("Basic {basic}")
    );
    assert!(
        token.call.raw.contains("grant_type=client_credentials"),
        "{}",
        token.call.raw
    );
    // The task on the HTTP+JSON binding, with the token as a bearer.
    let sent = at("POST", "/rest/message:send");
    assert_eq!(sent.call.headers["authorization"], "Bearer tok-guide");
    assert_eq!(sent.call.headers["a2a-version"], "1.0");
    assert_eq!(
        sent.call.body["message"]["parts"][0]["text"],
        "when does the museum open"
    );
    assert_eq!(sent.call.body["configuration"]["returnImmediately"], true);
    assert_eq!(
        at("GET", "/rest/tasks/tk-r").call.headers["authorization"],
        "Bearer tok-guide"
    );
    // Nothing went to the JSON-RPC address of the entry.
    assert!(
        !paths
            .iter()
            .any(|(method, path)| method == "POST" && path == "/guide"),
        "{paths:?}"
    );
}

/// An MCP server asks the caller through its GET stream (an elicitation about the call's task,
/// outside the call's reply), and its task's result asks for a conversation control, whose
/// `control_applied` goes back to it as a notification.
#[tokio::test]
#[serial]
async fn an_mcp_server_asks_on_its_get_stream_and_its_result_asks_for_a_control() {
    let called = Arc::new(Mutex::new((false, false, false)));
    let state = called.clone();
    let server = BackendDouble::start(move |hit: &Hit| {
        let rpc = |result: Value| json!({ "jsonrpc": "2.0", "id": hit.call.body["id"], "result": result }).to_string();
        let mut state = state.lock();
        let (called, asked, answered) = &mut *state;
        if hit.method == "GET" {
            if *called && !*asked {
                *asked = true;
                let schema = json!({ "type": "object", "properties": {
                    "list": { "type": "string", "enum": ["personal", "work"] } } });
                let ask = json!({ "jsonrpc": "2.0", "id": 7, "method": "elicitation/create", "params": {
                    "mode": "form", "message": "Which list?", "requestedSchema": schema,
                    "_meta": { "io.modelcontextprotocol/related-task": { "taskId": "tk-m" } } } });
                return (200, format!("data: {ask}\n\n"));
            }
            return (200, "data: {}\n\n".to_owned());
        }
        let body = &hit.call.body;
        if body["id"] == 7 && body.get("result").is_some() {
            *answered = true;
            return (200, "{}".to_owned());
        }
        match body["method"].as_str() {
            Some("initialize") => (200, rpc(json!({ "protocolVersion": "2026-07-28", "capabilities": {} }))),
            Some("tools/call") => {
                *called = true;
                (200, rpc(json!({ "taskId": "tk-m", "status": "working", "pollIntervalMs": 1000 })))
            }
            Some("tasks/get") if *answered => (200, rpc(json!({ "taskId": "tk-m", "status": "completed",
                "result": { "content": [{ "type": "text", "text": "Added to your personal list." }],
                            "_meta": { "io.waav/control": { "kind": "verbosity", "value": "brief",
                                                             "request_id": "c-9" } } } }))),
            Some("tasks/get") => (200, rpc(json!({ "taskId": "tk-m", "status": "working" }))),
            _ => (200, "{}".to_owned()),
        }
    })
    .await;
    let entry = json!({ "name": "lists", "type": "mcp", "url": format!("{}/mcp", server.url),
                        "tool": "add_item", "side_effects": "none" });
    let rig = rig_on(backend(entry, Some("lists-token")), None).await;
    rig.ready().await;
    rig.turn("add milk to my list").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    assert_eq!(
        (&question["kind"], &question["options"]),
        (&json!("choose"), &json!(["personal", "work"])),
        "{}",
        rig.timeline()
    );
    rig.answer(&question, "personal").await;
    rig.until_heard("Added to your personal list.").await;
    assert_eq!(
        controls_applied(&rig),
        [(
            "verbosity".into(),
            "brief".into(),
            "agent".into(),
            "applied".into()
        )],
        "{}",
        rig.timeline()
    );
    let hits = server.hits();
    // The answer went back as the JSON-RPC response of the request's id.
    let answer = hits
        .iter()
        .find(|hit| hit.call.body["id"] == 7)
        .expect("the answer");
    assert_eq!(
        answer.call.body["result"],
        json!({ "action": "accept", "content": { "list": "personal" } })
    );
    assert_eq!(answer.call.headers["authorization"], "Bearer lists-token");
    // The GET stream carried the bearer and asked for events.
    let get = hits
        .iter()
        .find(|hit| hit.method == "GET")
        .expect("the GET stream");
    assert_eq!(get.call.headers["authorization"], "Bearer lists-token");
    assert!(get.call.headers["accept"].contains("text/event-stream"));
    // How the control went, as a notification about the task.
    let applied = hits
        .iter()
        .find(|hit| hit.call.body["method"] == "notifications/io.waav/control_applied")
        .expect("the server hears how its control went");
    let params = &applied.call.body["params"];
    assert_eq!(
        (&params["request_id"], &params["outcome"], &params["kind"]),
        (&json!("c-9"), &json!("applied"), &json!("verbosity"))
    );
    assert_eq!(
        params["_meta"]["io.modelcontextprotocol/related-task"]["taskId"],
        "tk-m"
    );
}

/// Where the operator allows it, a session pushes to its own address, signed with its own
/// secret: never with the server's.
#[tokio::test(start_paused = true)]
#[serial]
async fn a_sessions_own_push_is_signed_with_its_own_secret() {
    const OWN: &str = "the-clients-own-push-secret";
    let tool = ToolDouble::start(json!({ "effect": "held a table for two" })).await;
    let hook = ToolDouble::start(json!({})).await;
    tool.delay(3_000);
    let operator = json!({ "tasks": {
        "tools": [{
            "name": "hold_table", "description": "Use it to hold a table.", "url": tool.url,
            "parameters": { "type": "object", "properties": {} },
            "side_effects": true, "confirm_phrase": "hold a table",
        }],
        "reconnect_grace_s": 60,
    } });
    let rig = Rig::start(Opts {
        operator: Some(operator),
        interaction: Some(json!({ "tasks": { "push_url": hook.url, "push_secret": OWN } })),
        ..Opts::default()
    })
    .await;
    rig.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(600, "hold_table", json!({ "party": 2 })),
            _ => Round::text(50, 20, &["ANSWER:\n", HELD]),
        })
    });
    rig.ready().await;
    rig.turn("can you hold a table for two").await;
    rig.until_event("task_question", 1).await;
    let question = rig.events("task_question")[0].clone();
    rig.send(Msg::TaskAnswer {
        request_id: question["request_id"].as_str().expect("an id").to_owned(),
        outcome: HitlOutcome::Accept { value: None },
    })
    .await;
    rig.until("the call is on the wire", 5_000, |_| {
        tool.calls().len() == 1
    })
    .await;
    let task_id = question["task_id"].as_str().expect("a task id").to_owned();
    rig.handle.shutdown().await;
    let calls = pushed(&hook, 1, 10_000).await;
    let push = &calls[0];
    assert!(
        push.signed_with(OWN),
        "signed with the session's own secret"
    );
    assert!(!push.signed_with(TOOL_SECRET));
    assert_eq!(push.body["task_id"], task_id);
    assert_eq!(push.body["state"], "completed");
    // The address and the secret stay in this process: the shared record has neither.
    let stored = tasks::store()
        .get(&rig.owner, &task_id)
        .await
        .expect("stored");
    let record = serde_json::to_string(&stored).unwrap_or_default();
    assert!(
        !record.contains(OWN) && !record.contains(&hook.url),
        "{record}"
    );
}
