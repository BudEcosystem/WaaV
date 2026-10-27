//! Interaction mode over a real `/ws` socket, against the test doubles.
//!
//! Three things are checked here and nowhere else: a session that asks for the mode gets the
//! protocol 1.1 events in order through the real handler; a session that does not ask sends
//! the same frames whether the server switch is on or off; and a session that asks while the
//! switch is off gets one `config_warning` and otherwise the same frames.
//!
//! The clock is real: the gateway's own timers and the socket decide the order, as in
//! production. The margins between the scripted steps are several hundred milliseconds.
//!
//! The server block is process-wide, so every test holds [`LOCK`] and sets the block itself.

#[path = "mock_providers/interaction.rs"]
mod ix_mocks;

use std::sync::Arc;
use std::time::Duration;

use axum::middleware;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use ix_mocks::{
    BackendDouble, Hit, LlmDouble, Round, SttDouble, ToolDouble, TtsDouble, VendorDouble, ga,
};
use waav_gateway::core::alias::{AliasDefinition, global_aliases};
use waav_gateway::core::interaction::config::{BackendSpec, ServerInteraction};
use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    middleware::auth::auth_middleware,
    routes,
    state::AppState,
};

/// The front model double of every session here (chapter 9: interaction mode needs one), on a
/// thread of its own for the whole test process.
static FRONT_URL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async move {
            let front = ix_mocks::FrontDouble::start(ix_mocks::FrontScript {
                decide: Some(ix_mocks::reads_the_turn),
                ..ix_mocks::FrontScript::default()
            })
            .await;
            let _ = tx.send(front.url.clone());
            std::future::pending::<()>().await;
        });
    });
    rx.recv().expect("the front double")
});

/// The server's interaction block, with the front model double unless it names one.
fn set_server_for_tests(mut server: ServerInteraction) {
    server.front_model.get_or_insert_with(|| {
        waav_gateway::core::interaction::config::FrontModelServer {
            url: FRONT_URL.clone(),
            model: None,
            api_key: None,
            command: None,
            licence: None,
        }
    });
    waav_gateway::core::interaction::config::set_server_for_tests(server);
}

/// One test at a time: the server's interaction block is one value for the whole process.
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn switch(enabled: bool) {
    set_server_for_tests(ServerInteraction {
        enabled,
        ..ServerInteraction::default()
    });
}

/// The switch on, and a default block for the sessions that send none.
fn switch_on_with_default() {
    set_server_for_tests(ServerInteraction {
        enabled: true,
        default: Some(json!(true)),
        ..ServerInteraction::default()
    });
}

fn test_config() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        livekit_api_key: None,
        livekit_api_secret: None,
        deepgram_api_key: None,
        elevenlabs_api_key: None,
        google_credentials: None,
        azure_speech_subscription_key: None,
        azure_speech_region: None,
        cartesia_api_key: None,
        openai_api_key: None,
        azure_openai_api_key: None,
        azure_openai_endpoint: None,
        grok_api_key: None,
        inworld_api_key: None,
        gemini_api_key: None,
        ultravox_api_key: None,
        speechmatics_api_key: None,
        yandex_api_key: None,
        yandex_folder_id: None,
        assemblyai_api_key: None,
        hume_api_key: None,
        groq_api_key: None,
        ibm_watson_api_key: None,
        ibm_watson_instance_id: None,
        ibm_watson_region: None,
        aws_access_key_id: None,
        aws_secret_access_key: None,
        aws_region: None,
        gnani_token: None,
        gnani_access_key: None,
        gnani_certificate_path: None,
        recording_s3_bucket: None,
        recording_s3_region: None,
        recording_s3_endpoint: None,
        recording_s3_access_key: None,
        recording_s3_secret_key: None,
        recording_s3_prefix: None,
        cache_path: None,
        cache_ttl_seconds: Some(3600),
        auth_service_url: None,
        auth_signing_key_path: None,
        auth_api_secrets: Vec::new(),
        auth_timeout_seconds: 5,
        auth_required: false,
        sip: None,
        cors_allowed_origins: None,
        rate_limit_requests_per_second: 1000,
        rate_limit_burst_size: 1000,
        max_websocket_connections: None,
        max_connections_per_ip: 1000,
        ws_processing_timeout_secs: 10,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        realtime_endpoint_overrides: Default::default(),
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
        aliases: Default::default(),
    }
}

