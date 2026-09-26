//! Microsoft Azure Speech Service region configuration.
//!
//! This module provides region configuration for Azure Speech Services,
//! supporting both Speech-to-Text and Text-to-Speech endpoints.
//!
//! # Region Selection
//!
//! Choose the region closest to your users for optimal latency,
//! or use specific regions for data residency requirements.
//!
//! # Example
//!
//! ```rust
//! use waav_gateway::core::providers::azure::AzureRegion;
//!
//! let region = AzureRegion::WestEurope;
//!
//! // STT endpoint
//! assert_eq!(region.stt_hostname(), "westeurope.stt.speech.microsoft.com");
//!
//! // TTS endpoint
//! assert_eq!(region.tts_hostname(), "westeurope.tts.speech.microsoft.com");
//!
//! // Voices list endpoint
//! assert!(region.voices_list_url().contains("cognitiveservices/voices/list"));
//! ```
//!
//! See: <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/regions>

/// Microsoft Azure Speech Service regions.
///
/// Choose the region closest to your users for optimal latency,
/// or use specific regions for data residency requirements.
///
/// See: <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/regions>
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AzureRegion {
    /// East US (Virginia)
    #[default]
    EastUS,
    /// East US 2 (Virginia)
    EastUS2,
    /// West US (California)
    WestUS,
    /// West US 2 (Washington)
    WestUS2,
    /// West US 3 (Arizona)
    WestUS3,
    /// Central US (Iowa)
    CentralUS,
    /// North Central US (Illinois)
    NorthCentralUS,
    /// South Central US (Texas)
    SouthCentralUS,
    /// West Europe (Netherlands)
    WestEurope,
    /// North Europe (Ireland)
    NorthEurope,
    /// UK South (London)
    UKSouth,
    /// France Central (Paris)
    FranceCentral,
    /// Germany West Central (Frankfurt)
    GermanyWestCentral,
    /// Switzerland North (Zurich)
    SwitzerlandNorth,
    /// East Asia (Hong Kong)
    EastAsia,
    /// Southeast Asia (Singapore)
    SoutheastAsia,
    /// Japan East (Tokyo)
    JapanEast,
    /// Japan West (Osaka)
    JapanWest,
    /// Korea Central (Seoul)
    KoreaCentral,
    /// Australia East (Sydney)
    AustraliaEast,
    /// Canada Central (Toronto)
    CanadaCentral,
    /// Brazil South (Sao Paulo)
    BrazilSouth,
    /// India Central (Pune)
    IndiaCentral,
    /// Custom region not explicitly listed.
    ///
    /// Use this for new or less common regions without requiring code changes.
    Custom(String),
}

impl AzureRegion {
    const DEFAULT_SAFE_ENDPOINT_REGION: &'static str = "eastus";

