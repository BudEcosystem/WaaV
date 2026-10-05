//! The HTTP clients that carry segment uploads, their settings, and pool warming.
//!
//! Uploads use their own clients, not the gateway's shared ones: a live call cannot wait out a
//! 300 s timeout or a cold connection, and an upload outage must not share a pool with anything
//! else. Every client speaks HTTP/1.1 unless its host is listed for HTTP/2, because HTTP/2 puts
//! every upload in flight on one connection behind a 65,535-byte initial window (plan, chapter 2,
//! "HTTP version"); a host moves to HTTP/2 only after it has been measured.
//!
//! Redirects are never followed: a transcription endpoint has no reason to redirect, and a
//! multipart upload cannot be replayed after one.

use std::time::Duration;

use futures::future::join_all;
use reqwest::redirect::Policy;

/// How long one warm request may take before it is abandoned (integration decisions, A10).
pub const WARM_TIMEOUT: Duration = Duration::from_millis(2000);

/// Settings of the upload clients, read once at start-up from `WAAV_STT_SEGMENT_*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpSettings {
    /// Hosts that get the HTTP/2 client. Empty until a host is measured.
    pub http2_hosts: Vec<String>,
    /// Cold setup was measured at 100 to 440 ms; a connect slower than 1.5 s is better abandoned
    /// and retried on a fresh connection. reqwest's default is no limit.
    pub connect_timeout: Duration,
    /// A guard for a request sent without its own timeout. Every upload sets its own.
    pub backstop_timeout: Duration,
    /// Below reqwest's 90 s so the gateway closes an idle connection before a vendor does.
    pub pool_idle_timeout: Duration,
    /// Equal to the largest depth the warming rule asks for, so the cap never undoes a warm-up.
    pub pool_max_idle_per_host: usize,
    pub tcp_keepalive: Duration,
    /// The operator's escape hatch (the gateway's `WAAV_ALLOW_LOOPBACK_ENDPOINTS`, for local
    /// development and tests): the public pool may then connect to private addresses, and a base
    /// a client named may be one.
    pub public_may_reach_private: bool,
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            http2_hosts: Vec::new(),
            connect_timeout: Duration::from_millis(1500),
            backstop_timeout: Duration::from_millis(15_000),
            pool_idle_timeout: Duration::from_millis(50_000),
            pool_max_idle_per_host: 64,
            tcp_keepalive: Duration::from_secs(15),
            public_may_reach_private: false,
        }
    }
}

impl HttpSettings {
    pub const HTTP2_HOSTS: &'static str = "WAAV_STT_SEGMENT_HTTP2_HOSTS";
    pub const CONNECT_TIMEOUT_MS: &'static str = "WAAV_STT_SEGMENT_CONNECT_TIMEOUT_MS";
    pub const BACKSTOP_TIMEOUT_MS: &'static str = "WAAV_STT_SEGMENT_CLIENT_BACKSTOP_TIMEOUT_MS";
    pub const POOL_IDLE_TIMEOUT_MS: &'static str = "WAAV_STT_SEGMENT_POOL_IDLE_TIMEOUT_MS";
    pub const POOL_MAX_IDLE_PER_HOST: &'static str = "WAAV_STT_SEGMENT_POOL_MAX_IDLE_PER_HOST";
    pub const TCP_KEEPALIVE_S: &'static str = "WAAV_STT_SEGMENT_TCP_KEEPALIVE_S";

