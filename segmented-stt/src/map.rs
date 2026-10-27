//! The capability map: one row per provider and model saying how a live call reaches it.
//!
//! The gateway embeds the routing map (`data/stt_live_routing.json`), which `assemble.py --routing`
//! writes from the sources in `docs/segmented-stt/capability-map/`: the full map without the
//! reviewer prose. Its shape is `stt_capability_map.schema.json` (schema version 2). Field names
//! here are the JSON names; closed vocabularies are enums, so a value the schema does not allow is
//! a load error rather than a silent mismatch. Unknown keys are ignored, so the full map with its
//! prose loads too.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::OnceLock;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::resolve::{AdapterKind, adapter_kind};
use crate::rollout::BUILT_RELEASE;

/// The schema version this crate reads.
pub const SCHEMA_VERSION: u32 = 2;

const EMBEDDED_JSON: &str = include_str!("../data/stt_live_routing.json");

/// Why a map document cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("capability map is not valid JSON for schema version 2: {0}")]
    Json(String),
    #[error(
        "capability map has schema version {found}; this gateway reads version {SCHEMA_VERSION}"
    )]
    SchemaVersion { found: u32 },
    #[error("capability map is inconsistent: {0}")]
    Invalid(String),
}

macro_rules! vocabulary {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            /// The spelling used in the map.
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

vocabulary!(
    /// How the provider's model string is used.
    ModelString {
        WireModel = "wire_model",
        GatewayAlias = "gateway_alias",
        DeploymentName = "deployment_name",
        Absent = "absent",
        Sensitive = "sensitive",
    }
);
vocabulary!(
    /// What today's client does with a model id it does not know.
    NativeModelHandling {
        Verbatim = "verbatim",
        Substituted = "substituted",
        Ignored = "ignored",
    }
);
vocabulary!(DeploymentModelSource {
    SettingsThenTable = "settings_then_table",
    TableOnly = "table_only",
});
vocabulary!(Egress {
    PublicOnly = "public_only",
    InCluster = "in_cluster",
});
vocabulary!(DeploymentBase {
    Required = "required",
    Ignored = "ignored",
});
vocabulary!(BaseUrlConvention {
    Origin = "origin",
    Versioned = "versioned",
});
vocabulary!(SetupProbe {
    InClusterAndAssumed = "in_cluster_and_assumed",
    None = "none",
});
vocabulary!(PrerequisiteKind {
    TrustRoot = "trust_root",
    TokenScope = "token_scope",
    WorkspaceId = "workspace_id",
    ConfigurationCall = "configuration_call",
    AccountFeature = "account_feature",
    PrivateAddress = "private_address",
    Other = "other",
});
vocabulary!(AvailabilityStatus {
    Open = "open",
    ClosedToNewCustomers = "closed_to_new_customers",
    EvaluationOnly = "evaluation_only",
    Unknown = "unknown",
});
vocabulary!(LifecycleStatus {
    Ga = "ga",
    Preview = "preview",
    Legacy = "legacy",
    Retiring = "retiring",
    Deprecated = "deprecated",
    Retired = "retired",
    Unknown = "unknown",
    Invalid = "invalid",
});
vocabulary!(DatesBasis {
    Announced = "announced",
    Inferred = "inferred",
});
vocabulary!(
    /// What today's code does with the row when no transport is usable.
    TodayBehaviour {
        Streams = "streams",
        StreamsSubstitutedModel = "streams_substituted_model",
        BuffersUntilHangup = "buffers_until_hangup",
        BlindTimedUploads = "blind_timed_uploads",
        RefusedAtSetup = "refused_at_setup",
        Fails = "fails",
        Unknown = "unknown",
    }
);
vocabulary!(RefusalCode {
    SttLiveUnsupported = "stt_live_unsupported",
    SttModelRetired = "stt_model_retired",
});
vocabulary!(
    /// The reasons a row may store. The resolver adds the ones that depend on the session.
    RowRefusalReason {
        AsyncOnly = "async_only",
        ClientNotImplemented = "client_not_implemented",
        Disabled = "disabled",
        ProviderNotBuilt = "provider_not_built",
        ModelNotServed = "model_not_served",
    }
);
vocabulary!(InputMode {
    LiveStream = "live_stream",
    VendorSegmented = "vendor_segmented",
    FileUpload = "file_upload",
});
vocabulary!(EnableRequirement {
    LiveProbe = "live_probe",
    LatencyWithinDeadline = "latency_within_deadline",
});
vocabulary!(InterimResults {
    Live = "live",
    PostCommit = "post_commit",
    None = "none",
    Unknown = "unknown",
});
vocabulary!(EndpointingOwner {
    Gateway = "gateway",
    Vendor = "vendor",
    Either = "either",
});
vocabulary!(VendorTurnSignal {
    None = "none",
    Silence = "silence",
    Semantic = "semantic",
    Unknown = "unknown",
});
vocabulary!(ConstraintMode {
    Only = "only",
    Except = "except",
});
vocabulary!(LatencyClass {
    Realtime = "realtime",
    Fast = "fast",
    Slow = "slow",
    Unknown = "unknown",
});
vocabulary!(Percentile {
    P50 = "p50",
    P90 = "p90",
    P95 = "p95",
    P99 = "p99",
    Mean = "mean",
    Typical = "typical",
    SingleSample = "single_sample",
});
vocabulary!(Quantity {
    EndOfSpeechToFinal = "end_of_speech_to_final",
    RequestRoundTrip = "request_round_trip",
    InferenceOnly = "inference_only",
});
vocabulary!(MeasurementBasis {
    OwnProbe = "own_probe",
    ThirdPartyBenchmark = "third_party_benchmark",
    VendorClaim = "vendor_claim",
});
vocabulary!(RateMetric {
    Requests = "requests",
    ConcurrentRequests = "concurrent_requests",
    ConcurrentSessions = "concurrent_sessions",
    AudioSeconds = "audio_seconds",
    Tokens = "tokens",
});
vocabulary!(RatePer {
    Second = "second",
    Minute = "minute",
    Hour = "hour",
    Day = "day",
    Month = "month",
    None = "none",
});
vocabulary!(RateScope {
    Credential = "credential",
    Account = "account",
    Organisation = "organisation",
    Project = "project",
    Application = "application",
    Resource = "resource",
    Deployment = "deployment",
    Region = "region",
    Unknown = "unknown",
});
vocabulary!(UploadFormat {
    Wav = "wav",
    RawPcm = "raw_pcm",
    Flac = "flac",
    Mp3 = "mp3",
    Mp4 = "mp4",
    Mpeg = "mpeg",
    Mpga = "mpga",
    M4a = "m4a",
    Ogg = "ogg",
    Opus = "opus",
    Webm = "webm",
    Aac = "aac",
    Aiff = "aiff",
});
vocabulary!(UploadContainer {
    Wav = "wav",
    RawPcm = "raw_pcm",
    Flac = "flac",
    Ogg = "ogg",
    Mp3 = "mp3",
});
vocabulary!(UploadEncoding {
    PcmS16le = "pcm_s16le",
    Flac = "flac",
    Opus = "opus",
    Mp3 = "mp3",
});
vocabulary!(Envelope {
    Multipart = "multipart",
    RawBody = "raw_body",
    JsonBase64 = "json_base64",
});
vocabulary!(ParamLocation {
    Form = "form",
    Query = "query",
    Json = "json",
    Header = "header",
});
vocabulary!(StreamEncoding {
    PcmS16le = "pcm_s16le",
    Mulaw = "mulaw",
    Alaw = "alaw",
    Opus = "opus",
    Flac = "flac",
});
vocabulary!(AudioGating {
    SpeechOnly = "speech_only",
    Continuous = "continuous",
});
vocabulary!(WarmMethod {
    Get = "GET",
    Head = "HEAD",
    Options = "OPTIONS",
});
vocabulary!(QualitySignal {
    SegmentNoSpeechProb = "segment_no_speech_prob",
    SegmentAvgLogprob = "segment_avg_logprob",
    SegmentCompressionRatio = "segment_compression_ratio",
    TokenLogprobs = "token_logprobs",
    WordLogprob = "word_logprob",
    WordConfidence = "word_confidence",
    UtteranceConfidence = "utterance_confidence",
    LanguageProbability = "language_probability",
    DetectedLanguages = "detected_languages",
    AudioEventTags = "audio_event_tags",
    NoSpeechFlag = "no_speech_flag",
});
vocabulary!(LanguageShape {
    Single = "single",
    List = "list",
});
vocabulary!(LanguageFormat {
    Iso639_1 = "iso639_1",
    Iso639_3 = "iso639_3",
    Bcp47 = "bcp47",
    Vendor = "vendor",
});
vocabulary!(ContextKind {
    Prompt = "prompt",
    Keywords = "keywords",
    Keyterms = "keyterms",
    PreviousText = "previous_text",
    VocabularyId = "vocabulary_id",
});
vocabulary!(Auth {
    Bearer = "bearer",
    BearerOptional = "bearer_optional",
    ApiKeyHeader = "api_key_header",
    Token = "token",
    Vendor = "vendor",
});
vocabulary!(UsageReported {
    Seconds = "seconds",
    Tokens = "tokens",
    None = "none",
});
vocabulary!(GatewayClientStatus {
    VerifiedLive = "verified_live",
    WireTested = "wire_tested",
    Unverified = "unverified",
    KnownBroken = "known_broken",
    NotImplemented = "not_implemented",
});
vocabulary!(FileRequestMode {
    Sync = "sync",
    SyncWait = "sync_wait",
    AsyncPoll = "async_poll",
});
vocabulary!(UploadPolicy {
    PerPause = "per_pause",
    PerTurn = "per_turn",
});
vocabulary!(BillingUnit {
    AudioSecond = "audio_second",
    AudioMinute = "audio_minute",
    AudioHour = "audio_hour",
    AudioToken = "audio_token",
    Request = "request",
    None = "none",
    Unknown = "unknown",
});
vocabulary!(RetentionKind {
    PerRequest = "per_request",
    AccountSetting = "account_setting",
    NotApplicable = "not_applicable",
    Unknown = "unknown",
});
vocabulary!(ProfileRequirement {
    UnderlyingModel = "underlying_model",
    RealtimeUrl = "realtime_url",
});
vocabulary!(Confidence {
    Probed = "probed",
    Documented = "documented",
    Assumed = "assumed",
});
vocabulary!(VerifiedBy {
    LiveProbe = "live_probe",
    Docs = "docs",
    CodeReading = "code_reading",
    VendorStatement = "vendor_statement",
    None = "none",
});

/// Upper bounds (ms, inclusive) on a seed 99th percentile for each latency class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyClasses {
    pub realtime_max_ms: u32,
    pub fast_max_ms: u32,
}

