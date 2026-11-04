//! The front servers every gateway of a deployment shares (spec 029 §5.8, SF-1 to SF-9).
//!
//! Each session holds one *pair* of a front server's slots for its whole life
//! ([`front_model::Lane`]): the decisions' slot `2k` and the interims' `2k + 1`. A pair is
//! leased. With a Redis the gateways share (the interaction task Redis, else
//! `WAAV_REDIS_URL`), no two live sessions of any gateway hold the same pair (SF-1). A lease
//! lapses [`LEASE_TTL`] after its gateway stops renewing it, and a gateway that cannot renew a
//! lease stops using the pair (SF-2). Without a shared Redis the leases are this process's
//! alone, which is right for one gateway per server.
//!
//! The servers are the configured URLs (`front_model.url`, `front_model.urls`), resolved again
//! every [`RESOLVE_EVERY`]. A hostname with several addresses (a Kubernetes headless Service,
//! Consul, round-robin DNS) is one server per address (SF-3). A new server, or one back after
//! it was lost (a restart empties its cache), has every slot warmed one at a time before a
//! session is placed on it (SF-9). Each server's health is its own (SF-5). A session is placed
//! where the most pairs are free. With none free it runs without the mode (`front_busy`,
//! SF-4); with no server at all it gets `front_unavailable`, logged once per outage (SF-8).
//! The pairs held and free per server, and the sessions refused, are on `/metrics` (SF-7).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::task::AbortHandle;
use tokio::time::Instant;
use tracing::{info, warn};

use crate::core::metrics::bridge;

use super::config::{FrontModelServer, ServerInteraction};
use super::front_model;
use super::persona::Persona;

/// How long a pair stays leased after its gateway last renewed it (SF-2: a dead gateway's
/// pairs free within it).
pub const LEASE_TTL: Duration = Duration::from_secs(30);
/// How often a held pair's lease is renewed: three renewals fit in one [`LEASE_TTL`].
const RENEW_EVERY: Duration = Duration::from_secs(10);
/// How often the configured URLs are resolved again (SF-3, SF-8).
pub const RESOLVE_EVERY: Duration = Duration::from_secs(5);
/// How often each server's `GET /health` is read (SF-5). A restart reloads the model, during
/// which the server answers 503: a check this often sees it.
pub const HEALTH_EVERY: Duration = Duration::from_secs(2);
/// Failed checks in a row before a server is lost: one slow answer is not an outage.
const LOST_AFTER: u32 = 2;

/// Why a session gets no pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No server is ready (`front_unavailable`): none is named, none answers, or every one is
    /// still warming.
    Unavailable(String),
    /// Every ready server's pairs are held (`front_busy`).
    Busy(String),
}

impl Refusal {
    /// The `config_warning` reason.
    pub fn reason(&self) -> &'static str {
        match self {
            Refusal::Unavailable(_) => "front_unavailable",
            Refusal::Busy(_) => "front_busy",
        }
    }

    pub fn why(&self) -> &str {
        match self {
            Refusal::Unavailable(why) | Refusal::Busy(why) => why,
        }
    }
}

// ---- leases

/// Where the pairs' leases are kept: a Redis the gateways share, or this process.
#[async_trait]
pub trait LeaseStore: Send + Sync {
    /// The holders of `keys`, `None` for a free one.
    async fn holders(&self, keys: &[String]) -> Result<Vec<Option<String>>, String>;
    /// Take `key` for `holder` for `ttl` when nobody holds it; `false` when somebody does.
    async fn take(&self, key: &str, holder: &str, ttl: Duration) -> Result<bool, String>;
    /// Extend `holder`'s lease of `key`; `false` when `holder` no longer holds it.
    async fn renew(&self, key: &str, holder: &str, ttl: Duration) -> Result<bool, String>;
    /// Give `key` back, when `holder` still holds it.
    async fn release(&self, key: &str, holder: &str) -> Result<(), String>;
}

/// Leases in this process: one gateway per server.
#[derive(Default)]
pub struct MemoryLeases(Mutex<HashMap<String, (String, Instant)>>);

impl MemoryLeases {
    fn live(&self, key: &str) -> Option<String> {
        let mut leases = self.0.lock();
        match leases.get(key) {
            Some((holder, until)) if *until > Instant::now() => Some(holder.clone()),
            Some(_) => {
                leases.remove(key);
                None
            }
            None => None,
        }
    }
}

#[async_trait]
impl LeaseStore for MemoryLeases {
    async fn holders(&self, keys: &[String]) -> Result<Vec<Option<String>>, String> {
        Ok(keys.iter().map(|key| self.live(key)).collect())
    }

    async fn take(&self, key: &str, holder: &str, ttl: Duration) -> Result<bool, String> {
        if self.live(key).is_some() {
            return Ok(false);
        }
        let until = Instant::now() + ttl;
        self.0
            .lock()
            .insert(key.to_owned(), (holder.to_owned(), until));
        Ok(true)
    }

