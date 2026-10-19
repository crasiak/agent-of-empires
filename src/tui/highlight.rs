//! Highlight predicates and the shared local/remote filter editor.
use super::{dialogs::DialogResult, styles::Theme};
use crate::session::repo_appearance::RepoColor;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{prelude::*, widgets::*};

use crate::session::SESSION_COLORS;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HighlightFilters {
    pub sessions: Vec<Option<String>>,
    pub projects: Vec<Option<RepoColor>>,
}
impl HighlightFilters {
    pub fn active(&self) -> bool {
        !self.sessions.is_empty() || !self.projects.is_empty()
    }
    pub fn matches_project(&self, color: Option<RepoColor>) -> bool {
        self.projects.is_empty() || self.projects.contains(&color)
    }
    pub fn matches(&self, session: Option<&str>, project: Option<RepoColor>) -> bool {
        (self.sessions.is_empty() || self.sessions.iter().any(|c| c.as_deref() == session))
            && self.matches_project(project)
    }
    pub fn summary(&self) -> String {
        let sessions = if self.sessions.is_empty() {
            "All".to_string()
        } else {
            self.sessions
                .iter()
                .map(|c| c.as_deref().unwrap_or("None"))
                .collect::<Vec<_>>()
                .join("+")
        };
        let projects = if self.projects.is_empty() {
            "All".to_string()
        } else {
            self.projects
                .iter()
                .map(|c| c.map(RepoColor::label).unwrap_or("None"))
                .collect::<Vec<_>>()
                .join("+")
        };
        format!("Highlights: sessions {sessions}; projects {projects} · F4 edit · Esc clear")
    }
}

pub struct HighlightFilterDialog {
    draft: HighlightFilters,
    cursor: usize,
}
impl HighlightFilterDialog {
    pub fn new(filters: &HighlightFilters) -> Self {
        Self {
            draft: filters.clone(),
            cursor: 0,
        }
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<HighlightFilters> {
        match key.code {
            KeyCode::Esc => return DialogResult::Cancel,
            KeyCode::Enter => return DialogResult::Submit(self.draft.clone()),
            KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1) % 16
            }
            KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = (self.cursor + 15) % 16
            }
            KeyCode::Char('c') => self.draft = HighlightFilters::default(),
            KeyCode::Char(' ') => match self.cursor {
                0 => self.draft.sessions.clear(),
                1..=6 => toggle(
                    &mut self.draft.sessions,
                    SESSION_COLORS.get(self.cursor - 1).map(|s| s.to_string()),
                ),
                7 => self.draft.projects.clear(),
                8..=14 => toggle(
                    &mut self.draft.projects,
                    RepoColor::ALL.get(self.cursor - 8).copied(),
                ),
                _ => self.draft = HighlightFilters::default(),
            },
            _ => {}
        }
        DialogResult::Continue
    }
    pub fn render(&self, frame: &mut Frame, theme: &Theme) {
        let area = frame.area();
        let width = area.width.min(62);
        let height = area.height.min(22);
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let mut lines = Vec::new();
        let mut row = |index: usize, label: String, checked: bool| {
            let style = if index == self.cursor {
                Style::default().fg(theme.text).bg(theme.selection)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::styled(
                format!(
                    " {} [{}] {label}",
                    if index == self.cursor { "›" } else { " " },
                    if checked { "x" } else { " " }
                ),
                style,
            ));
        };
        row(
            0,
            "All session highlights".into(),
            self.draft.sessions.is_empty(),
        );
        for (i, color) in SESSION_COLORS
            .iter()
            .map(|s| Some(s.to_string()))
            .chain([None])
            .enumerate()
        {
            row(
                i + 1,
                format!("Session: {}", color.as_deref().unwrap_or("None")),
                self.draft.sessions.contains(&color),
            );
        }
        row(
            7,
            "All project highlights".into(),
            self.draft.projects.is_empty(),
        );
        for (i, color) in RepoColor::ALL
            .into_iter()
            .map(Some)
            .chain([None])
            .enumerate()
        {
            row(
                i + 8,
                format!("Project: {}", color.map(RepoColor::label).unwrap_or("None")),
                self.draft.projects.contains(&color),
            );
        }
        row(15, "Clear both selectors (c)".into(), false);
        lines.push(Line::raw(" Space toggle · Enter apply · Esc cancel"));
        lines.push(Line::raw(" OR within a selector; AND between selectors"));
        let scroll = (self.cursor + 2).saturating_sub(height.saturating_sub(2) as usize) as u16;
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).scroll((scroll, 0)).block(
                Block::default()
                    .title(" Highlight filters ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .style(Style::default().bg(theme.background)),
            ),
            rect,
        );
    }
}
fn toggle<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if let Some(index) = values.iter().position(|v| *v == value) {
        values.remove(index);
    } else {
        values.push(value);
    }
}