    /// Reads the settings through `get` (the environment in production, a map in tests). An unset
    /// or blank variable keeps its default; a malformed one is an error that names it, so a typo
    /// fails start-up instead of silently running on the default.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut s = Self::default();
        let read = |name: &str| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };

        if let Some(v) = read(Self::HTTP2_HOSTS) {
            s.http2_hosts = v
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter_map(normalize_host_entry)
                .collect();
        }
        if let Some(v) = read(Self::CONNECT_TIMEOUT_MS) {
            s.connect_timeout = Duration::from_millis(positive(Self::CONNECT_TIMEOUT_MS, &v)?);
        }
        if let Some(v) = read(Self::BACKSTOP_TIMEOUT_MS) {
            s.backstop_timeout = Duration::from_millis(positive(Self::BACKSTOP_TIMEOUT_MS, &v)?);
        }
        if let Some(v) = read(Self::POOL_IDLE_TIMEOUT_MS) {
            s.pool_idle_timeout = Duration::from_millis(positive(Self::POOL_IDLE_TIMEOUT_MS, &v)?);
        }
        if let Some(v) = read(Self::POOL_MAX_IDLE_PER_HOST) {
            // Zero is legal: it disables pooling, which an operator may want while debugging.
            s.pool_max_idle_per_host = v.parse::<usize>().map_err(|_| {
                format!(
                    "{} must be a whole number, got {v:?}",
                    Self::POOL_MAX_IDLE_PER_HOST
                )
            })?;
        }
        if let Some(v) = read(Self::TCP_KEEPALIVE_S) {
            s.tcp_keepalive = Duration::from_secs(positive(Self::TCP_KEEPALIVE_S, &v)?);
        }
        Ok(s)
    }

    /// Reads the settings from the process environment.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn wants_http2(&self, url: &str) -> bool {
        if self.http2_hosts.is_empty() {
            return false;
        }
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) else {
            return false;
        };
        let with_port = parsed
            .port_or_known_default()
            .map(|p| format!("{host}:{p}"));
        self.http2_hosts
            .iter()
            .any(|h| *h == host || with_port.as_deref() == Some(h.as_str()))
    }
}

fn positive(name: &str, value: &str) -> Result<u64, String> {
    match value.parse::<u64>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!(
            "{name} must be a whole number above zero, got {value:?}"
        )),
    }
}

