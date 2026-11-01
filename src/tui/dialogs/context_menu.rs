//! Small popup menu anchored at a screen position, used for right-click
//! context actions on the sidebar list (Rename / Delete).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::*;

use super::DialogResult;
use crate::tui::styles::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    ToggleWorkArc,
    ExpandWorkArc,
    ProjectColor(Option<crate::session::repo_appearance::RepoColor>),
    Rename,
    Delete,
    /// Archive or unarchive the session (the `'z'` hotkey).
    ToggleArchive,
    /// Snooze or wake the session (the `'h'` hotkey). Snoozing opens the
    /// duration picker; unsnoozing wakes the row immediately.
    ToggleSnooze,
    /// Mark the session read or unread (the `'u'` hotkey).
    ToggleUnread,
    /// Open the new-session dialog (`'n'`).
    NewSession,
    /// New session prefilled from the right-clicked row (`'N'`): a session row
    /// inherits its repo path, group, agent and view, and its sandbox when it has
    /// one; a project or group row a member's path.
    NewFromSelection,
    /// Fork the session into an independent one resuming its conversation.
    Fork,
    /// Flip the persisted view between structured (ACP) and tmux terminal.
    /// Confirmed first, since the swap destroys in-flight history.
    SwitchView,
    /// Open the sort-order picker (`'o'`).
    OpenSortPicker,
    /// Attach another repo to this session.
    AddProject,
    /// Open the Edit Session dialog on its group field. The dialog creates
    /// the group when the path is new. Session rows in manual grouping only.
    MoveToGroup,
    /// Clear the session's group so it drops back to ungrouped. Offered only
    /// when the row is currently in a group.
    RemoveFromGroup,
    /// Set the session's per-row highlight color, or clear it with `None`.
    /// Picking the color the row already has clears it, so each row toggles.
    SessionColor(Option<&'static str>),
    /// Remove one highlight color (or, with `None`, every highlight) from the
    /// sessions the current filters show. Confirmed first with a count.
    ClearShownHighlight(Option<&'static str>),
    /// Set (or, with `None`, clear) the highlight on every committed `/` search
    /// match. Confirmed first with a count.
    ColorSearchMatches(Option<&'static str>),
    /// Narrow the F4 highlight filter to sessions of this one color.
    ShowOnlyColor(&'static str),
    /// Drill into a submenu. Handled inside the dialog; never submitted.
    Open(Submenu),
    /// Return from a submenu to its parent. Handled inside the dialog.
    Back,
    /// Open the group-by mode picker (`'g'`).
    OpenGroupPicker,
    /// Pin or unpin the project header (project view only, `'p'`).
    TogglePin,
    /// Purge every trashed session (`aoe session empty-trash`). Confirmed
    /// first, being irreversible.
    EmptyTrash,
    /// Restore all from Trash, or unarchive all. Reversible, so no confirm.
    RestoreAll,
    /// Collapse or expand the section the menu was opened on.
    ToggleSectionCollapse,
    /// Restore a trashed session.
    Restore,
}

/// A menu that opens in place of its parent, with a "‹ Back" row on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submenu {
    /// Session highlight colors plus the bulk operations on shown sessions.
    Highlight,
    /// The project header's repo colors.
    ProjectHighlight,
    /// Colors to apply to every committed `/` search match.
    ColorMatches,
}

/// What the session Highlight menu offers. `session` is `Some` when a session row
/// is selected, carrying its current color; `shown` flags, in `SESSION_COLORS`
/// order, which colors appear on at least one shown session; `search_matches`
/// adds the color-the-matches entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighlightOps {
    pub session: Option<Option<&'static str>>,
    pub shown: [bool; 5],
    pub search_matches: bool,
}

type MenuItem = (ContextMenuAction, &'static str);

/// Per-color labels in `SESSION_COLORS` order: (color, this-session row,
/// clear-from-shown row, show-only row, search-match row).
const SESSION_COLOR_ROWS: [(&str, &str, &str, &str, &str); 5] = [
    (
        "red",
        "Highlight red",
        "Clear red from shown",
        "Show only red",
        "Red",
    ),
    (
        "amber",
        "Highlight amber",
        "Clear amber from shown",
        "Show only amber",
        "Amber",
    ),
    (
        "green",
        "Highlight green",
        "Clear green from shown",
        "Show only green",
        "Green",
    ),
    (
        "purple",
        "Highlight purple",
        "Clear purple from shown",
        "Show only purple",
        "Purple",
    ),
    (
        "teal",
        "Highlight teal",
        "Clear teal from shown",
        "Show only teal",
        "Teal",
    ),
];

/// The Highlight menu's rows and, when there are search matches, the color
/// submenu they open. Empty when there is nothing to offer.
fn highlight_menu(ops: HighlightOps) -> (Vec<MenuItem>, Vec<(Submenu, Vec<MenuItem>)>) {
    let mut items = Vec::new();
    if let Some(current) = ops.session {
        for (color, label, ..) in SESSION_COLOR_ROWS {
            items.push((ContextMenuAction::SessionColor(Some(color)), label));
        }
        if current.is_some() {
            items.push((ContextMenuAction::SessionColor(None), "Remove highlight"));
        }
    }
    for ((color, _, label, ..), shown) in SESSION_COLOR_ROWS.into_iter().zip(ops.shown) {
        if shown {
            items.push((ContextMenuAction::ClearShownHighlight(Some(color)), label));
        }
    }
    if ops.shown.contains(&true) {
        items.push((
            ContextMenuAction::ClearShownHighlight(None),
            "Clear all shown highlights",
        ));
    }
    let mut submenus = Vec::new();
    if ops.search_matches {
        items.push((
            ContextMenuAction::Open(Submenu::ColorMatches),
            "Color search matches ›",
        ));
        let mut colors: Vec<MenuItem> = SESSION_COLOR_ROWS
            .into_iter()
            .map(|(color, .., label)| (ContextMenuAction::ColorSearchMatches(Some(color)), label))
            .collect();
        colors.push((
            ContextMenuAction::ColorSearchMatches(None),
            "Remove highlight",
        ));
        submenus.push((Submenu::ColorMatches, colors));
    }
    if let Some(Some(current)) = ops.session {
        if let Some((color, _, _, label, _)) =
            SESSION_COLOR_ROWS.into_iter().find(|row| row.0 == current)
        {
            items.push((ContextMenuAction::ShowOnlyColor(color), label));
        }
    }
    (items, submenus)
}

fn project_highlight_items() -> Vec<MenuItem> {
    use crate::session::repo_appearance::RepoColor;
    vec![
        (
            ContextMenuAction::ProjectColor(Some(RepoColor::Amber)),
            "Highlight Amber",
        ),
        (
            ContextMenuAction::ProjectColor(Some(RepoColor::Teal)),
            "Highlight Teal",
        ),
        (
            ContextMenuAction::ProjectColor(Some(RepoColor::Sky)),
            "Highlight Sky",
        ),
        (
            ContextMenuAction::ProjectColor(Some(RepoColor::Violet)),
            "Highlight Violet",
        ),
        (
            ContextMenuAction::ProjectColor(Some(RepoColor::Rose)),
            "Highlight Rose",
        ),
        (
            ContextMenuAction::ProjectColor(Some(RepoColor::Slate)),
            "Highlight Slate",
        ),
        (
            ContextMenuAction::ProjectColor(None),
            "Clear project highlight",
        ),
    ]
}

/// The in-menu hotkey for a row that carries its own: the color's initial for
/// color rows, `x` for removing one, `l` for a Highlight submenu.
fn row_hotkey(action: ContextMenuAction) -> Option<char> {
    match action {
        ContextMenuAction::SessionColor(color) | ContextMenuAction::ColorSearchMatches(color) => {
            Some(color.and_then(|c| c.chars().next()).unwrap_or('x'))
        }
        ContextMenuAction::ClearShownHighlight(None) => Some('c'),
        ContextMenuAction::ShowOnlyColor(_) => Some('f'),
        ContextMenuAction::Open(Submenu::Highlight | Submenu::ProjectHighlight) => Some('l'),
        ContextMenuAction::Open(Submenu::ColorMatches) => Some('m'),
        _ => None,
    }
}

pub struct ContextMenuDialog {
    items: Vec<MenuItem>,
    /// Rows each `Open` item drills into.
    submenus: Vec<(Submenu, Vec<MenuItem>)>,
    /// The menus a submenu was opened from, with their highlight, so Back
    /// restores them.
    parents: Vec<(Vec<MenuItem>, Option<usize>)>,
    /// The highlighted item. Opens at `Some(0)` so Enter submits the first
    /// item, but hover clears it to `None` once the cursor leaves every row.
    highlight: Option<usize>,
    /// Where the popup's top-left wants to sit; the renderer clamps it into
    /// the visible area.
    anchor: (u16, u16),
    /// Last rendered rect, so click-outside needs no layout math.
    last_area: Rect,
    scroll_offset: usize,
}

/// The menu item at `(col, row)`, or `None` on any border, past the last item,
/// or outside `area`. Every border direction is excluded, or a click on the
/// right-hand border would dispatch the row it sits beside.
fn row_to_item_idx(area: Rect, items_len: usize, col: u16, row: u16) -> Option<usize> {
    if !area.contains(Position::from((col, row))) {
        return None;
    }
    let inner_x = area.x.saturating_add(1);
    let last_inner_x = area.right().saturating_sub(1);
    if col < inner_x || col >= last_inner_x {
        return None;
    }
    let inner_y = area.y.saturating_add(1);
    let last_item_y = inner_y.saturating_add(items_len as u16);
    if row < inner_y || row >= last_item_y || row >= area.bottom().saturating_sub(1) {
        return None;
    }
    Some((row - inner_y) as usize)
}

impl ContextMenuDialog {
    pub(crate) fn with_work_arc(mut self, enabled: Option<bool>) -> Self {
        if let Some(enabled) = enabled {
            self.items.push((
                ContextMenuAction::ToggleWorkArc,
                if enabled {
                    "Pause work arc"
                } else {
                    "Enable work arc…"
                },
            ));
            self.items
                .push((ContextMenuAction::ExpandWorkArc, "Expand work arc"));
        }
        self
    }

    /// Build the session row's menu. Each `Option` argument hides its row when
    /// `None` and carries the current state when `Some`, which flips the label:
    /// `snooze` (hidden outside Attention sort, matching the `'h'` keybinding),
    /// `unread` (hidden when the feature is off), `can_fork` (only for an agent
    /// that can actually fork), and `switch_view` (only for a row that can
    /// change views, labelled with the view it lands on).
    pub fn for_session(
        anchor: (u16, u16),
        is_archived: bool,
        snooze: Option<bool>,
        unread: Option<bool>,
        can_fork: bool,
        switch_view: Option<bool>,
    ) -> Self {
        Self::for_session_with_highlights(
            anchor,
            is_archived,
            snooze,
            unread,
            can_fork,
            switch_view,
            None,
        )
    }

    pub fn for_session_with_highlights(
        anchor: (u16, u16),
        is_archived: bool,
        snooze: Option<bool>,
        unread: Option<bool>,
        can_fork: bool,
        switch_view: Option<bool>,
        highlights: Option<HighlightOps>,
    ) -> Self {
        let archive_label = if is_archived { "Unarchive" } else { "Archive" };
        let mut items = vec![
            (ContextMenuAction::NewFromSelection, "New Session"),
            (ContextMenuAction::Rename, "Rename"),
            (ContextMenuAction::ToggleArchive, archive_label),
        ];
        if let Some(is_snoozed) = snooze {
            let snooze_label = if is_snoozed { "Unsnooze" } else { "Snooze" };
            items.push((ContextMenuAction::ToggleSnooze, snooze_label));
        }
        if let Some(is_unread) = unread {
            let unread_label = if is_unread {
                "Mark read"
            } else {
                "Mark unread"
            };
            items.push((ContextMenuAction::ToggleUnread, unread_label));
        }
        items.push((ContextMenuAction::AddProject, "Add project"));
        let (highlight_items, mut submenus) = highlights.map(highlight_menu).unwrap_or_default();
        if !highlight_items.is_empty() {
            items.push((ContextMenuAction::Open(Submenu::Highlight), "Highlight ›"));
            submenus.push((Submenu::Highlight, highlight_items));
        }
        items.push((ContextMenuAction::Delete, "Delete"));
        if can_fork {
            items.push((ContextMenuAction::Fork, "Fork session"));
        }
        if let Some(is_structured) = switch_view {
            let label = if is_structured {
                "Switch to terminal"
            } else {
                "Switch to structured"
            };
            items.push((ContextMenuAction::SwitchView, label));
        }
        let mut menu = Self::new(anchor, items);
        menu.submenus = submenus;
        menu
    }

    /// The Highlight menu on its own, opened by the `L` key. `None` when it
    /// would have no rows.
    pub fn for_highlights(anchor: (u16, u16), ops: HighlightOps) -> Option<Self> {
        let (items, submenus) = highlight_menu(ops);
        if items.is_empty() {
            return None;
        }
        let mut menu = Self::new(anchor, items);
        menu.submenus = submenus;
        Some(menu)
    }

    /// Add the group entries to a session menu, right after Rename: "Move to
    /// group" always, "Remove from group" only when the row is in a group.
    pub fn with_group_actions(mut self, in_group: bool) -> Self {
        let at = self
            .items
            .iter()
            .position(|(action, _)| *action == ContextMenuAction::Rename)
            .map_or(self.items.len(), |idx| idx + 1);
        self.items
            .insert(at, (ContextMenuAction::MoveToGroup, "Move to group"));
        if in_group {
            self.items.insert(
                at + 1,
                (ContextMenuAction::RemoveFromGroup, "Remove from group"),
            );
        }
        self
    }

    /// A trashed row can only come back out or be deleted for good.
    pub fn for_trashed_session(anchor: (u16, u16)) -> Self {
        Self::new(
            anchor,
            vec![
                (ContextMenuAction::Restore, "Restore"),
                (ContextMenuAction::Delete, "Delete"),
            ],
        )
    }

    pub fn for_group(anchor: (u16, u16)) -> Self {
        Self::new(
            anchor,
            vec![
                (ContextMenuAction::NewFromSelection, "New Session"),
                (ContextMenuAction::Rename, "Rename Group"),
                (ContextMenuAction::Delete, "Delete Group"),
            ],
        )
    }

    /// Menu for a project header. Project groups are automatic, so there is no
    /// Rename/Delete; the pin toggle lets the project persist with no sessions.
    pub fn for_project_group(anchor: (u16, u16), is_pinned: bool) -> Self {
        let pin_label = if is_pinned {
            "Unpin project"
        } else {
            "Pin project"
        };
        let mut menu = Self::new(
            anchor,
            vec![
                (ContextMenuAction::NewFromSelection, "New Session"),
                (ContextMenuAction::TogglePin, pin_label),
                (
                    ContextMenuAction::Open(Submenu::ProjectHighlight),
                    "Project highlight ›",
                ),
            ],
        );
        menu.submenus = vec![(Submenu::ProjectHighlight, project_highlight_items())];
        menu
    }

    pub fn for_synthetic_project(anchor: (u16, u16)) -> Self {
        let mut menu = Self::for_project_group(anchor, false);
        menu.items
            .retain(|(action, _)| *action != ContextMenuAction::TogglePin);
        menu
    }

    pub fn for_project_highlights(anchor: (u16, u16)) -> Self {
        Self::new(anchor, project_highlight_items())
    }

    /// Menu for the Trash section header: empty it, restore everything, or
    /// fold it away.
    pub fn for_trash_section(anchor: (u16, u16), collapsed: bool) -> Self {
        let collapse_label = if collapsed { "Expand" } else { "Collapse" };
        Self::new(
            anchor,
            vec![
                (ContextMenuAction::EmptyTrash, "Empty Trash"),
                (ContextMenuAction::RestoreAll, "Restore All"),
                (ContextMenuAction::ToggleSectionCollapse, collapse_label),
            ],
        )
    }

    /// Menu for the Archived section header. Archiving is reversible, so there
    /// is no destructive "empty" action.
    pub fn for_archived_section(anchor: (u16, u16), collapsed: bool) -> Self {
        let collapse_label = if collapsed { "Expand" } else { "Collapse" };
        Self::new(
            anchor,
            vec![
                (ContextMenuAction::RestoreAll, "Restore All"),
                (ContextMenuAction::ToggleSectionCollapse, collapse_label),
            ],
        )
    }

    /// Menu for a right-click on empty sidebar space, holding the `'n'` /
    /// `'o'` / `'g'` entry points.
    pub fn for_empty_sidebar(anchor: (u16, u16)) -> Self {
        Self::new(
            anchor,
            vec![
                (ContextMenuAction::NewSession, "New Session"),
                (ContextMenuAction::OpenSortPicker, "Change Sort"),
                (ContextMenuAction::OpenGroupPicker, "Change Grouping"),
            ],
        )
    }

    fn new(anchor: (u16, u16), items: Vec<MenuItem>) -> Self {
        Self {
            items,
            submenus: Vec::new(),
            parents: Vec::new(),
            highlight: Some(0),
            anchor,
            last_area: Rect::default(),
            scroll_offset: 0,
        }
    }

    /// Open a submenu or step back out of one, both in place; every other
    /// action is submitted for the caller to dispatch.
    fn activate(&mut self, action: ContextMenuAction) -> DialogResult<ContextMenuAction> {
        match action {
            ContextMenuAction::Open(submenu) => {
                if let Some((_, children)) = self.submenus.iter().find(|(s, _)| *s == submenu) {
                    let mut items = vec![(ContextMenuAction::Back, "‹ Back")];
                    items.extend(children.iter().copied());
                    let parent = std::mem::replace(&mut self.items, items);
                    self.parents.push((parent, self.highlight));
                    self.highlight = Some(1);
                    self.scroll_offset = 0;
                }
                DialogResult::Continue
            }
            ContextMenuAction::Back => {
                self.back();
                DialogResult::Continue
            }
            action => DialogResult::Submit(action),
        }
    }

    /// Return to the parent menu. False at the top level.
    fn back(&mut self) -> bool {
        let Some((items, highlight)) = self.parents.pop() else {
            return false;
        };
        self.items = items;
        self.highlight = highlight;
        self.scroll_offset = 0;
        true
    }

    fn has(&self, action: ContextMenuAction) -> bool {
        self.items.iter().any(|(item, _)| *item == action)
    }

    /// The action `Enter` would submit, falling back to the first item when
    /// hover cleared the highlight.
    pub fn selected_action(&self) -> ContextMenuAction {
        self.items[self.highlight.unwrap_or(0)].0
    }

    #[cfg(test)]
    pub fn highlight_for_test(&self) -> Option<usize> {
        self.highlight
    }

    /// (action, label) pairs, so a test can assert which menu opened without
    /// rendering.
    #[cfg(test)]
    pub fn items_for_test(&self) -> &[MenuItem] {
        &self.items
    }

    /// Whether `(col, row)` falls outside the last rendered area, so the mouse
    /// router can close the menu on any click that misses it.
    pub fn click_is_outside(&self, col: u16, row: u16) -> bool {
        !self.last_area.contains(Position::from((col, row)))
    }

    /// Route a left-click: `Submit` on an item row, `Continue` elsewhere on
    /// the menu (it stays open), `None` outside it so the caller can close it.
    /// The highlight moves with the click first, so a near-miss still tracks
    /// where the user pointed.
    pub fn handle_click(&mut self, col: u16, row: u16) -> Option<DialogResult<ContextMenuAction>> {
        if !self.last_area.contains(Position::from((col, row))) {
            return None;
        }
        match row_to_item_idx(
            self.last_area,
            self.items.len().saturating_sub(self.scroll_offset),
            col,
            row,
        )
        .map(|i| i + self.scroll_offset)
        {
            None => {
                // A border or a gap: keep the menu open to try again.
                Some(DialogResult::Continue)
            }
            Some(idx) => {
                self.highlight = Some(idx);
                Some(self.activate(self.items[idx].0))
            }
        }
    }

    /// Move the highlight to whichever item the mouse is hovering, so the
    /// visual cue tracks the cursor the same way a desktop menu does. When
    /// the cursor is not over any item (a border row/column, or off the
    /// menu entirely) the highlight clears to `None` so the last-hovered
    /// item doesn't stay lit once the pointer leaves it. Returns true when
    /// the highlight actually changed, so the caller can skip a redraw on
    /// every pixel-level mouse twitch.
    ///
    /// This intentionally diverges from the focus-style dialogs
    /// (`ConfirmDialog` and friends), which keep their selection when the
    /// mouse drifts off so a click still lands on a sensible default. A
    /// multi-row popup menu follows desktop semantics instead: drift off
    /// and nothing is armed, so an accidental near-miss can't leave a
    /// misleading row lit for `Enter`.
    pub fn handle_hover(&mut self, col: u16, row: u16) -> bool {
        let next = row_to_item_idx(
            self.last_area,
            self.items.len().saturating_sub(self.scroll_offset),
            col,
            row,
        )
        .map(|i| i + self.scroll_offset);
        if self.highlight == next {
            return false;
        }
        self.highlight = next;
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<ContextMenuAction> {
        match key.code {
            // Esc and Left step out of a submenu; at the top Esc closes the menu.
            KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => {
                if !self.back() && key.code == KeyCode::Esc {
                    DialogResult::Cancel
                } else {
                    DialogResult::Continue
                }
            }
            // Enter submits the highlighted item. If mouse hover cleared the
            // highlight (cursor left every item) there's nothing to submit,
            // so keep the menu open rather than firing a stale action.
            KeyCode::Enter => match self.highlight {
                Some(idx) => self.activate(self.items[idx].0),
                None => DialogResult::Continue,
            },
            // Arrow keys always re-establish a concrete highlight, so the
            // keyboard works even after hover cleared it to `None`.
            KeyCode::Up | KeyCode::Char('k') => {
                let last = self.items.len() - 1;
                self.highlight = Some(match self.highlight {
                    None | Some(0) => last,
                    Some(idx) => idx - 1,
                });
                DialogResult::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.highlight = Some(match self.highlight {
                    None => 0,
                    Some(idx) => (idx + 1) % self.items.len(),
                });
                DialogResult::Continue
            }
            // Quick-pick hotkeys mirror the underlying actions' home-view
            // bindings (r/d for Rename/Delete; n/o/g for New / Sort /
            // Grouping). The hotkey only fires when the corresponding
            // action is actually in the current menu's item list, so the
            // session menu's `r` doesn't accidentally fire on the
            // empty-sidebar menu (which has different items).
            // Rows with their own hotkey (the highlight menus) win, so `r` picks
            // red there while it still renames from the session menu.
            KeyCode::Char(c) if self.items.iter().any(|(a, _)| row_hotkey(*a) == Some(c)) => {
                let action = self
                    .items
                    .iter()
                    .find(|(a, _)| row_hotkey(*a) == Some(c))
                    .map(|(a, _)| *a)
                    .expect("checked by the guard");
                self.activate(action)
            }
            KeyCode::Char(c) => {
                let action = match c {
                    'r' | 'R' => Some(ContextMenuAction::Rename),
                    'd' | 'D' => Some(ContextMenuAction::Delete),
                    // `z` restores a trashed row, as it does outside the menu.
                    'z' | 'Z' => {
                        if self.has(ContextMenuAction::Restore) {
                            Some(ContextMenuAction::Restore)
                        } else {
                            Some(ContextMenuAction::ToggleArchive)
                        }
                    }
                    'h' | 'H' => Some(ContextMenuAction::ToggleSnooze),
                    'u' | 'U' => Some(ContextMenuAction::ToggleUnread),
                    // `n` opens a new session from whichever new-session entry
                    // the current menu carries: the session/group/project menu
                    // prefills from the row (NewFromSelection), the empty-sidebar
                    // menu opens a blank one (NewSession).
                    'n' | 'N' => {
                        if self.has(ContextMenuAction::NewFromSelection) {
                            Some(ContextMenuAction::NewFromSelection)
                        } else {
                            Some(ContextMenuAction::NewSession)
                        }
                    }
                    'o' | 'O' => Some(ContextMenuAction::OpenSortPicker),
                    'g' | 'G' => Some(ContextMenuAction::OpenGroupPicker),
                    'p' | 'P' => Some(ContextMenuAction::TogglePin),
                    'e' | 'E' => Some(ContextMenuAction::EmptyTrash),
                    _ => None,
                };
                match action {
                    Some(a) if self.has(a) => DialogResult::Submit(a),
                    _ => DialogResult::Continue,
                }
            }
            _ => DialogResult::Continue,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let label_width = self
            .items
            .iter()
            .map(|(_, label)| label.chars().count() as u16)
            .max()
            .unwrap_or(0);
        // Border columns (2) + horizontal Padding (2) + breathing
        // room for the selection chevron (2).
        let width = (label_width + 6).max(16);
        let height = self.items.len() as u16 + 2;

        let mut x = self.anchor.0;
        let mut y = self.anchor.1;
        if x + width > area.right() {
            x = area.right().saturating_sub(width);
        }
        if y + height > area.bottom() {
            y = area.bottom().saturating_sub(height);
        }
        x = x.max(area.x);
        y = y.max(area.y);
        let dialog_area = Rect {
            x,
            y,
            width: width.min(area.width),
            height: height.min(area.height),
        };
        self.last_area = dialog_area;

        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .padding(Padding::horizontal(1))
            .border_style(Style::default().fg(theme.accent));

        let inner = block.inner(dialog_area);
        let visible = inner.height as usize;
        if let Some(focused) = self.highlight {
            if focused < self.scroll_offset {
                self.scroll_offset = focused;
            } else if focused >= self.scroll_offset + visible {
                self.scroll_offset = (focused + 1).saturating_sub(visible);
            }
        }
        self.scroll_offset = self
            .scroll_offset
            .min(self.items.len().saturating_sub(visible));
        frame.render_widget(block, dialog_area);

        let rows: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(visible)
            .map(|(idx, (_, label))| {
                let style = if Some(idx) == self.highlight {
                    Style::default()
                        .fg(theme.background)
                        .bg(theme.accent)
                        .bold()
                } else {
                    Style::default().fg(theme.text)
                };
                Line::from(format!(" {label} ")).style(style)
            })
            .collect();

        frame.render_widget(Paragraph::new(rows), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::dialogs::test_keys::key;
    use ContextMenuAction as A;

    #[test]
    fn project_highlight_menu_scrolls_focus_and_never_hits_clipped_border() {
        let mut menu = ContextMenuDialog::for_project_group((1, 1), false);
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('l'))),
            DialogResult::Continue
        ));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(48, 8)).unwrap();
        let theme = crate::tui::styles::load_theme("empire");
        // The submenu opens on its first color; Up reaches Back, then wraps to the end.
        menu.handle_key(key(KeyCode::Up));
        menu.handle_key(key(KeyCode::Up));
        terminal.draw(|f| menu.render(f, f.area(), &theme)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            text.contains("Highlight Rose") && text.contains("Clear project highlight"),
            "{text}"
        );
        assert!(menu.scroll_offset > 0);
        let x = menu.last_area.x + 2;
        let border_y = menu.last_area.bottom() - 1;
        assert!(matches!(
            menu.handle_click(x, border_y),
            Some(DialogResult::Continue)
        ));
        assert!(menu.handle_hover(x, border_y));
        assert_eq!(menu.highlight_for_test(), None);
        assert!(matches!(
            menu.handle_click(x, border_y - 1),
            Some(DialogResult::Submit(A::ProjectColor(None)))
        ));
        assert_eq!(menu.selected_action(), A::ProjectColor(None));
        menu.handle_key(key(KeyCode::Down));
        terminal.draw(|f| menu.render(f, f.area(), &theme)).unwrap();
        assert_eq!(menu.scroll_offset, 0);
        assert!(matches!(
            menu.handle_click(x, menu.last_area.y + 1),
            Some(DialogResult::Continue)
        ));
        assert_eq!(labels(&menu)[0], "New Session", "the Back row returns");
    }

    fn highlight_session(ops: HighlightOps) -> ContextMenuDialog {
        ContextMenuDialog::for_session_with_highlights(
            (0, 0),
            false,
            None,
            None,
            false,
            None,
            Some(ops),
        )
    }

    #[test]
    fn highlight_submenu_drills_in_hotkeys_colors_and_steps_back() {
        let ops = HighlightOps {
            session: Some(Some("red")),
            shown: [true, false, false, false, true],
            search_matches: true,
        };
        let mut menu = highlight_session(ops);
        let top = labels(&menu);
        assert!(top.contains(&"Highlight ›"));
        assert!(!top.contains(&"Highlight red"));
        // `r` still renames from the top menu.
        assert!(matches!(
            highlight_session(ops).handle_key(key(KeyCode::Char('r'))),
            DialogResult::Submit(A::Rename)
        ));

        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('l'))),
            DialogResult::Continue
        ));
        assert_eq!(
            labels(&menu),
            vec![
                "‹ Back",
                "Highlight red",
                "Highlight amber",
                "Highlight green",
                "Highlight purple",
                "Highlight teal",
                "Remove highlight",
                "Clear red from shown",
                "Clear teal from shown",
                "Clear all shown highlights",
                "Color search matches ›",
                "Show only red",
            ]
        );
        assert_eq!(menu.highlight_for_test(), Some(1));
        // Inside the submenu `r` means red, `x` remove, `c` clear all shown.
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('r'))),
            DialogResult::Submit(A::SessionColor(Some("red")))
        ));
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('x'))),
            DialogResult::Submit(A::SessionColor(None))
        ));
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('c'))),
            DialogResult::Submit(A::ClearShownHighlight(None))
        ));

        // A nested submenu, then Left and Esc each step back one level.
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('m'))),
            DialogResult::Continue
        ));
        assert_eq!(labels(&menu)[1], "Red");
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('g'))),
            DialogResult::Submit(A::ColorSearchMatches(Some("green")))
        ));
        assert!(matches!(
            menu.handle_key(key(KeyCode::Left)),
            DialogResult::Continue
        ));
        assert_eq!(labels(&menu)[0], "‹ Back");
        assert_eq!(labels(&menu)[1], "Highlight red");
        assert!(matches!(
            menu.handle_key(key(KeyCode::Esc)),
            DialogResult::Continue
        ));
        assert_eq!(labels(&menu), top);
        assert!(matches!(
            menu.handle_key(key(KeyCode::Esc)),
            DialogResult::Cancel
        ));
    }

    #[test]
    fn highlight_menu_rows_follow_what_there_is_to_do() {
        let plain = HighlightOps {
            session: Some(None),
            ..Default::default()
        };
        let mut menu = highlight_session(plain);
        menu.handle_key(key(KeyCode::Char('l')));
        let rows = labels(&menu);
        assert!(!rows.contains(&"Remove highlight"));
        assert!(!rows.iter().any(|r| r.starts_with("Clear")));
        assert!(!rows.contains(&"Color search matches ›"));

        // No session row and nothing shown: no Highlight entry, and `L` has no menu.
        assert!(!labels(&highlight_session(HighlightOps::default())).contains(&"Highlight ›"));
        assert!(ContextMenuDialog::for_highlights((0, 0), HighlightOps::default()).is_none());
        let bulk_only = HighlightOps {
            shown: [false, true, false, false, false],
            ..Default::default()
        };
        assert_eq!(
            labels(&ContextMenuDialog::for_highlights((0, 0), bulk_only).unwrap()),
            vec!["Clear amber from shown", "Clear all shown highlights"]
        );
    }

    /// A session menu at `(0, 0)` with snooze shown, unread hidden, fork on.
    fn session() -> ContextMenuDialog {
        ContextMenuDialog::for_session((0, 0), false, Some(false), None, true, None)
    }

    /// A session menu whose `last_area` is stubbed as if render had run, so
    /// click and hover routing can be exercised without a `Frame`.
    fn rendered_session(height: u16) -> ContextMenuDialog {
        let mut menu =
            ContextMenuDialog::for_session((10, 10), false, Some(false), None, true, None);
        menu.last_area = Rect::new(10, 10, 14, height);
        menu
    }

    fn labels(menu: &ContextMenuDialog) -> Vec<&'static str> {
        menu.items_for_test().iter().map(|(_, l)| *l).collect()
    }

    #[test]
    fn each_menu_lists_the_rows_its_context_allows() {
        let session_rows = |snooze, unread, archived| {
            labels(&ContextMenuDialog::for_session(
                (0, 0),
                archived,
                snooze,
                unread,
                true,
                None,
            ))
        };
        let cases: &[(&str, Vec<&str>, Vec<&str>)] = &[
            (
                "active session",
                session_rows(Some(false), None, false),
                vec![
                    "New Session",
                    "Rename",
                    "Archive",
                    "Snooze",
                    "Add project",
                    "Delete",
                    "Fork session",
                ],
            ),
            (
                "archived flips the archive label",
                session_rows(Some(false), None, true),
                vec![
                    "New Session",
                    "Rename",
                    "Unarchive",
                    "Snooze",
                    "Add project",
                    "Delete",
                    "Fork session",
                ],
            ),
            (
                "snoozed flips the snooze label",
                session_rows(Some(true), None, false),
                vec![
                    "New Session",
                    "Rename",
                    "Archive",
                    "Unsnooze",
                    "Add project",
                    "Delete",
                    "Fork session",
                ],
            ),
            (
                "unread feature on, row read",
                session_rows(None, Some(false), false),
                vec![
                    "New Session",
                    "Rename",
                    "Archive",
                    "Mark unread",
                    "Add project",
                    "Delete",
                    "Fork session",
                ],
            ),
            (
                "unread feature on, row unread",
                session_rows(None, Some(true), false),
                vec![
                    "New Session",
                    "Rename",
                    "Archive",
                    "Mark read",
                    "Add project",
                    "Delete",
                    "Fork session",
                ],
            ),
            (
                "both optional rows hidden",
                session_rows(None, None, false),
                vec![
                    "New Session",
                    "Rename",
                    "Archive",
                    "Add project",
                    "Delete",
                    "Fork session",
                ],
            ),
            (
                "group",
                labels(&ContextMenuDialog::for_group((0, 0))),
                vec!["New Session", "Rename Group", "Delete Group"],
            ),
            (
                "unpinned project",
                labels(&ContextMenuDialog::for_project_group((0, 0), false)),
                vec!["New Session", "Pin project", "Project highlight ›"],
            ),
            (
                "pinned project",
                labels(&ContextMenuDialog::for_project_group((0, 0), true)),
                vec!["New Session", "Unpin project", "Project highlight ›"],
            ),
        ];
        for (name, got, want) in cases {
            assert_eq!(got, want, "{name}");
        }

        // The group menu's actions differ from its labels; the session menu's
        // order is what the arrow-key tests below count on.
        let actions: Vec<A> = ContextMenuDialog::for_group((0, 0))
            .items_for_test()
            .iter()
            .map(|(a, _)| *a)
            .collect();
        assert_eq!(actions, vec![A::NewFromSelection, A::Rename, A::Delete]);
        let actions: Vec<A> = session().items_for_test().iter().map(|(a, _)| *a).collect();
        assert_eq!(
            actions,
            vec![
                A::NewFromSelection,
                A::Rename,
                A::ToggleArchive,
                A::ToggleSnooze,
                A::AddProject,
                A::Delete,
                A::Fork,
            ]
        );
    }

    #[test]
    fn switch_view_row_appears_only_for_a_switchable_row() {
        let rows = |switch_view| {
            labels(&ContextMenuDialog::for_session(
                (0, 0),
                false,
                None,
                None,
                false,
                switch_view,
            ))
        };
        assert!(!rows(None).iter().any(|l| l.starts_with("Switch to")));
        assert!(rows(Some(false)).contains(&"Switch to structured"));
        assert!(rows(Some(true)).contains(&"Switch to terminal"));
    }

    #[test]
    fn arrows_walk_the_items_and_wrap() {
        // Down n times from the default lands on the nth item; 7 wraps back.
        for (downs, want) in [
            (0, A::NewFromSelection),
            (1, A::Rename),
            (3, A::ToggleSnooze),
            (5, A::Delete),
            (7, A::NewFromSelection),
        ] {
            let mut menu = session();
            for _ in 0..downs {
                assert!(matches!(
                    menu.handle_key(key(KeyCode::Down)),
                    DialogResult::Continue
                ));
            }
            assert!(
                matches!(menu.handle_key(key(KeyCode::Enter)), DialogResult::Submit(a) if a == want)
            );
        }

        // Up from the first item wraps to the last.
        let mut menu = session();
        menu.handle_key(key(KeyCode::Up));
        assert!(matches!(
            menu.handle_key(key(KeyCode::Enter)),
            DialogResult::Submit(A::Fork)
        ));

        assert!(matches!(
            session().handle_key(key(KeyCode::Esc)),
            DialogResult::Cancel
        ));
    }

    #[test]
    fn hotkeys_beat_the_cursor_and_go_inert_without_their_row() {
        // Each hotkey fires its row from a menu whose cursor sits elsewhere.
        for (ch, want) in [
            ('n', A::NewFromSelection),
            ('r', A::Rename),
            ('z', A::ToggleArchive),
            ('h', A::ToggleSnooze),
            ('d', A::Delete),
        ] {
            let mut menu = session();
            menu.handle_key(key(KeyCode::Up)); // park on the last item
            assert!(
                matches!(menu.handle_key(key(KeyCode::Char(ch))), DialogResult::Submit(a) if a == want)
            );
        }

        // `n` on the empty-sidebar menu must resolve to its blank NewSession
        // entry, never to the row-scoped NewFromSelection.
        let mut menu = ContextMenuDialog::for_empty_sidebar((0, 0));
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('n'))),
            DialogResult::Submit(A::NewSession)
        ));

        let mut menu = ContextMenuDialog::for_project_group((0, 0), false);
        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('p'))),
            DialogResult::Submit(A::TogglePin)
        ));

        // A hotkey whose row the menu does not carry is inert, as is an
        // unbound key. `u` needs the unread row, `h` the snooze row, and the
        // session menu has no pin row at all.
        let mut unread_on =
            ContextMenuDialog::for_session((0, 0), false, None, Some(false), true, None);
        assert!(matches!(
            unread_on.handle_key(key(KeyCode::Char('u'))),
            DialogResult::Submit(A::ToggleUnread)
        ));
        for ch in ['u', 'h'] {
            let mut bare = ContextMenuDialog::for_session((0, 0), false, None, None, true, None);
            assert!(matches!(
                bare.handle_key(key(KeyCode::Char(ch))),
                DialogResult::Continue
            ));
        }
        for ch in ['p', 'x'] {
            assert!(matches!(
                session().handle_key(key(KeyCode::Char(ch))),
                DialogResult::Continue
            ));
        }
    }

    #[test]
    fn clicks_route_to_the_row_under_the_pointer() {
        // Items start one row inside the border, so y+1 is the first item.
        for (row, want) in [
            (11, A::NewFromSelection),
            (12, A::Rename),
            (13, A::ToggleArchive),
            (14, A::ToggleSnooze),
        ] {
            let mut menu = rendered_session(7);
            assert!(
                matches!(menu.handle_click(12, row), Some(DialogResult::Submit(a)) if a == want)
            );
        }

        // Every border stays inside the menu without dispatching a row. The
        // vertical ones share a row with an item, so column alone decides.
        let mut menu = rendered_session(4);
        for (col, row) in [(12, 10), (10, 11), (23, 11)] {
            assert!(matches!(
                menu.handle_click(col, row),
                Some(DialogResult::Continue)
            ));
        }
        assert!(menu.handle_click(40, 40).is_none());

        // Before a render captures `last_area`, every point is outside, so a
        // stray click cannot be mistaken for one inside the menu.
        assert!(
            ContextMenuDialog::for_session((10, 10), false, Some(false), None, true, None)
                .click_is_outside(10, 10)
        );
    }

    #[test]
    fn hover_tracks_the_pointer_and_clears_off_every_item() {
        let mut menu = rendered_session(7);
        assert_eq!(menu.selected_action(), A::NewFromSelection);
        assert!(menu.handle_hover(12, 12));
        assert_eq!(menu.selected_action(), A::Rename);
        assert!(!menu.handle_hover(12, 12), "same row is no redraw");

        // Sliding onto the border, or off the menu, unlights the row rather
        // than leaving the last item lit.
        for (col, row) in [(12u16, 10u16), (40, 40)] {
            let mut menu = rendered_session(7);
            menu.handle_hover(12, 12);
            assert_eq!(menu.highlight_for_test(), Some(1));
            assert!(menu.handle_hover(col, row));
            assert_eq!(menu.highlight_for_test(), None);
        }

        // With nothing highlighted Enter has no item to submit, and the
        // arrows restart from either end.
        let mut menu = rendered_session(7);
        menu.handle_hover(40, 40);
        assert!(!menu.handle_hover(41, 41), "still off every item");
        assert!(matches!(
            menu.handle_key(key(KeyCode::Enter)),
            DialogResult::Continue
        ));
        for (code, want) in [(KeyCode::Down, A::NewFromSelection), (KeyCode::Up, A::Fork)] {
            let mut menu = rendered_session(7);
            menu.handle_hover(40, 40);
            menu.handle_key(key(code));
            assert_eq!(menu.selected_action(), want);
        }
    }
}
