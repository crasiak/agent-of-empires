use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::TextArea;

use super::{idea_textarea, WindowAction};
use crate::session::tailored::intake::IntakeBrief;
use crate::tui::styles::Theme;

pub(super) struct IntakeWindow {
    input: TextArea<'static>,
    brief: Option<IntakeBrief>,
    error: Option<String>,
    next: Rect,
    focus_button: bool,
    preview_scroll: u16,
    preview_max: u16,
}

impl Default for IntakeWindow {
    fn default() -> Self {
        Self {
            input: idea_textarea(),
            brief: None,
            error: None,
            next: Rect::default(),
            focus_button: false,
            preview_scroll: 0,
            preview_max: 0,
        }
    }
}

impl IntakeWindow {
    pub(super) fn key(&mut self, key: KeyEvent) -> WindowAction {
        if key.kind == KeyEventKind::Release {
            return WindowAction::Continue;
        }
        if key.code == KeyCode::Esc && key.kind == KeyEventKind::Press {
            if self.brief.take().is_none() {
                return WindowAction::Close;
            }
            self.focus_button = false;
            return WindowAction::Continue;
        }
        if self.brief.is_some() {
            let scroll = match key.code {
                KeyCode::Up => Some(self.preview_scroll.saturating_sub(1)),
                KeyCode::Down => Some(self.preview_scroll.saturating_add(1)),
                KeyCode::PageUp => Some(self.preview_scroll.saturating_sub(10)),
                KeyCode::PageDown => Some(self.preview_scroll.saturating_add(10)),
                KeyCode::Home => Some(0),
                KeyCode::End => Some(self.preview_max),
                _ => None,
            };
            if let Some(scroll) = scroll {
                self.preview_scroll = scroll.min(self.preview_max);
                return WindowAction::Continue;
            }
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) && key.kind == KeyEventKind::Press {
            self.focus_button = !self.focus_button;
            return WindowAction::Continue;
        }
        if key.code == KeyCode::Enter
            && (key.modifiers == KeyModifiers::CONTROL
                || (key.modifiers.is_empty() && (self.focus_button || self.brief.is_some())))
        {
            if key.kind == KeyEventKind::Press {
                return self.advance();
            }
            return WindowAction::Continue;
        }
        if self.brief.is_none() && !self.focus_button {
            if super::is_capture_newline(key) {
                self.input.insert_newline();
            } else if !matches!(key.code, KeyCode::F(_) | KeyCode::Tab | KeyCode::BackTab) {
                self.input.input(key);
            }
        }
        WindowAction::Continue
    }

