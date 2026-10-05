//! `voice_agent:{prompt_id}:v{version}` — what makes a Bud agent a VOICE agent (spec 025 §5.3).
//!
//! budapp is the single writer. It projects the agent version's `voice` block — which STT and TTS
//! deployments the agent speaks through, the voice, turn-taking, interruption, greeting, fillers —
//! and nothing else: no prompt text, no tool definitions, no credentials (FRD S-5). The agent's
//! behaviour stays in budprompt, which WaaV reaches per turn through budgateway with the caller's
//! own credential.
//!
//! `prompt_id` is the field the caller's `prompt:<name>` alias entry carries: the budapp prompt UUID
//! for a saved agent, the draft id for a playground draft. A session resolves
//! `alias -> {prompt_id, version} -> this entry -> two voice_table entries`, all in memory.
//!
//! Parsing is LENIENT the way `voice_table` is (FRD-022 §6.1): a malformed optional block is
//! dropped to its defaults with a warning, never the whole entry. Only what a session cannot run
//! without is required — the agent's identity and both legs' deployments.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Redis prefix of the projection.
pub const VOICE_AGENT_PREFIX: &str = "voice_agent:";

/// The snapshot key for an agent version: `"{prompt_id}:v{version}"`.
pub fn voice_agent_key(prompt_id: &str, version: i64) -> String {
    format!("{prompt_id}:v{version}")
}

/// The speech-to-text leg.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSttLeg {
    pub endpoint_id: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub keyterms: Vec<String>,
    #[serde(default)]
    pub settings: serde_json::Map<String, Value>,
}

/// The text-to-speech leg.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentTtsLeg {
    pub endpoint_id: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub settings: serde_json::Map<String, Value>,
}

/// How the end of the user's turn is decided.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentTurnDetection {
    /// `semantic` (SmartTurn ensemble) | `server_vad` | `manual`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `low` | `medium` | `high` | `auto`.
    pub eagerness: String,
    pub silence_ms: u64,
    /// The longest silence before the turn ends, when the agent chose one. Optional so a value
    /// left out can be told from a chosen one; the gateway still reads exactly 3,000 (the default
    /// Bud publishes) as unset until Bud stops publishing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_endpointing_ms: Option<u64>,
}

impl AgentTurnDetection {
    /// The ceiling the agent chose, or `None` when it left the default.
    pub fn chosen_max_endpointing_ms(&self) -> Option<u64> {
        self.max_endpointing_ms.filter(|v| *v != 3000)
    }
}

impl Default for AgentTurnDetection {
    fn default() -> Self {
        Self {
            kind: "semantic".into(),
            eagerness: "auto".into(),
            silence_ms: 500,
            max_endpointing_ms: None,
        }
    }
}

/// When the user talking over the agent stops it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentInterruption {
    pub enabled: bool,
    pub min_words: usize,
    pub min_speech_ms: u64,
    pub ignore_phrases: Vec<String>,
    pub false_interruption_timeout_ms: u64,
}

impl Default for AgentInterruption {
    fn default() -> Self {
        Self {
            enabled: true,
            min_words: 1,
            min_speech_ms: 300,
            ignore_phrases: vec![
                "uh-huh".into(),
                "yeah".into(),
                "okay".into(),
                "mm-hmm".into(),
            ],
            false_interruption_timeout_ms: 2000,
        }
    }
}

/// What the agent says when the call opens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentGreeting {
    /// `none` | `static`.
    pub mode: String,
    pub text: Option<String>,
    pub interruptible: bool,
}

impl Default for AgentGreeting {
    fn default() -> Self {
        Self {
            mode: "none".into(),
            text: None,
            interruptible: false,
        }
    }
}

impl AgentGreeting {
    /// The text to speak at session start, when there is one.
    pub fn static_text(&self) -> Option<&str> {
        (self.mode == "static")
            .then_some(self.text.as_deref())
            .flatten()
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }
}

