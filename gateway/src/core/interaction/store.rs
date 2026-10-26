//! The process-wide task store behind `GET /v1/tasks`, and what outlives a session: the result
//! of a task that finished after its caller was gone, its way to the session the caller
//! reconnects with, the signed push when nobody does, and the sweep of tasks whose gateway
//! process died.
//!
//! Tasks are kept in this process. With a task Redis (`interaction.task_redis_url`) every
//! write goes there as well, and the reads, the reconnect and the sweep use it: another
//! gateway process then lists a task, hands its result to a reconnected caller, and says what
//! became of it when this process dies.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, RedisResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use tracing::{info, warn};

use super::background::post_signed;
use super::config;
use super::events::task_event_to_wire;
use super::types::{Effects, Outcome, TaskState, TaskView};
use crate::core::metrics::bridge;

/// How long the store keeps a finished task, and a reconnect's claim on a session.
const TERMINAL_KEPT: Duration = Duration::from_secs(600);
/// How long the task Redis keeps a task nobody ends: its gateway process and every process
/// that could have swept it are gone.
const LIVE_KEPT: Duration = Duration::from_secs(86_400);
/// A gateway process that has not renewed its lease for this long is suspected dead.
/// Renewed, and the sweep run, three times in that.
const LEASE: Duration = Duration::from_secs(30);
/// A suspected process is swept when it has not renewed its lease for this long more: one
/// that was only cut off from the task Redis renews it and re-asserts its tasks meanwhile.
const SWEEP_GRACE: Duration = Duration::from_secs(15);
/// The waits before each try of a push.
const PUSH_TRIES: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(2),
    Duration::from_secs(10),
];
/// What a swept task says went wrong.
const ORPHANED: &str = "the gateway process that ran this task stopped";
/// Task events kept for a reader of `GET /v1/tasks/events` that falls behind.
const PUBLISHED_KEPT: usize = 1024;
/// Writes kept for the task Redis while it does not answer: past this the oldest task event
/// is dropped (counted), and with no event among them the oldest write.
const WRITES_KEPT: usize = 4_096;

/// A task event of a session, as `GET /v1/tasks/events` streams it. With a task Redis it
/// comes from any gateway process of the deployment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Published {
    pub owner: String,
    pub session: String,
    pub task_id: String,
    pub parent_id: Option<String>,
    /// The event's wire `type` (`task_created` ... `task_output`).
    pub kind: String,
    /// The event as the session's client gets it, with the session's `stream_id`.
    pub event: serde_json::Value,
}

/// Whose tasks they are, and what the session asked for when a result finds nobody there.
#[derive(Clone, PartialEq)]
pub struct Session {
    /// The owner key (`handlers::tasks::owner`).
    pub owner: String,
    /// The session's `stream_id`.
    pub id: String,
    /// Where a result goes that no reconnected caller took (`tasks.push_url`).
    pub push_url: Option<String>,
    /// How long a caller who dropped may come back for it (`tasks.reconnect_grace_s`).
    pub grace: Duration,
    /// `tasks.resume_key`: a caller who comes back with it on another `stream_id` gets what
    /// the tasks came to.
    pub resume: Option<String>,
}

// The resume key and the push address stay out of logs.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("owner", &self.owner)
            .field("id", &self.id)
            .field("push_url", &self.push_url.is_some())
            .field("grace", &self.grace)
            .field("resume", &self.resume.is_some())
            .finish()
    }
}

impl Session {
    fn key(&self) -> Key {
        (
            self.owner.clone(),
            back_id(&self.id, self.resume.as_deref()),
        )
    }
}

/// What a caller comes back with: the session's resume key when it has one, else its
/// `stream_id` (the two never spell the same id).
pub fn back_id(stream_id: &str, resume: Option<&str>) -> String {
    match resume {
        Some(key) => format!("resume:{key}"),
        None => stream_id.to_owned(),
    }
}

/// One task as the store keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub owner: String,
    pub session: String,
    /// The gateway process that runs it; empty without a task Redis.
    pod: String,
    pub view: TaskView,
    /// How it ended, once it has.
    pub result: Option<Outcome>,
    push_url: Option<String>,
    grace_s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<String>,
}

impl Stored {
    fn of(&self) -> Session {
        Session {
            owner: self.owner.clone(),
            id: self.session.clone(),
            push_url: self.push_url.clone(),
            grace: Duration::from_secs(self.grace_s),
            resume: self.resume.clone(),
        }
    }
}

/// (owner, session).
type Key = (String, String);

/// A session whose caller dropped: until when a new connection may claim its results,
/// whether one did, and the results since.
struct Detached {
    until: Instant,
    claimed: bool,
    log: Vec<Outcome>,
}

struct Local {
    /// Each task with the time it became terminal.
    sessions: BTreeMap<Key, Vec<(Stored, Option<Instant>)>>,
    /// Used without a task Redis only.
    detached: BTreeMap<Key, Detached>,
    /// The sessions connected to this process that wait for results of an earlier connection.
    followers: BTreeMap<Key, mpsc::UnboundedSender<Outcome>>,
}

/// A view is kept while its task runs and for ten minutes after it ended.
fn kept(now: Instant, done: Option<Instant>) -> bool {
    done.is_none_or(|done| now.saturating_duration_since(done) < TERMINAL_KEPT)
}

impl Local {
    fn write(&mut self, now: Instant, session: &Session, pod: &str, view: TaskView) -> Stored {
        let key = session.key();
        if !self.sessions.contains_key(&key) {
            // One pass per new session drops what has expired, so the map stays bounded
            // without a timer and without anyone reading it.
            self.sessions.retain(|_, tasks| {
                tasks.retain(|(_, done)| kept(now, *done));
                !tasks.is_empty()
            });
        }
        let tasks = self.sessions.entry(key).or_default();
        let done = view.state.is_terminal().then_some(now);
        let known = tasks.iter().position(|(known, _)| known.view.id == view.id);
        let stored = Stored {
            owner: session.owner.clone(),
            session: session.id.clone(),
            pod: pod.to_owned(),
            view,
            result: known.and_then(|at| tasks[at].0.result.clone()),
            push_url: session.push_url.clone(),
            grace_s: session.grace.as_secs(),
            resume: session.resume.clone(),
        };
        match known {
            Some(at) => tasks[at] = (stored.clone(), done),
            None => tasks.push((stored.clone(), done)),
        }
        stored
    }

    fn result(&mut self, session: &Session, outcome: &Outcome) -> Option<Stored> {
        let tasks = self.sessions.get_mut(&session.key())?;
        let known =
            |(stored, _): &&mut (Stored, Option<Instant>)| stored.view.id == outcome.task_id;
        let (stored, _) = tasks.iter_mut().find(known)?;
        stored.result = Some(outcome.clone());
        Some(stored.clone())
    }

    /// The owner's tasks, of one session when `session` names it.
    fn list(&self, now: Instant, owner: &str, session: Option<&str>) -> Vec<&Stored> {
        // Only this owner's sessions are read: they are next to each other in the map.
        let own = self
            .sessions
            .range((owner.to_owned(), String::new())..)
            .take_while(|((known, _), _)| known == owner)
            .filter(|((_, known), _)| session.is_none_or(|session| session == known));
        own.flat_map(|(_, tasks)| tasks.iter())
            .filter(|(_, done)| kept(now, *done))
            .map(|(stored, _)| stored)
            .collect()
    }

    fn detach(&mut self, now: Instant, session: &Session) {
        self.detached
            .retain(|_, detached| now.saturating_duration_since(detached.until) < TERMINAL_KEPT);
        let detached = Detached {
            until: now + session.grace,
            claimed: false,
            log: Vec::new(),
        };
        self.detached.insert(session.key(), detached);
    }

    /// The results so far, for the first connection that asks within the grace.
    fn claim(&mut self, now: Instant, key: &Key) -> Option<Vec<Outcome>> {
        let open = |detached: &&mut Detached| !detached.claimed && now < detached.until;
        let detached = self.detached.get_mut(key).filter(open)?;
        detached.claimed = true;
        Some(detached.log.clone())
    }

    fn relay(&self, key: &Key, outcome: Outcome) {
        if let Some(follower) = self.followers.get(key) {
            let _ = follower.send(outcome);
        }
    }

    /// The tasks this process still runs, as `pod`'s.
    fn live(&self, pod: &str) -> Vec<Stored> {
        let tasks = self.sessions.values().flatten();
        let live = tasks.filter(|(stored, done)| done.is_none() && stored.view.state.is_live());
        live.map(|(stored, _)| Stored {
            pod: pod.to_owned(),
            ..stored.clone()
        })
        .collect()
    }
}

/// The tasks of every session of this process, by owner and session. A finished task stays
/// for ten minutes.
pub struct TaskStore {
    local: Mutex<Local>,
    /// The task Redis, once [`TaskStore::connect`] reached it.
    shared: OnceLock<Shared>,
    /// The task events of this process's sessions, and those the task Redis relays from the
    /// other processes, for the readers of `GET /v1/tasks/events`.
    published: OnceLock<broadcast::Sender<Published>>,
}

