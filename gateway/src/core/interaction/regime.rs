//! Which interaction regime a session runs, and with what owners, confirmations and truth: a
//! port of `docs/duplex-agent/providers/regime_resolve.py` (its `Resolver`) over the same
//! capability data, `assets/provider_capabilities.json` (a byte copy of the plan's file).
//!
//! A session description and the result are JSON objects with the Python's keys, so the
//! plan's self-test cases port as they are written (the tests below). The regimes are
//! `Cascade` (WaaV owns the voice; `variant` `full` or `half_cascade`), `ProviderSpeech` (B),
//! `Delegating` (C) and `NativeDuplex` (D).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value, json};

/// Wire names of the four verbs this build sends, in the order of every `verbs` array here.
pub const VERB_NAMES: [&str; 4] = ["say_exact", "say_guided", "add_context", "stop_output"];

/// The release this build is: the surfaces and providers of later releases are refused.
pub const RELEASE: u64 = 3;

/// What this build can send each provider, per verb of [`VERB_NAMES`]: the protocols with a
/// `verb` wire. Every other provider has none, whatever the vendor offers.
const BUILD_VERBS: &[(&str, [bool; 4])] = &[
    ("openai", [false, true, true, true]),
    ("deepgram", [true, true, false, false]),
    ("nova_sonic", [false, true, false, false]),
    // Azure sends OpenAI's verbs; Gemini adds context with `clientContent` `turnComplete: false`.
    ("azure", [false, true, true, true]),
    ("gemini", [false, false, true, false]),
    // Ultravox: `forced_agent_message`, `user_text_message` urgency `immediate` / `later`.
    ("ultravox", [true, true, true, false]),
    // ElevenLabs Agents: a `user_message` for a guided line, `contextual_update`.
    ("elevenlabs", [false, true, true, false]),
    // Inworld: `response.speak`, then OpenAI's; Yandex sends OpenAI's.
    ("inworld", [true, true, true, true]),
    ("yandex", [false, true, true, true]),
    // Grok: xAI's `force_message`; a guided line is xAI's `response.create` with `instructions`.
    ("grok", [true, true, true, true]),
];

/// The verbs this build can send the provider (its regime id, see [`provider_id`]).
pub fn verbs(provider: &str) -> [bool; 4] {
    BUILD_VERBS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(provider))
        .map_or([false; 4], |(_, verbs)| *verbs)
}

/// The capability data's provider id of a `/realtime` provider name or alias: the gateway's
/// own realtime name (`plugin::dispatch`), and the two the data knows that the gateway does
/// not dispatch.
pub fn provider_id(name: &str) -> &'static str {
    if let Some(builtin) = crate::plugin::dispatch::resolve_realtime_provider(name) {
        return builtin.canonical_name();
    }
    match name.to_ascii_lowercase().as_str() {
        "infer" | "waav-infer" | "waav_infer" => "infer",
        "gpt_live" | "gpt-live" => "gpt_live",
        _ => "unknown",
    }
}

/// Today's date, as the lifecycle rules compare it.
pub fn today() -> String {
    let date = time::OffsetDateTime::now_utc().date();
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

const LICENCES: [&str; 2] = ["lfm_open_eligible", "liquid_commercial"];
const LFM_PROFILES: [&str; 2] = ["lfm-standard", "lfm-large"];
const APACHE_PROFILES: [&str; 1] = ["apache-standard"];
const OWNER_KINDS: [&str; 3] = ["filler", "backchannel", "idle"];
const OWNER_VALUES: [&str; 4] = ["auto", "gateway", "provider", "none"];
const NARROWED_ACTS: [&str; 3] = ["wait", "progress", "refuse"];
const ACTS: [&str; 10] = [
    "wait",
    "backchannel",
    "delegate",
    "update",
    "abort",
    "social",
    "echo",
    "progress",
    "question",
    "refuse",
];

fn data() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("assets/provider_capabilities.json"))
            .expect("the packaged capability data is JSON")
    })
}

/// A regime the session cannot have. `fatal`: the plan refuses the session itself.
#[derive(Debug, Clone, PartialEq)]
struct Refusal {
    code: String,
    fatal: bool,
    detail: Value,
    warnings: Vec<String>,
}

impl Refusal {
    fn new(code: &str, fatal: bool, detail: Value) -> Self {
        Self {
            code: code.to_owned(),
            fatal,
            detail,
            warnings: Vec::new(),
        }
    }
}

fn get<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted
        .split('.')
        .try_fold(value, |node, part| node.as_object()?.get(part))
}

fn text<'a>(value: &'a Value, dotted: &str) -> Option<&'a str> {
    get(value, dotted).and_then(Value::as_str)
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
    }
}

/// Python's `re.search`.
fn search(pattern: &str, text: &str) -> bool {
    Regex::new(pattern).is_ok_and(|re| re.is_match(text))
}

fn surface(id: &str) -> Option<&'static Value> {
    data()["surfaces"]
        .as_array()?
        .iter()
        .find(|s| s["id"] == id)
}

