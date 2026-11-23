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

impl Item {
    fn group(&self) -> &'static str {
        match self {
            Self::Quick(_) => "Quick Pi sessions",
            Self::Session(entry) if entry.hidden => "Hidden Sessions",
            Self::Session(_) => "Previous Sessions",
            _ => "Workflows",
        }
    }

    fn presentation(&self) -> (String, String, String, HistoryAction) {
        match self {
            Self::New => (
                "Capture an idea".into(),
                "Idea".into(),
                "Account: Choose Personal or Work · Agent: Pi\nCapture an idea in its own purpose-scoped Session.\nReview proposed destinations before making changes.".into(),
                HistoryAction::New,
            ),
            Self::Intake => (
                "Intake".into(),
                "Task brief".into(),
                "Account: Current profile · Agent: Pi\nPrepare a task brief, review setup, then create one Session.\nCtrl+1 opens fresh input; this picker resumes your draft.".into(),
                HistoryAction::Intake,
            ),
            Self::Research => (
                "Tailored research".into(),
                "ORX / EVO".into(),
                "Account: Choose Personal or Work · Agent: Pi\nDevelop a research prompt, choose ORX/EVO, then review.\nOnly the explicit Run action starts research.".into(),
                HistoryAction::Research,
            ),
            Self::Document => (
                "Document chat".into(),
                "Markdown / PDF".into(),
                "Account: Choose Personal or Work · Agent: Pi\nDiscuss a public Markdown/PDF link without tools.\nReview the document and account before starting.".into(),
                HistoryAction::Document,
            ),
            Self::Workshop => (
                "Skill workshop".into(),
                "Skill".into(),
                "Account: Choose Personal or Work · Agent: Pi\nRefine a skill in a tool-free conversation.\nReview and local Commit are explicit actions.\nPublish, Land and Deploy need separate approval.".into(),
                HistoryAction::Workshop,
            ),
            Self::Quick(preset) => (
                preset.title().into(),
                "Pi".into(),
                format!(
                    "Account: {} · Agent: Pi\nLedger Profile: {}\nBlank conversation at ~/knowledge.\nModel defaults come from Ledger.",
                    account_label(preset.account()), preset.profile()
                ),
                HistoryAction::Quick(*preset),
            ),
            Self::Session(entry) => (
                entry.title.clone(),
                format!("{} · {}", account_label(entry.account), entry.status),
                format!(
                    "Account: {} · {}\nCreated: {}\n{}\nAoE Profile: {}\nEnter resumes this Session; Hide only changes this list.",
                    account_label(entry.account), entry.status,
                    entry.created_at.with_timezone(&chrono::Local).format("%m-%d %H:%M"),
                    entry.input_preview, entry.profile
                ),
                HistoryAction::Open(entry.id.clone()),
            ),
        }
    }
}

fn account_label(account: CaptureAccount) -> &'static str {
    match account {
        CaptureAccount::Personal => "Personal",
        CaptureAccount::Work => "Work",
    }
}

