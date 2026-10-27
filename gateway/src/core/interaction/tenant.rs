//! What a tenant keeps across its sessions: its own personas, registered with
//! `PUT /v1/interaction/personas/{id}` (read and removed with `GET` and `DELETE`) and selected by
//! a session with `front.persona`, and what is known about its callers (caller memory), written
//! with `PUT /v1/interaction/memory/{subject}` or by the session's background after a call, and
//! read when a session with a signed caller identity starts. Both live in this process, and in
//! the task Redis when it is connected, so every process of a deployment sees them.
//!
//! A tenant is the authenticated id of the request (`handlers::tasks::owner`).

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use redis::AsyncCommands;
use serde_json::Value;
use sha2::Sha256;
use tracing::warn;

use super::persona::Persona;
use super::store;

/// The personas a tenant may keep, and the versions kept of each.
pub const PERSONAS_PER_TENANT: usize = 20;
pub const VERSIONS_KEPT: usize = 5;
/// What caller memory keeps about one caller (newest first), what a session reads of it, and
/// for how long.
pub const FACTS_KEPT: usize = 10;
pub const FACTS_READ: usize = 3;
pub const FACT_CHARS: usize = 80;
const MEMORY_KEPT: Duration = Duration::from_secs(90 * 24 * 3600);
/// The longest a caller identity token may be valid for.
pub const TOKEN_LIFETIME_S: u64 = 300;
/// The time a session start waits for the memory of its caller.
pub const MEMORY_READ_BUDGET: Duration = Duration::from_millis(150);

/// One persona's kept versions, oldest first.
type Versions = Vec<(u32, Value)>;

#[derive(Default)]
struct Local {
    personas: BTreeMap<(String, String), Versions>,
    memory: BTreeMap<String, (Vec<String>, Instant)>,
}

static LOCAL: Mutex<Local> = Mutex::new(Local {
    personas: BTreeMap::new(),
    memory: BTreeMap::new(),
});

fn local() -> std::sync::MutexGuard<'static, Local> {
    LOCAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn personas_key(tenant: &str) -> String {
    format!("waav:ix:personas:{tenant}")
}

fn memory_key(key: &str) -> String {
    format!("waav:ix:memory:{key}")
}

/// Why a persona was not registered.
#[derive(Debug, PartialEq)]
pub enum PutRefused {
    /// It does not pass the checks of a packaged persona; the text says what failed.
    Invalid(String),
    /// The tenant keeps `PERSONAS_PER_TENANT` personas already.
    TooMany,
}

/// Registers a new version of a tenant's persona. `spec` is a persona object; its `id` is the
/// one of the path. Returns the version. Concurrent registrations each get a version of their
/// own, and the cap holds: the check and the write are one step, in the task Redis (a script)
/// or in this process (one lock).
pub async fn put_persona(tenant: &str, id: &str, mut spec: Value) -> Result<u32, PutRefused> {
    let Some(fields) = spec.as_object_mut() else {
        return Err(PutRefused::Invalid("a persona is an object".to_owned()));
    };
    fields.insert("id".to_owned(), Value::String(id.to_owned()));
    Persona::from_spec(&spec).map_err(PutRefused::Invalid)?;
    if let Some(conn) = store::store().redis() {
        match put_shared(conn, tenant, id, &spec).await {
            Ok(Ok((version, versions))) => {
                // This process's copy is the fallback of a failed read.
                let key = (tenant.to_owned(), id.to_owned());
                local().personas.insert(key, versions);
                return Ok(version);
            }
            Ok(Err(refused)) => return Err(refused),
            Err(error) => warn!(%error, "a tenant persona was not written to the task Redis"),
        }
    }
    let mut local = local();
    let mine = |(owner, _): &&(String, String)| owner == tenant;
    let known = local
        .personas
        .contains_key(&(tenant.to_owned(), id.to_owned()));
    if !known && local.personas.keys().filter(mine).count() >= PERSONAS_PER_TENANT {
        return Err(PutRefused::TooMany);
    }
    let versions = local
        .personas
        .entry((tenant.to_owned(), id.to_owned()))
        .or_default();
    Ok(add_version(versions, spec))
}

