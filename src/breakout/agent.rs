//! The Breakout agent's socket protocol: one JSON line in, one line out.
//!
//! The agent runs on the machine with the screen. A TUI on another host reaches it
//! through an ssh-forwarded socket and asks it to open that host's Sessions.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::request::BreakoutRequest;
use super::BreakoutOutcome;

/// The longest request or reply line read.
const MAX_LINE: u64 = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentResult {
    Opened,
    /// The host is not on this machine's allow-list.
    Refused,
    Failed,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentReply {
    pub nonce: String,
    pub result: AgentResult,
    pub outcome: Option<BreakoutOutcome>,
    pub message: Option<String>,
}

/// `<app_dir>/breakout/agent.sock`. The same relative path is the ssh forward's
/// remote end, so a TUI looks in its own app dir.
pub fn socket_path() -> anyhow::Result<PathBuf> {
    Ok(crate::session::get_app_dir()?
        .join("breakout")
        .join("agent.sock"))
}

fn own_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

/// The socket's directory must be a real directory of ours, mode 0700. It is
/// narrowed before the socket exists, so the socket is never reachable by others.
fn prepare_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(std::io::Error::other(format!(
                    "{} is not a directory",
                    dir.display()
                )));
            }
            if meta.uid() != own_uid() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{} belongs to another user", dir.display()),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        Err(error) => return Err(error),
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Remove a socket left by an earlier agent. Anything else at the path, or a
/// socket another agent still answers on, is left alone.
fn remove_stale(path: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !meta.file_type().is_socket() || meta.uid() != own_uid() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "{} exists and is not a socket of yours; remove it by hand",
                path.display()
            ),
        ));
    }
    if UnixStream::connect(path).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "another Breakout agent is already listening",
        ));
    }
    std::fs::remove_file(path)
}

/// Bind an owner-only socket, replacing a stale one of ours.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("socket path has no directory"))?;
    prepare_dir(parent)?;
    remove_stale(path)?;
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// One newline-terminated line of at most `MAX_LINE` bytes that parses and
/// validates. Anything else is dropped without a reply.
fn read_request(stream: &UnixStream) -> Option<BreakoutRequest> {
    let mut line = String::new();
    BufReader::new(stream.take(MAX_LINE))
        .read_line(&mut line)
        .ok()?;
    if !line.ends_with('\n') {
        return None;
    }
    let request: BreakoutRequest = serde_json::from_str(line.trim_end()).ok()?;
    request.validate().ok()?;
    Some(request)
}

fn answer(
    request: &BreakoutRequest,
    allowed_hosts: &[String],
    open: &impl Fn(&BreakoutRequest) -> Result<BreakoutOutcome, String>,
) -> AgentReply {
    let reply = |result, outcome, message| AgentReply {
        nonce: request.nonce.clone(),
        result,
        outcome,
        message,
    };
    // A request with no host would attach Sessions on this machine; the local TUI
    // does that itself, so the agent only serves named, allowed hosts.
    let refusal = match request.host.as_deref() {
        None => Some(
            "the request names no host; the Breakout agent only opens Sessions of hosts \
             in breakout.agent_hosts"
                .to_string(),
        ),
        Some(host) if !allowed_hosts.iter().any(|allowed| allowed == host) => Some(format!(
            "host {host} is not in breakout.agent_hosts on the machine running the \
             Breakout agent; add it there to allow it"
        )),
        Some(_) => None,
    };
    if let Some(message) = refusal {
        return reply(AgentResult::Refused, None, Some(message));
    }
    match open(request) {
        Ok(outcome) => reply(AgentResult::Opened, Some(outcome), None),
        Err(error) => reply(AgentResult::Failed, None, Some(error)),
    }
}

/// Serve requests one at a time until the listener closes. Connections from
/// other users and malformed requests get no reply.
pub fn serve(
    listener: UnixListener,
    allowed_hosts: Vec<String>,
    open: impl Fn(&BreakoutRequest) -> Result<BreakoutOutcome, String>,
) {
    serve_as(listener, own_uid(), &allowed_hosts, &open);
}