    /// Get the region identifier string used in Azure URLs.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// assert_eq!(AzureRegion::EastUS.as_str(), "eastus");
    /// assert_eq!(AzureRegion::WestEurope.as_str(), "westeurope");
    /// ```
    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            Self::EastUS => "eastus",
            Self::EastUS2 => "eastus2",
            Self::WestUS => "westus",
            Self::WestUS2 => "westus2",
            Self::WestUS3 => "westus3",
            Self::CentralUS => "centralus",
            Self::NorthCentralUS => "northcentralus",
            Self::SouthCentralUS => "southcentralus",
            Self::WestEurope => "westeurope",
            Self::NorthEurope => "northeurope",
            Self::UKSouth => "uksouth",
            Self::FranceCentral => "francecentral",
            Self::GermanyWestCentral => "germanywestcentral",
            Self::SwitzerlandNorth => "switzerlandnorth",
            Self::EastAsia => "eastasia",
            Self::SoutheastAsia => "southeastasia",
            Self::JapanEast => "japaneast",
            Self::JapanWest => "japanwest",
            Self::KoreaCentral => "koreacentral",
            Self::AustraliaEast => "australiaeast",
            Self::CanadaCentral => "canadacentral",
            Self::BrazilSouth => "brazilsouth",
            Self::IndiaCentral => "centralindia",
            Self::Custom(region) => region.as_str(),
        }
    }

    fn endpoint_region_identifier(&self) -> &str {
        let region = self.as_str();
        if is_valid_azure_region_identifier(region) {
            region
        } else {
            Self::DEFAULT_SAFE_ENDPOINT_REGION
        }
    }

    // =========================================================================
    // STT (Speech-to-Text) Endpoints
    // =========================================================================

    /// Get the STT hostname for the Azure Speech Service in this region.
    ///
    /// Format: `<region>.stt.speech.microsoft.com`
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let region = AzureRegion::WestEurope;
    /// assert_eq!(region.stt_hostname(), "westeurope.stt.speech.microsoft.com");
    /// ```
    #[inline]
    pub fn stt_hostname(&self) -> String {
        format!(
            "{}.stt.speech.microsoft.com",
            self.endpoint_region_identifier()
        )
    }

    /// Get the base WebSocket URL for the Azure Speech-to-Text Service in this region.
    ///
    /// Format: `wss://<region>.stt.speech.microsoft.com`
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let region = AzureRegion::SoutheastAsia;
    /// assert_eq!(
    ///     region.stt_websocket_base_url(),
    ///     "wss://southeastasia.stt.speech.microsoft.com"
    /// );
    /// ```
    #[inline]
    pub fn stt_websocket_base_url(&self) -> String {
        format!(
            "wss://{}.stt.speech.microsoft.com",
            self.endpoint_region_identifier()
        )
    }

    // =========================================================================
    // TTS (Text-to-Speech) Endpoints
    // =========================================================================

    /// Get the TTS hostname for the Azure Speech Service in this region.
    ///
    /// Format: `<region>.tts.speech.microsoft.com`
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let region = AzureRegion::WestEurope;
    /// assert_eq!(region.tts_hostname(), "westeurope.tts.speech.microsoft.com");
    /// ```
    #[inline]
    pub fn tts_hostname(&self) -> String {
        format!(
            "{}.tts.speech.microsoft.com",
            self.endpoint_region_identifier()
        )
    }

    /// Get the base REST URL for the Azure Text-to-Speech Service in this region.
    ///
    /// Format: `https://<region>.tts.speech.microsoft.com/cognitiveservices/v1`
    ///
    /// This is the endpoint for synthesizing speech from text.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let region = AzureRegion::EastUS;
    /// assert_eq!(
    ///     region.tts_rest_url(),
    ///     "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1"
    /// );
    /// ```
    #[inline]
    pub fn tts_rest_url(&self) -> String {
        format!(
            "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
            self.endpoint_region_identifier()
        )
    }

    /// Get the voices list endpoint URL for the Azure Text-to-Speech Service.
    ///
    /// Format: `https://<region>.tts.speech.microsoft.com/cognitiveservices/voices/list`
    ///
    /// Use this endpoint to retrieve the list of available voices for the region.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let region = AzureRegion::WestEurope;
    /// assert_eq!(
    ///     region.voices_list_url(),
    ///     "https://westeurope.tts.speech.microsoft.com/cognitiveservices/voices/list"
    /// );
    /// ```
    #[inline]
    pub fn voices_list_url(&self) -> String {
        format!(
            "https://{}.tts.speech.microsoft.com/cognitiveservices/voices/list",
            self.endpoint_region_identifier()
        )
    }

    // =========================================================================
    // Authentication Endpoints
    // =========================================================================

    /// Get the token issuing endpoint URL for obtaining authentication tokens.
    ///
    /// Format: `https://<region>.api.cognitive.microsoft.com/sts/v1.0/issueToken`
    ///
    /// Use this endpoint to exchange your subscription key for an access token.
    /// The access token is valid for 10 minutes.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let region = AzureRegion::EastUS;
    /// assert_eq!(
    ///     region.token_endpoint(),
    ///     "https://eastus.api.cognitive.microsoft.com/sts/v1.0/issueToken"
    /// );
    /// ```
    #[inline]
    pub fn token_endpoint(&self) -> String {
        format!(
            "https://{}.api.cognitive.microsoft.com/sts/v1.0/issueToken",
            self.endpoint_region_identifier()
        )
    }

    // =========================================================================
    // Backwards Compatibility - Deprecated Methods
    // =========================================================================

    /// Get the hostname for the Azure Speech Service in this region.
    ///
    /// **Deprecated**: Use [`stt_hostname`](Self::stt_hostname) instead for clarity.
    ///
    /// Format: `<region>.stt.speech.microsoft.com`
    #[deprecated(since = "0.2.0", note = "Use `stt_hostname` instead for clarity")]
    #[inline]
    pub fn hostname(&self) -> String {
        self.stt_hostname()
    }

    /// Get the base WebSocket URL for the Azure Speech Service in this region.
    ///
    /// **Deprecated**: Use [`stt_websocket_base_url`](Self::stt_websocket_base_url) instead for clarity.
    ///
    /// Format: `wss://<region>.stt.speech.microsoft.com`
    #[deprecated(
        since = "0.2.0",
        note = "Use `stt_websocket_base_url` instead for clarity"
    )]
    #[inline]
    pub fn websocket_base_url(&self) -> String {
        self.stt_websocket_base_url()
    }
}

impl std::str::FromStr for AzureRegion {
    type Err = String;

    /// Parse a region from a string identifier.
    ///
    /// Known region identifiers are matched to their explicit variants.
    /// Unknown safe identifiers are wrapped in the `Custom` variant.
    /// Malformed identifiers are rejected so they cannot alter generated Azure hosts.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let region: AzureRegion = "westeurope".parse().unwrap();
    /// assert_eq!(region, AzureRegion::WestEurope);
    ///
    /// // Unknown regions become Custom
    /// let custom: AzureRegion = "newregion".parse().unwrap();
    /// assert_eq!(custom, AzureRegion::Custom("newregion".to_string()));
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_ascii_lowercase();
        let region = match normalized.as_str() {
            "eastus" => Self::EastUS,
            "eastus2" => Self::EastUS2,
            "westus" => Self::WestUS,
            "westus2" => Self::WestUS2,
            "westus3" => Self::WestUS3,
            "centralus" => Self::CentralUS,
            "northcentralus" => Self::NorthCentralUS,
            "southcentralus" => Self::SouthCentralUS,
            "westeurope" => Self::WestEurope,
            "northeurope" => Self::NorthEurope,
            "uksouth" => Self::UKSouth,
            "francecentral" => Self::FranceCentral,
            "germanywestcentral" => Self::GermanyWestCentral,
            "switzerlandnorth" => Self::SwitzerlandNorth,
            "eastasia" => Self::EastAsia,
            "southeastasia" => Self::SoutheastAsia,
            "japaneast" => Self::JapanEast,
            "japanwest" => Self::JapanWest,
            "koreacentral" => Self::KoreaCentral,
            "australiaeast" => Self::AustraliaEast,
            "canadacentral" => Self::CanadaCentral,
            "brazilsouth" => Self::BrazilSouth,
            "centralindia" => Self::IndiaCentral,
            _ => {
                validate_azure_region_identifier(&normalized)?;
                Self::Custom(normalized)
            }
        };
        Ok(region)
    }
}