fn cascade() -> &'static Value {
    data()["providers"]
        .as_array()
        .and_then(|all| all.iter().find(|p| p["id"] == "cascade"))
        .expect("the capability data has the cascade")
}

/// The provider entry for a model, and the model (the entry's default when none is named).
fn lookup(provider: &str, model: Option<&str>) -> Option<(&'static Value, String)> {
    let entries: Vec<&Value> = data()["providers"]
        .as_array()?
        .iter()
        .filter(|p| p["provider_id"] == provider)
        .collect();
    let model = match model.filter(|m| !m.is_empty()) {
        Some(model) => model.to_owned(),
        None => entries
            .iter()
            .find_map(|e| e["default_model"].as_str().filter(|m| !m.is_empty()))
            .unwrap_or_default()
            .to_owned(),
    };
    let entry = entries.into_iter().find(|e| {
        e["model_patterns"]
            .as_array()
            .is_some_and(|ps| ps.iter().any(|p| search(p.as_str().unwrap_or(""), &model)))
    })?;
    Some((entry, model))
}

/// The lifecycle of a model at `today` (ISO dates compare as strings).
fn lifecycle_at(entry: &Value, model: &str, today: &str) -> Value {
    let rules = entry["lifecycle"]["rules"].as_array();
    for rule in rules.into_iter().flatten() {
        if !search(rule["models"].as_str().unwrap_or(""), model) {
            continue;
        }
        let shutdown = rule["shutdown"].as_str().unwrap_or("");
        if today >= shutdown {
            return json!({"status": "retired", "code": rule["refusal_code"],
                          "shutdown": shutdown, "replacement": rule["replacement"]});
        }
        let from = rule["warning_from"].as_str();
        if from.is_none_or(|from| today >= from) {
            return json!({"status": "retiring", "code": rule["warning_code"],
                          "shutdown": shutdown, "replacement": rule["replacement"]});
        }
    }
    json!({"status": "active", "code": null, "shutdown": null, "replacement": null})
}

/// How long a delegate call of the provider's may stay provisional before it is adopted
/// (capability data `background.provisional_runs.adopt_by_ms`): `None` when the provider's
/// calls are never provisional (chapter 6, "provider speculation").
pub fn adopt_by(provider: &str, model: Option<&str>) -> Option<u64> {
    let (entry, _) = lookup(provider, model)?;
    entry["background"]["provisional_runs"]["adopt_by_ms"].as_u64()
}

/// The date `select_regime` is given when the session's lifecycle was applied before.
const SKIP_LIFECYCLE: &str = "";

/// A provider's model (`None`: its default) when it is retiring or retired at `today`:
/// `{status, code, shutdown, replacement, model}`.
pub fn lifecycle(provider: &str, model: Option<&str>, today: &str) -> Option<Value> {
    let (entry, model) = lookup(provider, model)?;
    let mut found = lifecycle_at(entry, &model, today);
    if found["status"] == "active" {
        return None;
    }
    found["model"] = model.into();
    Some(found)
}

/// Whether the surface reaches the provider: `(reached, refusal, fatal)`.
fn reach(surf: &Value, caps: &Value) -> (bool, Option<String>, bool) {
    let pid = caps["provider_id"].as_str().unwrap_or("");
    if let Some(pr) = surf.get("provider_refusals").and_then(|r| r.get(pid)) {
        return (
            false,
            pr["code"].as_str().map(str::to_owned),
            pr["fatal"].as_bool().unwrap_or(false),
        );
    }
    match surf["kind"].as_str() {
        Some("refused") => return (true, None, false),
        Some("cascade") => return (pid == "cascade", None, false),
        _ => {}
    }
    if pid == "cascade" {
        return (false, Some("unsupported_vendor".to_owned()), true);
    }
    if let Some(own) = text(
        caps,
        &format!("surface_refusals.{}", surf["id"].as_str().unwrap_or("")),
    ) {
        return (false, Some(own.to_owned()), true);
    }
    let reaches = &surf["reaches"];
    if reaches == "all"
        || reaches
            .as_array()
            .is_some_and(|r| r.iter().any(|p| p == pid))
    {
        return (true, None, false);
    }
    match surf.get("unreached_refusal") {
        Some(un) => (
            false,
            un["code"].as_str().map(str::to_owned),
            un["fatal"].as_bool().unwrap_or(true),
        ),
        None => (false, Some("unsupported_vendor".to_owned()), true),
    }
}

fn absent(verb: &Value) -> bool {
    matches!(verb["support"].as_str(), Some("absent" | "retired"))
}

fn native(verb: &Value) -> bool {
    verb["support"] == "native"
}

fn exclusive_delegating(caps: &Value) -> bool {
    let (v, bg, own) = (&caps["verbs"], &caps["background"], &caps["ownership"]);
    if bg["delegation"] == "native_client" {
        return true;
    }
    if caps["native_mechanisms"]["filler"]["kind"] == "always_on"
        && own["turn_owner"] == "provider"
        && absent(&v["say_exact"])
    {
        return true; // Gemini 3.8 Live
    }
    absent(&v["say_exact"]) && bg["async_tools"] == "always_async" // Nova 2 Sonic
}