/// The gateway on a loopback port, with the doubles its sessions use.
struct Gateway {
    port: u16,
    tts: Arc<TtsDouble>,
    stt: Arc<SttDouble>,
    llm: LlmDouble,
    server: JoinHandle<()>,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Gateway {
    async fn start() -> Gateway {
        Gateway::start_with(|_| {}).await
    }

    /// With the server config edited first.
    async fn start_with(edit: impl FnOnce(&mut ServerConfig)) -> Gateway {
        // The LLM double listens on loopback.
        unsafe {
            std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
        }
        let mut config = test_config();
        edit(&mut config);
        let app_state = AppState::new(config).await;
        let auth = || middleware::from_fn_with_state(app_state.clone(), auth_middleware);
        let app = routes::api::create_api_router()
            .layer(auth())
            .merge(routes::ws::create_ws_router().layer(auth()))
            .merge(routes::realtime::create_realtime_router().layer(auth()))
            .merge(routes::webhooks::create_webhook_router())
            .with_state(app_state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Gateway {
            port,
            tts: TtsDouble::register(),
            stt: SttDouble::register(false),
            llm: LlmDouble::start().await,
            server,
        }
    }

    /// A `config` message for the doubles, with a conversation loop on the LLM double.
    fn config(&self, stream_id: &str) -> Value {
        json!({
            "type": "config",
            "stream_id": stream_id,
            "stt_config": {
                "provider": self.stt.provider, "api_key": "test", "language": "en-US",
                "sample_rate": 16000, "channels": 1, "punctuation": true,
            },
            "tts_config": {
                "provider": self.tts.provider, "api_key": "test", "voice_id": "voice",
                "audio_format": "pcm", "sample_rate": 16000,
            },
            "conversation_config": {
                "base_url": self.llm.base_url, "model": "mock", "api_key": "test-key",
            },
        })
    }

    async fn connect(&self) -> Client {
        self.connect_to("/ws").await
    }

    async fn connect_to(&self, path: &str) -> Client {
        let url = format!("ws://127.0.0.1:{}{path}", self.port);
        let (socket, _) = connect_async(&url).await.expect("connect");
        let (write, mut read) = socket.split();
        let frames = Arc::new(Mutex::new(Vec::new()));
        let seen = frames.clone();
        let reader = tokio::spawn(async move {
            while let Some(Ok(message)) = read.next().await {
                match message {
                    Message::Text(text) => {
                        let event = serde_json::from_str(&text).expect("a JSON frame");
                        seen.lock().push(Frame::Event(event));
                    }
                    Message::Binary(audio) => seen.lock().push(Frame::Audio(audio.len())),
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        });
        Client {
            write,
            frames,
            reader,
        }
    }

    /// One task, read by its id with the session it belongs to: the status and the body.
    async fn task(&self, task_id: &str, stream_id: &str) -> (u16, Value) {
        let url = format!(
            "http://127.0.0.1:{}/v1/tasks/{task_id}?stream_id={stream_id}",
            self.port
        );
        let response = reqwest::get(&url).await.expect("GET /v1/tasks/{task_id}");
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or_default())
    }

    /// The tasks of one session: with authentication off a caller must name it.
    async fn tasks(&self, stream_id: &str) -> Value {
        let url = format!(
            "http://127.0.0.1:{}/v1/tasks?stream_id={stream_id}",
            self.port
        );
        let response = reqwest::get(&url).await.expect("GET /v1/tasks");
        assert!(response.status().is_success(), "{}", response.status());
        response.json().await.expect("a JSON body")
    }
}

/// What the gateway sent on the socket.
#[derive(Debug)]
enum Frame {
    Event(Value),
    /// A binary frame, the agent's audio: its length in bytes.
    Audio(usize),
}

/// The client's side of one `/ws` session.
struct Client {
    write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    frames: Arc<Mutex<Vec<Frame>>>,
    reader: JoinHandle<()>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl Client {
    async fn send(&mut self, message: Value) {
        let text = message.to_string();
        self.write
            .send(Message::Text(text.into()))
            .await
            .expect("send");
    }

    /// Every text frame so far, in order.
    fn events(&self) -> Vec<Value> {
        let frames = self.frames.lock();
        let event = |frame: &Frame| match frame {
            Frame::Event(event) => Some(event.clone()),
            Frame::Audio(_) => None,
        };
        frames.iter().filter_map(event).collect()
    }

    fn of_type(&self, kind: &str) -> Vec<Value> {
        let of_kind = |event: &Value| event["type"] == kind;
        self.events().into_iter().filter(of_kind).collect()
    }

    /// Every frame so far, in order: the type of a text frame, `audio:<bytes>` for audio.
    fn types(&self) -> Vec<String> {
        let kind = |frame: &Frame| match frame {
            Frame::Event(event) => event["type"].as_str().unwrap_or_default().to_owned(),
            Frame::Audio(bytes) => format!("audio:{bytes}"),
        };
        self.frames.lock().iter().map(kind).collect()
    }

    /// Waits for the `count`-th event that matches; fails after ten seconds.
    async fn until_where(
        &self,
        what: &str,
        count: usize,
        matches: impl Fn(&Value) -> bool,
    ) -> Value {
        let arrived = async {
            loop {
                let found = self.events().into_iter().filter(&matches).nth(count - 1);
                if let Some(event) = found {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        match tokio::time::timeout(Duration::from_secs(10), arrived).await {
            Ok(event) => event,
            Err(_) => panic!("no {what} #{count}\n{}", self.timeline()),
        }
    }

    /// Waits for the `count`-th event of this type.
    async fn until(&self, kind: &str, count: usize) -> Value {
        self.until_where(kind, count, |event| event["type"] == kind)
            .await
    }

    /// Waits until the agent has said these words `count` times and all of the audio of the
    /// last time was delivered.
    async fn until_heard(&self, words: &str, count: usize) {
        let said =
            |event: &Value| event["type"] == "assistant_transcript" && event["delta"] == words;
        let line = self.until_where(words, count, said).await;
        let heard = |event: &Value| {
            event["utterance_id"] == line["utterance_id"] && event["heard"] == "full"
        };
        self.until_where("its end", 1, heard).await;
    }

    /// Closes the socket and waits for the gateway to close its side.
    async fn close(mut self) {
        let _ = self.write.send(Message::Close(None)).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut self.reader).await;
    }

    fn timeline(&self) -> String {
        let line = |frame: &Frame| format!("{frame:?}");
        let lines: Vec<String> = self.frames.lock().iter().map(line).collect();
        lines.join("\n")
    }
}

/// The index after the first event at or after `from` that matches; the test fails when there
/// is none. Chained calls assert an order.
fn next(events: &[Value], from: usize, what: &str, matches: impl Fn(&Value) -> bool) -> usize {
    let found = events[from..].iter().position(matches);
    let found = found.unwrap_or_else(|| panic!("no {what} after event {from}:\n{events:#?}"));
    from + found + 1
}

/// A plain answer in two sentences. The first is released after the acknowledgement is due and
/// the round ends after the first has begun to play, so the events have one order.
fn answer() -> Round {
    Round {
        tokens: vec![
            (700, "The store opens at nine. ".to_owned()),
            (50, "It ".to_owned()),
            (2_000, "closes at six.".to_owned()),
        ],
        ..Round::default()
    }
}

const LAST_SENTENCE: &str = "It closes at six.";

/// One turn's events from `from` on, in the order the protocol gives them. Returns the task id
/// and the index after the `task_result`.
fn assert_turn(events: &[Value], from: usize) -> (String, usize) {
    let at = next(events, from, "task_created", |e| {
        e["type"] == "task_created"
    });
    let created = &events[at - 1];
    assert_eq!(created["speculative"], true, "{created}");
    let task_id = created["task_id"].as_str().expect("task id").to_owned();

    // The floor is held while the front decides; the provisional run starts with that request,
    // so the hold may come just before the task.
    let held = next(events, from, "floor_state", |e| e["type"] == "floor_state");
    let at = next(events, at.max(held), "the acknowledgement", |e| {
        e["type"] == "assistant_transcript" && e["kind"] == "ack" && e["delta"] != ""
    });
    let at = next(events, at, "the first relay sentence", |e| {
        e["type"] == "assistant_transcript" && e["verbatim"] == true && e["sentence"]["seq"] == 0
    });
    assert_eq!(events[at - 1]["delta"], "The store opens at nine.");
    assert_eq!(events[at - 1]["task_id"], task_id.as_str());
    let at = next(events, at, "task_result", |e| e["type"] == "task_result");
    let result = &events[at - 1];
    assert_eq!(result["task_id"], task_id.as_str(), "{result}");
    assert_eq!(result["effects"], "none_committed", "{result}");
    (task_id, at)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_asks_for_the_mode_gets_the_events_in_order() {
    let _lock = LOCK.lock().await;
    switch(true);
    let gateway = Gateway::start().await;
    gateway.llm.respond_with(|_| Some(answer()));

    let mut client = gateway.connect().await;
    let mut config = gateway.config("ix-ws-on");
    config["interaction"] = json!(true);
    client.send(config).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.1");
    assert_eq!(ready["interaction"]["enabled"], true, "{ready}");
    assert_eq!(ready["interaction"]["regime"], "cascade", "{ready}");
    assert!(
        client.of_type("config_warning").is_empty(),
        "{}",
        client.timeline()
    );

    // A typed turn.
    client
        .send(json!({ "type": "agent_input", "text": "what time does the store open" }))
        .await;
    client.until("task_result", 1).await;
    let (typed_task, after_typed) = assert_turn(&client.events(), 0);

    let listed = gateway.tasks("ix-ws-on").await;
    // Authentication is off: nobody lists another session's tasks.
    let other = gateway.tasks("ix-ws-another").await;
    assert_eq!(other["tasks"], json!([]), "{other}");
    let ids: Vec<&str> = listed["tasks"]
        .as_array()
        .expect("a task list")
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(ids.contains(&typed_task.as_str()), "{listed}");

    // A spoken turn once the answer has been heard: the speech-to-text double is the vendor.
    // The transcript still goes to the client, as on a plain session.
    client.until_heard(LAST_SENTENCE, 1).await;
    gateway.stt.speech_final("when does the store open").await;
    client.until("task_result", 2).await;
    let events = client.events();
    let at = next(&events, after_typed, "the transcript", |e| {
        e["type"] == "stt_result" && e["transcript"] == "when does the store open"
    });
    let (spoken_task, _) = assert_turn(&events, at);
    assert_ne!(spoken_task, typed_task);

    // The three client messages of protocol 1.1 reach the actor.
    client
        .send(json!({ "type": "task_answer", "request_id": "q_nobody_1", "outcome": "accept" }))
        .await;
    let refused = client.until("error", 1).await;
    assert_eq!(refused["code"], "unknown_request_id", "{refused}");
    client
        .send(json!({ "type": "interaction_update", "patch": { "policy": { "ack_due_ms": 250 } } }))
        .await;
    let applied = json!(["policy.ack_due_ms"]);
    let updated =
        |event: &Value| event["type"] == "interaction_updated" && event["applied"] == applied;
    client.until_where("the update", 1, updated).await;

    // A task that is still running is cancelled by its id.
    client.until_heard(LAST_SENTENCE, 2).await;
    gateway
        .llm
        .respond_with(|_| Some(Round::text(60_000, 20, &["It opens at nine."])));
    client
        .send(json!({ "type": "agent_input", "text": "where is my order" }))
        .await;
    let running = client.until("task_created", 3).await;
    client
        .send(json!({ "type": "task_cancel", "task_id": running["task_id"] }))
        .await;
    let cancelled = client.until("task_cancelled", 1).await;
    assert_eq!(cancelled["task_id"], running["task_id"], "{cancelled}");
    assert_eq!(cancelled["by"], "client", "{cancelled}");

    client.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_client_reads_the_task_events_as_they_happen() {
    let _lock = LOCK.lock().await;
    switch(true);
    let gateway = Gateway::start().await;
    gateway.llm.respond_with(|_| Some(answer()));
    // A screen of the caller's, apart from the call: the session's task events as they come.
    let url = format!(
        "http://127.0.0.1:{}/v1/tasks/events?stream_id=ix-ws-events",
        gateway.port
    );
    let mut events = reqwest::get(&url).await.expect("GET /v1/tasks/events");
    assert_eq!(events.status().as_u16(), 200);
    let kind = events.headers()["content-type"]
        .to_str()
        .unwrap_or_default()
        .to_owned();
    assert!(kind.starts_with("text/event-stream"), "{kind}");

    let mut client = gateway.connect().await;
    let mut config = gateway.config("ix-ws-events");
    config["interaction"] = json!(true);
    client.send(config).await;
    client.until("ready", 1).await;
    client
        .send(json!({ "type": "agent_input", "text": "what time does the store open" }))
        .await;
    let result = client.until("task_result", 1).await;

    let mut read = String::new();
    let mut seen: Vec<(String, Value)> = Vec::new();
    while !seen.iter().any(|(name, _)| name == "task_result") {
        let chunk = tokio::time::timeout(Duration::from_secs(10), events.chunk())
            .await
            .expect("an event in time")
            .expect("the stream")
            .expect("more of it");
        read.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = read.find("\n\n") {
            let block: String = read.drain(..end + 2).collect();
            let field = |name: &str| {
                block
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .map(str::trim)
            };
            if let (Some(name), Some(data)) = (field("event:"), field("data:")) {
                seen.push((name.to_owned(), serde_json::from_str(data).expect("JSON")));
            }
        }
    }
    let names: Vec<&str> = seen.iter().map(|(name, _)| name.as_str()).collect();
    let at = |name: &str| names.iter().position(|seen| *seen == name).expect(name);
    assert!(
        at("task_created") < at("task_output") && at("task_output") < at("task_result"),
        "{names:?}"
    );
    for (name, data) in &seen {
        assert_eq!(data["type"], name.as_str());
        assert_eq!(data["stream_id"], "ix-ws-events", "{data}");
        assert_eq!(data["task_id"], result["task_id"], "{data}");
    }
    let output = &seen[at("task_output")].1;
    assert_eq!(output["text"], result["summary"], "{output}");
    // The client has shown it: taken, nothing is refused (the next error is the next
    // message's).
    client
        .send(json!({ "type": "task_seen", "task_id": result["task_id"] }))
        .await;
    client
        .send(json!({ "type": "task_answer", "request_id": "q_nobody_1", "outcome": "accept" }))
        .await;
    let refused = client.until("error", 1).await;
    assert_eq!(refused["code"], "unknown_request_id", "{refused}");
    client.close().await;
}

/// What a session that does not run the mode sent.
struct Plain {
    /// Every frame in order (`Client::types`).
    types: Vec<String>,
    ready: Value,
    warnings: Vec<Value>,
}

/// A session with this `interaction` value (or none) through fixed steps: config, a `speak`,
/// a spoken turn and a typed one (`agent_input`) that the conversation loop answers, and two
/// messages a session without the mode refuses.
async fn plain_session(gateway: &Gateway, interaction: Option<Value>) -> Plain {
    gateway
        .llm
        .respond_with(|_| Some(Round::text(20, 20, &["It is noon."])));
    let mut client = gateway.connect().await;
    let mut config = gateway.config("ix-ws-plain");
    if let Some(interaction) = interaction {
        config["interaction"] = interaction;
    }
    client.send(config).await;
    let ready = client.until("ready", 1).await;

    client
        .send(json!({ "type": "speak", "text": "Hello there." }))
        .await;
    client.until("tts_playback_complete", 1).await;

    gateway.stt.speech_final("what time is it").await;
    client.until("tts_playback_complete", 2).await;

    // A conversation session takes typed turns too.
    client
        .send(json!({ "type": "agent_input", "text": "hello" }))
        .await;
    client.until("tts_playback_complete", 3).await;

    for message in [
        json!({ "type": "task_cancel", "task_id": "task_nobody" }),
        json!({ "type": "task_answer", "request_id": "q_nobody_1", "outcome": "accept" }),
    ] {
        let errors = client.of_type("error").len();
        client.send(message).await;
        client.until("error", errors + 1).await;
    }
    // Anything the session sends unasked would come now.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let plain = Plain {
        types: client.types(),
        ready,
        warnings: client.of_type("config_warning"),
    };
    client.close().await;
    plain
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_does_not_ask_is_the_same_with_the_switch_on() {
    let _lock = LOCK.lock().await;
    let gateway = Gateway::start().await;

    switch(false);
    let baseline = plain_session(&gateway, None).await;
    switch(true);
    let plain = plain_session(&gateway, None).await;

    assert_eq!(plain.types, baseline.types);
    assert_eq!(plain.ready, baseline.ready);
    assert_eq!(plain.ready["protocol_version"], "1.0");
    assert!(plain.ready.get("interaction").is_none(), "{}", plain.ready);
    // Nothing of protocol 1.1 is among them.
    let known = [
        "ready",
        "tts_playback_complete",
        "stt_result",
        "error",
        "agent_response_started",
        "assistant_transcript",
        "agent_response_done",
    ];
    let is_1_0 = |kind: &String| kind.starts_with("audio:") || known.contains(&kind.as_str());
    assert!(plain.types.iter().all(is_1_0), "{:?}", plain.types);
    // The conversation loop answered the spoken turn in both.
    assert_eq!(
        gateway.tts.spoken(),
        [
            "Hello there.",
            "It is noon.",
            "It is noon.",
            "Hello there.",
            "It is noon.",
            "It is noon."
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asking_with_the_switch_off_gives_one_warning_and_the_plain_session() {
    let _lock = LOCK.lock().await;
    switch(false);
    let gateway = Gateway::start().await;

    let baseline = plain_session(&gateway, None).await;
    let asked = plain_session(&gateway, Some(json!(true))).await;

    assert_eq!(asked.warnings.len(), 1, "{:?}", asked.warnings);
    assert_eq!(asked.warnings[0]["code"], "interaction_disabled_by_server");
    assert!(baseline.warnings.is_empty(), "{:?}", baseline.warnings);
    assert_eq!(asked.ready, baseline.ready);
    // One frame more than the baseline: the warning, before `ready`.
    let mut types = asked.types;
    assert_eq!(types.remove(0), "config_warning");
    assert_eq!(types, baseline.types);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_ready_front_the_mode_is_refused_and_the_session_runs_without_it() {
    let _lock = LOCK.lock().await;
    switch(false);
    let gateway = Gateway::start().await;
    let baseline = plain_session(&gateway, None).await;
    // The switch on, and a front model that does not answer: no rule-based fallback (chapter 9).
    let down = ix_mocks::FrontDouble::refusing().await;
    set_server_for_tests(ServerInteraction {
        enabled: true,
        front_model: Some(waav_gateway::core::interaction::config::FrontModelServer {
            url: down.url.clone(),
            model: None,
            api_key: None,
            command: None,
            licence: None,
        }),
        ..ServerInteraction::default()
    });
    let asked = plain_session(&gateway, Some(json!(true))).await;
    assert_eq!(asked.warnings.len(), 1, "{:?}", asked.warnings);
    assert_eq!(asked.warnings[0]["code"], "interaction_refused");
    assert_eq!(asked.warnings[0]["detail"]["reason"], "front_unavailable");
    assert_eq!(asked.ready, baseline.ready);
    let mut types = asked.types;
    assert_eq!(types.remove(0), "config_warning");
    assert_eq!(types, baseline.types);
    // A `control` message to a session without the mode is an error, as task messages are.
    let mut client = gateway.connect().await;
    client.send(gateway.config("ix-ws-control")).await;
    client.until("ready", 1).await;
    client
        .send(json!({ "type": "control", "kind": "rate", "value": "slower" }))
        .await;
    let error = client.until("error", 1).await;
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("control")),
        "{error}"
    );
    client.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_the_mode_does_not_fit_runs_without_it() {
    let _lock = LOCK.lock().await;
    switch(true);
    let gateway = Gateway::start().await;
    // No conversation loop and no `tasks.llm`: nothing could run a task.
    let raw = |stream_id: &str| {
        let mut config = gateway.config(stream_id);
        config
            .as_object_mut()
            .expect("an object")
            .remove("conversation_config");
        config
    };

    // It asked: one warning says why, and the session is a plain one.
    let mut client = gateway.connect().await;
    let mut config = raw("ix-ws-raw");
    config["interaction"] = json!(true);
    client.send(config).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.0");
    assert!(ready.get("interaction").is_none(), "{ready}");
    let warnings = client.of_type("config_warning");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["code"], "interaction_needs_background");
    assert_eq!(warnings[0]["detail"]["reason"], "no_background");
    // Its speech is not metered: the provider's audio arrives as the one frame it came in.
    client
        .send(json!({ "type": "speak", "text": "Hello there." }))
        .await;
    client.until("tts_playback_complete", 1).await;
    let audio = gateway.tts.audio_ms("Hello there.") as usize * 32;
    let types = client.types();
    assert!(types.contains(&format!("audio:{audio}")), "{types:?}");
    client.close().await;

    // It did not ask, the server's default block did: it is told nothing.
    switch_on_with_default();
    let mut client = gateway.connect().await;
    client.send(raw("ix-ws-raw-default")).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.0");
    assert_eq!(client.types(), ["ready"]);
    client.close().await;

    // A session the default block fits runs the mode without asking.
    let mut client = gateway.connect().await;
    client.send(gateway.config("ix-ws-default")).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.1");
    assert_eq!(ready["interaction"]["enabled"], true, "{ready}");
    // Chapter 10: when the lines said while a service fails play, and which are loaded.
    let failure = &ready["interaction"]["failure"];
    assert_eq!(
        (
            &failure["stall_after_ms"],
            &failure["max_wait_s"],
            &failure["follow_up"]
        ),
        (&json!(2_500), &json!(30), &json!(false)),
        "{ready}"
    );
    assert!(
        ["session", "default", "none"].contains(&failure["lines"].as_str().unwrap_or_default())
    );
    client.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_alias_turns_the_mode_on_with_the_operators_tools() {
    let _lock = LOCK.lock().await;
    // No default block, and sessions may not bring tools: what follows is the alias's doing.
    switch(true);
    let gateway = Gateway::start().await;
    let tool = json!({
        "name": "book_table", "url": "https://tools.example/book", "side_effects": true
    });
    let definition = AliasDefinition {
        interaction: Some(json!({ "policy": { "ack_due_ms": 100 }, "tasks": { "tools": [tool] } })),
        ..AliasDefinition::default()
    };
    global_aliases().register("ix-ws-desk", definition);
    let named = |stream_id: &str| {
        let mut config = gateway.config(stream_id);
        config["alias"] = json!("ix-ws-desk");
        config
    };

    // The session names the alias and sends no block: it runs the alias's.
    let mut client = gateway.connect().await;
    client.send(named("ix-ws-alias")).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.1", "{ready}");
    let echo = &ready["interaction"];
    assert_eq!(echo["policy"]["ack_due_ms"], 100, "{ready}");
    let tools = echo["tasks"]["tools"].to_string();
    assert!(tools.contains("book_table"), "the operator's tool: {ready}");
    assert!(client.of_type("config_warning").is_empty());
    client.close().await;

    // The session's own value wins, here the one that turns the mode off.
    let mut client = gateway.connect().await;
    let mut config = named("ix-ws-alias-off");
    config["interaction"] = json!(false);
    client.send(config).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.0");
    assert!(ready.get("interaction").is_none(), "{ready}");
    client.close().await;
    global_aliases().remove("ix-ws-desk");
}

/// A `config` whose background is an inline DAG over the LLM double: it ends in text, or in
/// audio it synthesises itself, or in nothing a caller could be told.
fn dag_config(gateway: &Gateway, stream_id: &str, end: &str) -> Value {
    let mut config = gateway.config(stream_id);
    config
        .as_object_mut()
        .expect("an object")
        .remove("conversation_config");
    let mut nodes = vec![
        json!({ "id": "audio_input", "type": "audio_input" }),
        json!({ "id": "stt", "type": "stt_provider", "provider": gateway.stt.provider }),
        json!({ "id": "llm", "type": "llm_endpoint", "base_url": gateway.llm.base_url,
                "model": "mock", "api_key": "test-key", "streaming": true }),
    ];
    let mut edges = vec![
        json!({ "from": "audio_input", "to": "stt" }),
        json!({ "from": "stt", "to": "llm" }),
    ];
    match end {
        "text" => {
            nodes.push(json!({ "id": "out", "type": "text_output", "destination": "web_socket" }));
            edges.push(json!({ "from": "llm", "to": "out" }));
        }
        "audio" => {
            nodes.push(
                json!({ "id": "tts", "type": "tts_provider", "provider": gateway.tts.provider }),
            );
            nodes.push(json!({ "id": "out", "type": "audio_output", "destination": "web_socket" }));
            edges.push(json!({ "from": "llm", "to": "tts" }));
            edges.push(json!({ "from": "tts", "to": "out" }));
        }
        _ => {
            nodes.push(json!({ "id": "out", "type": "passthrough" }));
            edges.push(json!({ "from": "llm", "to": "out" }));
        }
    }
    config["dag_config"] = json!({ "definition": {
        "id": "ix-dag", "name": "Interaction DAG", "version": "1.0.0",
        "nodes": nodes, "edges": edges, "entry_node": "audio_input", "exit_nodes": ["out"],
    }});
    config["interaction"] = json!(true);
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dag_that_ends_in_text_is_the_background_and_one_that_speaks_is_refused() {
    let _lock = LOCK.lock().await;
    switch(true);
    let gateway = Gateway::start().await;
    gateway.llm.respond_with(|_| {
        Some(Round::text(
            700,
            20,
            &["The store opens at nine. ", "It closes at six."],
        ))
    });

    let mut client = gateway.connect().await;
    client.send(dag_config(&gateway, "ix-ws-dag", "text")).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.1", "{}", client.timeline());
    assert_eq!(ready["interaction"]["tasks"]["backend"], "dag", "{ready}");
    assert_eq!(
        ready["interaction"]["tasks"]["speculative_start"], "off",
        "{ready}"
    );
    assert!(
        client.of_type("config_warning").is_empty(),
        "{}",
        client.timeline()
    );

    client
        .send(json!({ "type": "agent_input", "text": "what time does the store open" }))
        .await;
    let result = client.until("task_result", 1).await;
    assert_eq!(result["state"], "completed", "{}", client.timeline());
    client.until_heard(LAST_SENTENCE, 1).await;
    let events = client.events();
    let created = client.of_type("task_created");
    assert_eq!(created.len(), 1, "{}", client.timeline());
    assert_eq!(created[0]["background"], "dag");
    assert_eq!(created[0]["speculative"], false);
    // The graph ran once, for the task, with the caller's words.
    let asked = gateway.llm.requests();
    assert_eq!(asked.len(), 1, "{asked:?}");
    let said = asked[0]["messages"]
        .as_array()
        .expect("messages")
        .last()
        .expect("a message");
    assert_eq!(said["content"], "what time does the store open");
    // Its text is spoken by the agent, sentence by sentence, and not sent as a DAG message.
    let relayed: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "assistant_transcript" && e["kind"] == "relay" && e["delta"] != "")
        .collect();
    let words: Vec<&str> = relayed.iter().filter_map(|e| e["delta"].as_str()).collect();
    assert_eq!(words, ["The store opens at nine.", LAST_SENTENCE]);
    assert!(
        relayed
            .iter()
            .all(|e| e["source"] == "background" && e["verbatim"] == true)
    );
    assert!(
        client.of_type("message").is_empty(),
        "{}",
        client.timeline()
    );
    client.close().await;

    // A graph that synthesises its own answer: one warning with its code, and a plain DAG
    // session, whose graph runs once per spoken turn as it always did.
    let mut client = gateway.connect().await;
    client
        .send(dag_config(&gateway, "ix-ws-dag-audio", "audio"))
        .await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.0");
    assert!(ready.get("interaction").is_none(), "{ready}");
    let warnings = client.of_type("config_warning");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["code"], "interaction_dag_audio_output");
    assert_eq!(
        warnings[0]["detail"],
        json!({ "surface": "ws", "reason": "dag_audio_output" })
    );
    gateway.stt.speech_final("when does the store open").await;
    let ran = async {
        while gateway.llm.requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), ran)
        .await
        .unwrap_or_else(|_| panic!("the plain DAG did not run\n{}", client.timeline()));
    let asked = gateway.llm.requests();
    let said = asked[1]["messages"]
        .as_array()
        .expect("messages")
        .last()
        .expect("a message");
    assert_eq!(said["content"], "when does the store open");
    assert!(client.of_type("task_created").is_empty());
    client.close().await;

    // A graph with nothing to say to the caller.
    let mut client = gateway.connect().await;
    client
        .send(dag_config(&gateway, "ix-ws-dag-mute", "none"))
        .await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.0");
    let warnings = client.of_type("config_warning");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["code"], "interaction_dag_no_text_output");
    client.close().await;
}

/// Asks for a table to be held and confirms it from the client, on a connection that has
/// asked nothing yet. Returns the question once the tool's `count`-th call is on the wire.
async fn hold_a_table(client: &mut Client, tool: &ToolDouble, count: usize) -> Value {
    client
        .send(json!({ "type": "agent_input", "text": "can you hold a table for two" }))
        .await;
    let question = client.until("task_question", 1).await;
    client
        .send(json!({
            "type": "task_answer", "request_id": question["request_id"], "outcome": "accept",
        }))
        .await;
    let sent = async {
        while tool.calls().len() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let sent = tokio::time::timeout(Duration::from_secs(10), sent).await;
    assert!(sent.is_ok(), "no tool call #{count}\n{}", client.timeline());
    question
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_outlives_a_lost_connection_for_the_caller_who_is_back_or_the_push_address() {
    const HELD: &str = "Your table for two is held.";
    const SECRET: &str = "s3cret-for-tools";
    const STREAM: &str = "ix-ws-reconnect";
    let _lock = LOCK.lock().await;
    let tool = ToolDouble::start(json!({ "effect": "held a table for two" })).await;
    let hook = ToolDouble::start(json!({})).await;
    tool.delay(1_500);
    // The operator's block: one tool that changes something, where a result nobody was there
    // for is pushed, and three seconds for a caller to come back.
    set_server_for_tests(ServerInteraction {
        enabled: true,
        tool_secret: Some(SECRET.to_owned()),
        default: Some(json!({ "tasks": {
            "tools": [{
                "name": "hold_table", "description": "Use it to hold a table.", "url": tool.url,
                "side_effects": true, "confirm_phrase": "hold a table",
            }],
            "push_url": hook.url,
            "reconnect_grace_s": 3,
        } })),
        ..ServerInteraction::default()
    });
    let gateway = Gateway::start().await;
    gateway.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(300, "hold_table", json!({ "party": 2 })),
            _ => Round::text(50, 20, &["ANSWER:\n", HELD]),
        })
    });

    let mut client = gateway.connect().await;
    client.send(gateway.config(STREAM)).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(
        ready["interaction"]["tasks"]["reconnect_grace_s"], 3,
        "{ready}"
    );
    assert_eq!(ready["interaction"]["tasks"]["push"], true, "{ready}");
    assert!(
        !ready.to_string().contains(&hook.url),
        "the address is not echoed"
    );
    let first = hold_a_table(&mut client, &tool, 1).await;
    assert_eq!(first["task_ref"], "t1", "{first}");
    let first = first["task_id"].as_str().expect("a task id").to_owned();

    // The connection is lost with the call on the wire: the socket goes without a close frame.
    drop(client);
    // The caller is back at once, on a new connection with the same `stream_id`.
    let mut back = gateway.connect().await;
    back.send(gateway.config(STREAM)).await;
    back.until("ready", 1).await;
    let result = back.until("task_result", 1).await;
    assert_eq!(result["task_id"], first.as_str(), "{result}");
    assert_eq!(
        (&result["state"], &result["summary"], &result["effects"]),
        (
            &json!("completed"),
            &json!(HELD),
            &json!(["held a table for two"])
        ),
        "{result}"
    );
    back.until_heard(HELD, 1).await;
    // The task is read by its id, with its result, and only with its session.
    let (status, task) = gateway.task(&first, STREAM).await;
    assert_eq!(status, 200, "{task}");
    assert_eq!(task["stream_id"], STREAM, "{task}");
    assert_eq!(
        task["result"],
        json!({ "state": "completed", "summary": HELD, "effects": ["held a table for two"] })
    );
    assert_eq!(gateway.task(&first, "ix-ws-another").await.0, 404);
    assert_eq!(gateway.task("task_nobody", STREAM).await.0, 404);
    // The grace passes: the caller had it, so nothing was pushed.
    tokio::time::sleep(Duration::from_millis(3_500)).await;
    assert!(hook.calls().is_empty(), "{:?}", hook.calls());

    // Lost again with a second call out, and this time nobody comes back: the result is
    // pushed, signed, once the grace is over.
    let second = hold_a_table(&mut back, &tool, 2).await;
    // A task ref is not used twice in a session, over its connections.
    assert_eq!(second["task_ref"], "t2", "{second}");
    let second = second["task_id"].as_str().expect("a task id").to_owned();
    let lost = std::time::Instant::now();
    drop(back);
    let arrived = async {
        while hook.calls().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let arrived = tokio::time::timeout(Duration::from_secs(10), arrived).await;
    assert!(arrived.is_ok(), "nothing was pushed");
    assert!(
        lost.elapsed() >= Duration::from_secs(3),
        "{:?}",
        lost.elapsed()
    );
    let push = &hook.calls()[0];
    assert!(push.signed_with(SECRET));
    assert_eq!(push.body["type"], "task_result", "{}", push.body);
    assert_eq!(push.body["task_id"], second.as_str(), "{}", push.body);
    assert_eq!(push.body["stream_id"], STREAM, "{}", push.body);
    assert_eq!(push.body["summary"], HELD, "{}", push.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_raw_session_runs_its_tasks_on_the_operators_webhook() {
    const HOOK_SECRET: &str = "s3cret-of-the-crm";
    let _lock = LOCK.lock().await;
    let hook = BackendDouble::start(|_: &Hit| (200, "{}".to_owned())).await;
    let entry = json!({ "name": "crm", "type": "webhook", "url": hook.url });
    let mut spec: BackendSpec = serde_json::from_value(entry).expect("a catalog entry");
    spec.secret = Some(HOOK_SECRET.to_owned());
    let server = |allow_client_backends| ServerInteraction {
        enabled: true,
        task_backends: vec![spec.clone()],
        allow_client_backends,
        ..ServerInteraction::default()
    };
    let gateway = Gateway::start().await;
    // Speech-to-text and text-to-speech only: the backend is the session's whole background.
    let raw = |stream_id: &str| {
        let mut config = gateway.config(stream_id);
        let config_map = config.as_object_mut().expect("an object");
        config_map.remove("conversation_config");
        config["interaction"] = json!({ "tasks": { "default_backend": "crm" } });
        config
    };

    // The operator does not let sessions choose: the name is refused, and nothing is left
    // that could run a task.
    set_server_for_tests(server(false));
    let mut client = gateway.connect().await;
    client.send(raw("ix-ws-hook-refused")).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.0", "{ready}");
    let codes: Vec<Value> = client
        .of_type("config_warning")
        .into_iter()
        .map(|warning| warning["code"].clone())
        .collect();
    assert_eq!(
        codes,
        [
            "interaction_override_refused",
            "interaction_needs_background"
        ]
    );
    client.close().await;

    set_server_for_tests(server(true));
    let mut client = gateway.connect().await;
    client.send(raw("ix-ws-hook")).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(ready["protocol_version"], "1.1", "{}", client.timeline());
    assert_eq!(ready["interaction"]["tasks"]["backend"], "webhook:crm");
    assert!(
        !ready.to_string().contains(&hook.url),
        "the address is not echoed"
    );
    assert!(
        client.of_type("config_warning").is_empty(),
        "{}",
        client.timeline()
    );

    client
        .send(json!({ "type": "agent_input", "text": "what is the status of my order" }))
        .await;
    let created = client.until("task_created", 1).await;
    assert_eq!(created["background"], "webhook:crm", "{created}");
    let task = created["task_id"].as_str().expect("a task id").to_owned();
    let arrived = async {
        while hook.hits().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let arrived = tokio::time::timeout(Duration::from_secs(10), arrived).await;
    assert!(arrived.is_ok(), "the webhook got no task");
    let sent = hook.hits()[0].call.clone();
    assert!(sent.signed_with(HOOK_SECRET));
    assert_eq!(sent.body["request"], "what is the status of my order");

    // The webhook reports on the gateway's callback route, with its own signature.
    let callback = format!(
        "http://127.0.0.1:{}{}",
        gateway.port,
        sent.body["callback"].as_str().unwrap()
    );
    let post = |secret: &'static str, event: Value| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let (now, body) = (
            now.expect("a clock").as_secs().to_string(),
            event.to_string(),
        );
        let signature = ix_mocks::sign(secret, &now, &body);
        let request = reqwest::Client::new()
            .post(&callback)
            .header("X-WaaV-Timestamp", now)
            .header("X-WaaV-Signature", signature)
            .body(body);
        async move {
            request
                .send()
                .await
                .expect("the callback route")
                .status()
                .as_u16()
        }
    };
    let result = json!({ "type": "result", "text": "It ships on Friday." });
    assert_eq!(post("not-the-secret", result.clone()).await, 401);
    assert_eq!(post(HOOK_SECRET, json!("not an event")).await, 400);
    assert_eq!(post(HOOK_SECRET, result.clone()).await, 202);
    client.until_heard("It ships on Friday.", 1).await;
    let done = client.until("task_result", 1).await;
    assert_eq!(
        (&done["task_id"], &done["state"], &done["summary"]),
        (
            &json!(task),
            &json!("completed"),
            &json!("It ships on Friday.")
        ),
        "{done}"
    );
    // The task is over: its callback address is gone.
    assert_eq!(post(HOOK_SECRET, result).await, 404);
    client.close().await;
    switch(false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tenant_registers_a_persona_and_caller_facts_over_http_and_a_session_selects_it() {
    let _lock = LOCK.lock().await;
    set_server_for_tests(ServerInteraction {
        enabled: true,
        tenant_personas: true,
        caller_memory_key: Some("ws-memory-key".to_owned()),
        ..ServerInteraction::default()
    });
    let gateway = Gateway::start().await;
    let http = reqwest::Client::new();
    let url = |path: &str| format!("http://127.0.0.1:{}{path}", gateway.port);
    let put = |path: &str, body: Value| http.put(url(path)).json(&body).send();

    let persona = json!({ "name": "Lumi", "org": "Lumen Air", "role": "booking assistant",
                          "banks": { "ack": [{ "text": "Lumi here, checking." }] } });
    let registered = put("/v1/interaction/personas/ws-lumi", persona.clone())
        .await
        .expect("PUT a persona");
    assert_eq!(registered.status().as_u16(), 200);
    let body: Value = registered.json().await.expect("a JSON body");
    assert_eq!(body, json!({ "id": "ws-lumi", "version": 1 }));
    let mut liar = persona.clone();
    liar["banks"]["ack"][0]["text"] = json!("Your booking is confirmed.");
    let refused = put("/v1/interaction/personas/ws-liar", liar)
        .await
        .expect("PUT");
    assert_eq!(refused.status().as_u16(), 422);
    let body: Value = refused.json().await.expect("a JSON body");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("completion_claim"),
        "{body}"
    );

    let facts = |list: Value| json!({ "facts": list });
    let kept = put(
        "/v1/interaction/memory/caller-7",
        facts(json!(["prefers aisle seats"])),
    );
    assert_eq!(kept.await.expect("PUT").status().as_u16(), 204);
    let numbered = put(
        "/v1/interaction/memory/caller-7",
        facts(json!(["card ends 4417"])),
    );
    assert_eq!(numbered.await.expect("PUT").status().as_u16(), 422);
    let get = |path: &str| http.get(url(path)).send();
    let read = get("/v1/interaction/memory/caller-7").await.expect("GET");
    assert_eq!(read.status().as_u16(), 200);
    let body: Value = read.json().await.expect("a JSON body");
    assert_eq!(body, json!({ "facts": ["prefers aisle seats"] }));

    // The tenant reads its persona back, by its newest version or a kept one.
    let second = put("/v1/interaction/personas/ws-lumi", persona.clone()).await;
    assert_eq!(second.expect("PUT").status().as_u16(), 200);
    let read = get("/v1/interaction/personas/ws-lumi").await.expect("GET");
    assert_eq!(read.status().as_u16(), 200);
    let body: Value = read.json().await.expect("a JSON body");
    assert_eq!(
        (
            &body["version"],
            &body["versions"],
            &body["persona"]["name"]
        ),
        (&json!(2), &json!([1, 2]), &json!("Lumi")),
        "{body}"
    );
    let first: Value = get("/v1/interaction/personas/ws-lumi@1")
        .await
        .expect("GET")
        .json()
        .await
        .expect("a JSON body");
    assert_eq!(first["version"], 1, "{first}");
    for missing in ["ws-lumi@9", "ws-nobody"] {
        let path = format!("/v1/interaction/personas/{missing}");
        let status = get(&path).await.expect("GET").status().as_u16();
        assert_eq!(status, 404, "{missing}");
    }
    // One removed is gone; the one the session selects below stays.
    let doomed = put("/v1/interaction/personas/ws-doomed", persona.clone()).await;
    assert_eq!(doomed.expect("PUT").status().as_u16(), 200);
    let delete = |path: &str| http.delete(url(path)).send();
    let removed = delete("/v1/interaction/personas/ws-doomed")
        .await
        .expect("DELETE");
    assert_eq!(removed.status().as_u16(), 204);
    let again = delete("/v1/interaction/personas/ws-doomed")
        .await
        .expect("DELETE");
    assert_eq!(again.status().as_u16(), 404);
    let gone = get("/v1/interaction/personas/ws-doomed")
        .await
        .expect("GET");
    assert_eq!(gone.status().as_u16(), 404);

    // A session names the persona, and passes a token nobody signed.
    let mut client = gateway.connect().await;
    let mut config = gateway.config("ix-ws-persona");
    config["interaction"] = json!({ "front": { "persona": "ws-lumi",
                                               "caller": { "identity_token": "v1.e30.forged" } } });
    client.send(config).await;
    let ready = client.until("ready", 1).await;
    assert_eq!(
        ready["interaction"]["front"]["persona"], "ws-lumi",
        "{ready}"
    );
    let warning = client.until("config_warning", 1).await;
    assert_eq!(warning["code"], "caller_identity_rejected", "{warning}");
    // A heartbeat is never answered with an error.
    client
        .send(json!({ "type": "playback_position", "utterance_id": "u_1", "played_ms": 100 }))
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client.of_type("error").is_empty(), "{}", client.timeline());

    // With the switch off the routes are not there for tenants.
    switch(true);
    let off = put("/v1/interaction/personas/ws-lumi", persona)
        .await
        .expect("PUT");
    assert_eq!(off.status().as_u16(), 404);
    let off = http.get(url("/v1/interaction/personas/ws-lumi")).send();
    assert_eq!(off.await.expect("GET").status().as_u16(), 404);
    let off = http.delete(url("/v1/interaction/personas/ws-lumi")).send();
    assert_eq!(off.await.expect("DELETE").status().as_u16(), 404);
    let off = http.get(url("/v1/interaction/memory/caller-7")).send();
    assert_eq!(off.await.expect("GET").status().as_u16(), 404);
}

// ---- regimes B and C on `/realtime`

const TOOL_SECRET: &str = "s3cret-for-provider-tools";
const BOOKED: &str = "Your table for two at 7 pm is booked.";

/// A gateway whose `/realtime` OpenAI provider is the vendor double, an operator that accepts
/// the provider's own speech as monitored, and one tool that changes something.
async fn provider_gateway(vendor: &VendorDouble, tool: &ToolDouble) -> Gateway {
    set_server_for_tests(ServerInteraction {
        enabled: true,
        tool_secret: Some(TOOL_SECRET.to_owned()),
        truth_ack: vec!["openai".to_owned()],
        default: Some(json!({ "tasks": { "tools": [{
            "name": "book_table", "description": "Book a table.", "url": tool.url,
            "side_effects": true, "confirm_phrase": "book a table",
        }] } })),
        ..ServerInteraction::default()
    });
    let url = vendor.url.clone();
    Gateway::start_with(move |config| {
        config.openai_api_key = Some("test".to_owned());
        config
            .realtime_endpoint_overrides
            .insert("openai".to_owned(), url);
    })
    .await
}

fn realtime_config(gateway: &Gateway, extra: Value) -> Value {
    let mut block = json!({ "tasks": { "llm": {
        "base_url": gateway.llm.base_url, "model": "mock", "api_key": "test-key",
    } } });
    for (key, value) in extra.as_object().expect("an object") {
        block[key] = value.clone();
    }
    json!({ "type": "config", "provider": "openai", "instructions": "You are the Larkspur assistant.",
            "interaction": block })
}

/// The gateway's lines the vendor was asked to say, in order.
fn lines(vendor: &VendorDouble) -> Vec<String> {
    vendor
        .of("response.create")
        .iter()
        .filter_map(|event| event["response"]["instructions"].as_str())
        .filter_map(|text| {
            text.split_once(ix_mocks::LINE_MARKER)
                .map(|(_, words)| words.to_owned())
        })
        .collect()
}

/// Regime B (the default for OpenAI with the operator's acknowledgement): the provider calls
/// the delegate function; the confirmation is the gateway's line, checked against what the
/// provider said, asked again when the words were wrong; the caller's spoken yes answers it;
/// the committed result is the gateway's line too, and the call's output says so; a promise
/// the provider makes on its own is reported.
#[tokio::test]
async fn regime_b_a_delegated_booking_is_confirmed_said_and_checked_on_realtime() {
    let _lock = LOCK.lock().await;
    let vendor = VendorDouble::start().await;
    let tool = ToolDouble::start(json!({ "effect": "booked a table for two" })).await;
    let gateway = provider_gateway(&vendor, &tool).await;
    gateway.llm.respond_with(|request| {
        Some(match ix_mocks::messages(request, "tool").len() {
            0 => Round::tool(100, "book_table", json!({ "party": 2, "time": "7 pm" })),
            _ => Round::text(50, 20, &["ANSWER:\n", BOOKED]),
        })
    });
    // The provider gets the first read-back wrong.
    let wrong = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let first = wrong.clone();
    vendor.speak_with(move |words| {
        if words.contains("Do you want me")
            && first.swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            words.replacen('2', "3", 1)
        } else {
            words.to_owned()
        }
    });

    let mut client = gateway.connect_to("/realtime").await;
    client.send(realtime_config(&gateway, json!({}))).await;
    let created = client.until("session_created", 1).await;
    assert_eq!(
        created["interaction"]["regime"], "provider_speech",
        "{created}"
    );
    assert_eq!(
        created["interaction"]["speech"]["provider"], "openai",
        "{created}"
    );
    assert_eq!(
        created["interaction"]["speech"]["delegate_tool"], "waav_delegate",
        "{created}"
    );
    // The provider's session carries the delegate function and the regime's instructions.
    let update = vendor
        .until("session.update", 1, |event| {
            event["type"] == "session.update"
        })
        .await;
    let session = &update["session"];
    assert!(
        session["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|t| t["name"] == "waav_delegate")),
        "{update}"
    );
    let instructions = session["instructions"].as_str().unwrap_or_default();
    assert!(
        instructions.starts_with("You are the Larkspur assistant."),
        "{instructions}"
    );
    assert!(
        instructions.contains("call the function waav_delegate"),
        "{instructions}"
    );

    vendor.send(ga::call(
        "resp_call",
        "call_1",
        "waav_delegate",
        json!({ "request": "book a table for two at seven" }),
    ));
    let task = client.until("task_created", 1).await;
    assert_eq!(task["origin"], "provider_tool", "{task}");
    let question = client.until("task_question", 1).await;
    assert_eq!(question["spoken"], true, "{question}");
    let prompt = question["prompt"].as_str().unwrap_or_default().to_owned();
    assert!(prompt.contains("Do you want me to do that?"), "{prompt}");
    // The wrong read-back is reported and the question asked again; the caller then answers.
    let claim = client.until("unverified_claim", 1).await;
    assert_eq!(
        (&claim["kind"], &claim["rule"]),
        (&json!("entity"), &json!("ExactWords")),
        "{claim}"
    );
    assert!(
        claim["text"].as_str().unwrap_or_default().contains('3'),
        "{claim}"
    );
    vendor
        .until("the question again", 2, |event| {
            event["type"] == "response.create"
                && event["response"]["instructions"]
                    .as_str()
                    .is_some_and(|t| t.contains("Do you want me"))
        })
        .await;
    // Its response: the line went out, and the vendor answers it at once.
    tokio::time::sleep(Duration::from_millis(400)).await;
    vendor.send(vec![ga::heard("yes please")]);
    let answer = client.until("task_answer_received", 1).await;
    assert_eq!(
        (&answer["outcome"], &answer["by"]),
        (&json!("accept"), &json!("voice")),
        "{answer}"
    );
    let result = client.until("task_result", 1).await;
    assert_eq!(
        result["effects"],
        json!(["booked a table for two"]),
        "{result}"
    );
    assert_eq!(result["spoken"], "full", "{result}");
    assert_eq!(tool.calls().len(), 1);
    // The call's output says the result was read; no response is asked for with it.
    let output = vendor
        .until("the call's output", 1, |event| {
            event["type"] == "conversation.item.create"
                && event["item"]["type"] == "function_call_output"
        })
        .await;
    assert_eq!(output["item"]["call_id"], "call_1", "{output}");
    let output: Value = serde_json::from_str(output["item"]["output"].as_str().unwrap()).unwrap();
    assert_eq!(output["status"], "completed", "{output}");
    assert!(
        output["note"]
            .as_str()
            .unwrap_or_default()
            .contains("Do not repeat"),
        "{output}"
    );
    vendor
        .until("the result line", 1, |event| {
            event["response"]["instructions"]
                .as_str()
                .is_some_and(|t| t.ends_with(BOOKED))
        })
        .await;
    let said = lines(&vendor);
    assert_eq!(said.last().map(String::as_str), Some(BOOKED), "{said:?}");
    assert!(
        vendor
            .of("response.create")
            .iter()
            .all(|r| r["response"]["instructions"].is_string()),
        "every response the gateway asked for is a line of its own"
    );

    // The provider promises something on its own: one verdict, no correction (report).
    tokio::time::sleep(Duration::from_millis(400)).await;
    vendor.send(ga::says("resp_own", "Great. I'll email you the receipt."));
    let claim = client.until("unverified_claim", 2).await;
    assert_eq!(
        (
            &claim["kind"],
            &claim["rule"],
            &claim["text"],
            &claim["corrected"]
        ),
        (
            &json!("outcome"),
            &json!("ChannelPromise"),
            &json!("I'll email you the receipt."),
            &json!(false)
        ),
        "{claim}"
    );
    // The delegate call never reached the client.
    assert!(
        client.of_type("function_call").is_empty(),
        "{:?}",
        client.of_type("function_call")
    );
}

/// Regime C (forced): the provider is the front. The caller cancels the running task by
/// voice; the call's output says so and the provider is asked to reply. A completion the
/// provider claims on its own is corrected by the policy's template.
#[tokio::test]
async fn regime_c_a_cancel_by_voice_answers_the_call_and_an_untrue_claim_is_corrected() {
    let _lock = LOCK.lock().await;
    let vendor = VendorDouble::start().await;
    let tool = ToolDouble::start(json!({})).await;
    let gateway = provider_gateway(&vendor, &tool).await;
    // A slow lookup: the caller cancels it first.
    gateway
        .llm
        .respond_with(|_| Some(Round::text(4_000, 20, &["ANSWER:\n", "It ships Tuesday."])));
    vendor.reply("Okay, I've cancelled that.");

    let mut client = gateway.connect_to("/realtime").await;
    let extra = json!({ "regime": "delegating", "speech": { "claim_policy": "correct" } });
    client.send(realtime_config(&gateway, extra)).await;
    let created = client.until("session_created", 1).await;
    assert_eq!(created["interaction"]["regime"], "delegating", "{created}");
    vendor
        .until("session.update", 1, |event| {
            event["type"] == "session.update"
        })
        .await;
    vendor.send(ga::call(
        "resp_call",
        "call_9",
        "waav_delegate",
        json!({ "request": "where is my order" }),
    ));
    let task = client.until("task_created", 1).await;
    assert_eq!(task["origin"], "provider_tool", "{task}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    vendor.send(vec![ga::heard("cancel that please")]);
    let cancelled = client.until("task_cancelled", 1).await;
    assert_eq!(cancelled["by"], "user", "{cancelled}");
    let output = vendor
        .until("the call's output", 1, |event| {
            event["type"] == "conversation.item.create"
                && event["item"]["type"] == "function_call_output"
        })
        .await;
    assert_eq!(output["item"]["call_id"], "call_9", "{output}");
    let body: Value = serde_json::from_str(output["item"]["output"].as_str().unwrap()).unwrap();
    assert_eq!(body["status"], "cancelled", "{body}");
    // A bare response follows the output: the provider words it. Its claim is true.
    vendor
        .until("a reply", 1, |event| {
            event["type"] == "response.create" && event["response"]["instructions"].is_null()
        })
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        client.of_type("unverified_claim").is_empty(),
        "{:?}",
        client.of_type("unverified_claim")
    );

    vendor.send(ga::says(
        "resp_own",
        "Also, your refund has been processed.",
    ));
    let claim = client.until("unverified_claim", 1).await;
    assert_eq!(
        (&claim["kind"], &claim["rule"], &claim["corrected"]),
        (
            &json!("completion"),
            &json!("CompletionClaim"),
            &json!(true)
        ),
        "{claim}"
    );
    vendor
        .until("the correction", 1, |event| {
            event["response"]["instructions"]
                .as_str()
                .is_some_and(|t| t.ends_with("I'll tell you when it is."))
        })
        .await;
    // The correction itself is checked like any line of the gateway's, and passes.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        client.of_type("unverified_claim").len(),
        1,
        "{:?}",
        client.of_type("unverified_claim")
    );
}

/// r15 (live SS2): a speech-to-speech session puts the persona's prompt into its front's cache
/// as it starts, as a cascade session does, so the caller's first turn is not read cold.
#[tokio::test]
async fn a_speech_to_speech_session_warms_its_front_as_it_starts() {
    let _lock = LOCK.lock().await;
    let vendor = VendorDouble::start().await;
    let tool = ToolDouble::start(json!({})).await;
    let gateway = provider_gateway(&vendor, &tool).await;
    let front = ix_mocks::FrontDouble::start(ix_mocks::FrontScript {
        decide: Some(ix_mocks::reads_the_turn),
        ..ix_mocks::FrontScript::default()
    })
    .await;
    set_server_for_tests(ServerInteraction {
        enabled: true,
        tool_secret: Some(TOOL_SECRET.to_owned()),
        truth_ack: vec!["openai".to_owned()],
        front_model: Some(waav_gateway::core::interaction::config::FrontModelServer {
            url: front.url.clone(),
            model: None,
            api_key: None,
            command: None,
            licence: None,
        }),
        ..ServerInteraction::default()
    });
    let mut client = gateway.connect_to("/realtime").await;
    client.send(realtime_config(&gateway, json!({}))).await;
    let created = client.until("session_created", 1).await;
    assert_eq!(
        created["interaction"]["regime"], "provider_speech",
        "{created}"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while front.warmed() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        front.warmed(),
        1,
        "the decisions' slot, once, before any caller turn"
    );
}

/// Lifecycle track, with interaction mode off: a session whose text-to-speech is Hume's hears
/// that Hume shuts down on 2026-11-13 (and is refused from that day), and a `/realtime`
/// session on a model the vendor already retired is refused before anything is dialled. A
/// session whose OpenAI traffic goes to a server of the operator's is not told anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retiring_provider_is_named_and_a_retired_model_is_refused() {
    let _lock = LOCK.lock().await;
    switch(false);
    let gateway = Gateway::start().await;
    let today = waav_gateway::core::interaction::regime::today();

    let mut client = gateway.connect().await;
    let mut config = gateway.config("ix-ws-hume");
    config["tts_config"] = json!({ "provider": "hume", "voice_id": "voice",
                                   "audio_format": "pcm", "sample_rate": 16000 });
    client.send(config).await;
    let said = client.until("error", 1).await;
    let notices: Vec<Value> = client
        .events()
        .into_iter()
        .filter(|event| event["code"] == "provider_sunset" || event["code"] == "provider_retired")
        .collect();
    assert_eq!(notices.len(), 1, "{}", client.timeline());
    let notice = &notices[0];
    if today.as_str() < "2026-11-13" {
        assert_eq!(notice["type"], "config_warning");
        assert_eq!(notice["code"], "provider_sunset");
        assert_eq!(notice["detail"]["side"], "tts");
        assert_eq!(notice["detail"]["shutdown"], "2026-11-13");
        // The session went on to the key check, which fails here: there is no Hume key.
        assert_ne!(said["code"], "provider_retired");
    } else {
        assert_eq!(said["code"], "provider_retired", "{said}");
    }
    assert!(client.of_type("ready").is_empty());
    client.close().await;

    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "gemini",
                      "model": "gemini-2.0-flash-live-001" }))
        .await;
    let refused = session.until("error", 1).await;
    assert_eq!(refused["code"], "model_retired", "{refused}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    // One error: the refusal comes before the key check, and no provider was made.
    assert_eq!(session.types(), ["error"], "{}", session.timeline());
    session.close().await;

    // OpenAI's realtime default, on the operator's own server: nothing to say.
    let vendor = VendorDouble::start().await;
    let url = vendor.url.clone();
    let own = Gateway::start_with(move |config| {
        config.openai_api_key = Some("test".to_owned());
        config
            .realtime_endpoint_overrides
            .insert("openai".to_owned(), url);
    })
    .await;
    let mut session = own.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "openai" }))
        .await;
    session.until("session_created", 1).await;
    assert!(
        session.of_type("warning").is_empty(),
        "{}",
        session.timeline()
    );
    assert!(
        session.of_type("error").is_empty(),
        "{}",
        session.timeline()
    );
    session.close().await;
}

/// Mode off: a speech-to-text model that transcribes files only is refused at config with
/// what to use instead (it was dialled and failed 405), and streaming text-to-speech asked of a
/// provider without it is said to fall back to HTTP (it was silent).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_only_model_is_refused_and_a_streaming_fallback_is_said() {
    let _lock = LOCK.lock().await;
    switch(false);
    let gateway = Gateway::start().await;