// =============================================================================
// Speech endpoint derived from a deployment's `api_base`
// =============================================================================

/// Host suffixes of Azure Speech's REGIONAL endpoints. The single label in front of each is the
/// region (`westeurope.api.cognitive.microsoft.com`, `westeurope.tts.speech.microsoft.com`, …).
const REGIONAL_SPEECH_HOST_SUFFIXES: &[&str] = &[
    ".api.cognitive.microsoft.com",
    ".tts.speech.microsoft.com",
    ".stt.speech.microsoft.com",
    ".voice.speech.microsoft.com",
];

/// Host suffix of a resource's custom-domain endpoint (`<name>.cognitiveservices.azure.com`).
const RESOURCE_SPEECH_HOST_SUFFIX: &str = ".cognitiveservices.azure.com";

/// Host suffixes an AI Services (AI Foundry) resource is ALSO published under. Those hosts serve
/// the OpenAI / Foundry surfaces, not Speech; the same resource serves Speech on its
/// `<name>.cognitiveservices.azure.com` host, so an `api_base` in one of these shapes is
/// rewritten to that host (and the rewrite is reported, see
/// [`AzureSpeechEndpoint::from_api_base_with_note`]).
const AI_SERVICES_ALIAS_HOST_SUFFIXES: &[&str] = &[".openai.azure.com", ".services.ai.azure.com"];

/// The shapes an `api_base` may take, spelled out in every refusal so the operator can fix the
/// credential without reading the source. Deliberately free of anything the caller supplied.
const ACCEPTED_API_BASE_SHAPES: &str = "https://<region>.api.cognitive.microsoft.com, \
     https://<region>.tts.speech.microsoft.com (or .stt. / .voice.), \
     https://<resource>.cognitiveservices.azure.com, or an AI Services resource's \
     https://<resource>.openai.azure.com / https://<resource>.services.ai.azure.com";

/// Where one deployment's Azure Speech requests are sent, derived from its `api_base`.
///
/// Vendor contract: an Azure Speech key belongs to ONE resource, and that resource lives in ONE
/// region. The key is accepted on that region's regional hosts and on the resource's own
/// custom-domain host, and refused (401) everywhere else. So the address has to come from the
/// credential — Bud publishes it as the deployment's `api_base` (the credential's required
/// "API Base URL") — rather than from a gateway-wide default region, which only works for keys
/// that happen to live in that region.
///
/// Sovereign clouds (`*.azure.us`, `*.azure.cn`) are not accepted shapes today; such an
/// `api_base` is refused rather than silently sent to the public cloud.
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::providers::azure::{AzureRegion, AzureSpeechEndpoint};
///
/// let regional = AzureSpeechEndpoint::from_api_base("https://westeurope.api.cognitive.microsoft.com/").unwrap();
/// assert_eq!(regional, AzureSpeechEndpoint::Region(AzureRegion::WestEurope));
///
/// let resource = AzureSpeechEndpoint::from_api_base("https://my-speech.cognitiveservices.azure.com").unwrap();
/// assert_eq!(
///     resource.tts_rest_url(),
///     "https://my-speech.cognitiveservices.azure.com/tts/cognitiveservices/v1"
/// );
///
/// assert!(AzureSpeechEndpoint::from_api_base("https://evil.example.com").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AzureSpeechEndpoint {
    /// A regional endpoint: requests go to `<region>.{tts,stt}.speech.microsoft.com`.
    ///
    /// A region that is not one of [`AzureRegion`]'s named variants arrives as
    /// [`AzureRegion::Custom`] and is used as-is — it is never replaced by the default region.
    Region(AzureRegion),
    /// A resource's custom-domain endpoint. `host` is the lowercase
    /// `<name>.cognitiveservices.azure.com` host with no scheme, port or path.
    ///
    /// Build it through [`AzureSpeechEndpoint::from_api_base`]; the URL builders refuse to put
    /// any other host on the wire (they fall back to the default region instead, the same policy
    /// [`AzureRegion`] applies to an unsafe `Custom` region).
    Resource {
        /// `<name>.cognitiveservices.azure.com`
        host: String,
    },
}

impl AzureSpeechEndpoint {
    /// Resolve a deployment's `api_base` into the endpoint its key is valid on.
    ///
    /// Accepted (case-insensitive; a trailing slash, path, query or the default `:443` port are
    /// tolerated and ignored — only the host matters):
    ///
    /// | `api_base` host | Result |
    /// |---|---|
    /// | `<r>.api.cognitive.microsoft.com` | `Region(r)` |
    /// | `<r>.tts.speech.microsoft.com`, `<r>.stt.…`, `<r>.voice.…` | `Region(r)` |
    /// | `<n>.cognitiveservices.azure.com` | `Resource { host: "<n>.cognitiveservices.azure.com" }` |
    /// | `<n>.openai.azure.com`, `<n>.services.ai.azure.com` | `Resource { host: "<n>.cognitiveservices.azure.com" }` (rewritten) |
    ///
    /// Everything else — an empty value, a non-`https` scheme, an IP address, embedded
    /// credentials, a non-default port, or any host outside the shapes above — is an `Err` whose
    /// message names the field (`api_base`) and the accepted shapes. The message never repeats the
    /// value: a key pasted into the wrong field must not come back out in a 400 or a log line.
    ///
    /// Use [`from_api_base_with_note`](Self::from_api_base_with_note) to learn whether the host
    /// was rewritten.
    pub fn from_api_base(api_base: &str) -> Result<Self, String> {
        Self::from_api_base_with_note(api_base).map(|(endpoint, _)| endpoint)
    }

