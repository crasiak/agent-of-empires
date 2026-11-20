//! Bulk restart: confirm, then restart marked Sessions two at a time.

use std::collections::{HashMap, VecDeque};

use super::bulk::BulkAction;
use super::operations::RestartSubmit;
use super::*;

/// Checkbox key on the bulk-restart confirm.
pub(super) const BULK_RESTART_IDLE_ONLY: &str = "idle_only";

/// Each restart spawns a harness and a launch, so they are staggered.
const BULK_RESTART_CONCURRENCY: usize = 2;

/// A bulk restart in progress. Entries are `(session id, title)`.
pub(super) struct BulkRestartRun {
    waiting: VecDeque<(String, String)>,
    in_flight: HashMap<String, String>,
    succeeded: usize,
    failed: Vec<(String, String)>,
}

impl HomeView {
    /// Show what a bulk restart would do and ask before doing it.
    pub(super) fn open_bulk_restart_confirm(&mut self) {
        if self.confirm_dialog.is_some() {
            return;
        }
        if self.bulk_restart.is_some() {
            self.flash_status("A bulk restart is still running");
            return;
        }
        let plan = self.bulk_plan(BulkAction::Restart);
        if plan.eligible.is_empty() {
            self.info_dialog = Some(InfoDialog::sized_to_fit(
                "Nothing to Restart",
                &format!(
                    "None of the marked sessions can be restarted.\n\n{}",
                    plan.skipped_lines()
                ),
            ));
            return;
        }
        let total = plan.eligible.len();
        let busy = plan.eligible.iter().filter(|c| c.is_busy()).count();
        let noun = if total == 1 { "session" } else { "sessions" };
        let mut message = format!(
            "Restart {total} marked {noun}?\n\n{} idle, {busy} busy (running, waiting, starting or unknown)\n\n\
             Each restarts with its current profile, agent and arguments.",
            total - busy
        );
        if !plan.skipped.is_empty() {
            message.push_str(&format!("\n\nSkipped:\n{}", plan.skipped_lines()));
        }
        let mut dialog = ConfirmDialog::new("Restart Marked Sessions", &message, "bulk_restart")
            .buttons("Restart", "Cancel");
        if busy > 0 && busy < total {
            dialog = dialog.checkbox(
                BULK_RESTART_IDLE_ONLY,
                &format!("Idle only (leave {busy} busy running)"),
            );
        }
        self.pending_bulk_restart = true;
        self.confirm_dialog = Some(dialog);
    }

    /// Start the confirmed bulk restart. Eligibility is recomputed, since Sessions may
    /// have changed while the confirm was open.
    pub(super) fn start_bulk_restart(&mut self, idle_only: bool) {
        if !std::mem::take(&mut self.pending_bulk_restart) {
            return;
        }
        let plan = self.bulk_plan(BulkAction::Restart);
        let waiting: VecDeque<(String, String)> = plan
            .eligible
            .into_iter()
            .filter(|c| !(idle_only && c.is_busy()))
            .map(|c| (c.id, c.title))
            .collect();
        self.clear_marks();
        if waiting.is_empty() {
            self.flash_status("Nothing left to restart");
            return;
        }
        self.bulk_restart = Some(BulkRestartRun {
            waiting,
            in_flight: HashMap::new(),
            succeeded: 0,
            failed: Vec::new(),
        });
        self.pump_bulk_restart();
    }

    /// Submit waiting restarts up to the concurrency limit.
    fn pump_bulk_restart(&mut self) {
        let Some(mut run) = self.bulk_restart.take() else {
            return;
        };
        while run.in_flight.len() < BULK_RESTART_CONCURRENCY {
            let Some((id, title)) = run.waiting.pop_front() else {
                break;
            };
            // Restarted outside this run since it was queued: follow that restart
            // rather than restart the Session twice.
            if self.restart_in_flight.contains(&id) {
                run.in_flight.insert(id, title);
                continue;
            }
            // Another change for this Session is still in flight, or it was restarted
            // moments ago; a restart now would be refused or skipped. Retry next tick.
            if self.session_feed.has_pending(&id)
                || self.session_feed.has_queued(&id)
                || self.restart_cooling_down(&id)
            {
                run.waiting.push_front((id, title));
                break;
            }
            match self.restart_session_by_id(&id, None, None, None, None, false) {
                RestartSubmit::Submitted => {
                    run.in_flight.insert(id, title);
                }
                RestartSubmit::Skipped => run
                    .failed
                    .push((title, "no longer restartable".to_string())),
                RestartSubmit::Refused(error) => run.failed.push((title, error)),
            }
        }
        self.bulk_restart = Some(run);
    }

    /// Record daemon errors against the bulk restart they belong to. Called from
    /// `present_command_errors`, which every drained command error passes through.
    pub(super) fn note_bulk_restart_errors(
        &mut self,
        errors: &[crate::tui::session_feed::SessionCommandError],
    ) {
        let Some(run) = self.bulk_restart.as_mut() else {
            return;
        };
        for error in errors {
            if let Some(title) = run.in_flight.remove(&error.id) {
                run.failed.push((title, error.message.clone()));
            }
        }
    }

    /// Advance the bulk restart: count finished restarts, submit the next ones, and
    /// report when the queue drains. Returns whether the screen needs a redraw.
    pub fn tick_bulk_restart(&mut self) -> bool {
        let Some(mut run) = self.bulk_restart.take() else {
            return false;
        };
        // A command leaves the pending set only in `drain_command_errors`, and a failure
        // there is recorded by `note_bulk_restart_errors` before this runs. So one that
        // left without an error succeeded.
        let finished: Vec<String> = run
            .in_flight
            .keys()
            .filter(|id| !self.session_feed.has_pending(id))
            .cloned()
            .collect();
        let progressed = !finished.is_empty();
        for id in finished {
            run.in_flight.remove(&id);
            run.succeeded += 1;
        }
        let waiting_before = run.waiting.len();
        self.bulk_restart = Some(run);
        self.pump_bulk_restart();
        let Some(run) = self.bulk_restart.take() else {
            return progressed;
        };
        if !run.waiting.is_empty() || !run.in_flight.is_empty() {
            let pumped = run.waiting.len() != waiting_before;
            self.bulk_restart = Some(run);
            return progressed || pumped;
        }
        let noun = if run.succeeded == 1 {
            "session"
        } else {
            "sessions"
        };
        if run.failed.is_empty() {
            self.flash_status(format!("Restarted {} {noun}", run.succeeded));
        } else {
            let lines: Vec<String> = run
                .failed
                .iter()
                .map(|(title, error)| format!("  {title}: {error}"))
                .collect();
            self.info_dialog = Some(InfoDialog::sized_to_fit(
                "Bulk Restart",
                &format!(
                    "Restarted {} {noun}. {} did not restart:\n\n{}",
                    run.succeeded,
                    run.failed.len(),
                    lines.join("\n")
                ),
            ));
        }
        true
    }
}
