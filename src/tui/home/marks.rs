//! The Marked set: transient Marks on sidebar Sessions, the target of Bulk actions.

use super::*;
use crate::session::{Item, Status};

impl HomeView {
    /// Whether a Mark may sit on this Session.
    pub(super) fn markable(&self, id: &str) -> bool {
        self.get_instance(id).is_some_and(|inst| {
            !inst.is_trashed() && !matches!(inst.status, Status::Creating | Status::Deleting)
        })
    }

    /// Toggle the Mark on the cursor row's Session.
    pub(super) fn toggle_mark_at_cursor(&mut self) {
        let Some(Item::Session { id, .. }) = self.flat_items.get(self.cursor) else {
            return;
        };
        let id = id.clone();
        if !self.marked.remove(&id) && self.markable(&id) {
            self.marked.insert(id);
        }
    }

    /// Drop Marks whose Session is gone, trashed or mid-deletion.
    pub(super) fn prune_marks(&mut self) {
        if self.marked.is_empty() {
            return;
        }
        let keep: std::collections::HashSet<String> = self
            .marked
            .iter()
            .filter(|id| self.markable(id))
            .cloned()
            .collect();
        self.marked = keep;
    }
}
