//! A Bud session's STT and TTS records carry their vendor exchange as children (Observability shows
//! what was sent to the vendor under each record, as it shows an LLM call under an agent turn):
//!
//! * a `speak` reaches the TTS vendor inside the scope its record returned, so the provider's
//!   vendor request is traced beneath the record;
//! * a final transcript's record gets a vendor span carrying the URL the STT connection opened and
//!   the vendor's answer, and the connection itself is one vendor span beneath the session.
//!
//! Its own binary: it registers mock vendors in the global provider registry and installs a
//! thread-local subscriber.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

use waav_gateway::core::stt::{BaseSTT, STTConfig, STTError, STTResult, STTResultCallback};
use waav_gateway::core::tts::{AudioCallback, BaseTTS, TTSConfig, TTSResult};
use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
use waav_gateway::global_registry;
use waav_gateway::observability::vendor_span::VendorScope;
use waav_gateway::plugin::metadata::ProviderMetadata;

const STT_URL: &str = "wss://stt.example/v1/listen?model=scope-model&language=en-US";

/// The scope each `speak` reached the vendor in: its parent span's id.
static SPEAK_SCOPES: once_cell::sync::Lazy<Mutex<Vec<Option<u64>>>> =
    once_cell::sync::Lazy::new(Mutex::default);
/// The result callback the manager registered, to deliver a transcript through.
static STT_CALLBACK: once_cell::sync::Lazy<Mutex<Option<STTResultCallback>>> =
    once_cell::sync::Lazy::new(Mutex::default);

struct ScopeTts {
    ready: bool,
}

#[async_trait::async_trait]
impl BaseTTS for ScopeTts {
    fn new(_config: TTSConfig) -> TTSResult<Self> {
        Ok(Self { ready: false })
    }
    async fn connect(&mut self) -> TTSResult<()> {
        self.ready = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> TTSResult<()> {
        self.ready = false;
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.ready
    }
    async fn speak(&mut self, _text: &str, _flush: bool) -> TTSResult<()> {
        // Where a provider queues its request, as the shared HTTP provider does.
        SPEAK_SCOPES
            .lock()
            .push(VendorScope::current().and_then(|s| s.parent().id().map(|i| i.into_u64())));
        Ok(())
    }
    async fn flush(&self) -> TTSResult<()> {
        Ok(())
    }
    async fn clear(&mut self) -> TTSResult<()> {
        Ok(())
    }
    fn on_audio(&mut self, _callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        Ok(())
    }
    fn get_provider_info(&self) -> Value {
        json!({"provider": "scope-tts"})
    }
}

struct ScopeStt {
    config: STTConfig,
    ready: bool,
}

#[async_trait::async_trait]
impl BaseSTT for ScopeStt {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        Ok(Self {
            config,
            ready: false,
        })
    }
    async fn connect(&mut self) -> Result<(), STTError> {
        self.ready = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<(), STTError> {
        self.ready = false;
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.ready
    }
    async fn send_audio(&mut self, _audio: Bytes) -> Result<(), STTError> {
        Ok(())
    }
    async fn on_result(&mut self, cb: STTResultCallback) -> Result<(), STTError> {
        *STT_CALLBACK.lock() = Some(cb);
        Ok(())
    }
    async fn on_error(
        &mut self,
        _cb: waav_gateway::core::stt::STTErrorCallback,
    ) -> Result<(), STTError> {
        Ok(())
    }
    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.config)
    }
    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        self.config = config;
        Ok(())
    }
    fn vendor_url(&self) -> Option<String> {
        Some(STT_URL.to_string())
    }
    fn get_provider_info(&self) -> &'static str {
        "scope-stt"
    }
}

/// (id, parent id, fields) per span.
#[derive(Clone, Default)]
struct Spans(Arc<Mutex<Vec<(u64, Option<u64>, HashMap<String, String>)>>>);

struct V<'a>(&'a mut HashMap<String, String>);
impl Visit for V<'_> {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        self.0.insert(f.name().to_string(), format!("{v:?}"));
    }
    fn record_str(&mut self, f: &Field, v: &str) {
        self.0.insert(f.name().to_string(), v.to_string());
    }
}

struct Capture(Spans);
impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Capture {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        fields.insert("<name>".into(), attrs.metadata().name().to_string());
        attrs.record(&mut V(&mut fields));
        let parent = attrs.parent().map(|p| p.into_u64());
        self.0.0.lock().push((id.into_u64(), parent, fields));
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        let mut all = self.0.0.lock();
        if let Some((_, _, f)) = all.iter_mut().rev().find(|(i, _, _)| *i == id.into_u64()) {
            values.record(&mut V(f));
        }
    }
}

