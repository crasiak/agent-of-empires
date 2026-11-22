//! Breakout from the home view: build the request from the Marked set, open the
//! surfaces off the event loop, remember them so they can be closed, and close a
//! Session's window when the Session is stopped, trashed or deleted.

use crate::breakout::request::{
    BreakoutRequest, BreakoutSession, CONFIRM_ABOVE, MAX_SESSIONS, PROTOCOL,
};

use super::bulk::BulkAction;
use super::*;

impl HomeView {
    /// Check the Marked set and either ask for confirmation or open the Breakout.
    pub(super) fn start_breakout(&mut self) {
        if self.breakout_busy() {
            return;
        }
        if let Transport::Unavailable(why) = transport() {
            self.info_dialog = Some(InfoDialog::new("Breakout Unavailable", why));
            return;
        }
        let plan = self.bulk_plan(BulkAction::Breakout);
        let count = plan.eligible.len();
        if count == 0 {
            self.info_dialog = Some(InfoDialog::sized_to_fit(
                "Nothing to Break Out",
                &format!(
                    "None of the marked sessions can be opened in a window.\n\n{}",
                    plan.skipped_lines()
                ),
            ));
            return;
        }
        if count > MAX_SESSIONS {
            self.info_dialog = Some(InfoDialog::new(
                "Too Many Sessions",
                &format!("Breakout opens at most {MAX_SESSIONS} sessions; {count} are marked."),
            ));
            return;
        }
        if count > CONFIRM_ABOVE {
            if self.confirm_dialog.is_some() {
                return;
            }
            self.pending_breakout = true;
            self.confirm_dialog = Some(
                ConfirmDialog::new(
                    "Break Out",
                    &format!("Break out {count} sessions?"),
                    "breakout",
                )
                .neutral()
                .buttons("Open", "Cancel"),
            );
            return;
        }
        self.run_breakout();
    }

    /// Build the request and open it on a background thread.
    pub(super) fn run_breakout(&mut self) {
        self.pending_breakout = false;
        if self.breakout_busy() {
            return;
        }
        let plan = self.bulk_plan(BulkAction::Breakout);
        let sessions: Vec<BreakoutSession> = plan
            .eligible
            .iter()
            .filter_map(|candidate| {
                let inst = self.get_instance(&candidate.id)?;
                Some(BreakoutSession {
                    id: inst.id.clone(),
                    title: inst.title.clone(),
                    profile: inst.source_profile.clone(),
                })
            })
            .collect();
        if sessions.is_empty() {
            return;
        }
        let layout = resolve_config_or_warn(&self.config_profile())
            .breakout
            .layout;
        let transport = transport();
        let request = BreakoutRequest {
            protocol: PROTOCOL,
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            host: match &transport {
                Transport::Agent { host, .. } => Some(host.clone()),
                _ => None,
            },
            layout,
            sessions,
        };
        // A local Breakout first sweeps windows whose Session no longer exists in
        // any loaded profile.
        let keep: Vec<String> = self
            .instances
            .keys()
            .map(|id| crate::breakout::id_prefix(id))
            .collect();
        self.breakout_opening_titles = crate::breakout::window_titles(&request);
        self.breakout_job = Some(spawn_open(request, transport, keep));
        self.flash_status("Opening breakout windows...");
    }

    /// Report a finished Breakout. Returns whether the screen needs a redraw.
    pub fn tick_breakout(&mut self) -> bool {
        let redraw = self.tick_open_job() | self.tick_close_job() | self.tick_session_close_job();
        self.drain_breakout_close_queue();
        redraw
    }

    /// Whether this TUI opened a Breakout window for the Session and has not closed it.
    pub fn is_broken_out(&self, session_id: &str) -> bool {
        self.breakout_titles
            .iter()
            .any(|title| crate::breakout::title_is_for(title, session_id))
    }

    /// Close the Breakout window of a Session that is going away. Its titles leave
    /// `breakout_titles` now; the close waits behind any running job. A splits
    /// window holds other Sessions too, so it stays: the Session's pane ends when
    /// its attach exits, and the window keeps its title until it is closed.
    pub(super) fn close_breakout_of(&mut self, session_id: &str) {
        let own_window = |title: &String| {
            crate::breakout::title_is_for(title, session_id)
                && !crate::breakout::is_splits_title(title)
        };
        let (gone, kept): (Vec<String>, Vec<String>) = std::mem::take(&mut self.breakout_titles)
            .into_iter()
            .partition(own_window);
        self.breakout_titles = kept;
        // A window still being opened closes once the open job lands.
        let opening = self
            .breakout_opening_titles
            .iter()
            .filter(|title| own_window(title))
            .filter(|title| !gone.contains(title))
            .cloned()
            .collect::<Vec<_>>();
        if gone.is_empty() && opening.is_empty() {
            return;
        }
        self.breakout_close_queue.extend(gone);
        self.breakout_close_queue.extend(opening);
        self.drain_breakout_close_queue();
    }

