use super::DialogResult;
use crate::{
    session::afk::review::{self, annotations, Format, Report, Selection},
    tui::styles::Theme,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{prelude::*, widgets::*};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

pub(super) enum Request {
    Read(Vec<Selection>),
    Export(Report, Format, PathBuf),
    Annotate(annotations::Request),
}
pub(super) enum Reply {
    Read(anyhow::Result<Report>),
    Written(anyhow::Result<String>),
}
type Dispatch = Box<dyn Fn(Request) -> Receiver<Reply> + Send>;

enum Editor {
    Export {
        path: String,
        format: Format,
    },
    Annotation {
        request: annotations::Request,
        submitted: bool,
    },
}

pub struct AfkReviewDialog {
    dispatch: Dispatch,
    selections: Vec<Selection>,
    pending: Option<Receiver<Reply>>,
    report: Option<Report>,
    selected: ListState,
    scroll: u16,
    editor_scroll: u16,
    editor_follow_tail: bool,
    editor: Option<Editor>,
    message: Option<String>,
}
impl AfkReviewDialog {
    pub fn new(selections: Vec<Selection>) -> Self {
        let (send, receive) = mpsc::channel::<(Request, mpsc::Sender<Reply>)>();
        std::thread::spawn(move || {
            for (request, reply) in receive {
                let result = match request {
                    Request::Read(selections) => Reply::Read(review::collect(&selections)),
                    Request::Export(report, format, path) => {
                        Reply::Written(review::export(&report, format, &path).map(|()| {
                            format!(
                                "{} private export: {}",
                                if report.complete {
                                    "Complete"
                                } else {
                                    "INCOMPLETE"
                                },
                                path.display()
                            )
                        }))
                    }
                    Request::Annotate(request) => {
                        Reply::Written(annotations::append(request).map(|note| {
                            format!(
                                "Annotation {} appended; no authority or answer changed",
                                note.id
                            )
                        }))
                    }
                };
                let _ = reply.send(result);
            }
        });
        Self::with_dispatch(
            selections,
            Box::new(move |request| {
                let (reply, result) = mpsc::channel();
                let _ = send.send((request, reply));
                result
            }),
        )
    }
    pub(super) fn with_dispatch(selections: Vec<Selection>, dispatch: Dispatch) -> Self {
        let mut result = Self {
            dispatch,
            selections,
            pending: None,
            report: None,
            selected: ListState::default().with_selected(Some(0)),
            scroll: 0,
            editor_scroll: 0,
            editor_follow_tail: true,
            editor: None,
            message: None,
        };
        result.refresh();
        result
    }
    #[cfg(test)]
    pub(super) fn editor_text_for_test(&self) -> Option<&str> {
        self.editor.as_ref().map(|editor| match editor {
            Editor::Export { path, .. } => path.as_str(),
            Editor::Annotation { request, .. } => request.body.as_str(),
        })
    }
    fn refresh(&mut self) {
        if self.pending.is_none() {
            self.pending = Some((self.dispatch)(Request::Read(self.selections.clone())));
        }
    }
    pub fn tick(&mut self) -> bool {
        let Some(receiver) = &self.pending else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(reply) => reply,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.message = Some("Review worker unavailable; inspect any attempted annotation/export before retry".into());
                return true;
            }
        };
        self.pending = None;
        match result {
            Reply::Read(Ok(report)) => { self.report = Some(report); self.selected.select(Some(0)); self.scroll = 0; }
            Reply::Read(Err(error)) => self.message = Some(format!("Read failed: {error}; any shown snapshot is older")),
            Reply::Written(Ok(message)) => { self.message = Some(message); self.editor = None; self.refresh(); }
            Reply::Written(Err(error)) => self.message = Some(format!("Write not confirmed: {error}. Retry unchanged request or inspect the destination; no automatic retry.")),
        }
        true
    }
    fn rows(&self) -> Vec<(usize, Option<usize>, String)> {
        let mut result = vec![];
        if let Some(report) = &self.report {
            for (index, row) in report.selections.iter().enumerate() {
                result.push((
                    index,
                    None,
                    format!(
                        "[{}] {} / {}{}",
                        row.selection.profile,
                        row.selection.session_id,
                        row.selection.window_id,
                        if row.errors.is_empty() {
                            ""
                        } else {
                            " [INCOMPLETE]"
                        }
                    ),
                ));
                if let Some(records) = row.window.as_ref().and_then(|w| w["records"].as_array()) {
                    for (record, value) in records.iter().enumerate() {
                        result.push((
                            index,
                            Some(record),
                            format!(
                                "  Decision {} | {}",
                                value["id"].as_str().unwrap_or("unknown"),
                                value["disposition"].as_str().unwrap_or("unavailable")
                            ),
                        ));
                    }
                }
            }
        }
        result
    }
    fn target(&self) -> Option<annotations::Target> {
        let rows = self.rows();
        let (window_index, record_index, _) = rows.get(self.selected.selected()?)?;
        let row = self.report.as_ref()?.selections.get(*window_index)?;
        if !row.errors.is_empty() {
            return None;
        }
        let window = row.window.as_ref()?;
        let (decision, hash) = match record_index {
            Some(index) => {
                let record = window["records"].as_array()?.get(*index)?;
                (
                    Some(record["id"].as_str()?.to_owned()),
                    record["payload_hash"].as_str()?,
                )
            }
            None => (None, window["grant_hash"].as_str()?),
        };
        Some(annotations::Target {
            selection: row.selection.clone(),
            decision,
            original_hash: hash.into(),
        })
    }
    fn begin_annotation(&mut self, kind: annotations::Kind) {
        self.editor_scroll = 0;
        self.editor_follow_tail = true;
        let Some(target) = self.target() else {
            self.message = Some("Select a complete stored window or decision first".into());
            return;
        };
        let prior = if kind == annotations::Kind::Correction {
            self.report
                .as_ref()
                .into_iter()
                .flat_map(|r| &r.selections)
                .flat_map(|w| &w.annotations)
                .rfind(|a| a.target == target)
                .map(|a| a.id.clone())
        } else {
            None
        };
        self.editor = Some(Editor::Annotation {
            request: annotations::Request {
                id: uuid::Uuid::new_v4().to_string(),
                target,
                kind,
                body: String::new(),
                prior,
            },
            submitted: false,
        });
        self.message = None;
    }
    pub fn handle_paste(&mut self, text: &str) {
        if self.pending.is_some() {
            return;
        }
        let input = match &mut self.editor {
            Some(Editor::Annotation {
                submitted: true, ..
            }) => {
                self.message = Some("Submitted annotation is fixed; paste ignored".into());
                return;
            }
            Some(Editor::Annotation { request, .. }) => &mut request.body,
            Some(Editor::Export { path, .. }) => path,
            None => return,
        };
        if input
            .len()
            .checked_add(text.len())
            .is_some_and(|size| size <= 2048)
        {
            input.push_str(text);
            self.editor_follow_tail = true;
            self.message = None;
        } else {
            self.message =
                Some("Input exceeds 2 KiB; entire insertion refused, nothing submitted".into());
        }
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<()> {
        if self.pending.is_some() {
            if key.code == KeyCode::Esc {
                if self.editor.take().is_some() {
                    self.message = Some("Submitted operation remains in progress; closing the editor does not cancel it".into());
                } else {
                    return DialogResult::Cancel;
                }
            }
            return DialogResult::Continue;
        }
        if self.editor.is_some() {
            match key.code {
                KeyCode::PageUp => {
                    self.editor_follow_tail = false;
                    self.editor_scroll = self.editor_scroll.saturating_sub(10);
                    return DialogResult::Continue;
                }
                KeyCode::PageDown => {
                    self.editor_follow_tail = false;
                    self.editor_scroll = self.editor_scroll.saturating_add(10);
                    return DialogResult::Continue;
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.handle_paste(&c.to_string());
                    return DialogResult::Continue;
                }
                _ => (),
            }
        }
        if let Some(editor) = &mut self.editor {
            match key.code {
                KeyCode::Esc => self.editor = None,
                KeyCode::Enter => {
                    let request = match editor {
                        Editor::Annotation { request, submitted }
                            if !request.body.is_empty()
                                || request.kind == annotations::Kind::Reviewed =>
                        {
                            *submitted = true;
                            Some(Request::Annotate(request.clone()))
                        }
                        Editor::Export { path, format } if !path.is_empty() => {
                            self.report.clone().map(|report| {
                                Request::Export(report, *format, PathBuf::from(path.as_str()))
                            })
                        }
                        _ => None,
                    };
                    if let Some(request) = request {
                        self.pending = Some((self.dispatch)(request));
                    }
                }
                code => {
                    let text = match editor {
                        Editor::Annotation {
                            submitted: true, ..
                        } => {
                            self.message = Some("Submitted annotation is fixed: Enter retries the same UUID/content; Esc goes back without undoing it".into());
                            return DialogResult::Continue;
                        }
                        Editor::Annotation { request, .. } => &mut request.body,
                        Editor::Export { path, .. } => path,
                    };
                    match code {
                        KeyCode::Backspace => {
                            text.pop();
                            self.editor_follow_tail = true;
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            text.clear();
                            self.editor_scroll = 0;
                            self.editor_follow_tail = true;
                        }
                        _ => (),
                    }
                }
            }
            return DialogResult::Continue;
        }
        match key.code {
            KeyCode::Esc => return DialogResult::Cancel,
            KeyCode::Char('r') => {
                self.message = None;
                self.refresh();
            }
            KeyCode::Char('j') | KeyCode::Char('m') if self.report.is_some() => {
                self.editor_scroll = 0;
                self.editor_follow_tail = true;
                self.editor = Some(Editor::Export {
                    path: String::new(),
                    format: if key.code == KeyCode::Char('j') {
                        Format::Json
                    } else {
                        Format::Markdown
                    },
                });
                self.message = None;
            }
            KeyCode::Char('n') => self.begin_annotation(annotations::Kind::Note),
            KeyCode::Char('c') => self.begin_annotation(annotations::Kind::Correction),
            KeyCode::Char('v') => self.begin_annotation(annotations::Kind::Reviewed),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            code => {
                let mut selected = self.selected.selected().unwrap_or(0);
                if super::navigate_list(&mut selected, self.rows().len(), code) {
                    self.selected.select(Some(selected));
                    self.scroll = 0;
                }
            }
        }
        DialogResult::Continue
    }
    fn detail(&self) -> String {
        let rows = self.rows();
        let Some((window_index, record_index, _)) =
            self.selected.selected().and_then(|i| rows.get(i))
        else {
            return "No stored selection loaded".into();
        };
        let row = &self.report.as_ref().unwrap().selections[*window_index];
        let mut text = format!(
            "Profile/session/window: {}/{}/{}\nCaptured source revision: {:?}\n",
            review::terminal_text(&row.selection.profile),
            review::terminal_text(&row.selection.session_id),
            review::terminal_text(&row.selection.window_id),
            row.source_revision
        );
        for error in &row.errors {
            text.push_str(&format!("INCOMPLETE: {}\n", review::terminal_text(error)));
        }
        let Some(mut window) = row.window.clone() else {
            text.push_str(&annotation_detail(row, None));
            return safe_lines(&text);
        };
        safe_value(&mut window);
        text.push_str(&format!("Stored state: {} | expired: {} | terminal reason: {}\nWindow action observations: {} | permitted decision: {}\nReservations: {} / {} | nudges: {} | question deferrals: {}\nTask: {}\nScope: {}\n",
            window["state"].as_str().unwrap_or("unavailable"), window["expired"], window["terminal_reason"].as_str().unwrap_or("unobserved"), window["action"]["outcomes"], window["action"]["decision"].as_str().unwrap_or("none recorded"), window["reservations_used"], window["grant"]["requests"], window["nudges_used"], window["question_deferrals_used"], window["grant"]["task"].as_str().unwrap_or("unavailable"), window["grant"]["scope"].as_str().unwrap_or("unavailable")));
        text.push_str(&format!("Host question guard unresolved: {}\nHost nudge unresolved: {}\nStored ambiguity: {}\nReviewed annotations do not resolve host guards; model human-required claims are separate.\n",
            shown_value(&window["unresolved_question_guard"]), shown_value(&window["unresolved_nudge"]), shown_value(&window["ambiguous"])));
        if let Some(index) = record_index {
            let record = &window["records"][*index];
            text.push_str(&format!("\nDecision {} | host disposition: {}\nOriginal payload hash: {}\nRecorded at: {} | body: {}\n",record["id"].as_str().unwrap_or("unknown"),record["disposition"].as_str().unwrap_or("unavailable"),record["payload_hash"].as_str().unwrap_or("unavailable"),record["recorded_at_ms"],record["body_status"]));
            let claims = &record["claims"];
            if claims.is_object() {
                text.push_str(&format!(
                    "Model recommendation: {} | human-required claim: {}\nQuestion: {}\n",
                    claims["recommendation"].as_str().unwrap_or("unavailable"),
                    claims["human_required"],
                    claims["question"].as_str().unwrap_or("unavailable")
                ));
                if let Some(criteria) = claims["criteria"].as_array() {
                    for criterion in criteria {
                        text.push_str(&format!(
                            "Criterion {}: weight {}\n",
                            criterion["name"].as_str().unwrap_or("unknown"),
                            criterion["weight"]
                        ));
                    }
                }
                if let Some(alternatives) = claims["alternatives"].as_array() {
                    for alternative in alternatives {
                        text.push_str(&format!(
                            "Alternative {} | scores {}\n  Pros: {}\n  Cons: {}\n",
                            alternative["name"].as_str().unwrap_or("unknown"),
                            alternative["scores"],
                            alternative["pros"].as_str().unwrap_or("unavailable"),
                            alternative["cons"].as_str().unwrap_or("unavailable")
                        ));
                    }
                }
                for field in [
                    "evidence",
                    "assumptions",
                    "uncertainties",
                    "rationale",
                    "risk",
                    "rollback",
                    "depends_on",
                ] {
                    text.push_str(&format!("{field}: {}\n", claims[field]));
                }
            } else {
                text.push_str("Original claim body unavailable; no rationale reconstructed\n");
            }
        } else {
            text.push_str(&format!("Checkpoints: {}\n", window["checkpoints"]));
            if let Some(reference) = window["shared_usage_reference"].as_object() {
                if let Some(shared) = self.report.as_ref().unwrap().shared_usage.iter().find(|s| {
                    s["profile"] == reference["profile"]
                        && s["operation_id"] == reference["operation_id"]
                }) {
                    text.push_str(&format!("Original shared operation: {shared}\n"));
                }
            }
        }
        text.push_str(&usage_detail(
            &window["usage"],
            &window["shared_usage_reference"],
        ));
        text.push_str(&format!(
            "Question facts: {}\nNudge observations: {}\n",
            window["question"], window["nudges"]
        ));
        let decision = record_index.and_then(|index| window["records"][index]["id"].as_str());
        text.push_str(&annotation_detail(row, decision));
        safe_lines(&text)
    }
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = super::dialog_block(" AFK return review ", theme);
        let (_, inner) = super::render_dialog_frame(frame, area, 124, 40, block);
        let inner = inner.inner(Margin {
            horizontal: 1,
            vertical: 0,
        });
        let layout = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(5),
        ])
        .split(inner);
        let header = self.report.as_ref().map_or_else(|| "Reading stored facts...".into(), |r| format!("{} | {} exact windows | capture {}..{}\nStored observations only; unknown remains unknown. Review does not revoke or grant authority.",if r.complete {"COMPLETE COLLECTION"}else{"INCOMPLETE COLLECTION"},r.selections.len(),r.capture_started_at_ms,r.capture_finished_at_ms));
        frame.render_widget(
            Paragraph::new(safe_lines(&header)).style(Style::default().fg(theme.text)),
            layout[0],
        );
        if let Some(editor) = &self.editor {
            let (title,text) = match editor {
                Editor::Export { path, format } => (format!("Private {format:?} export: new file in an owned 0700 directory"),path.clone()),
                Editor::Annotation { request, submitted } => (format!("{:?} annotation | id {}\nExact target: {}/{}/{} | decision {:?}\nOriginal hash: {} | prior {:?}\nAppend-only operator claim, never approval or an answer{}",request.kind,request.id,review::terminal_text(&request.target.selection.profile),review::terminal_text(&request.target.selection.session_id),review::terminal_text(&request.target.selection.window_id),request.target.decision,request.target.original_hash,request.prior,if *submitted { "; submitted content fixed for exact retry" } else { "" }),request.body.clone()),
            };
            let document = format!("{}\n\n{}\n\nEnter: submit explicitly | Esc: back | Ctrl+u: clear\nPgUp/PgDn: scroll input; typing follows the end\nClosing does not cancel a submitted write.", safe_lines(&title), review::terminal_text(&text));
            let lines = editor_lines(&document, layout[1].width);
            let maximum = u16::try_from(lines.len().saturating_sub(usize::from(layout[1].height)))
                .unwrap_or(u16::MAX);
            self.editor_scroll = if self.editor_follow_tail {
                maximum
            } else {
                self.editor_scroll.min(maximum)
            };
            frame.render_widget(
                Paragraph::new(lines)
                    .scroll((self.editor_scroll, 0))
                    .style(Style::default().fg(theme.text)),
                layout[1],
            );
        } else {
            let panes =
                Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                    .split(layout[1]);
            let rows = self.rows();
            let items: Vec<_> = rows
                .iter()
                .map(|(_, _, label)| ListItem::new(review::terminal_text(label)))
                .collect();
            frame.render_stateful_widget(
                List::new(items)
                    .style(Style::default().fg(theme.text))
                    .highlight_style(Style::default().fg(theme.accent).bold())
                    .highlight_symbol("> "),
                panes[0],
                &mut self.selected,
            );
            frame.render_widget(
                Paragraph::new(self.detail())
                    .wrap(Wrap { trim: false })
                    .scroll((self.scroll, 0))
                    .style(Style::default().fg(theme.text)),
                panes[1],
            );
        }
        let footer = format!("Up/Down: select | PgUp/PgDn: detail | r: refresh | j/m: JSON/Markdown export\nn: note | c: correction | v: reviewed note | Esc: back\n{}\n{}",if self.pending.is_some(){"Operation in progress; no automatic retry"}else{"No automatic action, authority change or question recovery"},review::terminal_text(self.message.as_deref().unwrap_or("")));
        frame.render_widget(
            Paragraph::new(footer)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text)),
            layout[2],
        );
    }
}