#[tokio::test]
async fn each_record_carries_its_vendor_exchange() {
    let spans = Spans::default();
    let _sub = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(Capture(spans.clone())),
    );
    let registry = global_registry();
    registry.register_tts(
        "scope-tts",
        Arc::new(|c: TTSConfig| ScopeTts::new(c).map(|t| Box::new(t) as Box<dyn BaseTTS>)),
        ProviderMetadata::tts("scope-tts", "Scope TTS"),
    );
    registry.register_stt(
        "scope-stt",
        Arc::new(|c: STTConfig| ScopeStt::new(c).map(|s| Box::new(s) as Box<dyn BaseSTT>)),
        ProviderMetadata::stt("scope-stt", "Scope STT"),
    );
    let vm = VoiceManager::new(
        VoiceManagerConfig::new(
            STTConfig {
                provider: "scope-stt".into(),
                model: "scope-model".into(),
                api_key: "stt-secret".into(),
                ..Default::default()
            },
            TTSConfig {
                provider: "scope-tts".into(),
                api_key: "t".into(),
                ..Default::default()
            },
        ),
        None,
    )
    .expect("voice manager");

    let session = tracing::info_span!(parent: None, "voice.ws_session");
    let tts_record = tracing::info_span!(parent: None, "voice.turn", leg = "tts");
    let stt_record = tracing::info_span!(parent: None, "voice.turn", leg = "stt");
    let (session_id, tts_id, stt_id) = (
        session.id().unwrap().into_u64(),
        tts_record.id().unwrap().into_u64(),
        stt_record.id().unwrap().into_u64(),
    );
    vm.set_stt_vendor_scope(VendorScope::new(session.clone(), "transcription"));
    let r = tts_record.clone();
    vm.set_speak_observer(Arc::new(move |_text: &str| {
        Some(VendorScope::new(r.clone(), "text_to_speech"))
    }));
    let r = stt_record.clone();
    vm.set_stt_final_observer(Arc::new(move |_text: &str| {
        Some(VendorScope::new(r.clone(), "transcription"))
    }));
    vm.on_stt_result(|_r: STTResult| Box::pin(async {}))
        .await
        .unwrap();
    vm.on_tts_audio(|_a| Box::pin(async {})).await.unwrap();
    vm.start().await.expect("start");

    // TTS: the provider is asked to speak inside the record's scope.
    vm.speak("Hello there.", true).await.unwrap();
    assert_eq!(*SPEAK_SCOPES.lock(), vec![Some(tts_id)]);

    // STT: the connection is a vendor call beneath the session, with the URL it opened...
    let calls = |spans: &Spans| -> Vec<(Option<u64>, HashMap<String, String>)> {
        spans
            .0
            .lock()
            .iter()
            .filter(|(_, _, f)| f["<name>"] == "voice.vendor_call")
            .map(|(_, p, f)| (*p, f.clone()))
            .collect()
    };
    let connection = calls(&spans);
    assert_eq!(connection.len(), 1);
    assert_eq!(connection[0].0, Some(session_id));
    assert_eq!(connection[0].1["url.full"], STT_URL);

    // ...and a final transcript's record gets the vendor's answer beneath it.
    let cb = STT_CALLBACK
        .lock()
        .clone()
        .expect("the manager registered a result callback");
    cb(STTResult::new("hello".into(), true, true, 0.93)).await;
    let all = calls(&spans);
    assert_eq!(all.len(), 2);
    let (parent, utterance) = &all[1];
    assert_eq!(*parent, Some(stt_id));
    assert_eq!(utterance["otel.name"], "transcription scope-model");
    assert_eq!(utterance["url.full"], STT_URL);
    let answer: Value = serde_json::from_str(&utterance["gen_ai.response.body"]).unwrap();
    assert_eq!(answer["transcript"], "hello", "what the vendor answered");
    assert_eq!(answer["is_final"], true);
    assert!(
        !utterance.contains_key("error.type"),
        "a completed exchange, not an abandoned one: {utterance:?}"
    );
    assert!(
        !all.iter()
            .any(|(_, f)| f.values().any(|v| v.contains("stt-secret")))
    );

    // A partial is not a record: nothing new.
    cb(STTResult::new("hel".into(), false, false, 0.5)).await;
    assert_eq!(calls(&spans).len(), 2);
    vm.stop().await.unwrap();
}
