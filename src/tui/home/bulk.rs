//! Bulk actions on the Marked set: which marked Sessions an action may touch, and why not.

use super::*;
use crate::session::{Item, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BulkAction {
    Send,
    Restart,
    Breakout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkipReason {
    Trashed,
    Archived,
    Transient,
    NoAgentPane,
    AwaitingPermission,
    RestartInFlight,
    Snoozed,
    Starting,
    Structured,
}

impl SkipReason {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Trashed => "in the trash",
            Self::Archived => "archived",
            Self::Transient => "being created or deleted",
            Self::NoAgentPane => "agent is not running",
            Self::AwaitingPermission => "waiting on a prompt",
            Self::RestartInFlight => "already restarting",
            Self::Snoozed => "snoozed",
            Self::Starting => "starting",
            Self::Structured => "structured view (open it in aoe)",
        }
    }
}

/// The facts eligibility needs about one marked Session, captured so the rule is pure.
#[derive(Debug, Clone)]
pub(super) struct BulkCandidate {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub trashed: bool,
    pub archived: bool,
    pub snoozed: bool,
    pub structured: bool,
    pub pane_alive: bool,
    pub awaiting_approval: bool,
    pub restart_in_flight: bool,
}

impl BulkCandidate {
    /// Mid-work: a restart would interrupt it. An Unknown Session may be running.
    pub(super) fn is_busy(&self) -> bool {
        matches!(
            self.status,
            Status::Running | Status::Waiting | Status::Starting | Status::Unknown
        )
    }
}

pub(super) struct BulkPlan {
    pub eligible: Vec<BulkCandidate>,
    pub skipped: Vec<(BulkCandidate, SkipReason)>,
}