    let mut client = gateway.connect().await;
    let mut config = gateway.config("ix-ws-whisper");
    config["stt_config"]["provider"] = json!("deepgram");
    config["stt_config"]["model"] = json!("whisper-large");
    client.send(config).await;
    // Refused with a streaming model to use instead: by the speech-to-text resolver when it
    // covers the session (`stt_live_unsupported`, the model in `details`), else by name.
    let refused = client.until("error", 1).await;
    let message = refused["message"].as_str().unwrap_or_default();
    let names_model =
        message.contains("whisper-large") || refused["details"]["model"] == "whisper-large";
    assert!(names_model && message.contains("nova-3"), "{refused}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(client.of_type("ready").is_empty(), "{}", client.timeline());
    client.close().await;

    let mut client = gateway.connect().await;
    let mut config = gateway.config("ix-ws-tts-streaming");
    config["tts_config"]["features"] = json!({ "streaming": true });
    client.send(config).await;
    client.until("ready", 1).await;
    let warnings = client.of_type("config_warning");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["code"], "tts_streaming_unsupported_or_degraded");
    assert_eq!(warnings[0]["detail"]["feature"], "streaming");
    client.close().await;
}

/// The provider fixes the plan ships only after each vendor's live probe (Lifecycle track) are
/// built off: with none listed the vendor gets today's frames and the client hears nothing new
/// (but `deepgram_user_text`, on by default since its probe); with the operator's
/// `provider_fixes` the vendor gets the documented frames, a withdrawn call reaches the client
/// as `function_call_cancelled`, and an escape flag still wins. A session turns a fix off for
/// itself, and on only with the operator's `provider_fixes_per_session`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_fixes_change_the_vendor_wire_only_when_the_operator_turns_them_on() {
    let _lock = LOCK.lock().await;
    let gemini = VendorDouble::start().await;
    let deepgram = VendorDouble::start().await;
    // A path: Gemini appends its `?key=` to the address.
    let gemini_url = format!("{}/gemini", gemini.url);
    let deepgram_url = format!("{}/deepgram", deepgram.url);
    let gateway = Gateway::start_with(move |config| {
        config.gemini_api_key = Some("gk".to_owned());
        config.deepgram_api_key = Some("dk".to_owned());
        let overrides = &mut config.realtime_endpoint_overrides;
        overrides.insert("gemini".to_owned(), gemini_url);
        overrides.insert("deepgram".to_owned(), deepgram_url);
    })
    .await;
    let fixes_per_session = |on: &[&str], escaped: &[&str], per_session: bool| {
        set_server_for_tests(ServerInteraction {
            provider_fixes: on.iter().map(|f| (*f).to_owned()).collect(),
            realtime_legacy_wire: escaped.iter().map(|f| (*f).to_owned()).collect(),
            provider_fixes_per_session: per_session,
            ..ServerInteraction::default()
        });
    };
    let fixes = |on: &[&str], escaped: &[&str]| fixes_per_session(on, escaped, false);
    let book = json!([{ "type": "function", "function": { "name": "book_table",
        "description": "Book a table.", "parameters": { "type": "object" } } }]);

    // One Gemini session: the setup it sent, and what the client heard of a withdrawn call.
    let gemini_session = |round: usize, own: Value| {
        let (gateway, gemini, book) = (&gateway, &gemini, book.clone());
        async move {
            let mut session = gateway.connect_to("/realtime").await;
            session
                .send(json!({ "type": "config", "provider": "gemini",
                              "model": "gemini-3.8-live", "tools": book, "provider_fixes": own }))
                .await;
            let setup = gemini
                .until("setup", round, |e| e.get("setup").is_some())
                .await;
            session.until("session_created", 1).await;
            gemini.send(vec![
                json!({ "toolCallCancellation": { "ids": ["call-7"] } }),
            ]);
            tokio::time::sleep(Duration::from_millis(400)).await;
            let heard = session.of_type("function_call_cancelled");
            let warned = session.of_type("warning");
            session.close().await;
            (setup, heard, warned)
        }
    };
    let deepgram_session = |round: usize, own: Value| {
        let (gateway, deepgram) = (&gateway, &deepgram);
        async move {
            let mut session = gateway.connect_to("/realtime").await;
            session
                .send(json!({ "type": "config", "provider": "deepgram", "provider_fixes": own }))
                .await;
            session.until("session_created", 1).await;
            session
                .send(json!({ "type": "text", "text": "Is it open?" }))
                .await;
            let typed = deepgram
                .until("typed text", round, |e| {
                    e["type"] == "InjectAgentMessage" || e["type"] == "InjectUserMessage"
                })
                .await;
            session.close().await;
            typed
        }
    };

    // Off: today's frames, but Deepgram's typed text (its probe passed: on by default).
    fixes(&[], &[]);
    let (setup, heard, warned) = gemini_session(1, Value::Null).await;
    let declaration = &setup["setup"]["tools"][0]["functionDeclarations"][0];
    assert_eq!(declaration["name"], "book_table");
    assert!(declaration.get("behavior").is_none(), "{setup}");
    assert!(
        heard.is_empty() && warned.is_empty(),
        "{heard:?} {warned:?}"
    );
    let typed = deepgram_session(1, Value::Null).await;
    assert_eq!(
        typed,
        json!({ "type": "InjectUserMessage", "content": "Is it open?" })
    );
    // A session turns it off for itself; it may not turn another on.
    let typed = deepgram_session(2, json!({ "deepgram_user_text": false })).await;
    assert_eq!(
        typed,
        json!({ "type": "InjectAgentMessage", "content": "Is it open?" })
    );
    let (setup, _, warned) = gemini_session(2, json!({ "gemini_tool_behavior": true })).await;
    let declaration = &setup["setup"]["tools"][0]["functionDeclarations"][0];
    assert!(declaration.get("behavior").is_none(), "{setup}");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0]["code"], "interaction_override_refused");
    assert_eq!(
        warned[0]["detail"]["path"],
        "provider_fixes.gemini_tool_behavior"
    );
    // With the operator's leave it may.
    fixes_per_session(&[], &[], true);
    let (setup, _, warned) = gemini_session(3, json!({ "gemini_tool_behavior": true })).await;
    assert_eq!(
        setup["setup"]["tools"][0]["functionDeclarations"][0]["behavior"], "BLOCKING",
        "{setup}"
    );
    assert!(warned.is_empty(), "{warned:?}");

    // On.
    fixes(
        &[
            "gemini_tool_behavior",
            "gemini_tool_call_cancellation",
            "deepgram_user_text",
        ],
        &[],
    );
    let (setup, heard, _) = gemini_session(4, Value::Null).await;
    assert_eq!(
        setup["setup"]["tools"][0]["functionDeclarations"][0]["behavior"], "BLOCKING",
        "{setup}"
    );
    assert_eq!(
        heard,
        [json!({ "type": "function_call_cancelled", "call_id": "call-7" })]
    );
    let typed = deepgram_session(3, Value::Null).await;
    assert_eq!(
        typed,
        json!({ "type": "InjectUserMessage", "content": "Is it open?" })
    );

    // Listed, but the operator's escape flag keeps today's wire.
    fixes(&["deepgram_user_text"], &["deepgram_text_as_agent_message"]);
    let typed = deepgram_session(4, Value::Null).await;
    assert_eq!(typed["type"], "InjectAgentMessage");
    fixes(&[], &[]);
}