fn serve_as(
    listener: UnixListener,
    owner: u32,
    allowed_hosts: &[String],
    open: &impl Fn(&BreakoutRequest) -> Result<BreakoutOutcome, String>,
) {
    for mut stream in listener.incoming().flatten() {
        if crate::process::peer_uid(&stream) != Some(owner) {
            tracing::warn!(target: "cli.breakout", "ignored a connection from another user");
            continue;
        }
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let Some(request) = read_request(&stream) else {
            tracing::warn!(target: "cli.breakout", "ignored a malformed request");
            continue;
        };
        tracing::info!(
            target: "cli.breakout",
            host = ?request.host,
            sessions = request.sessions.len(),
            "breakout request"
        );
        let reply = answer(&request, allowed_hosts, open);
        if let Ok(mut line) = serde_json::to_vec(&reply) {
            line.push(b'\n');
            let _ = stream.write_all(&line);
        }
    }
}

/// Send one request and wait for the reply, which must echo the request's nonce.
pub fn send(
    socket: &Path,
    request: &BreakoutRequest,
    timeout: Duration,
) -> std::io::Result<AgentReply> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reply = String::new();
    BufReader::new(stream.take(MAX_LINE)).read_line(&mut reply)?;
    let reply: AgentReply = serde_json::from_str(reply.trim_end())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if reply.nonce != request.nonce {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "reply nonce mismatch",
        ));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakout::request::{BreakoutLayout, BreakoutSession, PROTOCOL};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};

    fn request(host: Option<&str>) -> BreakoutRequest {
        BreakoutRequest {
            protocol: PROTOCOL,
            nonce: "0123456789abcdef".to_string(),
            host: host.map(str::to_string),
            layout: BreakoutLayout::Windows,
            sessions: vec![BreakoutSession {
                id: "abc".to_string(),
                title: "Vikings".to_string(),
                profile: String::new(),
            }],
        }
    }

    fn outcome(request: &BreakoutRequest) -> BreakoutOutcome {
        BreakoutOutcome {
            opened: request.sessions.len(),
            tiled: true,
            titles: vec!["t".to_string()],
            note: None,
        }
    }

    /// macOS caps socket paths at 104 bytes, so tests bind under /tmp.
    fn short_tmp() -> tempfile::TempDir {
        tempfile::TempDir::with_prefix_in("aoe-bo-", "/tmp").unwrap()
    }

    fn start(allowed: &[&str]) -> (tempfile::TempDir, PathBuf, mpsc::Receiver<BreakoutRequest>) {
        let dir = short_tmp();
        let path = dir.path().join("breakout").join("agent.sock");
        let listener = bind(&path).unwrap();
        let allowed: Vec<String> = allowed.iter().map(|h| h.to_string()).collect();
        let (seen_tx, seen_rx) = mpsc::channel();
        std::thread::spawn(move || {
            serve(listener, allowed, move |request| {
                let _ = seen_tx.send(request.clone());
                Ok(outcome(request))
            })
        });
        (dir, path, seen_rx)
    }

    const WAIT: Duration = Duration::from_secs(2);

    /// Write raw bytes and return whatever comes back before the server closes.
    fn raw(path: &Path, bytes: &[u8]) -> String {
        let mut stream = UnixStream::connect(path).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.set_write_timeout(Some(WAIT)).unwrap();
        let _ = stream.write_all(bytes);
        let mut reply = String::new();
        let _ = BufReader::new(&stream).read_line(&mut reply);
        reply
    }

    #[test]
    fn an_allowed_host_round_trips() {
        let (_dir, path, seen) = start(&["bender"]);
        let reply = send(&path, &request(Some("bender")), WAIT).unwrap();
        assert_eq!(reply.result, AgentResult::Opened);
        assert_eq!(reply.nonce, "0123456789abcdef");
        assert_eq!(reply.outcome.unwrap().opened, 1);
        assert_eq!(
            seen.recv_timeout(WAIT).unwrap().host.as_deref(),
            Some("bender")
        );
    }

    #[test]
    fn the_socket_and_its_directory_are_owner_only() {
        let dir = short_tmp();
        let parent = dir.path().join("breakout");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = parent.join("agent.sock");
        let _listener = bind(&path).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&parent), 0o700, "a wider directory is narrowed");
        assert_eq!(mode(&path), 0o600);

        let fresh = short_tmp();
        let fresh_path = fresh.path().join("breakout").join("agent.sock");
        let _listener = bind(&fresh_path).unwrap();
        assert_eq!(
            mode(fresh_path.parent().unwrap()),
            0o700,
            "created owner-only"
        );
        assert_eq!(mode(&fresh_path), 0o600);
    }

    #[test]
    fn a_symlinked_socket_directory_is_refused() {
        let dir = short_tmp();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("breakout");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(bind(&link.join("agent.sock")).is_err());
    }

    #[test]
    fn only_a_stale_socket_of_ours_is_replaced() {
        let dir = short_tmp();
        let path = dir.path().join("breakout").join("agent.sock");
        // A dead socket left by an earlier agent is replaced.
        drop(bind(&path).unwrap());
        assert!(
            std::fs::symlink_metadata(&path).is_ok(),
            "stale socket left"
        );
        let listener = bind(&path).unwrap();

        // A live agent is not replaced.
        let err = bind(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
        drop(listener);

        // Something that is not a socket is never removed.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "keep me").unwrap();
        assert!(bind(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
        assert_eq!(
            std::fs::metadata(&path).unwrap().uid(),
            nix::unistd::getuid().as_raw()
        );
    }

    #[test]
    fn hosts_off_the_allow_list_and_hostless_requests_are_refused() {
        let (_dir, path, seen) = start(&["bender"]);
        for host in [Some("evil"), Some("Bender"), Some("bender.local"), None] {
            let reply = send(&path, &request(host), WAIT).unwrap();
            assert_eq!(reply.result, AgentResult::Refused, "{host:?}");
            assert!(reply.outcome.is_none());
            let message = reply.message.unwrap();
            assert!(
                message.contains("breakout.agent_hosts"),
                "names the setting: {message}"
            );
        }
        assert!(seen.try_recv().is_err(), "the opener never ran");
    }

    #[test]
    fn an_empty_allow_list_refuses_everything() {
        let (_dir, path, seen) = start(&[]);
        let reply = send(&path, &request(Some("bender")), WAIT).unwrap();
        assert_eq!(reply.result, AgentResult::Refused);
        assert!(seen.try_recv().is_err());
    }

    #[test]
    fn malformed_requests_get_no_reply() {
        let (_dir, path, seen) = start(&["bender"]);
        for bad in [
            "not json\n",
            // A command or binary path is never accepted from the wire.
            r#"{"protocol":1,"nonce":"0123456789abcdef","host":"bender","layout":"windows","sessions":[{"id":"abc","title":"t","profile":""}],"command":"x"}"#,
            r#"{"protocol":1,"nonce":"0123456789abcdef","host":"bender","layout":"windows","sessions":[{"id":"abc","title":"t","profile":"","exe":"/bin/sh"}]}"#,
            // Validation runs before anything is built from the request.
            r#"{"protocol":1,"nonce":"0123456789abcdef","host":"bender","layout":"windows","sessions":[{"id":"a b","title":"t","profile":""}]}"#,
            r#"{"protocol":1,"nonce":"0123456789abcdef","host":"bender; rm","layout":"windows","sessions":[{"id":"abc","title":"t","profile":""}]}"#,
            r#"{"protocol":1,"nonce":"nothex!","host":"bender","layout":"windows","sessions":[{"id":"abc","title":"t","profile":""}]}"#,
            // The protocol version must match.
            r#"{"protocol":2,"nonce":"0123456789abcdef","host":"bender","layout":"windows","sessions":[{"id":"abc","title":"t","profile":""}]}"#,
        ] {
            let line = if bad.ends_with('\n') {
                bad.to_string()
            } else {
                format!("{bad}\n")
            };
            assert!(raw(&path, line.as_bytes()).is_empty(), "{bad}");
        }
        // A request without its newline is not a request.
        let mut good = serde_json::to_string(&request(Some("bender"))).unwrap();
        assert!(raw(&path, good.as_bytes()).is_empty());
        good.push('\n');
        assert!(!raw(&path, good.as_bytes()).is_empty(), "control: answered");
        assert_eq!(
            seen.recv_timeout(WAIT).unwrap().host.as_deref(),
            Some("bender")
        );
        assert!(seen.try_recv().is_err(), "only the good request ran");
    }

    #[test]
    fn an_oversized_request_is_dropped_without_reading_it_all() {
        let (_dir, path, seen) = start(&["bender"]);
        let mut stream = UnixStream::connect(&path).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream
            .set_write_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        // Far more than the limit and no newline: the agent stops at the limit and
        // closes, so the writer hits an error or a timeout long before the end.
        let chunk = vec![b'a'; 64 * 1024];
        let mut written = 0usize;
        for _ in 0..64 {
            match stream.write(&chunk) {
                Ok(n) => written += n,
                Err(_) => break,
            }
        }
        assert!(written < 64 * chunk.len(), "the agent kept reading");
        let mut reply = String::new();
        let _ = BufReader::new(&stream).read_line(&mut reply);
        assert!(reply.is_empty());
        assert!(seen.try_recv().is_err());
        // The agent still serves the next request.
        let reply = send(&path, &request(Some("bender")), WAIT).unwrap();
        assert_eq!(reply.result, AgentResult::Opened);
    }

    #[test]
    fn a_silent_client_times_out_and_the_next_one_is_served() {
        let (_dir, path, _seen) = start(&["bender"]);
        let idle = UnixStream::connect(&path).unwrap();
        let started = std::time::Instant::now();
        let reply = send(&path, &request(Some("bender")), Duration::from_secs(5)).unwrap();
        assert_eq!(reply.result, AgentResult::Opened);
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(idle);
    }

    #[test]
    fn a_peer_with_another_uid_gets_no_reply() {
        let dir = short_tmp();
        let path = dir.path().join("breakout").join("agent.sock");
        let listener = bind(&path).unwrap();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let other = nix::unistd::getuid().as_raw().wrapping_add(1);
        std::thread::spawn(move || {
            serve_as(listener, other, &["bender".to_string()], &|r| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(outcome(r))
            })
        });
        let line = format!(
            "{}\n",
            serde_json::to_string(&request(Some("bender"))).unwrap()
        );
        assert!(raw(&path, line.as_bytes()).is_empty());
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn requests_are_opened_one_at_a_time() {
        let dir = short_tmp();
        let path = dir.path().join("breakout").join("agent.sock");
        let listener = bind(&path).unwrap();
        let running = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let (r, m) = (Arc::clone(&running), Arc::clone(&most));
        std::thread::spawn(move || {
            serve(listener, vec!["bender".to_string()], move |request| {
                let now = r.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(150));
                r.fetch_sub(1, Ordering::SeqCst);
                Ok(outcome(request))
            })
        });
        let clients: Vec<_> = (0..3)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    send(&path, &request(Some("bender")), Duration::from_secs(5)).unwrap()
                })
            })
            .collect();
        for client in clients {
            assert_eq!(client.join().unwrap().result, AgentResult::Opened);
        }
        assert_eq!(most.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_opener_failure_is_reported() {
        let dir = short_tmp();
        let path = dir.path().join("agent.sock");
        let listener = bind(&path).unwrap();
        std::thread::spawn(move || {
            serve(listener, vec!["bender".to_string()], |_| {
                Err("no Ghostty".to_string())
            })
        });
        let reply = send(&path, &request(Some("bender")), WAIT).unwrap();
        assert_eq!(reply.result, AgentResult::Failed);
        assert_eq!(reply.message.as_deref(), Some("no Ghostty"));
    }

    #[test]
    fn a_reply_with_another_nonce_is_rejected() {
        let dir = short_tmp();
        let path = dir.path().join("fake.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let reply = AgentReply {
                nonce: "ffffffffffffffff".to_string(),
                result: AgentResult::Opened,
                outcome: None,
                message: None,
            };
            let mut out = serde_json::to_vec(&reply).unwrap();
            out.push(b'\n');
            stream.write_all(&out).unwrap();
        });
        let err = send(&path, &request(Some("bender")), WAIT).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn an_agent_that_never_answers_times_out() {
        let dir = short_tmp();
        let path = dir.path().join("wedged.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let hold = std::thread::spawn(move || listener.accept().map(|(s, _)| s));
        let started = std::time::Instant::now();
        let result = send(&path, &request(Some("bender")), Duration::from_millis(200));
        assert!(result.is_err() && started.elapsed() < Duration::from_secs(2));
        drop(hold);
    }

    #[test]
    fn remote_requests_build_their_own_ssh_commands() {
        let (_dir, path, seen) = start(&["bender"]);
        send(&path, &request(Some("bender")), WAIT).unwrap();
        let seen = seen.recv_timeout(WAIT).unwrap();
        // The opener (`run_local`) builds commands from validated fields only; a
        // host-bound request never runs a local binary path.
        for surface in crate::breakout::request::surfaces(&seen, "/evil/binary") {
            assert_eq!(surface.command, "ssh -t bender aoe session attach abc");
        }
    }
}