    async fn renew(&self, key: &str, holder: &str, ttl: Duration) -> Result<bool, String> {
        if self.live(key).as_deref() != Some(holder) {
            return Ok(false);
        }
        let until = Instant::now() + ttl;
        self.0
            .lock()
            .insert(key.to_owned(), (holder.to_owned(), until));
        Ok(true)
    }

    async fn release(&self, key: &str, holder: &str) -> Result<(), String> {
        let mut leases = self.0.lock();
        if leases.get(key).is_some_and(|(held, _)| held == holder) {
            leases.remove(key);
        }
        Ok(())
    }
}

/// Extends the lease only for the holder that has it.
const RENEW: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 0
";

/// Gives the lease back only for the holder that has it.
const RELEASE: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
";

/// Leases in a Redis the gateways share. It connects when first used and again after a
/// failure: a Redis that is down refuses placements rather than leaving every gateway to
/// lease alone.
pub struct RedisLeases {
    client: redis::Client,
    conn: tokio::sync::Mutex<Option<redis::aio::ConnectionManager>>,
}

impl RedisLeases {
    pub fn new(url: &str) -> Result<Self, String> {
        let client =
            redis::Client::open(url).map_err(|e| format!("invalid front lease Redis URL: {e}"))?;
        Ok(Self {
            client,
            conn: tokio::sync::Mutex::new(None),
        })
    }

    async fn conn(&self) -> Result<redis::aio::ConnectionManager, String> {
        let mut conn = self.conn.lock().await;
        if let Some(conn) = conn.as_ref() {
            return Ok(conn.clone());
        }
        let made = tokio::time::timeout(
            Duration::from_secs(2),
            redis::aio::ConnectionManager::new(self.client.clone()),
        )
        .await
        .map_err(|_| "the front lease Redis did not answer in 2 s".to_owned())?
        .map_err(|e| format!("the front lease Redis is unreachable: {e}"))?;
        *conn = Some(made.clone());
        Ok(made)
    }

    async fn query<T: redis::FromRedisValue>(&self, cmd: &redis::Cmd) -> Result<T, String> {
        let mut conn = self.conn().await?;
        cmd.query_async(&mut conn)
            .await
            .map_err(|e| format!("front lease Redis: {e}"))
    }
}

#[async_trait]
impl LeaseStore for RedisLeases {
    async fn holders(&self, keys: &[String]) -> Result<Vec<Option<String>>, String> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut cmd = redis::cmd("MGET");
        cmd.arg(keys);
        self.query(&cmd).await
    }

    async fn take(&self, key: &str, holder: &str, ttl: Duration) -> Result<bool, String> {
        let mut cmd = redis::cmd("SET");
        cmd.arg(key)
            .arg(holder)
            .arg("NX")
            .arg("PX")
            .arg(ttl.as_millis() as u64);
        let set: Option<String> = self.query(&cmd).await?;
        Ok(set.is_some())
    }

    async fn renew(&self, key: &str, holder: &str, ttl: Duration) -> Result<bool, String> {
        let mut cmd = redis::cmd("EVAL");
        cmd.arg(RENEW)
            .arg(1)
            .arg(key)
            .arg(holder)
            .arg(ttl.as_millis() as u64);
        let renewed: i64 = self.query(&cmd).await?;
        Ok(renewed == 1)
    }

    async fn release(&self, key: &str, holder: &str) -> Result<(), String> {
        let mut cmd = redis::cmd("EVAL");
        cmd.arg(RELEASE).arg(1).arg(key).arg(holder);
        let _: i64 = self.query(&cmd).await?;
        Ok(())
    }
}

/// A pair's lease key: the server's address and the pair.
fn lease_key(url: &str, pair: usize) -> String {
    format!("waav:front:lease:{url}:{pair}")
}

// ---- servers

/// What the pool asks the name service and the front servers. A trait so the pool's rules are
/// tested without a network ([`Http`] is the real one).
#[async_trait]
pub trait Servers: Send + Sync {
    /// The servers `origin` names now: itself when its host is an address, else one per
    /// address its hostname resolves to (none when it resolves to nothing).
    async fn resolve(&self, origin: &str) -> Vec<String>;
    /// Whether the server answers `GET /health` with a 2xx.
    async fn healthy(&self, url: &str) -> bool;
    /// The server's slots (`GET /props` `total_slots`), or why it cannot be the front.
    async fn slots(&self, url: &str) -> Result<usize, String>;
    /// Put the warm-up persona's prompt in slot `slot`'s cache; `false` when it failed.
    async fn warm(&self, url: &str, slot: usize, slots: usize) -> bool;
}

/// The front servers over HTTP, and their names in DNS.
pub struct Http {
    base: FrontModelServer,
    /// The persona whose prompt every slot is warmed with: the one the server's default
    /// interaction blocks name.
    persona: Arc<Persona>,
}