/// `https://Api.Groq.com/openai` and `api.groq.com` both name the host `api.groq.com`.
fn normalize_host_entry(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let host = match raw.find("://") {
        Some(i) => &raw[i + 3..],
        None => raw,
    };
    let host = host.split('/').next().unwrap_or(host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The shared upload clients. One set per process (or per test), cloned cheaply into each
/// transcriber.
///
/// `public` and `trusted` are configured alike; they are separate pools because they will carry
/// different address rules once the gateway's egress resolver is attached (public hosts never
/// reach private addresses; a trusted in-cluster base from a Bud deployment record may), and a
/// pooled connection must never cross that line.
#[derive(Debug, Clone)]
pub struct UploadClients {
    public: reqwest::Client,
    trusted: reqwest::Client,
    /// HTTP/2 twins, built only when some host is listed for HTTP/2.
    http2: Option<(reqwest::Client, reqwest::Client)>,
    settings: HttpSettings,
}

impl UploadClients {
    pub fn new(settings: &HttpSettings) -> Result<Self, String> {
        let http2 = if settings.http2_hosts.is_empty() {
            None
        } else {
            Some((
                build(settings, false, !settings.public_may_reach_private)?,
                build(settings, false, false)?,
            ))
        };
        Ok(Self {
            public: build(settings, true, !settings.public_may_reach_private)?,
            trusted: build(settings, true, false)?,
            http2,
            settings: settings.clone(),
        })
    }

    /// Whether a base a client named may be a private address (the operator's escape hatch).
    pub fn public_may_reach_private(&self) -> bool {
        self.settings.public_may_reach_private
    }

    /// The HTTP/1.1 client for public vendor hosts, or for a trusted in-cluster base.
    pub fn client(&self, trusted: bool) -> &reqwest::Client {
        if trusted { &self.trusted } else { &self.public }
    }

    /// The client for one URL: the HTTP/2 twin when its host is listed, else [`Self::client`].
    pub fn client_for(&self, url: &str, trusted: bool) -> &reqwest::Client {
        match &self.http2 {
            Some((public, trusted_h2)) if self.settings.wants_http2(url) => {
                if trusted {
                    trusted_h2
                } else {
                    public
                }
            }
            _ => self.client(trusted),
        }
    }

    pub fn settings(&self) -> &HttpSettings {
        &self.settings
    }
}

fn build(s: &HttpSettings, http1_only: bool, public_only: bool) -> Result<reqwest::Client, String> {
    let b = reqwest::Client::builder()
        .use_rustls_tls()
        .redirect(Policy::none())
        .connect_timeout(s.connect_timeout)
        .timeout(s.backstop_timeout)
        .pool_idle_timeout(s.pool_idle_timeout)
        .pool_max_idle_per_host(s.pool_max_idle_per_host)
        .tcp_keepalive(s.tcp_keepalive)
        .tcp_nodelay(true);
    let b = if public_only {
        b.dns_resolver(std::sync::Arc::new(PublicOnlyResolver))
    } else {
        b
    };
    let b = if http1_only {
        b.http1_only()
    } else {
        // ALPN decides; the adaptive window lets one connection carry several uploads without
        // stalling on the 65,535-byte initial window.
        b.http2_adaptive_window(true)
    };
    b.build()
        .map_err(|e| format!("could not build the segment upload client: {e}"))
}

/// The public pool's resolver: a name is used only through its public addresses, so a vendor host
/// (or a host a client named) that resolves to a private, loopback or metadata address is never
/// dialled. An IP literal skips resolution; [`check_untrusted_base`] refuses those at plan time.
struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| is_public_ip(&a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!(
                    "'{host}' resolves only to private addresses (SSRF protection)"
                )
                .into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Whether an address is on the public internet: not loopback, private, link-local (cloud
/// metadata), carrier-grade NAT, unspecified, broadcast, documentation, benchmarking, reserved or
/// multicast; an IPv4-mapped IPv6 address is judged as its IPv4 address.
pub fn is_public_ip(ip: &std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(&IpAddr::V4(v4));
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
                || seg[0] == 0x2001 && seg[1] == 0x0db8)
        }
    }
}

/// A base address a client named (not one from a Bud deployment record): `http` or `https`, and
/// not a private address or `localhost`. Names are checked again at connect time by the public
/// pool's resolver. `allow_private` is the operator's escape hatch.
pub fn check_untrusted_base(base: &str, allow_private: bool) -> Result<(), String> {
    let u = url::Url::parse(base.trim())
        .map_err(|e| format!("'{base}' is not a valid address: {e}"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(format!(
            "'{base}': only http and https addresses are accepted"
        ));
    }
    if allow_private {
        return Ok(());
    }
    match u.host() {
        Some(url::Host::Ipv4(ip)) if !is_public_ip(&ip.into()) => Err(format!(
            "'{base}' is a private or loopback address (SSRF protection)"
        )),
        Some(url::Host::Ipv6(ip)) if !is_public_ip(&ip.into()) => Err(format!(
            "'{base}' is a private or loopback address (SSRF protection)"
        )),
        Some(url::Host::Domain(d))
            if d.eq_ignore_ascii_case("localhost")
                || d.to_ascii_lowercase().ends_with(".localhost") =>
        {
            Err(format!("'{base}' is a loopback address (SSRF protection)"))
        }
        None => Err(format!("'{base}' has no host")),
        _ => Ok(()),
    }
}

/// The limiter, breaker and pool key of a URL: `scheme://host:port`, lower case, with the port
/// always written so `https://h` and `https://h:443` are one key.
pub fn host_key(url: &str) -> String {
    match url::Url::parse(url.trim()) {
        Ok(u) => {
            let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
            match u.port_or_known_default() {
                Some(port) => format!("{}://{host}:{port}", u.scheme()),
                None => format!("{}://{host}", u.scheme()),
            }
        }
        Err(_) => url.trim().to_ascii_lowercase(),
    }
}

/// The origin of a URL with a trailing slash, the target of a warm request.
pub fn origin(url: &str) -> Option<String> {
    let u = url::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(port) => format!("{}://{host}:{port}/", u.scheme()),
        None => format!("{}://{host}/", u.scheme()),
    })
}

