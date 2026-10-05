//! The resolver: reads the capability map once per session and picks a transport or a named
//! refusal.
//!
//! This is a port of `resolve()` in `docs/segmented-stt/capability-map/resolve.py`, the executable
//! specification. Every outcome, refusal code, reason and text, warning code, delivery and detail
//! key matches it; `tests/expected/` holds its answers and the tests below compare against them.
//! The one extension is [`ResolveRequest::adapter_built`].

use std::fmt;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::rollout::BUILT_RELEASE;

use crate::map::{
    CapabilityMap, ConstraintMode, EnableRequirement, GatewayClientStatus, InputMode, Lifecycle,
    LifecycleStatus, ModelString, NativeModelHandling, Percentile, Quantity, ResolvedTransport,
    Row, TodayBehaviour, Transport, VerifiedBy, py_strip,
};

/// The upload deadline when the deployment sets none (integration decisions section 2).
pub const DEFAULT_DEADLINE_MS: u32 = 6000;
/// A seed 99th percentile above this is reported as slow (decisions section 15 and A9).
pub const SLOW_TARGET_MS: u64 = 2500;
/// From this release a language or region constraint also applies to today's client (A7).
pub const NATIVE_CONSTRAINTS_FROM: u8 = 3;

const FILE_ADAPTERS: &[&str] = &[
    "openai_transcriptions",
    "groq_transcriptions",
    "azure_openai_transcriptions",
    "elevenlabs_batch",
    "assemblyai_sync",
    "deepgram_prerecorded",
    "azure_fast_transcription",
    "google_recognize",
    "speechmatics_batch",
    "regional_rest",
    "planned_file",
];
const COMMIT_ADAPTERS: &[&str] = &[
    "openai_realtime_transcription",
    "cartesia_manual_finalize",
    "planned_commit",
];
/// Encodings the segmenting engine cannot frame for its detector.
const UNDECODABLE: &[&str] = &[
    "flac",
    "opus",
    "ogg_opus",
    "webm_opus",
    "amr",
    "amr_wb",
    "mp3",
];

macro_rules! spelled {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// The reference resolver's spelling.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

spelled!(
    /// Who ends the caller's turn on this session.
    SessionKind {
        /// A voice agent with automatic turn detection: the gateway ends each turn.
        Gateway = "gateway",
        /// A voice agent in manual mode, a conversation loop or a DAG session.
        PushToTalk = "push_to_talk",
        /// A `/ws` session with no agent and no conversation loop; the client ends its own turns.
        Plain = "plain",
    }
);
spelled!(
    /// The session's `transcription_mode` preference.
    TranscriptionMode {
        Auto = "auto",
        Streaming = "streaming",
        Segmented = "segmented",
    }
);
spelled!(
    /// The deployment's latency tier (addendum B9), read from Release 4.
    LatencyTier {
        Standard = "standard",
        LowLatency = "low_latency",
    }
);
spelled!(
    /// Whether transports that need evidence first count as enabled in their release (`Assume`)
    /// or only when the map records the evidence (`Map`).
    Evidence {
        Assume = "assume",
        Map = "map",
    }
);
spelled!(
    /// What the session gets.
    Outcome {
        /// Today's client, unchanged or kept with a warning.
        Native = "native",
        /// The segmenting engine with a file-upload transcriber.
        Segmented = "segmented",
        /// A vendor socket on which the gateway's detector sends the commit.
        Commit = "commit",
        Refused = "refused",
    }
);
spelled!(
    /// Where a warning goes: a `config_warning` frame, a `ready.stt` notice, or only the log.
    Delivery {
        Frame = "frame",
        Notice = "notice",
        Log = "log",
    }
);
spelled!(
    /// The map layer that matched.
    Layer {
        Exact = "exact",
        Pattern = "pattern",
        ProviderDefault = "provider_default",
        GlobalDefault = "global_default",
        ModelUnset = "model_unset",
        DeclaredDefault = "declared_default",
        DeploymentOverride = "deployment_override",
    }
);
spelled!(
    /// What an adapter id is, from the adapter table of `CONVERSION_RULES.md`.
    AdapterKind {
        Native = "native",
        Segmented = "segmented",
        Commit = "commit",
        /// A streaming client no release builds (`planned_stream`, or an id this crate does not know).
        StreamNotBuilt = "stream_not_built",
    }
);

/// One session's inputs to [`resolve`].
#[derive(Clone, Copy)]
pub struct ResolveRequest<'a> {
    pub provider: &'a str,
    /// The model string as the session gave it; `""` when it named none.
    pub model: &'a str,
    /// The release in force, 0 to [`BUILT_RELEASE`]. A larger value is treated as that release.
    pub release: u8,
    pub session: SessionKind,
    pub mode: TranscriptionMode,
    /// The rollout switch covers the session. Ignored in Release 0, which covers nothing.
    pub covered: bool,
    pub language: Option<&'a str>,
    pub region: Option<&'a str>,
    /// A Bud deployment leg: the SDK placeholder rule does not apply.
    pub bud_leg: bool,
    /// The deployment's model, for providers whose model string is a deployment name.
    pub underlying_model: Option<&'a str>,
    /// The deployment override's profile.
    pub profile: Option<&'a str>,
    /// The session's wire audio encoding; `None` when not known, which skips the format check.
    pub encoding: Option<&'a str>,
    pub evidence: Evidence,
    /// The upload deadline in force, against which a transport's latency evidence is judged.
    pub deadline_ms: u32,
    /// The date lifecycle dates are compared with, `YYYY-MM-DD`.
    pub today: &'a str,
    pub latency_tier: LatencyTier,
    /// Which non-native adapters this build has. A transport whose adapter is not built is
    /// skipped like a transport of a later release: the session falls back to today's client or
    /// the row's refusal, and the reason stays `client_not_implemented`. `None` counts every
    /// adapter as built, as the reference resolver does.
    pub adapter_built: Option<&'a dyn Fn(&str) -> bool>,
}

impl<'a> ResolveRequest<'a> {
    /// Release 0, a voice agent with automatic turns, preference `auto`, covered from Release 1,
    /// evidence assumed, the default deadline, today's UTC date, the standard tier, every adapter
    /// built.
    pub fn new(provider: &'a str, model: &'a str) -> Self {
        ResolveRequest {
            provider,
            model,
            release: 0,
            session: SessionKind::Gateway,
            mode: TranscriptionMode::Auto,
            covered: true,
            language: None,
            region: None,
            bud_leg: false,
            underlying_model: None,
            profile: None,
            encoding: None,
            evidence: Evidence::Assume,
            deadline_ms: DEFAULT_DEADLINE_MS,
            today: utc_today(),
            latency_tier: LatencyTier::Standard,
            adapter_built: None,
        }
    }
}

impl fmt::Debug for ResolveRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolveRequest")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("release", &self.release)
            .field("session", &self.session)
            .field("mode", &self.mode)
            .field("covered", &self.covered)
            .field("language", &self.language)
            .field("region", &self.region)
            .field("bud_leg", &self.bud_leg)
            .field("underlying_model", &self.underlying_model)
            .field("profile", &self.profile)
            .field("encoding", &self.encoding)
            .field("evidence", &self.evidence)
            .field("deadline_ms", &self.deadline_ms)
            .field("today", &self.today)
            .field("latency_tier", &self.latency_tier)
            .field("adapter_built", &self.adapter_built.map(|_| "<filter>"))
            .finish()
    }
}

/// The resolver's answer for one session.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub outcome: Outcome,
    /// The canonical provider id, or the trimmed lowercase spelling of an unknown provider.
    pub provider: String,
    pub row_id: String,
    pub layer: Layer,
    /// The model string to send to the vendor.
    pub model_sent: String,
    /// The model as the session gave it, or a placeholder where the provider's model field is
    /// not a model (Reverie keeps an application id there): log this, never the raw string.
    pub model_input: String,
    pub transport: Option<ChosenTransport>,
    pub refusal: Option<Refusal>,
    pub warnings: Vec<Warning>,
    /// Explanations for logs and reviewers; not a wire contract.
    pub notes: Vec<String>,
    /// Whether the rollout switch covers the session (never in Release 0).
    pub covered: bool,
    pub release: u8,
    /// The preference that applied (Release 0 ignores any).
    pub mode: TranscriptionMode,
    pub label: String,
}

impl Resolution {
    /// The matched row.
    ///
    /// # Panics
    /// If `map` is not the map this resolution came from.
    pub fn row<'m>(&self, map: &'m CapabilityMap) -> &'m Row {
        map.row(&self.row_id)
            .unwrap_or_else(|| panic!("row '{}' is not in this capability map", self.row_id))
    }
}

/// The transport a session uses.
#[derive(Debug, Clone, PartialEq)]
pub struct ChosenTransport {
    /// Position in the row's transports, or in the deployment profile and its fallback.
    pub index: usize,
    pub adapter: String,
    pub input_mode: String,
    pub enabled_from_release: Option<u8>,
    /// The profile the transport came from.
    pub profile: Option<String>,
    pub transport: Transport,
}

/// A refusal at setup.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub code: String,
    pub reason: Option<String>,
    pub text: String,
    pub details: Map<String, Value>,
}

/// A coded warning and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    pub code: String,
    pub delivery: Delivery,
    pub detail: Map<String, Value>,
}

impl Warning {
    /// A string detail, if present and a string.
    pub fn detail_str(&self, key: &str) -> Option<&str> {
        self.detail.get(key).and_then(Value::as_str)
    }
}

/// What kind of code path an adapter id names.
pub fn adapter_kind(adapter: &str) -> AdapterKind {
    if adapter == "native" {
        AdapterKind::Native
    } else if FILE_ADAPTERS.contains(&adapter) {
        AdapterKind::Segmented
    } else if COMMIT_ADAPTERS.contains(&adapter) {
        AdapterKind::Commit
    } else {
        AdapterKind::StreamNotBuilt
    }
}

/// Whether the engine cannot frame this wire encoding for its detector (compressed formats).
pub fn is_undecodable_encoding(enc: &str) -> bool {
    let enc = enc.to_lowercase();
    UNDECODABLE.contains(&enc.as_str())
}

/// Resolves one live session. Pure: no I/O, and the same inputs give the same answer.
pub fn resolve(map: &CapabilityMap, req: &ResolveRequest) -> Resolution {
    let mut out = resolve_session(map, req);
    // On today's client a frame is kept only for the buffering warning and for an answer to the
    // session's own preference; every other fact is a notice or only logged (W3 2.16, W5 3.8).
    if out.outcome == Outcome::Native {
        let delivery = native_delivery(out.release, out.covered);
        for w in &mut out.warnings {
            if w.code != "stt_buffered_until_commit" && w.code != "stt_mode_unavailable" {
                w.delivery = delivery;
            }
        }
        // A session the engine does not cover gets the model string byte for byte: a
        // substitution served the row lookup only (W3 2.4 and 3.5 step 8).
        let raw = py_strip(req.model);
        if !out.covered && out.layer != Layer::DeclaredDefault && out.model_sent != raw {
            out.model_sent = raw.to_owned();
            let placeholder = out
                .warnings
                .iter()
                .any(|w| w.code == "stt_placeholder_model_ignored");
            out.notes.push(format!(
                "Not covered: the substitution served the row lookup only; today's client receives the model \
                 string as given{}",
                if placeholder { ", and may reject it at setup as it does today." } else { "." }
            ));
        }
    }
    out
}

/// Why a transport is not usable for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Skip {
    NeverEnabled,
    NotReleased,
    /// The extension: the adapter is not in this build. Counts as `NotReleased` everywhere.
    AdapterNotBuilt,
    EvidenceMissing,
    NotCovered,
    LanguageNotLive,
    RegionNotServed,
    PlainKeepsToday,
}

type Usable<'m> = (usize, ResolvedTransport<'m>);
type Skipped<'m> = (usize, ResolvedTransport<'m>, Skip);

const UNKNOWN_PROVIDER_NOTE: &str = "Unknown to the map: today's factory builds it unchanged and nothing new is \
     reported. If the registry does not know it either, a Bud leg is refused before admission and a standalone \
     session fails with today's unknown-provider error.";
const SENSITIVE_MODEL: &str = "<not shown: this provider's model field is not a model>";