/// What fills silence while a tool runs or the model is slow.
///
/// `messages` is a script for one wait, said in order: the first after `slow_response_after_ms`
/// (or a tool's own phrase after `tool_call_after_ms`), then the next every `follow_up_after_ms`
/// while the caller is still waiting. Every turn starts again from the first phrase and the list
/// never wraps, so the phrase heard says how long the caller has waited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentFillers {
    /// 0 disables tool fillers.
    pub tool_call_after_ms: u64,
    /// 0 disables slow-response fillers.
    pub slow_response_after_ms: u64,
    /// 0 says one filler per wait.
    pub follow_up_after_ms: u64,
    pub messages: Vec<String>,
    pub use_tool_status_messages: bool,
    /// A gentle pulsing tone while a tool runs, after its first phrase, so the caller never hears
    /// dead air. On by default; while it plays, no further phrase is said for that wait.
    pub tool_call_sound: bool,
}

impl Default for AgentFillers {
    fn default() -> Self {
        Self {
            tool_call_after_ms: 1200,
            slow_response_after_ms: 2500,
            follow_up_after_ms: 3000,
            messages: ["Hmm...", "One moment.", "Still checking..."]
                .map(String::from)
                .to_vec(),
            use_tool_status_messages: true,
            tool_call_sound: true,
        }
    }
}

/// Deterministic clean-up applied to the agent's text before it is spoken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentTextTransforms {
    pub strip_markdown: bool,
    pub strip_emoji: bool,
    /// `omit` | `read`.
    pub code_blocks: String,
    /// `domain` | `full` | `omit`.
    pub urls: String,
}

impl Default for AgentTextTransforms {
    fn default() -> Self {
        Self {
            strip_markdown: true,
            strip_emoji: true,
            code_blocks: "omit".into(),
            urls: "domain".into(),
        }
    }
}

/// Which field of a structured-output agent's JSON is spoken.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentStructuredOutput {
    pub speak_field: Option<String>,
    pub has_output_schema: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentIdle {
    pub end_after_silence_ms: u64,
}

impl Default for AgentIdle {
    fn default() -> Self {
        Self {
            end_after_silence_ms: 120_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentLimits {
    pub max_session_seconds: u64,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_session_seconds: 1800,
        }
    }
}

/// The agent's structured input, as far as a session must know it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentInput {
    pub required_variables: Vec<String>,
    pub has_input_schema: bool,
}

pub const DEFAULT_DEGRADATION_MESSAGE: &str =
    "Sorry, I'm having trouble right now. Please try again.";
pub const DEFAULT_APPROVAL_MESSAGE: &str = "I need approval before I can do that.";

/// One agent version's voice projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceAgentEntry {
    pub v: u32,
    pub prompt_id: String,
    pub prompt_name: Option<String>,
    pub version: i64,
    pub project_id: Option<String>,
    pub stt: AgentSttLeg,
    pub tts: AgentTtsLeg,
    pub turn_detection: AgentTurnDetection,
    pub interruption: AgentInterruption,
    pub greeting: AgentGreeting,
    pub fillers: AgentFillers,
    pub text_transforms: AgentTextTransforms,
    pub structured_output: AgentStructuredOutput,
    pub idle: AgentIdle,
    pub limits: AgentLimits,
    pub session_overrides: Vec<String>,
    pub degradation_message: String,
    pub approval_message: String,
    pub input: AgentInput,
    /// `{tool_key: [phrases]}` from budprompt's generated tool status messages.
    pub tool_status_messages: HashMap<String, Vec<String>>,
    /// Whether a speculative (pre-empted) turn may run: only a tool-free agent (NG-4).
    pub allow_preemptive: bool,
}

impl VoiceAgentEntry {
    /// The snapshot key this entry is stored under.
    pub fn key(&self) -> String {
        voice_agent_key(&self.prompt_id, self.version)
    }