pub fn project_color(color: RepoColor, theme: &Theme) -> Color {
    let (r, g, b) = match color {
        RepoColor::Amber => (0xf5, 0x9e, 0x0b),
        RepoColor::Teal => (0x14, 0xb8, 0xa6),
        RepoColor::Sky => (0x0e, 0xa5, 0xe9),
        RepoColor::Violet => (0x8b, 0x5c, 0xf6),
        RepoColor::Rose => (0xf4, 0x3f, 0x5e),
        RepoColor::Slate => (0x64, 0x74, 0x8b),
    };
    theme.fixed_hue(r, g, b)
}
pub fn session_color(color: &str, theme: &Theme) -> Option<Color> {
    match color {
        "red" => Some(theme.error),
        "amber" => Some(theme.waiting),
        "green" => Some(theme.running),
        "purple" => Some(theme.fixed_hue(0xa8, 0x55, 0xf7)),
        "teal" => Some(theme.fixed_hue(0x14, 0xb8, 0xa6)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_picker_row_all_none_clear_and_small_screen() {
        let mut dialog = HighlightFilterDialog::new(&HighlightFilters::default());
        for index in 1..=6 {
            dialog.cursor = index;
            dialog.handle_key(KeyCode::Char(' ').into());
        }
        assert_eq!(dialog.draft.sessions.len(), 6);
        assert!(dialog.draft.sessions.contains(&None));
        dialog.cursor = 0;
        dialog.handle_key(KeyCode::Char(' ').into());
        assert!(dialog.draft.sessions.is_empty());
        for index in 8..=14 {
            dialog.cursor = index;
            dialog.handle_key(KeyCode::Char(' ').into());
        }
        assert_eq!(dialog.draft.projects.len(), 7);
        assert!(dialog.draft.projects.contains(&None));
        dialog.cursor = 7;
        dialog.handle_key(KeyCode::Char(' ').into());
        assert!(dialog.draft.projects.is_empty());
        dialog.cursor = 6;
        dialog.handle_key(KeyCode::Char(' ').into());
        dialog.cursor = 14;
        dialog.handle_key(KeyCode::Char(' ').into());
        assert!(dialog.draft.matches(None, None));
        assert!(!dialog.draft.matches(Some("red"), None));
        assert!(!dialog.draft.matches(None, Some(RepoColor::Sky)));
        let theme = crate::tui::styles::load_theme_with_mode("empire", false);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(48, 8)).unwrap();
        terminal.draw(|f| dialog.render(f, &theme)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(
            text.contains("Project: None"),
            "focused item must scroll into view: {text}"
        );
        dialog.cursor = 15;
        dialog.handle_key(KeyCode::Char(' ').into());
        assert!(!dialog.draft.active());
        dialog.handle_key(KeyCode::Tab.into());
        assert_eq!(dialog.cursor, 0);
        dialog.handle_key(KeyCode::BackTab.into());
        assert_eq!(dialog.cursor, 15);
    }

    #[test]
    fn unions_intersections_none_and_cancel() {
        let f = HighlightFilters {
            sessions: vec![Some("red".into()), None],
            projects: vec![Some(RepoColor::Sky), Some(RepoColor::Rose)],
        };
        for (session, project, expected) in [
            (Some("red"), Some(RepoColor::Sky), true),
            (None, Some(RepoColor::Rose), true),
            (Some("green"), Some(RepoColor::Sky), false),
            (Some("red"), None, false),
        ] {
            assert_eq!(f.matches(session, project), expected);
        }
        assert!(HighlightFilters::default().matches(None, None));
        let mut dialog = HighlightFilterDialog::new(&f);
        dialog.handle_key(KeyCode::Char('c').into());
        assert!(matches!(
            dialog.handle_key(KeyCode::Esc.into()),
            DialogResult::Cancel
        ));
        assert!(f.active());
        assert!(
            matches!(dialog.handle_key(KeyCode::Enter.into()), DialogResult::Submit(filters) if !filters.active())
        );
    }
}