/// Appends `spec` as the next version, keeping the newest [`VERSIONS_KEPT`].
fn add_version(versions: &mut Versions, spec: Value) -> u32 {
    let version = versions.last().map_or(1, |(last, _)| last + 1);
    versions.push((version, spec));
    let drop = versions.len().saturating_sub(VERSIONS_KEPT);
    versions.drain(..drop);
    version
}

/// Writes the persona's versions only if they are still what was read (`ARGV[2]`, empty for
/// none) and, for a new persona, only while the tenant has fewer than `ARGV[4]`: 1 written,
/// 0 changed meanwhile, -1 too many.
const PUT_PERSONA: &str = r"
local current = redis.call('HGET', KEYS[1], ARGV[1])
if ARGV[2] == '' then
  if current then return 0 end
  if redis.call('HLEN', KEYS[1]) >= tonumber(ARGV[4]) then return -1 end
elseif current ~= ARGV[2] then
  return 0
end
redis.call('HSET', KEYS[1], ARGV[1], ARGV[3])
return 1
";

/// How often a registration that met another one is tried again.
const PUT_TRIES: usize = 20;

/// [`put_persona`] in the task Redis: read, add the version, write if nothing changed
/// meanwhile, else again.
async fn put_shared(
    mut conn: redis::aio::ConnectionManager,
    tenant: &str,
    id: &str,
    spec: &Value,
) -> redis::RedisResult<Result<(u32, Versions), PutRefused>> {
    let key = personas_key(tenant);
    for _ in 0..PUT_TRIES {
        let current: Option<String> = conn.hget(&key, id).await?;
        let mut versions: Versions = current
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let version = add_version(&mut versions, spec.clone());
        let json = serde_json::to_string(&versions).unwrap_or_default();
        let written: i64 = redis::cmd("EVAL")
            .arg(PUT_PERSONA)
            .arg(1)
            .arg(&key)
            .arg(id)
            .arg(current.unwrap_or_default())
            .arg(json)
            .arg(PERSONAS_PER_TENANT)
            .query_async(&mut conn)
            .await?;
        match written {
            1 => return Ok(Ok((version, versions))),
            -1 => return Ok(Err(PutRefused::TooMany)),
            _ => continue,
        }
    }
    Err((redis::ErrorKind::TryAgain, "the persona kept changing").into())
}

/// The versions kept of one of a tenant's personas, oldest first; `None` when it has none.
pub async fn persona_versions(tenant: &str, id: &str) -> Option<Vec<(u32, Value)>> {
    read_personas(tenant).await.remove(id)
}

/// Removes every version of one of a tenant's personas: `true` when there was one. A session
/// that runs it keeps it to its end; a new one that selects it gets the default persona.
pub async fn delete_persona(tenant: &str, id: &str) -> Result<bool, String> {
    let here = local()
        .personas
        .remove(&(tenant.to_owned(), id.to_owned()))
        .is_some();
    let Some(mut conn) = store::store().redis() else {
        return Ok(here);
    };
    let removed: redis::RedisResult<usize> = conn.hdel(personas_key(tenant), id).await;
    removed
        .map(|removed| removed > 0)
        .map_err(|error| format!("the task Redis did not take it: {error}"))
}

/// A tenant's personas, from the task Redis when it is connected.
async fn read_personas(tenant: &str) -> BTreeMap<String, Versions> {
    if let Some(mut conn) = store::store().redis() {
        let read: redis::RedisResult<BTreeMap<String, String>> =
            conn.hgetall(personas_key(tenant)).await;
        match read {
            Ok(all) => {
                return all
                    .into_iter()
                    .filter_map(|(id, json)| Some((id, serde_json::from_str(&json).ok()?)))
                    .collect();
            }
            Err(error) => warn!(%error, "the tenant personas were not read from the task Redis"),
        }
    }
    let local = local();
    let mine = local
        .personas
        .iter()
        .filter(|((owner, _), _)| owner == tenant);
    mine.map(|((_, id), versions)| (id.clone(), versions.clone()))
        .collect()
}