/// Opens up to `connections` pooled connections to the origin of `url` by sending that many
/// `HEAD` requests at once, so the next uploads do not pay DNS, TCP and TLS.
///
/// The requests carry no credential and their status is ignored: any answer leaves a reusable
/// connection, and a `HEAD` answer has no body to drain. They are sent together, not one after
/// another, because the pool hands back its newest idle connection and sequential requests would
/// all reuse one. Returns how many got an answer; failures are swallowed, since a failed warm-up
/// forfeits only the saving.
pub async fn warm(client: &reqwest::Client, url: &str, connections: usize) -> usize {
    let Some(target) = origin(url) else {
        return 0;
    };
    warm_with(connections, WARM_TIMEOUT, || client.head(target.as_str())).await
}

/// [`warm`] with a vendor's own warm request (AssemblyAI's unauthenticated `GET /v1/warm`).
/// A body, if any, is read to its end so the connection can go back to the pool.
pub async fn warm_with(
    connections: usize,
    timeout: Duration,
    request: impl Fn() -> reqwest::RequestBuilder,
) -> usize {
    let attempts = (0..connections).map(|_| {
        let req = request().timeout(timeout);
        async move {
            let Ok(mut resp) = req.send().await else {
                return false;
            };
            // A warm endpoint's body is tiny; stop reading a large one rather than buffer it.
            let mut read = 0usize;
            while let Ok(Some(chunk)) = resp.chunk().await {
                read += chunk.len();
                if read > 64 * 1024 {
                    break;
                }
            }
            true
        }
    });
    join_all(attempts)
        .await
        .into_iter()
        .filter(|ok| *ok)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::extract::ConnectInfo;
    use parking_lot::Mutex;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults_are_the_planned_values() {
        let s = HttpSettings::default();
        assert!(s.http2_hosts.is_empty());
        assert_eq!(s.connect_timeout, Duration::from_millis(1500));
        assert_eq!(s.backstop_timeout, Duration::from_millis(15_000));
        assert_eq!(s.pool_idle_timeout, Duration::from_millis(50_000));
        assert_eq!(s.pool_max_idle_per_host, 64);
        assert_eq!(s.tcp_keepalive, Duration::from_secs(15));
        assert_eq!(HttpSettings::from_lookup(|_| None).unwrap(), s);
    }

    #[test]
    fn every_variable_is_read() {
        let s = HttpSettings::from_lookup(lookup(&[
            (
                "WAAV_STT_SEGMENT_HTTP2_HOSTS",
                "api.groq.com, https://API.openai.com/v1 ,,",
            ),
            ("WAAV_STT_SEGMENT_CONNECT_TIMEOUT_MS", "900"),
            ("WAAV_STT_SEGMENT_CLIENT_BACKSTOP_TIMEOUT_MS", "12000"),
            ("WAAV_STT_SEGMENT_POOL_IDLE_TIMEOUT_MS", "30000"),
            ("WAAV_STT_SEGMENT_POOL_MAX_IDLE_PER_HOST", "0"),
            ("WAAV_STT_SEGMENT_TCP_KEEPALIVE_S", "20"),
        ]))
        .unwrap();
        assert_eq!(s.http2_hosts, vec!["api.groq.com", "api.openai.com"]);
        assert_eq!(s.connect_timeout, Duration::from_millis(900));
        assert_eq!(s.backstop_timeout, Duration::from_millis(12_000));
        assert_eq!(s.pool_idle_timeout, Duration::from_millis(30_000));
        assert_eq!(s.pool_max_idle_per_host, 0);
        assert_eq!(s.tcp_keepalive, Duration::from_secs(20));
    }

    #[test]
    fn a_blank_variable_keeps_the_default() {
        let s = HttpSettings::from_lookup(lookup(&[("WAAV_STT_SEGMENT_CONNECT_TIMEOUT_MS", "  ")]));
        assert_eq!(s.unwrap().connect_timeout, Duration::from_millis(1500));
    }

    #[test]
    fn a_malformed_variable_is_an_error_that_names_it() {
        for name in [
            HttpSettings::CONNECT_TIMEOUT_MS,
            HttpSettings::BACKSTOP_TIMEOUT_MS,
            HttpSettings::POOL_IDLE_TIMEOUT_MS,
            HttpSettings::POOL_MAX_IDLE_PER_HOST,
            HttpSettings::TCP_KEEPALIVE_S,
        ] {
            let err = HttpSettings::from_lookup(lookup(&[(name, "1.5s")])).unwrap_err();
            assert!(err.contains(name), "{err}");
        }
        let err = HttpSettings::from_lookup(lookup(&[(HttpSettings::CONNECT_TIMEOUT_MS, "0")]));
        assert!(err.unwrap_err().contains(HttpSettings::CONNECT_TIMEOUT_MS));
    }

    #[test]
    fn host_key_is_scheme_host_and_port() {
        assert_eq!(
            host_key("https://API.OpenAI.com/v1/audio/transcriptions"),
            "https://api.openai.com:443"
        );
        assert_eq!(
            host_key("https://api.openai.com:443/x"),
            "https://api.openai.com:443"
        );
        assert_eq!(
            host_key("http://whisper.ns.svc:8000/v1/audio"),
            "http://whisper.ns.svc:8000"
        );
        assert_eq!(host_key("http://127.0.0.1:9/a?b=c"), "http://127.0.0.1:9");
        assert_eq!(host_key("http://[::1]:8080/"), "http://[::1]:8080");
    }

    /// The public pool serves vendor hosts, and on a standalone session a host the client named:
    /// it must never connect to a private address, whatever the name resolves to (DNS rebinding).
    #[tokio::test]
    async fn the_public_pool_never_reaches_a_private_address_by_name() {
        let (addr, _) = counting_server().await;
        let by_name = format!("http://localhost:{}/v1/audio/transcriptions", addr.port());
        let clients = UploadClients::new(&HttpSettings::default()).unwrap();
        let refused = clients.client(false).head(by_name.as_str()).send().await;
        assert!(refused.is_err(), "localhost resolves only to loopback");
        let trusted = clients.client(true).head(by_name.as_str()).send().await;
        assert!(
            trusted.is_ok(),
            "a Bud deployment's in-cluster base may be private"
        );

        let dev = UploadClients::new(&HttpSettings {
            public_may_reach_private: true,
            ..Default::default()
        })
        .unwrap();
        assert!(
            dev.client(false)
                .head(by_name.as_str())
                .send()
                .await
                .is_ok()
        );
    }

    #[test]
    fn only_public_addresses_pass_the_filter() {
        use std::net::IpAddr;
        let ips: Vec<IpAddr> = [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        let public: Vec<String> = ips
            .iter()
            .filter(|ip| is_public_ip(ip))
            .map(|ip| ip.to_string())
            .collect();
        assert_eq!(public, vec!["93.184.216.34", "2606:4700:4700::1111"]);
    }

    #[test]
    fn a_client_named_base_must_be_public() {
        for bad in [
            "http://127.0.0.1:9/v1",
            "http://localhost/v1",
            "https://169.254.169.254/latest",
            "http://[::1]/v1",
            "http://10.0.0.5:8000",
            "ftp://files.example.com",
            "not a url",
        ] {
            assert!(check_untrusted_base(bad, false).is_err(), "{bad}");
        }
        for good in ["https://api.example.com/v1", "http://asr.example.com:8000"] {
            assert!(check_untrusted_base(good, false).is_ok(), "{good}");
        }
        assert!(
            check_untrusted_base("http://127.0.0.1:9/v1", true).is_ok(),
            "the operator's escape hatch"
        );
        assert!(
            check_untrusted_base("ftp://x.example.com", true).is_err(),
            "never another scheme"
        );
    }

    #[test]
    fn listed_hosts_get_the_http2_client_and_nothing_else_does() {
        let s = HttpSettings {
            http2_hosts: vec!["api.groq.com".into()],
            ..Default::default()
        };
        let clients = UploadClients::new(&s).unwrap();
        assert!(s.wants_http2("https://api.groq.com/openai/v1/audio/transcriptions"));
        assert!(!s.wants_http2("https://api.openai.com/v1/audio/transcriptions"));
        let h1 = clients.client(false) as *const reqwest::Client;
        let h2 = clients.client_for("https://api.groq.com/x", false) as *const reqwest::Client;
        assert_ne!(h2, h1, "the HTTP/2 twin is its own pool");
        let other = clients.client_for("https://api.openai.com/x", false) as *const reqwest::Client;
        assert_eq!(other, h1);
        assert_ne!(
            clients.client(true) as *const reqwest::Client,
            h1,
            "trusted is its own pool"
        );
        let plain = UploadClients::new(&HttpSettings::default()).unwrap();
        assert_eq!(
            plain.client_for("https://api.groq.com/x", true) as *const reqwest::Client,
            plain.client(true) as *const reqwest::Client
        );
    }

    type Seen = Arc<Mutex<Vec<(String, SocketAddr)>>>;

    /// A server that records the peer address of every request; one peer is one connection.
    async fn counting_server() -> (SocketAddr, Seen) {
        let seen: Seen = Arc::default();
        let record = seen.clone();
        let app = axum::Router::new().fallback(
            move |ConnectInfo(peer): ConnectInfo<SocketAddr>, req: axum::extract::Request| {
                let record = record.clone();
                async move {
                    record.lock().push((req.method().to_string(), peer));
                    // Hold each answer briefly so concurrent requests overlap on the server too.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    "ok"
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        (addr, seen)
    }

    #[tokio::test]
    async fn warm_opens_that_many_connections_and_uploads_reuse_them() {
        let (addr, seen) = counting_server().await;
        let clients = UploadClients::new(&HttpSettings::default()).unwrap();
        let client = clients.client(false);
        let url = format!("http://{addr}/v1/audio/transcriptions");

        assert_eq!(warm(client, &url, 3).await, 3);
        let warmed: HashSet<SocketAddr> = seen.lock().iter().map(|(_, p)| *p).collect();
        assert_eq!(
            warmed.len(),
            3,
            "three overlapping requests need three connections"
        );
        assert!(seen.lock().iter().all(|(m, _)| m == "HEAD"));

        // Three uploads in flight together now find three warm connections.
        let posts = (0..3).map(|_| client.post(url.as_str()).body("x").send());
        for r in join_all(posts).await {
            assert!(r.unwrap().status().is_success());
        }
        let all: HashSet<SocketAddr> = seen.lock().iter().map(|(_, p)| *p).collect();
        assert_eq!(all, warmed, "no upload was first on its connection");
    }

    #[tokio::test]
    async fn warm_swallows_a_dead_host() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let clients = UploadClients::new(&HttpSettings::default()).unwrap();
        assert_eq!(
            warm(clients.client(false), &format!("http://{addr}/x"), 2).await,
            0
        );
        assert_eq!(warm(clients.client(false), "not a url", 2).await, 0);
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let app = axum::Router::new()
            .route(
                "/a",
                axum::routing::post(|| async { axum::response::Redirect::temporary("/b") }),
            )
            .route("/b", axum::routing::post(|| async { "followed" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let clients = UploadClients::new(&HttpSettings::default()).unwrap();
        let resp = clients
            .client(false)
            .post(format!("http://{addr}/a"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 307);
    }
}