static STORE: TaskStore = TaskStore::new();

pub fn store() -> &'static TaskStore {
    &STORE
}

impl TaskStore {
    const fn new() -> Self {
        Self {
            local: Mutex::new(Local {
                sessions: BTreeMap::new(),
                detached: BTreeMap::new(),
                followers: BTreeMap::new(),
            }),
            shared: OnceLock::new(),
            published: OnceLock::new(),
        }
    }

    fn published(&self) -> &broadcast::Sender<Published> {
        self.published
            .get_or_init(|| broadcast::channel(PUBLISHED_KEPT).0)
    }

    /// The task Redis, once connected: what else the processes of a deployment share
    /// (`tenant`) is kept there too.
    pub(super) fn redis(&self) -> Option<ConnectionManager> {
        self.shared.get().map(|shared| shared.conn.clone())
    }

    fn local(&self) -> MutexGuard<'_, Local> {
        self.local.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A task of a session that asked for nothing after its end.
    #[cfg(test)]
    pub fn upsert(&self, owner: &str, session: &str, view: TaskView) {
        let session = Session {
            owner: owner.to_owned(),
            id: session.to_owned(),
            push_url: None,
            grace: Duration::ZERO,
            resume: None,
        };
        self.write(&session, view);
    }

    pub fn write(&self, session: &Session, view: TaskView) {
        let pod = self.shared.get().map_or("", |shared| shared.pod.as_str());
        let stored = self.local().write(Instant::now(), session, pod, view);
        if let Some(shared) = self.shared.get() {
            shared.send(Write::Task(Box::new(stored)));
        }
    }

    /// Keeps how a task ended with its view.
    pub fn result(&self, session: &Session, outcome: &Outcome) {
        let stored = self.local().result(session, outcome);
        if let (Some(shared), Some(stored)) = (self.shared.get(), stored) {
            shared.send(Write::Task(Box::new(stored)));
        }
    }

