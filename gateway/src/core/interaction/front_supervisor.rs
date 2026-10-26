//! The front model's llama-server run by the gateway itself (plan chapter 2, "Supervisor
//! (managed mode)"), when the operator gives its command (`interaction.front_model.command`):
//! spawned with a cleared environment, polled on `/health`, restarted with backoff when it
//! exits or stops answering, and stopped when the server shuts down. A session uses the model
//! only while it is ready ([`status`]), and takes it up again after a restart.
//!
//! With `front_model.url: "unix:"` the server listens on a private UNIX socket in a per-boot
//! run directory (`$WAAV_RUN_DIR/front-<boot>/`, mode 0700) with a per-boot key file (0600)
//! and a pid file; nothing listens on loopback. At start the children that earlier gateways
//! left (their gateway gone, the child still running the same program) are stopped. The
//! model is ready only once one whole decision (the canary) has come back. There is no
//! GPU-to-CPU fallback (product owner, 2026-10-16).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::config::FrontModelServer;

/// The `front_model.url` of a model the gateway runs on its private socket.
const ON_SOCKET: &str = "unix:";

/// The socket and the key of the model the gateway runs, once it made them.
static MANAGED: OnceLock<(PathBuf, String)> = OnceLock::new();

/// Whether the front server is the model the gateway runs on its private socket.
pub fn on_socket(server: &FrontModelServer) -> bool {
    server.url.starts_with(ON_SOCKET)
}

/// The socket path and key of the model the gateway runs on its socket.
pub fn managed() -> Option<(PathBuf, String)> {
    MANAGED.get().cloned()
}