    /// [`from_api_base`](Self::from_api_base), plus a human-readable note when the `api_base`
    /// was an AI Services alias host (`*.openai.azure.com` / `*.services.ai.azure.com`) that was
    /// rewritten to the resource's `*.cognitiveservices.azure.com` Speech host.
    ///
    /// The note is `None` when the host was used as given. It is safe to log or surface as a
    /// warning: like the error, it does not repeat the caller's value.
    pub fn from_api_base_with_note(api_base: &str) -> Result<(Self, Option<String>), String> {
        let refuse = |reason: &str| {
            format!(
                "api_base is not an Azure Speech endpoint ({reason}); accepted shapes: {ACCEPTED_API_BASE_SHAPES}"
            )
        };

        let trimmed = api_base.trim();
        if trimmed.is_empty() {
            return Err(refuse("it is empty"));
        }
        // `url::ParseError`'s messages are fixed strings; none of them echo the input.
        let url = url::Url::parse(trimmed).map_err(|e| refuse(&format!("not a URL: {e}")))?;
        if url.scheme() != "https" {
            return Err(refuse("the scheme must be https"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(refuse("it must not embed credentials"));
        }
        // `Url` drops a scheme-default port, so `:443` reads as `None` here. Any other port is
        // refused rather than dropped: the endpoint is rebuilt from the host alone.
        if url.port().is_some() {
            return Err(refuse("it must not name a port"));
        }
        let host = match url.host() {
            Some(url::Host::Domain(domain)) => domain.trim_end_matches('.').to_ascii_lowercase(),
            Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => {
                return Err(refuse(
                    "the host must be an Azure DNS name, not an IP address",
                ));
            }
            None => return Err(refuse("it has no host")),
        };

        for suffix in REGIONAL_SPEECH_HOST_SUFFIXES {
            if let Some(label) = single_label_before(&host, suffix) {
                let region = label
                    .parse::<AzureRegion>()
                    .map_err(|_| refuse("the region label is not a valid Azure region"))?;
                return Ok((Self::Region(region), None));
            }
        }

        if let Some(name) = single_label_before(&host, RESOURCE_SPEECH_HOST_SUFFIX) {
            if !is_valid_azure_region_identifier(name) {
                return Err(refuse("the resource name is not a valid DNS label"));
            }
            return Ok((Self::Resource { host: host.clone() }, None));
        }

        for suffix in AI_SERVICES_ALIAS_HOST_SUFFIXES {
            if let Some(name) = single_label_before(&host, suffix) {
                if !is_valid_azure_region_identifier(name) {
                    return Err(refuse("the resource name is not a valid DNS label"));
                }
                let note = format!(
                    "api_base names an AI Services resource by its {} host, which does not serve \
                     Azure Speech; using the same resource's {} host instead",
                    suffix.trim_start_matches('.'),
                    RESOURCE_SPEECH_HOST_SUFFIX.trim_start_matches('.'),
                );
                return Ok((
                    Self::Resource {
                        host: format!("{name}{RESOURCE_SPEECH_HOST_SUFFIX}"),
                    },
                    Some(note),
                ));
            }
        }

        Err(refuse("the host is not an Azure Speech host"))
    }

    /// The region, for a regional endpoint. `None` for a resource endpoint (the region is
    /// implied by the resource and not needed to address it).
    pub fn region(&self) -> Option<&AzureRegion> {
        match self {
            Self::Region(region) => Some(region),
            Self::Resource { .. } => None,
        }
    }

    /// The resource host the URL builders may use, or `None` when a directly-constructed
    /// `Resource` carries a host `from_api_base` would not have produced.
    fn safe_resource_host(host: &str) -> Option<&str> {
        single_label_before(host, RESOURCE_SPEECH_HOST_SUFFIX)
            .filter(|name| is_valid_azure_region_identifier(name))
            .map(|_| host)
    }

    /// The Text-to-Speech REST synthesis URL.
    ///
    /// - `Region(r)` → `https://<r>.tts.speech.microsoft.com/cognitiveservices/v1` (unchanged).
    /// - `Resource` → `https://<name>.cognitiveservices.azure.com/tts/cognitiveservices/v1`.
    ///
    /// Vendor contract (Microsoft's custom-domain rule for Speech REST/WebSocket APIs): a
    /// regional `<region>.tts.speech.microsoft.com/<path>` becomes
    /// `<name>.cognitiveservices.azure.com/tts/<path>` on the resource's own host. This is taken
    /// from the Speech private-endpoint / custom-domain documentation and has NOT yet been
    /// confirmed by a live probe from WaaV — probe it before relying on it for a new tenant.
    pub fn tts_rest_url(&self) -> String {
        match self {
            Self::Region(region) => region.tts_rest_url(),
            Self::Resource { host } => match Self::safe_resource_host(host) {
                Some(host) => format!("https://{host}/tts/cognitiveservices/v1"),
                None => AzureRegion::default().tts_rest_url(),
            },
        }
    }

    /// The voices list URL.
    ///
    /// - `Region(r)` → `https://<r>.tts.speech.microsoft.com/cognitiveservices/voices/list`.
    /// - `Resource` → `https://<name>.cognitiveservices.azure.com/tts/cognitiveservices/voices/list`
    ///   (the same custom-domain rule as [`tts_rest_url`](Self::tts_rest_url); pending a probe).
    pub fn voices_list_url(&self) -> String {
        match self {
            Self::Region(region) => region.voices_list_url(),
            Self::Resource { host } => match Self::safe_resource_host(host) {
                Some(host) => format!("https://{host}/tts/cognitiveservices/voices/list"),
                None => AzureRegion::default().voices_list_url(),
            },
        }
    }

    /// The hostname the Speech-to-Text WebSocket dials (and pins as its `Host` header).
    pub fn stt_hostname(&self) -> String {
        match self {
            Self::Region(region) => region.stt_hostname(),
            Self::Resource { host } => match Self::safe_resource_host(host) {
                Some(host) => host.to_string(),
                None => AzureRegion::default().stt_hostname(),
            },
        }
    }

    /// The Speech-to-Text WebSocket base URL, to which the caller appends
    /// `/speech/recognition/conversation/cognitiveservices/v1?…`.
    ///
    /// - `Region(r)` → `wss://<r>.stt.speech.microsoft.com` (unchanged).
    /// - `Resource` → `wss://<name>.cognitiveservices.azure.com/stt` — the same custom-domain
    ///   rule as [`tts_rest_url`](Self::tts_rest_url) (`/stt` prefix in front of the regional
    ///   path), so the recognition path and its query parameters are identical on both. Also
    ///   pending a live probe.
    pub fn stt_websocket_base_url(&self) -> String {
        match self {
            Self::Region(region) => region.stt_websocket_base_url(),
            Self::Resource { host } => match Self::safe_resource_host(host) {
                Some(host) => format!("wss://{host}/stt"),
                None => AzureRegion::default().stt_websocket_base_url(),
            },
        }
    }
}

/// The single DNS label in front of `suffix` (`suffix` starts with a dot), or `None` when `host`
/// does not end with it or has zero / several labels before it. `a.b.api.cognitive.microsoft.com`
/// and `api.cognitive.microsoft.com` are both `None` for the `.api.cognitive.microsoft.com` suffix.
fn single_label_before<'a>(host: &'a str, suffix: &str) -> Option<&'a str> {
    let label = host.strip_suffix(suffix)?;
    (!label.is_empty() && !label.contains('.')).then_some(label)
}