fn native_duplex_ready(release: u64, flags: &Value) -> bool {
    truthy(flags.get("infer_injection_channel"))
        && truthy(flags.get("native_duplex_criteria"))
        && release >= 5
}

fn speaks(caps: &Value) -> bool {
    let v = &caps["verbs"];
    native(&v["say_exact"]) || native(&v["say_guided"]) || native(&v["add_context"])
}

fn auto_regime(caps: &Value, release: u64, flags: &Value) -> Result<&'static str, Refusal> {
    if caps["kind"] == "native_duplex" {
        if native_duplex_ready(release, flags) {
            return Ok("NativeDuplex");
        }
        let reason = "native duplex model has no injection channel";
        return Err(Refusal::new(
            "interaction_unsupported",
            false,
            json!({"reason": reason}),
        ));
    }
    if exclusive_delegating(caps) {
        return Ok("Delegating");
    }
    if speaks(caps) {
        return Ok("ProviderSpeech");
    }
    if caps["ownership"]["turn_owner"] == "provider"
        && truthy(caps["background"].get("tools_declarable"))
    {
        return Ok("Delegating"); // Gemini 3.1 Flash Live
    }
    let reason = "no regime fits the descriptor";
    Err(Refusal::new(
        "interaction_unsupported",
        false,
        json!({"reason": reason}),
    ))
}

/// The regimes a forced `interaction.regime` may narrow to.
fn possible_regimes(caps: &Value, release: u64, flags: &Value) -> Vec<&'static str> {
    let mut out = Vec::new();
    if truthy(caps["ownership"].get("half_cascade_possible")) {
        out.push("Cascade");
    }
    if caps["kind"] == "native_duplex" {
        if native_duplex_ready(release, flags) {
            out.push("NativeDuplex");
        }
        return out;
    }
    if exclusive_delegating(caps) {
        out.push("Delegating");
        return out;
    }
    if speaks(caps) {
        out.push("ProviderSpeech");
    }
    if caps["ownership"]["turn_owner"] == "provider"
        && truthy(caps["background"].get("tools_declarable"))
    {
        out.push("Delegating");
    }
    out
}