fn resolve_session(map: &CapabilityMap, req: &ResolveRequest) -> Resolution {
    let release = req.release.min(BUILT_RELEASE);
    let covered = req.covered && release >= 1;
    let mut out = Resolution {
        outcome: Outcome::Native,
        provider: String::new(),
        row_id: String::new(),
        layer: Layer::GlobalDefault,
        model_sent: req.model.to_owned(),
        model_input: req.model.to_owned(),
        transport: None,
        refusal: None,
        warnings: Vec::new(),
        notes: Vec::new(),
        covered,
        release,
        mode: req.mode,
        label: String::new(),
    };
    if release == 0 && req.mode != TranscriptionMode::Auto {
        out.notes
            .push("Release 0 has no transcription_mode; the preference is ignored.".to_owned());
        out.mode = TranscriptionMode::Auto;
    }
    let mode = out.mode;

    // 1. Provider.
    let Some(pid) = map.provider_id(req.provider) else {
        out.provider = py_strip(req.provider).to_lowercase();
        out.row_id = map.global_row().id.clone();
        out.label = "today's client through the plugin registry (unclassified provider)".to_owned();
        out.notes.push(UNKNOWN_PROVIDER_NOTE.to_owned());
        return out;
    };
    let entry = map
        .provider(pid)
        .expect("provider_id returns a known provider");
    out.provider = pid.to_owned();
    let kn = &entry.key_normalisation;
    let model_string = entry.model_string;

    // 2. Model normalisation (W3 2.4): the lookup and the string sent change together.
    let raw = py_strip(req.model);
    let mut sent = raw.to_owned();
    let mut lookup = if model_string == ModelString::DeploymentName {
        py_strip(non_empty(req.underlying_model).unwrap_or(raw)).to_owned()
    } else {
        raw.to_owned()
    };
    let sensitive = model_string == ModelString::Sensitive;
    if matches!(model_string, ModelString::Absent | ModelString::Sensitive) {
        lookup.clear();
        if sensitive {
            out.model_input = if raw.is_empty() {
                String::new()
            } else {
                SENSITIVE_MODEL.to_owned()
            };
        }
    }
    let mut region = req.region.map(str::to_owned);
    if let Some(sep) = non_empty(kn.region_suffix_separator.as_deref())
        && !lookup.is_empty()
        && let Some((head, suffix)) = lookup.split_once(sep)
    {
        let (head, suffix) = (head.to_owned(), suffix.to_owned());
        if !suffix.is_empty() && non_empty(region.as_deref()).is_none() {
            region = Some(suffix.clone());
        }
        out.notes.push(format!(
            "Region suffix '{suffix}' taken from the model string; the string sent is unchanged."
        ));
        lookup = head;
    }
    if kn.strip_provider_prefix
        && let Some((head, tail)) = lookup.split_once('/')
        && map.provider_id(head) == Some(pid)
        && !tail.is_empty()
    {
        out.notes
            .push(format!("Provider prefix '{head}/' removed from the model."));
        let tail = tail.to_owned();
        sent.clone_from(&tail);
        lookup = tail;
    }
    let mut placeholder = None;
    if kn.placeholder_as_unset
        && !req.bud_leg
        && map.is_placeholder(&lookup.to_lowercase())
        && !has_specific_row(map, pid, &lookup)
    {
        placeholder = Some(std::mem::take(&mut lookup));
        sent.clear();
    }

    // 3. Row and layer.
    let (row_index, mut layer) = if lookup.is_empty() {
        // No model named, or a provider whose model string is not looked up: the declared answer.
        let row_index = match non_empty(entry.default_model.as_deref()) {
            Some(dm) => find_row(map, pid, dm).0,
            None => provider_default_row(map, pid).expect("every provider has a default row"),
        };
        let declared =
            matches!(model_string, ModelString::Absent | ModelString::Sensitive) && !raw.is_empty();
        if model_string == ModelString::Absent {
            sent.clear();
        }
        (
            row_index,
            if declared {
                Layer::DeclaredDefault
            } else {
                Layer::ModelUnset
            },
        )
    } else {
        find_row(map, pid, &lookup)
    };
    let row = map.row_at(row_index);
    out.row_id = row.id.clone();
    out.layer = layer;
    out.model_sent = if sensitive {
        out.model_input.clone()
    } else {
        sent
    };
    if let Some(received) = &placeholder {
        out.warnings.push(warning(
            "stt_placeholder_model_ignored",
            Delivery::Frame,
            [
                ("received", Value::from(received.as_str())),
                ("model", opt_str(entry.default_model.as_deref())),
            ],
        ));
    }
    let named = !raw.is_empty()
        && placeholder.is_none()
        && !matches!(model_string, ModelString::Absent | ModelString::Sensitive);
    let life = &row.lifecycle;
    let wu = row.when_unusable.as_ref();

    // 4. A retired model is refused in every release, except where today's client silently
    //    serves another model: decision 12 warns such a session until its refuse_from_release.
    let substituted_until = wu.is_some_and(|w| {
        w.today == TodayBehaviour::StreamsSubstitutedModel
            && w.refuse_from_release.is_some_and(|r| release < r)
    });
    if life.status == LifecycleStatus::Retired && !substituted_until {
        let text = wu
            .and_then(|w| w.refusal.as_ref())
            .map_or("This model is no longer served by the vendor.", |r| {
                r.text.as_str()
            });
        let replacement = life
            .replacement
            .as_ref()
            .map_or(Value::Null, |r| Value::from(r.clone()));
        return refuse(
            out,
            "stt_model_retired",
            None,
            text,
            [("replacement", replacement)],
        );
    }

    // 5. Transports, after a deployment override's profile (W3 3.5 step 6).
    let mut transports = map.transports(row);
    let profile = non_empty(req.profile);
    if let Some(name) = profile {
        match map.profile(name) {
            Some(p) if p.allowed_providers.iter().any(|a| a == pid) => {
                transports = map.profile_transport(name).into_iter().collect();
                if let Some(fallback) = non_empty(p.fallback_profile.as_deref()) {
                    transports.extend(map.profile_transport(fallback));
                }
                layer = Layer::DeploymentOverride;
                out.layer = layer;
            }
            _ => out
                .notes
                .push(format!("deployment_setting_not_applied: profile '{name}' is unknown or does not allow {pid}.")),
        }
    }

    // 6. Which transports this session can use.
    let mut usable: Vec<Usable> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    for (i, t) in transports.iter().enumerate() {
        let kind = adapter_kind(&t.transport.adapter);
        let why = match t.transport.enabled_from_release {
            None => Some(Skip::NeverEnabled),
            Some(efr) if efr > release => Some(Skip::NotReleased),
            Some(_)
                if kind != AdapterKind::Native
                    && req.adapter_built.is_some_and(|f| !f(&t.transport.adapter)) =>
            {
                Some(Skip::AdapterNotBuilt)
            }
            Some(_) if !evidence_present(row, t.transport, req.evidence, req.deadline_ms) => {
                Some(Skip::EvidenceMissing)
            }
            Some(_) if kind != AdapterKind::Native && !covered => Some(Skip::NotCovered),
            Some(_) => match constraint_blocks(t.transport, req.language, region.as_deref()) {
                Some(block)
                    if kind != AdapterKind::Native || release >= NATIVE_CONSTRAINTS_FROM =>
                {
                    Some(block)
                }
                Some(block) => {
                    let what = if block == Skip::LanguageNotLive {
                        "language"
                    } else {
                        "region"
                    };
                    out.notes.push(format!(
                        "The session's {what} is outside the native client's constraint; today's path is kept until \
                         Release 3 (addendum A7)."
                    ));
                    None
                }
                None => None,
            },
        };
        match why {
            Some(why) => skipped.push((i, *t, why)),
            None => usable.push((i, *t)),
        }
    }
    if req.session == SessionKind::Plain
        && wu.is_some_and(|w| w.today == TodayBehaviour::BuffersUntilHangup)
        && mode != TranscriptionMode::Segmented
    {
        // Addendum B4: a plain /ws client that ends its own turns keeps today's buffering client.
        let (kept, moved): (Vec<Usable>, Vec<Usable>) = usable
            .into_iter()
            .partition(|(_, t)| adapter_kind(&t.transport.adapter) == AdapterKind::Native);
        skipped.extend(
            moved
                .into_iter()
                .map(|(i, t)| (i, t, Skip::PlainKeepsToday)),
        );
        usable = kept;
    }
    if let Some(declared) = profile
        && skipped.first().is_some_and(|s| s.0 == 0)
        && let Some((_, first)) = usable.first()
    {
        out.warnings.push(warning(
            "stt_transport_fallback",
            Delivery::Frame,
            [
                ("declared", Value::from(declared)),
                ("effective", opt_str(first.profile)),
            ],
        ));
    }

    // 7. Pick by preference.
    let mut chosen = usable.first().copied();
    if req.latency_tier == LatencyTier::LowLatency
        && mode == TranscriptionMode::Auto
        && release >= 4
    {
        // Addendum B9: the low-latency tier prefers a usable commit transport to a file transport.
        if let Some(commit) = usable
            .iter()
            .find(|(_, t)| adapter_kind(&t.transport.adapter) == AdapterKind::Commit)
        {
            chosen = Some(*commit);
        }
    }
    if mode == TranscriptionMode::Streaming && !usable.is_empty() {
        match usable
            .iter()
            .find(|(_, t)| t.transport.input_mode == InputMode::LiveStream)
        {
            Some(live) => chosen = Some(*live),
            None => {
                return refuse(
                    out,
                    "stt_not_streaming",
                    None,
                    "The session asked for streaming and this model has no live-stream transport in this release.",
                    [],
                );
            }
        }
    }
    if mode == TranscriptionMode::Segmented && !usable.is_empty() {
        match usable
            .iter()
            .find(|(_, t)| adapter_kind(&t.transport.adapter) == AdapterKind::Segmented)
        {
            Some(seg) => chosen = Some(*seg),
            None => {
                let has_file = transports
                    .iter()
                    .any(|t| adapter_kind(&t.transport.adapter) == AdapterKind::Segmented);
                out.warnings.push(warning(
                    "stt_mode_unavailable",
                    Delivery::Frame,
                    [
                        ("requested", Value::from("segmented")),
                        ("source", Value::from("request")),
                        (
                            "reason",
                            Value::from(if has_file {
                                "not_enabled"
                            } else {
                                "no_transport"
                            }),
                        ),
                    ],
                ));
            }
        }
    }

    // 8. No usable transport: today's behaviour, the release's refusal, or today's codes.
    let Some((index, chosen)) = chosen else {
        let facts = Unusable {
            registered: entry.registered_in_gateway,
            pid,
            row,
            release,
            covered,
            named,
        };
        return no_usable_transport(out, &facts, &skipped, req.session, mode, req.today);
    };

    let t = chosen.transport;
    let kind = adapter_kind(&t.adapter);
    out.transport = Some(ChosenTransport {
        index,
        adapter: t.adapter.clone(),
        input_mode: t.input_mode.as_str().to_owned(),
        enabled_from_release: t.enabled_from_release,
        profile: chosen.profile.map(str::to_owned),
        transport: t.clone(),
    });
    if !t.enable_requires.is_empty() && req.evidence == Evidence::Assume {
        let needs: Vec<&str> = t
            .enable_requires
            .iter()
            .map(|r| match r {
                EnableRequirement::LiveProbe => "a live probe with a real key has succeeded",
                EnableRequirement::LatencyWithinDeadline => {
                    "a measurement shows it answers within the deadline"
                }
            })
            .collect();
        out.notes
            .push(format!("Enabled only once {}.", needs.join(" and ")));
    }
    if let Some(c) = t.constraints.as_ref().filter(|c| !c.is_empty()) {
        let mut parts = Vec::new();
        if let Some(l) = &c.languages {
            parts.push(format!("languages {} {}", l.mode, l.codes.join(", ")));
        }
        if let Some(r) = &c.regions {
            parts.push(format!("regions {} {}", r.mode, r.values.join(", ")));
        }
        out.notes.push(format!("Constraints: {}", parts.join("; ")));
    }

    if kind != AdapterKind::Native {
        if let Some(enc) = non_empty(req.encoding)
            && is_undecodable_encoding(enc)
        {
            let text = format!(
                "The session's audio encoding '{enc}' cannot be cut into utterances by the gateway."
            );
            return refuse(
                out,
                "stt_segmentation_unavailable",
                Some("audio_format"),
                &text,
                [],
            );
        }
        let (outcome, what) = match kind {
            AdapterKind::Segmented => (Outcome::Segmented, "per-utterance upload"),
            AdapterKind::Commit => (Outcome::Commit, "gateway-driven commit"),
            // `CapabilityMap::from_json` refuses an enabled transport whose adapter no release builds.
            AdapterKind::Native | AdapterKind::StreamNotBuilt => {
                unreachable!("adapter '{}' is never enabled", t.adapter)
            }
        };
        out.outcome = outcome;
        out.label = format!("{what} ({})", t.adapter);
        if kind == AdapterKind::Segmented && mode != TranscriptionMode::Segmented {
            out.warnings
                .push(warning("stt_segmented_mode", Delivery::Frame, []));
        }
        let bill = t.billing.as_ref().unwrap_or(&row.billing);
        if kind == AdapterKind::Segmented
            && let Some(min) = bill.min_billed_ms.filter(|ms| *ms > 0)
        {
            out.warnings.push(warning(
                "stt_min_billed_duration",
                Delivery::Frame,
                [("min_billed_ms", Value::from(min))],
            ));
        }
        if release >= 2
            && let Some(p99) = seed_p99(t).filter(|ms| *ms > SLOW_TARGET_MS)
        {
            out.warnings.push(warning(
                "stt_latency_slow",
                Delivery::Frame,
                [
                    ("final_latency_slow_ms", Value::from(p99)),
                    ("target_ms", Value::from(SLOW_TARGET_MS)),
                ],
            ));
        }
        if matches!(layer, Layer::ProviderDefault | Layer::GlobalDefault)
            && !row.applies_to_all_models
        {
            out.warnings
                .push(capability_assumed(layer, Delivery::Frame));
        }
        lifecycle_warning(&mut out.warnings, life, Delivery::Frame, req.today);
        if layer == Layer::ModelUnset
            && let Some(dm) = non_empty(entry.default_model.as_deref())
        {
            dm.clone_into(&mut out.model_sent);
        }
        return out;
    }

    // Today's client, chosen as a listed native transport.
    out.outcome = Outcome::Native;
    out.label = "native stream".to_owned();
    let delivery = native_delivery(release, covered);
    if let Some(status) = t.gateway_client.as_ref().map(|g| g.status)
        && matches!(
            status,
            GatewayClientStatus::KnownBroken | GatewayClientStatus::Unverified
        )
    {
        out.warnings.push(warning(
            "stt_client_unverified",
            delivery,
            [("status", Value::from(status.as_str()))],
        ));
        out.label
            .push_str(&format!(" (client {})", status.as_str().replace('_', " ")));
    }
    let handling = entry.native_model_handling;
    if named
        && matches!(
            handling,
            NativeModelHandling::Substituted | NativeModelHandling::Ignored
        )
        && matches!(layer, Layer::Pattern | Layer::ProviderDefault)
    {
        out.warnings.push(warning(
            "stt_model_substituted",
            delivery,
            [
                ("requested", Value::from(raw)),
                ("handling", Value::from(handling.as_str())),
                ("model_that_runs", opt_str(entry.default_model.as_deref())),
            ],
        ));
    }
    if matches!(layer, Layer::ProviderDefault | Layer::GlobalDefault) && !row.applies_to_all_models
    {
        out.warnings.push(capability_assumed(layer, delivery));
    }
    lifecycle_warning(&mut out.warnings, life, delivery, req.today);
    out
}

