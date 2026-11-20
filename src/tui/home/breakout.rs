//! Breakout from the home view: build the request from the Marked set, open the
//! surfaces off the event loop, and remember them so they can be closed.

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
                    &format!("Open {count} sessions in separate windows?"),
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
        self.breakout_job = Some(spawn_open(request));
        self.flash_status("Opening breakout windows...");
    }

    /// Report a finished Breakout. Returns whether the screen needs a redraw.
    pub fn tick_breakout(&mut self) -> bool {
        self.tick_open_job() | self.tick_close_job()
    }

    /// Refuse a Breakout or close while another is still running.
    fn breakout_busy(&mut self) -> bool {
        let busy = self.breakout_job.is_some() || self.breakout_close_job.is_some();
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
        match result {
            Ok(outcome) => {
                self.clear_marks();
                self.breakout_titles.extend(outcome.titles);
                match outcome.note {
                    Some(note) => {
                        self.info_dialog = Some(InfoDialog::sized_to_fit("Breakout", &note));
                    }
                    None => self.flash_status(format!("Opened {} windows", outcome.opened)),
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
            Ok(()) => {
                self.breakout_titles.clear();
                self.flash_status("Closed breakout windows");
            }
            Err(error) => {
                tracing::warn!(target: "tui.breakout", %error, "closing breakout windows failed");
                self.info_dialog = Some(InfoDialog::sized_to_fit("Close Failed", &error));
            }
        }
        true
    }

    /// Close every Breakout window this TUI opened, off the event loop.
    pub(super) fn close_breakout(&mut self) {
        if self.breakout_busy() {
            return;
        }
        if self.breakout_titles.is_empty() {
            self.flash_status("No breakout windows are open");
            return;
        }
        self.breakout_close_job = Some(spawn_close(self.breakout_titles.clone()));
        self.flash_status("Closing breakout windows...");
    }
}

type OpenJob = std::sync::mpsc::Receiver<Result<crate::breakout::BreakoutOutcome, String>>;
type CloseJob = std::sync::mpsc::Receiver<Result<(), String>>;

#[cfg(not(test))]
fn spawn_open(request: BreakoutRequest) -> OpenJob {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::breakout::run_local(&request));
    });
    rx
}

/// Tests never reach the driver: the job answers as if every Session opened.
#[cfg(test)]
fn spawn_open(request: BreakoutRequest) -> OpenJob {
    let (tx, rx) = std::sync::mpsc::channel();
    let titles: Vec<String> = request.sessions.into_iter().map(|s| s.title).collect();
    let _ = tx.send(Ok(crate::breakout::BreakoutOutcome {
        opened: titles.len(),
        tiled: true,
        titles,
        note: None,
    }));
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