enum PickerRow {
    Heading(&'static str),
    Gap,
    Choice(usize),
}

#[derive(Default)]
pub(super) struct HistoryPicker {
    query: tui_input::Input,
    pub rows: Vec<HistoryEntry>,
    selected: usize,
    detail_scroll: u16,
    detail_max_scroll: u16,
    detail_page: u16,
    hidden: bool,
    hits: Vec<(Rect, HistoryAction)>,
    toggle: Rect,
}

impl HistoryPicker {
    fn reset_details(&mut self) {
        self.detail_scroll = 0;
        self.detail_max_scroll = 0;
    }

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
                .unwrap_or_else(|| {
                    self.reset_details();
                    self.selected.min(items.len().saturating_sub(1))
                }),
            _ => self.selected.min(items.len().saturating_sub(1)),
        };
        true
    }

    pub fn handle_paste(&mut self, text: &str) {
        self.query = tui_input::Input::new(format!("{}{text}", self.query.value()));
        self.selected = 0;
        self.reset_details();
    }

    pub fn recent(&mut self) {
        self.hidden = false;
        self.selected = 0;
        self.reset_details();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> HistoryAction {
        let items = self.items();
        let previous = self.selected;
        match key.code {
            KeyCode::PageDown => {
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_add(self.detail_page.max(1))
                    .min(self.detail_max_scroll);
                return HistoryAction::None;
            }
            KeyCode::PageUp => {
                self.detail_scroll = self.detail_scroll.saturating_sub(self.detail_page.max(1));
                return HistoryAction::None;
            }
            KeyCode::Down => self.selected = (self.selected + 1).min(items.len().saturating_sub(1)),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::F(3) => {
                self.hidden = !self.hidden;
                self.selected = 0;
                self.reset_details();
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
                self.reset_details();
            }
        }
        if self.selected != previous {
            self.reset_details();
        }
        HistoryAction::None
    }

    pub fn handle_click(&mut self, col: u16, row: u16) -> HistoryAction {
        let point = Position::new(col, row);
        if self.toggle.contains(point) {
            self.hidden = !self.hidden;
            self.selected = 0;
            self.reset_details();
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
            Constraint::Length(if error.is_some() { 2 } else { 0 }),
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
                "Showing hidden · F3 show recent"
            } else {
                "Showing recent · F3 show hidden"
            })
            .style(Style::default().fg(theme.dimmed)),
            rows[1],
        );
        let items = self.items();
        self.selected = self.selected.min(items.len().saturating_sub(1));
        let content = rows[2];
        let side_by_side = content.width >= 96 && content.height >= 8;
        let (list, details) = if side_by_side {
            let columns = Layout::horizontal([
                Constraint::Percentage(55),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(content);
            (columns[0], columns[2])
        } else {
            let detail_height = if content.height >= 12 {
                6
            } else if content.height >= 9 {
                4
            } else {
                0
            };
            let sections =
                Layout::vertical([Constraint::Min(0), Constraint::Length(detail_height)])
                    .split(content);
            (sections[0], sections[1])
        };
        let mut display = Vec::new();
        let mut group = None;
        for (index, item) in items.iter().enumerate() {
            if group != Some(item.group()) {
                if group.is_some() {
                    display.push(PickerRow::Gap);
                }
                display.push(PickerRow::Heading(item.group()));
                group = Some(item.group());
            }
            display.push(PickerRow::Choice(index));
        }
        let selected_row = display
            .iter()
            .position(|row| matches!(row, PickerRow::Choice(index) if *index == self.selected))
            .unwrap_or(0);
        let start = selected_row.saturating_sub(usize::from(list.height).saturating_sub(1));
        if items.is_empty() {
            frame.render_widget(Paragraph::new("No matching choices or Sessions"), list);
        }
        for (offset, row) in display
            .iter()
            .skip(start)
            .take(usize::from(list.height))
            .enumerate()
        {
            let rect = Rect::new(list.x, list.y + offset as u16, list.width, 1);
            let index = match row {
                PickerRow::Heading(label) => {
                    frame.render_widget(
                        Paragraph::new(*label).style(
                            Style::default()
                                .fg(theme.group)
                                .add_modifier(Modifier::BOLD),
                        ),
                        rect,
                    );
                    continue;
                }
                PickerRow::Gap => continue,
                PickerRow::Choice(index) => *index,
            };
            let item = &items[index];
            let (title, tag, _, action) = item.presentation();
            let selected = index == self.selected;
            let style = if selected {
                Style::default().fg(theme.accent).bg(theme.selection)
            } else {
                Style::default().fg(theme.text)
            };
            frame.render_widget(Block::new().style(style), rect);
            let button_width = if matches!(item, Item::Session(_)) {
                8.min(rect.width)
            } else {
                0
            };
            let text = Rect::new(rect.x, rect.y, rect.width.saturating_sub(button_width), 1);
            let tag_width = (Line::from(tag.as_str()).width() as u16).min(text.width / 2);
            let columns = Layout::horizontal([
                Constraint::Min(0),
                Constraint::Length(2),
                Constraint::Length(tag_width),
            ])
            .split(text);
            let prefix = if matches!(item, Item::Session(_)) {
                ""
            } else {
                "+ "
            };
            frame.render_widget(
                Paragraph::new(format!(
                    "{} {prefix}{title}",
                    if selected { "›" } else { " " }
                ))
                .style(style.add_modifier(Modifier::BOLD)),
                columns[0],
            );
            frame.render_widget(
                Paragraph::new(tag)
                    .alignment(Alignment::Right)
                    .style(style.fg(if selected { theme.accent } else { theme.dimmed })),
                columns[2],
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
                        .style(style.fg(theme.accent)),
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
        if let Some(item) = items
            .get(self.selected)
            .filter(|_| details.height > 1 && details.width > 2)
        {
            let (title, _, description, _) = item.presentation();
            let text = Text::from(description);
            let block = Block::default()
                .borders(if side_by_side {
                    Borders::ALL
                } else {
                    Borders::TOP
                })
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    format!(" {title} "),
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(theme.border));
            let body = block.inner(details);
            let line_count: usize = text
                .lines
                .iter()
                .map(|line| crate::tui::components::text::wrapped_line_count(line, body.width))
                .sum();
            self.detail_max_scroll =
                u16::try_from(line_count.saturating_sub(usize::from(body.height)))
                    .unwrap_or(u16::MAX);
            self.detail_page = body.height.max(1);
            self.detail_scroll = self.detail_scroll.min(self.detail_max_scroll);
            frame.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: true })
                    .scroll((self.detail_scroll, 0))
                    .block(block)
                    .style(Style::default().fg(theme.dimmed)),
                details,
            );
        } else {
            self.reset_details();
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
            Paragraph::new(if self.detail_max_scroll > 0 {
                "↑↓ choose · Enter open · PgUp/PgDn details · Del hide · F3 hidden"
            } else {
                "↑↓ select · Enter open · Del hide/restore · F3 hidden · Ctrl+Y close"
            })
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
            assert!(!lines
                .join("\n")
                .contains("Refine a skill in a tool-free conversation."));
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
        for (preset, (width, height)) in [QuickPreset::Work, QuickPreset::Personal]
            .into_iter()
            .flat_map(|preset| [(100, 30), (42, 12), (16, 6)].map(|size| (preset, size)))
        {
            let mut picker = HistoryPicker::default();
            picker.handle_paste(preset.title());
            assert_eq!(picker.items().len(), 1);
            assert_eq!(
                picker.handle_key(key(KeyCode::Enter)),
                HistoryAction::Quick(preset)
            );
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
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