/// What [`no_usable_transport`] needs from the session beyond the request.
struct Unusable<'m> {
    /// The provider has a live client registered in the gateway today.
    registered: bool,
    pid: &'m str,
    row: &'m Row,
    release: u8,
    covered: bool,
    named: bool,
}

fn no_usable_transport(
    mut out: Resolution,
    s: &Unusable,
    skipped: &[Skipped],
    session: SessionKind,
    mode: TranscriptionMode,
    today: &str,
) -> Resolution {
    let wu = s.row.when_unusable.as_ref();
    let row_refusal = wu.and_then(|w| w.refusal.as_ref());
    let row_text = row_refusal.map_or("", |r| r.text.as_str());
    let row_reason = row_refusal.and_then(|r| r.reason).map(|r| r.as_str());
    let row_code = row_refusal.map_or("stt_live_unsupported", |r| r.code.as_str());
    let today_kind = wu.map(|w| w.today);
    // A transport the session could use if the switch covered it.
    let would_if_covered = skipped.iter().any(|(_, _, why)| *why == Skip::NotCovered);
    // Constraints on the only usable transports: the session's language or region is not served.
    let blocked: Vec<Skip> = skipped
        .iter()
        .map(|(_, _, why)| *why)
        .filter(|why| matches!(why, Skip::LanguageNotLive | Skip::RegionNotServed))
        .collect();
    let language_blocked = blocked.contains(&Skip::LanguageNotLive);

    // Self-hosted and Azure OpenAI keep today's two codes while not covered (addendum A1).
    if !s.registered
        && today_kind == Some(TodayBehaviour::RefusedAtSetup)
        && (s.release == 0 || !s.covered)
    {
        let code = if session == SessionKind::Gateway {
            "stt_not_streaming"
        } else {
            "unsupported_deployment"
        };
        return refuse(
            out,
            code,
            None,
            "Today's refusal of a deployment that transcribes uploaded files, with today's text (the code is \
             stt_not_streaming on a voice-agent leg and unsupported_deployment on a named-deployment leg).",
            [],
        );
    }

    if let Some(w) = wu
        && w.refuse_from_release.is_some_and(|r| s.release >= r)
    {
        return refuse(out, row_code, row_reason, row_text, []);
    }

    let Some(today_kind) = today_kind else {
        // Every listed transport is skipped and the row records no behaviour today: name the
        // reason (W3 3.5 step 7).
        let mut reason = if language_blocked {
            "language_not_live"
        } else if !blocked.is_empty() {
            "disabled"
        } else {
            "client_not_implemented"
        };
        if would_if_covered && blocked.is_empty() {
            reason = "not_covered_yet";
        }
        let text = if blocked.is_empty() {
            "No transport of this model is usable for this session."
        } else {
            "No transport of this model can serve the session's language or region on a live call."
        };
        return refuse(out, "stt_live_unsupported", Some(reason), text, []);
    };

    let delivery = native_delivery(s.release, s.covered);
    let life = &s.row.lifecycle;
    match today_kind {
        TodayBehaviour::BuffersUntilHangup => {
            if session == SessionKind::Gateway {
                // Addendum B10: not_covered_yet only when a covered session would get a transport.
                if would_if_covered {
                    return refuse(
                        out,
                        "stt_live_unsupported",
                        Some("not_covered_yet"),
                        row_text,
                        [],
                    );
                }
                let reason = row_reason.unwrap_or("client_not_implemented");
                return refuse(
                    out,
                    "stt_live_unsupported",
                    Some(reason),
                    &without_operator_clause(row_text),
                    [],
                );
            }
            if mode == TranscriptionMode::Streaming {
                return refuse(
                    out,
                    "stt_not_streaming",
                    None,
                    "Today's client for this model returns text only when the client sends audio_end or hangs up; \
                     it does not stream.",
                    [],
                );
            }
            out.outcome = Outcome::Native;
            out.label = "today's buffering client".to_owned();
            let model = Value::from(out.model_input.as_str());
            let detail = [("provider", Value::from(s.pid)), ("model", model)];
            out.warnings.insert(
                0,
                warning("stt_buffered_until_commit", Delivery::Frame, detail),
            );
            lifecycle_warning(&mut out.warnings, life, delivery, today);
            out
        }
        TodayBehaviour::RefusedAtSetup | TodayBehaviour::Fails => {
            let mut reason = row_reason;
            let mut text = row_text.to_owned();
            let held_back = skipped.iter().any(|(_, _, why)| {
                matches!(
                    why,
                    Skip::NotReleased | Skip::AdapterNotBuilt | Skip::EvidenceMissing
                )
            });
            if would_if_covered {
                reason = Some("not_covered_yet");
            } else if !blocked.is_empty() && !held_back {
                // Wire reasons of W5 section 3.9: a language served by batch only, or a region not served.
                let language = language_blocked;
                reason = Some(if language {
                    "language_not_live"
                } else {
                    "disabled"
                });
                text = format!(
                    "This model is not served on a live call in the session's {}.",
                    if language { "language" } else { "region" }
                );
            }
            if reason != Some("not_covered_yet") {
                text = without_operator_clause(&text);
            }
            refuse(out, row_code, reason, &text, [])
        }
        TodayBehaviour::BlindTimedUploads => {
            if mode == TranscriptionMode::Streaming {
                return refuse(
                    out,
                    "stt_not_streaming",
                    None,
                    "Today's client for this model uploads timed pieces; it does not stream.",
                    [],
                );
            }
            out.outcome = Outcome::Native;
            out.label = "today's timed-upload client (known broken)".to_owned();
            out.warnings.push(warning(
                "stt_client_unverified",
                delivery,
                [
                    ("status", Value::from("known_broken")),
                    (
                        "why",
                        Value::from("uploads blind timed pieces and marks each as an end of turn"),
                    ),
                ],
            ));
            lifecycle_warning(&mut out.warnings, life, delivery, today);
            out
        }
        TodayBehaviour::StreamsSubstitutedModel => {
            let substitute = wu.and_then(|w| w.substituted_model.as_deref());
            out.outcome = Outcome::Native;
            out.label = format!(
                "today's client, streaming {} instead",
                substitute.unwrap_or("None")
            );
            let requested = if s.named {
                Value::from(out.model_input.as_str())
            } else {
                Value::Null
            };
            out.warnings.push(warning(
                "stt_model_substituted",
                delivery,
                [
                    ("requested", requested),
                    ("model_that_runs", opt_str(substitute)),
                ],
            ));
            lifecycle_warning(&mut out.warnings, life, delivery, today);
            out
        }
        TodayBehaviour::Streams | TodayBehaviour::Unknown => {
            let unknown = today_kind == TodayBehaviour::Unknown;
            out.outcome = Outcome::Native;
            out.label = format!(
                "today's client{}",
                if unknown {
                    " (behaviour for this model not established)"
                } else {
                    ""
                }
            );
            if unknown {
                out.warnings.push(warning(
                    "stt_client_unverified",
                    delivery,
                    [
                        ("status", Value::from("unverified")),
                        (
                            "why",
                            Value::from("today's behaviour for this model is unknown"),
                        ),
                    ],
                ));
            }
            if matches!(out.layer, Layer::ProviderDefault | Layer::GlobalDefault)
                && !s.row.applies_to_all_models
            {
                out.warnings.push(capability_assumed(out.layer, delivery));
            }
            lifecycle_warning(&mut out.warnings, life, delivery, today);
            out
        }
    }
}

fn refuse<const N: usize>(
    mut out: Resolution,
    code: &str,
    reason: Option<&str>,
    text: &str,
    details: [(&str, Value); N],
) -> Resolution {
    out.outcome = Outcome::Refused;
    out.label = match reason {
        Some(reason) => format!("refused {code} ({reason})"),
        None => format!("refused {code}"),
    };
    out.refusal = Some(Refusal {
        code: code.to_owned(),
        reason: reason.map(str::to_owned),
        text: text.to_owned(),
        details: details
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    });
    out.transport = None;
    out
}

fn warning<const N: usize>(code: &str, delivery: Delivery, detail: [(&str, Value); N]) -> Warning {
    Warning {
        code: code.to_owned(),
        delivery,
        detail: detail.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
    }
}

fn capability_assumed(layer: Layer, delivery: Delivery) -> Warning {
    warning(
        "stt_capability_assumed",
        delivery,
        [("capability_source", Value::from(layer.as_str()))],
    )
}