    /// A task event of the session, for the readers of `GET /v1/tasks/events`: this
    /// process's (only while there are any) and, with a task Redis, every other process's.
    pub fn publish(&self, session: &Session, mut event: serde_json::Value) {
        let channel = self.published();
        // Through the task Redis only while another process has a reader.
        let shared = self.shared.get().filter(|shared| shared.others_read());
        if channel.receiver_count() == 0 && shared.is_none() {
            return;
        }
        let text = |key: &str| {
            event
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        let (Some(kind), Some(task_id)) = (text("type"), text("task_id")) else {
            return;
        };
        let parent_id = text("parent_id");
        event["stream_id"] = json!(session.id);
        let published = Published {
            owner: session.owner.clone(),
            session: session.id.clone(),
            task_id,
            parent_id,
            kind,
            event,
        };
        if let Some(shared) = shared {
            shared.send(Write::Event(Box::new(published.clone())));
        }
        if channel.receiver_count() > 0 {
            let _ = channel.send(published);
        }
    }

    /// The task events from now on: of this process's sessions and, with a task Redis, of
    /// every gateway process's.
    pub fn subscribe(&self) -> broadcast::Receiver<Published> {
        let channel = self.published();
        // The first reader of this process: the others publish to it from now on.
        if channel.receiver_count() == 0
            && let Some(shared) = self.shared.get()
        {
            shared.send(Write::Reading);
        }
        channel.subscribe()
    }

    /// The tasks this process holds for the owner, of one session when `session` names it,
    /// oldest first within a session.
    pub fn list(&self, owner: &str, session: Option<&str>) -> Vec<TaskView> {
        let local = self.local();
        let stored = local.list(Instant::now(), owner, session);
        stored
            .into_iter()
            .map(|stored| stored.view.clone())
            .collect()
    }

    /// The owner's tasks from the task Redis, so those of every gateway process; without one,
    /// or when it does not answer, those of this process.
    pub async fn list_shared(&self, owner: &str, session: Option<&str>) -> Vec<TaskView> {
        if let Some(shared) = self.shared.get() {
            match shared.list(owner, session).await {
                Ok(stored) => return stored.into_iter().map(|stored| stored.view).collect(),
                Err(error) => warn!(%error, "the task Redis did not list; reading this process"),
            }
        }
        self.list(owner, session)
    }

    /// The highest task ref the session has used (`t3` is 3), over all its connections.
    pub async fn refs_used(&self, owner: &str, session: &str) -> u32 {
        let tasks = self.list_shared(owner, Some(session)).await;
        let number = |view: &TaskView| view.task_ref.0.trim_start_matches('t').parse::<u32>().ok();
        tasks.iter().filter_map(number).max().unwrap_or(0)
    }

    /// One task of the owner's, with its result.
    pub async fn get(&self, owner: &str, task_id: &str) -> Option<Stored> {
        if let Some(shared) = self.shared.get() {
            match shared.get(task_id).await {
                Ok(stored) => return stored.filter(|stored| stored.owner == owner),
                Err(error) => warn!(%error, "the task Redis did not answer; reading this process"),
            }
        }
        let local = self.local();
        let mut own = local.list(Instant::now(), owner, None).into_iter();
        own.find(|stored| stored.view.id.0 == task_id).cloned()
    }

    /// The session's caller dropped while a task goes on: for the session's grace a new
    /// connection of the same owner with the same `stream_id` may claim what it comes to.
    pub fn detach(&self, session: &Session) {
        match self.shared.get() {
            Some(shared) => shared.send(Write::Detach {
                key: session.key(),
                grace: session.grace,
                fresh: true,
            }),
            None => self.local().detach(Instant::now(), session),
        }
    }

    /// A task of a session that is gone has ended. `grace_left` is `None` when nobody can
    /// come back (the client closed the session): the outcome is pushed now. Otherwise it is
    /// what is left of the grace: the outcome goes to the connection that claimed the
    /// session, or claims it in that time, and it is pushed only when none has by then. A
    /// push needs the session's push address.
    pub fn deliver(
        &'static self,
        session: &Session,
        outcome: Outcome,
        grace_left: Option<Duration>,
        secret: Option<String>,
    ) {
        let key = session.key();
        if grace_left.is_some() {
            match self.shared.get() {
                Some(shared) => shared.send(Write::Log {
                    key: key.clone(),
                    outcome: Box::new(outcome.clone()),
                }),
                None => {
                    let mut local = self.local();
                    if let Some(detached) = local.detached.get_mut(&key) {
                        detached.log.push(outcome.clone());
                    }
                    local.relay(&key, outcome.clone());
                }
            }
        }
        let Some(url) = session.push_url.clone() else {
            return;
        };
        let stream_id = session.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(grace_left.unwrap_or_default()).await;
            if !self.claimed(&key).await {
                push(&url, secret.as_deref(), &stream_id, &outcome).await;
            }
        });
    }

    async fn claimed(&self, key: &Key) -> bool {
        match self.shared.get() {
            Some(shared) => shared.claimed(key).await.unwrap_or_else(|error| {
                warn!(%error, "the task Redis did not say whether the caller is back");
                false
            }),
            None => {
                let local = self.local();
                local
                    .detached
                    .get(key)
                    .is_some_and(|detached| detached.claimed)
            }
        }
    }

    /// Claims the results of an earlier connection of this session, for a caller who is back
    /// within its grace: the ones so far are returned, later ones come to [`Follow`]. `None`
    /// when there is nothing to claim: no connection dropped, the grace is over, or another
    /// connection has claimed it.
    pub async fn claim(&self, owner: &str, session: &str) -> Option<Vec<Outcome>> {
        let key = (owner.to_owned(), session.to_owned());
        match self.shared.get() {
            Some(shared) => shared.claim(&key).await.unwrap_or_else(|error| {
                warn!(%error, "the task Redis did not answer a reconnect");
                None
            }),
            None => self.local().claim(Instant::now(), &key),
        }
    }

    /// The results of this session's earlier connections, as they come. One connection
    /// follows a session: a later one takes over.
    pub fn follow(&'static self, owner: &str, session: &str) -> Follow {
        let key = (owner.to_owned(), session.to_owned());
        let (tx, rx) = mpsc::unbounded_channel();
        self.local().followers.insert(key.clone(), tx);
        Follow {
            store: self,
            key,
            rx,
        }
    }

    /// Uses the task Redis at `url` from now on, and keeps this process's lease there. `Err`
    /// when it cannot be reached: the store then stays in this process.
    pub async fn connect(&'static self, url: &str) -> Result<(), String> {
        let client =
            redis::Client::open(url).map_err(|e| format!("invalid task Redis URL: {e}"))?;
        let conn = tokio::time::timeout(
            Duration::from_secs(5),
            ConnectionManager::new(client.clone()),
        )
        .await
        .map_err(|_| "timed out connecting to the task Redis".to_owned())?
        .map_err(|e| format!("could not connect to the task Redis: {e}"))?;
        let shared = Shared {
            pod: uuid::Uuid::new_v4().simple().to_string(),
            conn,
            writes: WriteQueue::default(),
            readers_heard: Mutex::new(None),
        };
        if self.shared.set(shared).is_err() {
            return Err("the task Redis is already connected".to_owned());
        }
        let Some(shared) = self.shared.get() else {
            return Ok(());
        };
        info!(pod = %shared.pod, "interaction tasks are kept in the task Redis");
        tokio::spawn(shared.write_all());
        tokio::spawn(async move {
            let mut beat = tokio::time::interval(LEASE / 3);
            loop {
                beat.tick().await;
                if let Err(error) = self.beat(shared).await {
                    warn!(%error, "the task Redis lease was not renewed");
                }
            }
        });
        tokio::spawn(async move {
            loop {
                let ended = self.listen(&client).await;
                warn!(error = ?ended.err(), "the task Redis subscription ended; subscribing again");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        Ok(())
    }

    /// Renews this process's lease, then sweeps every process whose lease ran out and stayed
    /// out for [`SWEEP_GRACE`]: each task it left running is recorded as ended, with what is
    /// true of its effects, and goes the way of a result that finds nobody there. A process
    /// whose own lease had run out (it was cut off, not dead) writes its live tasks again, so
    /// what a sweep may have recorded meanwhile is put right.
    async fn beat(&'static self, shared: &Shared) -> RedisResult<()> {
        let mut conn = shared.conn.clone();
        let (lease,): (Option<String>,) = redis::pipe()
            .cmd("SET")
            .arg(pod_key(&shared.pod))
            .arg(1)
            .arg("EX")
            .arg(LEASE.as_secs())
            .arg("GET")
            .sadd(PODS, &shared.pod)
            .ignore()
            .del(suspect_key(&shared.pod))
            .ignore()
            .query_async(&mut conn)
            .await?;
        if lease.is_none() {
            let live = self.local().live(&shared.pod);
            if !live.is_empty() {
                warn!(
                    tasks = live.len(),
                    "the task Redis lease had run out; this process re-asserts its tasks"
                );
            }
            for stored in live {
                shared.send(Write::Task(Box::new(stored)));
            }
        }
        if self.published().receiver_count() > 0 {
            shared.send(Write::Reading);
        }
        let pods: Vec<String> = conn.smembers(PODS).await?;
        for pod in pods.into_iter().filter(|pod| *pod != shared.pod) {
            if conn.exists(pod_key(&pod)).await? {
                continue;
            }
            // Suspected first: swept only when the lease is still out a grace later.
            let suspected: Option<String> = redis::cmd("SET")
                .arg(suspect_key(&pod))
                .arg(1)
                .arg("NX")
                .arg("PX")
                .arg(SUSPECTED_FOR.as_millis() as u64)
                .query_async(&mut conn)
                .await?;
            let left: i64 = conn.pttl(suspect_key(&pod)).await?;
            if suspected.is_some() || !ripe(left) {
                continue;
            }
            // One process sweeps a dead one.
            let mine: Option<String> = redis::cmd("SET")
                .arg(format!("{NS}:sweep:{pod}"))
                .arg(&shared.pod)
                .arg("NX")
                .arg("EX")
                .arg(LEASE.as_secs())
                .query_async(&mut conn)
                .await?;
            if mine.is_none() {
                continue;
            }
            let ids: Vec<String> = conn.smembers(live_key(&pod)).await?;
            for id in ids {
                // It came back after all: it holds its tasks again.
                if conn.exists(pod_key(&pod)).await? {
                    break;
                }
                let record: Option<String> = conn.get(task_key(&id)).await?;
                let stored = record.and_then(|json| serde_json::from_str::<Stored>(&json).ok());
                let Some((stored, outcome)) = stored.and_then(orphaned) else {
                    continue;
                };
                warn!(task_id = %id, %pod, state = ?outcome.state, "interaction task swept: its gateway process stopped");
                bridge::record_degraded("interaction", "task_orphaned");
                let session = stored.of();
                shared.send(Write::Task(Box::new(stored)));
                shared.send(Write::Detach {
                    key: session.key(),
                    grace: session.grace,
                    fresh: false,
                });
                let secret = config::server().tool_secret.clone();
                self.deliver(&session, outcome, Some(session.grace), secret);
            }
            if conn.exists(pod_key(&pod)).await? {
                continue;
            }
            redis::pipe()
                .del(live_key(&pod))
                .ignore()
                .srem(PODS, &pod)
                .ignore()
                .del(suspect_key(&pod))
                .ignore()
                .query_async::<()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    /// Hands every outcome published by a gateway process to the session that follows it
    /// here, and every other process's task event to this process's readers of
    /// `GET /v1/tasks/events`. Returns when the subscription ends.
    async fn listen(&self, client: &redis::Client) -> RedisResult<()> {
        let pod = self.shared.get().map(|shared| shared.pod.clone());
        let mut pubsub = client.get_async_pubsub().await?;
        pubsub
            .subscribe(&[EVENTS, TASK_EVENTS, TASK_READERS, CALLBACK_RELAY])
            .await?;
        let mut published = pubsub.on_message();
        while let Some(message) = published.next().await {
            let Ok(json) = message.get_payload::<String>() else {
                continue;
            };
            if message.get_channel_name() == CALLBACK_RELAY {
                if let Ok(relayed) = serde_json::from_str::<RelayedCallback>(&json)
                    && Some(&relayed.pod) == pod.as_ref()
                {
                    relayed.apply();
                }
            } else if message.get_channel_name() == TASK_READERS {
                if let Some(shared) = self.shared.get()
                    && Some(&json) != pod.as_ref()
                {
                    shared.heard_reader();
                }
            } else if message.get_channel_name() == TASK_EVENTS {
                let Ok(from) = serde_json::from_str::<FromPod>(&json) else {
                    continue;
                };
                // This process's own events reached its readers when they were published.
                let channel = self.published();
                if Some(&from.pod) != pod.as_ref() && channel.receiver_count() > 0 {
                    let _ = channel.send(from.event);
                }
            } else if let Ok(relayed) = serde_json::from_str::<Relayed>(&json) {
                self.local().relay(&relayed.key, relayed.outcome);
            }
        }
        Ok(())
    }
}

/// A backend's callback (a webhook's, an A2A agent's push) that reached a process other than
/// the one that runs its task: handed to that process as it came, which checks it as its own.
/// A push's token is not on the channel: its SHA-256 is (`remote::token_digest`).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayedCallback {
    /// The process that runs the task.
    pub pod: String,
    pub task_id: String,
    pub timestamp: Option<String>,
    pub signature: Option<String>,
    #[serde(default)]
    pub token_sha256: Option<String>,
    pub body: String,
}

// Only whether a proof came is logged.
impl std::fmt::Debug for RelayedCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayedCallback")
            .field("pod", &self.pod)
            .field("task_id", &self.task_id)
            .field("timestamp", &self.timestamp)
            .field("signature", &self.signature.is_some())
            .field("token_sha256", &self.token_sha256.is_some())
            .field("body", &self.body.len())
            .finish()
    }
}

impl RelayedCallback {
    /// A callback as it reached this process, ready to hand on: a push's token as its digest.
    pub fn of(task_id: String, sender: super::remote::CallbackFrom<'_>, body: String) -> Self {
        RelayedCallback {
            pod: String::new(),
            task_id,
            timestamp: sender.timestamp.map(str::to_owned),
            signature: sender.signature.map(str::to_owned),
            token_sha256: sender.token.map(super::remote::token_digest),
            body,
        }
    }

    /// The owner takes it as it would have taken it from the backend.
    fn apply(&self) {
        let sender = super::remote::CallbackFrom {
            timestamp: self.timestamp.as_deref(),
            signature: self.signature.as_deref(),
            token: None,
            token_sha256: self.token_sha256.as_deref(),
        };
        match super::remote::callback(&self.task_id, sender, &self.body) {
            Ok(()) => {
                info!(task_id = %self.task_id, "interaction callback taken from another gateway process")
            }
            Err(why) => {
                warn!(task_id = %self.task_id, ?why, "interaction callback from another gateway process refused")
            }
        }
    }
}

/// Where a callback for a task this process does not run goes.
#[derive(Debug, PartialEq, Eq)]
pub enum CallbackPlace {
    /// Handed to the process that runs the task.
    Relayed,
    /// No live task of that id in the task Redis (or no task Redis): 404.
    Unknown,
    /// Its process runs it, but the task Redis did not take the callback: 503, try again.
    NotRelayed,
}

/// The task id of a hand-off to a person (`controls.human.url`): its callbacks are recorded
/// by [`TaskStore::hold_person`], not as a task.
const PERSON_PREFIX: &str = "human_";
/// A hand-off's record outlives its wait by this much.
const PERSON_SLACK: Duration = Duration::from_secs(60);
/// The hand-off waits at most this long for its record to be written.
const PERSON_WRITE: Duration = Duration::from_secs(2);

/// A hand-off to a person, recorded in the task Redis while it waits for the operator's
/// callback; the record goes when this does.
pub struct PersonHeld {
    store: &'static TaskStore,
    task: Option<String>,
}

impl Drop for PersonHeld {
    fn drop(&mut self) {
        if let (Some(task), Some(shared)) = (self.task.take(), self.store.shared.get()) {
            shared.send(Write::Forget(person_key(&task)));
        }
    }
}

impl TaskStore {
    /// A hand-off to a person waits for the operator's callback on `task` (a `human_<hex>`
    /// id) for `wait`: with a task Redis it is recorded under this process, so a callback that
    /// reaches another process is handed here (as a task's is).
    pub async fn hold_person(&'static self, task: &str, wait: Duration) -> PersonHeld {
        let Some(shared) = self.shared.get() else {
            return PersonHeld {
                store: self,
                task: None,
            };
        };
        let kept = (wait + PERSON_SLACK).as_secs().max(1);
        let mut conn = shared.conn.clone();
        let set = conn.set_ex::<_, _, ()>(person_key(task), &shared.pod, kept);
        match tokio::time::timeout(PERSON_WRITE, set).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "the task Redis did not record a hand-off to a person"),
            Err(_) => warn!("the task Redis did not record a hand-off to a person in time"),
        }
        PersonHeld {
            store: self,
            task: Some(task.to_owned()),
        }
    }

    /// The process that waits for a task's callbacks, by the task Redis: a live task's, or a
    /// hand-off to a person's. `Ok(None)`: none.
    async fn callback_owner(shared: &Shared, task_id: &str) -> RedisResult<Option<String>> {
        if task_id.starts_with(PERSON_PREFIX) {
            return shared.conn.clone().get(person_key(task_id)).await;
        }
        let stored = shared.get(task_id).await?;
        Ok(stored
            .filter(|stored| stored.view.state.is_live())
            .map(|stored| stored.pod))
    }

    /// A callback for a task no run of this process waits for (plan chapter 3, "Callbacks that
    /// land on the wrong pod"): with a task Redis whose record of the task (or of a hand-off
    /// to a person) names a live process other than this one, it is published to that
    /// process, which checks and takes it. The check of the signature or the token is the
    /// owner's: the secret and the token are kept nowhere else (a push's token goes as its
    /// digest).
    pub async fn relay_callback(&self, mut relayed: RelayedCallback) -> CallbackPlace {
        let Some(shared) = self.shared.get() else {
            return CallbackPlace::Unknown;
        };
        let pod = match Self::callback_owner(shared, &relayed.task_id).await {
            Ok(Some(pod)) => pod,
            Ok(None) => return CallbackPlace::Unknown,
            Err(error) => {
                warn!(%error, "the task Redis did not say which process runs a task");
                return CallbackPlace::NotRelayed;
            }
        };
        let mut conn = shared.conn.clone();
        let owner_alive = conn.exists::<_, bool>(pod_key(&pod)).await;
        match owner_alive {
            Ok(true) if pod != shared.pod => {}
            Ok(_) => return CallbackPlace::Unknown,
            Err(error) => {
                warn!(%error, "the task Redis did not say whether a process is alive");
                return CallbackPlace::NotRelayed;
            }
        }
        relayed.pod = pod;
        let json = serde_json::to_string(&relayed).unwrap_or_default();
        match conn.publish::<_, _, i64>(CALLBACK_RELAY, json).await {
            Ok(_) => CallbackPlace::Relayed,
            Err(error) => {
                warn!(%error, "the task Redis did not take a callback for another process");
                CallbackPlace::NotRelayed
            }
        }
    }
}

