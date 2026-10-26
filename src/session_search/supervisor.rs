//! Keep session-search's `serve` and `sync` running from the `aoe serve` daemon.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const IDLE_REPROBE: Duration = Duration::from_secs(30);
const HEALTH_DEADLINE: Duration = Duration::from_secs(30);
const HEALTH_POLL: Duration = Duration::from_millis(100);
const BACKOFF_CAP: Duration = Duration::from_secs(60);
const FAILURE_WINDOW: Duration = Duration::from_secs(600);
const MAX_FAILURES: usize = 5;
const STOP_GRACE: Duration = Duration::from_secs(5);

static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Why the supervisor stopped restarting, for the overlay.
pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().ok().and_then(|slot| slot.clone())
}

fn set_last_error(message: String) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = Some(message);
    }
}

/// `$SESSION_SEARCH_SOCKET`, else `$XDG_STATE_HOME/session-search/sock`,
/// else `~/.local/state/session-search/sock`.
pub fn socket_path() -> PathBuf {
    let set = |name| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(socket) = set("SESSION_SEARCH_SOCKET") {
        return PathBuf::from(socket);
    }
    let state = set("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".local")
            .join("state")
    });
    state.join("session-search").join("sock")
}

/// True when the service answers `GET /v1/health` with a 200 within a second.
pub fn probe(socket: &Path) -> bool {
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket) else {
        return false;
    };
    if stream.set_read_timeout(Some(PROBE_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(PROBE_TIMEOUT)).is_err()
        || stream
            .write_all(b"GET /v1/health HTTP/1.0\r\nHost: x\r\n\r\n")
            .is_err()
    {
        return false;
    }
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut head = Vec::new();
    let mut buf = [0u8; 256];
    while !head.contains(&b'\n') && Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&head);
    let line = line.lines().next().unwrap_or_default();
    line.starts_with("HTTP/") && line.split_whitespace().nth(1) == Some("200")
}

async fn probe_async(socket: &Path) -> bool {
    let socket = socket.to_path_buf();
    tokio::task::spawn_blocking(move || probe(&socket))
        .await
        .unwrap_or(false)
}

/// Start the supervisor on the daemon's runtime when the setting is on.
pub fn spawn_if_enabled(enabled: bool, shutdown: CancellationToken) {
    if !enabled {
        return;
    }
    let log_dir = match crate::session::get_app_dir() {
        Ok(dir) => dir,
        Err(err) => {
            tracing::warn!("session-search supervisor disabled: {err:#}");
            return;
        }
    };
    let supervisor = Supervisor::new(PathBuf::from("session-search"), socket_path(), log_dir);
    crate::task_util::spawn_supervised(
        "server.session_search",
        crate::task_util::PanicPolicy::Log,
        supervisor.run(shutdown),
    );
}

#[derive(Clone, Copy)]
enum Role {
    Serve,
    Sync,
}

impl Role {
    fn arg(self) -> &'static str {
        match self {
            Role::Serve => "serve",
            Role::Sync => "sync",
        }
    }
}

/// Restarts `serve` and `sync` with back-off; gives up after repeated failures.
pub struct Supervisor {
    bin: PathBuf,
    socket: PathBuf,
    log_dir: PathBuf,
    backoff_base: Duration,
    failures: VecDeque<Instant>,
    #[cfg(test)]
    idle_hook: Option<tokio::sync::mpsc::UnboundedSender<()>>,
}

impl Supervisor {
    pub fn new(bin: PathBuf, socket: PathBuf, log_dir: PathBuf) -> Self {
        Self {
            bin,
            socket,
            log_dir,
            backoff_base: Duration::from_secs(1),
            failures: VecDeque::new(),
            #[cfg(test)]
            idle_hook: None,
        }
    }

    #[cfg(test)]
    fn with_backoff_base(mut self, base: Duration) -> Self {
        self.backoff_base = base;
        self
    }

    #[cfg(test)]
    fn with_idle_hook(mut self, hook: tokio::sync::mpsc::UnboundedSender<()>) -> Self {
        self.idle_hook = Some(hook);
        self
    }