/// Where a warning about today's client goes (W3 2.16, addendum A6): only the log and the counter
/// in Release 0 and on a session the switch does not cover in Releases 1 and 2; a notice otherwise.
fn native_delivery(release: u8, covered: bool) -> Delivery {
    if release == 0 || (release < 3 && !covered) {
        Delivery::Log
    } else {
        Delivery::Notice
    }
}

fn lifecycle_warning(
    warnings: &mut Vec<Warning>,
    life: &Lifecycle,
    delivery: Delivery,
    today: &str,
) {
    let shutdown_on = life.shutdown_on.as_deref();
    let dated = non_empty(shutdown_on).is_some();
    if life.status == LifecycleStatus::Deprecated
        || (life.status == LifecycleStatus::Retiring && dated)
    {
        let past = non_empty(shutdown_on).is_some_and(|sd| sd <= today);
        let replacement = life
            .replacement
            .as_ref()
            .map_or(Value::Null, |r| Value::from(r.clone()));
        warnings.push(warning(
            "stt_model_deprecated",
            delivery,
            [
                ("shutdown_on", opt_str(shutdown_on)),
                ("past_shutdown", Value::from(past)),
                ("replacement", replacement),
            ],
        ));
    }
}

fn opt_str(s: Option<&str>) -> Value {
    s.map_or(Value::Null, Value::from)
}

/// Python truthiness of an optional string.
fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.is_empty())
}

/// Exact model or alias, else the most specific anchored glob, else the provider default row,
/// else the global default row (W3 2.4 and 2.5).
fn find_row(map: &CapabilityMap, pid: &str, lookup: &str) -> (usize, Layer) {
    let rows = map.row_indexes_for(pid);
    let low = py_strip(lookup).to_lowercase();
    for &i in rows {
        let keys = map.row_keys(i);
        if keys
            .model
            .as_deref()
            .is_some_and(|m| m == low || keys.aliases.contains(&low))
        {
            return (i, Layer::Exact);
        }
    }
    let mut best: Option<(usize, &str)> = None;
    for &i in rows {
        let Some(glob) = non_empty(map.row_at(i).r#match.model_glob.as_deref()) else {
            continue;
        };
        if glob == "*" || !glob_match(map.row_keys(i).glob.as_deref().unwrap_or_default(), &low) {
            continue;
        }
        let better = match best {
            None => true,
            Some((b, best_glob)) => {
                (map.row_keys(i).glob_literal_len, glob)
                    > (map.row_keys(b).glob_literal_len, best_glob)
            }
        };
        if better {
            best = Some((i, glob));
        }
    }
    if let Some((i, _)) = best {
        return (i, Layer::Pattern);
    }
    match provider_default_row(map, pid) {
        Some(i) => (i, Layer::ProviderDefault),
        None => (map.global_row_index(), Layer::GlobalDefault),
    }
}