impl Http {
    pub fn new(base: FrontModelServer, persona: Arc<Persona>) -> Self {
        Self { base, persona }
    }

    fn server(&self, url: &str) -> FrontModelServer {
        FrontModelServer {
            url: url.to_owned(),
            ..self.base.clone()
        }
    }
}

#[async_trait]
impl Servers for Http {
    async fn resolve(&self, origin: &str) -> Vec<String> {
        let origin = origin.trim().trim_end_matches('/');
        let Ok(parsed) = reqwest::Url::parse(origin) else {
            return Vec::new();
        };
        let Some(host) = parsed.host_str() else {
            return Vec::new();
        };
        // An address is one server; a certificate names a host, so an HTTPS name stays one.
        let literal = matches!(parsed.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)));
        if literal || parsed.scheme() == "https" {
            return vec![origin.to_owned()];
        }
        let Some(port) = parsed.port_or_known_default() else {
            return Vec::new();
        };
        let Ok(found) = tokio::net::lookup_host((host, port)).await else {
            return Vec::new();
        };
        let mut servers: Vec<String> = found
            .map(|address| {
                let mut url = parsed.clone();
                let _ = url.set_ip_host(address.ip());
                url.as_str().trim_end_matches('/').to_owned()
            })
            .collect();
        servers.sort();
        servers.dedup();
        servers
    }

    async fn healthy(&self, url: &str) -> bool {
        front_model::answers_health(&self.server(url), HEALTH_TIMEOUT).await
    }

    async fn slots(&self, url: &str) -> Result<usize, String> {
        front_model::server_slots(&self.server(url)).await
    }

    async fn warm(&self, url: &str, slot: usize, slots: usize) -> bool {
        front_model::warm_slot(&self.server(url), &self.persona, slot, slots).await
    }
}

/// How long one health check may take.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(1);

/// Where one server stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// New, or back after it was lost: its slots are being warmed (SF-9).
    Warming,
    /// Every slot warm: sessions are placed on it.
    Ready,
    /// Not answering: its sessions take a pair elsewhere (SF-5).
    Lost,
}

struct Server {
    slots: usize,
    state: ServerState,
    failures: u32,
    /// Bumped each time the server starts warming: a placement of an earlier generation
    /// lost the slots it was primed in.
    generation: u64,
    /// A warm-up runs for it.
    warming: bool,
    /// Its pairs held by any gateway, at the last count.
    held: usize,
}

impl Server {
    fn pairs(&self) -> usize {
        (self.slots / 2).max(1)
    }
}

struct Inner {
    base: FrontModelServer,
    origins: Vec<String>,
    /// This process, in every lease it holds.
    gateway: String,
    leases: Arc<dyn LeaseStore>,
    servers: Arc<dyn Servers>,
    known: Mutex<BTreeMap<String, Server>>,
    /// The URLs named no server at the last resolution, and that was logged (SF-8).
    empty_logged: AtomicBool,
    warming: Mutex<HashMap<String, AbortHandle>>,
    ttl: Duration,
    renew_every: Duration,
}

/// The front servers this gateway places its sessions on.
#[derive(Clone)]
pub struct FrontPool {
    inner: Arc<Inner>,
}

impl FrontPool {
    pub fn new(
        base: FrontModelServer,
        leases: Arc<dyn LeaseStore>,
        servers: Arc<dyn Servers>,
    ) -> Self {
        let mut origins: Vec<String> = std::iter::once(base.url.clone())
            .chain(base.urls.iter().cloned())
            .map(|url| url.trim().trim_end_matches('/').to_owned())
            .filter(|url| !url.is_empty())
            .collect();
        origins.dedup();
        Self {
            inner: Arc::new(Inner {
                base,
                origins,
                gateway: uuid::Uuid::new_v4().simple().to_string(),
                leases,
                servers,
                known: Mutex::new(BTreeMap::new()),
                empty_logged: AtomicBool::new(false),
                warming: Mutex::new(HashMap::new()),
                ttl: LEASE_TTL,
                renew_every: RENEW_EVERY,
            }),
        }
    }

    /// A pool whose leases last `ttl` and are renewed every `renew_every` (tests).
    pub fn with_lease_times(self, ttl: Duration, renew_every: Duration) -> Self {
        let inner = Arc::into_inner(self.inner).expect("a pool not yet shared");
        Self {
            inner: Arc::new(Inner {
                ttl,
                renew_every,
                ..inner
            }),
        }
    }