/// Provider-level facts the resolver needs before it can look a model up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    #[serde(default)]
    pub aliases: Vec<String>,
    /// The model a session that names none gets; `None` sends it to the provider default row.
    #[serde(default)]
    pub default_model: Option<String>,
    pub model_string: ModelString,
    #[serde(default)]
    pub key_normalisation: KeyNormalisation,
    #[serde(default)]
    pub registered_in_gateway: bool,
    #[serde(default = "verbatim")]
    pub native_model_handling: NativeModelHandling,
    #[serde(default)]
    pub deployment_model_source: Option<DeploymentModelSource>,
    #[serde(default)]
    pub egress: Option<Egress>,
    #[serde(default)]
    pub deployment_base: Option<DeploymentBase>,
    #[serde(default)]
    pub production_base: Option<String>,
    #[serde(default)]
    pub base_url_convention: Option<BaseUrlConvention>,
    /// Base address per canonical region.
    #[serde(default)]
    pub regions: BTreeMap<String, String>,
    #[serde(default)]
    pub setup_probe: Option<SetupProbe>,
    #[serde(default)]
    pub prerequisites: Vec<Prerequisite>,
    #[serde(default)]
    pub availability: Option<Availability>,
}

fn verbatim() -> NativeModelHandling {
    NativeModelHandling::Verbatim
}