fn validate_azure_region_identifier(region: &str) -> Result<(), String> {
    if is_valid_azure_region_identifier(region) {
        Ok(())
    } else {
        Err(format!(
            "custom Azure region must be a single DNS label containing only ASCII letters, digits, or hyphen, got {region:?}"
        ))
    }
}

fn is_valid_azure_region_identifier(region: &str) -> bool {
    if region.is_empty() || region.len() > 63 {
        return false;
    }

    let mut chars = region.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }

    let mut last = first;
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
        last = c;
    }

    last.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Basic Region Tests
    // =========================================================================

    #[test]
    fn test_azure_region_default() {
        let region = AzureRegion::default();
        assert_eq!(region, AzureRegion::EastUS);
        assert_eq!(region.as_str(), "eastus");
    }

    #[test]
    fn test_azure_region_custom() {
        let region = AzureRegion::Custom("newregion".to_string());
        assert_eq!(region.as_str(), "newregion");
        assert_eq!(region.stt_hostname(), "newregion.stt.speech.microsoft.com");
        assert_eq!(region.tts_hostname(), "newregion.tts.speech.microsoft.com");
    }

    // =========================================================================
    // STT Endpoint Tests
    // =========================================================================

    #[test]
    fn test_azure_region_stt_hostname() {
        let region = AzureRegion::WestEurope;
        assert_eq!(region.stt_hostname(), "westeurope.stt.speech.microsoft.com");
    }

    #[test]
    fn test_azure_region_stt_websocket_url() {
        let region = AzureRegion::SoutheastAsia;
        assert_eq!(
            region.stt_websocket_base_url(),
            "wss://southeastasia.stt.speech.microsoft.com"
        );
    }

    // =========================================================================
    // TTS Endpoint Tests
    // =========================================================================

    #[test]
    fn test_azure_region_tts_hostname() {
        let region = AzureRegion::WestEurope;
        assert_eq!(region.tts_hostname(), "westeurope.tts.speech.microsoft.com");
    }

    #[test]
    fn test_azure_region_tts_rest_url() {
        let region = AzureRegion::EastUS;
        assert_eq!(
            region.tts_rest_url(),
            "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1"
        );
    }

    #[test]
    fn test_azure_region_voices_list_url() {
        let region = AzureRegion::WestEurope;
        assert_eq!(
            region.voices_list_url(),
            "https://westeurope.tts.speech.microsoft.com/cognitiveservices/voices/list"
        );
    }

    #[test]
    fn test_azure_region_tts_endpoints_for_all_regions() {
        // Test a sampling of regions to ensure TTS endpoints are correctly formed
        let test_cases = vec![
            (AzureRegion::EastUS, "eastus"),
            (AzureRegion::WestEurope, "westeurope"),
            (AzureRegion::JapanEast, "japaneast"),
            (AzureRegion::AustraliaEast, "australiaeast"),
        ];

        for (region, region_str) in test_cases {
            assert_eq!(
                region.tts_hostname(),
                format!("{}.tts.speech.microsoft.com", region_str)
            );
            assert_eq!(
                region.tts_rest_url(),
                format!(
                    "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
                    region_str
                )
            );
            assert_eq!(
                region.voices_list_url(),
                format!(
                    "https://{}.tts.speech.microsoft.com/cognitiveservices/voices/list",
                    region_str
                )
            );
        }
    }

    // =========================================================================
    // Authentication Endpoint Tests
    // =========================================================================

    #[test]
    fn test_azure_region_token_endpoint() {
        let region = AzureRegion::EastUS;
        assert_eq!(
            region.token_endpoint(),
            "https://eastus.api.cognitive.microsoft.com/sts/v1.0/issueToken"
        );
    }

    // =========================================================================
    // FromStr Tests
    // =========================================================================

    #[test]
    fn test_azure_region_from_str() {
        assert_eq!(
            "eastus".parse::<AzureRegion>().unwrap(),
            AzureRegion::EastUS
        );
        assert_eq!(
            "WESTEUROPE".parse::<AzureRegion>().unwrap(),
            AzureRegion::WestEurope
        );
        assert_eq!(
            "unknown".parse::<AzureRegion>().unwrap(),
            AzureRegion::Custom("unknown".to_string())
        );
    }

    #[test]
    fn test_azure_region_from_str_rejects_unsafe_custom_labels() {
        for input in [
            "",
            " ",
            "-bad",
            "bad-",
            "bad_label",
            "evil.com/path",
            "evil.com@127.0.0.1",
            "127.0.0.1:9000/foo",
            "[::1]:9000/foo",
        ] {
            let err = input
                .parse::<AzureRegion>()
                .expect_err("unsafe custom region must be rejected");
            assert!(
                err.contains("single DNS label"),
                "unexpected error for {input:?}: {err}"
            );
        }
    }

    #[test]
    fn test_azure_region_endpoint_builders_ignore_unsafe_direct_custom() {
        let region = AzureRegion::Custom("127.0.0.1:9000/foo".to_string());

        assert_eq!(region.stt_hostname(), "eastus.stt.speech.microsoft.com");
        assert_eq!(
            region.stt_websocket_base_url(),
            "wss://eastus.stt.speech.microsoft.com"
        );
        assert_eq!(region.tts_hostname(), "eastus.tts.speech.microsoft.com");
        assert_eq!(
            region.tts_rest_url(),
            "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1"
        );
        assert_eq!(
            region.voices_list_url(),
            "https://eastus.tts.speech.microsoft.com/cognitiveservices/voices/list"
        );
        assert_eq!(
            region.token_endpoint(),
            "https://eastus.api.cognitive.microsoft.com/sts/v1.0/issueToken"
        );
    }

    #[test]
    fn test_azure_region_from_str_case_insensitive() {
        assert_eq!(
            "EASTUS".parse::<AzureRegion>().unwrap(),
            AzureRegion::EastUS
        );
        assert_eq!(
            "EastUs".parse::<AzureRegion>().unwrap(),
            AzureRegion::EastUS
        );
        assert_eq!(
            "WestEurope".parse::<AzureRegion>().unwrap(),
            AzureRegion::WestEurope
        );
    }

    #[test]
    fn test_region_from_str_all_known_regions() {
        let test_cases = vec![
            ("eastus", AzureRegion::EastUS),
            ("eastus2", AzureRegion::EastUS2),
            ("westus", AzureRegion::WestUS),
            ("westus2", AzureRegion::WestUS2),
            ("westus3", AzureRegion::WestUS3),
            ("centralus", AzureRegion::CentralUS),
            ("northcentralus", AzureRegion::NorthCentralUS),
            ("southcentralus", AzureRegion::SouthCentralUS),
            ("westeurope", AzureRegion::WestEurope),
            ("northeurope", AzureRegion::NorthEurope),
            ("uksouth", AzureRegion::UKSouth),
            ("francecentral", AzureRegion::FranceCentral),
            ("germanywestcentral", AzureRegion::GermanyWestCentral),
            ("switzerlandnorth", AzureRegion::SwitzerlandNorth),
            ("eastasia", AzureRegion::EastAsia),
            ("southeastasia", AzureRegion::SoutheastAsia),
            ("japaneast", AzureRegion::JapanEast),
            ("japanwest", AzureRegion::JapanWest),
            ("koreacentral", AzureRegion::KoreaCentral),
            ("australiaeast", AzureRegion::AustraliaEast),
            ("canadacentral", AzureRegion::CanadaCentral),
            ("brazilsouth", AzureRegion::BrazilSouth),
            ("centralindia", AzureRegion::IndiaCentral),
        ];

        for (input, expected) in test_cases {
            assert_eq!(
                input.parse::<AzureRegion>().unwrap(),
                expected,
                "Parsing '{}' should produce {:?}",
                input,
                expected
            );
        }
    }

    // =========================================================================
    // All Regions as_str Tests
    // =========================================================================

    #[test]
    fn test_all_regions_as_str() {
        let regions = vec![
            (AzureRegion::EastUS, "eastus"),
            (AzureRegion::EastUS2, "eastus2"),
            (AzureRegion::WestUS, "westus"),
            (AzureRegion::WestUS2, "westus2"),
            (AzureRegion::WestUS3, "westus3"),
            (AzureRegion::CentralUS, "centralus"),
            (AzureRegion::NorthCentralUS, "northcentralus"),
            (AzureRegion::SouthCentralUS, "southcentralus"),
            (AzureRegion::WestEurope, "westeurope"),
            (AzureRegion::NorthEurope, "northeurope"),
            (AzureRegion::UKSouth, "uksouth"),
            (AzureRegion::FranceCentral, "francecentral"),
            (AzureRegion::GermanyWestCentral, "germanywestcentral"),
            (AzureRegion::SwitzerlandNorth, "switzerlandnorth"),
            (AzureRegion::EastAsia, "eastasia"),
            (AzureRegion::SoutheastAsia, "southeastasia"),
            (AzureRegion::JapanEast, "japaneast"),
            (AzureRegion::JapanWest, "japanwest"),
            (AzureRegion::KoreaCentral, "koreacentral"),
            (AzureRegion::AustraliaEast, "australiaeast"),
            (AzureRegion::CanadaCentral, "canadacentral"),
            (AzureRegion::BrazilSouth, "brazilsouth"),
            (AzureRegion::IndiaCentral, "centralindia"),
        ];

        for (region, expected) in regions {
            assert_eq!(
                region.as_str(),
                expected,
                "Region {:?} should produce '{}'",
                region,
                expected
            );
        }
    }

    // =========================================================================
    // Backwards Compatibility Tests
    // =========================================================================

    #[test]
    #[allow(deprecated)]
    fn test_deprecated_hostname_method() {
        let region = AzureRegion::WestEurope;
        assert_eq!(region.hostname(), region.stt_hostname());
    }

    #[test]
    #[allow(deprecated)]
    fn test_deprecated_websocket_base_url_method() {
        let region = AzureRegion::SoutheastAsia;
        assert_eq!(region.websocket_base_url(), region.stt_websocket_base_url());
    }

    // =========================================================================
    // AzureSpeechEndpoint::from_api_base
    // =========================================================================

    fn resource(host: &str) -> AzureSpeechEndpoint {
        AzureSpeechEndpoint::Resource {
            host: host.to_string(),
        }
    }

    #[test]
    fn from_api_base_accepts_every_documented_shape() {
        // (api_base, expected endpoint, expect a rewrite note)
        let cases = [
            // Regional token/API host, with and without a trailing slash or a path.
            (
                "https://westeurope.api.cognitive.microsoft.com/",
                AzureSpeechEndpoint::Region(AzureRegion::WestEurope),
                false,
            ),
            (
                "https://westeurope.api.cognitive.microsoft.com",
                AzureSpeechEndpoint::Region(AzureRegion::WestEurope),
                false,
            ),
            (
                "https://eastus2.api.cognitive.microsoft.com/sts/v1.0/issueToken",
                AzureSpeechEndpoint::Region(AzureRegion::EastUS2),
                false,
            ),
            // Regional Speech hosts.
            (
                "https://japaneast.tts.speech.microsoft.com/cognitiveservices/v1",
                AzureSpeechEndpoint::Region(AzureRegion::JapanEast),
                false,
            ),
            (
                "https://southeastasia.stt.speech.microsoft.com",
                AzureSpeechEndpoint::Region(AzureRegion::SoutheastAsia),
                false,
            ),
            (
                "https://uksouth.voice.speech.microsoft.com/",
                AzureSpeechEndpoint::Region(AzureRegion::UKSouth),
                false,
            ),
            // Case-insensitive, surrounding whitespace, explicit default port, trailing root dot.
            (
                "  HTTPS://WestEurope.API.Cognitive.Microsoft.COM/  ",
                AzureSpeechEndpoint::Region(AzureRegion::WestEurope),
                false,
            ),
            (
                "https://westeurope.api.cognitive.microsoft.com:443/",
                AzureSpeechEndpoint::Region(AzureRegion::WestEurope),
                false,
            ),
            (
                "https://westeurope.api.cognitive.microsoft.com./",
                AzureSpeechEndpoint::Region(AzureRegion::WestEurope),
                false,
            ),
            // A region WaaV has no named variant for stays usable — never becomes eastus.
            (
                "https://italynorth.api.cognitive.microsoft.com/",
                AzureSpeechEndpoint::Region(AzureRegion::Custom("italynorth".to_string())),
                false,
            ),
            // Resource custom domain, used as given (lowercased).
            (
                "https://my-speech.cognitiveservices.azure.com/",
                resource("my-speech.cognitiveservices.azure.com"),
                false,
            ),
            (
                "https://My-Speech.CognitiveServices.Azure.com",
                resource("my-speech.cognitiveservices.azure.com"),
                false,
            ),
            // AI Services alias hosts → the resource's cognitiveservices host, with a note.
            (
                "https://my-foundry.openai.azure.com/",
                resource("my-foundry.cognitiveservices.azure.com"),
                true,
            ),
            (
                "https://My-Foundry.services.ai.azure.com/api/projects/p1",
                resource("my-foundry.cognitiveservices.azure.com"),
                true,
            ),
        ];

        for (api_base, expected, expect_note) in cases {
            let (endpoint, note) = AzureSpeechEndpoint::from_api_base_with_note(api_base)
                .unwrap_or_else(|e| panic!("{api_base:?} must be accepted: {e}"));
            assert_eq!(endpoint, expected, "{api_base:?}");
            assert_eq!(note.is_some(), expect_note, "{api_base:?} note: {note:?}");
            assert_eq!(
                AzureSpeechEndpoint::from_api_base(api_base).unwrap(),
                expected,
                "{api_base:?}"
            );
        }
    }

    #[test]
    fn from_api_base_rewrite_note_names_both_hosts_but_not_the_value() {
        let (_, note) =
            AzureSpeechEndpoint::from_api_base_with_note("https://secretname.openai.azure.com/")
                .unwrap();
        let note = note.expect("an alias host is reported");
        assert!(note.contains("openai.azure.com"), "{note}");
        assert!(note.contains("cognitiveservices.azure.com"), "{note}");
        assert!(!note.contains("secretname"), "{note}");
    }

    #[test]
    fn from_api_base_refuses_everything_else() {
        let refused = [
            "",
            "   ",
            // Not Azure.
            "https://evil.example.com",
            "https://evil.example.com/westeurope.api.cognitive.microsoft.com",
            "https://westeurope.api.cognitive.microsoft.com.evil.com/",
            "https://evilapi.cognitive.microsoft.com/",
            "https://localhost/",
            // Not https.
            "http://westeurope.api.cognitive.microsoft.com/",
            "wss://westeurope.stt.speech.microsoft.com/",
            "ftp://westeurope.api.cognitive.microsoft.com/",
            // No scheme at all.
            "westeurope.api.cognitive.microsoft.com",
            // IP literals.
            "https://20.50.1.1/",
            "https://[::1]/",
            // Credentials / a different host hidden behind userinfo.
            "https://user:topsecret@westeurope.api.cognitive.microsoft.com/",
            "https://westeurope.api.cognitive.microsoft.com@evil.example.com/",
            // Non-default port.
            "https://westeurope.api.cognitive.microsoft.com:8443/",
            // Zero or several labels in front of the suffix.
            "https://api.cognitive.microsoft.com/",
            "https://a.westeurope.api.cognitive.microsoft.com/",
            "https://cognitiveservices.azure.com/",
            "https://a.b.cognitiveservices.azure.com/",
            "https://openai.azure.com/",
            // Sovereign clouds are not accepted shapes (refused, not sent to the public cloud).
            "https://my-speech.cognitiveservices.azure.us/",
            "https://chinaeast2.api.cognitive.azure.cn/",
        ];

        for api_base in refused {
            let err = AzureSpeechEndpoint::from_api_base(api_base)
                .expect_err(&format!("{api_base:?} must be refused"));
            assert!(err.contains("api_base"), "{api_base:?}: {err}");
            assert!(
                err.contains("cognitiveservices.azure.com")
                    && err.contains("api.cognitive.microsoft.com"),
                "the refusal must list the accepted shapes: {err}"
            );
            if !api_base.trim().is_empty() {
                assert!(
                    !err.contains(api_base.trim()),
                    "the refusal must not echo the value: {err}"
                );
            }
        }
    }

    #[test]
    fn from_api_base_refusal_never_echoes_credentials() {
        let err = AzureSpeechEndpoint::from_api_base(
            "https://user:topsecret@westeurope.api.cognitive.microsoft.com/",
        )
        .unwrap_err();
        assert!(!err.contains("topsecret"), "{err}");
        assert!(!err.contains("user:"), "{err}");

        // A key pasted into the api_base field in place of a host.
        let err = AzureSpeechEndpoint::from_api_base("https://0123456789abcdef0123456789abcdef/")
            .unwrap_err();
        assert!(!err.contains("0123456789abcdef"), "{err}");
    }

    // =========================================================================
    // AzureSpeechEndpoint URL builders
    // =========================================================================

    #[test]
    fn speech_endpoint_region_urls_match_the_region_builders() {
        let endpoint = AzureSpeechEndpoint::Region(AzureRegion::WestEurope);
        let region = AzureRegion::WestEurope;
        assert_eq!(endpoint.tts_rest_url(), region.tts_rest_url());
        assert_eq!(endpoint.voices_list_url(), region.voices_list_url());
        assert_eq!(endpoint.stt_hostname(), region.stt_hostname());
        assert_eq!(
            endpoint.stt_websocket_base_url(),
            region.stt_websocket_base_url()
        );
        assert_eq!(endpoint.region(), Some(&AzureRegion::WestEurope));
    }

    #[test]
    fn speech_endpoint_resource_urls_use_the_custom_domain_prefixes() {
        let endpoint = resource("my-speech.cognitiveservices.azure.com");
        assert_eq!(
            endpoint.tts_rest_url(),
            "https://my-speech.cognitiveservices.azure.com/tts/cognitiveservices/v1"
        );
        assert_eq!(
            endpoint.voices_list_url(),
            "https://my-speech.cognitiveservices.azure.com/tts/cognitiveservices/voices/list"
        );
        assert_eq!(
            endpoint.stt_hostname(),
            "my-speech.cognitiveservices.azure.com"
        );
        assert_eq!(
            endpoint.stt_websocket_base_url(),
            "wss://my-speech.cognitiveservices.azure.com/stt"
        );
        assert_eq!(endpoint.region(), None);
    }

    #[test]
    fn speech_endpoint_builders_ignore_unsafe_direct_resource_host() {
        // Mirrors `test_azure_region_endpoint_builders_ignore_unsafe_direct_custom`: a host that
        // `from_api_base` would never produce is not put on the wire.
        for host in [
            "evil.example.com",
            "127.0.0.1:9000/foo",
            "a.b.cognitiveservices.azure.com",
            "evil.com/x.cognitiveservices.azure.com",
        ] {
            let endpoint = resource(host);
            assert_eq!(
                endpoint.tts_rest_url(),
                "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1",
                "{host}"
            );
            assert_eq!(
                endpoint.voices_list_url(),
                "https://eastus.tts.speech.microsoft.com/cognitiveservices/voices/list",
                "{host}"
            );
            assert_eq!(
                endpoint.stt_hostname(),
                "eastus.stt.speech.microsoft.com",
                "{host}"
            );
            assert_eq!(
                endpoint.stt_websocket_base_url(),
                "wss://eastus.stt.speech.microsoft.com",
                "{host}"
            );
        }
    }
}
