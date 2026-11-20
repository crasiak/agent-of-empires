//! The Marked set: transient Marks on sidebar Sessions, the target of Bulk actions.

use super::*;
use crate::session::{Item, Status};

impl HomeView {
    /// Sidebar title text for the Marked set, or `None` when nothing is marked.
    pub(super) fn marks_title(&self) -> Option<String> {
        if self.marked.is_empty() {
            return None;
        }
        let visible = self
            .flat_items
            .iter()
            .filter(|item| matches!(item, Item::Session { id, .. } if self.marked.contains(id)))
            .count();
        let total = self.marked.len();
        let hidden = total - visible;
        Some(if hidden == 0 {
            format!("{total} marked")
        } else {
            format!("{total} marked ({hidden} hidden)")
        })
    }

    /// Whether a Mark may sit on this Session.
    pub(super) fn markable(&self, id: &str) -> bool {
        self.get_instance(id).is_some_and(|inst| {
            !inst.is_trashed() && !matches!(inst.status, Status::Creating | Status::Deleting)
        })
    }

    /// Toggle the Mark on the cursor row. On a Group header, mark every Session the
    /// Group shows, or unmark them when all are already marked.
    pub(super) fn toggle_mark_at_cursor(&mut self) {
        match self.flat_items.get(self.cursor) {
            Some(Item::Session { id, .. }) => {
                let id = id.clone();
                if !self.marked.remove(&id) && self.markable(&id) {
                    self.marked.insert(id);
                }
            }
            Some(Item::Group { .. }) => self.toggle_group_marks(),
            None => {}
        }
    }

    /// Collapsed children are absent from `flat_items`, so membership comes from the
    /// Group helper, narrowed to what the active filters show.
    fn toggle_group_marks(&mut self) {
        let shown: std::collections::HashSet<String> =
            self.shown_session_ids().into_iter().collect();
        let members: Vec<String> = self
            .active_sessions_in_selected_group()
            .into_iter()
            .filter(|id| shown.contains(id) && self.markable(id))
            .collect();
        if members.is_empty() {
            return;
        }
        if members.iter().all(|id| self.marked.contains(id)) {
            for id in &members {
                self.marked.remove(id);
            }
        } else {
            self.marked.extend(members);
        }
    }

    /// Drop every Mark. Returns whether there were any.
    pub(super) fn clear_marks(&mut self) -> bool {
        let had = !self.marked.is_empty();
        self.marked.clear();
        had
    }

    /// Mark the cursor row, step the cursor, and mark the row reached.
    pub(super) fn extend_marks(&mut self, delta: i32) {
        self.mark_cursor_session();
        self.move_cursor(delta);
        self.mark_cursor_session();
    }

    fn mark_cursor_session(&mut self) {
        if let Some(Item::Session { id, .. }) = self.flat_items.get(self.cursor) {
            let id = id.clone();
            if self.markable(&id) {
                self.marked.insert(id);
            }
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