/// Exceptions to the plain lookup rule; each changes the lookup and the string sent together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyNormalisation {
    #[serde(default)]
    pub strip_provider_prefix: bool,
    #[serde(default)]
    pub placeholder_as_unset: bool,
    #[serde(default)]
    pub region_suffix_separator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prerequisite {
    pub kind: PrerequisiteKind,
    pub detail: String,
    #[serde(default)]
    pub wire: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub status: AvailabilityStatus,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// A named transport a row may reference and a deployment override may select.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub transport: Transport,
    #[serde(default)]
    pub allowed_providers: Vec<String>,
    /// The file profile used when this profile's client is not built.
    #[serde(default)]
    pub fallback_profile: Option<String>,
    /// Deployment override fields that must be present for the profile to apply.
    #[serde(default)]
    pub requires: Vec<ProfileRequirement>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
}

/// One model or model pattern of one provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub r#match: RowMatch,
    #[serde(default)]
    pub passthrough: bool,
    /// A default row whose facts hold for every model, so its capability is not "assumed".
    #[serde(default)]
    pub applies_to_all_models: bool,
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub transports: Vec<TransportEntry>,
    #[serde(default)]
    pub when_unusable: Option<WhenUnusable>,
    pub billing: Billing,
    /// Absent from the routing map, which strips reviewer prose; present in the full map.
    #[serde(default)]
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowMatch {
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_glob: Option<String>,
    #[serde(default)]
    pub model_aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub status: LifecycleStatus,
    #[serde(default)]
    pub deprecated_on: Option<String>,
    #[serde(default)]
    pub shutdown_on: Option<String>,
    #[serde(default)]
    pub dates_basis: Option<DatesBasis>,
    /// `None` when the row names no replacement, which a warning reports as `null`.
    #[serde(default)]
    pub replacement: Option<Vec<String>>,
}

/// What a session gets when no transport of the row is usable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhenUnusable {
    pub today: TodayBehaviour,
    #[serde(default)]
    pub substituted_model: Option<String>,
    #[serde(default)]
    pub refuse_from_release: Option<u8>,
    #[serde(default)]
    pub refusal: Option<RowRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowRefusal {
    pub code: RefusalCode,
    #[serde(default)]
    pub reason: Option<RowRefusalReason>,
    pub text: String,
}

/// An entry of a row's transports: inline, or a reference to a named profile used whole.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TransportEntry {
    Profile { profile: String },
    Inline(Box<Transport>),
}

impl<'de> Deserialize<'de> for TransportEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value.get("profile") {
            Some(name) => match name.as_str() {
                Some(name) => Ok(TransportEntry::Profile {
                    profile: name.to_owned(),
                }),
                None => Err(de::Error::custom(
                    "a transport's 'profile' must be a profile name",
                )),
            },
            None => Transport::deserialize(value)
                .map(|t| TransportEntry::Inline(Box::new(t)))
                .map_err(de::Error::custom),
        }
    }
}

/// One way the gateway can reach a model on a live call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transport {
    pub input_mode: InputMode,
    /// The code path; [`adapter_kind`] classifies it.
    pub adapter: String,
    /// `None`: no release enables this transport.
    #[serde(default)]
    pub enabled_from_release: Option<u8>,
    #[serde(default)]
    pub enable_requires: Vec<EnableRequirement>,
    pub interim_results: InterimResults,
    pub endpointing_owner: EndpointingOwner,
    pub vendor_turn_signal: VendorTurnSignal,
    #[serde(default)]
    pub constraints: Option<Constraints>,
    #[serde(default)]
    pub latency: Latency,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub upload: Option<Upload>,
    #[serde(default)]
    pub stream_audio: Option<StreamAudio>,
    #[serde(default)]
    pub commit: Option<Commit>,
    #[serde(default)]
    pub warm: Option<Warm>,
    #[serde(default)]
    pub quality_signals: Vec<QualitySignal>,
    #[serde(default)]
    pub quality_signals_require: Vec<String>,
    #[serde(default)]
    pub dialect: Dialect,
    #[serde(default)]
    pub gateway_client: Option<GatewayClient>,
    #[serde(default)]
    pub file_request_mode: Option<FileRequestMode>,
    #[serde(default)]
    pub segment_profile: Option<SegmentProfile>,
    /// Overrides the row's billing for this transport.
    #[serde(default)]
    pub billing: Option<Billing>,
    #[serde(default)]
    pub privacy: Option<Privacy>,
}

/// Languages and regions a transport serves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraints {
    #[serde(default)]
    pub languages: Option<LanguageConstraint>,
    #[serde(default)]
    pub regions: Option<RegionConstraint>,
}