/// The persona a session of `tenant` selects with `front.persona`: `id` (the newest version)
/// or `id@version`. `None` when the tenant has none by that name.
pub async fn persona(tenant: &str, selector: &str) -> Option<Value> {
    let (id, version) = match selector.split_once('@') {
        Some((id, version)) => (id, Some(version.parse::<u32>().ok()?)),
        None => (selector, None),
    };
    let all = read_personas(tenant).await;
    let versions = all.get(id)?;
    let found = match version {
        Some(version) => versions.iter().find(|(kept, _)| *kept == version),
        None => versions.last(),
    };
    found.map(|(_, spec)| spec.clone())
}

// ---- caller memory

/// The caller a token names, when the operator's `key` signed it for `tenant` and it is valid
/// now (`now` in unix seconds): `v1.<payload>.<mac>`, the payload the unpadded base64url of
/// `{"sub", "tenant", "exp"}`, the mac the unpadded base64url of HMAC-SHA-256 over
/// `v1.<payload>`. A token may be valid for at most five minutes.
pub fn verify(token: &str, key: &str, tenant: &str, now: u64) -> Option<String> {
    let rest = token.strip_prefix("v1.")?;
    let (payload, mac) = rest.split_once('.')?;
    let mut signer = Hmac::<Sha256>::new_from_slice(key.as_bytes()).ok()?;
    signer.update(format!("v1.{payload}").as_bytes());
    signer
        .verify_slice(&URL_SAFE_NO_PAD.decode(mac).ok()?)
        .ok()?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    let exp = claims["exp"].as_u64()?;
    let sub = claims["sub"].as_str().filter(|sub| !sub.is_empty())?;
    let valid = now < exp && exp <= now + TOKEN_LIFETIME_S && claims["tenant"] == tenant;
    valid.then(|| sub.to_owned())
}

/// A token as the operator mints it (the shape [`verify`] reads).
pub fn mint(key: &str, sub: &str, tenant: &str, exp: u64) -> String {
    let claims = serde_json::json!({ "sub": sub, "tenant": tenant, "exp": exp });
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let mut signer = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("any key length");
    signer.update(format!("v1.{payload}").as_bytes());
    let mac = URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes());
    format!("v1.{payload}.{mac}")
}

/// Where a caller's facts are kept: a keyed hash, so the store never holds the caller's id.
fn caller_key(key: &str, tenant: &str, sub: &str) -> String {
    let mut signer = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("any key length");
    signer.update(format!("{tenant}\n{sub}").as_bytes());
    hex::encode(signer.finalize().into_bytes())
}

/// Checks one fact to keep about a caller: 1 to 80 characters, with no digit, `@`, currency
/// sign or link (an account fact comes from the background, never from memory).
pub fn check_fact(fact: &str) -> Result<(), String> {
    let fact = fact.trim();
    let banned = |c: char| c.is_ascii_digit() || "@$£€¥#".contains(c);
    if fact.is_empty() || fact.chars().count() > FACT_CHARS {
        return Err(format!("{fact:?}: a fact is 1 to {FACT_CHARS} characters"));
    }
    if fact.chars().any(banned) || fact.contains("://") || fact.contains("www.") {
        return Err(format!(
            "{fact:?}: a fact holds no number, code, address, amount or link"
        ));
    }
    Ok(())
}

/// Checks the facts to keep about a caller: at most ten, each one by [`check_fact`].
pub fn check_facts(facts: &[String]) -> Result<(), String> {
    if facts.len() > FACTS_KEPT {
        return Err(format!("at most {FACTS_KEPT} facts"));
    }
    facts.iter().try_for_each(|fact| check_fact(fact))
}

/// Keeps what is known about a caller (replacing what was), for 90 days. An empty list
/// forgets the caller.
pub async fn remember(
    key: &str,
    tenant: &str,
    sub: &str,
    facts: Vec<String>,
) -> Result<(), String> {
    check_facts(&facts)?;
    let facts: Vec<String> = facts.iter().map(|fact| fact.trim().to_owned()).collect();
    // Forgetting a caller forgets the language they asked for too.
    if facts.is_empty() {
        store_facts(&language_key(key, tenant, sub), Vec::new()).await?;
    }
    store_facts(&caller_key(key, tenant, sub), facts).await
}