/// Where the per-boot directories are: `WAAV_RUN_DIR`, else `<temp dir>/waav-run`.
fn run_root() -> PathBuf {
    std::env::var_os("WAAV_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("waav-run"))
}

/// `/health` while the model loads, and the longest load.
const LOADING_POLL: Duration = Duration::from_millis(250);
const LOAD_MAX: Duration = Duration::from_secs(120);
/// `/health` once it is ready, and the failed checks in a row that restart it.
const READY_POLL: Duration = Duration::from_secs(5);
const HEALTH_MISSES: u32 = 3;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
/// Restarts wait 0.5, 1, 2, 4 ... 30 s; more than 10 in 10 minutes and it is left stopped.
const BACKOFF_FIRST: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const RESTARTS_MAX: usize = 10;
const RESTART_WINDOW: Duration = Duration::from_secs(600);
/// The restart window of a session (chapter 9): a session whose front stays unavailable this
/// long, the longest backoff and the longest load of one restart, ends interaction mode.
pub const RESTART_WAIT: Duration = Duration::from_secs(BACKOFF_MAX.as_secs() + LOAD_MAX.as_secs());
/// SIGTERM, then SIGKILL after this.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// The only variables the child gets: `LLAMA_ARG_*` and `LLAMA_API_KEY` could switch
/// features of the server on.
const KEPT_ENV: [&str; 4] = ["PATH", "LD_LIBRARY_PATH", "CUDA_VISIBLE_DEVICES", "HOME"];

/// The operator's statement that it may run LFM weights (`front_model.licence`,
/// `WAAV_FRONT_LICENCE`): its entity is under the LFM Open License threshold, or it holds a
/// Liquid commercial licence.
pub const LICENCES: [&str; 2] = ["lfm_open_eligible", "liquid_commercial"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrontState {
    Starting = 0,
    Ready = 1,
    Restarting = 2,
    Failed = 3,
}

struct Supervisor {
    state: AtomicU8,
    /// How often it became ready: a session that lost the model takes it again once this
    /// moves on.
    generation: AtomicU64,
    stop: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

static SUPERVISOR: OnceLock<Supervisor> = OnceLock::new();

impl Supervisor {
    fn set(&self, state: FrontState) {
        self.state.store(state as u8, Ordering::Release);
    }

    fn state(&self) -> FrontState {
        match self.state.load(Ordering::Acquire) {
            1 => FrontState::Ready,
            2 => FrontState::Restarting,
            3 => FrontState::Failed,
            _ => FrontState::Starting,
        }
    }
}

/// Whether the supervised model is ready, and how often it became ready; `None` when the
/// gateway does not run the model (an operator-run server, or none).
pub fn status() -> Option<(bool, u64)> {
    let supervisor = SUPERVISOR.get()?;
    let ready = supervisor.state() == FrontState::Ready;
    Some((ready, supervisor.generation.load(Ordering::Acquire)))
}

/// What a session does with its model front, given the generation it lost the model at
/// (`lost`) and [`status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Ask the model (also when the gateway does not run it).
    Use,
    /// The model is ready again since the session lost it: ask it, and say so.
    Back,
    /// Not ready: the session loses it at this generation (and says so).
    Lost(u64),
    /// Still lost: nothing is decided until it is back.
    Wait,
}

pub fn gate(lost: Option<u64>, status: Option<(bool, u64)>) -> Gate {
    match (lost, status) {
        (_, None) | (None, Some((true, _))) => Gate::Use,
        (Some(lost), Some((true, generation))) if generation > lost => Gate::Back,
        (Some(_), Some(_)) => Gate::Wait,
        (None, Some((false, generation))) => Gate::Lost(generation),
    }
}

/// The supervisor's state, for the log and the readiness report.
pub fn state() -> Option<FrontState> {
    SUPERVISOR.get().map(Supervisor::state)
}

/// What stops the command from running: an LFM model file (by its name) needs the licence
/// statement and a `LICENSE` file beside it (plan chapter 2, "Licence attestation").
pub fn licence_problem(command: &[String], licence: Option<&str>) -> Option<String> {
    let model = command
        .windows(2)
        .find(|pair| pair[0] == "-m" || pair[0] == "--model")
        .map(|pair| pair[1].as_str())
        .or_else(|| command.iter().find_map(|arg| arg.strip_prefix("--model=")))?;
    let path = Path::new(model);
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if !name.contains("lfm") {
        return None;
    }
    if !licence.is_some_and(|licence| LICENCES.contains(&licence)) {
        return Some(format!(
            "{model} is under the LFM Open License: interaction.front_model.licence \
             (WAAV_FRONT_LICENCE) must be lfm_open_eligible or liquid_commercial"
        ));
    }
    let beside = path.parent().unwrap_or(Path::new(".")).join("LICENSE");
    (!beside.is_file()).then(|| format!("{model} has no LICENSE file beside it"))
}

/// Start running the front model with the operator's command. Once per process; an error
/// says why it was not started (interaction mode is then refused).
pub fn start(server: &FrontModelServer) -> Result<(), String> {
    let command = server.command.clone().unwrap_or_default();
    if command.is_empty() {
        return Err("interaction.front_model.command is empty".to_owned());
    }
    if let Some(problem) = licence_problem(&command, server.licence.as_deref()) {
        return Err(problem);
    }
    let supervisor = Supervisor {
        state: AtomicU8::new(FrontState::Starting as u8),
        generation: AtomicU64::new(0),
        stop: CancellationToken::new(),
        task: Mutex::new(None),
    };
    if SUPERVISOR.set(supervisor).is_err() {
        return Err("the front model already runs".to_owned());
    }
    let supervisor = SUPERVISOR.get().expect("just set");
    let mut command = command;
    let mut pid_file = None;
    if on_socket(server) {
        let root = run_root();
        stop_stale(&root, &command[0]);
        // `sun_path` holds 108 bytes with its end: a longer path cannot be listened on.
        let longest = root.join("front-000000000000/front.sock");
        if longest.as_os_str().len() > SOCKET_PATH_MAX {
            return Err(format!(
                "the front model's socket under {} would be longer than {SOCKET_PATH_MAX} bytes: \
                 set WAAV_RUN_DIR to a shorter directory",
                root.display()
            ));
        }
        let (dir, socket, key) = run_dir(&root).map_err(|error| {
            format!(
                "the front model's run directory under {}: {error}",
                root.display()
            )
        })?;
        command.extend([
            "--host".to_owned(),
            socket.display().to_string(),
            "--api-key-file".to_owned(),
            dir.join("front.key").display().to_string(),
        ]);
        let _ = MANAGED.set((socket, key));
        pid_file = Some(dir.join("front.pid"));
        info!(run_dir = %dir.display(), "front model listens on its private socket");
    }
    let task = tokio::spawn(supervise(command, server.clone(), pid_file, supervisor));
    *supervisor.task.try_lock().expect("nobody else has it yet") = Some(task);
    Ok(())
}

/// Stop the model, when the gateway runs it: after the sessions are gone.
pub async fn stop() {
    let Some(supervisor) = SUPERVISOR.get() else {
        return;
    };
    supervisor.stop.cancel();
    if let Some(task) = supervisor.task.lock().await.take() {
        let _ = task.await;
    }
}

async fn supervise(
    command: Vec<String>,
    server: FrontModelServer,
    pid_file: Option<PathBuf>,
    supervisor: &'static Supervisor,
) {
    let mut restarts: VecDeque<Instant> = VecDeque::new();
    let mut backoff = BACKOFF_FIRST;
    loop {
        supervisor.set(FrontState::Starting);
        match spawn(&command) {
            Ok(mut child) => {
                info!(pid = child.id(), "front model started");
                // What a later gateway reads to stop this child if this gateway dies first.
                if let (Some(file), Some(pid)) = (&pid_file, child.id()) {
                    let line = format!("{pid} {}\n", std::process::id());
                    if let Err(error) = std::fs::write(file, line) {
                        warn!(%error, "front model: pid file not written");
                    }
                }
                let was_ready = watch(&mut child, &server, supervisor).await;
                supervisor.set(FrontState::Restarting);
                end(child).await;
                if was_ready {
                    backoff = BACKOFF_FIRST;
                }
            }
            Err(error) => warn!(%error, program = %command[0], "front model did not start"),
        }
        if supervisor.stop.is_cancelled() {
            return;
        }
        let now = Instant::now();
        restarts.push_back(now);
        while restarts
            .front()
            .is_some_and(|at| now.duration_since(*at) > RESTART_WINDOW)
        {
            restarts.pop_front();
        }
        if restarts.len() > RESTARTS_MAX {
            warn!("front model restarted too often; interaction mode ends on its sessions");
            return supervisor.set(FrontState::Failed);
        }
        supervisor.set(FrontState::Restarting);
        info!(in_ms = backoff.as_millis() as u64, "front model restarts");
        tokio::select! {
            _ = supervisor.stop.cancelled() => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

fn spawn(command: &[String]) -> std::io::Result<Child> {
    let mut child = Command::new(&command[0]);
    child
        .args(&command[1..])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in KEPT_ENV {
        if let Ok(value) = std::env::var(key) {
            child.env(key, value);
        }
    }
    // The child is told when the gateway dies however it dies (a SIGKILL, an abort), which
    // `kill_on_drop` does not cover.
    #[cfg(target_os = "linux")]
    // SAFETY: `prctl` is async-signal-safe and touches only the calling (child) process.
    unsafe {
        child.pre_exec(|| {
            prctl(PR_SET_PDEATHSIG, SIGTERM as u64, 0, 0, 0);
            Ok(())
        });
    }
    child.spawn()
}

/// Until the model exits, stops answering, does not load in time or the gateway stops it.
/// Returns whether it was ready meanwhile.
async fn watch(child: &mut Child, server: &FrontModelServer, supervisor: &Supervisor) -> bool {
    let started = Instant::now();
    let (mut ready, mut misses) = (false, 0);
    loop {
        let poll = if ready { READY_POLL } else { LOADING_POLL };
        tokio::select! {
            _ = supervisor.stop.cancelled() => return ready,
            status = child.wait() => {
                warn!(?status, "front model exited");
                return ready;
            }
            _ = tokio::time::sleep(poll) => {}
        }
        let answered = super::front_model::answers_health(server, HEALTH_TIMEOUT).await;
        // Ready only once one whole decision came back (the canary); one that does not is
        // asked again at the next check, until the load's time is up.
        let canary = if answered && !ready {
            match super::front_model::canary(server).await {
                Ok(ms) => Some(ms),
                Err(error) => {
                    warn!(%error, "front model canary failed");
                    None
                }
            }
        } else {
            None
        };
        if answered && (ready || canary.is_some()) {
            misses = 0;
            if !ready {
                ready = true;
                supervisor.generation.fetch_add(1, Ordering::AcqRel);
                supervisor.set(FrontState::Ready);
                info!(
                    load_ms = started.elapsed().as_millis() as u64,
                    canary_ms = canary,
                    "front model ready"
                );
            }
        } else if ready {
            misses += 1;
            if misses >= HEALTH_MISSES {
                warn!("front model stopped answering");
                return ready;
            }
        } else if started.elapsed() > LOAD_MAX {
            warn!("front model not ready in 120 s");
            return ready;
        }
    }
}

/// This boot's run directory under `root` (0700), its key file (0600, a fresh random key) and
/// the socket path.
fn run_dir(root: &Path) -> std::io::Result<(PathBuf, PathBuf, String)> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)?;
    // A short name: a UNIX socket's path has at most 107 bytes.
    let boot = uuid::Uuid::new_v4().simple().to_string();
    let dir = root.join(format!("front-{}", &boot[..12]));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let key = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("front.key"))?;
    std::io::Write::write_all(&mut file, format!("{key}\n").as_bytes())?;
    // The gateway's pid from the start (no child yet): another gateway's `stop_stale` leaves
    // a live gateway's directory alone before its child runs, or while it cannot start one.
    std::fs::write(dir.join("front.pid"), format!("0 {}\n", std::process::id()))?;
    Ok((dir.clone(), dir.join("front.sock"), key))
}

/// Whether process `pid` is alive.
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    pid > 0 && unsafe { kill(pid, 0) } == 0
}

/// The first word of a process's command line.
fn program_of(pid: i32) -> Option<String> {
    let line = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let first = line.split(|byte| *byte == 0).next()?;
    Some(String::from_utf8_lossy(first).into_owned())
}

/// Stops the front servers earlier gateways left running (plan chapter 2, "Stale children"):
/// for each `front-*` directory under `root` whose gateway is gone, a child that is still
/// alive and runs `program` gets SIGTERM, then SIGKILL after two seconds, and the directory
/// goes. A directory of a gateway that still runs is left alone.
fn stop_stale(root: &Path, program: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let named = dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("front-"));
        if !named {
            continue;
        }
        let text = std::fs::read_to_string(dir.join("front.pid")).unwrap_or_default();
        let mut pids = text
            .split_whitespace()
            .map(|word| word.parse::<i32>().unwrap_or(0));
        let (child, gateway) = (pids.next().unwrap_or(0), pids.next().unwrap_or(0));
        if alive(gateway) {
            continue;
        }
        if alive(child) && program_of(child).as_deref() == Some(program) {
            warn!(pid = child, dir = %dir.display(), "front model: an earlier gateway's child is stopped");
            // SAFETY: a plain signal to a process that runs the front's program.
            unsafe {
                kill(child, SIGTERM);
            }
            let until = Instant::now() + STOP_GRACE;
            while alive(child) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
            if alive(child) {
                // SAFETY: as above.
                unsafe {
                    kill(child, SIGKILL);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// SIGTERM, then SIGKILL after two seconds.
async fn end(mut child: Child) {
    if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
        // SAFETY: a plain signal to our own child, which `child` has not reaped yet.
        unsafe {
            kill(pid, SIGTERM);
        }
        if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_ok() {
            return;
        }
    }
    let _ = child.kill().await;
}

/// The longest UNIX socket path Linux takes (`sun_path` is 108 bytes with its NUL).
const SOCKET_PATH_MAX: usize = 107;
const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;
#[cfg(target_os = "linux")]
const PR_SET_PDEATHSIG: i32 = 1;

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
    #[cfg(target_os = "linux")]
    fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn a_session_takes_the_model_up_again_only_after_a_restart() {
        // No supervisor: the session uses its model (and its own failures decide).
        assert_eq!(gate(None, None), Gate::Use);
        assert_eq!(gate(None, Some((true, 1))), Gate::Use);
        // The model is down: the session loses it at the current generation.
        assert_eq!(gate(None, Some((false, 1))), Gate::Lost(1));
        assert_eq!(gate(Some(1), Some((false, 1))), Gate::Wait);
        // Ready at the generation it was lost at (the model failed the session, the server
        // was not restarted): still the router.
        assert_eq!(gate(Some(1), Some((true, 1))), Gate::Wait);
        // Ready again after a restart.
        assert_eq!(gate(Some(1), Some((true, 2))), Gate::Back);
    }

    #[test]
    fn a_boots_run_directory_is_private_and_holds_its_key() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().expect("temp dir");
        let (dir, socket, key) = run_dir(root.path()).expect("made");
        let mode =
            |path: &Path| std::fs::metadata(path).expect("there").permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("front.key")), 0o600);
        assert_eq!(
            std::fs::read_to_string(dir.join("front.key")).expect("key"),
            format!("{key}\n")
        );
        assert_eq!(key.len(), 64);
        assert_eq!(socket, dir.join("front.sock"));
        assert!(
            dir.file_name()
                .expect("named")
                .to_string_lossy()
                .starts_with("front-")
        );
        // Another boot gets another directory and key.
        let (other, _, other_key) = run_dir(root.path()).expect("made");
        assert!(other != dir && other_key != key);
        // A gateway that starts meanwhile leaves a live gateway's directory alone before its
        // child runs (no child pid yet).
        stop_stale(root.path(), "llama-server");
        assert!(dir.join("front.key").exists() && other.join("front.key").exists());
        let socketed = FrontModelServer {
            url: "unix:".to_owned(),
            ..FrontModelServer::default()
        };
        assert!(on_socket(&socketed));
        assert!(!on_socket(&FrontModelServer::default()));
    }

    #[test]
    fn an_earlier_gateways_child_is_stopped_and_a_live_gateways_left_alone() {
        let root = tempfile::tempdir().expect("temp dir");
        let spawn_sleep = || {
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("sleep runs")
        };
        // A gateway that is gone: a finished process's pid.
        let mut gone = std::process::Command::new("true")
            .spawn()
            .expect("true runs");
        let gone_pid = gone.id();
        gone.wait().expect("ended");
        let mut stale = spawn_sleep();
        let mut kept = spawn_sleep();
        let sleep_program = program_of(stale.id() as i32).expect("sleep's command line");
        let write = |name: &str, child: u32, gateway: u32| {
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).expect("made");
            std::fs::write(dir.join("front.pid"), format!("{child} {gateway}\n")).expect("written");
            dir
        };
        let stale_dir = write("front-old", stale.id(), gone_pid);
        // This process stands for a gateway that still runs.
        let kept_dir = write("front-live", kept.id(), std::process::id());
        // A directory of a child that runs another program is removed, the program not touched.
        let other_dir = write("front-other", std::process::id(), gone_pid);
        stop_stale(root.path(), &sleep_program);
        let status = stale.wait().expect("stopped");
        assert!(!status.success(), "ended by the signal");
        assert!(!stale_dir.exists() && !other_dir.exists());
        assert!(kept_dir.exists());
        assert!(
            kept.try_wait().expect("asked").is_none(),
            "a live gateway's child runs on"
        );
        let _ = kept.kill();
        let _ = kept.wait();
    }

    #[test]
    fn lfm_weights_need_the_licence_statement_and_their_licence_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let model = dir.path().join("LFM2.5-1.2B-Instruct-Q4_K_M.gguf");
        let command = words(&format!("llama-server -m {} --port 18741", model.display()));
        // Another model: nothing to state.
        assert_eq!(
            licence_problem(&words("llama-server -m /m/Qwen3.5-2B.gguf"), None),
            None
        );
        assert_eq!(licence_problem(&words("llama-server --port 1"), None), None);
        // An LFM file without the statement, or with another word.
        let problem = licence_problem(&command, None).expect("refused");
        assert!(problem.contains("lfm_open_eligible"), "{problem}");
        assert!(licence_problem(&command, Some("yes")).is_some());
        // With it, the LICENSE file must be beside the weights.
        let problem = licence_problem(&command, Some("lfm_open_eligible")).expect("no LICENSE");
        assert!(problem.contains("LICENSE"), "{problem}");
        std::fs::write(dir.path().join("LICENSE"), "LFM Open License v1.0").expect("written");
        assert_eq!(licence_problem(&command, Some("liquid_commercial")), None);
        let joined = vec![
            "llama-server".to_owned(),
            format!("--model={}", model.display()),
        ];
        assert_eq!(licence_problem(&joined, Some("lfm_open_eligible")), None);
        assert!(licence_problem(&joined, None).is_some());
    }

    /// A stand-in llama-server's answer: `/health` is ok, and a chat completion (the canary)
    /// streams one decision.
    fn answer(request: &[u8]) -> Vec<u8> {
        if !request.starts_with(b"POST") {
            return b"HTTP/1.1 200 OK\r\ncontent-length: 15\r\n\r\n{\"status\":\"ok\"}".to_vec();
        }
        let line: Vec<char> = r#"{"act":"delegate","say":"Let me check our hours."}"#
            .chars()
            .collect();
        let mut body = String::new();
        for piece in line.chunks(8) {
            let delta: String = piece.iter().collect();
            let chunk =
                serde_json::json!({ "choices": [{ "index": 0, "delta": { "content": delta } }] });
            body.push_str(&format!("data: {chunk}\n\n"));
        }
        body.push_str(
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        format!("{head}{body}").into_bytes()
    }

    /// A stand-in server: a shell that answers `/health` with a tiny HTTP responder is not
    /// portable, so the child here is `sleep`, and the health address is a local listener
    /// the test controls (it also answers the canary's decision).
    #[tokio::test]
    async fn the_model_is_ready_on_health_restarted_when_it_exits_and_stopped_at_the_end() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bound");
        let port = listener.local_addr().expect("address").port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = vec![0u8; 64 * 1024];
                    let read = socket.read(&mut request).await.unwrap_or(0);
                    let _ = socket.write_all(&answer(&request[..read])).await;
                });
            }
        });
        let server = FrontModelServer {
            url: format!("http://127.0.0.1:{port}"),
            model: None,
            api_key: None,
            // Exits after a second: the supervisor starts it again.
            command: Some(words("sleep 1")),
            licence: None,
        };
        assert_eq!(status(), None);
        // The stand-in answers the canary's decision.
        let canary = super::super::front_model::canary(&server).await;
        assert!(canary.is_ok(), "{canary:?}");
        start(&server).expect("started");
        assert!(start(&server).is_err(), "once per process");
        let ready = async {
            loop {
                if status().is_some_and(|(ready, _)| ready) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .expect("ready");
        let first = status().expect("supervised").1;
        // The child exits; it is started again and becomes ready once more.
        let again = async {
            loop {
                if status().is_some_and(|(ready, generation)| ready && generation > first) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), again)
            .await
            .expect("ready again after the restart");
        // The child is ended and the supervisor stops with it.
        tokio::time::timeout(Duration::from_secs(5), stop())
            .await
            .expect("stopped");
        assert!(SUPERVISOR.get().expect("set").stop.is_cancelled());
    }
}