    pub async fn run(mut self, shutdown: CancellationToken) {
        // A service that already answers belongs to someone else: manage neither process.
        if probe_async(&self.socket).await {
            #[cfg(test)]
            if let Some(hook) = &self.idle_hook {
                let _ = hook.send(());
            }
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    _ = tokio::time::sleep(IDLE_REPROBE) => {}
                }
                if !probe_async(&self.socket).await {
                    break;
                }
            }
        }
        if let Err(err) = std::fs::create_dir_all(&self.log_dir) {
            set_last_error(format!(
                "session-search logs at {}: {err}",
                self.log_dir.display()
            ));
            return;
        }

        let mut serve: Option<Child> = None;
        let mut sync: Option<Child> = None;
        let gave_up = loop {
            if serve.is_none() {
                match self.start_serve(&shutdown).await {
                    Ok(Some(child)) => serve = Some(child),
                    Ok(None) => break false,
                    Err(failure) => match self.backoff(failure, &shutdown).await {
                        Some(true) => continue,
                        Some(false) => break true,
                        None => break false,
                    },
                }
            }
            if sync.is_none() {
                match self.start(Role::Sync) {
                    Ok(child) => sync = Some(child),
                    Err(failure) => match self.backoff(failure, &shutdown).await {
                        Some(true) => continue,
                        Some(false) => break true,
                        None => break false,
                    },
                }
            }
            let failure = tokio::select! {
                _ = shutdown.cancelled() => break false,
                status = wait(&mut serve) => {
                    serve = None;
                    self.exit_failure(Role::Serve, status)
                }
                status = wait(&mut sync) => {
                    sync = None;
                    self.exit_failure(Role::Sync, status)
                }
            };
            match self.backoff(failure, &shutdown).await {
                Some(true) => {}
                Some(false) => break true,
                None => break false,
            }
        };
        if gave_up {
            shutdown.cancelled().await;
        }
        for child in [serve, sync].into_iter().flatten() {
            stop(child).await;
        }
    }

    /// `Ok(None)` means shutdown arrived first.
    async fn start_serve(&self, shutdown: &CancellationToken) -> Result<Option<Child>, String> {
        let mut child = self.start(Role::Serve)?;
        let deadline = Instant::now() + HEALTH_DEADLINE;
        loop {
            if probe_async(&self.socket).await {
                return Ok(Some(child));
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(self.exit_failure(Role::Serve, Ok(status)));
            }
            if Instant::now() >= deadline {
                stop(child).await;
                return Err(format!(
                    "session-search serve did not answer health within {}s",
                    HEALTH_DEADLINE.as_secs()
                ));
            }
            tokio::select! {
                _ = shutdown.cancelled() => {
                    stop(child).await;
                    return Ok(None);
                }
                _ = tokio::time::sleep(HEALTH_POLL) => {}
            }
        }
    }

    fn log_path(&self, role: Role) -> PathBuf {
        self.log_dir
            .join(format!("session-search-{}.log", role.arg()))
    }

    fn start(&self, role: Role) -> Result<Child, String> {
        let open = || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.log_path(role))
        };
        let spawn = || -> std::io::Result<Child> {
            let stdout = open()?;
            let stderr = stdout.try_clone()?;
            Command::new(&self.bin)
                .arg(role.arg())
                .stdin(Stdio::null())
                .stdout(stdout)
                .stderr(stderr)
                .process_group(0)
                .kill_on_drop(true)
                .spawn()
        };
        spawn().map_err(|err| format!("session-search {}: {err}", role.arg()))
    }

    fn exit_failure(
        &self,
        role: Role,
        status: std::io::Result<std::process::ExitStatus>,
    ) -> String {
        let status = match status {
            Ok(status) => status.to_string(),
            Err(err) => err.to_string(),
        };
        let last_line = std::fs::read_to_string(self.log_path(role))
            .ok()
            .and_then(|log| {
                log.lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_owned)
            });
        match last_line {
            Some(line) => format!("session-search {} {status}: {line}", role.arg()),
            None => format!("session-search {} {status}", role.arg()),
        }
    }

    /// Records a failure and waits out its back-off. `Some(true)` to restart,
    /// `Some(false)` when the failure budget is spent, `None` on shutdown.
    async fn backoff(&mut self, failure: String, shutdown: &CancellationToken) -> Option<bool> {
        let now = Instant::now();
        self.failures.push_back(now);
        while self
            .failures
            .front()
            .is_some_and(|at| now.duration_since(*at) > FAILURE_WINDOW)
        {
            self.failures.pop_front();
        }
        tracing::warn!("{failure}");
        if self.failures.len() >= MAX_FAILURES {
            set_last_error(failure);
            return Some(false);
        }
        let exponent = u32::try_from(self.failures.len() - 1).unwrap_or(u32::MAX);
        let delay = self
            .backoff_base
            .saturating_mul(2u32.saturating_pow(exponent))
            .min(BACKOFF_CAP);
        tokio::select! {
            _ = shutdown.cancelled() => None,
            _ = tokio::time::sleep(delay) => Some(true),
        }
    }
}