/// Round 9's Hume EVI, ElevenLabs Agents and Inworld fixes over a local vendor on `/realtime`:
/// off, today's frames; on (the operator's `provider_fixes`), Hume gets `session_settings`
/// with its audio and the typed turn as `user_input`, ElevenLabs refuses an agent that is not
/// `pcm_16000` with an `error` naming the format, and Inworld gets its own voice and model.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_9_provider_fixes_reach_hume_elevenlabs_and_inworld_only_when_on() {
    let _lock = LOCK.lock().await;
    let hume = VendorDouble::start().await;
    let eleven = VendorDouble::start().await;
    let inworld = VendorDouble::start().await;
    let urls = [
        ("hume", format!("{}/hume", hume.url)),
        ("elevenlabs", format!("{}/elevenlabs", eleven.url)),
        ("inworld", format!("{}/inworld", inworld.url)),
    ];
    let gateway = Gateway::start_with(move |config| {
        config.hume_api_key = Some("hk".to_owned());
        config.elevenlabs_api_key = Some("ek".to_owned());
        config.inworld_api_key = Some("ik".to_owned());
        for (provider, url) in urls {
            config
                .realtime_endpoint_overrides
                .insert(provider.to_owned(), url);
        }
    })
    .await;
    let fixes = |on: &[&str]| {
        set_server_for_tests(ServerInteraction {
            provider_fixes: on.iter().map(|f| (*f).to_owned()).collect(),
            ..ServerInteraction::default()
        });
    };
    let typed = |e: &Value| e["type"] == "text_input" || e["type"] == "user_input";
    let hume_session = |round: usize| {
        let (gateway, hume) = (&gateway, &hume);
        async move {
            let mut session = gateway.connect_to("/realtime").await;
            session
                .send(json!({ "type": "config", "provider": "hume" }))
                .await;
            session.until("session_created", 1).await;
            session
                .send(json!({ "type": "text", "text": "Is it open?" }))
                .await;
            let text = hume.until("typed text", round, typed).await;
            session.close().await;
            text
        }
    };
    let inworld_session = || {
        let (gateway, inworld) = (&gateway, &inworld);
        async move {
            let round = inworld.of("session.update").len() + 1;
            let mut session = gateway.connect_to("/realtime").await;
            session
                .send(
                    json!({ "type": "config", "provider": "inworld", "voice": "Clive",
                              "model": "openai/gpt-4.1-mini" }),
                )
                .await;
            let update = inworld
                .until("session.update", round, |e| e["type"] == "session.update")
                .await;
            session.close().await;
            update["session"].clone()
        }
    };

    // Off: today's frames (Hume's typed text and audio settings are on by default since
    // 24 October: their escape flags restore today's).
    set_server_for_tests(ServerInteraction {
        realtime_legacy_wire: ["hume_text_input", "hume_settings_when_needed"]
            .map(str::to_owned)
            .to_vec(),
        ..ServerInteraction::default()
    });
    assert_eq!(hume_session(1).await["type"], "text_input");
    assert!(
        hume.of("session_settings").is_empty(),
        "{:?}",
        hume.received()
    );
    let session = inworld_session().await;
    assert_eq!(session["audio"]["output"]["voice"], "alloy");
    assert!(session.get("model").is_none(), "{session}");

    // On (Hume's by default).
    fixes(&[
        "elevenlabs_server_events",
        "openai_clone_voice",
        "inworld_model",
    ]);
    assert_eq!(
        hume_session(2).await,
        json!({ "type": "user_input", "text": "Is it open?" })
    );
    let settings = hume.of("session_settings");
    assert_eq!(
        settings.first().map(|s| &s["audio"]),
        Some(&json!({ "encoding": "linear16", "sample_rate": 44100, "channels": 1 })),
        "{settings:?}"
    );
    let session = inworld_session().await;
    assert_eq!(session["audio"]["output"]["voice"], "Clive");
    assert_eq!(session["model"], "openai/gpt-4.1-mini");

    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "elevenlabs", "model": "agent_1" }))
        .await;
    session.until("session_created", 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    eleven.send(vec![json!({ "type": "conversation_initiation_metadata",
        "conversation_initiation_metadata_event": { "conversation_id": "conv_1",
            "agent_output_audio_format": "pcm_44100", "user_input_audio_format": "pcm_16000" } })]);
    let error = session.until("error", 1).await;
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("pcm_44100")),
        "{error}"
    );
    session.close().await;
    fixes(&[]);
}