fn forced_name(forced: &str) -> Option<&'static str> {
    match forced {
        "cascade" => Some("Cascade"),
        "provider_speech" => Some("ProviderSpeech"),
        "delegating" => Some("Delegating"),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn select_regime(
    surf: &Value,
    caps: &Value,
    model: &str,
    cfg: &Value,
    server: &Value,
    today: &str,
    release: u64,
    flags: &Value,
) -> Result<Map<String, Value>, Refusal> {
    let mut warnings: Vec<String> = Vec::new();
    let lc = lifecycle_at(caps, model, today);
    if lc["status"] == "retired" && today != SKIP_LIFECYCLE {
        let detail = json!({"shutdown": lc["shutdown"], "replacement": lc["replacement"]});
        return Err(Refusal::new(
            lc["code"].as_str().unwrap_or(""),
            true,
            detail,
        ));
    }
    let forced = text(cfg, "regime").unwrap_or("auto");
    if surf["kind"] == "cascade" {
        if !matches!(forced, "auto" | "cascade") {
            warnings.push("regime_downgraded".to_owned());
        }
        return Ok(decide(
            surf,
            cascade(),
            model,
            "Cascade",
            Some("full"),
            cfg,
            warnings,
            flags,
        ));
    }
    let (ok, code, fatal) = reach(surf, caps);
    if !ok {
        let detail = json!({"surface": surf["id"], "provider": caps["provider_id"]});
        return Err(Refusal::new(&code.unwrap_or_default(), fatal, detail));
    }
    let voice_owner = text(cfg, "speech.voice_owner");
    let truth = text(cfg, "speech.truth");
    if voice_owner == Some("gateway") || truth == Some("enforced") {
        if truthy(caps["ownership"].get("half_cascade_possible")) {
            let variant = Some("half_cascade");
            return Ok(decide(
                surf, caps, model, "Cascade", variant, cfg, warnings, flags,
            ));
        }
        if truth == Some("enforced") {
            let detail = json!({"provider": caps["provider_id"], "model": model});
            return Err(Refusal::new("truth_enforcement_unavailable", true, detail));
        }
        warnings.push("voice_owner_unavailable".to_owned());
    }
    let mut regime = match auto_regime(caps, release, flags) {
        Ok(regime) => regime,
        Err(mut refusal) => {
            warnings.append(&mut refusal.warnings);
            refusal.warnings = warnings;
            return Err(refusal);
        }
    };
    let mut variant = None;
    if forced != "auto" {
        let want = forced_name(forced).unwrap_or("");
        if possible_regimes(caps, release, flags).contains(&want) {
            regime = want;
            variant = (want == "Cascade").then_some("half_cascade");
        } else {
            warnings.push("regime_downgraded".to_owned());
        }
    }
    let d = decide(surf, caps, model, regime, variant, cfg, warnings, flags);
    require_truth_ack(d, caps, model, &server["truth_ack"])
}

/// Monitored regimes run only for providers the operator lists, as `provider` or
/// `provider:model`.
fn require_truth_ack(
    d: Map<String, Value>,
    caps: &Value,
    model: &str,
    ack: &Value,
) -> Result<Map<String, Value>, Refusal> {
    if d["truth"] == "enforced" {
        return Ok(d);
    }
    let pid = caps["provider_id"].as_str().unwrap_or("");
    let listed = ack.as_array().is_some_and(|ack| {
        ack.iter()
            .filter_map(Value::as_str)
            .any(|e| e == pid || e == format!("{pid}:{model}"))
    });
    if listed {
        return Ok(d);
    }
    let alt =
        truthy(caps["ownership"].get("half_cascade_possible")).then_some("speech.truth: enforced");
    let detail = json!({"provider": pid, "model": model, "path": "interaction.speech.truth_ack",
                        "alternative": alt});
    let mut refusal = Refusal::new("truth_ack_required", false, detail);
    refusal.warnings = string_list(&d["warnings"]);
    Err(refusal)
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn auto_value(caps: &Value, kind: &str, model: &str, flags: &Value) -> String {
    let auto = &caps["owners"][kind]["auto"];
    if let Some(pattern) = auto.get("if_model_family_matches").and_then(Value::as_str) {
        let picked = match flags
            .get("model_family")
            .and_then(Value::as_str)
            .filter(|f| !f.is_empty())
        {
            None => &auto["unknown"],
            Some(family) if search(pattern, family) => &auto["then"],
            Some(_) => &auto["else"],
        };
        return picked.as_str().unwrap_or("").to_owned();
    }
    if let Some(pattern) = auto.get("if_model_matches").and_then(Value::as_str) {
        let picked = if search(pattern, model) {
            &auto["then"]
        } else {
            &auto["else"]
        };
        return picked.as_str().unwrap_or("").to_owned();
    }
    auto.as_str().unwrap_or("").to_owned()
}

fn family_unknown(caps: &Value, kind: &str, flags: &Value) -> bool {
    let auto = &caps["owners"][kind]["auto"];
    auto.get("if_model_family_matches").is_some() && !truthy(flags.get("model_family"))
}

fn half_cascade_owner(kind: &str, requested: &str) -> &'static str {
    match (kind, requested) {
        ("backchannel", _) | (_, "none") => "none",
        _ => "gateway",
    }
}

/// `(result, warning, how)`.
#[allow(clippy::too_many_arguments)]
fn resolve_owner(
    caps: &Value,
    kind: &str,
    requested: &str,
    regime: &str,
    variant: Option<&str>,
    model: &str,
    flags: &Value,
) -> (String, Option<String>, String) {
    let (res, how) = if regime == "Cascade" && variant == Some("half_cascade") {
        let how = "half-cascade: WaaV text-to-speech is the voice (Addendum A2 item 8)";
        (
            half_cascade_owner(kind, requested).to_owned(),
            how.to_owned(),
        )
    } else if regime == "NativeDuplex" {
        (
            "provider".to_owned(),
            "intrinsic to the native duplex model".to_owned(),
        )
    } else {
        let table = if regime == "Cascade" { cascade() } else { caps };
        let want = if requested == "auto" {
            let default_regime = if std::ptr::eq(table, caps) {
                auto_regime(caps, 5, flags).ok()
            } else {
                None
            };
            if regime == "Delegating" && default_regime != Some("Delegating") {
                "provider".to_owned()
            } else {
                auto_value(table, kind, model, flags)
            }
        } else {
            requested.to_owned()
        };
        let out = &table["owners"][kind][want.as_str()];
        let mut res = out["result"].as_str().unwrap_or("").to_owned();
        let how = out["how"].as_str().unwrap_or("").to_owned();
        if let Some(flag) = out.get("unless_operator_flag").and_then(Value::as_str)
            && flags
                .get("operator_flags")
                .and_then(Value::as_array)
                .is_some_and(|f| f.iter().any(|f| f == flag))
        {
            res = out["then"].as_str().unwrap_or("").to_owned();
        }
        (res, how)
    };
    let warn =
        (requested != "auto" && res != requested).then(|| format!("{kind}_owner_overridden"));
    (res, warn, how)
}

/// The speech path of a confirmation line.
fn confirmation_path(
    caps: &Value,
    regime: &str,
    irreversible: bool,
    client_ui: bool,
    voice_mismatch: &str,
) -> &'static str {
    if regime == "Cascade" {
        return "exact_waav_tts";
    }
    if native(&caps["verbs"]["say_exact"]) {
        return "native_exact";
    }
    let reproducible = &caps["ownership"]["voice_reproducible"]["value"];
    if reproducible == "same" || reproducible == "same_when_voice_known" {
        return "gateway_clip_same_voice";
    }
    if irreversible {
        if client_ui {
            return "client_ui";
        }
        if matches!(voice_mismatch, "confirmations_only" | "always") {
            return "gateway_clip_different_voice";
        }
        return "unanswerable_no_verbatim_channel";
    }
    if matches!(
        caps["verbs"]["say_guided"]["support"].as_str(),
        Some("native" | "emulated")
    ) {
        return "say_guided_exact_words_checked";
    }
    if client_ui {
        "client_ui"
    } else {
        "unanswerable_no_verbatim_channel"
    }
}