/// Pends forever on an empty slot so `select!` ignores it.
async fn wait(child: &mut Option<Child>) -> std::io::Result<std::process::ExitStatus> {
    match child {
        Some(child) => child.wait().await,
        None => std::future::pending().await,
    }
}

/// SIGTERM the child's group, SIGKILL it if it outlives the grace period.
async fn stop(mut child: Child) {
    let Some(pid) = child.id() else {
        return;
    };
    crate::process::worker::terminate_process_group(pid);
    if tokio::time::timeout(STOP_GRACE, child.wait())
        .await
        .is_err()
    {
        crate::process::worker::kill_process_group(pid);
        let _ = child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Answers every connection with a 200, like `GET /v1/health`.
    fn serve_health(socket: &Path) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{}")
                    .await;
            }
        })
    }

    /// A fake `session-search` that appends each subcommand to `runs`.
    fn fake_bin(dir: &Path, sync_body: &str) -> std::path::PathBuf {
        let runs = dir.join("runs");
        let bin = dir.join("session-search");
        let script = format!(
            "#!/bin/sh\necho \"$1\" >> '{}'\ncase \"$1\" in\n  serve) exec sleep 600 ;;\n  sync) {sync_body} ;;\nesac\n",
            runs.display()
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    fn runs(dir: &Path, sub: &str) -> usize {
        std::fs::read_to_string(dir.join("runs"))
            .unwrap_or_default()
            .lines()
            .filter(|line| *line == sub)
            .count()
    }

    async fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ok() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[test]
    fn probe_is_false_without_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!probe(&dir.path().join("sock")));
    }

    #[tokio::test]
    async fn probe_is_true_against_a_healthy_server() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sock");
        let server = serve_health(&socket);
        let probed = tokio::task::spawn_blocking(move || probe(&socket))
            .await
            .unwrap();
        server.abort();
        assert!(probed);
    }

    #[tokio::test]
    async fn an_answering_service_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sock");
        let server = serve_health(&socket);
        let bin = fake_bin(dir.path(), "exit 0");
        let (idle_tx, mut idle_rx) = tokio::sync::mpsc::unbounded_channel();
        let supervisor =
            Supervisor::new(bin, socket, dir.path().join("logs")).with_idle_hook(idle_tx);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(supervisor.run(shutdown.clone()));

        idle_rx.recv().await.expect("supervisor reports idle");
        assert!(!dir.path().join("runs").exists(), "started a child");

        shutdown.cancel();
        task.await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn a_dying_sync_is_restarted_then_given_up_on() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sock");
        let bin = fake_bin(dir.path(), "echo 'sync failed loudly'; exit 3");
        let supervisor = Supervisor::new(bin, socket.clone(), dir.path().join("logs"))
            .with_backoff_base(Duration::from_millis(1));
        let shutdown = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(supervisor.run(shutdown.clone()));

        // The fake serve cannot listen itself, so the test answers once it starts.
        wait_until("serve to start", || runs(dir.path(), "serve") == 1).await;
        let server = serve_health(&socket);

        wait_until("sync to restart", || runs(dir.path(), "sync") >= 2).await;
        wait_until("give up", || last_error().is_some()).await;
        let error = last_error().unwrap();
        assert!(error.contains("sync failed loudly"), "{error}");
        assert_eq!(runs(dir.path(), "sync"), 5, "restarted after giving up");
        assert_eq!(runs(dir.path(), "serve"), 1);

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("supervisor stops on shutdown")
            .unwrap();
        server.abort();
    }
}
