//! Integration tests against a real Redis/Valkey.
//!
//! The unit suite proves the logic with an in-memory store. This suite proves the two things a
//! fake store cannot: that `RedisStore` speaks the wire protocol correctly, and that keyspace
//! notifications actually arrive and drive the snapshot.
//!
//! Skipped unless `BUD_AUTH_TEST_REDIS_URL` is set, and **skipped loudly** — a silent skip
//! would mean this suite could rot into never running while still reporting green.
//!
//! ```bash
//! BUD_AUTH_TEST_REDIS_URL=redis://127.0.0.1:6399/9 cargo test --test redis_integration
//! ```
//!
//! The server needs keyspace notifications enabled:
//! `redis-cli config set notify-keyspace-events AKE`

use std::sync::Arc;
use std::time::Duration;

use bud_auth::hydrate::{KeyEvent, hydrate_all};
use bud_auth::redis_store::{RedisStore, event_from_channel, keyspace_patterns};
use bud_auth::snapshot::BudAuth;
use bud_auth::store::ControlPlaneStore;
use bud_auth::{BudPlane, hash_api_key};

fn url() -> Option<String> {
    match std::env::var("BUD_AUTH_TEST_REDIS_URL") {
        Ok(u) if !u.trim().is_empty() => Some(u),
        _ => {
            eprintln!(
                "SKIPPING redis integration: set BUD_AUTH_TEST_REDIS_URL to run it. \
                 A skip here means the wire protocol and keyspace delivery were NOT verified."
            );
            None
        }
    }
}