    /// Whether the agent author lets a caller change `setting` for one session (FRD S-4).
    pub fn allows_override(&self, setting: &str) -> bool {
        self.session_overrides.iter().any(|s| s == setting)
    }

    /// A spoken phrase for a tool that is running, when the agent has one for it.
    ///
    /// budprompt keys its phrases by `tool_key`, which for an MCP tool is the server's name joined
    /// to the tool's; the stream names the tool alone. A key that ENDS WITH the tool name is
    /// therefore the match.
    pub fn tool_phrase(&self, tool_name: &str) -> Option<&str> {
        if tool_name.is_empty() {
            return None;
        }
        self.tool_status_messages
            .iter()
            .find(|(key, _)| {
                key.as_str() == tool_name
                    || key.ends_with(&format!("__{tool_name}"))
                    || key.ends_with(&format!(".{tool_name}"))
                    || key.ends_with(&format!(":{tool_name}"))
            })
            .and_then(|(_, phrases)| phrases.first().map(String::as_str))
    }
}

fn required_str(obj: &serde_json::Map<String, Value>, field: &str) -> Result<String, String> {
    obj.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("`{field}` is missing or not a non-empty string"))
}

/// One optional block: its value, or its default with a warning when it does not parse.
fn block<T: serde::de::DeserializeOwned + Default>(
    obj: &serde_json::Map<String, Value>,
    field: &str,
    key: &str,
) -> T {
    match obj.get(field) {
        None | Some(Value::Null) => T::default(),
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_else(|e| {
            tracing::warn!(
                key = %key,
                field = %field,
                error = %e,
                "voice_agent block did not parse; using its defaults (the rest of the entry stands)"
            );
            T::default()
        }),
    }
}

/// Every top-level field this build models. Anything else is accepted and logged: a newer budapp's
/// field must not take an agent off the air on an older WaaV.
const KNOWN_FIELDS: &[&str] = &[
    "v",
    "prompt_id",
    "prompt_name",
    "version",
    "project_id",
    "stt",
    "tts",
    "turn_detection",
    "interruption",
    "greeting",
    "fillers",
    "text_transforms",
    "structured_output",
    "idle",
    "limits",
    "session_overrides",
    "degradation_message",
    "approval_message",
    "input",
    "tool_status_messages",
    "allow_preemptive",
];

