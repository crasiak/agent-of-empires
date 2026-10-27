use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{prelude::*, widgets::*};
use tui_input::backend::crossterm::EventHandler;

use crate::session::tailored::{history::HistoryEntry, CaptureAccount};
use crate::tui::styles::Theme;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HistoryAction {
    None,
    New,
    Research,
    Workshop,
    Document,
    Open(String),
    Visibility { id: String, hidden: bool },
}

#[derive(Clone)]
enum Item {
    New,
    Research,
    Workshop,
    Document,
    Session(HistoryEntry),
}

#[derive(Default)]
pub(super) struct HistoryPicker {
    query: tui_input::Input,
    pub rows: Vec<HistoryEntry>,
    selected: usize,
    hidden: bool,
    hits: Vec<(Rect, HistoryAction)>,
    toggle: Rect,
}

impl HistoryPicker {
    fn items(&self) -> Vec<Item> {
        let query = self.query.value().trim().to_lowercase();
        let mut items = Vec::new();
        if !self.hidden && "capture an idea".contains(&query) {
            items.push(Item::New);
        }
        if !self.hidden && "tailored research".contains(&query) {
            items.push(Item::Research);
        }
        if !self.hidden && "document chat".contains(&query) {
            items.push(Item::Document);
        }
        if !self.hidden && "skill workshop".contains(&query) {
            items.push(Item::Workshop);
        }
        items.extend(
            self.rows
                .iter()
                .filter(|entry| {
                    entry.hidden == self.hidden
                        && format!(
                            "{} {} {} {:?} {}",
                            entry.title,
                            entry.input_preview,
                            entry.profile,
                            entry.account,
                            entry.status
                        )
                        .to_lowercase()
                        .contains(&query)
                })
                .cloned()
                .map(Item::Session),
        );
        items
    }

    pub fn replace_rows(&mut self, rows: Vec<HistoryEntry>) -> bool {
        if rows == self.rows {
            return false;
        }
        let old = self.items().get(self.selected).cloned();
        self.rows = rows;
        let items = self.items();
        self.selected = match old {
            Some(Item::Session(old)) => items.iter().position(|item| matches!(item, Item::Session(entry) if entry.id == old.id && entry.profile == old.profile))
                .unwrap_or(self.selected.min(items.len().saturating_sub(1))),
            _ => self.selected.min(items.len().saturating_sub(1)),
        };
        true
    }

    pub fn handle_paste(&mut self, text: &str) {
        self.query = tui_input::Input::new(format!("{}{text}", self.query.value()));
        self.selected = 0;
    }

