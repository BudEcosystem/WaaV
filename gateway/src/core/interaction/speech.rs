//! The ordered speech worker: one task that awaits each `SpeechCmd` (speak, cut) on the
//! VoiceManager before taking the next, so a cut followed by a speak can never lose the new line.

use std::sync::Arc;

use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::{debug, warn};

use crate::core::audio::PumpControl;
use crate::core::voice_manager::VoiceManager;

use super::types::{PumpEvent, SpeechCmd};

/// Spawn the worker; it ends when every sender is dropped. `events` is the pump's event channel:
/// a speak the VoiceManager refuses is reported there as `PumpEvent::Failed`.
pub fn spawn(
    vm: Arc<VoiceManager>,
    ctl: Arc<PumpControl>,
    events: UnboundedSender<PumpEvent>,
) -> UnboundedSender<SpeechCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            // How long each command holds the worker: a line waits behind the ones before it.
            let (began, what) = (std::time::Instant::now(), speech_cmd_name(&cmd));
            match cmd {
                SpeechCmd::Speak { utt, text, discard } => {
                    ctl.expect(utt, discard);
                    // Read after any earlier clear returned, so only a clear that races this
                    // very speak can refuse it.
                    let epoch = vm.clear_epoch();
                    let spoken = if discard {
                        // A warm-up line goes into the TTS cache and is billed to nobody.
                        let warmed = vm.warm_if_epoch(&text, epoch).await;
                        debug!(utt = utt.0, "interaction warm-up line synthesised");
                        warmed
                    } else {
                        // A line from the speech cache is no sign that the voice works
                        // (chapter 10's watcher).
                        if vm.tts_cached(&text).await {
                            ctl.note_cached(utt);
                        }
                        vm.speak_if_epoch(&text, true, true, epoch).await
                    };
                    if !matches!(spoken, Ok(true)) {
                        if let Err(error) = spoken {
                            warn!(%error, "interaction speak failed");
                        }
                        ctl.fail(utt);
                        let _ = events.send(PumpEvent::Failed { utt });
                    }
                }
                SpeechCmd::Clip { utt, text } => {
                    // Synthesised like a warm-up line, its audio kept in the clip bank.
                    ctl.capture(utt);
                    let epoch = vm.clear_epoch();
                    let rendered = vm.warm_if_epoch(&text, epoch).await;
                    // Logged as a warm-up line: like one, it holds the provider at start.
                    debug!(utt = utt.0, "interaction warm-up line synthesised (clip)");
                    if !matches!(rendered, Ok(true)) {
                        if let Err(error) = rendered {
                            warn!(%error, "interaction clip render failed");
                        }
                        ctl.fail(utt);
                        let _ = events.send(PumpEvent::Failed { utt });
                    }
                }
                SpeechCmd::Rate(factor) => {
                    let taken = vm.set_speaking_rate_factor(factor).await;
                    debug!(factor, taken, "interaction speaking rate");
                }
                SpeechCmd::Voice(voice) => {
                    let taken = vm.set_tts_voice(&voice).await;
                    debug!(%voice, taken, "interaction voice");
                }
                SpeechCmd::Cut => {
                    // Again, for a speak that was queued here before the actor's own cut.
                    ctl.cut();
                    if let Err(error) = vm.clear_tts().await {
                        warn!(%error, "interaction clear failed");
                    }
                }
                SpeechCmd::CutAudio => {
                    ctl.cut_keeping_warm();
                    vm.clear_audio_out().await;
                }
            }
            let ms = began.elapsed().as_millis() as u64;
            debug!(cmd = what, ms, "interaction speech command done");
        }
    });
    tx
}

