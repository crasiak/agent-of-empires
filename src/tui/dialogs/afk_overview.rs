use super::DialogResult;
use crate::{
    session::afk::overview::{self, Overview},
    tui::styles::Theme,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{prelude::*, widgets::*};
use std::sync::mpsc::{self, Receiver};

type Reply = anyhow::Result<Overview>;
type Dispatch = Box<dyn Fn() -> Receiver<Reply> + Send>;

pub struct AfkOverviewDialog {
    dispatch: Dispatch,
    pending: Option<Receiver<Reply>>,
    snapshot: Option<Overview>,
    error: Option<String>,
    include_archived: bool,
    selection: ListState,
}
impl Default for AfkOverviewDialog {
    fn default() -> Self {
        Self::new()
    }
}
impl AfkOverviewDialog {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<mpsc::Sender<Reply>>();
        std::thread::spawn(move || {
            for reply in rx {
                let _ = reply.send(overview::snapshot());
            }
        });
        Self::with_dispatch(Box::new(move || {
            let (reply, result) = mpsc::channel();
            let _ = tx.send(reply);
            result
        }))
    }
    fn with_dispatch(dispatch: Dispatch) -> Self {
        let mut dialog = Self {
            dispatch,
            pending: None,
            snapshot: None,
            error: None,
            include_archived: true,
            selection: ListState::default().with_selected(Some(0)),
        };
        dialog.refresh();
        dialog
    }
    fn refresh(&mut self) {
        if self.pending.is_none() {
            self.pending = Some((self.dispatch)());
        }
    }
    pub fn tick(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                match result {
                    Ok(snapshot) => {
                        self.snapshot = Some(snapshot);
                        self.error = None;
                    }
                    Err(error) => {
                        self.snapshot = None;
                        self.error = Some(format!("Overview unavailable: {error}"));
                    }
                }
                self.pending = None;
                self.selection.select(Some(0));
                true
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.error = Some("Overview worker unavailable".into());
                self.snapshot = None;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
        }
    }
    fn rows(&self) -> Vec<(String, String)> {
        let Some(snapshot) = &self.snapshot else {
            return vec![];
        };
        let mut rows = Vec::new();
        for profile in &snapshot.profiles {
            for error in &profile.errors {
                rows.push((
                    format!("[{}] PROFILE ERROR", profile.profile),
                    error.clone(),
                ));
            }
            for session in &profile.sessions {
                if !self.include_archived
                    && (session.archived || session.trashed)
                    && session.errors.is_empty()
                {
                    continue;
                }
                let state = session
                    .delegation
                    .as_ref()
                    .map_or(session.requested_presence.as_str(), |d| d.state.as_str());
                let lifecycle = if session.trashed {
                    " / trashed"
                } else if session.archived {
                    " / archived"
                } else {
                    ""
                };
                let marker = if session.errors.is_empty() {
                    ""
                } else {
                    " [ERROR]"
                };
                let review = if session.delegation.as_ref().is_some_and(|d| {
                    d.questions_requiring_review > 0 || d.human_required_records > 0
                }) {
                    " / human review"
                } else {
                    ""
                };
                let label = format!(
                    "[{}] {} | {}{} | {}{}{}",
                    profile.profile,
                    session.title,
                    session.activity,
                    lifecycle,
                    state,
                    review,
                    marker
                );
                let mut detail = format!("{} / {} ({})\nSupport: {}\nRequested presence: {}; expiry: {}\nPresence ack: {}", profile.profile, session.id, session.tool, session.support, session.requested_presence, session.presence_expires_at_ms.map_or_else(|| "none".into(), |n| n.to_string()), session.presence_acknowledgement);
                if let Some(d) = &session.delegation {
                    detail.push_str(&format!("\nHost window: {}; confirmed: {}; expiry: {}\nReservations: {}/{}; nudges: {}; question deferrals: {}\nHuman review: {} questions, {} guards, {} records; answers not recorded\nQuestion outcome: {}; effects: {:?}\nRuntime ack: {}", d.state, d.host_confirmed, d.expires_at_ms, d.reservations_used, d.requests, d.nudges_used, d.question_deferrals_used, d.questions_requiring_review, d.unresolved_question_guards, d.human_required_records, d.latest_question_outcome.as_deref().unwrap_or("unobserved"), d.action_outcomes, d.runtime_acknowledgement));
                } else {
                    detail.push_str("\nNo stored delegation summary; no live authority inferred");
                }
                if !session.errors.is_empty() {
                    detail = format!("ERROR: {}\n{}", session.errors.join("; "), detail);
                }
                rows.push((label, detail));
            }
            if profile.sessions.is_empty() && profile.errors.is_empty() {
                rows.push((
                    format!("[{}] No stored sessions", profile.profile),
                    "Read-only profile snapshot".into(),
                ));
            }
        }
        rows
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<()> {
        match key.code {
            KeyCode::Esc => return DialogResult::Cancel,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('a') => {
                self.include_archived = !self.include_archived;
                self.selection.select(Some(0));
            }
            code => {
                let mut selected = self.selection.selected().unwrap_or(0);
                if super::navigate_list(&mut selected, self.rows().len(), code) {
                    self.selection.select(Some(selected));
                }
            }
        }
        DialogResult::Continue
    }
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = super::dialog_block(" Global AFK overview (read-only) ", theme);
        let (_, inner) = super::render_dialog_frame(frame, area, 116, 32, block);
        let inner = inner.inner(Margin {
            horizontal: 1,
            vertical: 0,
        });
        let layout = Layout::vertical([
            Constraint::Length(5),
            Constraint::Min(2),
            Constraint::Length(9),
            Constraint::Length(2),
        ])
        .split(inner);
        let captured = self.snapshot.as_ref().map_or_else(
            || "not loaded".into(),
            |s| {
                format!(
                    "{}{}",
                    s.captured_at_ms,
                    if s.complete() { "" } else { " INCOMPLETE" }
                )
            },
        );
        let header = format!("All profiles | stored snapshot: {captured} | acknowledgements not fresh\nNo shared budget; batch budget controls not implemented\nOutside batches; bulk opt-out not recorded by this storage version\nAccount coverage unknown; SDK reservations are not delivery or billing\n{}", if self.pending.is_some() { "Reading host summaries (no runtime probes)..." } else { "Activity is stored, not a live process or authority check" });
        frame.render_widget(
            Paragraph::new(header).style(Style::default().fg(theme.text)),
            layout[0],
        );
        let rows = self.rows();
        let items: Vec<_> = rows
            .iter()
            .map(|(label, _)| ListItem::new(label.as_str()))
            .collect();
        if rows.is_empty() {
            frame.render_widget(
                Paragraph::new(self.error.as_deref().unwrap_or("No sessions in this view"))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(theme.text)),
                layout[1],
            );
        } else {
            frame.render_stateful_widget(
                List::new(items)
                    .style(Style::default().fg(theme.text))
                    .highlight_style(Style::default().fg(theme.accent).bold())
                    .highlight_symbol("> "),
                layout[1],
                &mut self.selection,
            );
        }
        let detail = self
            .selection
            .selected()
            .and_then(|index| rows.get(index))
            .map_or("", |(_, detail)| detail.as_str());
        frame.render_widget(
            Paragraph::new(detail)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text)),
            layout[2],
        );
        let footer = format!("Up/Down: select   a: archived/trashed {}   r: reread   Esc: close\nRead-only: no activation, enrollment, renewal or budget enforcement", if self.include_archived { "shown" } else { "hidden" });
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(theme.text)),
            layout[3],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::afk::overview::{ProfileOverview, SessionOverview};
    use std::sync::{Arc, Mutex};
    #[test]
    fn navigation_archive_filter_and_refresh_never_offer_activation() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let mut dialog = AfkOverviewDialog::with_dispatch(Box::new(move || {
            let (tx, rx) = mpsc::channel();
            recorded.lock().unwrap().push(tx);
            rx
        }));
        dialog.handle_key(KeyEvent::from(KeyCode::Char('r')));
        assert_eq!(calls.lock().unwrap().len(), 1);
        let row = |title: &str, archived| SessionOverview {
            id: title.into(),
            title: title.into(),
            tool: "unsupported".into(),
            activity: "stopped".into(),
            archived,
            trashed: false,
            support: "unsupported host".into(),
            requested_presence: "off".into(),
            presence_expires_at_ms: None,
            presence_acknowledgement: "unobserved".into(),
            delegation: None,
            errors: vec![],
        };
        calls.lock().unwrap()[0]
            .send(Ok(Overview {
                captured_at_ms: 1000,
                profiles: vec![
                    ProfileOverview {
                        profile: "alpha".into(),
                        sessions: vec![row("workstream", false), row("archived", true)],
                        errors: vec![],
                    },
                    ProfileOverview {
                        profile: "beta".into(),
                        sessions: vec![],
                        errors: vec!["unreadable profile".into()],
                    },
                ],
                shared_budget: "not implemented",
                batch_membership: "outside",
                bulk_exclusions: "not recorded",
                account_coverage: "unknown",
                observation: "stored only",
            }))
            .unwrap();
        assert!(dialog.tick());
        dialog.snapshot.as_mut().unwrap().profiles[0].sessions[0].delegation =
            Some(overview::DelegationSummary {
                window: "window".into(),
                state: "ended".into(),
                host_confirmed: true,
                expires_at_ms: 61000,
                reservations_used: 1,
                requests: 2,
                nudges_used: 0,
                question_deferrals_used: 1,
                unresolved_question_guards: 0,
                questions_requiring_review: 1,
                latest_question_outcome: Some("accepted".into()),
                question_answer_observation: "answers not recorded",
                human_required_records: 0,
                action_outcomes: vec![],
                runtime_acknowledgement: "stored; freshness unknown".into(),
                coverage: "reservations only",
            });
        assert!(dialog.rows()[0].0.contains("human review"));
        assert_eq!(dialog.rows().len(), 3);
        dialog.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(dialog.selection.selected(), Some(1));
        dialog.handle_key(KeyEvent::from(KeyCode::Char('a')));
        assert_eq!(dialog.rows().len(), 2);
        assert_eq!(dialog.selection.selected(), Some(0));
        for key in [KeyCode::Enter, KeyCode::Char('o')] {
            dialog.handle_key(KeyEvent::from(key));
        }
        assert_eq!(calls.lock().unwrap().len(), 1);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 34)).unwrap();
        terminal
            .draw(|f| dialog.render(f, f.area(), &Theme::default()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        for expected in [
            "Global AFK overview",
            "INCOMPLETE",
            "No shared budget",
            "not implemented",
            "[alpha] workstream",
            "[beta] PROFILE ERROR",
            "unsupported host",
            "unobserved",
            "archived/trashed hidden",
            "no activation",
            "Human review: 1 questions, 0 guards",
            "Question outcome: accepted",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(!text.contains("[alpha] archived"));
        dialog.handle_key(KeyEvent::from(KeyCode::Char('r')));
        assert_eq!(calls.lock().unwrap().len(), 2);
        calls.lock().unwrap()[1]
            .send(Err(anyhow::anyhow!("scan failed")))
            .unwrap();
        assert!(dialog.tick());
        assert!(dialog.snapshot.is_none());
        assert!(dialog.error.as_ref().unwrap().contains("scan failed"));
        assert_eq!(
            dialog.handle_key(KeyEvent::from(KeyCode::Esc)),
            DialogResult::Cancel
        );
    }
}