impl Constraints {
    pub fn is_empty(&self) -> bool {
        self.languages.is_none() && self.regions.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageConstraint {
    pub mode: ConstraintMode,
    pub codes: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionConstraint {
    pub mode: ConstraintMode,
    pub values: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Latency {
    pub class: LatencyClass,
    #[serde(default)]
    pub measurements: Vec<LatencyMeasurement>,
}

impl Default for Latency {
    fn default() -> Self {
        Latency {
            class: LatencyClass::Unknown,
            measurements: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyMeasurement {
    pub percentile: Percentile,
    pub quantity: Quantity,
    pub value_ms: u64,
    pub basis: MeasurementBasis,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub measured_on: Option<String>,
    #[serde(default)]
    pub pause_ms_assumed: Option<u32>,
    #[serde(default)]
    pub sample_count: Option<u32>,
}

/// Vendor limits. `None` means the vendor publishes none, or the row does not record one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default)]
    pub max_upload_bytes: Option<u64>,
    #[serde(default)]
    pub max_audio_ms: Option<u64>,
    #[serde(default)]
    pub min_audio_ms: Option<u64>,
    #[serde(default)]
    pub vendor_timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_session_ms: Option<u64>,
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub context_window_ms: Option<u64>,
    #[serde(default)]
    pub rates: Vec<RateLimit>,
    /// The vendor plan whose rates apply; rates of other plans are for reference.
    #[serde(default)]
    pub assumed_plan: Option<String>,
    #[serde(default)]
    pub ramp: bool,
    #[serde(default)]
    pub single_process: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    pub metric: RateMetric,
    pub per: RatePer,
    pub value: u64,
    #[serde(default)]
    pub plan: Option<String>,
    pub scope: RateScope,
    #[serde(default)]
    pub note: Option<String>,
}

/// How a file transport sends audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upload {
    pub accepted: Vec<UploadFormat>,
    pub preferred: PreferredAudio,
    #[serde(default)]
    pub sample_rates_hz: Vec<u32>,
    #[serde(default)]
    pub prefer_call_rate: bool,
    pub envelope: Envelope,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub headers: Vec<UploadHeader>,
    #[serde(default)]
    pub vendor_params: Vec<VendorParam>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreferredAudio {
    pub container: UploadContainer,
    pub encoding: UploadEncoding,
    pub sample_rate_hz: u32,
    pub channels: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorParam {
    pub name: String,
    pub value: String,
    pub r#in: ParamLocation,
}

/// The audio a socket transport requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamAudio {
    pub encodings: Vec<StreamEncoding>,
    pub sample_rates_hz: Vec<u32>,
    #[serde(default)]
    pub channels: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub audio_gating: AudioGating,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warm {
    pub method: WarmMethod,
    pub path: String,
}

/// The exact request dialect of a transport.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dialect {
    /// The language field's wire name; `None` when the vendor takes no language.
    #[serde(default)]
    pub language_param: Option<String>,
    #[serde(default)]
    pub language_shape: Option<LanguageShape>,
    #[serde(default)]
    pub language_format: Option<LanguageFormat>,
    #[serde(default)]
    pub language_required: bool,
    #[serde(default)]
    pub context_params: Vec<ContextParam>,
    #[serde(default)]
    pub response_formats: Vec<String>,
    /// The vendor interface; the regional adapter picks its code path by it.
    #[serde(default)]
    pub wire: Option<String>,
    #[serde(default)]
    pub auth: Option<Auth>,
    #[serde(default)]
    pub path: Option<String>,
    /// Request fields that may be dropped, and the request retried, when the vendor refuses them.
    #[serde(default)]
    pub droppable_params: Vec<String>,
    #[serde(default)]
    pub usage_reported: Option<UsageReported>,
    #[serde(default)]
    pub limit_headers: Option<LimitHeaders>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextParam {
    pub kind: ContextKind,
    pub wire_name: String,
    #[serde(default)]
    pub max_items: Option<u32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_chars: Option<u32>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Names of the vendor's rate-limit response headers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitHeaders {
    #[serde(default)]
    pub limit_requests: Option<String>,
    #[serde(default)]
    pub remaining_requests: Option<String>,
    #[serde(default)]
    pub reset_requests: Option<String>,
    #[serde(default)]
    pub retry_after: Option<String>,
    #[serde(default)]
    pub concurrency_current: Option<String>,
    #[serde(default)]
    pub concurrency_limit: Option<String>,
}

/// The honest state of the gateway's code for a transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayClient {
    pub status: GatewayClientStatus,
    #[serde(default)]
    pub last_verified_on: Option<String>,
}

/// Per-transport segmenting values; anything absent takes the engine's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentProfile {
    #[serde(default)]
    pub pre_roll_ms: Option<u32>,
    #[serde(default)]
    pub trailing_silence_ms: Option<u32>,
    #[serde(default)]
    pub max_segment_ms: Option<u32>,
    #[serde(default)]
    pub max_in_flight: Option<u32>,
    #[serde(default)]
    pub request_timeout_ms: Option<u32>,
    #[serde(default)]
    pub upload_policy: Option<UploadPolicy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Billing {
    pub unit: BillingUnit,
    #[serde(default)]
    pub min_billed_ms: Option<u64>,
    #[serde(default)]
    pub increment_ms: Option<u64>,
    #[serde(default)]
    pub conditional_minimums: Vec<ConditionalMinimum>,
    #[serde(default)]
    pub surcharges: Vec<Surcharge>,
    #[serde(default)]
    pub bills_silence: Option<bool>,
    #[serde(default)]
    pub bills_failed_requests: Option<bool>,
    #[serde(default)]
    pub pricing_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalMinimum {
    pub feature: String,
    #[serde(default)]
    pub above: Option<u64>,
    pub min_billed_ms: u64,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surcharge {
    pub feature: String,
    pub percent: f64,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Privacy {
    pub retention: Retention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retention {
    pub kind: RetentionKind,
    #[serde(default)]
    pub param: Option<RetentionParam>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionParam {
    pub name: String,
    pub value: String,
    pub r#in: ParamLocation,
}

/// Where a row's facts came from. Only `verified_by` and `probe_ref` steer resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    #[serde(default)]
    pub confidence: Option<Confidence>,
    #[serde(default)]
    pub verified_on: Option<String>,
    #[serde(default)]
    pub verified_by: Option<VerifiedBy>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub probe_ref: Option<String>,
    #[serde(default)]
    pub recheck_on: Option<String>,
    #[serde(default)]
    pub unverified: Vec<String>,
}

/// A row's transport with a profile reference replaced by the profile's transport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedTransport<'m> {
    pub transport: &'m Transport,
    /// The profile the transport came from, if it was a reference.
    pub profile: Option<&'m str>,
}

/// Providers in document order, so that colliding spellings resolve to the later provider as in
/// the reference resolver.
#[derive(Debug, Clone, Default)]
struct OrderedProviders(Vec<(String, Provider)>);

impl<'de> Deserialize<'de> for OrderedProviders {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedVisitor;
        impl<'de> Visitor<'de> for OrderedVisitor {
            type Value = OrderedProviders;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of provider entries")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some((id, entry)) = access.next_entry::<String, Provider>()? {
                    out.push((id, entry));
                }
                Ok(OrderedProviders(out))
            }
        }
        deserializer.deserialize_map(OrderedVisitor)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Document {
    schema_version: u32,
    map_version: String,
    #[serde(default)]
    map_revision: u64,
    #[serde(default)]
    release: u8,
    latency_classes: LatencyClasses,
    #[serde(default)]
    sdk_placeholder_models: Vec<String>,
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
    providers: OrderedProviders,
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct VersionProbe {
    schema_version: u32,
}

/// A row's match keys, lowercased once at load.
#[derive(Debug, Clone)]
pub(crate) struct RowKeys {
    pub(crate) model: Option<String>,
    pub(crate) aliases: Vec<String>,
    pub(crate) glob: Option<String>,
    /// Characters of the glob as written, `*` excepted: the more, the more specific.
    pub(crate) glob_literal_len: usize,
}

/// The loaded map with its lookup indexes.
#[derive(Debug, Clone)]
pub struct CapabilityMap {
    schema_version: u32,
    map_version: String,
    map_revision: u64,
    release: u8,
    latency_classes: LatencyClasses,
    sdk_placeholder_models: Vec<String>,
    profiles: BTreeMap<String, Profile>,
    providers: Vec<(String, Provider)>,
    rows: Vec<Row>,
    provider_index: HashMap<String, usize>,
    names: HashMap<String, usize>,
    flat_names: HashMap<String, usize>,
    placeholders: HashSet<String>,
    rows_by_provider: HashMap<String, Vec<usize>>,
    row_index: HashMap<String, usize>,
    row_keys: Vec<RowKeys>,
    global_row: usize,
}

impl CapabilityMap {
    /// Parses and checks a map document (schema version 2).
    pub fn from_json(json: &str) -> Result<CapabilityMap, MapError> {
        let probe: VersionProbe =
            serde_json::from_str(json).map_err(|e| MapError::Json(e.to_string()))?;
        if probe.schema_version != SCHEMA_VERSION {
            return Err(MapError::SchemaVersion {
                found: probe.schema_version,
            });
        }
        let doc: Document =
            serde_json::from_str(json).map_err(|e| MapError::Json(locate(json, e)))?;
        Self::index(doc)
    }

    /// The routing map built into this binary, parsed on first use. A unit test proves it valid.
    pub fn embedded() -> &'static CapabilityMap {
        static MAP: OnceLock<CapabilityMap> = OnceLock::new();
        MAP.get_or_init(|| match CapabilityMap::from_json(EMBEDDED_JSON) {
            Ok(map) => map,
            Err(e) => panic!("the embedded speech-to-text routing map (data/stt_live_routing.json) is invalid: {e}"),
        })
    }

    fn index(doc: Document) -> Result<CapabilityMap, MapError> {
        let providers = doc.providers.0;
        let mut provider_index = HashMap::new();
        let mut names = HashMap::new();
        let mut flat_names = HashMap::new();
        for (i, (id, entry)) in providers.iter().enumerate() {
            provider_index.insert(id.clone(), i);
            for spelling in std::iter::once(id).chain(&entry.aliases) {
                let key = py_strip(spelling).to_lowercase();
                flat_names.insert(key.replace('-', "_"), i);
                names.insert(key, i);
            }
        }
        let mut rows_by_provider: HashMap<String, Vec<usize>> = HashMap::new();
        let mut row_index = HashMap::new();
        let mut row_keys = Vec::with_capacity(doc.rows.len());
        for (i, row) in doc.rows.iter().enumerate() {
            if row_index.insert(row.id.clone(), i).is_some() {
                return invalid(format!("row id '{}' appears twice", row.id));
            }
            let provider = &row.r#match.provider;
            if provider != "*" && !provider_index.contains_key(provider) {
                return invalid(format!(
                    "row '{}' names provider '{provider}', which has no entry",
                    row.id
                ));
            }
            rows_by_provider
                .entry(provider.clone())
                .or_default()
                .push(i);
            let glob = row.r#match.model_glob.as_deref();
            row_keys.push(RowKeys {
                model: row.r#match.model.as_deref().map(str::to_lowercase),
                aliases: row
                    .r#match
                    .model_aliases
                    .iter()
                    .map(|a| a.to_lowercase())
                    .collect(),
                glob: glob.map(str::to_lowercase),
                glob_literal_len: glob.map_or(0, |g| g.chars().filter(|c| *c != '*').count()),
            });
        }
        let Some(&global_row) = rows_by_provider.get("*").and_then(|rows| rows.first()) else {
            return invalid("no global default row (provider '*')".to_owned());
        };
        for (id, _) in &providers {
            let has_default = rows_by_provider.get(id).is_some_and(|rows| {
                rows.iter()
                    .any(|&r| doc.rows[r].r#match.model_glob.as_deref() == Some("*"))
            });
            if !has_default {
                return invalid(format!(
                    "provider '{id}' has no default row (model_glob '*')"
                ));
            }
        }
        for (name, profile) in &doc.profiles {
            check_transport(&format!("profile '{name}'"), &profile.transport)?;
            if let Some(fallback) = &profile.fallback_profile
                && !doc.profiles.contains_key(fallback)
            {
                return invalid(format!(
                    "profile '{name}' falls back to unknown profile '{fallback}'"
                ));
            }
        }
        for row in &doc.rows {
            for (k, entry) in row.transports.iter().enumerate() {
                match entry {
                    TransportEntry::Profile { profile } if !doc.profiles.contains_key(profile) => {
                        return invalid(format!(
                            "row '{}' transport {k} references unknown profile '{profile}'",
                            row.id
                        ));
                    }
                    TransportEntry::Profile { .. } => {}
                    TransportEntry::Inline(t) => {
                        check_transport(&format!("row '{}' transport {k}", row.id), t)?
                    }
                }
            }
            if let Some(wu) = &row.when_unusable
                && wu.today == TodayBehaviour::StreamsSubstitutedModel
                && wu.substituted_model.is_none()
            {
                return invalid(format!(
                    "row '{}' streams a substituted model but does not name it",
                    row.id
                ));
            }
        }
        Ok(CapabilityMap {
            schema_version: doc.schema_version,
            map_version: doc.map_version,
            map_revision: doc.map_revision,
            release: doc.release,
            latency_classes: doc.latency_classes,
            placeholders: doc
                .sdk_placeholder_models
                .iter()
                .map(|m| m.to_lowercase())
                .collect(),
            sdk_placeholder_models: doc.sdk_placeholder_models,
            profiles: doc.profiles,
            providers,
            rows: doc.rows,
            provider_index,
            names,
            flat_names,
            rows_by_provider,
            row_index,
            row_keys,
            global_row,
        })
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn map_version(&self) -> &str {
        &self.map_version
    }

    pub fn map_revision(&self) -> u64 {
        self.map_revision
    }

    /// The release this copy of the map was assembled for. Informational: the release in force
    /// comes from the rollout settings.
    pub fn release(&self) -> u8 {
        self.release
    }

    pub fn latency_classes(&self) -> LatencyClasses {
        self.latency_classes
    }

    /// Model strings SDKs send when the customer named none (the Python kit sends `nova-3`).
    pub fn sdk_placeholder_models(&self) -> &[String] {
        &self.sdk_placeholder_models
    }

    pub(crate) fn is_placeholder(&self, lowercased: &str) -> bool {
        self.placeholders.contains(lowercased)
    }

    /// The canonical provider id for a spelling: trimmed, without case, by id or alias, with `-`
    /// and `_` equivalent.
    pub fn provider_id(&self, raw: &str) -> Option<&str> {
        let key = py_strip(raw).to_lowercase();
        let i = self
            .names
            .get(&key)
            .or_else(|| self.flat_names.get(&key.replace('-', "_")))?;
        Some(self.providers[*i].0.as_str())
    }

    /// The provider entry for a canonical id.
    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.provider_index.get(id).map(|&i| &self.providers[i].1)
    }

    /// Canonical provider ids and entries, in map order.
    pub fn providers(&self) -> impl Iterator<Item = (&str, &Provider)> {
        self.providers.iter().map(|(id, p)| (id.as_str(), p))
    }

    /// Every row, in map order.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The rows of a canonical provider id (`*` for the global default), in map order.
    pub fn rows_for(&self, provider: &str) -> impl Iterator<Item = &Row> {
        self.row_indexes_for(provider)
            .iter()
            .map(|&i| &self.rows[i])
    }

    pub(crate) fn row_indexes_for(&self, provider: &str) -> &[usize] {
        self.rows_by_provider
            .get(provider)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn row_at(&self, i: usize) -> &Row {
        &self.rows[i]
    }

    pub(crate) fn row_keys(&self, i: usize) -> &RowKeys {
        &self.row_keys[i]
    }

    pub fn row(&self, id: &str) -> Option<&Row> {
        self.row_index.get(id).map(|&i| &self.rows[i])
    }

    /// The row an unknown provider falls to.
    pub fn global_row(&self) -> &Row {
        &self.rows[self.global_row]
    }

    pub(crate) fn global_row_index(&self) -> usize {
        self.global_row
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    pub fn profiles(&self) -> impl Iterator<Item = (&str, &Profile)> {
        self.profiles.iter().map(|(name, p)| (name.as_str(), p))
    }

    /// The row's transports in order, profile references replaced by the profile's transport.
    pub fn transports<'m>(&'m self, row: &'m Row) -> Vec<ResolvedTransport<'m>> {
        row.transports
            .iter()
            .map(|entry| match entry {
                TransportEntry::Inline(t) => ResolvedTransport {
                    transport: t,
                    profile: None,
                },
                // `from_json` refuses a map with a dangling reference, so this finds the profile
                // for any row of this map.
                TransportEntry::Profile { profile } => {
                    self.profile_transport(profile).unwrap_or_else(|| {
                        panic!("row '{}' references unknown profile '{profile}'", row.id)
                    })
                }
            })
            .collect()
    }

    /// A profile's transport, remembering the profile name.
    pub fn profile_transport(&self, name: &str) -> Option<ResolvedTransport<'_>> {
        self.profiles
            .get_key_value(name)
            .map(|(name, p)| ResolvedTransport {
                transport: &p.transport,
                profile: Some(name.as_str()),
            })
    }

    /// Applies `edit` to every inline and profile transport. Match keys are untouched, so the
    /// indexes stay valid.
    #[cfg(test)]
    pub(crate) fn edit_transports(&mut self, mut edit: impl FnMut(&mut Transport)) {
        for row in &mut self.rows {
            for entry in &mut row.transports {
                if let TransportEntry::Inline(t) = entry {
                    edit(t);
                }
            }
        }
        for profile in self.profiles.values_mut() {
            edit(&mut profile.transport);
        }
    }

    #[cfg(test)]
    pub(crate) fn row_mut(&mut self, id: &str) -> Option<&mut Row> {
        self.row_index.get(id).map(|&i| &mut self.rows[i])
    }
}

fn invalid<T>(problem: String) -> Result<T, MapError> {
    Err(MapError::Invalid(problem))
}

/// A transport whose adapter no release builds (`planned_*`, or an id this crate does not know) is
/// never enabled (the schema's rule): no code path could serve it.
fn check_transport(place: &str, t: &Transport) -> Result<(), MapError> {
    let unbuildable = t.adapter.starts_with("planned_")
        || adapter_kind(&t.adapter) == AdapterKind::StreamNotBuilt;
    if unbuildable && t.enabled_from_release.is_some() {
        return invalid(format!(
            "{place}: adapter '{}' is not built in any release, so enabled_from_release must be null",
            t.adapter
        ));
    }
    if t.enabled_from_release.is_some_and(|r| r > BUILT_RELEASE) {
        return invalid(format!(
            "{place}: enabled_from_release is not a release (0 to {BUILT_RELEASE})"
        ));
    }
    Ok(())
}

/// Names the entry a parse error falls in: the routing map is one line, so a column alone does not
/// find it.
fn locate(json: &str, err: serde_json::Error) -> String {
    let message = err.to_string();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return message;
    };
    if let Some(rows) = value.get("rows").and_then(|r| r.as_array()) {
        for row in rows {
            if let Err(e) = serde_json::from_value::<Row>(row.clone()) {
                let id = row.get("id").and_then(|v| v.as_str()).unwrap_or("<no id>");
                return format!("row '{id}': {e}");
            }
        }
    }
    if let Some(profiles) = value.get("profiles").and_then(|p| p.as_object()) {
        for (name, profile) in profiles {
            if let Err(e) = serde_json::from_value::<Profile>(profile.clone()) {
                return format!("profile '{name}': {e}");
            }
        }
    }
    if let Some(providers) = value.get("providers").and_then(|p| p.as_object()) {
        for (id, entry) in providers {
            if let Err(e) = serde_json::from_value::<Provider>(entry.clone()) {
                return format!("provider '{id}': {e}");
            }
        }
    }
    message
}

/// Python's `str.strip()`: Unicode white space plus the four ASCII separators Python also strips.
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> &'static CapabilityMap {
        CapabilityMap::embedded()
    }

    #[test]
    fn the_embedded_map_parses() {
        let m = map();
        assert_eq!(m.schema_version(), 2);
        assert!(!m.map_version().is_empty());
        assert_eq!(m.rows().len(), 412);
        assert_eq!(m.providers().count(), 34);
        assert_eq!(m.profiles().count(), 12);
        assert_eq!(
            m.latency_classes(),
            LatencyClasses {
                realtime_max_ms: 600,
                fast_max_ms: 1200
            }
        );
        assert_eq!(m.sdk_placeholder_models(), ["nova-3"]);
        assert_eq!(m.global_row().id, "global:any");
    }

    #[test]
    fn every_row_resolves_its_transports() {
        let m = map();
        let mut referenced = 0;
        for row in m.rows() {
            let resolved = m.transports(row);
            assert_eq!(resolved.len(), row.transports.len(), "{}", row.id);
            for (entry, t) in row.transports.iter().zip(&resolved) {
                match entry {
                    TransportEntry::Profile { profile } => {
                        referenced += 1;
                        assert_eq!(t.profile, Some(profile.as_str()));
                        assert_eq!(t.transport, &m.profile(profile).unwrap().transport);
                    }
                    TransportEntry::Inline(inline) => {
                        assert_eq!(t.profile, None);
                        assert_eq!(t.transport, inline.as_ref());
                    }
                }
            }
        }
        assert!(referenced > 0, "some row references a profile");
        for (name, p) in m.profiles() {
            if let Some(fb) = &p.fallback_profile {
                assert!(m.profile(fb).is_some(), "{name} falls back to {fb}");
            }
        }
    }

    #[test]
    fn every_provider_has_a_default_row_and_rows_are_indexed_by_id() {
        let m = map();
        for (id, _) in m.providers() {
            assert!(
                m.rows_for(id)
                    .any(|r| r.r#match.model_glob.as_deref() == Some("*")),
                "{id}"
            );
        }
        for row in m.rows() {
            assert_eq!(m.row(&row.id).map(|r| &r.id), Some(&row.id));
        }
        assert!(m.row("openai:no-such-row").is_none());
        assert_eq!(m.rows_for("nobody").count(), 0);
    }

    #[test]
    fn provider_spellings_resolve_like_the_reference() {
        let m = map();
        for (raw, want) in [
            ("openai", Some("openai")),
            ("  OpenAI ", Some("openai")),
            ("Azure", Some("microsoft-azure")),
            ("microsoft_azure", Some("microsoft-azure")),
            ("MICROSOFT-AZURE", Some("microsoft-azure")),
            ("azure-openai", Some("azure_openai")),
            ("azure_openai", Some("azure_openai")),
            ("self-hosted", Some("self_hosted")),
            ("self_hosted", Some("self_hosted")),
            ("openai_compatible", Some("self_hosted")),
            ("openai-compatible", Some("self_hosted")),
            ("waav_infer", Some("waav-infer")),
            ("infer", Some("waav-infer")),
            ("阿里云", Some("alibaba-cloud")),
            ("\u{1f}huawei\u{1c}", Some("huawei-cloud")),
            ("rev.ai", Some("revai")),
            ("acme-speech", None),
            ("", None),
        ] {
            assert_eq!(m.provider_id(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn typed_fields_carry_the_routing_facts() {
        let m = map();
        let openai = m.provider("openai").unwrap();
        assert_eq!(openai.default_model.as_deref(), Some("gpt-transcribe"));
        assert_eq!(openai.model_string, ModelString::WireModel);
        assert!(
            openai.key_normalisation.strip_provider_prefix
                && openai.key_normalisation.placeholder_as_unset
        );
        assert_eq!(openai.native_model_handling, NativeModelHandling::Verbatim);
        let huawei = m.provider("huawei-cloud").unwrap();
        assert_eq!(
            huawei.key_normalisation.region_suffix_separator.as_deref(),
            Some("@")
        );

        let row = m.row("openai:gpt-transcribe").unwrap();
        let ts = m.transports(row);
        let file = ts[0].transport;
        assert_eq!(file.input_mode, InputMode::FileUpload);
        assert_eq!(file.adapter, "openai_transcriptions");
        assert_eq!(file.enabled_from_release, Some(1));
        assert_eq!(file.file_request_mode, Some(FileRequestMode::Sync));
        let upload = file.upload.as_ref().unwrap();
        assert_eq!(upload.envelope, Envelope::Multipart);
        assert_eq!(upload.preferred.sample_rate_hz, 16000);
        assert_eq!(upload.preferred.encoding, UploadEncoding::PcmS16le);
        assert_eq!(file.dialect.language_param.as_deref(), Some("languages"));
        assert_eq!(file.dialect.language_shape, Some(LanguageShape::List));
        assert_eq!(
            file.dialect.path.as_deref(),
            Some("/v1/audio/transcriptions")
        );
        assert_eq!(file.dialect.context_params[1].kind, ContextKind::Keywords);
        assert_eq!(file.limits.max_upload_bytes, Some(26_214_400));
        assert_eq!(file.limits.assumed_plan.as_deref(), Some("tier_1"));
        assert!(file.limits.ramp);
        assert_eq!(file.limits.rates[0].metric, RateMetric::Requests);
        assert_eq!(
            file.segment_profile.as_ref().unwrap().request_timeout_ms,
            Some(10_000)
        );
        assert_eq!(
            file.gateway_client.as_ref().unwrap().status,
            GatewayClientStatus::NotImplemented
        );
        let socket = ts[1].transport;
        assert_eq!(socket.input_mode, InputMode::VendorSegmented);
        assert_eq!(socket.commit.unwrap().audio_gating, AudioGating::SpeechOnly);
        assert_eq!(
            socket.stream_audio.as_ref().unwrap().sample_rates_hz,
            [24000]
        );
        assert_eq!(socket.billing.as_ref().unwrap().unit, BillingUnit::Unknown);
        let wu = row.when_unusable.as_ref().unwrap();
        assert_eq!(wu.today, TodayBehaviour::BuffersUntilHangup);
        assert_eq!(
            wu.refusal.as_ref().unwrap().reason,
            Some(RowRefusalReason::ClientNotImplemented)
        );

        assert_eq!(
            m.row("groq:whisper-large-v3-turbo")
                .unwrap()
                .billing
                .min_billed_ms,
            Some(10_000)
        );
        let deepgram = m.row("deepgram:whisper-large").unwrap();
        let constraints = m.transports(deepgram)[0]
            .transport
            .constraints
            .clone()
            .unwrap();
        let regions = constraints.regions.unwrap();
        assert_eq!(regions.mode, ConstraintMode::Except);
        assert_eq!(regions.values, ["eu", "au", "in"]);
    }

    #[test]
    fn a_profile_reference_remembers_the_profile_name() {
        let m = map();
        let ts = m.transports(m.row("self_hosted:any").unwrap());
        assert_eq!(ts[0].profile, Some("openai-compatible-file"));
        assert_eq!(ts[0].transport.adapter, "openai_transcriptions");
        let realtime = m.profile("vllm-realtime").unwrap();
        assert_eq!(realtime.fallback_profile.as_deref(), Some("vllm-file"));
        assert_eq!(
            realtime.requires,
            [
                ProfileRequirement::RealtimeUrl,
                ProfileRequirement::UnderlyingModel
            ]
        );
        assert_eq!(realtime.allowed_providers, ["self_hosted"]);
        assert_eq!(
            m.profile_transport("nim-file").unwrap().profile,
            Some("nim-file")
        );
        assert!(m.profile_transport("no-such-profile").is_none());
    }

    #[test]
    fn the_full_map_with_its_prose_loads_and_keeps_provenance() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/segmented-stt/capability-map/stt_live_capabilities.json"
        );
        let full = CapabilityMap::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(full.rows().len(), map().rows().len());
        let probe = full
            .row("nectec:partii5")
            .unwrap()
            .provenance
            .as_ref()
            .unwrap();
        assert_eq!(probe.verified_by, Some(VerifiedBy::LiveProbe));
        assert!(probe.probe_ref.is_some());
        // The routing map keeps only the probe evidence the resolver reads.
        let kept = map()
            .row("nectec:partii5")
            .unwrap()
            .provenance
            .as_ref()
            .unwrap();
        assert_eq!(kept.verified_by, Some(VerifiedBy::LiveProbe));
        assert!(kept.probe_ref.is_some());
        assert!(
            map()
                .rows()
                .iter()
                .filter(|r| r.provenance.is_some())
                .all(|r| r
                    .provenance
                    .as_ref()
                    .is_some_and(|p| p.verified_by.is_some() || p.probe_ref.is_some())),
            "nothing else of provenance is kept"
        );
    }

    fn edited(edit: impl FnOnce(&mut serde_json::Value)) -> Result<CapabilityMap, MapError> {
        let mut doc: serde_json::Value = serde_json::from_str(EMBEDDED_JSON).unwrap();
        edit(&mut doc);
        CapabilityMap::from_json(&doc.to_string())
    }

    #[test]
    fn a_broken_map_is_refused_with_its_reason() {
        let err = edited(|d| d["schema_version"] = 1.into()).unwrap_err();
        assert!(matches!(err, MapError::SchemaVersion { found: 1 }), "{err}");

        let err = edited(|d| d["rows"][0]["transports"] = serde_json::json!([{"profile": "nope"}]))
            .unwrap_err();
        assert!(err.to_string().contains("unknown profile 'nope'"), "{err}");

        let err = edited(|d| d["rows"][3]["lifecycle"]["status"] = "gone".into()).unwrap_err();
        let id = &map().rows()[3].id;
        assert!(err.to_string().contains(&format!("row '{id}'")), "{err}");

        let err = edited(|d| {
            d["rows"]
                .as_array_mut()
                .unwrap()
                .retain(|r| r["match"]["provider"] != "*")
        })
        .unwrap_err();
        assert!(err.to_string().contains("no global default row"), "{err}");

        let err = edited(|d| {
            d["profiles"]["nim-realtime"]["transport"]["enabled_from_release"] = 5.into()
        })
        .unwrap_err();
        assert!(err.to_string().contains("planned_stream"), "{err}");
        let err = edited(|d| {
            d["profiles"]["vllm-realtime"]["transport"]["enabled_from_release"] = 4.into()
        })
        .unwrap_err();
        assert!(err.to_string().contains("planned_commit"), "{err}");
        let err = edited(|d| d["rows"][0]["transports"][0]["enabled_from_release"] = 7.into())
            .unwrap_err();
        assert!(err.to_string().contains("not a release"), "{err}");

        let err = edited(|d| d["profiles"]["vllm-realtime"]["fallback_profile"] = "missing".into())
            .unwrap_err();
        assert!(
            err.to_string().contains("unknown profile 'missing'"),
            "{err}"
        );

        let err = edited(|d| d["rows"][2]["id"] = d["rows"][1]["id"].clone()).unwrap_err();
        assert!(err.to_string().contains("appears twice"), "{err}");

        assert!(matches!(
            CapabilityMap::from_json("{"),
            Err(MapError::Json(_))
        ));
    }

    #[test]
    fn python_strip_matches_str_strip() {
        assert_eq!(py_strip("  a b \t\n"), "a b");
        assert_eq!(py_strip("\u{1c}\u{1d}x\u{1e}\u{1f}"), "x");
        assert_eq!(py_strip("\u{a0}\u{3000}y\u{2028}"), "y");
        assert_eq!(py_strip("\u{200b}z"), "\u{200b}z");
    }
}