/// A connection's wait for the results of its session's earlier connections.
pub struct Follow {
    store: &'static TaskStore,
    key: Key,
    rx: mpsc::UnboundedReceiver<Outcome>,
}

impl Follow {
    /// The next outcome; `None` when a later connection follows the session.
    pub async fn next(&mut self) -> Option<Outcome> {
        self.rx.recv().await
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        self.rx.close();
        let mut local = self.store.local();
        // A later connection's entry is not this one's to remove.
        if local
            .followers
            .get(&self.key)
            .is_some_and(|tx| tx.is_closed())
        {
            local.followers.remove(&self.key);
        }
    }
}

/// What a swept task becomes: its process stopped, so it did not finish. A task that could
/// change something may have had a call on the wire, so its state and effects are `unknown`
/// (what it had committed before is named in the summary); one that could not has failed with
/// its effects as they were. `None` for a task that had ended.
fn orphaned(mut stored: Stored) -> Option<(Stored, Outcome)> {
    let view = &mut stored.view;
    if !view.state.is_live() {
        return None;
    }
    let mut summary = ORPHANED.to_owned();
    if view.side_effects {
        if let Effects::Committed(labels) = &view.effects {
            summary = format!("{summary}; done before that: {}", labels.join(", "));
        }
        view.state = TaskState::Unknown;
        view.effects = Effects::Unknown;
    } else {
        view.state = TaskState::Failed;
    }
    view.cancelling = false;
    view.open_question = None;
    let outcome = Outcome {
        task_id: view.id.clone(),
        task_ref: view.task_ref.clone(),
        revision: view.revision,
        seq: 0,
        state: view.state,
        summary,
        effects: view.effects.clone(),
        lineage: view.lineage.clone(),
        update: None,
    };
    stored.result = Some(outcome.clone());
    Some((stored, outcome))
}

/// The signed push of a result nobody was there for: the `task_result` or `task_cancelled`
/// event with the session's `stream_id` (and no `t_ms`: the session's clock is gone), signed
/// like a tool call. A 4xx is the receiver's refusal and ends it; anything else is tried
/// three times.
async fn push(url: &str, secret: Option<&str>, stream_id: &str, outcome: &Outcome) {
    let terminal = outcome.events(0).pop();
    let Some(event) = terminal.and_then(|event| task_event_to_wire(&event, false)) else {
        return;
    };
    let mut body = serde_json::to_value(&event).unwrap_or_default();
    if let Some(body) = body.as_object_mut() {
        body.remove("t_ms");
        body.insert("stream_id".to_owned(), json!(stream_id));
        // The short spoken update next to the full result (`summary`).
        if let Some(update) = &outcome.update {
            body.insert("update".to_owned(), json!(update));
        }
    }
    push_body(url, secret, stream_id, body.to_string(), &outcome.task_id.0).await
}

/// One signed push to the operator's push address, with `key` as its `Idempotency-Key`: a
/// task's outcome, or a session that failed (`session_failed`, chapter 10). A 4xx is the
/// receiver's refusal and ends it; anything else is tried three times.
pub(super) async fn push_body(
    url: &str,
    secret: Option<&str>,
    stream_id: &str,
    body: String,
    key: &str,
) {
    for wait in PUSH_TRIES {
        tokio::time::sleep(wait).await;
        let failure = match post_signed(url, body.clone(), secret, Some(key)).await {
            Ok(response) if response.status().is_success() => {
                info!(key, stream_id, "interaction push sent");
                return;
            }
            Ok(response) if response.status().is_client_error() => {
                warn!(key, stream_id, status = %response.status(), "interaction push refused");
                return bridge::record_degraded("interaction", "push_failed");
            }
            Ok(response) => format!("HTTP {}", response.status()),
            Err(super::background::NotPosted::Unsent(reason))
            | Err(super::background::NotPosted::Unknown(reason)) => reason,
        };
        warn!(key, stream_id, %failure, "interaction push failed");
    }
    bridge::record_degraded("interaction", "push_failed");
}

// ---- the task Redis

const NS: &str = "waav:ix";
/// The set of gateway processes that hold a lease.
const PODS: &str = "waav:ix:pods";
/// The channel outcomes of sessions that are gone are published on.
const EVENTS: &str = "waav:ix:events";
/// The channel every task event is published on, for `GET /v1/tasks/events` on any process.
const TASK_EVENTS: &str = "waav:ix:task-events";
/// The channel a process with a reader of `GET /v1/tasks/events` names itself on: when its
/// first reader comes, and at each lease renewal while it has one. A process publishes its
/// task events only for [`LEASE`] after it last heard of another process's reader.
const TASK_READERS: &str = "waav:ix:task-readers";
/// The channel a callback that reached the wrong process is handed to its task's process on.
const CALLBACK_RELAY: &str = "waav:ix:callback-relay";

/// A task's record: its JSON, kept a day while it runs and ten minutes after.
fn task_key(id: &str) -> String {
    format!("{NS}:task:{id}")
}

/// A key of one session. The owner's length is in front of it, so no owner and session
/// spell the key of another pair.
fn session_key(kind: &str, (owner, session): &Key) -> String {
    format!("{NS}:{kind}:{}:{owner}:{session}", owner.len())
}

/// The process that waits for a hand-off's callbacks (a `human_<hex>` task id).
fn person_key(task: &str) -> String {
    format!("{NS}:person:{task}")
}

