//! Owner-only requests from external tools (Switchboard Jump) to show one
//! session in this TUI. At the dashboard the main loop answers and attaches;
//! while the TUI is blocked inside `tmux attach`, the listener thread switches
//! the TUI's own client. Results are a closed vocabulary.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::session::tui_control::{self, TuiRecord};
use crate::session::Instance;

const MAX_REQUEST: u64 = 4096;
const IO_TIMEOUT: Duration = Duration::from_millis(500);
/// How long the listener waits for the main loop before answering `busy`.
const MAIN_LOOP_WAIT: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusResult {
    Accepted,
    Starting,
    AlreadyShowing,
    NotRunning,
    NeedsInput,
    Shelved,
    UnknownSession,
    Busy,
    Error,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusRequest {
    pub focus: String,
    pub nonce: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FocusReply {
    pub result: FocusResult,
    pub nonce: String,
}

/// What the target looks like right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TargetState {
    pub structured: bool,
    pub shelved: bool,
    pub exists: bool,
    pub needs_restart: bool,
    /// Starting it would open a dialog (custom instruction, store move).
    pub needs_input: bool,
}

/// The dashboard decision, shared with the attached-state listener.
pub fn decide(target: Option<TargetState>) -> FocusResult {
    let Some(t) = target else {
        return FocusResult::UnknownSession;
    };
    if t.structured {
        FocusResult::Error
    } else if t.shelved {
        FocusResult::Shelved
    } else if !t.exists {
        FocusResult::NotRunning
    } else if t.needs_restart && t.needs_input {
        FocusResult::NeedsInput
    } else if t.needs_restart {
        FocusResult::Starting
    } else {
        FocusResult::Accepted
    }
}

/// Same restart rule as `App::attach_session`: hook status or a command
/// override beats shell detection.
pub fn needs_restart(instance: &Instance, session: &crate::tmux::Session) -> (bool, bool) {
    let exists = session.exists();
    let pane_dead = exists && session.is_pane_dead();
    let needs = if !exists || pane_dead {
        true
    } else if crate::hooks::read_hook_status(&instance.id).is_some()
        || instance.has_command_override()
    {
        false
    } else {
        !instance.expects_shell() && session.is_pane_running_shell()
    };
    (exists, needs)
}

/// The target's state from an instance snapshot; dialog gates are unknown here.
pub fn target_state(instance: &Instance) -> TargetState {
    if instance.is_structured() {
        return TargetState {
            structured: true,
            ..Default::default()
        };
    }
    let shelved = instance.ensure_startable().is_err();
    let Ok(session) = instance.tmux_session() else {
        return TargetState {
            shelved,
            ..Default::default()
        };
    };
    let (exists, needs_restart) = needs_restart(instance, &session);
    TargetState {
        structured: false,
        shelved,
        exists,
        needs_restart,
        needs_input: false,
    }
}

/// A request forwarded to the main loop, which must answer exactly once.
pub struct Incoming {
    pub session_id: String,
    pub reply: mpsc::Sender<FocusResult>,
}

/// Switches the TUI's own tmux client; injectable for tests.
pub type Switcher = Box<dyn Fn(&str, &str) -> bool + Send>;

/// The TUI is blocked inside `tmux attach`; the listener answers directly.
pub struct Attached {
    pub client_tty: String,
    pub showing: String,
    /// Every session shown during this attach, for return bookkeeping.
    pub shown: Vec<String>,
    pub instances: HashMap<String, Instance>,
    pub switch: Switcher,
    /// Live target state; `target_state` outside tests.
    pub state_of: fn(&Instance) -> TargetState,
}

pub enum Mode {
    Dashboard,
    Attached(Attached),
}

/// Real switcher: `switch-client -c <tty> -t <session>` on AoE's own client.
pub fn tmux_switcher() -> Switcher {
    Box::new(|tty, session| {
        crate::tmux::tmux_command()
            .args(["switch-client", "-c", tty, "-t", session])
            .status()
            .is_ok_and(|s| s.success())
    })
}

fn answer_attached(state: &mut Attached, target_id: &str) -> FocusResult {
    if state.showing == target_id {
        return FocusResult::AlreadyShowing;
    }
    let Some(instance) = state.instances.get(target_id) else {
        return FocusResult::UnknownSession;
    };
    match decide(Some((state.state_of)(instance))) {
        FocusResult::Accepted => {}
        // The restart flow and its dialogs need the main loop.
        FocusResult::Starting | FocusResult::NeedsInput => return FocusResult::Busy,
        other => return other,
    }
    let Ok(session) = instance.tmux_session() else {
        return FocusResult::Error;
    };
    if !(state.switch)(&state.client_tty, session.name()) {
        return FocusResult::Error;
    }
    state.showing = target_id.to_string();
    state.shown.push(target_id.to_string());
    FocusResult::Accepted
}

pub struct Control {
    dir: PathBuf,
    record: Mutex<TuiRecord>,
    pub mode: Arc<Mutex<Mode>>,
}

impl Control {
    pub fn new(dir: PathBuf, record: TuiRecord) -> Arc<Self> {
        Arc::new(Self {
            dir,
            record: Mutex::new(record),
            mode: Arc::new(Mutex::new(Mode::Dashboard)),
        })
    }

    pub fn publish(&self, attached: Option<&str>) {
        let mut record = self.record.lock().expect("tui control record");
        record.attached_session_id = attached.map(str::to_owned);
        if let Err(e) = tui_control::write_record(&self.dir, &record) {
            tracing::warn!(target: "tui.focus", "failed to publish TUI control record: {e}");
        }
    }

    pub fn remove(&self) {
        let pid = self.record.lock().expect("tui control record").pid;
        tui_control::remove_own(&self.dir, pid);
    }

    pub fn tty(&self) -> String {
        self.record.lock().expect("tui control record").tty.clone()
    }

    fn socket(&self) -> PathBuf {
        let pid = self.record.lock().expect("tui control record").pid;
        tui_control::socket_path(&self.dir, pid)
    }

    /// Bind the owner-only socket and serve requests on a thread. Requests in
    /// dashboard mode are forwarded through `forward`.
    pub fn listen(
        self: &Arc<Self>,
        forward: tokio::sync::mpsc::UnboundedSender<Incoming>,
    ) -> std::io::Result<()> {
        let path = self.socket();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let control = Arc::clone(self);
        thread::Builder::new()
            .name("aoe-tui-focus".to_string())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    control.serve(stream, &forward);
                }
            })?;
        Ok(())
    }

    fn serve(
        &self,
        mut stream: UnixStream,
        forward: &tokio::sync::mpsc::UnboundedSender<Incoming>,
    ) {
        if !peer_is_owner(&stream) {
            return;
        }
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let Some(request) = read_request(&stream) else {
            return;
        };
        let result = self.handle(&request.focus, forward);
        if result == FocusResult::Accepted {
            if let Mode::Attached(state) = &*self.mode.lock().expect("focus mode") {
                self.publish(Some(&state.showing));
            }
        }
        let reply = FocusReply {
            result,
            nonce: request.nonce,
        };
        if let Ok(mut line) = serde_json::to_vec(&reply) {
            line.push(b'\n');
            let _ = stream.write_all(&line);
        }
    }

    fn handle(
        &self,
        target_id: &str,
        forward: &tokio::sync::mpsc::UnboundedSender<Incoming>,
    ) -> FocusResult {
        {
            let mut mode = self.mode.lock().expect("focus mode");
            if let Mode::Attached(state) = &mut *mode {
                return answer_attached(state, target_id);
            }
        }
        let (tx, rx) = mpsc::channel();
        if forward
            .send(Incoming {
                session_id: target_id.to_string(),
                reply: tx,
            })
            .is_err()
        {
            return FocusResult::Error;
        }
        rx.recv_timeout(MAIN_LOOP_WAIT).unwrap_or(FocusResult::Busy)
    }
}