/// ElevenLabs' HTTP streaming text-to-speech: `features.streaming` in a `/ws` session's
/// `tts_config` sends its lines to `/v1/text-to-speech/{voice}/stream` (the server's
/// `ELEVENLABS_TTS_URL`, a local stand-in here) with the convert endpoint's request, the audio
/// reaches the client, and no `tts_streaming_unsupported_or_degraded` is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_streaming_speech_goes_to_the_stream_endpoint() {
    let _lock = LOCK.lock().await;
    switch(false);
    // PCM silence, as the vendor streams it.
    let vendor = BackendDouble::start(|_: &Hit| (200, "\0".repeat(3_200))).await;
    // Read once per process: this file's only ElevenLabs test sets it before any use.
    unsafe {
        std::env::set_var(
            "ELEVENLABS_TTS_URL",
            format!("{}/v1/text-to-speech", vendor.url),
        );
    }
    let gateway = Gateway::start_with(|config| {
        config.elevenlabs_api_key = Some("el-test-key".to_owned());
    })
    .await;
    let mut client = gateway.connect().await;
    let mut config = gateway.config("ix-ws-elevenlabs-stream");
    config["tts_config"] = json!({ "provider": "elevenlabs", "voice_id": "JBFqnCBsd6RMkjVDRZzb",
                                   "model": "eleven_flash_v2_5", "audio_format": "linear16",
                                   "sample_rate": 16000, "features": { "streaming": true } });
    client.send(config).await;
    client.until("ready", 1).await;
    client
        .send(json!({ "type": "speak", "text": "Your table is booked." }))
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while vendor.hits().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let hits = vendor.hits();
    let synthesis: Vec<&Hit> = hits.iter().filter(|hit| hit.method == "POST").collect();
    assert_eq!(synthesis.len(), 1, "{hits:?}");
    assert_eq!(
        synthesis[0].path,
        "/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb/stream?output_format=pcm_16000"
    );
    assert_eq!(synthesis[0].call.headers["xi-api-key"], "el-test-key");
    assert_eq!(
        synthesis[0].call.body,
        json!({ "text": "Your table is booked.", "model_id": "eleven_flash_v2_5" })
    );
    let audio: usize = client
        .types()
        .iter()
        .filter_map(|kind| kind.strip_prefix("audio:")?.parse::<usize>().ok())
        .sum();
    assert!(audio >= 3_200, "{}", client.types().join(" "));
    assert!(
        client.of_type("config_warning").is_empty(),
        "{:?}",
        client.of_type("config_warning")
    );
    client.close().await;
    // `POST /speak` honours the same `features`: the streaming endpoint again.
    let spoken = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/speak", gateway.port))
        .json(&json!({ "text": "See you at seven.",
                       "tts_config": { "provider": "elevenlabs", "voice_id": "JBFqnCBsd6RMkjVDRZzb",
                                       "model": "eleven_flash_v2_5", "audio_format": "linear16",
                                       "sample_rate": 16000, "features": { "streaming": true } } }))
        .send()
        .await
        .expect("POST /speak");
    assert_eq!(spoken.status(), 200);
    let audio = spoken.bytes().await.expect("the audio");
    assert!(audio.len() >= 3_200, "{}", audio.len());
    let hits = vendor.hits();
    let synthesis: Vec<&Hit> = hits.iter().filter(|hit| hit.method == "POST").collect();
    assert_eq!(synthesis.len(), 2, "{hits:?}");
    assert_eq!(
        synthesis[1].path,
        "/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb/stream?output_format=pcm_16000"
    );
    assert_eq!(
        synthesis[1].call.body,
        json!({ "text": "See you at seven.", "model_id": "eleven_flash_v2_5" })
    );
}