fn provider_default_row(map: &CapabilityMap, pid: &str) -> Option<usize> {
    map.row_indexes_for(pid)
        .iter()
        .copied()
        .find(|&i| map.row_at(i).r#match.model_glob.as_deref() == Some("*"))
}

fn has_specific_row(map: &CapabilityMap, pid: &str, model: &str) -> bool {
    matches!(find_row(map, pid, model).1, Layer::Exact | Layer::Pattern)
}

/// An anchored match of a lowercased pattern whose only wildcard is `*`. As in the reference's
/// regular expression, `*` matches any run of characters except a line break.
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern.contains('\n') {
        return glob_match_slow(pattern, text);
    }
    if text.contains('\n') {
        return false;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, rest) = parts.split_first().expect("split yields at least one part");
    let Some((last, middle)) = rest.split_last() else {
        return text == *first;
    };
    if text.len() < first.len() + last.len() || !text.starts_with(first) || !text.ends_with(last) {
        return false;
    }
    let mut window = &text[first.len()..text.len() - last.len()];
    for part in middle {
        match window.find(part) {
            Some(at) => window = &window[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// The general case, for a pattern that itself contains a line break.
fn glob_match_slow(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // can[j]: the first i pattern characters match the first j text characters.
    let mut can = vec![false; t.len() + 1];
    can[0] = true;
    for &pc in &p {
        let mut next = vec![false; t.len() + 1];
        if pc == '*' {
            for j in 0..=t.len() {
                next[j] = can[j] || (j > 0 && next[j - 1] && t[j - 1] != '\n');
            }
        } else {
            for j in 1..=t.len() {
                next[j] = can[j - 1] && t[j - 1] == pc;
            }
        }
        can = next;
    }
    can[t.len()]
}

/// Why the transport cannot serve this language or region. An unknown language or region is not
/// checked: the expected tables say "where the constraint admits the session".
fn constraint_blocks(t: &Transport, language: Option<&str>, region: Option<&str>) -> Option<Skip> {
    let c = t.constraints.as_ref()?;
    if let (Some(lc), Some(language)) = (&c.languages, non_empty(language)) {
        let hit = lc.codes.iter().any(|code| language_matches(code, language));
        if (lc.mode == ConstraintMode::Only && !hit) || (lc.mode == ConstraintMode::Except && hit) {
            return Some(Skip::LanguageNotLive);
        }
    }
    if let (Some(rc), Some(region)) = (&c.regions, non_empty(region)) {
        let region = region.to_lowercase();
        let hit = rc.values.iter().any(|v| v.to_lowercase() == region);
        if (rc.mode == ConstraintMode::Only && !hit) || (rc.mode == ConstraintMode::Except && hit) {
            return Some(Skip::RegionNotServed);
        }
    }
    None
}

/// A bare language covers every region of it, and a bare session language meets a regional code.
fn language_matches(code: &str, language: &str) -> bool {
    let (c, l) = (code.to_lowercase(), language.to_lowercase());
    let base = |s: &str| s.split('-').next().unwrap_or_default().to_owned();
    c == l || (!c.contains('-') && base(&l) == c) || (!l.contains('-') && base(&c) == l)
}

fn evidence_present(row: &Row, t: &Transport, evidence: Evidence, deadline_ms: u32) -> bool {
    if t.enable_requires.is_empty() || evidence == Evidence::Assume {
        return true;
    }
    let provenance = row.provenance.as_ref();
    t.enable_requires.iter().all(|need| match need {
        EnableRequirement::LiveProbe => provenance.is_some_and(|p| {
            p.verified_by == Some(VerifiedBy::LiveProbe)
                && non_empty(p.probe_ref.as_deref()).is_some()
        }),
        EnableRequirement::LatencyWithinDeadline => t.latency.measurements.iter().any(|m| {
            matches!(
                m.quantity,
                Quantity::EndOfSpeechToFinal | Quantity::RequestRoundTrip
            ) && matches!(m.percentile, Percentile::P95 | Percentile::P99)
                && m.value_ms <= u64::from(deadline_ms)
        }),
    })
}

/// The seed 99th percentile from end of speech to final transcript.
fn seed_p99(t: &Transport) -> Option<u64> {
    t.latency
        .measurements
        .iter()
        .find(|m| m.percentile == Percentile::P99 && m.quantity == Quantity::EndOfSpeechToFinal)
        .map(|m| m.value_ms)
}

/// Today's date in UTC, `YYYY-MM-DD`. One string is kept per calendar day.
pub fn utc_today() -> &'static str {
    static TODAY: Mutex<Option<(u64, &'static str)>> = Mutex::new(None);
    let day = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    let mut cached = TODAY.lock().unwrap_or_else(PoisonError::into_inner);
    match *cached {
        Some((d, text)) if d == day => text,
        _ => {
            let text: &'static str = Box::leak(civil_date(day).into_boxed_str());
            *cached = Some((day, text));
            text
        }
    }
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
fn civil_date(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Addendum B10: a refusal whose cause is not the rollout switch must not tell the customer to
/// ask the operator; it says segmented speech-to-text is not available in this release.
pub fn without_operator_clause(text: &str) -> String {
    // The reference uses three regular expressions; each step below is one of them, applied left to
    // right without overlap as `re.sub` applies it.
    // "A, B, or ask ..." -> "A or B."
    let step1 = substitute(text, |s, at| {
        let rest = s.get(at..)?.strip_prefix(", ")?;
        let group_len = rest.find([',', '.'])?;
        if group_len == 0 {
            return None;
        }
        let after = rest[group_len..].strip_prefix(", or ")?;
        let clause = operator_clause_at(after)?;
        after[clause..].strip_prefix('.')?;
        let end = at + 2 + group_len + ", or ".len() + clause + 1;
        Some((end, format!(" or {}.", &rest[..group_len])))
    });
    // "A, or ask ..." -> "A."
    let step2 = substitute(&step1, |s, at| {
        let rest = s.get(at..)?;
        let comma = usize::from(rest.starts_with(','));
        let after = rest[comma..].strip_prefix(" or ")?;
        let clause = operator_clause_at(after)?;
        after[clause..].strip_prefix('.')?;
        Some((at + comma + " or ".len() + clause + 1, ".".to_owned()))
    });
    // "A, ask ..., or C" -> "A, or C"
    let mut new = substitute(&step2, |s, at| {
        let rest = s.get(at..)?;
        let clause = operator_clause_at(rest)?;
        rest[clause..].strip_prefix(", ")?;
        Some((at + clause + 2, String::new()))
    });
    if new != text {
        new.push_str(
            " Segmented speech-to-text for this provider is not available in this release.",
        );
    }
    new
}

/// The length of the operator clause at the start of `s`, if it starts with one.
fn operator_clause_at(s: &str) -> Option<usize> {
    const CLAUSES: [&str; 2] = [
        "ask the operator to enable segmented speech-to-text for this deployment",
        "ask the operator whether segmented speech-to-text can be enabled for this deployment",
    ];
    CLAUSES.iter().find(|c| s.starts_with(*c)).map(|c| c.len())
}

/// Replaces, left to right and without overlap, each match that `at` finds. `at(text, i)` returns
/// the end of a match starting at byte `i` and its replacement. Matches start on ASCII bytes, so
/// every slice falls on a character boundary.
fn substitute(text: &str, at: impl Fn(&str, usize) -> Option<(usize, String)>) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut copied, mut i) = (0, 0);
    while i < text.len() {
        match text.is_char_boundary(i).then(|| at(text, i)).flatten() {
            Some((end, replacement)) => {
                out.push_str(&text[copied..i]);
                out.push_str(&replacement);
                copied = end;
                i = end;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use serde_json::json;

    use super::*;

    const TODAY: &str = "2026-10-05";
    /// Providers whose sessions the reference tables resolve as Bud deployment legs.
    const BUD_LEG_PROVIDERS: &[&str] = &["self_hosted", "azure_openai", "waav-infer"];
    const C1: &str = "ask the operator to enable segmented speech-to-text for this deployment";
    const C2: &str =
        "ask the operator whether segmented speech-to-text can be enabled for this deployment";
    const NOT_AVAILABLE: &str =
        " Segmented speech-to-text for this provider is not available in this release.";

    fn map() -> &'static CapabilityMap {
        CapabilityMap::embedded()
    }

    fn req(provider: &'static str, model: &'static str) -> ResolveRequest<'static> {
        ResolveRequest {
            today: TODAY,
            ..ResolveRequest::new(provider, model)
        }
    }

    fn run(r: ResolveRequest) -> Resolution {
        resolve(map(), &r)
    }

    fn adapter(res: &Resolution) -> Option<&str> {
        res.transport.as_ref().map(|t| t.adapter.as_str())
    }

    fn code(res: &Resolution) -> Option<&str> {
        res.refusal.as_ref().map(|r| r.code.as_str())
    }

    fn reason(res: &Resolution) -> Option<&str> {
        res.refusal.as_ref().and_then(|r| r.reason.as_deref())
    }

    fn codes(res: &Resolution) -> BTreeSet<&str> {
        res.warnings.iter().map(|w| w.code.as_str()).collect()
    }

    #[track_caller]
    fn assert_warns(res: &Resolution, want: &[&str]) {
        let have = codes(res);
        let missing: Vec<_> = want.iter().filter(|c| !have.contains(*c)).collect();
        assert!(
            missing.is_empty(),
            "warnings lack {missing:?} (have {have:?})"
        );
    }

    #[track_caller]
    fn assert_no_warns(res: &Resolution, unwanted: &[&str]) {
        let have = codes(res);
        let extra: Vec<_> = unwanted.iter().filter(|c| have.contains(*c)).collect();
        assert!(extra.is_empty(), "unexpected warnings {extra:?}");
    }

    // The cases of `self_test()` in resolve.py, in its order and under its names.

    #[test]
    fn openai_file_model_is_uploaded_per_utterance_from_release_1() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("openai", "gpt-transcribe")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("openai_transcriptions"));
    }

    #[test]
    fn groq_whisper_is_uploaded_per_utterance_from_release_1_and_carries_the_10_s_minimum() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("groq", "whisper-large-v3-turbo")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("groq_transcriptions"));
        assert_warns(&res, &["stt_min_billed_duration", "stt_segmented_mode"]);
    }

    #[test]
    fn elevenlabs_scribe_v2_is_uploaded_per_utterance_from_release_1() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("elevenlabs", "scribe_v2")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("elevenlabs_batch"));
    }

    #[test]
    fn a_self_hosted_deployment_uses_the_default_profile_from_release_1_with_no_assumed_warning() {
        let res = run(ResolveRequest {
            release: 1,
            bud_leg: true,
            ..req("self_hosted", "openai/whisper-large-v3")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("openai_transcriptions"));
        assert_no_warns(&res, &["stt_capability_assumed"]);
    }

    #[test]
    fn waav_infer_uses_its_own_profile_from_release_1() {
        let res = run(ResolveRequest {
            release: 1,
            bud_leg: true,
            ..req("waav-infer", "parakeet")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("openai_transcriptions"));
    }

    #[test]
    fn azure_openai_deployment_uses_the_azure_profile_from_release_1() {
        let res = run(ResolveRequest {
            release: 1,
            bud_leg: true,
            ..req("azure-openai", "my-transcriber")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("azure_openai_transcriptions"));
    }

    macro_rules! deepgram_nova_3_stays_native {
        ($($name:ident: $release:expr),+ $(,)?) => {$(
            #[test]
            fn $name() {
                let res = run(ResolveRequest { release: $release, ..req("deepgram", "nova-3") });
                assert_eq!(res.outcome, Outcome::Native);
                assert_eq!(adapter(&res), Some("native"));
                assert_eq!(res.label, "native stream");
                assert_no_warns(&res, &["stt_segmented_mode"]);
            }
        )+};
    }

    deepgram_nova_3_stays_native!(
        deepgram_nova_3_stays_native_in_release_0: 0,
        deepgram_nova_3_stays_native_in_release_1: 1,
        deepgram_nova_3_stays_native_in_release_2: 2,
        deepgram_nova_3_stays_native_in_release_3: 3,
        deepgram_nova_3_stays_native_in_release_4: 4,
        deepgram_nova_3_stays_native_in_release_5: 5,
        deepgram_nova_3_stays_native_in_release_6: 6,
    );

    #[test]
    fn deepgram_whisper_large_is_refused_before_release_3() {
        let res = run(ResolveRequest {
            release: 2,
            ..req("deepgram", "whisper-large")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("client_not_implemented"));
    }

    #[test]
    fn deepgram_whisper_large_is_uploaded_per_utterance_from_release_3() {
        let res = run(ResolveRequest {
            release: 3,
            ..req("deepgram", "whisper-large")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("deepgram_prerecorded"));
    }

    #[test]
    fn deepgram_whisper_on_an_uncovered_release_3_session_not_covered_yet() {
        let res = run(ResolveRequest {
            release: 3,
            covered: false,
            ..req("deepgram", "whisper-large")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("not_covered_yet"));
    }

    #[test]
    fn elevenlabs_scribe_v2_is_refused_in_release_0_on_every_session() {
        let res = run(ResolveRequest {
            session: SessionKind::PushToTalk,
            ..req("elevenlabs", "scribe_v2")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("client_not_implemented"));
    }

    #[test]
    fn gpt_live_transcribe_is_refused_on_a_push_to_talk_session_before_release_4() {
        let res = run(ResolveRequest {
            release: 3,
            session: SessionKind::PushToTalk,
            ..req("openai", "gpt-live-transcribe")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("client_not_implemented"));
    }

    #[test]
    fn gpt_live_transcribe_goes_through_the_gateway_driven_commit_from_release_4() {
        let res = run(ResolveRequest {
            release: 4,
            ..req("openai", "gpt-live-transcribe")
        });
        assert_eq!(res.outcome, Outcome::Commit);
        assert_eq!(adapter(&res), Some("openai_realtime_transcription"));
    }

    #[test]
    fn cartesia_moves_to_manual_finalize_in_release_4() {
        let res = run(ResolveRequest {
            release: 4,
            ..req("cartesia", "ink-whisper")
        });
        assert_eq!(res.outcome, Outcome::Commit);
        assert_eq!(adapter(&res), Some("cartesia_manual_finalize"));
    }

    #[test]
    fn cartesia_streams_natively_before_release_4() {
        let res = run(ResolveRequest {
            release: 3,
            ..req("cartesia", "ink-whisper")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_eq!(adapter(&res), Some("native"));
    }

    #[test]
    fn assemblyai_universal_2_streams_a_substituted_model_in_release_2() {
        let res = run(ResolveRequest {
            release: 2,
            ..req("assemblyai", "universal-2")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_model_substituted"]);
    }

    #[test]
    fn assemblyai_universal_2_is_refused_as_asynchronous_only_from_release_3() {
        let res = run(ResolveRequest {
            release: 3,
            ..req("assemblyai", "universal-2")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("async_only"));
    }

    #[test]
    fn a_retired_model_that_todays_client_silently_replaces_is_warned_before_release_3() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("baidu", "19362")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_model_substituted"]);
    }

    #[test]
    fn a_retired_model_that_todays_client_silently_replaces_is_refused_from_release_3() {
        let res = run(ResolveRequest {
            release: 3,
            ..req("baidu", "19362")
        });
        assert_eq!(code(&res), Some("stt_model_retired"));
    }

    #[test]
    fn a_yandex_asynchronous_only_model_keeps_todays_timed_uploader_before_release_3() {
        let res = run(req("yandex", "deferred-general"));
        assert_eq!(res.outcome, Outcome::Native);
    }

    #[test]
    fn gladia_solaria_3_is_refused_as_asynchronous_only_from_release_3() {
        let res = run(ResolveRequest {
            release: 3,
            ..req("gladia", "solaria-3")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("async_only"));
    }

    #[test]
    fn an_unknown_openai_id_fails_slow_to_upload_marked_assumed() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("openai", "gpt-9-transcribe")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(res.row_id, "openai:any");
        assert_warns(&res, &["stt_capability_assumed"]);
    }

    #[test]
    fn an_unknown_deepgram_id_stays_native_assumed_as_a_notice() {
        let res = run(ResolveRequest {
            release: 3,
            ..req("deepgram", "nova-9-experimental")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_eq!(res.row_id, "deepgram:nova-any");
    }

    #[test]
    fn an_unknown_provider_passes_through_to_todays_factory() {
        let res = run(ResolveRequest {
            release: 5,
            ..req("acme-speech", "x")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_eq!(res.row_id, "global:any");
        assert_eq!(res.layer, Layer::GlobalDefault);
    }

    #[test]
    fn bhashini_keeps_its_provider_named_prefix() {
        let res = run(ResolveRequest {
            session: SessionKind::PushToTalk,
            ..req("bhashini", "bhashini/iitm/asr-misc--gpu--t4")
        });
        assert_eq!(res.row_id, "bhashini:bhashini-iitm-asr-misc--gpu--t4");
    }

    #[test]
    fn openai_strips_a_leading_openai_prefix() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("openai", "openai/whisper-1")
        });
        assert_eq!(res.row_id, "openai:whisper-1");
        assert_eq!(res.model_sent, "whisper-1");
    }

    #[test]
    fn huawei_takes_the_region_from_the_model_suffix() {
        let res = run(ResolveRequest {
            release: 5,
            ..req("huawei-cloud", "chinese_16k_common@cn-east-3")
        });
        assert_eq!(res.row_id, "huawei-cloud:chinese_16k_common");
    }

    #[test]
    fn the_python_sdk_placeholder_on_elevenlabs_is_treated_as_no_model() {
        let res = run(req("elevenlabs", "nova-3"));
        assert_eq!(res.row_id, "elevenlabs:scribe_v2_realtime");
        assert_warns(&res, &["stt_placeholder_model_ignored"]);
    }

    #[test]
    fn on_an_uncovered_session_todays_client_receives_the_model_string_unchanged() {
        let res = run(ResolveRequest {
            session: SessionKind::PushToTalk,
            ..req("openai", "openai/whisper-1")
        });
        assert_eq!(res.row_id, "openai:whisper-1");
        assert_eq!(res.model_sent, "openai/whisper-1");
        assert_warns(&res, &["stt_buffered_until_commit"]);
    }

    #[test]
    fn an_empty_model_resolves_to_the_providers_declared_default_not_a_guess() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("openai", "")
        });
        assert_eq!(res.row_id, "openai:gpt-transcribe");
        assert_eq!(res.layer, Layer::ModelUnset);
        assert_eq!(res.model_sent, "gpt-transcribe");
        assert_no_warns(&res, &["stt_capability_assumed"]);
    }

    #[test]
    fn an_alias_selects_its_row() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("groq", "turbo")
        });
        assert_eq!(res.row_id, "groq:whisper-large-v3-turbo");
    }

    #[test]
    fn a_provider_alias_with_a_hyphen_resolves() {
        let res = run(req("Azure", "default"));
        assert_eq!(res.provider, "microsoft-azure");
    }

    #[test]
    fn whisper_1_carries_a_deprecation_warning() {
        let res = run(ResolveRequest {
            release: 1,
            ..req("openai", "whisper-1")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_warns(&res, &["stt_model_deprecated"]);
    }

    #[test]
    fn a_retired_model_is_refused_in_every_release() {
        let res = run(ResolveRequest {
            release: 6,
            ..req("groq", "distil-whisper-large-v3-en")
        });
        assert_eq!(code(&res), Some("stt_model_retired"));
    }

    #[test]
    fn openai_file_model_voice_agent_with_automatic_turns_release_0_refused_no_segmented_path_yet()
    {
        let res = run(req("openai", "gpt-transcribe"));
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("client_not_implemented"));
    }

    #[test]
    fn openai_file_model_voice_agent_uncovered_release_1_session_not_covered_yet() {
        let res = run(ResolveRequest {
            release: 1,
            covered: false,
            ..req("openai", "gpt-transcribe")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("not_covered_yet"));
    }

    #[test]
    fn a_covered_session_waiting_for_its_vendors_release_is_not_told_to_ask_the_operator() {
        let res = run(ResolveRequest {
            release: 2,
            ..req("bhashini", "ai4bharat/conformer-hi-gpu--t4")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("client_not_implemented"));
    }

    #[test]
    fn a_plain_ws_session_on_a_buffering_model_keeps_todays_client_even_when_covered_b4() {
        let res = run(ResolveRequest {
            release: 3,
            session: SessionKind::Plain,
            ..req("openai", "whisper-1")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_buffered_until_commit"]);
    }

    #[test]
    fn a_plain_ws_session_that_asks_for_segmented_gets_the_engine_b4() {
        let res = run(ResolveRequest {
            release: 2,
            session: SessionKind::Plain,
            mode: TranscriptionMode::Segmented,
            ..req("openai", "whisper-1")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
    }

    #[test]
    fn a_manual_mode_agent_or_conversation_loop_on_a_buffering_model_gets_the_engine_when_covered_b4()
     {
        let res = run(ResolveRequest {
            release: 1,
            session: SessionKind::PushToTalk,
            ..req("groq", "whisper-large-v3")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
    }

    #[test]
    fn the_low_latency_tier_reaches_gpt_transcribe_on_the_socket_from_release_4_b9() {
        let res = run(ResolveRequest {
            release: 4,
            latency_tier: LatencyTier::LowLatency,
            ..req("openai", "gpt-transcribe")
        });
        assert_eq!(res.outcome, Outcome::Commit);
        assert_eq!(adapter(&res), Some("openai_realtime_transcription"));
    }

    #[test]
    fn the_standard_tier_keeps_gpt_transcribe_on_file_upload_in_release_4() {
        let res = run(ResolveRequest {
            release: 4,
            ..req("openai", "gpt-transcribe")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
    }

    #[test]
    fn openai_file_model_push_to_talk_release_0_todays_client_with_a_warning() {
        let res = run(ResolveRequest {
            session: SessionKind::PushToTalk,
            ..req("openai", "gpt-transcribe")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_buffered_until_commit"]);
    }

    #[test]
    fn groq_uncovered_release_2_session_push_to_talk_todays_client_with_a_warning() {
        let res = run(ResolveRequest {
            release: 2,
            session: SessionKind::PushToTalk,
            covered: false,
            ..req("groq", "whisper-large-v3")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_buffered_until_commit"]);
    }

    #[test]
    fn self_hosted_before_release_1_keeps_todays_codes_on_a_voice_agent_leg() {
        let res = run(ResolveRequest {
            bud_leg: true,
            ..req("self_hosted", "whisper-large-v3")
        });
        assert_eq!(code(&res), Some("stt_not_streaming"));
    }

    #[test]
    fn self_hosted_before_release_1_keeps_todays_codes_on_a_named_deployment_leg() {
        let res = run(ResolveRequest {
            session: SessionKind::PushToTalk,
            bud_leg: true,
            ..req("self_hosted", "whisper-large-v3")
        });
        assert_eq!(code(&res), Some("unsupported_deployment"));
    }

    #[test]
    fn yandex_keeps_the_blind_uploader_until_release_5_reported_as_known_broken() {
        let res = run(ResolveRequest {
            release: 2,
            ..req("yandex", "general")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_client_unverified"]);
    }

    #[test]
    fn streaming_only_preference_on_a_file_only_model_is_refused() {
        let res = run(ResolveRequest {
            release: 2,
            mode: TranscriptionMode::Streaming,
            ..req("openai", "whisper-1")
        });
        assert_eq!(code(&res), Some("stt_not_streaming"));
    }

    #[test]
    fn segmented_preference_on_a_streaming_model_takes_the_file_transport_when_it_exists() {
        let res = run(ResolveRequest {
            release: 3,
            mode: TranscriptionMode::Segmented,
            ..req("deepgram", "nova-3")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("deepgram_prerecorded"));
    }

    #[test]
    fn segmented_preference_on_a_model_with_no_file_transport_streams_with_a_warning() {
        let res = run(ResolveRequest {
            release: 2,
            mode: TranscriptionMode::Segmented,
            ..req("elevenlabs", "scribe_v2_realtime")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_mode_unavailable"]);
    }

    #[test]
    fn amazon_batch_only_language_keeps_todays_path_before_release_3() {
        let res = run(ResolveRequest {
            release: 2,
            language: Some("cy-GB"),
            ..req("aws-transcribe", "standard")
        });
        assert_eq!(res.outcome, Outcome::Native);
    }

    #[test]
    fn amazon_batch_only_language_is_refused_from_release_3() {
        let res = run(ResolveRequest {
            release: 3,
            language: Some("cy-GB"),
            ..req("aws-transcribe", "standard")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
        assert_eq!(reason(&res), Some("language_not_live"));
    }

    #[test]
    fn deepgram_hosted_whisper_is_not_offered_in_the_eu_region() {
        let res = run(ResolveRequest {
            release: 3,
            region: Some("eu"),
            ..req("deepgram", "whisper-large")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
    }

    #[test]
    fn tencent_flash_is_uploaded_in_release_5_on_the_china_site() {
        let res = run(ResolveRequest {
            release: 5,
            region: Some("china"),
            ..req("tencent", "16k_zh")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("regional_rest"));
    }

    #[test]
    fn tencent_outside_china_keeps_the_known_broken_streaming_client_in_release_5() {
        let res = run(ResolveRequest {
            release: 5,
            region: Some("international"),
            ..req("tencent", "16k_zh")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_client_unverified"]);
    }

    #[test]
    fn speechmatics_melia_1_needs_evidence_the_map_does_not_carry_yet() {
        let res = run(ResolveRequest {
            release: 5,
            evidence: Evidence::Map,
            ..req("speechmatics", "melia-1")
        });
        assert_eq!(code(&res), Some("stt_live_unsupported"));
    }

    #[test]
    fn a_regional_vendor_without_a_recorded_live_probe_stays_on_todays_path_in_release_5_map_evidence()
     {
        let res = run(ResolveRequest {
            release: 5,
            session: SessionKind::PushToTalk,
            evidence: Evidence::Map,
            ..req("fpt-ai", "general")
        });
        assert_eq!(res.outcome, Outcome::Native);
        assert_warns(&res, &["stt_buffered_until_commit"]);
    }

    #[test]
    fn reveries_model_field_is_never_shown() {
        let res = run(req("reverie", "my-app-id-123"));
        assert_eq!(
            res.model_input,
            "<not shown: this provider's model field is not a model>"
        );
        assert_eq!(res.row_id, "reverie:any");
    }

    #[test]
    fn a_compressed_audio_format_cannot_be_segmented() {
        let res = run(ResolveRequest {
            release: 1,
            encoding: Some("opus"),
            ..req("openai", "gpt-transcribe")
        });
        assert_eq!(code(&res), Some("stt_segmentation_unavailable"));
        assert_eq!(reason(&res), Some("audio_format"));
    }

    #[test]
    fn a_declared_socket_profile_that_no_release_builds_falls_back_to_its_file_profile() {
        let res = run(ResolveRequest {
            release: 2,
            bud_leg: true,
            profile: Some("vllm-realtime"),
            ..req("self_hosted", "mistralai/Voxtral-Mini-4B-Realtime-2602")
        });
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_warns(&res, &["stt_transport_fallback"]);
    }

    // Parity with the reference resolver's generated answers.

    fn expected_path(name: &str) -> String {
        format!("{}/tests/expected/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    fn release_table(release: u8) -> Value {
        let text =
            std::fs::read_to_string(expected_path(&format!("release-{release}.json"))).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    /// The model string the reference tables use for each row.
    fn sample_models() -> HashMap<String, (String, String)> {
        release_table(0)["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let s = |k: &str| e[k].as_str().unwrap().to_owned();
                (s("row_id"), (s("provider"), s("model")))
            })
            .collect()
    }

    fn session_named(s: &str) -> SessionKind {
        match s {
            "gateway" => SessionKind::Gateway,
            "push_to_talk" => SessionKind::PushToTalk,
            "plain" => SessionKind::Plain,
            other => panic!("unknown session kind {other}"),
        }
    }

    fn mode_named(s: &str) -> TranscriptionMode {
        match s {
            "auto" => TranscriptionMode::Auto,
            "streaming" => TranscriptionMode::Streaming,
            "segmented" => TranscriptionMode::Segmented,
            other => panic!("unknown mode {other}"),
        }
    }

    #[test]
    fn the_shipped_map_resolves_as_the_release_table_says() {
        let mut checked = 0;
        for release in 0..=6u8 {
            let table = release_table(release);
            assert_eq!(table["release"], release);
            assert_eq!(table["map_version"], map().map_version());
            for e in table["entries"].as_array().unwrap() {
                let s = |k: &str| e[k].as_str();
                let provider = s("provider").unwrap();
                let res = run(ResolveRequest {
                    release,
                    session: session_named(s("session").unwrap()),
                    bud_leg: BUD_LEG_PROVIDERS.contains(&provider),
                    ..ResolveRequest::new(
                        if provider == "*" {
                            "unlisted-provider"
                        } else {
                            provider
                        },
                        s("model").unwrap(),
                    )
                });
                let warnings: Vec<&str> = codes(&res).into_iter().collect();
                let want_warnings: Vec<&str> = e["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| w.as_str().unwrap())
                    .collect();
                let at = format!("release {release}, {e}");
                assert_eq!(res.row_id, s("row_id").unwrap(), "{at}");
                assert_eq!(res.outcome.as_str(), s("outcome").unwrap(), "{at}");
                assert_eq!(adapter(&res), s("adapter"), "{at}");
                assert_eq!(code(&res), s("refusal_code"), "{at}");
                assert_eq!(reason(&res), s("refusal_reason"), "{at}");
                assert_eq!(warnings, want_warnings, "{at}");
                checked += 1;
            }
        }
        assert_eq!(checked, 7 * 3 * map().rows().len());
    }

    /// A resolution in the shape `tests/gen_parity.py` writes.
    fn reference_view(res: &Resolution, fields: &[Value]) -> Value {
        let field = |name: &str| match name {
            "outcome" => json!(res.outcome.as_str()),
            "provider" => json!(res.provider),
            "row_id" => json!(res.row_id),
            "layer" => json!(res.layer.as_str()),
            "model_sent" => json!(res.model_sent),
            "model_input" => json!(res.model_input),
            "label" => json!(res.label),
            "covered" => json!(res.covered),
            "mode" => json!(res.mode.as_str()),
            "adapter" => json!(res.transport.as_ref().map(|t| &t.adapter)),
            "transport_index" => json!(res.transport.as_ref().map(|t| t.index)),
            "transport_profile" => json!(res.transport.as_ref().and_then(|t| t.profile.as_ref())),
            "refusal" => res.refusal.as_ref().map_or(Value::Null, |r| {
                json!([r.code, r.reason, r.text, r.details])
            }),
            "warnings" => res
                .warnings
                .iter()
                .map(|w| json!([w.code, w.delivery.as_str(), w.detail]))
                .collect(),
            "notes" => json!(res.notes),
            other => panic!("unknown output field {other}"),
        };
        fields.iter().map(|f| field(f.as_str().unwrap())).collect()
    }

    #[test]
    fn the_rust_resolver_reproduces_the_reference_grid() {
        let text = std::fs::read_to_string(expected_path("grid.jsonl")).unwrap();
        let mut lines = text.lines();
        let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["map_version"], map().map_version());
        assert_eq!(header["map_revision"], map().map_revision());
        let today = header["today"].as_str().unwrap();
        let in_fields: Vec<&str> = header["in"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap())
            .collect();
        let out_fields = header["out"].as_array().unwrap();
        let mut failures = Vec::new();
        let mut checked = 0;
        for (n, line) in lines.enumerate() {
            let case: Value = serde_json::from_str(line).unwrap();
            let input = |name: &str| &case[0][in_fields.iter().position(|f| *f == name).unwrap()];
            let text = |name: &str| input(name).as_str();
            let request = ResolveRequest {
                provider: text("provider").unwrap(),
                model: text("model").unwrap(),
                release: input("release").as_u64().unwrap() as u8,
                session: session_named(text("session").unwrap()),
                mode: mode_named(text("mode").unwrap()),
                covered: input("covered").as_bool().unwrap(),
                language: text("language"),
                region: text("region"),
                bud_leg: input("bud_leg").as_bool().unwrap(),
                underlying_model: text("underlying_model"),
                profile: text("profile"),
                encoding: text("encoding"),
                evidence: if text("evidence") == Some("map") {
                    Evidence::Map
                } else {
                    Evidence::Assume
                },
                deadline_ms: input("deadline_ms").as_u64().unwrap() as u32,
                today,
                latency_tier: if text("latency_tier") == Some("low_latency") {
                    LatencyTier::LowLatency
                } else {
                    LatencyTier::Standard
                },
                adapter_built: None,
            };
            let got = reference_view(&resolve(map(), &request), out_fields);
            if got != case[1] {
                let diff: Vec<String> = out_fields
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| got[*i] != case[1][*i])
                    .map(|(i, f)| format!("{f}: rust {} python {}", got[i], case[1][i]))
                    .collect();
                failures.push(format!(
                    "line {}: {}\n    {}",
                    n + 2,
                    case[0],
                    diff.join("\n    ")
                ));
            }
            checked += 1;
        }
        assert!(checked > 12_000, "the grid has {checked} resolutions");
        assert!(
            failures.is_empty(),
            "{} of {checked} resolutions differ from resolve.py; first ones:\n{}",
            failures.len(),
            failures
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn release_0_refuses_only_what_the_map_justifies() {
        let samples = sample_models();
        let mut refused = 0;
        for row in map().rows().iter().filter(|r| r.r#match.provider != "*") {
            let (provider, model) = &samples[&row.id];
            let today_kind = row.when_unusable.as_ref().map(|w| w.today);
            for session in [
                SessionKind::Gateway,
                SessionKind::PushToTalk,
                SessionKind::Plain,
            ] {
                let res = run(ResolveRequest {
                    session,
                    covered: false,
                    bud_leg: BUD_LEG_PROVIDERS.contains(&provider.as_str()),
                    ..req("", "")
                }
                .with(provider, model));
                if res.outcome != Outcome::Refused {
                    continue;
                }
                refused += 1;
                let justified = matches!(
                    today_kind,
                    Some(TodayBehaviour::Fails | TodayBehaviour::RefusedAtSetup)
                ) || (today_kind == Some(TodayBehaviour::BuffersUntilHangup)
                    && session == SessionKind::Gateway)
                    || (row.lifecycle.status == LifecycleStatus::Retired
                        && row.lifecycle.shutdown_on.as_deref().unwrap_or("0000") <= TODAY);
                assert!(
                    justified,
                    "{} on a {session} session is refused in Release 0: {:?}",
                    row.id, res.refusal
                );
            }
        }
        assert_eq!(
            refused,
            9 + 168 + 72 + 32,
            "resolve.py --check-release-0 counts 281 refusals"
        );
    }

    impl<'a> ResolveRequest<'a> {
        fn with(self, provider: &'a str, model: &'a str) -> Self {
            ResolveRequest {
                provider,
                model,
                ..self
            }
        }
    }

    // The adapter_built extension.

    #[test]
    fn an_unbuilt_file_adapter_keeps_todays_client_or_the_rows_refusal() {
        let not_openai = |a: &str| a != "openai_transcriptions";
        for covered in [true, false] {
            let base = ResolveRequest {
                release: 1,
                covered,
                adapter_built: Some(&not_openai),
                ..req("openai", "gpt-transcribe")
            };
            let agent = run(base);
            assert_eq!(code(&agent), Some("stt_live_unsupported"));
            assert_eq!(
                reason(&agent),
                Some("client_not_implemented"),
                "never not_covered_yet: covering would not help"
            );
            assert_eq!(
                agent.refusal.as_ref().unwrap().text,
                "This model transcribes uploaded files and has no streaming interface. On this gateway it is not yet \
                 served to a voice agent that decides when the caller has finished speaking. Use a streaming \
                 transcription model or set the agent's turn detection to manual. Segmented speech-to-text for this \
                 provider is not available in this release."
            );
            for session in [SessionKind::PushToTalk, SessionKind::Plain] {
                let res = run(ResolveRequest { session, ..base });
                assert_eq!(res.outcome, Outcome::Native);
                assert_eq!(res.label, "today's buffering client");
                assert_eq!(res.warnings.len(), 1);
                assert_eq!(res.warnings[0].code, "stt_buffered_until_commit");
                assert_eq!(res.warnings[0].delivery, Delivery::Frame);
                assert_eq!(res.warnings[0].detail_str("model"), Some("gpt-transcribe"));
            }
        }
        let built = run(ResolveRequest {
            release: 1,
            ..req("openai", "gpt-transcribe")
        });
        assert_eq!(built.outcome, Outcome::Segmented);
    }

    #[test]
    fn an_unbuilt_filter_never_touches_todays_client() {
        let nothing = |_: &str| false;
        for release in 0..=6 {
            let res = run(ResolveRequest {
                release,
                adapter_built: Some(&nothing),
                ..req("deepgram", "nova-3")
            });
            assert_eq!(res.outcome, Outcome::Native);
            assert_eq!(adapter(&res), Some("native"));
        }
    }

    #[test]
    fn an_unbuilt_adapter_resolves_exactly_like_a_transport_of_a_later_release() {
        let samples = sample_models();
        let adapters: BTreeSet<&str> = map()
            .rows()
            .iter()
            .flat_map(|r| map().transports(r))
            .filter(|t| t.transport.enabled_from_release.is_some())
            .map(|t| t.transport.adapter.as_str())
            .filter(|a| adapter_kind(a) != AdapterKind::Native)
            .collect();
        assert!(adapters.len() >= 10, "{adapters:?}");
        let profiles: Vec<Option<&str>> = std::iter::once(None)
            .chain(map().profiles().map(|(name, _)| Some(name)))
            .collect();
        let profile_adapters: BTreeSet<&str> = map()
            .profiles()
            .map(|(_, p)| p.transport.adapter.as_str())
            .collect();
        let variants = [
            (TranscriptionMode::Auto, LatencyTier::Standard),
            (TranscriptionMode::Auto, LatencyTier::LowLatency),
            (TranscriptionMode::Segmented, LatencyTier::Standard),
            (TranscriptionMode::Streaming, LatencyTier::Standard),
        ];
        let mut compared = 0;
        for &unbuilt in &adapters {
            let built = |a: &str| a != unbuilt;
            // Release 7 does not exist, so the edited transports are "not released" in every release.
            let mut later = map().clone();
            later.edit_transports(|t| {
                if t.adapter == unbuilt && t.enabled_from_release.is_some() {
                    t.enabled_from_release = Some(7);
                }
            });
            for row in map().rows() {
                let (provider, model) = &samples[&row.id];
                // Deployment profiles are tried on each Bud provider's default row.
                let leg = BUD_LEG_PROVIDERS.contains(&provider.as_str())
                    && row.r#match.model_glob.as_deref() == Some("*")
                    && profile_adapters.contains(unbuilt);
                if !leg
                    && !map()
                        .transports(row)
                        .iter()
                        .any(|t| t.transport.adapter == unbuilt)
                {
                    continue;
                }
                let provider = if provider == "*" {
                    "unlisted-provider"
                } else {
                    provider.as_str()
                };
                for release in 0..=6u8 {
                    for &profile in if leg { &profiles[..] } else { &profiles[..1] } {
                        for session in [
                            SessionKind::Gateway,
                            SessionKind::PushToTalk,
                            SessionKind::Plain,
                        ] {
                            for covered in [true, false] {
                                for (mode, latency_tier) in variants {
                                    let r = ResolveRequest {
                                        release,
                                        session,
                                        covered,
                                        mode,
                                        latency_tier,
                                        profile,
                                        bud_leg: BUD_LEG_PROVIDERS.contains(&provider),
                                        ..req("", "")
                                    }
                                    .with(provider, model);
                                    let filtered = resolve(
                                        map(),
                                        &ResolveRequest {
                                            adapter_built: Some(&built),
                                            ..r
                                        },
                                    );
                                    assert_eq!(
                                        filtered,
                                        resolve(&later, &r),
                                        "{unbuilt} unbuilt: {r:?}"
                                    );
                                    compared += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(compared > 10_000, "{compared}");
    }

    // Branches the shipped map never reaches.

    #[test]
    fn a_slow_seed_is_reported_from_release_2() {
        let mut slow = map().clone();
        let id = "openai:gpt-4o-mini-transcribe";
        let row = slow.row_mut(id).unwrap();
        for entry in &mut row.transports {
            if let crate::map::TransportEntry::Inline(t) = entry {
                for m in &mut t.latency.measurements {
                    if m.percentile == Percentile::P99 && m.quantity == Quantity::EndOfSpeechToFinal
                    {
                        m.value_ms = 3000;
                    }
                }
            }
        }
        let r = |release| {
            resolve(
                &slow,
                &ResolveRequest {
                    release,
                    ..req("openai", "gpt-4o-mini-transcribe")
                },
            )
        };
        assert!(!codes(&r(1)).contains("stt_latency_slow"));
        let res = r(2);
        let got: Vec<_> = res
            .warnings
            .iter()
            .map(|w| (w.code.as_str(), w.delivery, Value::Object(w.detail.clone())))
            .collect();
        let deprecated = json!({"shutdown_on": "2027-02-26", "past_shutdown": false, "replacement": ["gpt-transcribe", "gpt-live-transcribe"]});
        assert_eq!(
            got,
            [
                ("stt_segmented_mode", Delivery::Frame, json!({})),
                (
                    "stt_latency_slow",
                    Delivery::Frame,
                    json!({"final_latency_slow_ms": 3000, "target_ms": 2500})
                ),
                ("stt_model_deprecated", Delivery::Frame, deprecated),
            ]
        );
    }

    #[test]
    fn a_recorded_live_probe_enables_a_transport_under_map_evidence() {
        let mut probed = map().clone();
        let r = ResolveRequest {
            release: 5,
            session: SessionKind::PushToTalk,
            evidence: Evidence::Map,
            ..req("fpt-ai", "general")
        };
        probed.row_mut("fpt-ai:general").unwrap().provenance = Some(crate::map::Provenance {
            verified_by: Some(VerifiedBy::LiveProbe),
            probe_ref: Some("probe/fpt-1".into()),
            ..Default::default()
        });
        let res = resolve(&probed, &r);
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("regional_rest"));
        assert_eq!(codes(&res), BTreeSet::from(["stt_segmented_mode"]));

        probed
            .row_mut("fpt-ai:general")
            .unwrap()
            .provenance
            .as_mut()
            .unwrap()
            .probe_ref = Some(String::new());
        let res = resolve(&probed, &r);
        assert_eq!(res.outcome, Outcome::Native);
        assert_eq!(res.label, "today's buffering client");
    }

    #[test]
    fn the_full_map_resolves_like_the_routing_map() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/segmented-stt/capability-map/stt_live_capabilities.json"
        );
        let full = CapabilityMap::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        let samples = sample_models();
        for row in map().rows() {
            let (provider, model) = &samples[&row.id];
            for release in [1, 5, 6] {
                for evidence in [Evidence::Assume, Evidence::Map] {
                    let r = ResolveRequest {
                        release,
                        evidence,
                        bud_leg: BUD_LEG_PROVIDERS.contains(&provider.as_str()),
                        ..req("", "")
                    }
                    .with(if provider == "*" { "x" } else { provider }, model);
                    assert_eq!(resolve(&full, &r), resolve(map(), &r), "{r:?}");
                }
            }
        }
    }

    /// The routing map with an edit applied to its JSON, as `tests/gen_parity.py` would see it.
    fn edited_map(edit: impl FnOnce(&mut Value)) -> CapabilityMap {
        let mut doc: Value =
            serde_json::from_str(include_str!("../data/stt_live_routing.json")).unwrap();
        edit(&mut doc);
        CapabilityMap::from_json(&doc.to_string()).unwrap()
    }

    fn bare_row(id: &str, matcher: Value, when_unusable: Value, transports: Value) -> Value {
        json!({
            "id": id,
            "match": matcher,
            "lifecycle": {"status": "ga"},
            "transports": transports,
            "when_unusable": when_unusable,
            "billing": {"unit": "unknown", "min_billed_ms": null, "bills_silence": null},
        })
    }

    fn rows_of(doc: &mut Value) -> &mut Vec<Value> {
        doc["rows"].as_array_mut().unwrap()
    }

    #[test]
    fn a_tie_between_patterns_goes_to_the_longer_literal_then_the_larger_pattern_then_the_first_row()
     {
        let m = edited_map(|doc| {
            for (id, glob) in [
                ("tie-a", "zq-*"),
                ("tie-b", "*-zq"),
                ("tie-c", "zq-*"),
                ("tie-d", "zq-x-*"),
            ] {
                let row = bare_row(
                    &format!("openai:{id}"),
                    json!({"provider": "openai", "model_glob": glob}),
                    json!({"today": "streams"}),
                    json!([]),
                );
                rows_of(doc).push(row);
            }
        });
        for (model, row) in [
            ("zq-x-zq", "openai:tie-d"),
            ("zq-y-zq", "openai:tie-a"),
            ("y-zq", "openai:tie-b"),
            ("ZQ-Y-ZQ", "openai:tie-a"),
        ] {
            let res = resolve(&m, &req("openai", model));
            assert_eq!(
                (res.row_id.as_str(), res.layer),
                (row, Layer::Pattern),
                "{model}"
            );
            assert_eq!(res.label, "today's client");
        }
    }

    #[test]
    fn the_placeholder_rule_yields_to_a_row_for_the_placeholder_itself() {
        let m = edited_map(|doc| {
            let row = bare_row(
                "openai:nova-3",
                json!({"provider": "openai", "model": "nova-3"}),
                json!({"today": "streams"}),
                json!([]),
            );
            rows_of(doc).push(row);
        });
        let res = resolve(
            &m,
            &ResolveRequest {
                release: 1,
                ..req("openai", "nova-3")
            },
        );
        assert_eq!(
            (res.row_id.as_str(), res.layer),
            ("openai:nova-3", Layer::Exact)
        );
        assert!(res.warnings.is_empty());
        let res = resolve(
            &m,
            &ResolveRequest {
                release: 1,
                ..req("groq", "nova-3")
            },
        );
        assert_eq!(
            (res.row_id.as_str(), res.layer),
            ("groq:whisper-large-v3-turbo", Layer::ModelUnset)
        );
        assert_eq!(res.warnings[0].code, "stt_placeholder_model_ignored");
        assert_eq!(
            Value::Object(res.warnings[0].detail.clone()),
            json!({"received": "nova-3", "model": "whisper-large-v3-turbo"})
        );
    }

    #[test]
    fn a_constraint_refusal_is_not_claimed_while_an_unbuilt_transport_waits() {
        let m = edited_map(|doc| {
            let file = doc["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == "openai:gpt-transcribe")
                .unwrap()["transports"][0]
                .clone();
            let mut regional = file.clone();
            regional["adapter"] = json!("groq_transcriptions");
            regional["constraints"] = json!({"regions": {"mode": "only", "values": ["us"]}});
            let refusal = json!({
                "today": "fails",
                "refuse_from_release": null,
                "refusal": {
                    "code": "stt_live_unsupported",
                    "reason": "client_not_implemented",
                    "text": format!("Use a streaming model, or {C1}."),
                },
            });
            let row = bare_row(
                "openai:held-back",
                json!({"provider": "openai", "model": "held-back"}),
                refusal,
                json!([file, regional]),
            );
            rows_of(doc).push(row);
        });
        let r = ResolveRequest {
            release: 1,
            region: Some("eu"),
            ..req("openai", "held-back")
        };
        let res = resolve(&m, &r);
        assert_eq!(res.outcome, Outcome::Segmented);
        assert_eq!(adapter(&res), Some("openai_transcriptions"));
        let not_openai = |a: &str| a != "openai_transcriptions";
        let res = resolve(
            &m,
            &ResolveRequest {
                adapter_built: Some(&not_openai),
                ..r
            },
        );
        let refusal = res.refusal.unwrap();
        assert_eq!(refusal.reason.as_deref(), Some("client_not_implemented"));
        assert_eq!(
            refusal.text,
            format!("Use a streaming model.{NOT_AVAILABLE}")
        );
        assert_eq!(
            res.label,
            "refused stt_live_unsupported (client_not_implemented)"
        );
    }

    #[test]
    fn latency_evidence_counts_a_measurement_equal_to_the_deadline() {
        let m = edited_map(|doc| {
            let row = rows_of(doc)
                .iter_mut()
                .find(|r| r["id"] == "speechmatics:melia-1")
                .unwrap();
            row["transports"][0]["latency"]["measurements"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "percentile": "p99", "quantity": "end_of_speech_to_final", "value_ms": 4000,
                    "basis": "own_probe", "source": "x",
                }));
            row["provenance"] = json!({"verified_by": "live_probe", "probe_ref": "p"});
        });
        let r = |deadline_ms| ResolveRequest {
            release: 5,
            evidence: Evidence::Map,
            deadline_ms,
            ..req("speechmatics", "melia-1")
        };
        let res = resolve(&m, &r(4000));
        assert_eq!(adapter(&res), Some("speechmatics_batch"));
        let got: Vec<_> = res
            .warnings
            .iter()
            .map(|w| (w.code.as_str(), Value::Object(w.detail.clone())))
            .collect();
        assert_eq!(
            got,
            [
                ("stt_segmented_mode", json!({})),
                (
                    "stt_latency_slow",
                    json!({"final_latency_slow_ms": 4000, "target_ms": 2500})
                ),
            ]
        );
        let res = resolve(&m, &r(3999));
        assert_eq!(reason(&res), Some("client_not_implemented"));
        // Addendum B10: a covered session waiting for its vendor is never sent to the operator.
        let text = res.refusal.unwrap().text;
        assert!(!text.contains("operator"), "{text}");
        assert!(text.ends_with(
            "Segmented speech-to-text for this provider is not available in this release."
        ));
    }

    #[test]
    fn an_explicit_region_wins_over_a_model_suffix() {
        let r = |region| ResolveRequest {
            release: 5,
            region,
            ..req("huawei-cloud", "chinese_16k_common@cn-east-3")
        };
        let res = run(r(None));
        assert_eq!(
            (res.outcome, adapter(&res)),
            (Outcome::Segmented, Some("regional_rest"))
        );
        assert_eq!(res.model_sent, "chinese_16k_common@cn-east-3");
        assert!(res.notes.contains(&"Region suffix 'cn-east-3' taken from the model string; the string sent is unchanged.".to_owned()));
        let res = run(r(Some("eu")));
        assert_eq!(
            (res.outcome, adapter(&res)),
            (Outcome::Native, Some("native"))
        );
        assert_eq!(res.label, "native stream (client known broken)");
        let res = run(r(Some("cn-north-4")));
        assert_eq!(adapter(&res), Some("regional_rest"));
    }

    // Pieces.

    #[test]
    fn the_operator_clause_is_removed_as_resolve_py_removes_it() {
        let cases: Vec<(String, String)> = vec![
            (format!("A, B, or {C1}."), format!("A or B.{NOT_AVAILABLE}")),
            (format!("A, or {C2}."), format!("A.{NOT_AVAILABLE}")),
            (format!("A or {C1}."), format!("A.{NOT_AVAILABLE}")),
            (
                format!("A, {C1}, or C."),
                format!("A, or C.{NOT_AVAILABLE}"),
            ),
            (
                "No clause here, or there.".into(),
                "No clause here, or there.".into(),
            ),
            (C1.into(), C1.into()),
            (format!("A, B, or {C1}X"), format!("A, B, or {C1}X")),
            (
                format!("Use é, ü, or {C1}."),
                format!("Use é or ü.{NOT_AVAILABLE}"),
            ),
            (
                format!("First, second, third, or {C1}."),
                format!("First, second or third.{NOT_AVAILABLE}"),
            ),
            (format!("A,, or {C1}."), format!("A,.{NOT_AVAILABLE}")),
            (
                format!("A, B\nC, or {C2}."),
                format!("A or B\nC.{NOT_AVAILABLE}"),
            ),
            (format!("X, {C1}, {C2}, Y"), format!("X, Y{NOT_AVAILABLE}")),
            (
                format!("One, or {C1}. Two, three, or {C2}. Four {C1}, five."),
                format!("One. Two or three. Four five.{NOT_AVAILABLE}"),
            ),
            (format!("x. y, or {C1}."), format!("x. y.{NOT_AVAILABLE}")),
            (format!("A, B,, or {C1}."), format!("A, B,.{NOT_AVAILABLE}")),
            (String::new(), String::new()),
            (
                format!("a, b, or {C1}. c, d, or {C1}."),
                format!("a or b. c or d.{NOT_AVAILABLE}"),
            ),
            (format!(",  , or {C1}."), format!(" or  .{NOT_AVAILABLE}")),
            (format!("A, , or {C1}."), format!("A, .{NOT_AVAILABLE}")),
            (format!(", , or {C1}."), format!(", .{NOT_AVAILABLE}")),
            (format!("A,  or {C1}."), format!("A, .{NOT_AVAILABLE}")),
        ];
        for (input, want) in cases {
            assert_eq!(without_operator_clause(&input), want, "{input:?}");
        }
    }

    #[test]
    fn globs_are_anchored_case_folded_and_stop_at_line_breaks() {
        for (pattern, text, want) in [
            ("nova-*", "nova-3", true),
            ("nova-*", "xnova-3", false),
            ("*-en", "distil-en", true),
            ("*-en", "distil-en-us", false),
            ("a*b*c", "abc", true),
            ("a*b*c", "axxbyyc", true),
            ("a*b*c", "acb", false),
            ("ab*ba", "aba", false),
            ("ab*ba", "abba", true),
            ("x*y", "xy", true),
            ("a*", "a\n", false),
            ("a*c", "a\nc", false),
            ("a\nb*", "a\nbc", true),
            ("*", "", true),
            ("阿*云", "阿里云", true),
            ("gpt-*-transcribe", "gpt-4o-mini-transcribe", true),
            ("gpt-*-transcribe", "gpt-transcribe", false),
        ] {
            assert_eq!(
                glob_match(pattern, text),
                want,
                "{pattern:?} against {text:?}"
            );
        }
    }

    #[test]
    fn adapters_and_encodings_are_classified_like_the_reference() {
        assert_eq!(adapter_kind("native"), AdapterKind::Native);
        assert_eq!(adapter_kind("regional_rest"), AdapterKind::Segmented);
        assert_eq!(adapter_kind("planned_file"), AdapterKind::Segmented);
        assert_eq!(
            adapter_kind("cartesia_manual_finalize"),
            AdapterKind::Commit
        );
        assert_eq!(adapter_kind("planned_commit"), AdapterKind::Commit);
        assert_eq!(adapter_kind("planned_stream"), AdapterKind::StreamNotBuilt);
        assert_eq!(adapter_kind("something_new"), AdapterKind::StreamNotBuilt);
        assert!(is_undecodable_encoding("opus") && is_undecodable_encoding("OGG_OPUS"));
        assert!(!is_undecodable_encoding("pcm_s16le") && !is_undecodable_encoding("mulaw"));
    }

    #[test]
    fn a_new_request_has_the_reference_defaults() {
        let r = ResolveRequest::new("openai", "gpt-transcribe");
        assert_eq!(
            (r.release, r.session, r.mode),
            (0, SessionKind::Gateway, TranscriptionMode::Auto)
        );
        assert!(r.covered && !r.bud_leg && r.adapter_built.is_none());
        assert_eq!(
            (r.evidence, r.latency_tier, r.deadline_ms),
            (Evidence::Assume, LatencyTier::Standard, 6000)
        );
        assert_eq!(
            (
                r.language,
                r.region,
                r.encoding,
                r.profile,
                r.underlying_model
            ),
            (None, None, None, None, None)
        );
        assert_eq!(r.today, utc_today());
        let res = resolve(map(), &r);
        assert!(!res.covered, "Release 0 covers nothing");
        assert_eq!(res.row(map()).id, "openai:gpt-transcribe");
        let res = run(ResolveRequest {
            release: 9,
            ..req("openai", "gpt-transcribe")
        });
        assert_eq!(res.release, 6);
        assert!(format!("{r:?}").contains("adapter_built: None"));
    }

    #[test]
    fn release_0_ignores_a_preference() {
        let res = run(ResolveRequest {
            mode: TranscriptionMode::Streaming,
            ..req("deepgram", "nova-3")
        });
        assert_eq!(res.mode, TranscriptionMode::Auto);
        assert_eq!(
            res.notes[0],
            "Release 0 has no transcription_mode; the preference is ignored."
        );
    }

    #[test]
    fn dates_are_utc_calendar_dates() {
        for (days, date) in [
            (0, "1970-01-01"),
            (11_016, "2000-02-29"),
            (20_731, "2026-10-05"),
            (47_541, "2100-03-01"),
        ] {
            assert_eq!(civil_date(days), date);
        }
        let today = utc_today();
        assert_eq!(today.len(), 10);
        assert!(today.as_bytes()[4] == b'-' && today.as_bytes()[7] == b'-');
        assert!(std::ptr::eq(today, utc_today()), "one string per day");
    }

    #[test]
    fn spellings_name_the_reference_values() {
        assert_eq!(SessionKind::PushToTalk.as_str(), "push_to_talk");
        assert_eq!(Layer::DeploymentOverride.as_str(), "deployment_override");
        assert_eq!(Delivery::Notice.to_string(), "notice");
        assert_eq!(Outcome::Refused.as_str(), "refused");
        assert_eq!(LatencyTier::LowLatency.as_str(), "low_latency");
        assert_eq!(Evidence::Map.as_str(), "map");
        assert_eq!(AdapterKind::StreamNotBuilt.as_str(), "stream_not_built");
    }
}