impl BulkPlan {
    /// One `title: reason` line per skipped Session.
    pub(super) fn skipped_lines(&self) -> String {
        self.skipped
            .iter()
            .map(|(candidate, reason)| format!("  {}: {}", candidate.title, reason.label()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(super) fn plan_bulk(
    action: BulkAction,
    candidates: Vec<BulkCandidate>,
    attention_sort: bool,
) -> BulkPlan {
    let mut plan = BulkPlan {
        eligible: Vec::new(),
        skipped: Vec::new(),
    };
    for candidate in candidates {
        match skip_reason(action, &candidate, attention_sort) {
            Some(reason) => plan.skipped.push((candidate, reason)),
            None => plan.eligible.push(candidate),
        }
    }
    plan
}

fn skip_reason(action: BulkAction, c: &BulkCandidate, attention_sort: bool) -> Option<SkipReason> {
    if c.trashed {
        return Some(SkipReason::Trashed);
    }
    if c.archived {
        return Some(SkipReason::Archived);
    }
    if matches!(c.status, Status::Creating | Status::Deleting) {
        return Some(SkipReason::Transient);
    }
    match action {
        // Text typed into a booting agent is lost or garbled.
        BulkAction::Send if c.restart_in_flight => Some(SkipReason::RestartInFlight),
        BulkAction::Send if c.status == Status::Starting => Some(SkipReason::Starting),
        BulkAction::Send if c.structured => c
            .awaiting_approval
            .then_some(SkipReason::AwaitingPermission),
        BulkAction::Send if !c.pane_alive => Some(SkipReason::NoAgentPane),
        // Text typed at a permission prompt could answer it.
        BulkAction::Send => (c.status == Status::Waiting).then_some(SkipReason::AwaitingPermission),
        // The CLI cannot attach a structured view; a waiting Session is what the user wants to see.
        BulkAction::Breakout if c.structured => Some(SkipReason::Structured),
        BulkAction::Breakout => (!c.pane_alive).then_some(SkipReason::NoAgentPane),
        BulkAction::Restart if c.restart_in_flight => Some(SkipReason::RestartInFlight),
        // The single restart skips a snoozed Session only under Attention sort.
        BulkAction::Restart => (c.snoozed && attention_sort).then_some(SkipReason::Snoozed),
    }
}

impl HomeView {
    /// Marked Sessions in sidebar order, then any the sidebar does not show, by title.
    pub(super) fn marked_in_view_order(&self) -> Vec<String> {
        let mut ordered: Vec<String> = self
            .flat_items
            .iter()
            .filter_map(|item| match item {
                Item::Session { id, .. } if self.marked.contains(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let mut hidden: Vec<&String> = self
            .marked
            .iter()
            .filter(|id| !ordered.contains(id))
            .collect();
        hidden.sort_by_key(|id| self.get_instance(id).map(|inst| inst.title.clone()));
        ordered.extend(hidden.into_iter().cloned());
        ordered
    }

    /// Split the Marked set into the Sessions `action` may touch and the rest.
    pub(super) fn bulk_plan(&self, action: BulkAction) -> BulkPlan {
        let candidates = self
            .marked_in_view_order()
            .into_iter()
            .filter_map(|id| {
                let inst = self.get_instance(&id)?;
                Some(BulkCandidate {
                    title: inst.title.clone(),
                    status: inst.status,
                    trashed: inst.is_trashed(),
                    archived: inst.is_archived(),
                    snoozed: inst.is_snoozed(),
                    structured: inst.is_structured(),
                    pane_alive: inst.agent_pane.state == crate::session::PanePresence::Alive,
                    awaiting_approval: self
                        .structured_pending_approvals
                        .get(&id)
                        .is_some_and(|pending| !pending.is_empty()),
                    restart_in_flight: self.restart_in_flight.contains(&id),
                    id,
                })
            })
            .collect();
        plan_bulk(
            action,
            candidates,
            self.sort_order == crate::session::config::SortOrder::Attention,
        )
    }

    /// Open the Bulk action menu, or say why not.
    pub(super) fn open_bulk_menu(&mut self, anchor: (u16, u16)) {
        if self.marked.is_empty() {
            self.flash_status("No sessions marked. Space marks the cursor row");
            return;
        }
        self.context_menu = Some(ContextMenuDialog::for_marks(
            anchor,
            !self.breakout_titles.is_empty(),
        ));
    }

    /// List what each Bulk action would skip.
    pub(super) fn show_bulk_skips(&mut self) {
        let mut sections = Vec::new();
        for (name, action) in [
            ("Send", BulkAction::Send),
            ("Restart", BulkAction::Restart),
            ("Break out", BulkAction::Breakout),
        ] {
            let plan = self.bulk_plan(action);
            let total = plan.eligible.len() + plan.skipped.len();
            let noun = if total == 1 { "session" } else { "sessions" };
            sections.push(if plan.skipped.is_empty() {
                format!("{name}: all {total} marked {noun}")
            } else {
                format!(
                    "{name}: {} of {total} marked {noun}. Skipped:\n{}",
                    plan.eligible.len(),
                    plan.skipped_lines()
                )
            });
        }
        self.info_dialog = Some(InfoDialog::sized_to_fit(
            "Marked Sessions",
            &sections.join("\n\n"),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(title: &str) -> BulkCandidate {
        BulkCandidate {
            id: title.to_string(),
            title: title.to_string(),
            status: Status::Idle,
            trashed: false,
            archived: false,
            snoozed: false,
            structured: false,
            pane_alive: true,
            awaiting_approval: false,
            restart_in_flight: false,
        }
    }

    fn reason_for(
        action: BulkAction,
        attention: bool,
        edit: impl FnOnce(&mut BulkCandidate),
    ) -> Option<SkipReason> {
        let mut c = candidate("s");
        edit(&mut c);
        let plan = plan_bulk(action, vec![c], attention);
        plan.skipped.first().map(|(_, reason)| *reason)
    }

    #[test]
    fn send_eligibility() {
        use BulkAction::Send;
        assert_eq!(reason_for(Send, false, |_| {}), None);
        assert_eq!(
            reason_for(Send, false, |c| c.trashed = true),
            Some(SkipReason::Trashed)
        );
        assert_eq!(
            reason_for(Send, false, |c| c.archived = true),
            Some(SkipReason::Archived)
        );
        assert_eq!(
            reason_for(Send, false, |c| c.status = Status::Creating),
            Some(SkipReason::Transient)
        );
        assert_eq!(
            reason_for(Send, false, |c| c.pane_alive = false),
            Some(SkipReason::NoAgentPane)
        );
        assert_eq!(
            reason_for(Send, false, |c| c.status = Status::Waiting),
            Some(SkipReason::AwaitingPermission)
        );
        // A structured Session has no agent pane; only a pending approval blocks it.
        assert_eq!(
            reason_for(Send, false, |c| {
                c.structured = true;
                c.pane_alive = false;
            }),
            None
        );
        assert_eq!(
            reason_for(Send, false, |c| {
                c.structured = true;
                c.awaiting_approval = true;
            }),
            Some(SkipReason::AwaitingPermission)
        );
        assert_eq!(
            reason_for(Send, false, |c| c.status = Status::Starting),
            Some(SkipReason::Starting)
        );
        assert_eq!(
            reason_for(Send, false, |c| c.restart_in_flight = true),
            Some(SkipReason::RestartInFlight)
        );
        // A running Session takes the message as queued input.
        assert_eq!(
            reason_for(Send, false, |c| c.status = Status::Running),
            None
        );
    }

    #[test]
    fn restart_eligibility() {
        use BulkAction::Restart;
        assert_eq!(reason_for(Restart, false, |_| {}), None);
        assert_eq!(
            reason_for(Restart, false, |c| c.status = Status::Stopped),
            None,
            "a stopped Session can be restarted"
        );
        assert_eq!(
            reason_for(Restart, false, |c| c.status = Status::Running),
            None,
            "a busy Session is eligible; the confirm offers to leave it running"
        );
        assert_eq!(
            reason_for(Restart, false, |c| c.trashed = true),
            Some(SkipReason::Trashed)
        );
        assert_eq!(
            reason_for(Restart, false, |c| c.status = Status::Creating),
            Some(SkipReason::Transient)
        );
        assert_eq!(
            reason_for(Restart, false, |c| c.status = Status::Deleting),
            Some(SkipReason::Transient)
        );
        assert_eq!(
            reason_for(Restart, false, |c| c.restart_in_flight = true),
            Some(SkipReason::RestartInFlight)
        );
        assert_eq!(
            reason_for(Restart, true, |c| c.snoozed = true),
            Some(SkipReason::Snoozed),
            "matches the single restart under Attention sort"
        );
        assert_eq!(reason_for(Restart, false, |c| c.snoozed = true), None);
        assert_eq!(
            reason_for(Restart, false, |c| c.archived = true),
            Some(SkipReason::Archived)
        );
    }

    #[test]
    fn breakout_eligibility() {
        use BulkAction::Breakout;
        assert_eq!(reason_for(Breakout, false, |_| {}), None);
        assert_eq!(
            reason_for(Breakout, false, |c| c.structured = true),
            Some(SkipReason::Structured),
            "the CLI cannot attach a structured view"
        );
        assert_eq!(
            reason_for(Breakout, false, |c| c.pane_alive = false),
            Some(SkipReason::NoAgentPane)
        );
        assert_eq!(
            reason_for(Breakout, false, |c| c.status = Status::Waiting),
            None,
            "a waiting Session is exactly what the user wants to look at"
        );
        assert_eq!(
            reason_for(Breakout, false, |c| c.archived = true),
            Some(SkipReason::Archived)
        );
    }

    #[test]
    fn plan_keeps_order_and_splits() {
        let mut shelved = candidate("b");
        shelved.archived = true;
        let plan = plan_bulk(
            BulkAction::Restart,
            vec![candidate("a"), shelved, candidate("c")],
            false,
        );
        let titles: Vec<&str> = plan.eligible.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["a", "c"]);
        assert_eq!(plan.skipped_lines(), "  b: archived");
    }

    #[test]
    fn busy_means_running_waiting_starting_or_unknown() {
        for (status, busy) in [
            (Status::Running, true),
            (Status::Waiting, true),
            (Status::Starting, true),
            (Status::Unknown, true),
            (Status::Idle, false),
            (Status::Stopped, false),
            (Status::Error, false),
        ] {
            let mut c = candidate("s");
            c.status = status;
            assert_eq!(c.is_busy(), busy, "{status:?}");
        }
    }
}