fn db_index(url: &str) -> u8 {
    url.rsplit('/')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn blob(endpoint: &str) -> String {
    format!(
        r#"{{"tts":{{"endpoint_id":"{endpoint}","project_id":"p1"}},"__metadata__":{{"api_key_id":"ak1","user_id":"u1","api_key_project_id":"p1"}}}}"#
    )
}

async fn wipe(client: &redis::Client, prefix: &str) {
    let mut c = client.get_multiplexed_async_connection().await.unwrap();
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{prefix}*"))
        .query_async(&mut c)
        .await
        .unwrap_or_default();
    for k in keys {
        let _: () = redis::cmd("DEL")
            .arg(&k)
            .query_async(&mut c)
            .await
            .unwrap_or(());
    }
}

async fn set_key(client: &redis::Client, key: &str, value: &str) {
    let mut c = client.get_multiplexed_async_connection().await.unwrap();
    let _: () = redis::cmd("SET")
        .arg(key)
        .arg(value)
        .query_async(&mut c)
        .await
        .unwrap();
}

async fn del_key(client: &redis::Client, key: &str) {
    let mut c = client.get_multiplexed_async_connection().await.unwrap();
    let _: () = redis::cmd("DEL")
        .arg(key)
        .query_async(&mut c)
        .await
        .unwrap();
}

/// The wire protocol: SCAN paging and MGET batching against a real server.
#[tokio::test]
async fn hydrates_a_large_estate_from_a_real_server() {
    let Some(url) = url() else { return };
    let store = RedisStore::new(&url).expect("client");
    let client = store.client().clone();
    wipe(&client, "api_key:it1-").await;

    // More than one SCAN page (the store pages at 500) so cursor handling is exercised.
    for i in 0..1200 {
        set_key(&client, &format!("api_key:it1-{i}"), &blob("e1")).await;
    }

    let auth = BudAuth::new();
    let stats = hydrate_all(&store, &auth).await.expect("hydrate");

    assert!(
        stats.api_keys >= 1200,
        "SCAN paging lost keys: got {}",
        stats.api_keys
    );
    assert!(
        auth.resolve("it1-1199").is_some(),
        "the last page was dropped"
    );
    assert_eq!(
        auth.generation(),
        1,
        "a real hydration published more than one generation"
    );

    wipe(&client, "api_key:it1-").await;
}

/// A missing key must come back as an authoritative absence, not an error.
#[tokio::test]
async fn a_missing_key_is_absent_not_unavailable() {
    let Some(url) = url() else { return };
    let store = RedisStore::new(&url).expect("client");
    assert_eq!(
        store.get("api_key:definitely-not-here").await.unwrap(),
        None
    );
}

/// An unreachable server must surface as `Unavailable` — the distinction that stops a
/// transient failure being cached as a denial.
#[tokio::test]
async fn an_unreachable_server_is_unavailable_not_absent() {
    let store = RedisStore::new("redis://127.0.0.1:6398/0").expect("client");
    let err = store.get("api_key:x").await.unwrap_err();
    assert!(
        matches!(err, bud_auth::StoreError::Unavailable(_)),
        "an unreachable server did not report Unavailable; a wobble would be cached as absent"
    );
}

/// TC-AUTH-08 end to end: revocation reaches the snapshot through a real keyspace event.
///
/// This is the one that a fake store genuinely cannot prove — that the server emits the
/// notification, that the pattern matches, and that the payload is the key name.
#[tokio::test]
async fn a_real_keyspace_event_propagates_a_revocation() {
    let Some(url) = url() else { return };
    let store = Arc::new(RedisStore::new(&url).expect("client"));
    let client = store.client().clone();
    let db = db_index(&url);

    let raw_key = "bud_it_revocation";
    let hashed = hash_api_key(raw_key);
    let redis_key = format!("api_key:{hashed}");

    wipe(&client, &redis_key).await;
    set_key(&client, &redis_key, &blob("e1")).await;

    let plane = Arc::new(BudPlane::new(
        Arc::clone(&store) as Arc<dyn ControlPlaneStore>,
        None,
    ));
    plane.boot().await.expect("boot");
    assert!(
        plane.authenticate(raw_key).await.is_ok(),
        "the seeded key did not authenticate after boot"
    );

    // Subscribe exactly as production does.
    let mut pubsub = client.get_async_pubsub().await.expect("pubsub");
    for p in keyspace_patterns(db) {
        pubsub.psubscribe(&p).await.expect("psubscribe");
    }

    let watched = redis_key.clone();
    let plane_for_loop = Arc::clone(&plane);
    let handle = tokio::spawn(async move {
        use futures_util::StreamExt;
        let mut stream = pubsub.on_message();
        // Loop, exactly as production does. An earlier draft took a single message and was
        // flaky: the database is shared, so the first event to arrive is frequently somebody
        // else's. Any consumer that processes one notification and stops will silently miss
        // revocations in production for the same reason.
        while let Some(msg) = stream.next().await {
            let channel = msg.get_channel_name().to_string();
            let key: String = msg.get_payload().unwrap_or_default();
            if let Some(ev) = event_from_channel(&channel).and_then(KeyEvent::parse) {
                let _ = plane_for_loop.on_key_event(&key, ev).await;
                if key == watched && matches!(ev, KeyEvent::Del) {
                    break;
                }
            }
        }
    });

    // Give the subscription a moment to register before triggering the event.
    tokio::time::sleep(Duration::from_millis(150)).await;
    del_key(&client, &redis_key).await;

    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

    assert!(
        plane.authenticate(raw_key).await.is_err(),
        "a revoked credential still authenticates; the keyspace event never reached the snapshot"
    );
}

/// The reconnect path against a real server: keys created while we were not listening must be
/// visible after re-hydration, and keys deleted must be gone.
#[tokio::test]
async fn rehydration_reconciles_changes_made_while_disconnected() {
    let Some(url) = url() else { return };
    let store = RedisStore::new(&url).expect("client");
    let client = store.client().clone();
    wipe(&client, "api_key:it2-").await;

    set_key(&client, "api_key:it2-before", &blob("e1")).await;
    let auth = BudAuth::new();
    hydrate_all(&store, &auth).await.expect("first hydrate");
    assert!(auth.resolve("it2-before").is_some());

    // Changes we never saw an event for.
    del_key(&client, "api_key:it2-before").await;
    set_key(&client, "api_key:it2-after", &blob("e1")).await;

    hydrate_all(&store, &auth).await.expect("rehydrate");

    assert!(
        auth.resolve("it2-after").is_some(),
        "a key created while disconnected is still invisible"
    );
    assert!(
        auth.resolve("it2-before").is_none(),
        "a key deleted while disconnected survived re-hydration"
    );

    wipe(&client, "api_key:it2-").await;
}