/// A process's lease: gone 30 seconds after its last renewal.
fn pod_key(pod: &str) -> String {
    format!("{NS}:pod:{pod}")
}

/// The ids of the tasks a process runs.
fn live_key(pod: &str) -> String {
    format!("{NS}:live:{pod}")
}

/// A process whose lease ran out: set when another process first sees it so, gone when it
/// renews its lease or is swept.
fn suspect_key(pod: &str) -> String {
    format!("{NS}:suspect:{pod}")
}

/// How long a suspicion is kept: past the grace, so it is still there when the grace is over.
const SUSPECTED_FOR: Duration = Duration::from_secs(SWEEP_GRACE.as_secs() + LEASE.as_secs());

/// Whether a suspicion with `left_ms` to live (Redis `PTTL`) is [`SWEEP_GRACE`] old. A
/// suspicion with no expiry (`-1`) is not ours and is taken as old; a missing one (`-2`) is not.
fn ripe(left_ms: i64) -> bool {
    match u64::try_from(left_ms) {
        Ok(left) => {
            let age = (SUSPECTED_FOR.as_millis() as u64).saturating_sub(left);
            age >= SWEEP_GRACE.as_millis() as u64
        }
        Err(_) => left_ms == -1,
    }
}

/// An outcome on its way to the session that follows it.
#[derive(Serialize, Deserialize)]
struct Relayed {
    key: Key,
    outcome: Outcome,
}

enum Write {
    Task(Box<Stored>),
    /// The marker a reconnect claims. `fresh`: a session's own end, which starts its log
    /// anew; a sweep keeps a marker and a log that are there.
    Detach {
        key: Key,
        grace: Duration,
        fresh: bool,
    },
    /// An outcome for the session's reconnect: kept in its log and published.
    Log {
        key: Key,
        outcome: Box<Outcome>,
    },
    /// A task event for the readers of `GET /v1/tasks/events` on the other processes.
    Event(Box<Published>),
    /// This process has a reader of `GET /v1/tasks/events` ([`TASK_READERS`]).
    Reading,
    /// A key that is no longer needed (a hand-off's record).
    Forget(String),
}

/// Task events dropped from a full [`WriteQueue`] since the process started.
static EVENTS_DROPPED: AtomicU64 = AtomicU64::new(0);

/// The task events this process dropped because the task Redis did not keep up.
pub fn events_dropped() -> u64 {
    EVENTS_DROPPED.load(Ordering::Relaxed)
}

/// The writes on their way to the task Redis, in order, at most [`WRITES_KEPT`]: the store's
/// callers never wait for Redis, and a Redis that does not answer costs bounded memory.
#[derive(Default)]
struct WriteQueue {
    writes: Mutex<VecDeque<Write>>,
    ready: tokio::sync::Notify,
}

impl WriteQueue {
    fn push(&self, write: Write) {
        let mut writes = self.writes.lock().unwrap_or_else(PoisonError::into_inner);
        if writes.len() >= WRITES_KEPT {
            // A task event is only news; a task's record is what a reconnect and a list read.
            match writes.iter().position(|w| matches!(w, Write::Event(_))) {
                Some(at) => {
                    writes.remove(at);
                    EVENTS_DROPPED.fetch_add(1, Ordering::Relaxed);
                    bridge::record_degraded("interaction", "task_event_dropped");
                }
                None => {
                    writes.pop_front();
                    warn!("the task Redis does not keep up: the oldest write is dropped");
                    bridge::record_degraded("interaction", "task_redis_write_dropped");
                }
            }
        }
        writes.push_back(write);
        drop(writes);
        self.ready.notify_one();
    }

    async fn pop(&self) -> Write {
        loop {
            let next = self
                .writes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            match next {
                Some(write) => return write,
                None => self.ready.notified().await,
            }
        }
    }
}

/// A task event on [`TASK_EVENTS`], with the process it came from.
#[derive(Serialize, Deserialize)]
struct FromPod {
    pod: String,
    event: Published,
}

struct Shared {
    /// This process's name in the task Redis.
    pod: String,
    conn: ConnectionManager,
    /// Every write, in order: the store's callers never wait for Redis.
    writes: WriteQueue,
    /// When another process last said it has a reader of `GET /v1/tasks/events`.
    readers_heard: Mutex<Option<Instant>>,
}

impl Shared {
    fn send(&self, write: Write) {
        self.writes.push(write);
    }

    fn heard_reader(&self) {
        *self
            .readers_heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }

    /// Whether another process has had a reader of `GET /v1/tasks/events` within a lease.
    fn others_read(&self) -> bool {
        let heard = *self
            .readers_heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        heard.is_some_and(|at| at.elapsed() < LEASE)
    }

    async fn write_all(&self) {
        loop {
            let write = self.writes.pop().await;
            if let Err(error) = self.write(write).await {
                warn!(%error, "a write to the task Redis failed");
                bridge::record_degraded("interaction", "task_redis_write");
            }
        }
    }

    async fn write(&self, write: Write) -> RedisResult<()> {
        let mut conn = self.conn.clone();
        let mut pipe = redis::pipe();
        match write {
            Write::Task(stored) => {
                let live = stored.view.state.is_live();
                let kept = if live { LIVE_KEPT } else { TERMINAL_KEPT };
                let id = &stored.view.id.0;
                let key = (stored.owner.clone(), stored.session.clone());
                let index = session_key("tasks", &key);
                let json = serde_json::to_string(&stored).unwrap_or_default();
                pipe.set_ex(task_key(id), json, kept.as_secs()).ignore();
                pipe.sadd(&index, id).ignore();
                pipe.expire(&index, LIVE_KEPT.as_secs() as i64).ignore();
                if !stored.owner.is_empty() {
                    // Only a caller with an id may list without naming a session.
                    let sessions = format!("{NS}:sessions:{}", stored.owner);
                    pipe.sadd(&sessions, &stored.session).ignore();
                    pipe.expire(&sessions, LIVE_KEPT.as_secs() as i64).ignore();
                }
                if live {
                    pipe.sadd(live_key(&stored.pod), id).ignore();
                } else {
                    pipe.srem(live_key(&stored.pod), id).ignore();
                }
            }
            Write::Detach { key, grace, fresh } => {
                let marker = pipe
                    .cmd("SET")
                    .arg(session_key("detached", &key))
                    .arg("open");
                if !fresh {
                    marker.arg("NX");
                }
                marker.arg("EX").arg(grace.as_secs().max(1)).ignore();
                if fresh {
                    pipe.del(session_key("log", &key)).ignore();
                }
            }
            Write::Log { key, outcome } => {
                let log = session_key("log", &key);
                let json = serde_json::to_string(&outcome).unwrap_or_default();
                pipe.rpush(&log, json).ignore();
                pipe.expire(&log, TERMINAL_KEPT.as_secs() as i64).ignore();
                let outcome = *outcome;
                let relayed = serde_json::to_string(&Relayed { key, outcome }).unwrap_or_default();
                pipe.publish(EVENTS, relayed).ignore();
            }
            Write::Event(event) => {
                let from = FromPod {
                    pod: self.pod.clone(),
                    event: *event,
                };
                let json = serde_json::to_string(&from).unwrap_or_default();
                pipe.publish(TASK_EVENTS, json).ignore();
            }
            Write::Reading => {
                pipe.publish(TASK_READERS, &self.pod).ignore();
            }
            Write::Forget(key) => {
                pipe.del(key).ignore();
            }
        }
        pipe.query_async(&mut conn).await
    }

    async fn get(&self, task_id: &str) -> RedisResult<Option<Stored>> {
        let record: Option<String> = self.conn.clone().get(task_key(task_id)).await?;
        Ok(record.and_then(|json| serde_json::from_str(&json).ok()))
    }

    async fn list(&self, owner: &str, session: Option<&str>) -> RedisResult<Vec<Stored>> {
        let mut conn = self.conn.clone();
        let sessions: Vec<String> = match session {
            Some(session) => vec![session.to_owned()],
            None => conn.smembers(format!("{NS}:sessions:{owner}")).await?,
        };
        let mut found = Vec::new();
        for session in sessions {
            let index = session_key("tasks", &(owner.to_owned(), session.clone()));
            let ids: Vec<String> = conn.smembers(&index).await?;
            if ids.is_empty() {
                continue;
            }
            let keys: Vec<String> = ids.iter().map(|id| task_key(id)).collect();
            let records: Vec<Option<String>> =
                redis::cmd("MGET").arg(&keys).query_async(&mut conn).await?;
            for (id, record) in ids.iter().zip(records) {
                let stored = record.and_then(|json| serde_json::from_str::<Stored>(&json).ok());
                match stored {
                    // The record names its owner and session itself.
                    Some(stored) if stored.owner == owner && stored.session == session => {
                        found.push(stored)
                    }
                    Some(_) => {}
                    // The record has expired.
                    None => conn.srem::<_, _, ()>(&index, id).await?,
                }
            }
        }
        let order = |stored: &Stored| {
            let task_ref = &stored.view.task_ref.0;
            (
                stored.session.clone(),
                stored.view.started_at,
                task_ref.len(),
                task_ref.clone(),
            )
        };
        found.sort_by_key(order);
        Ok(found)
    }