/// Round 9 provider fixes on the wire, through `/realtime` and the vendor doubles (each as its
/// vendor's reference has the frame): Gemini's turn transcripts end in the caller's final line
/// and an update resumes on a new connection with Google's handle and the new setup; Grok's
/// session is xAI's (`session.voice`, on by default; its escape flag keeps OpenAI's frame);
/// OpenAI's manual turns are `turn_detection: null`. Off, today's frames.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_nine_provider_fixes_change_the_gemini_grok_and_openai_wires() {
    let _lock = LOCK.lock().await;
    let gemini = VendorDouble::start().await;
    let grok = VendorDouble::start().await;
    let openai = VendorDouble::start().await;
    let urls = [
        ("gemini", format!("{}/gemini", gemini.url)),
        ("grok", format!("{}/grok", grok.url)),
        ("openai", format!("{}/openai", openai.url)),
    ];
    let gateway = Gateway::start_with(move |config| {
        config.gemini_api_key = Some("gk".to_owned());
        config.grok_api_key = Some("xk".to_owned());
        config.openai_api_key = Some("ok".to_owned());
        for (provider, url) in urls {
            config
                .realtime_endpoint_overrides
                .insert(provider.to_owned(), url);
        }
    })
    .await;
    let fixes = |on: &[&str]| {
        set_server_for_tests(ServerInteraction {
            provider_fixes: on.iter().map(|f| (*f).to_owned()).collect(),
            ..ServerInteraction::default()
        });
    };
    let is_setup = |e: &Value| e.get("setup").is_some();
    let is_update = |e: &Value| e["type"] == "session.update";
    let final_caller_lines = |session: &Client| -> Vec<Value> {
        session
            .of_type("transcript")
            .into_iter()
            .filter(|t| t["role"] == "user" && t["is_final"] == true)
            .map(|t| t["text"].clone())
            .collect()
    };
    let caller_turn = || {
        vec![
            json!({ "serverContent": { "inputTranscription": { "text": "Book a " } } }),
            json!({ "serverContent": { "inputTranscription": { "text": "table." } } }),
            json!({ "serverContent": { "turnComplete": true } }),
        ]
    };

    // Off: today's frames.
    fixes(&[]);
    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "gemini", "model": "gemini-3.8-live" }))
        .await;
    gemini.until("setup", 1, is_setup).await;
    session.until("session_created", 1).await;
    gemini.send(caller_turn());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(final_caller_lines(&session).is_empty());
    session.close().await;
    // Grok's fixes are on by default (probed 23 Oct): its escape flag keeps today's frame.
    set_server_for_tests(ServerInteraction {
        realtime_legacy_wire: vec!["grok_openai_session".to_owned()],
        ..ServerInteraction::default()
    });
    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "grok", "voice": "eve" }))
        .await;
    let update = grok.until("session.update", 1, is_update).await;
    assert_eq!(
        update["session"]["audio"]["output"]["voice"], "alloy",
        "{update}"
    );
    session.close().await;
    // OpenAI's manual turns as `null` are on by default since 24 October: its escape flag.
    set_server_for_tests(ServerInteraction {
        realtime_legacy_wire: vec!["openai_turn_detection_none".to_owned()],
        ..ServerInteraction::default()
    });
    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "openai",
                      "turn_detection": { "mode": "manual" } }))
        .await;
    let update = openai.until("session.update", 1, is_update).await;
    assert_eq!(
        update["session"]["audio"]["input"]["turn_detection"],
        json!({ "type": "none" })
    );
    session.close().await;

    // On (Grok's and OpenAI's manual turns by default).
    fixes(&["gemini_turn_transcripts", "gemini_update_resume"]);
    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(
            json!({ "type": "config", "provider": "gemini", "model": "gemini-3.8-live",
                      "instructions": "Be brief." }),
        )
        .await;
    gemini.until("setup", 2, is_setup).await;
    session.until("session_created", 1).await;
    gemini.send(vec![
        json!({ "sessionResumptionUpdate": { "newHandle": "h-9", "resumable": true } }),
    ]);
    gemini.send(caller_turn());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(final_caller_lines(&session), [json!("Book a table.")]);
    session
        .send(json!({ "type": "update_session", "instructions": "Answer in French." }))
        .await;
    let resumed = gemini.until("setup", 3, is_setup).await;
    assert_eq!(
        resumed["setup"]["sessionResumption"],
        json!({ "handle": "h-9" }),
        "{resumed}"
    );
    assert_eq!(
        resumed["setup"]["systemInstruction"],
        json!({ "parts": [{ "text": "Answer in French." }] })
    );
    session.close().await;
    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "grok", "voice": "eve" }))
        .await;
    let update = grok.until("session.update", 2, is_update).await;
    assert_eq!(update["session"]["voice"], "eve", "{update}");
    assert!(update["session"]["audio"]["output"].get("voice").is_none());
    session.close().await;
    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "openai",
                      "turn_detection": { "mode": "manual" } }))
        .await;
    let update = openai.until("session.update", 2, is_update).await;
    let input = update["session"]["audio"]["input"].as_object().unwrap();
    assert_eq!(input.get("turn_detection"), Some(&Value::Null), "{update}");
    session.close().await;
    fixes(&[]);
}

