//! Redis/Valkey implementation of the control-plane store.
//!
//! Two production details that are easy to get wrong:
//!
//! * **The connection is cached.** A miss is the common escalation (drift heal), so
//!   reconnecting per miss would roughly double its cost — and a miss storm is precisely when
//!   that matters. redis-rs multiplexed connections do not self-heal, so the slot is cleared on
//!   error and the next caller re-establishes.
//! * **`SCAN`, never `KEYS`.** `KEYS` blocks the server for the duration of the scan, which on a
//!   shared Valkey means blocking budgateway's auth too.

use std::collections::HashMap;

use redis::AsyncCommands;

use crate::store::{ControlPlaneStore, StoreError};

pub struct RedisStore {
    client: redis::Client,
    conn: tokio::sync::Mutex<Option<redis::aio::MultiplexedConnection>>,
    /// Page size for `SCAN`. Large enough that hydrating a big estate is a handful of round
    /// trips, small enough that no single reply is unbounded.
    scan_count: usize,
}

impl RedisStore {
    pub fn new(url: &str) -> Result<Self, StoreError> {
        let client =
            redis::Client::open(url).map_err(|e| StoreError::Unavailable(e.to_string()))?;
        Ok(Self {
            client,
            conn: tokio::sync::Mutex::new(None),
            scan_count: 500,
        })
    }

    pub fn client(&self) -> &redis::Client {
        &self.client
    }

    async fn conn(&self) -> Result<redis::aio::MultiplexedConnection, StoreError> {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_ref() {
            return Ok(c.clone());
        }
        let c = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| StoreError::Unavailable(e.to_string()))?;
        *guard = Some(c.clone());
        Ok(c)
    }

    /// Drop the cached connection so the next caller re-establishes it.
    async fn invalidate(&self) {
        *self.conn.lock().await = None;
    }
}

#[async_trait::async_trait]
impl ControlPlaneStore for RedisStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        let mut c = self.conn().await?;
        match c.get::<_, Option<String>>(key).await {
            Ok(v) => Ok(v),
            Err(e) => {
                self.invalidate().await;
                Err(StoreError::Unavailable(e.to_string()))
            }
        }
    }

    async fn scan(&self, pattern: &str) -> Result<HashMap<String, String>, StoreError> {
        let mut c = self.conn().await?;

        // Collect keys first with a cursor loop, then fetch values in batches. Both halves
        // avoid an unbounded single reply.
        let mut keys: Vec<String> = Vec::new();
        let mut cursor: u64 = 0;
        loop {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(self.scan_count)
                .query_async(&mut c)
                .await
                .map_err(|e| {
                    let msg = e.to_string();
                    StoreError::Unavailable(msg)
                })?;
            keys.extend(batch);
            cursor = next;
            if cursor == 0 {
                break;
            }
        }

        let mut out = HashMap::with_capacity(keys.len());
        for chunk in keys.chunks(self.scan_count) {
            if chunk.is_empty() {
                continue;
            }
            let values: Vec<Option<String>> = c
                .mget(chunk)
                .await
                .map_err(|e| StoreError::Unavailable(e.to_string()))?;
            for (k, v) in chunk.iter().zip(values) {
                if let Some(v) = v {
                    out.insert(k.clone(), v);
                }
            }
        }
        Ok(out)
    }
}

/// The keyspace patterns to subscribe to.
///
/// Specific database patterns rather than a wildcard: some Redis-compatible servers (Valkey
/// among them) do not reliably deliver events for wildcard `PSUBSCRIBE`, which fails silently —
/// the subscription succeeds and no events ever arrive.
pub fn keyspace_patterns(db: u8) -> Vec<String> {
    vec![
        format!("__keyevent@{db}__:set"),
        format!("__keyevent@{db}__:del"),
        format!("__keyevent@{db}__:expired"),
    ]
}

/// Extract the event name from a keyspace channel, e.g. `__keyevent@6__:set` -> `set`.
pub fn event_from_channel(channel: &str) -> Option<&str> {
    channel.rsplit(':').next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_are_database_specific() {
        let p = keyspace_patterns(6);
        assert_eq!(p.len(), 3);
        assert!(p.contains(&"__keyevent@6__:set".to_string()));
        assert!(p.contains(&"__keyevent@6__:del".to_string()));
        assert!(p.contains(&"__keyevent@6__:expired".to_string()));
        assert!(
            !p.iter().any(|s| s.contains('*')),
            "a wildcard pattern subscribes successfully and then silently delivers nothing on Valkey"
        );
    }

    #[test]
    fn channel_parsing_recovers_the_event() {
        assert_eq!(event_from_channel("__keyevent@6__:set"), Some("set"));
        assert_eq!(
            event_from_channel("__keyevent@11__:expired"),
            Some("expired")
        );
    }

    #[test]
    fn an_invalid_url_fails_at_construction_not_at_first_use() {
        assert!(RedisStore::new("not-a-url").is_err());
    }
}