    fn any_breakout_job(&self) -> bool {
        self.breakout_job.is_some()
            || self.breakout_close_job.is_some()
            || self.breakout_session_close_job.is_some()
    }

    fn drain_breakout_close_queue(&mut self) {
        if self.breakout_close_queue.is_empty() || self.any_breakout_job() {
            return;
        }
        if under_ssh() {
            // The windows are on the machine with the screen; nothing here can close them.
            self.breakout_close_queue.clear();
            self.flash_status(REMOTE_CLOSE);
            return;
        }
        let titles = std::mem::take(&mut self.breakout_close_queue);
        self.breakout_session_close_job = Some(spawn_close(titles));
    }

    /// Refuse a Breakout or close while another is still running.
    fn breakout_busy(&mut self) -> bool {
        let busy = self.any_breakout_job();
        if busy {
            self.flash_status("A breakout is still opening or closing");
        }
        busy
    }

    fn tick_open_job(&mut self) -> bool {
        let Some(job) = self.breakout_job.as_ref() else {
            return false;
        };
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the breakout worker stopped".to_string())
            }
        };
        self.breakout_job = None;
        self.breakout_opening_titles.clear();
        match result {
            Ok(outcome) => {
                self.clear_marks();
                // Titles queued to close went with their Sessions while this job ran.
                let queued = &self.breakout_close_queue;
                self.breakout_titles.extend(
                    outcome
                        .titles
                        .into_iter()
                        .filter(|title| !queued.contains(title)),
                );
                match outcome.note {
                    Some(note) => {
                        self.info_dialog = Some(InfoDialog::sized_to_fit("Breakout", &note));
                    }
                    None => self.flash_status(format!("Opened {} sessions", outcome.opened)),
                }
            }
            Err(error) => {
                tracing::warn!(target: "tui.breakout", %error, "breakout failed");
                self.info_dialog = Some(InfoDialog::sized_to_fit("Breakout Failed", &error));
            }
        }
        true
    }

    fn tick_close_job(&mut self) -> bool {
        let Some(job) = self.breakout_close_job.as_ref() else {
            return false;
        };
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the close worker stopped".to_string())
            }
        };
        self.breakout_close_job = None;
        match result {
            Ok(closed) => {
                self.breakout_titles.clear();
                self.flash_status(match closed {
                    0 => "No breakout windows were open".to_string(),
                    1 => "Closed 1 breakout window".to_string(),
                    n => format!("Closed {n} breakout windows"),
                });
            }
            Err(error) => {
                tracing::warn!(target: "tui.breakout", %error, "closing breakout windows failed");
                self.info_dialog = Some(InfoDialog::sized_to_fit("Close Failed", &error));
            }
        }
        true
    }

    fn tick_session_close_job(&mut self) -> bool {
        let Some(job) = self.breakout_session_close_job.as_ref() else {
            return false;
        };
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the close worker stopped".to_string())
            }
        };
        self.breakout_session_close_job = None;
        if let Err(error) = result {
            tracing::warn!(target: "tui.breakout", %error, "closing a session's breakout window failed");
            self.flash_status("Could not close the breakout window; use Close breakout windows");
        }
        true
    }

    /// Close every Breakout window on this machine, tracked or not, off the event loop.
    pub(super) fn close_breakout(&mut self) {
        if self.breakout_busy() {
            return;
        }
        if under_ssh() {
            // Only the markers are here; the windows are closed where they are shown.
            self.breakout_titles.clear();
            self.flash_status(REMOTE_CLOSE);
            return;
        }
        if !crate::breakout::supported_here() {
            self.flash_status("Breakout needs macOS with Ghostty");
            return;
        }
        self.breakout_close_job = Some(spawn_sweep());
        self.flash_status("Closing breakout windows...");
    }
}