fn shown_value(value: &serde_json::Value) -> String {
    if value.is_null() {
        "unavailable".into()
    } else {
        value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string())
    }
}
fn usage_detail(usage: &serde_json::Value, shared: &serde_json::Value) -> String {
    let measurement = &usage["measurement"];
    format!("\nUsage (stored only, not a live observation)\nCoverage: {}\nUnavailable: {}\nReported tokens: {} | complete: {}\nOperator estimate nanoUSD: {} | valuation complete: {} | NOT an invoice\nSDK-recorded estimate nanoUSD: {} | billing and price basis unknown\nMeasurement gaps: {}\nStored attribution gap: {} | denial: {} | pending evidence: {}\nClosing: {} | source complete: {}\nBaseline: {}\nObserved through: {}\nCutoff: {}\nOperator guard/rate basis: {}\nFreshness: {}\nIn-flight overshoot: unknown; already-admitted work may exceed observed guards, not an account ceiling\nShared usage reference: {} (reported once; do not add to local totals)\n",
        usage["coverage"].as_str().unwrap_or(if measurement.is_object() { "stored measurement; completeness shown below" } else { "unavailable" }),
        usage["unavailable"].as_str().unwrap_or("none recorded"),
        shown_value(&measurement["reported_tokens"]), shown_value(&measurement["tokens_complete"]),
        shown_value(&measurement["operator_estimate_nano"]), shown_value(&measurement["valuation_complete"]),
        shown_value(&measurement["sdk_recorded_nano"]), shown_value(&measurement["gaps"]),
        shown_value(&usage["gap"]), shown_value(&usage["blocked"]), shown_value(&usage["negative_evidence_pending"]),
        shown_value(&usage["closing"]), shown_value(&usage["source_complete"]),
        shown_value(&usage["baseline"]), shown_value(&usage["observed_through"]), shown_value(&usage["cutoff"]),
        shown_value(&usage["guard"]), usage["freshness"].as_str().unwrap_or("stored; no live observation"), shown_value(shared))
}
fn annotation_detail(row: &review::SelectedWindow, decision: Option<&str>) -> String {
    let mut text =
        "\nOperator annotations (not answers, approvals or host observations):\n".to_owned();
    for note in row
        .annotations
        .iter()
        .filter(|note| decision.is_none() || note.target.decision.as_deref() == decision)
    {
        text.push_str(&format!("Annotation {} | {:?} at {}\nTarget: {}/{}/{}\nDecision target: {}\nOriginal target hash: {}\nPrior annotation: {}\n{}\n",
            review::terminal_text(&note.id), note.kind, note.created_at_ms,
            review::terminal_text(&note.target.selection.profile), review::terminal_text(&note.target.selection.session_id), review::terminal_text(&note.target.selection.window_id),
            review::terminal_text(note.target.decision.as_deref().unwrap_or("window grant")), review::terminal_text(&note.target.original_hash),
            review::terminal_text(note.prior.as_deref().unwrap_or("none")), review::terminal_text(note.body.as_deref().unwrap_or("[expired body; identity retained]"))));
    }
    text
}
fn editor_lines(document: &str, width: u16) -> Vec<Line<'static>> {
    let limit = usize::from(width.max(1));
    let mut lines = Vec::new();
    for source in document.split('\n') {
        let mut line = String::new();
        let mut columns = 0;
        for c in source.chars() {
            let size = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if columns > 0 && columns + size > limit {
                lines.push(Line::from(std::mem::take(&mut line)));
                columns = 0;
            }
            line.push(c);
            columns += size;
        }
        lines.push(Line::from(line));
    }
    lines
}

// Preserve document layout, not terminal controls inside a line.
pub(super) fn safe_lines(text: &str) -> String {
    text.split('\n')
        .map(review::terminal_text)
        .collect::<Vec<_>>()
        .join("\n")
}
fn safe_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => *text = review::terminal_text(text),
        serde_json::Value::Array(values) => values.iter_mut().for_each(safe_value),
        serde_json::Value::Object(values) => values.values_mut().for_each(safe_value),
        _ => (),
    }
}

#[cfg(test)]
mod tests;