    /// One resolution of the configured URLs: servers that appear start warming, servers
    /// that are gone are dropped (their sessions' placements are lost).
    pub async fn resolve(&self) {
        let mut found = Vec::new();
        for origin in &self.inner.origins {
            found.extend(self.inner.servers.resolve(origin).await);
        }
        found.sort();
        found.dedup();
        let mut known = self.inner.known.lock();
        known.retain(|url, _| {
            let kept = found.contains(url);
            if !kept {
                info!(server = %url, "interaction front server gone; its sessions take a pair elsewhere");
            }
            kept
        });
        for url in &found {
            known.entry(url.clone()).or_insert_with(|| {
                info!(server = %url, "interaction front server found");
                Server {
                    slots: 1,
                    state: ServerState::Warming,
                    failures: 0,
                    generation: 1,
                    warming: false,
                    held: 0,
                }
            });
        }
        // SF-8: an empty pool is a state, said once.
        if found.is_empty() {
            if !self.inner.empty_logged.swap(true, Ordering::Relaxed) {
                warn!(
                    urls = ?self.inner.origins,
                    "interaction front: the front model URLs name no server; sessions run \
                     without the mode until one appears"
                );
            }
        } else if self.inner.empty_logged.swap(false, Ordering::Relaxed) {
            info!(servers = found.len(), "interaction front servers appeared");
        }
    }

    /// One health check of every server (SF-5): a server that stops answering is lost, one
    /// that answers again warms again.
    pub async fn check(&self) {
        let urls: Vec<String> = self.inner.known.lock().keys().cloned().collect();
        let checks = urls.iter().map(|url| self.inner.servers.healthy(url));
        let answers = futures::future::join_all(checks).await;
        let mut known = self.inner.known.lock();
        for (url, healthy) in urls.iter().zip(answers) {
            let Some(server) = known.get_mut(url) else {
                continue;
            };
            match (server.state, healthy) {
                (ServerState::Lost, true) => {
                    info!(server = %url, "interaction front server answers again; warming it");
                    server.state = ServerState::Warming;
                    server.generation += 1;
                    server.failures = 0;
                }
                (_, true) => server.failures = 0,
                (ServerState::Lost, false) => {}
                (_, false) => {
                    server.failures += 1;
                    if server.failures >= LOST_AFTER {
                        warn!(server = %url, "interaction front server lost");
                        server.state = ServerState::Lost;
                        server.generation += 1;
                        if let Some(warming) = self.inner.warming.lock().remove(url) {
                            warming.abort();
                        }
                        server.warming = false;
                    }
                }
            }
        }
    }

    /// Warm every server that is warming, each slot in turn (SF-9); servers in parallel.
    pub async fn warm(&self) {
        let pending: Vec<(String, u64)> = {
            let mut known = self.inner.known.lock();
            known
                .iter_mut()
                .filter(|(_, server)| server.state == ServerState::Warming && !server.warming)
                .map(|(url, server)| {
                    server.warming = true;
                    (url.clone(), server.generation)
                })
                .collect()
        };
        let warms = pending
            .into_iter()
            .map(|(url, generation)| self.warm_server(url, generation));
        futures::future::join_all(warms).await;
    }

    async fn warm_server(&self, url: String, generation: u64) {
        let warmed = self.warm_slots(&url).await;
        let mut known = self.inner.known.lock();
        let Some(server) = known
            .get_mut(&url)
            .filter(|server| server.generation == generation)
        else {
            return;
        };
        server.warming = false;
        match warmed {
            Ok(slots) => {
                server.slots = slots;
                server.state = ServerState::Ready;
                info!(server = %url, slots, pairs = server.pairs(), "interaction front server ready");
            }
            // Tried again at the next round.
            Err(why) => warn!(server = %url, %why, "interaction front server not warmed"),
        }
    }

    async fn warm_slots(&self, url: &str) -> Result<usize, String> {
        let slots = self.inner.servers.slots(url).await?;
        // One at a time: on a CPU front, slots warmed together each took the whole budget
        // and none finished (IM0, 4 sessions on a cold server: 12 of 12 turns missed).
        for slot in 0..slots {
            if !self.inner.servers.warm(url, slot, slots).await {
                return Err(format!("slot {slot} did not warm"));
            }
        }
        Ok(slots)
    }

    /// Count the pairs held on every server by any gateway, for `/metrics` and the log
    /// (SF-7).
    pub async fn count(&self) {
        let servers: Vec<(String, usize)> = {
            let known = self.inner.known.lock();
            (known.iter())
                .filter(|(_, server)| server.state == ServerState::Ready)
                .map(|(url, server)| (url.clone(), server.pairs()))
                .collect()
        };
        let keys: Vec<String> = servers
            .iter()
            .flat_map(|(url, pairs)| (0..*pairs).map(|pair| lease_key(url, pair)))
            .collect();
        let Ok(holders) = self.inner.leases.holders(&keys).await else {
            return;
        };
        let mut holders = holders.into_iter();
        let mut known = self.inner.known.lock();
        for (url, pairs) in servers {
            let held = holders.by_ref().take(pairs).flatten().count();
            if let Some(server) = known.get_mut(&url) {
                if server.held != held {
                    info!(server = %url, held, pairs, "interaction front pairs held");
                }
                server.held = held;
            }
            bridge::set_front_pairs(&url, held, pairs - held);
        }
    }