    pub fn recent(&mut self) {
        self.hidden = false;
        self.selected = 0;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> HistoryAction {
        let items = self.items();
        match key.code {
            KeyCode::Down => self.selected = (self.selected + 1).min(items.len().saturating_sub(1)),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::F(3) => {
                self.hidden = !self.hidden;
                self.selected = 0;
            }
            KeyCode::Enter => {
                return match items.get(self.selected) {
                    Some(Item::New) => HistoryAction::New,
                    Some(Item::Research) => HistoryAction::Research,
                    Some(Item::Workshop) => HistoryAction::Workshop,
                    Some(Item::Document) => HistoryAction::Document,
                    Some(Item::Session(entry)) => HistoryAction::Open(entry.id.clone()),
                    None => HistoryAction::None,
                }
            }
            KeyCode::Delete => {
                if let Some(Item::Session(entry)) = items.get(self.selected) {
                    return HistoryAction::Visibility {
                        id: entry.id.clone(),
                        hidden: !entry.hidden,
                    };
                }
            }
            _ => {
                self.query.handle_event(&crossterm::event::Event::Key(key));
                self.selected = 0;
            }
        }
        HistoryAction::None
    }

    pub fn handle_click(&mut self, col: u16, row: u16) -> HistoryAction {
        let point = Position::new(col, row);
        if self.toggle.contains(point) {
            self.hidden = !self.hidden;
            self.selected = 0;
            return HistoryAction::None;
        }
        self.hits
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, action)| action.clone())
            .unwrap_or(HistoryAction::None)
    }

    pub fn render(&mut self, frame: &mut Frame, inner: Rect, theme: &Theme, error: Option<&str>) {
        self.hits.clear();
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(self.query.value())
                .block(Block::bordered().title(" Find a preset or previous capture ")),
            rows[0],
        );
        self.toggle = rows[1];
        frame.render_widget(
            Paragraph::new(if self.hidden {
                "Hidden captures · F3 show recent · Restore does not start a Session"
            } else {
                "Recent captures · F3 show hidden · Hide never stops or deletes a Session"
            })
            .style(Style::default().fg(theme.dimmed)),
            rows[1],
        );
        let items = self.items();
        self.selected = self.selected.min(items.len().saturating_sub(1));
        let visible = (rows[2].height as usize / 2).max(1);
        let start = self.selected.saturating_sub(visible.saturating_sub(1));
        if items.is_empty() {
            frame.render_widget(Paragraph::new("No matching captures"), rows[2]);
        }
        for (index, item) in items.iter().enumerate().skip(start).take(visible) {
            let y = rows[2].y + ((index - start) * 2) as u16;
            let rect = Rect::new(
                rows[2].x,
                y,
                rows[2].width,
                2.min(rows[2].bottom().saturating_sub(y)),
            );
            if rect.height == 0 {
                continue;
            }
            let style = if index == self.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(theme.text)
            };
            let (title, detail, action) = match item {
                Item::Document => (
                    "+ Document chat".into(),
                    "Public Markdown/PDF link, direct tool-free conversation".into(),
                    HistoryAction::Document,
                ),
                Item::Workshop => ("+ Skill workshop".into(), "Tool-free refinement, exact Review/local Commit, destination Publish, separate Land and targeted Deploy".into(), HistoryAction::Workshop),
                Item::Research => (
                    "+ Tailored research".into(),
                    "Develop prompt, choose ORX/EVO, review then explicit Run".into(),
                    HistoryAction::Research,
                ),
                Item::New => (
                    "+ Capture an idea".into(),
                    "New Session with its own minimal Ledger profile".into(),
                    HistoryAction::New,
                ),
                Item::Session(entry) => (
                    format!(
                        "{} · {} · {} · {}",
                        entry.title,
                        match entry.account {
                            CaptureAccount::Personal => "personal",
                            CaptureAccount::Work => "work",
                        },
                        entry.status,
                        entry
                            .created_at
                            .with_timezone(&chrono::Local)
                            .format("%m-%d %H:%M")
                    ),
                    format!("{} · {}", entry.profile, entry.input_preview),
                    HistoryAction::Open(entry.id.clone()),
                ),
            };
            let button_width = if matches!(item, Item::Session(_)) {
                8.min(rect.width)
            } else {
                0
            };
            let text = Rect::new(
                rect.x,
                rect.y,
                rect.width.saturating_sub(button_width),
                rect.height,
            );
            frame.render_widget(
                Paragraph::new(vec![Line::from(title), Line::from(detail)]).style(style),
                text,
            );
            if let Item::Session(entry) = item {
                let button = Rect::new(
                    rect.right().saturating_sub(button_width),
                    rect.y,
                    button_width,
                    1,
                );
                frame.render_widget(
                    Paragraph::new(if entry.hidden { "Restore" } else { "Hide" })
                        .style(Style::default().fg(theme.accent)),
                    button,
                );
                self.hits.push((
                    button,
                    HistoryAction::Visibility {
                        id: entry.id.clone(),
                        hidden: !entry.hidden,
                    },
                ));
            }
            self.hits.push((rect, action));
        }
        if let Some(error) = error {
            frame.render_widget(
                Paragraph::new(error)
                    .wrap(Wrap { trim: true })
                    .style(Style::default().fg(theme.error)),
                rows[3],
            );
        }
        frame.render_widget(
            Paragraph::new("↑↓ select · Enter open · Del hide/restore · F3 hidden · Ctrl+Y close")
                .style(Style::default().fg(theme.dimmed)),
            rows[4],
        );
        if rows[0].width > 2 && rows[0].height > 1 {
            frame.set_cursor_position((
                rows[0].x
                    + 1
                    + self
                        .query
                        .visual_cursor()
                        .min(rows[0].width.saturating_sub(2) as usize) as u16,
                rows[0].y + 1,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }
    #[test]
    fn history_reopens_and_hides_an_existing_identity_instead_of_creating() {
        let entry = HistoryEntry {
            id: "saved".into(),
            profile: "personal-profile".into(),
            title: "Capture an idea".into(),
            account: CaptureAccount::Personal,
            input_preview: "Database idea".into(),
            status: "idle".into(),
            created_at: chrono::Utc::now(),
            hidden: false,
            research: false,
            workshop: false,
        };
        let mut picker = HistoryPicker::default();
        picker.replace_rows(vec![entry.clone()]);
        picker.handle_key(key(KeyCode::Down));
        picker.handle_key(key(KeyCode::Down));
        picker.handle_key(key(KeyCode::Down));
        picker.handle_key(key(KeyCode::Down));
        assert_eq!(
            picker.handle_key(key(KeyCode::Enter)),
            HistoryAction::Open("saved".into())
        );
        assert_eq!(
            picker.handle_key(key(KeyCode::Delete)),
            HistoryAction::Visibility {
                id: "saved".into(),
                hidden: true
            }
        );
        let mut hidden = entry;
        hidden.hidden = true;
        picker.replace_rows(vec![hidden]);
        picker.handle_key(key(KeyCode::F(3)));
        assert_eq!(
            picker.handle_key(key(KeyCode::Delete)),
            HistoryAction::Visibility {
                id: "saved".into(),
                hidden: false
            }
        );
    }
}