    fn advance(&mut self) -> WindowAction {
        if let Some(brief) = &self.brief {
            return WindowAction::ConfigureIntake(brief.clone());
        }
        match IntakeBrief::new(self.input.lines().join("\n")) {
            Ok(brief) => {
                self.brief = Some(brief);
                self.error = None;
                self.focus_button = true;
                self.preview_scroll = 0;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        WindowAction::Continue
    }

    pub(super) fn paste(&mut self, text: &str) {
        if self.brief.is_none() && !self.focus_button {
            self.input.insert_str(text);
        }
    }

    pub(super) fn click(&mut self, col: u16, row: u16) -> WindowAction {
        if self.next.contains(Position::new(col, row)) {
            self.advance()
        } else {
            WindowAction::Continue
        }
    }

    pub(super) fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .split(area);
        frame.render_widget(Paragraph::new("Intake · one reviewed task Session\nNo model call or Session is created until configuration is confirmed.").wrap(Wrap { trim: false }), rows[0]);
        if let Some(brief) = &self.brief {
            let text = Text::from(
                brief.prompt("chosen in next form", "review suggested group in next form"),
            );
            let lines: usize = text
                .lines
                .iter()
                .map(|line| {
                    crate::tui::components::text::wrapped_line_count(
                        line,
                        rows[1].width.saturating_sub(2),
                    )
                })
                .sum();
            self.preview_max = lines
                .saturating_sub(rows[1].height.saturating_sub(2) as usize)
                .min(u16::MAX as usize) as u16;
            self.preview_scroll = self.preview_scroll.min(self.preview_max);
            frame.render_widget(
                Paragraph::new(text)
                    .scroll((self.preview_scroll, 0))
                    .wrap(Wrap { trim: true })
                    .block(Block::bordered().title(" Prepared brief / task / initial prompt ")),
                rows[1],
            );
        } else {
            self.input.set_block(
                Block::bordered().title(" Describe the task · Enter inserts a newline "),
            );
            frame.render_widget(&self.input, rows[1]);
        }
        frame.render_widget(Paragraph::new("Next: review profile, Pi command, repo/worktree, group and permissions. Suggestions are editable; ambiguous access is not inferred. Arc uses its normal consent dialog after creation.").wrap(Wrap { trim: false }).style(Style::default().fg(theme.dimmed)), rows[2]);
        self.next = rows[3];
        frame.render_widget(
            Paragraph::new(if self.brief.is_some() {
                "[ Configure Session… ]"
            } else {
                "[ Prepare brief ]"
            })
            .style(Style::default().fg(theme.accent).add_modifier(
                if self.focus_button {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                },
            )),
            rows[3],
        );
        frame.render_widget(Paragraph::new(self.error.as_deref().unwrap_or("Tab to button, Enter next · Ctrl+Enter next · ↑↓/PgUp/PgDn review · Esc edit/back · F2 picker · Ctrl+Q close")).wrap(Wrap { trim: false }).style(Style::default().fg(if self.error.is_some() { theme.error } else { theme.dimmed })), rows[4]);
    }
}

impl crate::tui::home::HomeView {
    pub(in crate::tui::home) fn configure_intake(&mut self, brief: IntakeBrief) {
        let result = (|| -> anyhow::Result<crate::tui::dialogs::NewSessionDialog> {
            anyhow::ensure!(
                self.creating_stub_id.is_none(),
                "Wait for the current Session creation to finish"
            );
            anyhow::ensure!(
                self.available_tools
                    .available_list()
                    .iter()
                    .any(|tool| tool == "pi"),
                "Intake requires an installed Pi agent; no alternate agent or account is selected"
            );
            let profile = self.config_profile();
            let profiles = crate::session::list_profiles_for_display()?;
            anyhow::ensure!(
                profiles.contains(&profile),
                "The selected AoE profile no longer exists"
            );
            let source = self
                .selected_session
                .as_ref()
                .and_then(|id| self.get_instance(id))
                .filter(|row| {
                    row.source_profile == profile
                        && !row.scratch
                        && !row.is_archived()
                        && !row.is_trashed()
                });
            let group = source
                .map(|row| row.group_path.clone())
                .filter(|group| !group.is_empty())
                .or_else(|| {
                    self.active_profile
                        .as_ref()
                        .and(self.selected_group.as_ref())
                        .filter(|group| {
                            !crate::session::is_synthetic_project_header(group)
                                && !crate::session::is_archived_section_path(group)
                                && !crate::session::is_trash_section_path(group)
                        })
                        .map(|group| crate::session::project_group_display_name(group).to_string())
                });
            let path = source.map(|row| row.repo_path().to_string()).or_else(|| {
                self.active_profile
                    .as_ref()
                    .and(self.selected_group.as_ref())
                    .and_then(|group| self.group_repo_path(group))
            });
            let mut dialog = crate::tui::dialogs::NewSessionDialog::new(
                self.available_tools.clone(),
                self.all_group_paths(),
                &profile,
                profiles,
            );
            if let Some(path) = path {
                dialog.set_path(path);
            }
            if let Some(group) = group {
                dialog.set_group(group);
            }
            dialog.set_intake(brief);
            Ok(dialog)
        })();
        match result {
            Ok(dialog) => {
                self.close_tailored();
                self.new_dialog = Some(dialog);
            }
            Err(error) => {
                if let Some(form) = self
                    .tailored_window
                    .as_mut()
                    .and_then(|window| window.intake.as_mut())
                {
                    form.error = Some(error.to_string());
                }
                self.flash_status(format!("Intake setup failed: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intake_edits_multiline_and_requires_two_distinct_confirmations() {
        let mut form = IntakeWindow::default();
        form.paste("first line");
        for code in [KeyCode::Enter, KeyCode::Char('j'), KeyCode::Char('m')] {
            let modifiers = if code == KeyCode::Enter {
                KeyModifiers::NONE
            } else {
                KeyModifiers::CONTROL
            };
            assert!(matches!(
                form.key(KeyEvent::new(code, modifiers)),
                WindowAction::Continue
            ));
        }
        form.paste("second line");
        let submit = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(matches!(form.key(submit), WindowAction::Continue));
        assert_eq!(
            form.brief.as_ref().unwrap().task,
            "first line\n\n\nsecond line"
        );
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            assert!(matches!(
                form.key(KeyEvent::new_with_kind(submit.code, submit.modifiers, kind)),
                WindowAction::Continue
            ));
        }
        assert!(matches!(form.key(submit), WindowAction::ConfigureIntake(_)));
        form.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(form.brief.is_none());
        assert_eq!(form.input.lines().join("\n"), "first line\n\n\nsecond line");
    }
}