    /// The pool's own loop: resolve, check, warm and count, for the life of the process.
    pub fn start(&self) {
        let pool = self.clone();
        tokio::spawn(async move {
            let mut resolved: Option<std::time::Instant> = None;
            loop {
                if resolved.is_none_or(|at| at.elapsed() >= RESOLVE_EVERY) {
                    pool.resolve().await;
                    resolved = Some(std::time::Instant::now());
                }
                pool.check().await;
                pool.spawn_warm();
                pool.count().await;
                tokio::time::sleep(HEALTH_EVERY).await;
            }
        });
    }

    /// Warm what is warming in the background: a warm-up of several seconds does not hold up
    /// the health checks.
    fn spawn_warm(&self) {
        let pending: Vec<(String, u64)> = {
            let mut known = self.inner.known.lock();
            known
                .iter_mut()
                .filter(|(_, server)| server.state == ServerState::Warming && !server.warming)
                .map(|(url, server)| {
                    server.warming = true;
                    (url.clone(), server.generation)
                })
                .collect()
        };
        for (url, generation) in pending {
            let pool = self.clone();
            let key = url.clone();
            let task = tokio::spawn(async move { pool.warm_server(url, generation).await });
            self.inner.warming.lock().insert(key, task.abort_handle());
        }
    }

    /// A pair for `session` on the ready server with the most pairs free (SF-3, SF-4).
    pub async fn place(&self, session: &str) -> Result<Placement, Refusal> {
        let (ready, any) = {
            let known = self.inner.known.lock();
            let ready: Vec<(String, usize, u64)> = (known.iter())
                .filter(|(_, server)| server.state == ServerState::Ready)
                .map(|(url, server)| (url.clone(), server.pairs(), server.generation))
                .collect();
            (ready, !known.is_empty())
        };
        if ready.is_empty() {
            let why = if any {
                "no front server is ready (each is warming or not answering)"
            } else {
                "the front model's URLs name no server"
            };
            bridge::record_front_refused("front_unavailable");
            return Err(Refusal::Unavailable(why.to_owned()));
        }
        let keys: Vec<String> = ready
            .iter()
            .flat_map(|(url, pairs, _)| (0..*pairs).map(|pair| lease_key(url, pair)))
            .collect();
        let holders = match self.inner.leases.holders(&keys).await {
            Ok(holders) => holders,
            Err(why) => {
                bridge::record_front_refused("front_unavailable");
                return Err(Refusal::Unavailable(why));
            }
        };
        let mut holders = holders.into_iter();
        let mut free: Vec<(String, u64, Vec<usize>)> = ready
            .into_iter()
            .map(|(url, pairs, generation)| {
                let open = (0..pairs)
                    .zip(holders.by_ref().take(pairs))
                    .filter(|(_, holder)| holder.is_none())
                    .map(|(pair, _)| pair)
                    .collect();
                (url, generation, open)
            })
            .collect();
        // The most pairs free first; the address breaks a tie, so gateways agree.
        free.sort_by(|a, b| b.2.len().cmp(&a.2.len()).then_with(|| a.0.cmp(&b.0)));
        let holder = format!("{}/{session}", self.inner.gateway);
        for (url, generation, pairs) in free {
            for pair in pairs {
                let key = lease_key(&url, pair);
                match self.inner.leases.take(&key, &holder, self.inner.ttl).await {
                    Ok(true) => return Ok(self.placement(url, pair, generation, key, holder)),
                    // Another gateway took it since the count.
                    Ok(false) => continue,
                    Err(why) => {
                        bridge::record_front_refused("front_unavailable");
                        return Err(Refusal::Unavailable(why));
                    }
                }
            }
        }
        bridge::record_front_refused("front_busy");
        Err(Refusal::Busy(
            "every slot pair of the front servers is held by a live session".to_owned(),
        ))
    }

    fn placement(
        &self,
        url: String,
        pair: usize,
        generation: u64,
        key: String,
        holder: String,
    ) -> Placement {
        let lost = Arc::new(AtomicBool::new(false));
        let renewing = {
            let (leases, key, holder) =
                (Arc::clone(&self.inner.leases), key.clone(), holder.clone());
            let (lost, ttl, every) = (Arc::clone(&lost), self.inner.ttl, self.inner.renew_every);
            tokio::spawn(async move {
                let mut proven = Instant::now();
                loop {
                    tokio::time::sleep(every).await;
                    match leases.renew(&key, &holder, ttl).await {
                        Ok(true) => proven = Instant::now(),
                        // The lease lapsed and another session may hold the pair now.
                        Ok(false) => break,
                        // Unproven past the lease's life: another may hold it (SF-2).
                        Err(_) if proven.elapsed() >= ttl => break,
                        Err(_) => {}
                    }
                }
                lost.store(true, Ordering::Relaxed);
            })
            .abort_handle()
        };
        let server = FrontModelServer {
            url: url.clone(),
            ..self.inner.base.clone()
        };
        Placement {
            server,
            url,
            pair,
            generation,
            key,
            holder,
            lost,
            renewing,
            pool: Arc::clone(&self.inner),
        }
    }