#[allow(clippy::too_many_arguments)]
fn decide(
    surf: &Value,
    caps: &Value,
    model: &str,
    regime: &str,
    variant: Option<&str>,
    cfg: &Value,
    mut warnings: Vec<String>,
    flags: &Value,
) -> Map<String, Value> {
    let mut owners = Map::new();
    let mut how = Map::new();
    for kind in OWNER_KINDS {
        let key = format!("{kind}_owner");
        let requested = text(cfg, &key).unwrap_or("auto");
        let (res, warn, h) = resolve_owner(caps, kind, requested, regime, variant, model, flags);
        owners.insert(key.clone(), res.into());
        how.insert(key, h.into());
        if let Some(warn) = warn {
            warnings.push(warn);
        }
        let unknown = "model_family_unknown";
        if regime == "ProviderSpeech"
            && requested == "auto"
            && family_unknown(caps, kind, flags)
            && !warnings.iter().any(|w| w == unknown)
        {
            warnings.push(unknown.to_owned());
        }
    }
    let front = match regime {
        "Cascade" => "local",
        "ProviderSpeech" if text(cfg, "speech.narrator") == Some("front_model") => {
            "narrator_front_model"
        }
        "ProviderSpeech" => "narrator_templates",
        _ => "provider",
    };
    let masked: Vec<&str> = if front == "narrator_front_model" {
        ACTS.into_iter()
            .filter(|a| !NARROWED_ACTS.contains(a))
            .collect()
    } else if front == "local" && owners["backchannel_owner"] != "gateway" {
        vec!["backchannel"]
    } else {
        Vec::new()
    };
    let truth = if regime == "Cascade" {
        "enforced"
    } else {
        "monitored"
    };
    let voice_mismatch = text(cfg, "speech.voice_mismatch").unwrap_or("confirmations_only");
    let client_ui = truthy(Some(
        flags
            .get("client_ui")
            .or_else(|| surf.get("client_ui"))
            .unwrap_or(&Value::Bool(true)),
    ));
    let relay = if regime == "Cascade" {
        json!("verbatim_streamed")
    } else {
        caps["relay"]["mode"].clone()
    };
    let filler = owners["filler_owner"].as_str().unwrap_or("");
    let ack = match regime {
        "Cascade" => {
            "E + ack_due_ms (300 ms; 700 ms in fast mode), skipped when the first answer sentence is ready; hard limit E + 2,000 ms"
        }
        "ProviderSpeech" if filler == "gateway" => {
            "later of E + 300 ms and the provider's delegate call; skipped when an answer sentence is ready or the provider spoke after E; hard limit E + 2,000 ms with a call open"
        }
        "ProviderSpeech" if filler == "provider" => {
            "none from the gateway: the provider's own preambles or soft timeout; gateway acknowledgement acts masked"
        }
        "ProviderSpeech" => "none (filler_owner none)",
        _ => {
            "none from the gateway: the provider is the front (W7 reports provider_first_audio_after_e_ms)"
        }
    };
    let variant = if regime == "Cascade" {
        json!(variant.unwrap_or("full"))
    } else {
        json!(variant)
    };
    let provisional = if regime == "Cascade" {
        true
    } else {
        !caps["background"]["provisional_runs"].is_null()
    };
    let mut d = Map::new();
    d.insert("regime".into(), regime.into());
    d.insert("variant".into(), variant);
    d.insert("front".into(), front.into());
    d.insert("masked_acts".into(), json!(masked));
    d.extend(owners);
    d.insert("owner_how".into(), Value::Object(how));
    d.insert(
        "voice_owner".into(),
        (if regime == "Cascade" {
            "gateway"
        } else {
            "provider"
        })
        .into(),
    );
    d.insert("truth".into(), truth.into());
    d.insert("relay".into(), relay);
    let path =
        |irreversible| confirmation_path(caps, regime, irreversible, client_ui, voice_mismatch);
    d.insert("confirm_irreversible".into(), path(true).into());
    d.insert("confirm_reversible".into(), path(false).into());
    d.insert("ack".into(), ack.into());
    d.insert(
        "egress".into(),
        surf.get("egress").cloned().unwrap_or(Value::Null),
    );
    d.insert("provisional_runs".into(), provisional.into());
    d.insert("warnings".into(), json!(warnings));
    d
}