/// How long the agent may take to open and tile the windows.
#[cfg(not(test))]
const AGENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Shown where a remote TUI would close windows: they are on another machine.
const REMOTE_CLOSE: &str = "Breakout windows are closed on the machine with the screen";

/// Where a Breakout from this TUI opens its windows.
enum Transport {
    /// This machine has the screen.
    Local,
    /// aoe is being used over ssh; ask the agent on the machine with the screen.
    Agent {
        socket: std::path::PathBuf,
        host: String,
    },
    Unavailable(&'static str),
}

/// Whether this TUI is shown through ssh, so its screen is on another machine.
fn under_ssh() -> bool {
    #[cfg(test)]
    {
        test_support::UNDER_SSH.with(|u| u.get())
    }
    #[cfg(not(test))]
    {
        ssh_env_present()
    }
}

fn ssh_env_present() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

fn short_host_name() -> Option<String> {
    let name = nix::unistd::gethostname().ok()?.into_string().ok()?;
    let short = name.split('.').next().unwrap_or_default();
    (!short.is_empty()).then(|| short.to_string())
}

fn transport() -> Transport {
    if under_ssh() {
        let socket = crate::breakout::agent::socket_path()
            .ok()
            .filter(|p| p.exists());
        return match (socket, short_host_name()) {
            (Some(socket), Some(host)) => Transport::Agent { socket, host },
            _ => Transport::Unavailable(
                "aoe is running over ssh and no Breakout agent socket is forwarded. \
                 See docs/guides/breakout.md.",
            ),
        };
    }
    if crate::breakout::supported_here() {
        Transport::Local
    } else {
        Transport::Unavailable("Breakout needs macOS with Ghostty on the machine showing aoe.")
    }
}

/// Whether "Close breakout windows" does anything here: closes local windows, or
/// clears the markers of windows shown on another machine.
pub(super) fn close_offered() -> bool {
    under_ssh() || crate::breakout::supported_here()
}

/// The agent's answer as a Breakout result. A silent agent is never a reason to
/// open windows on this machine instead.
fn agent_outcome(
    reply: std::io::Result<crate::breakout::agent::AgentReply>,
) -> Result<crate::breakout::BreakoutOutcome, String> {
    use crate::breakout::agent::AgentResult;
    let reply = reply.map_err(|error| {
        format!(
            "The Breakout agent did not answer ({error}). Check that `aoe breakout-agent` \
             is running on the machine with the screen and that your ssh connection \
             forwards its socket."
        )
    })?;
    let message = reply.message.unwrap_or_default();
    match (reply.result, reply.outcome) {
        (AgentResult::Opened, Some(outcome)) => Ok(outcome),
        (AgentResult::Opened, None) => Err("The Breakout agent sent no result.".to_string()),
        (AgentResult::Refused, _) => Err(format!("The Breakout agent refused: {message}")),
        (AgentResult::Failed, _) => Err(format!(
            "The Breakout agent could not open the windows: {message}"
        )),
    }
}

type OpenJob = std::sync::mpsc::Receiver<Result<crate::breakout::BreakoutOutcome, String>>;
type SweepJob = std::sync::mpsc::Receiver<Result<usize, String>>;
type CloseJob = std::sync::mpsc::Receiver<Result<(), String>>;

/// Open through the transport. Locally, windows of Sessions outside `keep` are
/// swept first (a failure is only logged); the agent never sweeps.
#[cfg(not(test))]
fn spawn_open(request: BreakoutRequest, transport: Transport, keep: Vec<String>) -> OpenJob {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = match transport {
            Transport::Local => {
                match crate::breakout::sweep_local(crate::breakout::SweepScope::Orphans {
                    keep_id_prefixes: &keep,
                }) {
                    Ok(closed) => {
                        tracing::debug!(target: "tui.breakout", closed, "swept orphaned breakout windows")
                    }
                    Err(error) => {
                        tracing::warn!(target: "tui.breakout", %error, "sweeping orphaned breakout windows failed")
                    }
                }
                crate::breakout::run_local(&request)
            }
            Transport::Agent { socket, .. } => agent_outcome(crate::breakout::agent::send(
                &socket,
                &request,
                AGENT_TIMEOUT,
            )),
            Transport::Unavailable(why) => Err(why.to_string()),
        };
        let _ = tx.send(result);
    });
    rx
}