    /// The state of each known server, for tests and the log.
    pub fn servers(&self) -> Vec<(String, ServerState, usize)> {
        let known = self.inner.known.lock();
        (known.iter())
            .map(|(url, server)| (url.clone(), server.state, server.pairs()))
            .collect()
    }
}

/// A session's pair on one front server, held until it is dropped.
pub struct Placement {
    server: FrontModelServer,
    url: String,
    pair: usize,
    generation: u64,
    key: String,
    holder: String,
    lost: Arc<AtomicBool>,
    renewing: AbortHandle,
    pool: Arc<Inner>,
}

impl std::fmt::Debug for Placement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Placement")
            .field("url", &self.url)
            .field("pair", &self.pair)
            .field("lost", &self.lost())
            .finish()
    }
}

impl Placement {
    /// The front server, at the address the session is placed on.
    pub fn server(&self) -> &FrontModelServer {
        &self.server
    }

    /// The pair: the session's [`front_model::Lane`].
    pub fn pair(&self) -> usize {
        self.pair
    }

    /// The session must stop using the pair: its lease could not be proven (SF-2), or the
    /// server was lost, restarted or is gone (SF-5).
    pub fn lost(&self) -> bool {
        if self.lost.load(Ordering::Relaxed) {
            return true;
        }
        let known = self.pool.known.lock();
        known.get(&self.url).is_none_or(|server| {
            server.state != ServerState::Ready || server.generation != self.generation
        })
    }
}

impl Drop for Placement {
    fn drop(&mut self) {
        self.renewing.abort();
        let (leases, key, holder) = (
            Arc::clone(&self.pool.leases),
            std::mem::take(&mut self.key),
            std::mem::take(&mut self.holder),
        );
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = leases.release(&key, &holder).await;
            });
        }
    }
}

// ---- the process's pool

static POOL: OnceLock<FrontPool> = OnceLock::new();

/// The persona every slot is warmed with (SF-9): the one the server's default interaction
/// blocks name (`agent_default`, then `default`, then `realtime_default`), else the packaged
/// default. A session of that persona finds its prompt in the slot's cache.
pub fn warm_persona(server: &ServerInteraction) -> Arc<Persona> {
    let named = [
        &server.agent_default,
        &server.default,
        &server.realtime_default,
    ]
    .into_iter()
    .flatten()
    .find_map(|block| block["front"]["persona"].as_str().map(str::to_owned));
    Persona::load_in(named.as_deref().unwrap_or("default"), "en")
}

/// The pool sessions are placed from, once [`install`] ran: every front model the gateway
/// does not run itself.
pub fn get() -> Option<&'static FrontPool> {
    POOL.get()
}

