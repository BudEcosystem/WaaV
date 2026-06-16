//! Fleet guardrail: a PARAMETRIZED regression matrix that asserts uniform
//! invariants across EVERY registered realtime (speech-to-speech) provider.
//!
//! Unlike the per-provider integration/unit tests (which pin each provider's
//! bespoke wire), this file iterates the LIVE registry —
//! [`get_supported_realtime_providers()`] — and asserts the SAME contract for
//! every entry. Its job is to catch a FUTURE provider that drifts or is
//! registered incompletely: a new provider that doesn't construct from a valid
//! config, ignores case in its registry name, accepts an empty API key, returns
//! a malformed `get_provider_info()`, or reports `is_ready()` before connecting
//! will fail HERE even if its own per-provider tests are green (or missing).
//!
//! The per-provider config nuances (Azure needs an endpoint + a deployment in
//! `model`; OpenAI parses `model` into an enum) are CENTRALIZED in
//! [`valid_config_for`] so the "creates with a valid config" assertion passes
//! uniformly for all providers.

use waav_gateway::core::realtime::{
    RealtimeConfig, create_realtime_provider, get_supported_realtime_providers,
};

/// The realtime providers this matrix is written to cover, in the registry's
/// own order. The COUNT GUARDRAIL test below pins the registry to EXACTLY this
/// set so adding/removing a provider without updating the matrix fails loudly.
///
/// BUMP THIS (and `valid_config_for`) WHEN YOU ADD A REALTIME PROVIDER.
const EXPECTED_PROVIDERS: [&str; 9] = [
    "openai",
    "hume",
    "azure",
    "grok",
    "inworld",
    "deepgram",
    "elevenlabs",
    "gemini",
    "ultravox",
];

/// Build a [`RealtimeConfig`] that SUCCESSFULLY constructs `name`'s provider.
///
/// This centralizes the per-provider construction nuances so the uniform
/// "creates with a valid config" assertion holds for ALL providers:
///
/// - every provider validates a NON-EMPTY `api_key` FIRST (empty ⇒ `Err`), so
///   the api key is always set here (and the empty-key case is tested by
///   clearing it);
/// - **azure** (`azure/protocol.rs`): `from_config` REQUIRES both `endpoint`
///   (the Azure resource) AND a non-empty `model` (the deployment name) — either
///   missing ⇒ `InvalidConfiguration`. Both are set here;
/// - **openai** (`openai/protocol.rs`): `from_config` parses `model` into the
///   `OpenAIRealtimeModel` enum via `from_str_or_default`, which DEFAULTS an
///   unknown string to `gpt-realtime` rather than erroring — so any model
///   string constructs. We pass the real GA default `"gpt-realtime"` explicitly;
/// - **grok / inworld / deepgram**: take a raw/lenient model string (inworld and
///   deepgram don't even require it at construction — only `connect_spec`/the
///   Settings wire consult it). A descriptive model is supplied anyway;
/// - **hume**: only needs the api key (defaults supply the rest).
///
/// A `voice` is set everywhere it is meaningful; it is harmless where unused.
fn valid_config_for(name: &str) -> RealtimeConfig {
    let base = RealtimeConfig {
        api_key: "test-key".to_string(),
        provider: name.to_string(),
        voice: Some("alloy".to_string()),
        ..Default::default()
    };

    match name {
        // Azure routes by DEPLOYMENT (`model`) on a per-RESOURCE host
        // (`endpoint`); `from_config` errors without BOTH.
        "azure" => RealtimeConfig {
            model: "gpt-realtime-deploy".to_string(),
            endpoint: Some("my-resource".to_string()),
            ..base
        },
        // OpenAI parses `model` into an enum; the GA default is accepted verbatim.
        "openai" => RealtimeConfig {
            model: "gpt-realtime".to_string(),
            ..base
        },
        // xAI routes by the raw model string in the connect query.
        "grok" => RealtimeConfig {
            model: "grok-realtime".to_string(),
            ..base
        },
        // Inworld routes by a minted session (not `model`); a model is harmless.
        "inworld" => RealtimeConfig {
            model: "inworld-realtime".to_string(),
            ..base
        },
        // Deepgram Voice Agent: `model` maps to the `think` LLM (lenient/defaulted);
        // a Deepgram `aura-2-*` speak voice is the natural `voice` here.
        "deepgram" => RealtimeConfig {
            model: "gpt-4o-mini".to_string(),
            voice: Some("aura-2-thalia-en".to_string()),
            ..base
        },
        // ElevenLabs ConvAI: the agent IS the model — `from_config` REQUIRES a
        // non-empty `model` (the pre-created agent_id) or it errors with
        // InvalidConfiguration. A descriptive agent id is supplied.
        "elevenlabs" => RealtimeConfig {
            model: "agent_test123".to_string(),
            ..base
        },
        // Gemini Live: `from_config` only needs a non-empty api key; the model
        // defaults to the current Live model when omitted. A descriptive Live
        // model + a Gemini prebuilt voice are supplied.
        "gemini" => RealtimeConfig {
            model: "gemini-2.0-flash-live-001".to_string(),
            voice: Some("Puck".to_string()),
            ..base
        },
        // Ultravox: `from_config` only needs a non-empty api key; the model
        // defaults to `fixie-ai/ultravox` when omitted. A descriptive model + an
        // Ultravox voice are supplied.
        "ultravox" => RealtimeConfig {
            model: "fixie-ai/ultravox".to_string(),
            voice: Some("Mark".to_string()),
            ..base
        },
        // Hume EVI: only the api key is required (defaults: V3, Linear16, 44.1 kHz).
        // Catch-all so a NEWLY-added provider still gets a sane api-key-only config;
        // if it needs more, the matrix's "creates" assertion fails loudly and points
        // the author back here (and to the COUNT GUARDRAIL).
        _ => base,
    }
}