/// Adds what the background wrote after a call to what is kept about the caller: new facts
/// first, none twice (whatever the case), the oldest dropped beyond [`FACTS_KEPT`]. A fact the
/// memory may not keep is left out. Returns how many were added. Nothing is written when what
/// is kept could not be read.
pub async fn add_facts(
    key: &str,
    tenant: &str,
    sub: &str,
    facts: Vec<String>,
) -> Result<usize, String> {
    let id = caller_key(key, tenant, sub);
    let mut kept = kept_facts(&id, WRITE_READ_BUDGET).await?;
    let known = |fact: &str, all: &[String]| all.iter().any(|kept| kept.eq_ignore_ascii_case(fact));
    let mut added = Vec::new();
    for fact in facts.iter().map(|fact| fact.trim()) {
        if check_fact(fact).is_ok() && !known(fact, &kept) && !known(fact, &added) {
            added.push(fact.to_owned());
        }
    }
    if added.is_empty() {
        return Ok(0);
    }
    let count = added.len();
    added.append(&mut kept);
    added.truncate(FACTS_KEPT);
    store_facts(&id, added).await.map(|()| count)
}

/// The language a caller asked for (a `language` control), kept beside the caller's facts for
/// as long: the next call with the same identity starts in it.
pub async fn remember_language(
    key: &str,
    tenant: &str,
    sub: &str,
    code: &str,
) -> Result<(), String> {
    let code = super::controls::language_code(code).ok_or("not a language code")?;
    store_facts(&language_key(key, tenant, sub), vec![code]).await
}

/// The language a caller asked for on an earlier call, within [`MEMORY_READ_BUDGET`].
pub async fn language_wish(key: &str, tenant: &str, sub: &str) -> Option<String> {
    let kept = kept_facts(&language_key(key, tenant, sub), MEMORY_READ_BUDGET).await;
    let kept = kept.inspect_err(|error| warn!(%error, "the caller's language wish was not read"));
    kept.ok()?.into_iter().next()
}

fn language_key(key: &str, tenant: &str, sub: &str) -> String {
    format!("{}:language", caller_key(key, tenant, sub))
}

/// How long a write, or the route that lists what is kept, waits for the task Redis.
const WRITE_READ_BUDGET: Duration = Duration::from_secs(2);

/// Writes the facts kept under a caller's keyed hash, for 90 days; none forgets the caller.
async fn store_facts(id: &str, facts: Vec<String>) -> Result<(), String> {
    let id = id.to_owned();
    let Some(mut conn) = store::store().redis() else {
        // No task Redis: this process keeps it, and drops what has expired meanwhile.
        let mut local = local();
        let now = Instant::now();
        local.memory.retain(|_, (_, until)| now < *until);
        if facts.is_empty() {
            local.memory.remove(&id);
        } else {
            local.memory.insert(id, (facts, now + MEMORY_KEPT));
        }
        return Ok(());
    };
    let written: redis::RedisResult<()> = if facts.is_empty() {
        conn.del(memory_key(&id)).await
    } else {
        let json = serde_json::to_string(&facts).unwrap_or_default();
        conn.set_ex(memory_key(&id), json, MEMORY_KEPT.as_secs())
            .await
    };
    written.map_err(|error| format!("the task Redis did not take it: {error}"))
}

/// What a session reads about its caller: the newest [`FACTS_READ`] facts, within
/// [`MEMORY_READ_BUDGET`]; nothing when the read is late or fails.
pub async fn facts(key: &str, tenant: &str, sub: &str) -> Vec<String> {
    match kept_facts(&caller_key(key, tenant, sub), MEMORY_READ_BUDGET).await {
        Ok(mut facts) => {
            facts.truncate(FACTS_READ);
            facts
        }
        Err(error) => {
            warn!(%error, "caller memory was not read");
            Vec::new()
        }
    }
}