fn speech_cmd_name(cmd: &SpeechCmd) -> &'static str {
    match cmd {
        SpeechCmd::Speak { discard: true, .. } => "warm",
        SpeechCmd::Speak { .. } => "speak",
        SpeechCmd::Clip { .. } => "clip",
        SpeechCmd::Rate(_) => "rate",
        SpeechCmd::Voice(_) => "voice",
        SpeechCmd::Cut => "cut",
        SpeechCmd::CutAudio => "cut_audio",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::interaction::types::UttId;
    use crate::core::stt::STTConfig;
    use crate::core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSResult};
    use crate::core::voice_manager::VoiceManagerConfig;
    use crate::plugin::metadata::ProviderMetadata;
    use serial_test::serial;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::Semaphore;
    use tokio::sync::mpsc::UnboundedReceiver;

    /// The text whose synthesis blocks until `HOLD` gets a permit.
    const HELD: &str = "hold";
    static HOLD: Semaphore = Semaphore::const_new(0);

    /// The mock TTS of `tests/conversation_loop.rs`: one PCM chunk per speak, then completion.
    /// The chunk is 200 ms of 16 kHz audio whose every sample is the first byte of the text.
    struct MockTts {
        callback: Option<Arc<dyn AudioCallback>>,
    }

    #[async_trait::async_trait]
    impl BaseTTS for MockTts {
        fn new(_config: TTSConfig) -> TTSResult<Self> {
            Ok(Self { callback: None })
        }

        async fn connect(&mut self) -> TTSResult<()> {
            Ok(())
        }

        async fn disconnect(&mut self) -> TTSResult<()> {
            Ok(())
        }

        fn is_ready(&self) -> bool {
            true
        }

        async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
            if text == HELD {
                // Holds the provider (the VoiceManager's TTS lock) and produces nothing.
                HOLD.acquire().await.expect("never closed").forget();
                return Ok(());
            }
            if let Some(cb) = &self.callback {
                let sample = text.as_bytes()[0] as i16;
                cb.on_audio(AudioData {
                    data: sample.to_le_bytes().repeat(3200),
                    sample_rate: 16_000,
                    format: "pcm".to_string(),
                    duration_ms: None,
                })
                .await;
                cb.on_complete().await;
            }
            Ok(())
        }

        async fn flush(&self) -> TTSResult<()> {
            Ok(())
        }

        async fn clear(&mut self) -> TTSResult<()> {
            Ok(())
        }

        fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
            self.callback = Some(callback);
            Ok(())
        }

        fn get_provider_info(&self) -> serde_json::Value {
            serde_json::json!({ "provider": "mock-tts-ix-speech" })
        }
    }

    struct Rig {
        vm: Arc<VoiceManager>,
        ctl: Arc<PumpControl>,
        speech: UnboundedSender<SpeechCmd>,
        events: UnboundedReceiver<PumpEvent>,
        /// The first sample of every frame that reached the egress callback.
        played: Arc<Mutex<Vec<u8>>>,
    }

    /// A VoiceManager on the mock TTS with the metered pump, wired as `handlers/ws/interaction.rs`
    /// wires a session: control set before the egress callback, completion chained to the control.
    async fn rig() -> Rig {
        static REGISTER: std::sync::Once = std::sync::Once::new();
        REGISTER.call_once(|| {
            crate::plugin::global_registry().register_tts(
                "mock-tts-ix-speech",
                Arc::new(|config: TTSConfig| {
                    MockTts::new(config).map(|tts| Box::new(tts) as Box<dyn BaseTTS>)
                }),
                ProviderMetadata::tts("mock-tts-ix-speech", "Mock TTS (interaction speech worker)"),
            );
        });
        let stt = STTConfig {
            provider: "deepgram".to_string(),
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let tts = TTSConfig {
            provider: "mock-tts-ix-speech".to_string(),
            ..Default::default()
        };
        let vm = Arc::new(
            VoiceManager::new(VoiceManagerConfig::new(stt, tts), None).expect("voice manager"),
        );

        let (tx, events) = mpsc::unbounded_channel();
        let ctl = Arc::new(PumpControl::new(tx.clone(), 0));
        vm.set_metered_playback(Some(ctl.clone()));
        let played = Arc::new(Mutex::new(Vec::new()));
        let sink = played.clone();
        vm.on_tts_audio(move |audio: AudioData| {
            sink.lock().unwrap().push(audio.data[0]);
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

        let speech = spawn(vm.clone(), ctl.clone(), tx);
        Rig {
            vm,
            ctl,
            speech,
            events,
            played,
        }
    }

    impl Rig {
        fn speak(&self, utt: u64, text: &str) {
            let cmd = SpeechCmd::Speak {
                utt: UttId(utt),
                text: text.to_string(),
                discard: false,
            };
            self.speech.send(cmd).expect("worker alive");
        }

        fn events(&mut self) -> Vec<PumpEvent> {
            std::iter::from_fn(|| self.events.try_recv().ok()).collect()
        }
    }

    async fn after(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    #[tokio::test(start_paused = true)]
    #[serial]
    async fn cut_then_speak_plays_the_new_line() {
        let mut rig = rig().await;
        rig.speak(1, "a long first line");
        after(50).await;
        assert_eq!(*rig.played.lock().unwrap(), [b'a'; 3], "20 ms frames");

        // The actor's order: its own cut, then `Cut` and the next `Speak` to the worker.
        assert_eq!(rig.ctl.cut(), [(UttId(1), 60)]);
        rig.speech.send(SpeechCmd::Cut).expect("worker alive");
        rig.speak(2, "b, the new line");
        after(500).await;

        let played = rig.played.lock().unwrap().clone();
        assert_eq!(played[..3], [b'a'; 3], "the first line stopped at the cut");
        assert_eq!(played[3..], [b'b'; 10], "the new line played whole");
        let events = rig.events();
        assert!(!events.iter().any(|e| matches!(e, PumpEvent::Failed { .. })));
        assert!(events.contains(&PumpEvent::SynthDone { utt: UttId(2) }));
        assert!(matches!(
            events.last(),
            Some(PumpEvent::Idle {
                utt: UttId(2),
                delivered_ms: 200,
                ..
            })
        ));
    }

    #[tokio::test(start_paused = true)]
    #[serial]
    async fn a_clip_is_kept_in_the_bank_and_played_3_db_down_on_demand() {
        let mut rig = rig().await;
        let render = SpeechCmd::Clip {
            utt: UttId(1),
            text: "mhm".to_owned(),
        };
        rig.speech.send(render).expect("worker alive");
        after(100).await;
        assert!(
            rig.played.lock().unwrap().is_empty(),
            "a clip is not played when it is made"
        );
        assert!(
            rig.events()
                .contains(&PumpEvent::SynthDone { utt: UttId(1) })
        );
        // The session checks it and puts it in the bank.
        assert_eq!(rig.ctl.clips(), 0);
        let clip = rig.ctl.take_captured(UttId(1)).expect("its audio is kept");
        assert!(rig.ctl.take_captured(UttId(1)).is_none());
        rig.ctl.set_clips(vec![clip]);
        assert_eq!(rig.ctl.clips(), 1);

        assert!(rig.ctl.play_clip(0, UttId(1 << 40)));
        after(500).await;
        // Every sample was b'm' (109): 3 dB down it is 77.
        assert_eq!(*rig.played.lock().unwrap(), [77; 10]);
        let events = rig.events();
        let clip = UttId(1 << 40);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PumpEvent::Started { utt, .. } if *utt == clip))
        );
        assert!(matches!(
            events.last(),
            Some(PumpEvent::Idle { utt, delivered_ms: 200, .. }) if *utt == UttId(1 << 40)
        ));
        // The next utterance plays as usual.
        rig.speak(2, "b");
        after(500).await;
        assert_eq!(rig.played.lock().unwrap().last(), Some(&b'b'));
    }

    #[tokio::test(start_paused = true)]
    #[serial]
    async fn a_speak_queued_before_the_cut_is_cut_by_the_worker() {
        let mut rig = rig().await;
        // The actor cut first; the worker still holds an older speak, then the cut.
        rig.ctl.cut();
        rig.speak(1, "a stale line");
        rig.speech.send(SpeechCmd::Cut).expect("worker alive");
        rig.speak(2, "b");
        after(500).await;

        let played = rig.played.lock().unwrap().clone();
        assert_eq!(
            played[played.len() - 10..],
            [b'b'; 10],
            "the new line played whole"
        );
        assert!(
            played.len() <= 11,
            "at most one frame of the stale line: {played:?}"
        );
        let events = rig.events();
        assert!(!events.iter().any(|e| matches!(e, PumpEvent::Failed { .. })));
    }

    #[tokio::test(start_paused = true)]
    #[serial]
    async fn a_speak_refused_by_the_epoch_yields_failed() {
        let mut rig = rig().await;
        // Something else holds the TTS provider (a slow synthesis), a clear waits behind it,
        // and the worker's speak waits behind the clear: the clear bumps the epoch first.
        let vm = rig.vm.clone();
        let held = tokio::spawn(async move { vm.speak(HELD, true).await });
        after(1).await;
        let vm = rig.vm.clone();
        let clear = tokio::spawn(async move { vm.clear_tts().await });
        after(1).await;
        rig.speak(7, "never spoken");
        after(1).await;
        HOLD.add_permits(1);
        after(500).await;

        held.await.expect("join").expect("held speak");
        clear.await.expect("join").expect("clear");
        assert_eq!(rig.events(), [PumpEvent::Failed { utt: UttId(7) }]);
        assert!(
            !rig.played.lock().unwrap().contains(&b'n'),
            "the refused line never played"
        );

        // The worker goes on: the next line plays.
        rig.speak(8, "b");
        after(500).await;
        assert_eq!(rig.played.lock().unwrap().last(), Some(&b'b'));
        assert!(
            rig.events()
                .contains(&PumpEvent::SynthDone { utt: UttId(8) })
        );
    }
}