/// Parse one `voice_agent:` blob.
///
/// Errors only for what a session cannot run without: the agent's id and version, and both legs'
/// `endpoint_id`. Everything else degrades to its default.
pub fn parse_voice_agent_blob(raw: &str) -> Result<VoiceAgentEntry, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| format!("not json: {e}"))?;
    let Value::Object(obj) = value else {
        return Err("entry is not an object".into());
    };
    let prompt_id = required_str(&obj, "prompt_id")?;
    let version = obj
        .get("version")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .filter(|v| *v >= 1)
        .ok_or_else(|| "`version` is missing or not a positive integer".to_string())?;
    let key = voice_agent_key(&prompt_id, version);

    let leg = |field: &str| -> Result<serde_json::Map<String, Value>, String> {
        match obj.get(field) {
            Some(Value::Object(m)) => {
                required_str(m, "endpoint_id").map_err(|e| format!("{field}: {e}"))?;
                Ok(m.clone())
            }
            _ => Err(format!("`{field}` is missing or not an object")),
        }
    };
    let stt_obj = leg("stt")?;
    let tts_obj = leg("tts")?;
    let stt = serde_json::from_value::<AgentSttLeg>(Value::Object(stt_obj.clone())).unwrap_or_else(
        |e| {
            tracing::warn!(key = %key, error = %e, "voice_agent stt leg has malformed options; keeping only its deployment");
            AgentSttLeg {
                endpoint_id: required_str(&stt_obj, "endpoint_id").unwrap_or_default(),
                ..Default::default()
            }
        },
    );
    let tts = serde_json::from_value::<AgentTtsLeg>(Value::Object(tts_obj.clone())).unwrap_or_else(
        |e| {
            tracing::warn!(key = %key, error = %e, "voice_agent tts leg has malformed options; keeping only its deployment");
            AgentTtsLeg {
                endpoint_id: required_str(&tts_obj, "endpoint_id").unwrap_or_default(),
                ..Default::default()
            }
        },
    );

    for field in obj.keys() {
        if !KNOWN_FIELDS.contains(&field.as_str()) {
            tracing::debug!(key = %key, field = %field, "voice_agent entry carries a field this build does not model; ignored");
        }
    }

    let text = |field: &str, default: &str| -> String {
        obj.get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(default)
            .to_string()
    };

    Ok(VoiceAgentEntry {
        v: obj.get("v").and_then(Value::as_u64).unwrap_or(1) as u32,
        prompt_name: obj
            .get("prompt_name")
            .and_then(Value::as_str)
            .map(str::to_string),
        project_id: obj
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        stt,
        tts,
        turn_detection: block(&obj, "turn_detection", &key),
        interruption: block(&obj, "interruption", &key),
        greeting: block(&obj, "greeting", &key),
        fillers: block(&obj, "fillers", &key),
        text_transforms: block(&obj, "text_transforms", &key),
        structured_output: block(&obj, "structured_output", &key),
        idle: block(&obj, "idle", &key),
        limits: block(&obj, "limits", &key),
        session_overrides: block(&obj, "session_overrides", &key),
        degradation_message: text("degradation_message", DEFAULT_DEGRADATION_MESSAGE),
        approval_message: text("approval_message", DEFAULT_APPROVAL_MESSAGE),
        input: block(&obj, "input", &key),
        tool_status_messages: block(&obj, "tool_status_messages", &key),
        allow_preemptive: obj
            .get("allow_preemptive")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        prompt_id,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bytes budapp's publisher writes (`services/budapp/tests/fixtures/voice_agent_entry.json`
    /// — the same file, byte-identical). budapp's test builds it; this one parses it.
    const FIXTURE: &str = include_str!("../tests/fixtures/voice_agent_entry.json");

    #[test]
    fn parses_the_shared_fixture() {
        let e = parse_voice_agent_blob(FIXTURE).expect("fixture parses");
        assert_eq!(e.prompt_id, "c0de0000-0000-4000-8000-000000000003");
        assert_eq!(e.version, 3);
        assert_eq!(e.prompt_name.as_deref(), Some("support"));
        assert_eq!(
            e.project_id.as_deref(),
            Some("9e0d0000-0000-4000-8000-000000000004")
        );
        assert_eq!(e.stt.endpoint_id, "6f1c0000-0000-4000-8000-000000000001");
        assert_eq!(e.stt.language.as_deref(), Some("en"));
        assert_eq!(e.stt.keyterms, vec!["Bud".to_string()]);
        assert_eq!(e.tts.endpoint_id, "9a020000-0000-4000-8000-000000000002");
        assert_eq!(e.tts.voice.as_deref(), Some("alloy"));
        assert_eq!(e.tts.speed, Some(1.0));
        assert_eq!(e.turn_detection.kind, "semantic");
        assert_eq!(e.greeting.static_text(), Some("Hi, how can I help?"));
        assert_eq!(e.input.required_variables, vec!["customer_id".to_string()]);
        assert!(!e.allow_preemptive, "the fixture agent has tools");
        assert_eq!(
            e.tool_phrase("lookup_order"),
            Some("Let me look that order up.")
        );
        assert!(e.allows_override("tts.voice"));
        assert!(!e.allows_override("stt.language"));
        assert_eq!(e.fillers.follow_up_after_ms, 3000);
        assert_eq!(
            e.fillers.messages,
            ["Hmm...", "One moment.", "Still checking..."],
            "budapp's default filler script"
        );
        assert_eq!(e.fillers.messages, AgentFillers::default().messages);
        assert!(
            e.fillers.tool_call_sound,
            "budapp publishes the tone on by default"
        );
        assert_eq!(e.key(), "c0de0000-0000-4000-8000-000000000003:v3");
    }

    #[test]
    fn the_tool_call_tone_is_on_unless_the_agent_turns_it_off() {
        let base =
            r#"{"prompt_id":"p","version":1,"stt":{"endpoint_id":"s"},"tts":{"endpoint_id":"t"}"#;
        let older = parse_voice_agent_blob(&format!(
            "{base},\"fillers\":{{\"tool_call_after_ms\":900}}}}"
        ))
        .expect("an entry published before the setting");
        assert!(older.fillers.tool_call_sound);
        assert_eq!(older.fillers.tool_call_after_ms, 900);
        let off = parse_voice_agent_blob(&format!(
            "{base},\"fillers\":{{\"tool_call_sound\":false}}}}"
        ))
        .expect("parses");
        assert!(!off.fillers.tool_call_sound);
    }

    #[test]
    fn a_malformed_block_falls_back_to_its_defaults_not_the_whole_entry() {
        let raw = r#"{"prompt_id":"p","version":1,"stt":{"endpoint_id":"s"},"tts":{"endpoint_id":"t"},
                      "turn_detection":"not-an-object","interruption":{"min_words":"many"},"future":1}"#;
        let e = parse_voice_agent_blob(raw).expect("entry survives");
        assert_eq!(e.turn_detection, AgentTurnDetection::default());
        assert_eq!(e.interruption, AgentInterruption::default());
        assert_eq!(e.degradation_message, DEFAULT_DEGRADATION_MESSAGE);
    }

    #[test]
    fn a_malformed_leg_option_keeps_the_leg() {
        let raw = r#"{"prompt_id":"p","version":1,"stt":{"endpoint_id":"s","keyterms":"x"},"tts":{"endpoint_id":"t","speed":"fast"}}"#;
        let e = parse_voice_agent_blob(raw).expect("entry survives");
        assert_eq!(e.stt.endpoint_id, "s");
        assert!(e.stt.keyterms.is_empty());
        assert_eq!(e.tts.endpoint_id, "t");
        assert_eq!(e.tts.speed, None);
    }

    #[test]
    fn what_a_session_cannot_run_without_is_required() {
        for raw in [
            r#"{"version":1,"stt":{"endpoint_id":"s"},"tts":{"endpoint_id":"t"}}"#,
            r#"{"prompt_id":"p","stt":{"endpoint_id":"s"},"tts":{"endpoint_id":"t"}}"#,
            r#"{"prompt_id":"p","version":0,"stt":{"endpoint_id":"s"},"tts":{"endpoint_id":"t"}}"#,
            r#"{"prompt_id":"p","version":1,"tts":{"endpoint_id":"t"}}"#,
            r#"{"prompt_id":"p","version":1,"stt":{"endpoint_id":""},"tts":{"endpoint_id":"t"}}"#,
            r#"{"prompt_id":"p","version":1,"stt":{"endpoint_id":"s"},"tts":"t"}"#,
            r#"[]"#,
            r#"not json"#,
        ] {
            assert!(
                parse_voice_agent_blob(raw).is_err(),
                "should be refused: {raw}"
            );
        }
    }

    #[test]
    fn a_string_version_is_accepted() {
        let raw = r#"{"prompt_id":"p","version":"2","stt":{"endpoint_id":"s"},"tts":{"endpoint_id":"t"}}"#;
        assert_eq!(parse_voice_agent_blob(raw).unwrap().version, 2);
    }

    #[test]
    fn a_greeting_with_no_text_is_no_greeting() {
        let g = AgentGreeting {
            mode: "static".into(),
            text: Some("   ".into()),
            interruptible: false,
        };
        assert_eq!(g.static_text(), None);
        assert_eq!(AgentGreeting::default().static_text(), None);
    }
}
