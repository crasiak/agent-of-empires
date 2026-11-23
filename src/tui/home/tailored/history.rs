use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{prelude::*, widgets::*};
use tui_input::backend::crossterm::EventHandler;

use crate::session::tailored::{history::HistoryEntry, quick::QuickPreset, CaptureAccount};
use crate::tui::styles::Theme;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HistoryAction {
    None,
    New,
    Intake,
    Research,
    Workshop,
    Document,
    Quick(QuickPreset),
    Open(String),
    Visibility { id: String, hidden: bool },
}

#[derive(Clone)]
enum Item {
    New,
    Intake,
    Research,
    Workshop,
    Document,
    Quick(QuickPreset),
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
        if !self.hidden && "intake task brief".contains(&query) {
            items.push(Item::Intake);
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
        for preset in [QuickPreset::Work, QuickPreset::Personal] {
            if !self.hidden && preset.title().to_lowercase().contains(&query) {
                items.push(Item::Quick(preset));
            }
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
                    Some(Item::Intake) => HistoryAction::Intake,
                    Some(Item::Research) => HistoryAction::Research,
                    Some(Item::Workshop) => HistoryAction::Workshop,
                    Some(Item::Document) => HistoryAction::Document,
                    Some(Item::Quick(preset)) => HistoryAction::Quick(*preset),
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
                .block(Block::bordered().title(" Find a preset or previous Session ")),
            rows[0],
        );
        self.toggle = rows[1];
        frame.render_widget(
            Paragraph::new(if self.hidden {
                "Hidden Sessions · F3 show recent · Restore does not start a Session"
            } else {
                "Recent Sessions · F3 show hidden · Hide never stops or deletes a Session"
            })
            .style(Style::default().fg(theme.dimmed)),
            rows[1],
        );
        let items = self.items();
        self.selected = self.selected.min(items.len().saturating_sub(1));
        let visible = (rows[2].height as usize / 2).max(1);
        let start = self.selected.saturating_sub(visible.saturating_sub(1));
        if items.is_empty() {
            frame.render_widget(Paragraph::new("No matching Sessions"), rows[2]);
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
                Item::Intake => (
                    "+ Intake".into(),
                    "Prepare a task brief, review setup, create one Session · Ctrl+1".into(),
                    HistoryAction::Intake,
                ),
                Item::Quick(preset) => (
                    format!("+ {}", preset.title()),
                    "Blank Pi conversation at ~/knowledge · model defaults from Ledger".into(),
                    HistoryAction::Quick(*preset),
                ),
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
    fn picker_groups_choices_and_only_expands_the_selected_item() {
        for (width, height) in [(110, 30), (80, 28)] {
            let mut picker = HistoryPicker::default();
            picker.replace_rows(vec![HistoryEntry {
                id: "saved".into(),
                profile: "personal-profile".into(),
                title: "Saved capture".into(),
                account: CaptureAccount::Personal,
                input_preview: "Database idea".into(),
                status: "idle".into(),
                created_at: chrono::DateTime::UNIX_EPOCH,
                hidden: false,
                research: false,
                workshop: false,
            }]);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            let draw =
                |picker: &mut HistoryPicker,
                 terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>| {
                    terminal
                        .draw(|frame| picker.render(frame, frame.area(), &Theme::default(), None))
                        .unwrap();
                    terminal
                        .backend()
                        .buffer()
                        .content()
                        .chunks(width as usize)
                        .map(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>())
                        .collect::<Vec<_>>()
                };
            let lines = draw(&mut picker, &mut terminal);
            let row = |label: &str| lines.iter().position(|line| line.contains(label)).unwrap();
            assert!(row("Workflows") < row("Quick Pi sessions"));
            assert!(row("Quick Pi sessions") < row("Previous Sessions"));
            assert!(!lines.join("\n").contains("Tool-free refinement"));
            assert_eq!(
                picker.handle_click(0, row("Workflows") as u16),
                HistoryAction::None
            );
            picker.handle_key(key(KeyCode::Down));
            assert_eq!(
                picker.handle_key(key(KeyCode::Enter)),
                HistoryAction::Intake
            );
            for _ in 0..4 {
                picker.handle_key(key(KeyCode::Down));
            }
            let quick = draw(&mut picker, &mut terminal).join("\n");
            assert!(quick.contains("Account: Work"));
            assert!(quick.contains("Agent: Pi"));
            assert!(quick.contains("Model defaults"));
            assert_eq!(
                picker.handle_key(key(KeyCode::Enter)),
                HistoryAction::Quick(QuickPreset::Work)
            );
            picker.handle_paste("Database idea");
            let saved = draw(&mut picker, &mut terminal).join("\n");
            assert!(saved.contains("Profile: personal-profile"));
            assert!(!saved.contains("Model defaults"));
            let hit = picker
                .hits
                .iter()
                .find(|(_, action)| *action == HistoryAction::Open("saved".into()))
                .unwrap()
                .0;
            assert_eq!(
                picker.handle_click(hit.x, hit.y),
                HistoryAction::Open("saved".into())
            );
        }
    }

    #[test]
    fn quick_tiles_filter_and_activate_by_keyboard_and_mouse_but_not_hidden_history() {
        for preset in [QuickPreset::Work, QuickPreset::Personal] {
            let mut picker = HistoryPicker::default();
            picker.handle_paste(preset.title());
            assert_eq!(picker.items().len(), 1);
            assert_eq!(
                picker.handle_key(key(KeyCode::Enter)),
                HistoryAction::Quick(preset)
            );
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
            terminal
                .draw(|frame| picker.render(frame, frame.area(), &Theme::default(), None))
                .unwrap();
            let hit = picker
                .hits
                .iter()
                .find(|(_, action)| *action == HistoryAction::Quick(preset))
                .unwrap()
                .0;
            assert_eq!(
                picker.handle_click(hit.x, hit.y),
                HistoryAction::Quick(preset)
            );
            picker.handle_key(key(KeyCode::F(3)));
            assert!(picker.items().is_empty());
            assert_eq!(picker.handle_key(key(KeyCode::Enter)), HistoryAction::None);
        }
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
        picker.handle_paste("Database idea");
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