/// THE FLEET MATRIX. Iterates EVERY provider the registry reports and asserts the
/// uniform construction + identity + readiness contract for each. A provider that
/// drifts on ANY of these fails here with its name in the message.
#[tokio::test]
async fn realtime_provider_matrix_uniform_invariants() {
    let providers = get_supported_realtime_providers();
    assert!(
        !providers.is_empty(),
        "registry reported zero realtime providers"
    );

    for name in providers {
        // 1. Creates by the EXACT registered name with a valid config.
        let created = create_realtime_provider(name, valid_config_for(name));
        assert!(
            created.is_ok(),
            "[{name}] must create from valid_config_for(): {:?}",
            created.err()
        );

        // 2. Creates CASE-INSENSITIVELY (the registry lower-cases the lookup).
        let upper = name.to_uppercase();
        assert!(
            create_realtime_provider(&upper, valid_config_for(name)).is_ok(),
            "[{name}] must create case-insensitively (tried {upper:?})"
        );

        // 3. REJECTS an empty api key — every provider validates the key first
        //    (openai/azure/grok/inworld ⇒ AuthenticationFailed via the embedded
        //    OpenAI protocol, deepgram ⇒ AuthenticationFailed, hume ⇒
        //    InvalidConfiguration). The uniform contract is simply `is_err()`.
        let empty_key = RealtimeConfig {
            api_key: String::new(),
            ..valid_config_for(name)
        };
        assert!(
            create_realtime_provider(name, empty_key).is_err(),
            "[{name}] must reject an empty api_key"
        );

        // 4 & 5 operate on the freshly-created provider.
        let provider = create_realtime_provider(name, valid_config_for(name))
            .unwrap_or_else(|e| panic!("[{name}] create for info/ready checks failed: {e:?}"));

        // 4. get_provider_info() is a JSON OBJECT that names the provider. The
        //    scaffold-driven providers (azure/grok/inworld/deepgram) and the
        //    bespoke ones (openai/hume) all set `"provider": "<id>"`; assert both
        //    that the object carries a `provider` key AND that it equals `name`.
        let info = provider.get_provider_info();
        assert!(
            info.is_object(),
            "[{name}] get_provider_info() must be a JSON object, got: {info}"
        );
        let provider_field = info.get("provider").and_then(|v| v.as_str());
        assert_eq!(
            provider_field,
            Some(name),
            "[{name}] get_provider_info()[\"provider\"] must equal the provider id, got: {info}"
        );

        // 5. A freshly-created (un-connected) provider is NOT ready.
        assert!(
            !provider.is_ready(),
            "[{name}] a freshly-created provider must not be is_ready() before connect()"
        );
    }
}

/// COUNT GUARDRAIL. Pins the registry to EXACTLY the providers the matrix knows
/// about, by sorted-set compare AND by length. Adding or removing a realtime
/// provider without updating [`EXPECTED_PROVIDERS`] (and `valid_config_for`)
/// fails HERE — a deliberate tripwire so the matrix can never silently skip a
/// new provider.
///
/// BUMP `EXPECTED_PROVIDERS` (and add a `valid_config_for` arm if the provider
/// needs more than an api key) WHEN YOU ADD A REALTIME PROVIDER.
#[test]
fn realtime_provider_registry_count_guardrail() {
    let mut actual = get_supported_realtime_providers();
    actual.sort_unstable();

    let mut expected = EXPECTED_PROVIDERS.to_vec();
    expected.sort_unstable();

    assert_eq!(
        actual, expected,
        "the realtime provider registry drifted from the matrix's EXPECTED_PROVIDERS — \
         bump EXPECTED_PROVIDERS (and add a valid_config_for arm) when you add/remove a provider"
    );
    assert_eq!(
        get_supported_realtime_providers().len(),
        9,
        "expected EXACTLY 9 realtime providers; bump this when you add a realtime provider"
    );
}

/// OpenAI-CLONE conformance: azure / grok / inworld each reuse the OpenAI GA wire
/// via an embedded `OpenAiProtocol` (they delegate every wire method to it,
/// overriding only the endpoint/auth and the rare bootstrap event — see each
/// provider's `protocol.rs`). The wire equivalence itself is pinned by those
/// per-provider unit tests (`delegates_ga_wire_verbatim`), which assert the
/// SESSION + audio frames are byte-equivalent to OpenAI's; reaching the embedded
/// `OpenAiProtocol` from here would require private internals. This test asserts
/// the public, behavioral half of that contract: all three OpenAI-clone providers
/// CONSTRUCT through the same factory as OpenAI does.
#[tokio::test]
async fn openai_clone_providers_construct_via_shared_factory() {
    for clone in ["azure", "grok", "inworld"] {
        let created = create_realtime_provider(clone, valid_config_for(clone));
        assert!(
            created.is_ok(),
            "[{clone}] OpenAI-protocol clone must construct via the shared factory: {:?}",
            created.err()
        );
    }
}