/// The front profile (decisions section 2.1 with Addenda B1 and B7): `(profile, warnings,
/// qualified)`.
fn resolve_front_profile(server: &Value, cfg: &Value) -> (String, Vec<String>, bool) {
    let mut warnings = Vec::new();
    let fm = server
        .get("front_model")
        .filter(|f| f.is_object())
        .cloned()
        .unwrap_or(json!({}));
    let lic = fm.get("licence").and_then(Value::as_str);
    let licensed = lic.is_some_and(|l| LICENCES.contains(&l));
    let qualified = string_list(server.get("qualified_profiles").unwrap_or(&Value::Null));
    let is_qualified = |p: &str| qualified.iter().any(|q| q == p);
    let usable = |p: &str, need_qualified: bool| {
        if LFM_PROFILES.contains(&p) && !licensed {
            Some("not_licensed")
        } else if need_qualified && !is_qualified(p) {
            Some("not_qualified")
        } else {
            None
        }
    };
    let allow_unqualified = truthy(fm.get("allow_unqualified"));
    let auto = || {
        if licensed && let Some(p) = LFM_PROFILES.into_iter().find(|p| is_qualified(p)) {
            return (p.to_owned(), true);
        }
        if let Some(p) = APACHE_PROFILES.into_iter().find(|p| is_qualified(p)) {
            return (p.to_owned(), true);
        }
        ("scripted".to_owned(), false)
    };
    let (mut resolved, mut qualified_now) = auto();
    let operator = fm
        .get("profile")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .unwrap_or("auto");
    if operator != "auto" && usable(operator, !allow_unqualified).is_none() {
        resolved = operator.to_owned();
        qualified_now = is_qualified(operator);
    }
    let requested = text(cfg, "front.profile")
        .filter(|p| !p.is_empty())
        .unwrap_or("auto");
    if requested != "auto" {
        let enabled = fm
            .get("profiles_enabled")
            .map(string_list)
            .unwrap_or_else(|| vec!["auto".to_owned()]);
        let reason = if !enabled.iter().any(|e| e == requested) {
            Some("not_enabled")
        } else if requested == "scripted" {
            None
        } else {
            usable(requested, !allow_unqualified)
        };
        if reason.is_some() {
            warnings.push("front_profile_substituted".to_owned());
        } else {
            resolved = requested.to_owned();
            qualified_now = is_qualified(requested);
        }
    }
    if resolved == "scripted" {
        warnings.push("front_model_scripted".to_owned());
    }
    (resolved, warnings, qualified_now)
}

/// A small part of the lenient parse: server-only keys and bad values of the regime keys.
/// `(block, warning codes)`; `None` when the session asks for nothing.
fn parse_block(block: &Value, from_client: bool) -> (Option<Value>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut cfg = match block {
        Value::Null | Value::Bool(false) => return (None, warnings),
        Value::Bool(true) => return (Some(json!({})), warnings),
        other => other.clone(),
    };
    fn strip_licence(node: &mut Value, warnings: &mut Vec<String>) {
        if let Some(map) = node.as_object_mut() {
            if map.remove("licence").is_some() {
                warnings.push("interaction_override_refused".to_owned());
            }
            for value in map.values_mut() {
                strip_licence(value, warnings);
            }
        }
    }
    strip_licence(&mut cfg, &mut warnings);
    let mut server_only = vec![
        "defaults",
        "allow_client_override",
        "realtime_legacy_wire",
        "speech.truth_ack",
        "duplex.compute",
        "tasks.task_backends",
        "tasks.allow_client_backends",
        "tasks.allow_client_tools",
        "tasks.allow_client_push",
        "tasks.phone_continuation",
        "tasks.speech_policy",
    ];
    if from_client {
        server_only.push("speech.fallback");
    }
    for path in server_only {
        if let Some((node, leaf)) = parent_mut(&mut cfg, path)
            && node.remove(leaf).is_some()
        {
            warnings.push("interaction_override_refused".to_owned());
        }
    }
    let enums: [(&str, &[&str]); 8] = [
        (
            "regime",
            &["auto", "cascade", "provider_speech", "delegating"],
        ),
        ("filler_owner", &OWNER_VALUES),
        ("backchannel_owner", &OWNER_VALUES),
        ("idle_owner", &OWNER_VALUES),
        ("speech.voice_owner", &["auto", "provider", "gateway"]),
        ("speech.truth", &["monitored", "enforced"]),
        ("speech.narrator", &["templates", "front_model"]),
        (
            "speech.voice_mismatch",
            &["never", "confirmations_only", "always"],
        ),
    ];
    for (path, allowed) in enums {
        if let Some((node, leaf)) = parent_mut(&mut cfg, path) {
            let bad = node
                .get(leaf)
                .is_some_and(|v| !v.as_str().is_some_and(|v| allowed.contains(&v)));
            if bad {
                node.remove(leaf);
                warnings.push("interaction_value_ignored".to_owned());
            }
        }
    }
    (Some(cfg), warnings)
}

