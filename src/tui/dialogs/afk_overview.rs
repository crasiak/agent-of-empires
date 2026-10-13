use super::{afk_review::AfkReviewDialog, DialogResult};
use crate::{
    session::afk::{
        overview::{self, Overview},
        review::{self, Selection},
    },
    tui::styles::Theme,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{prelude::*, widgets::*};
use std::{
    collections::BTreeSet,
    sync::mpsc::{self, Receiver},
};

type Reply = anyhow::Result<Overview>;
type Dispatch = Box<dyn Fn() -> Receiver<Reply> + Send>;
type ReviewFactory = Box<dyn Fn(Vec<Selection>) -> AfkReviewDialog + Send>;
type Row = (String, String, Vec<Selection>);

pub struct AfkOverviewDialog {
    dispatch: Dispatch,
    pending: Option<Receiver<Reply>>,
    snapshot: Option<Overview>,
    error: Option<String>,
    include_archived: bool,
    usage_details: bool,
    selection: ListState,
    marked: BTreeSet<Selection>,
    review: Option<AfkReviewDialog>,
    review_factory: ReviewFactory,
    notice: Option<String>,
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
            usage_details: false,
            selection: ListState::default().with_selected(Some(0)),
            marked: BTreeSet::new(),
            review: None,
            review_factory: Box::new(AfkReviewDialog::new),
            notice: None,
        };
        dialog.refresh();
        dialog
    }
    #[cfg(test)]
    pub(crate) fn with_review_for_test(
        report: review::Report,
        submissions: std::sync::Arc<std::sync::Mutex<Vec<review::annotations::Request>>>,
    ) -> Self {
        use super::afk_review::{Reply as ReviewReply, Request as ReviewRequest};
        let selections = report
            .selections
            .iter()
            .map(|row| row.selection.clone())
            .collect();
        let mut child = AfkReviewDialog::with_dispatch(
            selections,
            Box::new(move |request| {
                let (reply, result) = mpsc::channel();
                match request {
                    ReviewRequest::Read(_) => {
                        assert!(reply.send(ReviewReply::Read(Ok(report.clone()))).is_ok());
                    }
                    ReviewRequest::Annotate(request) => submissions.lock().unwrap().push(request),
                    ReviewRequest::Export(..) => panic!("unexpected export in paste fixture"),
                }
                result
            }),
        );
        assert!(child.tick());
        let mut overview = Self::with_dispatch(Box::new(|| mpsc::channel().1));
        overview.pending = None;
        overview.review = Some(child);
        overview
    }
    #[cfg(test)]
    pub(crate) fn review_editor_text_for_test(&self) -> Option<&str> {
        self.review
            .as_ref()
            .and_then(AfkReviewDialog::editor_text_for_test)
    }
    fn refresh(&mut self) {
        if self.pending.is_none() {
            self.pending = Some((self.dispatch)());
        }
    }
    pub fn tick(&mut self) -> bool {
        let child_changed = self.review.as_mut().is_some_and(AfkReviewDialog::tick);
        let Some(rx) = &self.pending else {
            return child_changed;
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
            Err(mpsc::TryRecvError::Empty) => child_changed,
        }
    }
    fn usage_text(value: &serde_json::Value) -> String {
        let amount = |v: &serde_json::Value| {
            v.as_u64()
                .map_or_else(|| "unknown".into(), |n| n.to_string())
        };
        let window = &value["window"];
        let m = if window.is_object() {
            &window["measurement"]
        } else if value["measurement"].is_object() {
            &value["measurement"]
        } else {
            value
        };
        let guard = if window.is_object() {
            &window["guard"]
        } else {
            &value["guard"]
        };
        format!("Usage coverage: {} (stored, never a fresh observation)\nReported component tokens: {}; complete: {}\nSDK-recorded cost nanoUSD: {}; price basis/subscription billing unknown\nOperator token threshold: {}\nOperator estimate threshold nanoUSD: {}; fixed quotes, NOT an invoice\nOperator-valued amount nanoUSD: {}; complete: {}\nBaseline entry: {}; observed through: {}; cutoff: {}\nClosing: {}; attribution gap: {}; denied: {}\nIndependent work, hidden retries and descendant billing remain unknown",
            value["coverage"].as_str().unwrap_or("unavailable"),amount(&m["reported_tokens"]),m["tokens_complete"],amount(&m["sdk_recorded_nano"]),guard["tokens"].as_u64().map_or_else(||"none (no token guard)".into(),|n|n.to_string()),guard["usd"]["limit_nano"].as_u64().map_or_else(||"none (no USD guard)".into(),|n|n.to_string()),amount(m.get("operator_estimate_nano").unwrap_or(&value["operator_estimate_nano"])),m["valuation_complete"],window["baseline"]["count"],window["observed_through"]["count"],window["cutoff"]["count"],window["closing"],window["gap"],window["blocked"])
    }
    fn rows(&self) -> Vec<Row> {
        let Some(snapshot) = &self.snapshot else {
            return vec![];
        };
        let mut rows = Vec::new();
        for historical in &snapshot.historical_authority {
            rows.push((format!("[{}] HISTORICAL AUTHORITY {}", historical.profile, historical.instance_id),
                format!("{}; {} evidence references; {} retained windows.\nReturn review exact window: {}", historical.observation, historical.evidence.len(), historical.windows.len(), historical.review_windows.last().map_or("unavailable", |s| s.window_id.as_str())), historical.review_windows.last().cloned().into_iter().collect()));
            Self::older_windows(&mut rows, &historical.review_windows);
        }
        for profile in &snapshot.profiles {
            for error in &profile.errors {
                rows.push((
                    format!("[{}] PROFILE ERROR", profile.profile),
                    error.clone(),
                    vec![],
                ));
            }
            for batch in &profile.batches {
                let budget = if batch["shared_requests"].is_null() {
                    if batch["usage"]["guard"].is_object() {
                        "Shared usage guard; no shared request cap".into()
                    } else {
                        "No shared budget".into()
                    }
                } else {
                    format!(
                        "{}/{} SDK charges",
                        batch["shared_charges"], batch["shared_requests"]
                    )
                };
                rows.push((format!("[{}] BATCH {} | {}", profile.profile, batch["operation_id"].as_str().unwrap_or("unknown"), budget),format!("Original operation: {}\n{}; held unbound: {}\nExpiry: {}; expired: {}; stopped: {}\n{}\nOriginal members: {}", batch["operation_id"],budget,batch["held_unbound"],batch["expires_at_ms"],batch["expired"],batch["stopped"],batch["observation"].as_str().unwrap_or("stored only"),batch["members"]), profile.review_batches.get(batch["operation_id"].as_str().unwrap_or("")).cloned().unwrap_or_default()));
                if self.usage_details {
                    rows.last_mut().unwrap().1 = Self::usage_text(&batch["usage"]);
                }
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
                detail.push_str(&format!(
                    "\nBulk preference: {} (epoch {:?}); {}",
                    session.bulk_preference,
                    session.exception_epoch,
                    session.batch.as_ref().map_or_else(
                        || session.batch_membership.into(),
                        |b| format!("batch {}", b["operation_id"])
                    )
                ));
                detail.push_str(&format!(
                    "\nUsage: {}; reported tokens: {}; SDK cost basis unverified",
                    session.usage["coverage"].as_str().unwrap_or("unavailable"),
                    session.usage["measurement"]["reported_tokens"]
                        .as_u64()
                        .map_or_else(|| "unknown".into(), |n| n.to_string())
                ));
                if let Some(guard) = session.usage["window"]
                    .get("guard")
                    .filter(|g| !g.is_null())
                {
                    detail.push_str(&format!(
                        "\nObserved guard: {}; denial: {}; closing: {}",
                        guard,
                        session.usage["window"]["blocked"],
                        session.usage["window"]["closing"]
                    ));
                }
                if !session.errors.is_empty() {
                    detail = format!("ERROR: {}\n{}", session.errors.join("; "), detail);
                }
                if self.usage_details {
                    detail = Self::usage_text(&session.usage);
                }
                if let Some(window) = session.review_windows.last() {
                    detail.push_str(&format!(
                        "\nReturn review exact window: {}",
                        review::terminal_text(&window.window_id)
                    ));
                }
                rows.push((
                    label,
                    detail,
                    session.review_windows.last().cloned().into_iter().collect(),
                ));
                Self::older_windows(&mut rows, &session.review_windows);
            }
            if profile.sessions.is_empty() && profile.errors.is_empty() {
                rows.push((
                    format!("[{}] No stored sessions", profile.profile),
                    "Read-only profile snapshot".into(),
                    vec![],
                ));
            }
        }
        rows
    }
    fn older_windows(rows: &mut Vec<Row>, windows: &[Selection]) {
        for window in windows.iter().rev().skip(1) {
            rows.push((
                format!(
                    "  [{}] {} | historical window {}",
                    review::terminal_text(&window.profile),
                    review::terminal_text(&window.session_id),
                    review::terminal_text(&window.window_id)
                ),
                "Exact stored history; Space marks this original window, v opens passive review"
                    .into(),
                vec![window.clone()],
            ));
        }
    }
    fn current_windows(&self) -> Vec<Selection> {
        self.selection
            .selected()
            .and_then(|index| self.rows().get(index).map(|row| row.2.clone()))
            .unwrap_or_default()
    }
    fn mark_current(&mut self) {
        let windows = self.current_windows();
        if windows.is_empty() {
            self.notice = Some("No exact stored window on this row".into());
            return;
        }
        if windows.iter().all(|window| self.marked.contains(window)) {
            for window in windows {
                self.marked.remove(&window);
            }
        } else {
            let proposed: BTreeSet<_> = self.marked.iter().cloned().chain(windows).collect();
            if proposed.len() > review::MAX_SELECTIONS {
                self.notice = Some("At most 16 exact windows; no marks changed".into());
                return;
            }
            self.marked = proposed;
        }
        self.notice = None;
    }
    fn open_review(&mut self) {
        let windows: BTreeSet<_> = if self.marked.is_empty() {
            self.current_windows().into_iter().collect()
        } else {
            self.marked.clone()
        };
        if windows.is_empty() || windows.len() > review::MAX_SELECTIONS {
            self.notice =
                Some("Select 1..16 exact stored windows first; no partial selection opened".into());
            return;
        }
        self.review = Some((self.review_factory)(windows.into_iter().collect()));
        self.notice = None;
    }
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(review) = &mut self.review {
            review.handle_paste(text);
        }
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<()> {
        if let Some(review) = &mut self.review {
            if matches!(review.handle_key(key), DialogResult::Cancel) {
                self.review = None;
            }
            return DialogResult::Continue;
        }
        match key.code {
            KeyCode::Char(' ') => self.mark_current(),
            KeyCode::Char('v') => self.open_review(),
            KeyCode::Char('x') => {
                self.marked.clear();
                self.notice = None;
            }
            KeyCode::Esc => return DialogResult::Cancel,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('u') => self.usage_details = !self.usage_details,
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
        if let Some(review) = &mut self.review {
            review.render(frame, area, theme);
            return;
        }
        let block = super::dialog_block(" Global AFK overview (read-only) ", theme);
        let (_, inner) = super::render_dialog_frame(frame, area, 116, 32, block);
        let inner = inner.inner(Margin {
            horizontal: 1,
            vertical: 0,
        });
        let layout = Layout::vertical([
            Constraint::Length(5),
            Constraint::Min(2),
            Constraint::Length(12),
            Constraint::Length(3),
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
        let header = format!("All profiles | stored snapshot: {captured} | acknowledgements not fresh\n{}\n{}\nAccount coverage unknown; SDK reservations are not delivery or billing\n{}", self.snapshot.as_ref().map_or("Shared budgets unobserved",|s|s.shared_budget),self.snapshot.as_ref().map_or("Bulk preferences unobserved",|s|s.bulk_exclusions),if self.pending.is_some() { "Reading host summaries (no runtime probes)..." } else { "Activity is stored, not a live process or authority check" });
        frame.render_widget(
            Paragraph::new(super::afk_review::safe_lines(&header))
                .style(Style::default().fg(theme.text)),
            layout[0],
        );
        let rows = self.rows();
        let items: Vec<_> = rows
            .iter()
            .map(|(label, _, selections)| {
                let mark = if !selections.is_empty()
                    && selections.iter().all(|s| self.marked.contains(s))
                {
                    "[x] "
                } else {
                    ""
                };
                ListItem::new(format!("{mark}{}", review::terminal_text(label)))
            })
            .collect();
        if rows.is_empty() {
            frame.render_widget(
                Paragraph::new(review::terminal_text(
                    self.error.as_deref().unwrap_or("No sessions in this view"),
                ))
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
            .map_or("", |(_, detail, _)| detail.as_str());
        frame.render_widget(
            Paragraph::new(super::afk_review::safe_lines(detail))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text)),
            layout[2],
        );
        let footer = format!("Space: mark | v: return review | x: clear marks | {}/16 original windows marked\nUp/Down: select | u: usage/details | a: archived/trashed {} | r: reread | Esc: close\n{}", self.marked.len(), if self.include_archived { "shown" } else { "hidden" }, self.notice.as_deref().unwrap_or("Read-only: no activation, enrollment, renewal or budget enforcement"));
        frame.render_widget(
            Paragraph::new(super::afk_review::safe_lines(&footer))
                .style(Style::default().fg(theme.text)),
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
            review_windows: vec![],
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
            legacy_evidence: vec![],
            bulk_preference: "unrecorded".into(),
            exception_epoch: Some(0),
            batch: None,
            batch_membership: "outside original batch windows",
            usage: serde_json::json!({"coverage":"not_collected"}),
            errors: vec![],
        };
        calls.lock().unwrap()[0]
            .send(Ok(Overview {
                migration_evidence: vec![],
                historical_authority: vec![],
                captured_at_ms: 1000,
                profiles: vec![
                    ProfileOverview {
                        review_batches: Default::default(),
                        profile: "alpha".into(),
                        sessions: vec![row("workstream", false), row("archived", true)],
                        batches: vec![],
                        errors: vec![],
                    },
                    ProfileOverview {
                        review_batches: Default::default(),
                        profile: "beta".into(),
                        sessions: vec![],
                        batches: vec![],
                        errors: vec!["unreadable profile".into()],
                    },
                ],
                shared_budget: "No shared budget",
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
            "Bulk preference: unrecorded",
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
        dialog.handle_key(KeyEvent::from(KeyCode::Char('u')));
        terminal
            .draw(|f| dialog.render(f, f.area(), &Theme::default()))
            .unwrap();
        let usage = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(usage.contains("Usage coverage: not_collected"));
        assert!(usage.contains("none (no token guard)"));
        assert!(usage.contains("NOT an invoice"));
        assert_eq!(calls.lock().unwrap().len(), 1);
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

#[cfg(test)]
mod review_tests;
