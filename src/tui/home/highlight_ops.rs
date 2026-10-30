//! Highlight operations beyond a single row: the `L` menu, clearing colors from the
//! shown sessions, coloring every `/` search match, and narrowing the F4 filter.

use super::*;
use crate::session::SESSION_COLORS;
use crate::tui::dialogs::HighlightOps;

/// The `SESSION_COLORS` entry for a stored color, so menu actions can carry it as
/// `&'static str`.
fn static_color(color: Option<&str>) -> Option<&'static str> {
    color.and_then(|c| SESSION_COLORS.iter().copied().find(|known| *known == c))
}

fn sessions_noun(n: usize) -> &'static str {
    if n == 1 {
        "session"
    } else {
        "sessions"
    }
}

impl HomeView {
    /// Sessions the current view shows: in the active profile, passing the F4 highlight
    /// filter and the `y` hide-stopped filter, outside Archived and Trash. Sessions folded
    /// inside a collapsed group count, since only the fold hides them.
    pub(super) fn shown_session_ids(&self) -> Vec<String> {
        self.instances
            .values()
            .filter(|inst| {
                !inst.is_archived()
                    && !inst.is_trashed()
                    && self.instance_matches_highlight_view(inst)
                    && !self.hidden_by_filter(inst)
            })
            .map(|inst| inst.id.clone())
            .collect()
    }

    /// Sessions under a committed `/` search match, Trash excluded.
    fn search_match_session_ids(&self) -> Vec<String> {
        self.search_matches
            .iter()
            .filter_map(|&index| match self.flat_items.get(index) {
                Some(Item::Session { id, .. }) => Some(id.clone()),
                _ => None,
            })
            .filter(|id| self.get_instance(id).is_some_and(|inst| !inst.is_trashed()))
            .collect()
    }

    /// What the Highlight menu offers right now: the cursor's session, the colors on
    /// shown sessions, and whether there are search matches to color.
    pub(super) fn highlight_ops(&self) -> HighlightOps {
        let mut shown = [false; 5];
        for id in self.shown_session_ids() {
            let color = self.get_instance(&id).and_then(|i| i.color.as_deref());
            if let Some(index) = SESSION_COLORS.iter().position(|c| Some(*c) == color) {
                shown[index] = true;
            }
        }
        let session = match self.flat_items.get(self.cursor) {
            Some(Item::Session { id, .. }) => self
                .get_instance(id)
                .filter(|inst| !inst.is_trashed())
                .map(|inst| static_color(inst.color.as_deref())),
            _ => None,
        };
        HighlightOps {
            session,
            shown,
            search_matches: !self.search_match_session_ids().is_empty(),
        }
    }

    /// `L`: open the Highlight menu beside the cursor row.
    pub(super) fn open_highlight_menu(&mut self) {
        if !self.show_session_colors {
            self.flash_status("Session highlights are off (session.show_session_colors)");
            return;
        }
        let ops = self.highlight_ops();
        match ContextMenuDialog::for_highlights(self.cursor_row_anchor(), ops) {
            Some(menu) => self.context_menu = Some(menu),
            None => self.flash_status("Nothing to highlight: select a session or search with /"),
        }
    }

    /// Where a keyboard-opened menu sits: just below the cursor's row, found by
    /// hit-testing the rendered rows so it follows the list and shelf scroll.
    fn cursor_row_anchor(&self) -> (u16, u16) {
        for area in [self.list_inner_area, self.shelf_inner_area] {
            for y in area.y..area.bottom() {
                if self.resolve_row_to_index(area.x, y) == Some(self.cursor) {
                    return (area.x.saturating_add(2), y.saturating_add(1));
                }
            }
        }
        (2, 2)
    }

    /// Confirm removing `color` (every color with `None`) from the shown sessions.
    pub(super) fn prompt_clear_shown_highlight(&mut self, color: Option<&'static str>) {
        let ids: Vec<String> = self
            .shown_session_ids()
            .into_iter()
            .filter(|id| {
                self.get_instance(id)
                    .and_then(|inst| inst.color.as_deref())
                    .is_some_and(|c| color.is_none_or(|want| want == c))
            })
            .collect();
        let what = color.map_or_else(
            || "all highlights".to_string(),
            |c| format!("the {c} highlight"),
        );
        if ids.is_empty() {
            self.flash_status(format!("No shown session has {what}"));
            return;
        }
        let n = ids.len();
        let message = format!("Remove {what} from {n} shown {}?", sessions_noun(n));
        self.prompt_highlight_bulk("Clear highlights", &message, ids, None);
    }

    /// Confirm setting `color` (clearing with `None`) on every search match.
    pub(super) fn prompt_color_search_matches(&mut self, color: Option<&'static str>) {
        let ids = self.search_match_session_ids();
        if ids.is_empty() {
            self.flash_status("No search matches to color: search with / first");
            return;
        }
        let n = ids.len();
        let noun = sessions_noun(n);
        let message = match color {
            Some(c) => format!("Highlight {n} matching {noun} {c}?"),
            None => format!("Remove the highlight from {n} matching {noun}?"),
        };
        self.prompt_highlight_bulk("Color search matches", &message, ids, color);
    }

    fn prompt_highlight_bulk(
        &mut self,
        title: &str,
        message: &str,
        ids: Vec<String>,
        color: Option<&'static str>,
    ) {
        if self.confirm_dialog.is_some() {
            return;
        }
        self.pending_highlight_bulk = Some((ids, color));
        self.confirm_dialog = Some(ConfirmDialog::new(title, message, "highlight_bulk").neutral());
    }

    /// Apply the confirmed bulk change in one storage update per profile.
    pub(super) fn apply_highlight_bulk(&mut self) {
        let Some((ids, color)) = self.pending_highlight_bulk.take() else {
            return;
        };
        if let Err(e) = self.bulk_apply_user_action(&ids, |inst| {
            if let Err(err) = inst.set_color(color.map(str::to_string)) {
                tracing::error!("set_color (bulk highlight) failed: {}", err);
            }
        }) {
            tracing::error!("bulk highlight failed: {}", e);
        }
        self.rebuild_flat_items_keeping_cursor();
        let n = ids.len();
        let noun = sessions_noun(n);
        self.flash_status(match color {
            Some(c) => format!("Highlighted {n} {noun} {c}"),
            None => format!("Cleared the highlight from {n} {noun}"),
        });
    }

    /// Narrow the F4 filter to one session color, keeping the project selector.
    pub(super) fn show_only_session_color(&mut self, color: &'static str) {
        self.highlight_filters.sessions = vec![Some(color.to_string())];
        self.rebuild_flat_items_keeping_cursor();
    }
}