/// Tests never reach the driver: the job answers as if every Session opened,
/// with the window titles a real Breakout gives them.
#[cfg(test)]
fn spawn_open(request: BreakoutRequest, transport: Transport, _keep: Vec<String>) -> OpenJob {
    test_support::OPENED_FOR.with(|o| {
        *o.borrow_mut() = Some(test_support::Opened {
            request_host: request.host.clone(),
            agent: match transport {
                Transport::Agent { socket, host } => Some((socket, host)),
                _ => None,
            },
        })
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let titles = crate::breakout::window_titles(&request);
    let _ = tx.send(Ok(crate::breakout::BreakoutOutcome {
        opened: request.sessions.len(),
        tiled: true,
        titles,
        note: None,
    }));
    rx
}

#[cfg(not(test))]
fn spawn_sweep() -> SweepJob {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::breakout::sweep_local(
            crate::breakout::SweepScope::Everything,
        ));
    });
    rx
}

/// Tests never reach the driver: the sweep answers as if nothing was open.
#[cfg(test)]
fn spawn_sweep() -> SweepJob {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(Ok(0));
    rx
}

#[cfg(not(test))]
fn spawn_close(titles: Vec<String>) -> CloseJob {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::breakout::close_local(&titles));
    });
    rx
}

/// Tests never reach the driver: the job answers as if the windows closed.
#[cfg(test)]
fn spawn_close(_titles: Vec<String>) -> CloseJob {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(Ok(()));
    rx
}

#[cfg(test)]
pub(super) mod test_support {
    use std::cell::{Cell, RefCell};

    thread_local! {
        /// Stands in for the ssh environment so tests do not depend on how they are run.
        pub(in crate::tui::home) static UNDER_SSH: Cell<bool> = const { Cell::new(false) };
        /// How the last Breakout was opened.
        pub(in crate::tui::home) static OPENED_FOR: RefCell<Option<Opened>> =
            const { RefCell::new(None) };
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(in crate::tui::home) struct Opened {
        pub(in crate::tui::home) request_host: Option<String>,
        /// The agent socket and host, when the agent transport was used.
        pub(in crate::tui::home) agent: Option<(std::path::PathBuf, String)>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakout::agent::{AgentReply, AgentResult};
    use serial_test::serial;

    fn reply(result: AgentResult, opened: Option<usize>, message: Option<&str>) -> AgentReply {
        AgentReply {
            nonce: "0123456789abcdef".to_string(),
            result,
            outcome: opened.map(|opened| crate::breakout::BreakoutOutcome {
                opened,
                tiled: true,
                titles: vec![],
                note: None,
            }),
            message: message.map(str::to_string),
        }
    }

    #[test]
    fn a_silent_agent_is_an_error_that_names_the_agent_and_the_forward() {
        let timed_out = std::io::Error::new(std::io::ErrorKind::WouldBlock, "timed out");
        let error = agent_outcome(Err(timed_out)).unwrap_err();
        assert!(error.contains("Breakout agent"), "{error}");
        assert!(error.contains("ssh"), "{error}");
        assert!(error.contains("forwards its socket"), "{error}");
    }

    #[test]
    fn agent_replies_become_breakout_results() {
        assert_eq!(
            agent_outcome(Ok(reply(AgentResult::Opened, Some(2), None)))
                .unwrap()
                .opened,
            2
        );
        let refused = agent_outcome(Ok(reply(
            AgentResult::Refused,
            None,
            Some("host x is not in breakout.agent_hosts"),
        )))
        .unwrap_err();
        assert!(refused.contains("breakout.agent_hosts"), "{refused}");
        let failed =
            agent_outcome(Ok(reply(AgentResult::Failed, None, Some("no Ghostty")))).unwrap_err();
        assert!(failed.contains("no Ghostty"), "{failed}");
        assert!(agent_outcome(Ok(reply(AgentResult::Opened, None, None))).is_err());
    }

    #[test]
    #[serial]
    fn ssh_is_detected_from_the_environment() {
        let _none = crate::session::test_support::EnvGuard::unset(&["SSH_CONNECTION", "SSH_TTY"]);
        assert!(!ssh_env_present());
        drop(_none);
        let _conn = crate::session::test_support::EnvGuard::unset(&["SSH_TTY"])
            .and_set("SSH_CONNECTION", "10.0.0.1 2 10.0.0.2 22");
        assert!(ssh_env_present());
        drop(_conn);
        let _tty = crate::session::test_support::EnvGuard::unset(&["SSH_CONNECTION"])
            .and_set("SSH_TTY", "/dev/ttys004");
        assert!(ssh_env_present());
    }
}