fn peer_is_owner(stream: &UnixStream) -> bool {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: valid socket fd and out-pointers for the duration of the call.
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0;
    ok && uid == nix::unistd::getuid().as_raw()
}

fn read_request(stream: &UnixStream) -> Option<FocusRequest> {
    let mut line = String::new();
    BufReader::new(stream.take(MAX_REQUEST))
        .read_line(&mut line)
        .ok()?;
    if !line.ends_with('\n') {
        return None;
    }
    let request: FocusRequest = serde_json::from_str(line.trim_end()).ok()?;
    let id_ok = !request.focus.is_empty()
        && request.focus.len() <= 128
        && request
            .focus
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    let nonce_ok = (8..=64).contains(&request.nonce.len())
        && request.nonce.bytes().all(|b| b.is_ascii_hexdigit());
    (id_ok && nonce_ok).then_some(request)
}

/// Send one request to a TUI's socket and wait for its reply.
pub fn send(
    socket: &Path,
    session_id: &str,
    nonce: &str,
    timeout: Duration,
) -> std::io::Result<FocusReply> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut line = serde_json::to_vec(&serde_json::json!({"focus": session_id, "nonce": nonce}))?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reply = String::new();
    BufReader::new(stream.take(MAX_REQUEST)).read_line(&mut reply)?;
    let reply: FocusReply = serde_json::from_str(reply.trim_end())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if reply.nonce != nonce {
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

    fn state(
        structured: bool,
        shelved: bool,
        exists: bool,
        needs_restart: bool,
        needs_input: bool,
    ) -> TargetState {
        TargetState {
            structured,
            shelved,
            exists,
            needs_restart,
            needs_input,
        }
    }

    #[test]
    fn dashboard_decisions_cover_the_closed_vocabulary() {
        use FocusResult::*;
        for (target, want) in [
            (None, UnknownSession),
            (Some(state(true, false, true, false, false)), Error),
            (Some(state(false, true, true, false, false)), Shelved),
            (Some(state(false, false, false, true, false)), NotRunning),
            (Some(state(false, false, true, true, true)), NeedsInput),
            (Some(state(false, false, true, true, false)), Starting),
            (Some(state(false, false, true, false, false)), Accepted),
        ] {
            assert_eq!(decide(target), want, "{target:?}");
        }
    }

    fn short_tmp() -> tempfile::TempDir {
        // macOS caps socket paths at 104 bytes.
        tempfile::TempDir::with_prefix_in("aoe-focus-", "/tmp").unwrap()
    }

    fn control(dir: &Path) -> Arc<Control> {
        let dir = dir.join("tui-control");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        Control::new(
            dir,
            TuiRecord {
                protocol: tui_control::PROTOCOL,
                pid: std::process::id(),
                start_id: "s".into(),
                tty: "/dev/ttys001".into(),
                profile: "p".into(),
                attached_session_id: None,
            },
        )
    }

    #[test]
    fn dashboard_requests_round_trip_through_the_main_loop() {
        let tmp = short_tmp();
        let control = control(tmp.path());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Incoming>();
        control.listen(tx).unwrap();
        let socket = control.socket();
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let main = thread::spawn(move || {
            let incoming = rx.blocking_recv().unwrap();
            assert_eq!(incoming.session_id, "abc123");
            incoming.reply.send(FocusResult::Starting).unwrap();
        });
        let reply = send(
            &socket,
            "abc123",
            "0123456789abcdef",
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(reply.result, FocusResult::Starting);
        main.join().unwrap();
    }

    #[test]
    fn an_unresponsive_main_loop_answers_busy() {
        let tmp = short_tmp();
        let control = control(tmp.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<Incoming>();
        control.listen(tx).unwrap();
        let reply = send(
            &control.socket(),
            "abc",
            "0123456789abcdef",
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(reply.result, FocusResult::Busy);
    }

    #[test]
    fn malformed_requests_get_no_reply() {
        let tmp = short_tmp();
        let control = control(tmp.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<Incoming>();
        control.listen(tx).unwrap();
        for bad in [
            "{\"focus\":\"a b\",\"nonce\":\"0123456789abcdef\"}\n",
            "{\"focus\":\"abc\",\"nonce\":\"short\"}\n",
            "{\"focus\":\"abc\",\"nonce\":\"0123456789abcdef\",\"extra\":1}\n",
            "{\"focus\":\"abc\",\"nonce\":\"0123456789abcdef\"}",
        ] {
            let mut stream = UnixStream::connect(control.socket()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream.write_all(bad.as_bytes()).unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut out = String::new();
            let _ = stream.read_to_string(&mut out);
            assert!(out.is_empty(), "replied to {bad:?}: {out}");
        }
    }

    type Calls = Arc<Mutex<Vec<(String, String)>>>;

    fn attached(
        switch_ok: bool,
        calls: Calls,
        state_of: fn(&Instance) -> TargetState,
    ) -> (Attached, Vec<Instance>) {
        let live = Instance::new("live", "/tmp/live");
        let restart = Instance::new("restart", "/tmp/restart");
        let instances = [&live, &restart]
            .into_iter()
            .map(|i| (i.id.clone(), i.clone()))
            .collect();
        let state = Attached {
            client_tty: "/dev/ttys001".into(),
            showing: "current".into(),
            shown: vec!["current".into()],
            instances,
            switch: Box::new(move |tty, session| {
                calls.lock().unwrap().push((tty.into(), session.into()));
                switch_ok
            }),
            state_of,
        };
        (state, vec![live, restart])
    }

    fn by_title(i: &Instance) -> TargetState {
        state(false, false, true, i.title == "restart", false)
    }

    #[test]
    fn attached_requests_switch_only_aoes_own_client() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let (mut state, insts) = attached(true, calls.clone(), by_title);
        let (live, restart) = (&insts[0], &insts[1]);
        assert_eq!(
            answer_attached(&mut state, "current"),
            FocusResult::AlreadyShowing
        );
        assert_eq!(
            answer_attached(&mut state, "missing"),
            FocusResult::UnknownSession
        );
        assert_eq!(
            answer_attached(&mut state, &restart.id),
            FocusResult::Busy,
            "restart needs the main loop"
        );
        assert!(calls.lock().unwrap().is_empty(), "no switch for refusals");

        assert_eq!(answer_attached(&mut state, &live.id), FocusResult::Accepted);
        let expected = (
            "/dev/ttys001".to_string(),
            live.tmux_session().unwrap().name().to_string(),
        );
        assert_eq!(*calls.lock().unwrap(), vec![expected]);
        assert_eq!(state.showing, live.id);
        assert_eq!(state.shown, vec!["current".to_string(), live.id.clone()]);
        assert_eq!(
            answer_attached(&mut state, &live.id),
            FocusResult::AlreadyShowing
        );
    }

    #[test]
    fn a_failed_switch_reports_error_and_keeps_state() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let (mut state, insts) = attached(false, calls, by_title);
        assert_eq!(
            answer_attached(&mut state, &insts[0].id),
            FocusResult::Error
        );
        assert_eq!(state.showing, "current");
        assert_eq!(state.shown, vec!["current".to_string()]);
    }
}
