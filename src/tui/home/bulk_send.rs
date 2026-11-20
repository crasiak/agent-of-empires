//! Bulk send: one message to every eligible marked Session.
//!
//! Delivery goes through the daemon's send endpoints on a worker thread. The home
//! view's interactive send holds one pending send tied to the cursor row, so it
//! cannot be looped.

use crate::acp::client::{locate_daemon, HttpClient, HttpError};
use crate::tui::worker::Worker;

use super::bulk::BulkAction;
use super::*;
use crate::tui::dialogs::SendMessageDialog;

pub(super) struct BulkSendTarget {
    pub id: String,
    pub title: String,
    pub structured: bool,
}

pub(super) struct BulkSendRequest {
    pub targets: Vec<BulkSendTarget>,
    pub message: String,
}

pub(super) struct BulkSendResult {
    pub sent: usize,
    /// `(title, reason)` for each Session the message did not reach.
    pub failed: Vec<(String, String)>,
}

/// The daemon answers a refusal with a short code; the wording lives here.
fn refusal_text(error: HttpError) -> String {
    match &error {
        // A structured prompt sent with `no_revive` to a stopped agent.
        HttpError::RevivalRequired => "not running".to_string(),
        HttpError::Server { body, .. } => match body.as_str() {
            "session_not_running" => "not running".to_string(),
            "session_archived" => "archived".to_string(),
            "session_trashed" => "in the trash".to_string(),
            _ => error.to_string(),
        },
        _ => error.to_string(),
    }
}

/// Send to each target in turn. One failure does not stop the rest.
async fn deliver(request: BulkSendRequest) -> BulkSendResult {
    let fail_all = |reason: String, targets: Vec<BulkSendTarget>| BulkSendResult {
        sent: 0,
        failed: targets
            .into_iter()
            .map(|target| (target.title, reason.clone()))
            .collect(),
    };
    let endpoint = match locate_daemon().await {
        Ok(endpoint) => endpoint,
        Err(error) => return fail_all(error.to_string(), request.targets),
    };
    let client = match HttpClient::new(endpoint) {
        Ok(client) => client,
        Err(error) => return fail_all(error.to_string(), request.targets),
    };
    let mut result = BulkSendResult {
        sent: 0,
        failed: Vec::new(),
    };
    for target in request.targets {
        // `no_revive`: a dead agent fails here instead of being started.
        let outcome = if target.structured {
            client
                .prompt(&target.id, &request.message, true)
                .await
                .map(|_| ())
        } else {
            client
                .send_message(&target.id, &request.message, true)
                .await
        };
        match outcome {
            Ok(()) => result.sent += 1,
            Err(error) => result.failed.push((target.title, refusal_text(error))),
        }
    }
    result
}

/// One-shot background sender. Spawned per bulk send and dropped with its result.
pub(super) struct BulkSendWorker {
    worker: Worker<BulkSendRequest, BulkSendResult>,
}

impl BulkSendWorker {
    fn spawn() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        Self {
            worker: Worker::spawn(
                "aoe-bulk-send",
                move |request: BulkSendRequest| match runtime.as_ref() {
                    Ok(runtime) => runtime.block_on(deliver(request)),
                    Err(error) => BulkSendResult {
                        sent: 0,
                        failed: request
                            .targets
                            .into_iter()
                            .map(|target| (target.title, format!("send worker: {error}")))
                            .collect(),
                    },
                },
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn seeded_for_test(result: BulkSendResult) -> Self {
        Self {
            worker: Worker::seeded_for_test("aoe-bulk-send-test", result),
        }
    }
}

impl HomeView {
    /// Open the compose dialog for every marked Session that can take a message.
    pub(super) fn open_bulk_send_dialog(&mut self) {
        if self.bulk_send_worker.is_some() {
            self.flash_status("A bulk send is still running");
            return;
        }
        let plan = self.bulk_plan(BulkAction::Send);
        if plan.eligible.is_empty() {
            self.info_dialog = Some(InfoDialog::sized_to_fit(
                "Nothing to Send To",
                &format!(
                    "None of the marked sessions can take a message.\n\n{}",
                    plan.skipped_lines()
                ),
            ));
            return;
        }
        let count = plan.eligible.len();
        let noun = if count == 1 { "session" } else { "sessions" };
        let label = if plan.skipped.is_empty() {
            format!("{count} marked {noun}")
        } else {
            format!("{count} marked {noun}, {} skipped", plan.skipped.len())
        };
        // Only one send slot is ever set, so a submit cannot reach both.
        self.pending_send_session = None;
        self.pending_bulk_send = Some(
            plan.eligible
                .into_iter()
                .map(|c| BulkSendTarget {
                    id: c.id,
                    title: c.title,
                    structured: c.structured,
                })
                .collect(),
        );
        let mut dialog = SendMessageDialog::new(&label);
        if let Some(buf) = self.pending_paste.take() {
            if !buf.is_empty() {
                dialog.handle_paste(&buf);
            }
        }
        self.send_message_dialog = Some(dialog);
    }

    /// Hand the message to the worker and clear the Marks.
    pub(super) fn submit_bulk_send(&mut self, targets: Vec<BulkSendTarget>, message: String) {
        if message.trim().is_empty() || targets.is_empty() {
            return;
        }
        let count = targets.len();
        let worker = BulkSendWorker::spawn();
        worker.worker.request(BulkSendRequest { targets, message });
        self.bulk_send_worker = Some(worker);
        self.clear_marks();
        let noun = if count == 1 { "session" } else { "sessions" };
        self.flash_status(format!("Sending to {count} {noun}..."));
    }

    /// Report a finished bulk send. Returns whether the screen needs a redraw.
    pub fn tick_bulk_send(&mut self) -> bool {
        let Some(worker) = self.bulk_send_worker.as_ref() else {
            return false;
        };
        let result = match worker.worker.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => BulkSendResult {
                sent: 0,
                failed: vec![(
                    "bulk send".to_string(),
                    "the send worker stopped".to_string(),
                )],
            },
        };
        self.bulk_send_worker = None;
        let noun = if result.sent == 1 {
            "session"
        } else {
            "sessions"
        };
        if result.failed.is_empty() {
            self.flash_status(format!("Sent to {} {noun}", result.sent));
        } else {
            let lines: Vec<String> = result
                .failed
                .iter()
                .map(|(title, reason)| format!("  {title}: {reason}"))
                .collect();
            self.info_dialog = Some(InfoDialog::sized_to_fit(
                "Bulk Send",
                &format!(
                    "Sent to {} {noun}. {} did not get the message:\n\n{}",
                    result.sent,
                    result.failed.len(),
                    lines.join("\n")
                ),
            ));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_codes_get_plain_wording() {
        for (code, want) in [
            ("session_not_running", "not running"),
            ("session_archived", "archived"),
            ("session_trashed", "in the trash"),
        ] {
            let error = HttpError::Server {
                status: reqwest::StatusCode::CONFLICT,
                body: code.to_string(),
            };
            assert_eq!(refusal_text(error), want, "{code}");
        }
        assert_eq!(refusal_text(HttpError::RevivalRequired), "not running");
    }
}
