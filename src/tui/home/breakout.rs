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
        if !crate::breakout::supported_here() {
            self.info_dialog = Some(InfoDialog::new(
                "Breakout Unavailable",
                "Breakout needs macOS with Ghostty on the machine showing aoe.",
            ));
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
        let request = BreakoutRequest {
            protocol: PROTOCOL,
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            host: None,
            layout,
            sessions,
        };
        // Windows whose Session no longer exists in any loaded profile are swept first.
        let keep: Vec<String> = self
            .instances
            .keys()
            .map(|id| crate::breakout::id_prefix(id))
            .collect();
        self.breakout_opening_titles = crate::breakout::window_titles(&request);
        self.breakout_job = Some(spawn_open(request, keep));
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
        if !crate::breakout::supported_here() {
            self.flash_status("Breakout needs macOS with Ghostty");
            return;
        }
        self.breakout_close_job = Some(spawn_sweep());
        self.flash_status("Closing breakout windows...");
    }
}

type OpenJob = std::sync::mpsc::Receiver<Result<crate::breakout::BreakoutOutcome, String>>;
type SweepJob = std::sync::mpsc::Receiver<Result<usize, String>>;
type CloseJob = std::sync::mpsc::Receiver<Result<(), String>>;

/// Sweep windows of Sessions outside `keep` (a failure is only logged), then open.
#[cfg(not(test))]
fn spawn_open(request: BreakoutRequest, keep: Vec<String>) -> OpenJob {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        match crate::breakout::sweep_local(&keep) {
            Ok(closed) => {
                tracing::debug!(target: "tui.breakout", closed, "swept orphaned breakout windows")
            }
            Err(error) => {
                tracing::warn!(target: "tui.breakout", %error, "sweeping orphaned breakout windows failed")
            }
        }
        let _ = tx.send(crate::breakout::run_local(&request));
    });
    rx
}

/// Tests never reach the driver: the job answers as if every Session opened,
/// with the window titles a real Breakout gives them.
#[cfg(test)]
fn spawn_open(request: BreakoutRequest, _keep: Vec<String>) -> OpenJob {
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
        let _ = tx.send(crate::breakout::sweep_local(&[]));
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