/// r11-fix: a Grok session's `session_created` names the model xAI's `session.created` reports
/// (`grok_events`, on by default; live 24 Oct `grok-voice-3.0`), not `""`; with the escape flag
/// `grok_openai_events` the session's own model, as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_eleven_session_created_names_the_model_xai_reports() {
    let _lock = LOCK.lock().await;
    let grok = VendorDouble::start().await;
    let url = format!("{}/grok", grok.url);
    let gateway = Gateway::start_with(move |config| {
        config.grok_api_key = Some("xk".to_owned());
        config
            .realtime_endpoint_overrides
            .insert("grok".to_owned(), url);
    })
    .await;
    let is_update = |e: &Value| e["type"] == "session.update";
    let created = json!({ "type": "session.created",
                          "session": { "id": "s", "model": "grok-voice-3.0" } });
    set_server_for_tests(ServerInteraction::default());
    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "grok" }))
        .await;
    grok.until("session.update", 1, is_update).await;
    grok.send(vec![created.clone()]);
    let ready = session.until("session_created", 1).await;
    assert_eq!(ready["model"], "grok-voice-3.0", "{ready}");
    session.close().await;
    set_server_for_tests(ServerInteraction {
        realtime_legacy_wire: vec!["grok_openai_events".to_owned()],
        ..ServerInteraction::default()
    });
    let mut session = gateway.connect_to("/realtime").await;
    session
        .send(json!({ "type": "config", "provider": "grok" }))
        .await;
    grok.until("session.update", 2, is_update).await;
    grok.send(vec![created]);
    let ready = session.until("session_created", 1).await;
    assert_eq!(ready["model"], "", "{ready}");
    session.close().await;
    set_server_for_tests(ServerInteraction::default());
}