    async fn claimed(&self, key: &Key) -> RedisResult<bool> {
        let marker: Option<String> = self.conn.clone().get(session_key("detached", key)).await?;
        Ok(marker.as_deref() == Some("claimed"))
    }

    async fn claim(&self, key: &Key) -> RedisResult<Option<Vec<Outcome>>> {
        let mut conn = self.conn.clone();
        // Only an open marker is claimed, and only once: `XX GET` sets it where it exists and
        // returns what it was.
        let was: Option<String> = redis::cmd("SET")
            .arg(session_key("detached", key))
            .arg("claimed")
            .arg("XX")
            .arg("GET")
            .arg("EX")
            .arg(TERMINAL_KEPT.as_secs())
            .query_async(&mut conn)
            .await?;
        if was.as_deref() != Some("open") {
            return Ok(None);
        }
        let log: Vec<String> = conn.lrange(session_key("log", key), 0, -1).await?;
        let outcomes = log
            .iter()
            .filter_map(|json| serde_json::from_str(json).ok());
        Ok(Some(outcomes.collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::{TaskId, TaskRef};
    use super::*;

    fn view(id: &str, state: TaskState) -> TaskView {
        TaskView {
            id: TaskId(id.to_owned()),
            task_ref: TaskRef("t1".to_owned()),
            revision: 1,
            title: id.to_owned(),
            state,
            cancelling: false,
            effects: Effects::NoneCommitted,
            open_question: None,
            side_effects: false,
            started_at: 0,
            lineage: Default::default(),
        }
    }

    fn session(owner: &str, id: &str, grace_s: u64) -> Session {
        Session {
            owner: owner.to_owned(),
            id: id.to_owned(),
            push_url: None,
            grace: Duration::from_secs(grace_s),
            resume: None,
        }
    }

    fn outcome(id: &str, state: TaskState, summary: &str) -> Outcome {
        Outcome {
            update: None,
            task_id: TaskId(id.to_owned()),
            task_ref: TaskRef("t1".to_owned()),
            revision: 1,
            seq: 7,
            state,
            summary: summary.to_owned(),
            effects: Effects::Committed(vec!["table booking".to_owned()]),
            lineage: Default::default(),
        }
    }

    fn ids(views: Vec<TaskView>) -> Vec<String> {
        views.into_iter().map(|view| view.id.0).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn the_store_lists_by_owner_and_drops_finished_tasks_after_ten_minutes() {
        let store = TaskStore::new();
        store.upsert("alice", "s1", view("task_a", TaskState::Working));
        store.upsert("alice", "s1", view("task_b", TaskState::Working));
        store.upsert("alice", "s2", view("task_c", TaskState::Completed));
        store.upsert("bob", "s3", view("task_d", TaskState::Working));
        // An update replaces the view.
        store.upsert("alice", "s1", view("task_a", TaskState::Completed));

        assert_eq!(
            ids(store.list("alice", None)),
            ["task_a", "task_b", "task_c"]
        );
        assert_eq!(ids(store.list("alice", Some("s2"))), ["task_c"]);
        assert_eq!(ids(store.list("bob", None)), ["task_d"]);
        assert!(store.list("carol", None).is_empty());
        assert!(store.list("alice", Some("s3")).is_empty());
        // Without a task Redis the shared read is this process's.
        assert_eq!(
            ids(store.list_shared("alice", Some("s2")).await),
            ["task_c"]
        );

        tokio::time::advance(TERMINAL_KEPT - Duration::from_secs(1)).await;
        assert_eq!(store.list("alice", None).len(), 3);
        // Finished tasks go; a live one stays.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(ids(store.list("alice", None)), ["task_b"]);
        assert_eq!(ids(store.list("bob", None)), ["task_d"]);

        // Nobody has to read the store for it to shrink: a new session's first task drops
        // what has expired, and a session left with nothing goes with it.
        let held =
            |store: &TaskStore| -> usize { store.local().sessions.values().map(Vec::len).sum() };
        assert_eq!(held(&store), 4);
        store.upsert("carol", "s4", view("task_e", TaskState::Working));
        assert_eq!(held(&store), 3);
        let gone = ("alice".to_owned(), "s2".to_owned());
        assert!(!store.local().sessions.contains_key(&gone));
    }

    #[tokio::test]
    async fn a_result_is_kept_with_its_task_and_read_by_its_owner_only() {
        let store = TaskStore::new();
        let alice = session("alice", "s1", 60);
        store.write(&alice, view("task_a", TaskState::Working));
        let booked = outcome("task_a", TaskState::Completed, "Your table is booked.");
        // The result arrives before the view that ends the task, and stays with it.
        store.result(&alice, &booked);
        store.write(&alice, view("task_a", TaskState::Completed));

        let stored = store
            .get("alice", "task_a")
            .await
            .expect("the owner reads it");
        assert_eq!(stored.session, "s1");
        assert_eq!(stored.view.state, TaskState::Completed);
        assert_eq!(stored.result, Some(booked));
        assert_eq!(store.get("bob", "task_a").await, None);
        assert_eq!(store.get("alice", "task_b").await, None);
        // A result for a task the store never saw is not kept.
        store.result(&alice, &outcome("task_z", TaskState::Failed, "no"));
        assert_eq!(store.get("alice", "task_z").await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_session_is_claimed_once_and_only_within_its_grace() {
        let store: &'static TaskStore = Box::leak(Box::new(TaskStore::new()));
        let dropped = session("alice", "s1", 60);
        // Nothing to claim before a connection dropped.
        assert_eq!(store.claim("alice", "s1").await, None);

        store.detach(&dropped);
        let early = outcome("task_a", TaskState::Completed, "Booked.");
        store.deliver(&dropped, early.clone(), Some(Duration::from_secs(60)), None);
        // Another owner, or another session, has no claim.
        assert_eq!(store.claim("bob", "s1").await, None);
        assert_eq!(store.claim("alice", "s2").await, None);
        assert!(!store.claimed(&dropped.key()).await);

        // The connection that is back gets what ended meanwhile, then what ends later.
        let mut follow = store.follow("alice", "s1");
        assert_eq!(store.claim("alice", "s1").await, Some(vec![early]));
        assert!(store.claimed(&dropped.key()).await);
        let late = outcome("task_b", TaskState::Unknown, "lost");
        store.deliver(&dropped, late.clone(), Some(Duration::from_secs(30)), None);
        assert_eq!(follow.next().await, Some(late));
        // Also one that ends when the grace is long over: the caller came back in time.
        tokio::time::advance(Duration::from_secs(120)).await;
        let later = outcome("task_c", TaskState::Completed, "Done.");
        store.deliver(&dropped, later.clone(), Some(Duration::ZERO), None);
        assert_eq!(follow.next().await, Some(later));
        // After a clean close nothing is handed on.
        store.deliver(
            &dropped,
            outcome("task_d", TaskState::Completed, "No."),
            None,
            None,
        );
        let nothing = tokio::time::timeout(Duration::from_secs(1), follow.next()).await;
        assert!(nothing.is_err(), "{nothing:?}");
        // One connection claims a session.
        assert_eq!(store.claim("alice", "s1").await, None);

        // After the grace nobody claims it.
        let late_caller = session("carol", "s9", 60);
        store.detach(&late_caller);
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(store.claim("carol", "s9").await, None);

        // A later connection takes the session over; the earlier one's end leaves it be.
        let second = store.follow("alice", "s1");
        drop(follow);
        assert!(store.local().followers.contains_key(&dropped.key()));
        drop(second);
        assert!(!store.local().followers.contains_key(&dropped.key()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_caller_with_a_resume_key_is_back_on_another_stream_id() {
        let store: &'static TaskStore = Box::leak(Box::new(TaskStore::new()));
        let first = Session {
            resume: Some("caller-42".to_owned()),
            ..session("alice", "call-1", 60)
        };
        store.detach(&first);
        let booked = outcome("task_a", TaskState::Completed, "Booked.");
        store.deliver(&first, booked.clone(), Some(Duration::from_secs(60)), None);
        // Not under the old stream_id, nor another owner's key: under the key.
        let back = back_id("call-2", Some("caller-42"));
        assert_eq!(store.claim("alice", "call-1").await, None);
        assert_eq!(store.claim("bob", &back).await, None);
        assert_eq!(store.claim("alice", &back).await, Some(vec![booked]));
        // A stream_id never spells a key.
        assert_ne!(
            back_id("resume:caller-42", None),
            back_id("x", Some("other"))
        );
        assert_eq!(back_id("call-3", None), "call-3");
    }

    #[test]
    fn a_swept_task_says_only_what_is_true_of_it() {
        let stored = |view: TaskView| Stored {
            resume: None,
            owner: "alice".to_owned(),
            session: "s1".to_owned(),
            pod: "dead".to_owned(),
            view,
            result: None,
            push_url: Some("https://hooks.example.com/tasks".to_owned()),
            grace_s: 60,
        };
        // A lookup cannot have changed anything: it failed, with its effects as they were.
        let (lookup, outcome) = orphaned(stored(view("task_a", TaskState::Working))).unwrap();
        assert_eq!(lookup.view.state, TaskState::Failed);
        assert_eq!(lookup.view.effects, Effects::NoneCommitted);
        assert_eq!(
            (outcome.state, outcome.summary.as_str()),
            (TaskState::Failed, ORPHANED)
        );
        assert_eq!(lookup.result, Some(outcome));
        assert_eq!(
            lookup.of().push_url.as_deref(),
            Some("https://hooks.example.com/tasks")
        );
        assert_eq!(lookup.of().grace, Duration::from_secs(60));

        // An action may have had a call on the wire: nobody knows what it did. What it had
        // done before is still said.
        let mut action = view("task_b", TaskState::InputRequired);
        action.side_effects = true;
        action.cancelling = true;
        action.open_question = Some(super::super::types::HitlId("q_task_b_1".to_owned()));
        action.effects = Effects::Committed(vec!["table booking".to_owned()]);
        let (action, outcome) = orphaned(stored(action)).unwrap();
        assert_eq!(action.view.state, TaskState::Unknown);
        assert_eq!(action.view.effects, Effects::Unknown);
        assert!(!action.view.cancelling && action.view.open_question.is_none());
        assert_eq!(outcome.effects, Effects::Unknown);
        assert_eq!(
            outcome.summary,
            "the gateway process that ran this task stopped; done before that: table booking"
        );

        // A task that had ended is left as it is.
        assert_eq!(orphaned(stored(view("task_c", TaskState::Completed))), None);
    }

    #[test]
    fn no_owner_and_session_spell_the_key_of_another_pair() {
        let key = |owner: &str, session: &str| {
            session_key("tasks", &(owner.to_owned(), session.to_owned()))
        };
        assert_eq!(key("alice", "s1"), "waav:ix:tasks:5:alice:s1");
        assert_ne!(key("a", "b:c"), key("a:b", "c"));
        assert_ne!(key("", "1:x"), key("x", ""));
        assert_ne!(key("", "0::s"), key("", "s"));
        // A task id read from a URL reaches only task records.
        assert!(!key("", "").starts_with(&task_key("")));
    }

    #[test]
    fn a_stored_task_reads_back_as_it_was_written() {
        let mut action = view("task_a", TaskState::Unknown);
        action.effects = Effects::Unknown;
        let stored = Stored {
            resume: None,
            owner: "alice".to_owned(),
            session: "s1".to_owned(),
            pod: "pod-1".to_owned(),
            view: action,
            result: Some(outcome("task_a", TaskState::Unknown, "lost")),
            push_url: None,
            grace_s: 60,
        };
        let json = serde_json::to_string(&stored).unwrap();
        assert_eq!(serde_json::from_str::<Stored>(&json).unwrap(), stored);
        assert!(serde_json::from_str::<Effects>("\"some\"").is_err());
        assert_eq!(
            serde_json::from_str::<Effects>("\"none_committed\"").unwrap(),
            Effects::NoneCommitted
        );
    }

    /// Two gateway processes on one task Redis, as two stores. Needs a Redis that this test
    /// may write to: `WAAV_TEST_TASK_REDIS_URL=redis://127.0.0.1:16399/`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs WAAV_TEST_TASK_REDIS_URL"]
    async fn two_processes_share_tasks_results_and_a_dead_ones_tasks_are_swept() {
        let url = std::env::var("WAAV_TEST_TASK_REDIS_URL").expect("WAAV_TEST_TASK_REDIS_URL");
        let a: &'static TaskStore = Box::leak(Box::new(TaskStore::new()));
        let b: &'static TaskStore = Box::leak(Box::new(TaskStore::new()));
        a.connect(&url).await.expect("store a connects");
        b.connect(&url).await.expect("store b connects");
        assert!(a.connect(&url).await.is_err(), "one connection per store");
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let (owner, stream) = (format!("owner-{unique}"), format!("s-{unique}"));
        let task = |n: u8| format!("task_{unique}{n}");
        let settle = || tokio::time::sleep(Duration::from_millis(300));

        // Written by one process, listed and read by the other, by its owner only.
        let mut dropped = session(&owner, &stream, 5);
        dropped.push_url = None;
        let mut booking = view(&task(1), TaskState::Working);
        booking.side_effects = true;
        a.write(&dropped, booking.clone());
        settle().await;
        assert_eq!(ids(b.list_shared(&owner, None).await), [task(1)]);
        assert_eq!(ids(b.list_shared(&owner, Some(&stream)).await), [task(1)]);
        assert!(
            b.list_shared("somebody-else", Some(&stream))
                .await
                .is_empty()
        );
        assert!(
            b.list_shared(&owner, Some("another-session"))
                .await
                .is_empty()
        );
        assert_eq!(
            b.get(&owner, &task(1)).await.map(|stored| stored.view),
            Some(booking.clone())
        );
        assert_eq!(b.get("somebody-else", &task(1)).await, None);
        assert!(b.list(&owner, None).is_empty(), "b holds none of it itself");

        // The caller drops from a and comes back on b within the grace: b's connection gets
        // the result that a's task came to, and nothing is left for a second claim.
        assert_eq!(b.claim(&owner, &stream).await, None);
        let mut follow = b.follow(&owner, &stream);
        a.detach(&dropped);
        let booked = outcome(&task(1), TaskState::Completed, "Your table is booked.");
        booking.state = TaskState::Completed;
        a.result(&dropped, &booked);
        a.write(&dropped, booking);
        a.deliver(&dropped, booked.clone(), Some(Duration::from_secs(5)), None);
        let relayed = tokio::time::timeout(Duration::from_secs(5), follow.next()).await;
        assert_eq!(relayed.expect("published to b"), Some(booked.clone()));
        assert_eq!(b.claim(&owner, &stream).await, Some(vec![booked.clone()]));
        assert!(a.claimed(&dropped.key()).await);
        assert_eq!(b.claim(&owner, &stream).await, None);
        let stored = b.get(&owner, &task(1)).await.expect("kept");
        assert_eq!(
            (stored.view.state, stored.result),
            (TaskState::Completed, Some(booked))
        );

        // A process that died with a task running: its lease is gone, so the next beat of a
        // live one records what is true of the task and hands it to the session's follower.
        let dead = format!("dead-{unique}");
        let mut lost = view(&task(2), TaskState::Working);
        lost.side_effects = true;
        let record = Stored {
            resume: None,
            owner: owner.clone(),
            session: stream.clone(),
            pod: dead.clone(),
            view: lost,
            result: None,
            push_url: None,
            grace_s: 5,
        };
        let shared = a.shared.get().expect("connected");
        shared.send(Write::Task(Box::new(record)));
        settle().await;
        let mut conn = shared.conn.clone();
        let _: () = conn.sadd(PODS, &dead).await.unwrap();
        assert_eq!(
            b.get(&owner, &task(2)).await.unwrap().view.state,
            TaskState::Working
        );
        // Its lease is out, but only for the grace: it is suspected, not swept.
        b.beat(b.shared.get().unwrap()).await.expect("a beat");
        b.beat(b.shared.get().unwrap()).await.expect("a beat");
        settle().await;
        assert_eq!(
            b.get(&owner, &task(2)).await.unwrap().view.state,
            TaskState::Working
        );
        let suspected: bool = conn.exists(suspect_key(&dead)).await.unwrap();
        assert!(suspected);
        // The grace is over (the suspicion is aged here rather than waited for).
        let aged = (SUSPECTED_FOR - SWEEP_GRACE).as_millis() as i64 - 100;
        let _: () = conn.pexpire(suspect_key(&dead), aged).await.unwrap();
        b.beat(b.shared.get().unwrap()).await.expect("the sweep");
        let swept = tokio::time::timeout(Duration::from_secs(5), follow.next()).await;
        let swept = swept.expect("published to b").expect("an outcome");
        assert_eq!(
            (swept.state, swept.effects.clone()),
            (TaskState::Unknown, Effects::Unknown)
        );
        settle().await;
        let stored = a.get(&owner, &task(2)).await.expect("kept");
        assert_eq!(stored.view.state, TaskState::Unknown);
        assert_eq!(stored.view.effects, Effects::Unknown);
        assert_eq!(stored.result, Some(swept));
        let pods: Vec<String> = conn.smembers(PODS).await.unwrap();
        assert!(!pods.contains(&dead), "the dead process is forgotten");
        let live: Vec<String> = conn.smembers(live_key(&dead)).await.unwrap();
        assert!(live.is_empty());
        let suspected: bool = conn.exists(suspect_key(&dead)).await.unwrap();
        assert!(!suspected);

        // A process that is alive but was cut off from the Redis past its lease: another one
        // suspects it, and it re-asserts its tasks at its next beat, which ends the suspicion.
        let running = session(&owner, &format!("{stream}-2"), 5);
        a.write(&running, view(&task(3), TaskState::Working));
        settle().await;
        let _: () = conn.del(pod_key(&shared.pod)).await.unwrap();
        b.beat(b.shared.get().unwrap()).await.expect("a beat");
        let suspected: bool = conn.exists(suspect_key(&shared.pod)).await.unwrap();
        assert!(suspected, "suspected, not swept");
        // Even a sweep that went through meanwhile is put right.
        let record: String = conn.get(task_key(&task(3))).await.unwrap();
        let (swept, _) = orphaned(serde_json::from_str(&record).unwrap()).unwrap();
        shared.send(Write::Task(Box::new(swept)));
        let _: () = conn.srem(PODS, &shared.pod).await.unwrap();
        settle().await;
        assert_eq!(
            b.get(&owner, &task(3)).await.unwrap().view.state,
            TaskState::Failed
        );
        a.beat(shared).await.expect("a's beat");
        settle().await;
        let back = b.get(&owner, &task(3)).await.unwrap();
        assert_eq!((back.view.state, back.result), (TaskState::Working, None));
        let suspected: bool = conn.exists(suspect_key(&shared.pod)).await.unwrap();
        assert!(!suspected);
        let pods: Vec<String> = conn.smembers(PODS).await.unwrap();
        assert!(pods.contains(&shared.pod));
        let live: Vec<String> = conn.smembers(live_key(&shared.pod)).await.unwrap();
        assert!(live.contains(&task(3)));
        b.beat(b.shared.get().unwrap()).await.expect("b's beat");
        settle().await;
        assert_eq!(
            b.get(&owner, &task(3)).await.unwrap().view.state,
            TaskState::Working
        );

        // A hand-off to a person waits on a: its callback that reaches b is handed to a; once
        // the hand-off is over, nobody waits for it.
        let person = format!("human_{unique}");
        let held = a.hold_person(&person, Duration::from_secs(5)).await;
        let signed = super::super::remote::CallbackFrom {
            timestamp: Some("1"),
            signature: Some("sha256=00"),
            ..Default::default()
        };
        let relayed = RelayedCallback::of(person.clone(), signed, "{}".to_owned());
        assert_eq!(
            b.relay_callback(relayed.clone()).await,
            CallbackPlace::Relayed
        );
        assert_eq!(
            a.relay_callback(relayed.clone()).await,
            CallbackPlace::Unknown,
            "its own process does not hand it to itself"
        );
        drop(held);
        settle().await;
        assert_eq!(b.relay_callback(relayed).await, CallbackPlace::Unknown);
    }

    #[test]
    fn a_suspicion_is_ripe_only_when_the_grace_is_over() {
        let total = SUSPECTED_FOR.as_millis() as i64;
        let grace = SWEEP_GRACE.as_millis() as i64;
        assert!(!ripe(total));
        assert!(!ripe(total - grace + 1));
        assert!(ripe(total - grace));
        assert!(ripe(0));
        // No expiry: not one of ours, taken as old. Gone: nothing to sweep on.
        assert!(ripe(-1));
        assert!(!ripe(-2));
    }
    #[tokio::test]
    async fn a_task_event_reaches_the_readers_with_its_session() {
        let store = super::store();
        let session = Session {
            owner: "store-publish-owner".to_owned(),
            id: "call-7".to_owned(),
            push_url: None,
            grace: Duration::ZERO,
            resume: None,
        };
        let event = json!({ "type": "task_output", "task_id": "task_x", "parent_id": "task_p",
                            "text": "Booked." });
        // Nobody reads: nothing is kept.
        store.publish(&session, event.clone());
        let mut reader = store.subscribe();
        store.publish(&session, json!({ "type": "heartbeat", "t_ms": 1 }));
        store.publish(&session, event);
        let published = loop {
            let next = reader.recv().await.expect("an event");
            if next.owner == session.owner {
                break next;
            }
        };
        assert_eq!(
            (
                published.kind.as_str(),
                published.task_id.as_str(),
                published.parent_id.as_deref()
            ),
            ("task_output", "task_x", Some("task_p"))
        );
        assert_eq!(published.session, "call-7");
        assert_eq!(published.event["stream_id"], "call-7");
        assert_eq!(published.event["text"], "Booked.");
    }

    #[test]
    fn a_task_event_crosses_the_task_redis_with_the_process_it_came_from() {
        let event = Published {
            owner: "o".to_owned(),
            session: "call-8".to_owned(),
            task_id: "task_y".to_owned(),
            parent_id: None,
            kind: "task_result".to_owned(),
            event: json!({ "type": "task_result", "task_id": "task_y", "stream_id": "call-8" }),
        };
        let wire = serde_json::to_string(&FromPod {
            pod: "pod-a".to_owned(),
            event: event.clone(),
        })
        .unwrap();
        let back: FromPod = serde_json::from_str(&wire).unwrap();
        assert_eq!((back.pod.as_str(), back.event), ("pod-a", event));
    }

    /// A task Redis that does not answer costs bounded memory: past `WRITES_KEPT` the oldest
    /// task event goes (counted) and every task write is kept; with no event left, the
    /// oldest write goes.
    #[tokio::test]
    async fn a_full_write_queue_drops_the_oldest_task_event_first() {
        let queue = WriteQueue::default();
        let event = |n: usize| {
            Write::Event(Box::new(Published {
                owner: "o".to_owned(),
                session: "s".to_owned(),
                task_id: format!("task_{n}"),
                parent_id: None,
                kind: "task_output".to_owned(),
                event: json!({}),
            }))
        };
        let detach = |n: usize| Write::Detach {
            key: ("o".to_owned(), format!("s{n}")),
            grace: Duration::ZERO,
            fresh: true,
        };
        queue.push(detach(0));
        for n in 0..WRITES_KEPT - 1 {
            queue.push(event(n));
        }
        let dropped = events_dropped();
        queue.push(detach(1));
        queue.push(detach(2));
        assert_eq!(events_dropped() - dropped, 2);
        let mut kept = Vec::new();
        while !queue.writes.lock().unwrap().is_empty() {
            kept.push(queue.pop().await);
        }
        assert_eq!(kept.len(), WRITES_KEPT);
        let name = |write: &Write| match write {
            Write::Event(event) => event.task_id.clone(),
            Write::Detach { key, .. } => key.1.clone(),
            _ => String::new(),
        };
        assert_eq!(name(&kept[0]), "s0");
        assert_eq!(name(&kept[1]), "task_2", "events 0 and 1 dropped");
        assert_eq!(name(&kept[WRITES_KEPT - 1]), "s2");
        // Only task writes left: the oldest goes.
        for n in 3..WRITES_KEPT + 4 {
            queue.push(detach(n));
        }
        assert_eq!(name(&queue.pop().await), "s4");
    }

    /// Two gateway processes on one task Redis: a reader of `GET /v1/tasks/events` on one
    /// gets the task events of the other's sessions, once each, and its own once.
    #[tokio::test]
    #[ignore = "needs WAAV_TEST_TASK_REDIS_URL"]
    async fn a_task_event_of_one_process_reaches_the_readers_of_another() {
        let url = std::env::var("WAAV_TEST_TASK_REDIS_URL").expect("WAAV_TEST_TASK_REDIS_URL");
        let a: &'static TaskStore = Box::leak(Box::new(TaskStore::new()));
        let b: &'static TaskStore = Box::leak(Box::new(TaskStore::new()));
        a.connect(&url).await.expect("store a connects");
        b.connect(&url).await.expect("store b connects");
        // The subscriptions are made in the background.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let on_a = session(&format!("owner-{unique}"), &format!("s-{unique}"), 0);
        let mut reader = b.subscribe();
        // a publishes once it heard of b's reader.
        tokio::time::sleep(Duration::from_millis(500)).await;
        // Nobody reads on a: its event still reaches b.
        a.publish(
            &on_a,
            json!({ "type": "task_output", "task_id": "task_far", "text": "Booked." }),
        );
        let next = tokio::time::timeout(Duration::from_secs(5), reader.recv()).await;
        let far = next.expect("the event in time").expect("an event");
        assert_eq!(
            (far.owner.as_str(), far.task_id.as_str(), far.kind.as_str()),
            (on_a.owner.as_str(), "task_far", "task_output")
        );
        assert_eq!(far.event["stream_id"], json!(on_a.id));
        // b's own event reaches its reader once, not again through the task Redis.
        let on_b = session(&on_a.owner, "s-b", 0);
        b.publish(
            &on_b,
            json!({ "type": "task_created", "task_id": "task_near" }),
        );
        let near = reader.recv().await.expect("b's own event");
        assert_eq!(near.task_id, "task_near");
        let again = tokio::time::timeout(Duration::from_millis(800), reader.recv()).await;
        assert!(again.is_err(), "nothing more: {again:?}");
    }
}