/// Everything kept about a caller, newest first (`GET /v1/interaction/memory/{subject}`).
pub async fn all_facts(key: &str, tenant: &str, sub: &str) -> Result<Vec<String>, String> {
    kept_facts(&caller_key(key, tenant, sub), WRITE_READ_BUDGET).await
}

/// The facts kept under a caller's keyed hash, from the task Redis within `budget` when it is
/// connected, else from this process.
async fn kept_facts(id: &str, budget: Duration) -> Result<Vec<String>, String> {
    if let Some(mut conn) = store::store().redis() {
        let read = conn.get::<_, Option<String>>(memory_key(id));
        return match tokio::time::timeout(budget, read).await {
            Ok(Ok(found)) => {
                let facts = found.and_then(|json| serde_json::from_str(&json).ok());
                Ok(facts.unwrap_or_default())
            }
            Ok(Err(error)) => Err(format!("the task Redis did not answer: {error}")),
            Err(_) => Err("the task Redis did not answer in time".to_owned()),
        };
    }
    let mut local = local();
    Ok(match local.memory.get(id) {
        Some((facts, until)) if Instant::now() < *until => facts.clone(),
        Some(_) => {
            local.memory.remove(id);
            Vec::new()
        }
        None => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const KEY: &str = "memory-key";

    #[test]
    fn a_token_is_the_callers_only_when_signed_for_this_tenant_and_valid_now() {
        let token = mint(KEY, "caller-7", "acme", 1_000_200);
        assert_eq!(
            verify(&token, KEY, "acme", 1_000_000).as_deref(),
            Some("caller-7")
        );
        // Another tenant, another key, expired, minted for too long, tampered.
        assert_eq!(verify(&token, KEY, "other", 1_000_000), None);
        assert_eq!(verify(&token, "another-key", "acme", 1_000_000), None);
        assert_eq!(verify(&token, KEY, "acme", 1_000_200), None);
        let long = mint(KEY, "caller-7", "acme", 1_000_000 + TOKEN_LIFETIME_S + 1);
        assert_eq!(verify(&long, KEY, "acme", 1_000_000), None);
        let forged = token.replacen("v1.", "v1.e", 1);
        assert_eq!(verify(&forged, KEY, "acme", 1_000_000), None);
        assert_eq!(verify("not a token", KEY, "acme", 1_000_000), None);
    }

    #[test]
    fn facts_hold_no_numbers_codes_or_links_and_at_most_ten() {
        let ok = [
            "prefers aisle seats".to_owned(),
            "uses a wheelchair".to_owned(),
        ];
        assert_eq!(check_facts(&ok), Ok(()));
        for bad in [
            "card ends in 4417",
            "email is sam@example.com",
            "owes $20",
            "see https://x.example",
            "",
        ] {
            assert!(check_facts(&[bad.to_owned()]).is_err(), "{bad:?}");
        }
        let eleven = vec!["likes tea".to_owned(); FACTS_KEPT + 1];
        assert!(check_facts(&eleven).is_err());
        assert!(check_facts(&["x".repeat(81)]).is_err());
    }

    #[tokio::test]
    async fn a_caller_is_remembered_per_tenant_and_forgotten_on_an_empty_list() {
        let facts = vec!["prefers aisle seats".to_owned()];
        remember(KEY, "tenant-m", "sub-1", facts.clone())
            .await
            .unwrap();
        assert_eq!(super::facts(KEY, "tenant-m", "sub-1").await, facts);
        assert!(super::facts(KEY, "tenant-n", "sub-1").await.is_empty());
        remember(KEY, "tenant-m", "sub-1", Vec::new())
            .await
            .unwrap();
        assert!(super::facts(KEY, "tenant-m", "sub-1").await.is_empty());
    }

    #[tokio::test]
    async fn a_callers_language_wish_is_kept_per_tenant_and_forgotten_with_the_caller() {
        let (tenant, sub) = ("tenant-lang", "sub-lang");
        assert_eq!(language_wish(KEY, tenant, sub).await, None);
        remember_language(KEY, tenant, sub, "es_mx").await.unwrap();
        assert_eq!(
            language_wish(KEY, tenant, sub).await.as_deref(),
            Some("es-MX")
        );
        assert_eq!(language_wish(KEY, "tenant-other", sub).await, None);
        // Kept apart from the facts: a session's facts do not show it.
        assert!(facts(KEY, tenant, sub).await.is_empty());
        assert!(
            remember_language(KEY, tenant, sub, "not a code")
                .await
                .is_err()
        );
        remember(KEY, tenant, sub, Vec::new()).await.unwrap();
        assert_eq!(language_wish(KEY, tenant, sub).await, None);
    }

    #[tokio::test]
    async fn the_backgrounds_facts_come_first_once_each_and_ten_are_kept() {
        let (tenant, sub) = ("tenant-bg", "sub-bg");
        let older: Vec<String> = (0..9)
            .map(|n| format!("older fact {}", "x".repeat(n)))
            .collect();
        remember(KEY, tenant, sub, older.clone()).await.unwrap();
        let written = vec![
            "Prefers aisle seats".to_owned(),
            "OLDER FACT".to_owned(),
            "Card ends in 4417".to_owned(),
            "prefers aisle seats".to_owned(),
            "Uses a wheelchair".to_owned(),
        ];
        assert_eq!(add_facts(KEY, tenant, sub, written).await, Ok(2));
        let kept = all_facts(KEY, tenant, sub).await.unwrap();
        assert_eq!(kept.len(), FACTS_KEPT);
        assert_eq!(
            kept[..3],
            ["Prefers aisle seats", "Uses a wheelchair", "older fact"]
        );
        assert_eq!(kept.last().map(String::as_str), Some(older[7].as_str()));
        // A session reads the newest three; nothing new adds nothing.
        assert_eq!(facts(KEY, tenant, sub).await, kept[..FACTS_READ]);
        assert_eq!(
            add_facts(KEY, tenant, sub, vec!["uses a WHEELCHAIR".to_owned()]).await,
            Ok(0)
        );
        // Another tenant's caller of the same id has nothing.
        assert!(
            all_facts(KEY, "tenant-other", sub)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_tenants_persona_is_read_with_its_versions_and_removed() {
        let tenant = "tenant-read";
        put_persona(tenant, "lumi", spec("Let me look."))
            .await
            .unwrap();
        put_persona(tenant, "lumi", spec("One moment."))
            .await
            .unwrap();
        let versions = persona_versions(tenant, "lumi").await.unwrap();
        let numbers: Vec<u32> = versions.iter().map(|(version, _)| *version).collect();
        assert_eq!(numbers, [1, 2]);
        assert_eq!(versions[1].1["banks"]["ack"][0]["text"], "One moment.");
        assert!(persona_versions("someone-else", "lumi").await.is_none());
        assert_eq!(delete_persona("someone-else", "lumi").await, Ok(false));
        assert_eq!(delete_persona(tenant, "lumi").await, Ok(true));
        assert!(persona_versions(tenant, "lumi").await.is_none());
        assert_eq!(persona(tenant, "lumi").await, None);
        assert_eq!(delete_persona(tenant, "lumi").await, Ok(false));
        // Its place under the cap is free again, and a new registration starts at version 1.
        assert_eq!(
            put_persona(tenant, "lumi", spec("Let me look.")).await,
            Ok(1)
        );
    }

    fn spec(ack: &str) -> Value {
        json!({
            "name": "Lumi", "org": "Lumen Air", "role": "booking assistant",
            "banks": { "ack": [ { "text": ack } ] }
        })
    }

    #[tokio::test]
    async fn a_tenants_persona_is_checked_versioned_and_selected_by_id() {
        let tenant = "tenant-personas";
        assert_eq!(
            put_persona(tenant, "lumi", spec("Let me look.")).await,
            Ok(1)
        );
        assert_eq!(
            put_persona(tenant, "lumi", spec("One moment.")).await,
            Ok(2)
        );
        let newest = persona(tenant, "lumi").await.unwrap();
        assert_eq!(newest["banks"]["ack"][0]["text"], "One moment.");
        assert_eq!(newest["id"], "lumi");
        let first = persona(tenant, "lumi@1").await.unwrap();
        assert_eq!(first["banks"]["ack"][0]["text"], "Let me look.");
        assert_eq!(persona("someone-else", "lumi").await, None);
        assert_eq!(persona(tenant, "lumi@9").await, None);
        // A line that claims what has not happened is refused, as in a packaged persona.
        let refused = put_persona(tenant, "liar", spec("Your booking is confirmed.")).await;
        assert!(
            matches!(refused, Err(PutRefused::Invalid(why)) if why.contains("completion_claim"))
        );
        // Five versions are kept, twenty personas at most.
        for n in 0..6 {
            put_persona(tenant, "lumi", spec(&format!("Checking that {n}.")))
                .await
                .unwrap();
        }
        assert_eq!(persona(tenant, "lumi@3").await, None);
        assert!(persona(tenant, "lumi@4").await.is_some());
        for n in 1..PERSONAS_PER_TENANT {
            put_persona(tenant, &format!("p{n}"), spec("Let me look."))
                .await
                .unwrap();
        }
        assert_eq!(
            put_persona(tenant, "one-too-many", spec("Let me look.")).await,
            Err(PutRefused::TooMany)
        );
    }

    /// Registrations at the same time: each version once, and never more personas than the
    /// cap. Run against the task Redis too with `WAAV_TEST_TASK_REDIS_URL` (ignored test below).
    async fn concurrent_puts_keep_versions_and_the_cap(tenant: &str) {
        let puts = (0..10).map(|n| {
            let tenant = tenant.to_owned();
            tokio::spawn(async move {
                put_persona(&tenant, "lumi", spec(&format!("Checking that {n}."))).await
            })
        });
        let mut versions: Vec<u32> = futures_util::future::join_all(puts)
            .await
            .into_iter()
            .map(|put| put.unwrap().unwrap())
            .collect();
        versions.sort_unstable();
        assert_eq!(versions, (1..=10).collect::<Vec<_>>());
        assert!(persona(tenant, "lumi@10").await.is_some());
        assert!(persona(tenant, "lumi@5").await.is_none(), "five kept");

        let puts = (0..2 * PERSONAS_PER_TENANT).map(|n| {
            let tenant = tenant.to_owned();
            tokio::spawn(async move {
                put_persona(&tenant, &format!("p{n}"), spec("Let me look.")).await
            })
        });
        let results = futures_util::future::join_all(puts).await;
        let taken = results
            .iter()
            .filter(|put| matches!(put, Ok(Ok(1))))
            .count();
        let refused = results
            .iter()
            .filter(|put| matches!(put, Ok(Err(PutRefused::TooMany))))
            .count();
        // `lumi` is one of the twenty.
        assert_eq!(
            (taken, refused),
            (PERSONAS_PER_TENANT - 1, PERSONAS_PER_TENANT + 1)
        );
        assert_eq!(read_personas(tenant).await.len(), PERSONAS_PER_TENANT);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_registrations_in_this_process_are_atomic() {
        concurrent_puts_keep_versions_and_the_cap("tenant-concurrent").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "needs WAAV_TEST_TASK_REDIS_URL; connects the process-wide store"]
    async fn concurrent_registrations_in_the_task_redis_are_atomic() {
        let url = std::env::var("WAAV_TEST_TASK_REDIS_URL").expect("WAAV_TEST_TASK_REDIS_URL");
        store::store().connect(&url).await.expect("connects");
        let tenant = format!("tenant-{}", uuid::Uuid::new_v4().simple());
        concurrent_puts_keep_versions_and_the_cap(&tenant).await;
        // Written to the Redis, not only kept here.
        let mut conn = store::store().redis().unwrap();
        let held: usize = conn.hlen(personas_key(&tenant)).await.unwrap();
        assert_eq!(held, PERSONAS_PER_TENANT);
    }
}