/// The object holding a dotted path's last key, and that key.
fn parent_mut<'a, 'p>(
    cfg: &'a mut Value,
    path: &'p str,
) -> Option<(&'a mut Map<String, Value>, &'p str)> {
    let (parent, leaf) = path.rsplit_once('.').unwrap_or(("", path));
    let mut node = cfg;
    for part in parent.split('.').filter(|p| !p.is_empty()) {
        node = node.as_object_mut()?.get_mut(part)?;
    }
    Some((node.as_object_mut()?, leaf))
}

fn mid_call_language(surf: &Value) -> &'static str {
    match surf["mid_call_language"].as_str() {
        Some("step_down") => "steps down to the plain conversation path",
        Some("as_controlling_ws_session") => "as the /ws session that drives it",
        _ => "repeat-request template, continues in English",
    }
}

/// Resolve one session description. Keys: `surface`, `provider`, `model`, `model_family`,
/// `release`, `today`, `bud_mode`, `interaction` (the block), `server` (`enabled`,
/// `build_features`, `truth_ack`, `front_model`, `qualified_profiles`), `has_background`,
/// `language`, `echo_gates_pass`, `client_ui`, `operator_flags`, `infer_injection_channel`,
/// `native_duplex_criteria`, `from_client`.
///
/// The result has `interaction` (`on` or `off`), `refusal` (the code that turned it off),
/// `fatal`, `detail` and `warnings`, and when on the regime decision.
pub fn resolve_session(s: &Value) -> Value {
    let mut result = Map::new();
    let done = |mut result: Map<String, Value>, code: Option<&str>, fatal: bool, detail: Value| {
        result.insert("refusal".into(), json!(code));
        result.insert("fatal".into(), fatal.into());
        result.insert("detail".into(), detail);
        if code.is_none() && !fatal {
            result.insert("interaction".into(), "on".into());
        }
        Value::Object(result)
    };
    let Some(surf) = s["surface"].as_str().and_then(surface) else {
        return done(result, Some("unknown_surface"), true, json!({}));
    };
    let release = s["release"].as_u64().unwrap_or(5);
    let today = s["today"].as_str().unwrap_or("2026-10-05");
    let server = s.get("server").cloned().unwrap_or(json!({}));
    let mut flags = Map::new();
    for key in [
        "operator_flags",
        "infer_injection_channel",
        "native_duplex_criteria",
        "client_ui",
        "model_family",
    ] {
        if let Some(v) = s.get(key) {
            flags.insert(key.into(), v.clone());
        }
    }
    let flags = Value::Object(flags);
    let provider = if surf["kind"] == "cascade" {
        "cascade"
    } else {
        s["provider"].as_str().unwrap_or("cascade")
    };
    result.insert("surface".into(), surf["id"].clone());
    result.insert("release".into(), release.into());
    result.insert("interaction".into(), "off".into());
    result.insert("warnings".into(), json!([]));
    let warn = |result: &mut Map<String, Value>, code: &str| {
        if let Some(list) = result.get_mut("warnings").and_then(Value::as_array_mut) {
            list.push(code.into());
        }
    };
    let Some((caps, model)) = lookup(provider, s["model"].as_str()) else {
        let error = format!("unknown provider {provider:?}");
        return done(
            result,
            Some("unknown_provider"),
            true,
            json!({"error": error}),
        );
    };
    result.insert("provider_entry".into(), caps["id"].clone());
    result.insert("model".into(), model.clone().into());
    // `lifecycle: false`: the gateway applied the lifecycle to the session before (see
    // `lifecycle.rs`), so neither check below refuses or warns again.
    let lifecycle = s["lifecycle"] != Value::Bool(false);
    let lc = if lifecycle {
        lifecycle_at(caps, &model, today)
    } else {
        json!({"status": "active"})
    };
    if lc["status"] == "retired" {
        let detail = json!({"shutdown": lc["shutdown"], "replacement": lc["replacement"]});
        return done(result, lc["code"].as_str(), true, detail);
    }
    if lc["status"] == "retiring" {
        warn(&mut result, lc["code"].as_str().unwrap_or(""));
    }
    let available = caps["available_from_release"].as_u64().unwrap_or(0);
    if release < available {
        let reason = format!("adapter not available before Release {available}");
        return done(
            result,
            Some("unknown_provider"),
            true,
            json!({"reason": reason}),
        );
    }
    let bud = truthy(s.get("bud_mode"));
    if surf["bud_mode"] == "refused" && bud {
        return done(result, text(surf, "bud_refusal.code"), true, json!({}));
    }
    if surf["bud_mode"] == "only" && !bud {
        let code = text(surf, "non_bud_refusal.code").unwrap_or("bud_mode_required");
        return done(result, Some(code), true, json!({}));
    }
    let (ok, code, fatal) = reach(surf, caps);
    if !ok && fatal {
        let detail = json!({"surface": surf["id"], "provider": caps["provider_id"]});
        return done(result, code.as_deref(), true, detail);
    }
    let from_client = s
        .get("from_client")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let (cfg, parse_warnings) =
        parse_block(s.get("interaction").unwrap_or(&Value::Null), from_client);
    let Some(cfg) = cfg else {
        return done(result, Some("interaction_absent"), false, json!({}));
    };
    for w in &parse_warnings {
        warn(&mut result, w);
    }
    if !server
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        return done(
            result,
            Some("interaction_disabled_by_server"),
            false,
            json!({}),
        );
    }
    if !server
        .get("build_features")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        return done(
            result,
            Some("interaction_missing_build_features"),
            false,
            json!({}),
        );
    }
    if surf["kind"] == "refused" {
        let fatal = surf["refusal"]["fatal"].as_bool().unwrap_or(false);
        return done(result, text(surf, "refusal.code"), fatal, json!({}));
    }
    let mut first = surf["first_release"].as_u64();
    if truthy(surf.get("first_release_if_echo_gates_fail")) && !truthy(s.get("echo_gates_pass")) {
        first = surf["first_release_if_echo_gates_fail"].as_u64();
    }
    if first.is_none_or(|first| release < first) {
        let detail = json!({"surface": surf["id"], "release": first});
        return done(
            result,
            Some("interaction_surface_unavailable"),
            false,
            detail,
        );
    }
    let language = s["language"]
        .as_str()
        .filter(|l| !l.is_empty())
        .or_else(|| text(&cfg, "front.caller.language").filter(|l| !l.is_empty()))
        .unwrap_or("en");
    if !(language.starts_with("en") || (language.starts_with("hi") && release >= 4)) {
        let detail = json!({"language": language});
        return done(
            result,
            Some("interaction_locale_unsupported"),
            false,
            detail,
        );
    }
    if truthy(surf.get("needs_background")) && !truthy(s.get("has_background")) {
        return done(
            result,
            Some("interaction_needs_background"),
            false,
            json!({}),
        );
    }
    let today = if lifecycle { today } else { SKIP_LIFECYCLE };
    match select_regime(surf, caps, &model, &cfg, &server, today, release, &flags) {
        Err(refusal) => {
            for w in &refusal.warnings {
                warn(&mut result, w);
            }
            done(result, Some(&refusal.code), refusal.fatal, refusal.detail)
        }
        Ok(mut d) => {
            if let Some(Value::Array(ws)) = d.remove("warnings") {
                for w in ws {
                    warn(&mut result, w.as_str().unwrap_or(""));
                }
            }
            let local = d["front"] == "local";
            result.extend(d);
            if local {
                let (profile, front_warnings, qualified) = resolve_front_profile(&server, &cfg);
                result.insert("front_profile".into(), profile.into());
                result.insert("front_qualified".into(), qualified.into());
                for w in &front_warnings {
                    warn(&mut result, w);
                }
            }
            let dtmf = get(&cfg, "hitl.channels")
                .and_then(Value::as_array)
                .is_some_and(|c| c.iter().any(|c| c == "dtmf"));
            if release < 4 && dtmf {
                warn(&mut result, "interaction_value_ignored");
            }
            result.insert(
                "mid_call_unsupported_language".into(),
                mid_call_language(surf).into(),
            );
            done(result, None, false, json!({}))
        }
    }
}