/// Start the process's pool for `front`. Its leases are in `redis_url` when there is one
/// (every gateway of the deployment uses the same), else in this process.
pub fn install(
    front: &FrontModelServer,
    redis_url: Option<&str>,
    persona: Arc<Persona>,
) -> Result<(), String> {
    let leases: Arc<dyn LeaseStore> = match redis_url {
        Some(url) => Arc::new(RedisLeases::new(url)?),
        None => Arc::new(MemoryLeases::default()),
    };
    let servers = Arc::new(Http::new(front.clone(), persona));
    let pool = FrontPool::new(front.clone(), leases, servers);
    pool.start();
    POOL.set(pool)
        .map_err(|_| "the front pool is already installed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Front servers and a name service in memory: which names resolve to what, which servers
    /// answer, and every warm-up in the order it ran.
    #[derive(Default)]
    struct Fake {
        names: Mutex<HashMap<String, Vec<String>>>,
        down: Mutex<Vec<String>>,
        slots: Mutex<HashMap<String, usize>>,
        warmed: Mutex<Vec<(String, usize)>>,
        warming_now: AtomicUsize,
        most_at_once: AtomicUsize,
    }

    impl Fake {
        fn name(&self, origin: &str, servers: &[&str]) {
            let servers = servers.iter().map(|s| (*s).to_owned()).collect();
            self.names.lock().insert(origin.to_owned(), servers);
        }

        fn serve(&self, url: &str, slots: usize) {
            self.slots.lock().insert(url.to_owned(), slots);
        }

        fn set_down(&self, url: &str, down: bool) {
            let mut list = self.down.lock();
            list.retain(|known| known != url);
            if down {
                list.push(url.to_owned());
            }
        }
    }

    #[async_trait]
    impl Servers for Fake {
        async fn resolve(&self, origin: &str) -> Vec<String> {
            self.names.lock().get(origin).cloned().unwrap_or_default()
        }

        async fn healthy(&self, url: &str) -> bool {
            !self.down.lock().iter().any(|down| down == url)
        }

        async fn slots(&self, url: &str) -> Result<usize, String> {
            self.slots
                .lock()
                .get(url)
                .copied()
                .ok_or_else(|| "no answer".to_owned())
        }

        async fn warm(&self, url: &str, slot: usize, _slots: usize) -> bool {
            let now = self.warming_now.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_at_once.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.warming_now.fetch_sub(1, Ordering::SeqCst);
            self.warmed.lock().push((url.to_owned(), slot));
            true
        }
    }

    const NAME: &str = "http://front.ns.svc:8080";
    const A: &str = "http://10.0.0.1:8080";
    const B: &str = "http://10.0.0.2:8080";

    fn front() -> FrontModelServer {
        FrontModelServer {
            url: NAME.to_owned(),
            ..FrontModelServer::default()
        }
    }

    async fn ready(pool: &FrontPool) {
        pool.resolve().await;
        pool.check().await;
        pool.warm().await;
    }

    /// SF-1: two gateways on one lease store never hold the same pair, and a pair given back
    /// is taken again.
    #[tokio::test]
    async fn two_gateways_never_hold_the_same_pair() {
        let fake = Arc::new(Fake::default());
        fake.name(NAME, &[A]);
        fake.serve(A, 4);
        let leases: Arc<dyn LeaseStore> = Arc::new(MemoryLeases::default());
        let one = FrontPool::new(front(), Arc::clone(&leases), fake.clone());
        let two = FrontPool::new(front(), Arc::clone(&leases), fake.clone());
        ready(&one).await;
        ready(&two).await;
        let first = one.place("s1").await.expect("a pair");
        let second = two.place("s2").await.expect("a pair");
        assert_eq!(first.server().url, A);
        assert_ne!(first.pair(), second.pair());
        let full = two.place("s3").await.expect_err("both pairs held");
        assert_eq!(full.reason(), "front_busy");
        drop(first);
        tokio::task::yield_now().await;
        let third = two.place("s3").await.expect("the pair given back");
        assert_ne!(third.pair(), second.pair());
    }

    /// SF-2: a gateway that died holds its pairs no longer than a lease's life, and one that
    /// cannot renew its lease stops using the pair.
    #[tokio::test(start_paused = true)]
    async fn a_dead_gateways_pairs_free_within_the_lease_and_an_unproven_lease_is_lost() {
        let fake = Arc::new(Fake::default());
        fake.name(NAME, &[A]);
        fake.serve(A, 2);
        let leases: Arc<dyn LeaseStore> = Arc::new(MemoryLeases::default());
        let times = (Duration::from_secs(30), Duration::from_secs(10));
        let dead = FrontPool::new(front(), Arc::clone(&leases), fake.clone())
            .with_lease_times(times.0, times.1);
        let alive = FrontPool::new(front(), Arc::clone(&leases), fake.clone())
            .with_lease_times(times.0, times.1);
        ready(&dead).await;
        ready(&alive).await;
        let held = dead.place("s1").await.expect("the pair");
        // The process dies: nothing renews or gives back its lease.
        held.renewing.abort();
        std::mem::forget(held);
        assert_eq!(alive.place("s2").await.unwrap_err().reason(), "front_busy");
        tokio::time::advance(Duration::from_secs(31)).await;
        let taken = alive.place("s2").await.expect("freed by the lease's life");
        // Its renewal starts counting.
        tokio::task::yield_now().await;

        // Another holder has the pair (its lease lapsed while this gateway was stalled): the
        // next renewal says so and the session stops using it.
        leases.release(&taken.key, &taken.holder).await.unwrap();
        assert!(leases.take(&taken.key, "other/s9", times.0).await.unwrap());
        assert!(!taken.lost());
        tokio::time::advance(Duration::from_secs(11)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(taken.lost());
    }

    /// SF-3: one name with two addresses is two servers; a session goes where most pairs are
    /// free and stays there.
    #[tokio::test]
    async fn a_name_with_several_addresses_is_one_server_each_and_places_where_most_are_free() {
        let fake = Arc::new(Fake::default());
        fake.name(NAME, &[A, B]);
        fake.serve(A, 4);
        fake.serve(B, 8);
        let pool = FrontPool::new(front(), Arc::new(MemoryLeases::default()), fake.clone());
        ready(&pool).await;
        let pairs: Vec<usize> = pool.servers().iter().map(|(.., pairs)| *pairs).collect();
        assert_eq!(pairs, [2, 4]);
        let mut held = Vec::new();
        for session in 0..4 {
            held.push(pool.place(&format!("s{session}")).await.unwrap());
        }
        let on = |url: &str| held.iter().filter(|p| p.server().url == url).count();
        // B (4 free) takes the first two, then both have 2 free and A wins the tie.
        assert_eq!((on(A), on(B)), (1, 3));
        assert!(held.iter().all(|placement| !placement.lost()));
    }

    /// SF-5: a server that stops answering is lost alone; its sessions take a pair on another
    /// one; back, it is warmed again before it takes a session.
    #[tokio::test]
    async fn a_lost_server_loses_only_its_own_sessions_and_is_warmed_again_when_back() {
        let fake = Arc::new(Fake::default());
        fake.name(NAME, &[A, B]);
        fake.serve(A, 2);
        fake.serve(B, 2);
        let pool = FrontPool::new(front(), Arc::new(MemoryLeases::default()), fake.clone());
        ready(&pool).await;
        let on_a = pool.place("s1").await.unwrap();
        let on_b = pool.place("s2").await.unwrap();
        assert_eq!(
            (on_a.server().url.as_str(), on_b.server().url.as_str()),
            (A, B)
        );
        fake.set_down(A, true);
        pool.check().await;
        assert!(!on_a.lost(), "one failed check is not an outage");
        pool.check().await;
        assert!(on_a.lost() && !on_b.lost());
        // The lost server's session would take a pair elsewhere: B's is held.
        drop(on_a);
        tokio::task::yield_now().await;
        assert_eq!(pool.place("s1").await.unwrap_err().reason(), "front_busy");
        // A restarts: warmed again, then placeable.
        fake.set_down(A, false);
        fake.warmed.lock().clear();
        pool.check().await;
        // Still warming: not placed on it, and B's pair is held.
        assert_eq!(pool.place("s1").await.unwrap_err().reason(), "front_busy");
        pool.warm().await;
        assert_eq!(*fake.warmed.lock(), [(A.to_owned(), 0), (A.to_owned(), 1)]);
        assert_eq!(pool.place("s1").await.unwrap().server().url, A);
    }

    /// SF-8: a name with no addresses refuses with `front_unavailable`, and a server that
    /// appears later is taken without a restart.
    #[tokio::test]
    async fn an_empty_pool_refuses_and_picks_up_a_server_that_appears() {
        let fake = Arc::new(Fake::default());
        let pool = FrontPool::new(front(), Arc::new(MemoryLeases::default()), fake.clone());
        ready(&pool).await;
        let refused = pool.place("s1").await.unwrap_err();
        assert_eq!(refused.reason(), "front_unavailable");
        assert!(refused.why().contains("name no server"), "{refused:?}");
        assert!(pool.inner.empty_logged.load(Ordering::Relaxed));
        fake.name(NAME, &[A]);
        fake.serve(A, 2);
        ready(&pool).await;
        assert!(!pool.inner.empty_logged.load(Ordering::Relaxed));
        let held = pool.place("s1").await.expect("the server that appeared");
        assert_eq!(pool.place("s2").await.unwrap_err().reason(), "front_busy");
        // A server that is gone loses its sessions.
        fake.name(NAME, &[]);
        pool.resolve().await;
        assert!(pool.servers().is_empty());
        assert!(held.lost());
    }

    /// SF-9: every slot of a new server is warmed, one at a time, before a session is
    /// placed on it.
    #[tokio::test]
    async fn every_slot_is_warmed_one_at_a_time_before_a_session_is_placed() {
        let fake = Arc::new(Fake::default());
        fake.name(NAME, &[A, B]);
        fake.serve(A, 4);
        fake.serve(B, 4);
        let pool = FrontPool::new(front(), Arc::new(MemoryLeases::default()), fake.clone());
        pool.resolve().await;
        pool.check().await;
        let cold = pool.place("s1").await.unwrap_err();
        assert_eq!(cold.reason(), "front_unavailable");
        pool.warm().await;
        let warmed = fake.warmed.lock().clone();
        for url in [A, B] {
            let slots: Vec<usize> = (warmed.iter())
                .filter(|(at, _)| at == url)
                .map(|(_, slot)| *slot)
                .collect();
            assert_eq!(slots, [0, 1, 2, 3], "{url}");
        }
        // One slot of each server at a time: the two servers warm side by side.
        assert!(fake.most_at_once.load(Ordering::SeqCst) <= 2);
        assert!(pool.place("s1").await.is_ok());
    }

    /// A server that cannot be the front (no slot count) is never placed on.
    #[tokio::test]
    async fn a_server_that_does_not_say_its_slots_is_never_ready() {
        let fake = Arc::new(Fake::default());
        fake.name(NAME, &[A]);
        let pool = FrontPool::new(front(), Arc::new(MemoryLeases::default()), fake.clone());
        ready(&pool).await;
        assert_eq!(pool.servers(), [(A.to_owned(), ServerState::Warming, 1)]);
        assert_eq!(
            pool.place("s1").await.unwrap_err().reason(),
            "front_unavailable"
        );
    }
}