/// The first release in which interaction mode reaches the provider: for non-Bud gateways,
/// for Bud deployments, and on phone and room calls. The capability data's own column, checked
/// against the resolver by the tests.
#[cfg(test)]
fn interaction_release(caps: &Value) -> Value {
    let flags = json!({"infer_injection_channel": true, "native_duplex_criteria": true});
    if caps["kind"] != "cascade" && auto_regime(caps, 5, &flags).is_err() {
        return json!({"non_bud": null, "bud": null, "phone_and_rooms": null});
    }
    let available = caps["available_from_release"].as_u64().unwrap_or(0);
    let first = |pred: &dyn Fn(&Value) -> bool| {
        let mut best: Option<u64> = None;
        for surf in data()["surfaces"].as_array().into_iter().flatten() {
            let Some(first) = surf["first_release"].as_u64() else {
                continue;
            };
            if surf["kind"] == "refused" || !pred(surf) || !reach(surf, caps).0 {
                continue;
            }
            let r = first.max(available);
            best = Some(best.map_or(r, |b| b.min(r)));
        }
        best
    };
    let mut out = json!({
        "non_bud": first(&|s| s["bud_mode"] != "only"),
        "bud": first(&|s| s["bud_mode"] != "refused"),
        "phone_and_rooms": first(&|s| matches!(s["id"].as_str(), Some("livekit" | "sip" | "ws_realtime_voice"))),
    });
    if caps["kind"] == "cascade" {
        out["phone_and_rooms_if_echo_gates_fail"] = surface("livekit").map_or(Value::Null, |s| {
            s["first_release_if_echo_gates_fail"].clone()
        });
    }
    out
}

#[cfg(test)]
#[path = "regime_tests.rs"]
mod tests;
