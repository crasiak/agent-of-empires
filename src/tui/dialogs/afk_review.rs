use super::DialogResult;
use crate::{
    session::afk::review::{self, annotations, Format, Report, Selection},
    tui::styles::Theme,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{prelude::*, widgets::*};
use std::{
    collections::{BTreeMap, BTreeSet},
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

#[derive(Clone, Copy)]
enum WriteKind {
    Annotation(annotations::Kind),
    Export { complete: bool },
}
impl WriteKind {
    fn receipt(self) -> &'static str {
        match self {
            Self::Annotation(annotations::Kind::Note) => "Note saved",
            Self::Annotation(annotations::Kind::Correction) => "Correction saved",
            Self::Annotation(annotations::Kind::Reviewed) => "Reviewed claim saved",
            Self::Export { complete: true } => "Export saved",
            Self::Export { complete: false } => "Export saved (INCOMPLETE)",
        }
    }
}

mod presentation;

#[derive(Clone, Default)]
pub(super) struct ReviewLabels {
    pub names: BTreeMap<(String, String), Option<String>>,
    pub historical: BTreeSet<Selection>,
}
impl ReviewLabels {
    pub(super) fn name(&self, selection: &Selection) -> &str {
        self.names
            .get(&(selection.profile.clone(), selection.session_id.clone()))
            .and_then(Option::as_deref)
            .unwrap_or_else(|| {
                if self.historical.contains(selection) {
                    "Historical session (name unavailable)"
                } else {
                    "Session name unavailable"
                }
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Summary,
    Decisions,
    Diagnostics,
}

struct Row {
    window: Selection,
    decision: Option<String>,
    label: String,
}

pub struct AfkReviewDialog {
    dispatch: Dispatch,
    selections: Vec<Selection>,
    labels: ReviewLabels,
    view: View,
    diagnostic_origin: View,
    selected_window: Option<Selection>,
    decisions: BTreeMap<Selection, Option<String>>,
    read_failed: Option<String>,
    // Uncertainty belongs to the unchanged editor/pending request; abandoned attempts stay sticky.
    write_unconfirmed: bool,
    earlier_write_unconfirmed: bool,
    worker_unavailable: bool,
    pending_write: Option<WriteKind>,
    receipt: Option<&'static str>,
    actions_help: bool,
    pending: Option<Receiver<Reply>>,
    report: Option<Report>,
    selected: ListState,
    scroll: u16,
    scroll_page: u16,
    editor_scroll: u16,
    editor_follow_tail: bool,
    editor: Option<Editor>,
    message: Option<String>,
}
impl AfkReviewDialog {
    pub(super) fn new(selections: Vec<Selection>, labels: ReviewLabels) -> Self {
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
            labels,
            Box::new(move |request| {
                let (reply, result) = mpsc::channel();
                let _ = send.send((request, reply));
                result
            }),
        )
    }
    pub(super) fn with_dispatch(
        selections: Vec<Selection>,
        labels: ReviewLabels,
        dispatch: Dispatch,
    ) -> Self {
        let mut result = Self {
            dispatch,
            selected_window: selections.first().cloned(),
            selections,
            labels,
            view: View::Summary,
            diagnostic_origin: View::Summary,
            decisions: BTreeMap::new(),
            read_failed: None,
            write_unconfirmed: false,
            earlier_write_unconfirmed: false,
            worker_unavailable: false,
            pending_write: None,
            receipt: None,
            actions_help: false,
            pending: None,
            report: None,
            selected: ListState::default().with_selected(Some(0)),
            scroll: 0,
            scroll_page: 10,
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
            self.pending_write = None;
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
                self.worker_unavailable = true;
                if self.pending_write.take().is_some() {
                    self.unconfirmed_write();
                } else {
                    self.read_failed = Some("Review worker unavailable".into());
                }
                self.message = Some("Review worker unavailable; inspect any attempted annotation/export before retry".into());
                return true;
            }
        };
        self.pending = None;
        let written = self.pending_write.take();
        self.worker_unavailable = false;
        match result {
            Reply::Read(Ok(report)) => {
                self.report = Some(report);
                self.read_failed = None;
                self.sync_selection();
            }
            Reply::Read(Err(error)) => self.read_failed = Some(format!("{error}")),
            Reply::Written(Ok(message)) => {
                self.write_unconfirmed = false;
                self.receipt = written.map(WriteKind::receipt);
                self.message = Some(message);
                self.editor = None;
                self.refresh();
            }
            Reply::Written(Err(error)) => {
                self.unconfirmed_write();
                self.message = Some(format!("Write not confirmed: {error}. Retry unchanged request or inspect the destination; no automatic retry."));
            }
        }
        true
    }
    fn unconfirmed_write(&mut self) {
        if self.editor.is_some() {
            self.write_unconfirmed = true;
        } else {
            self.earlier_write_unconfirmed = true;
            self.write_unconfirmed = false;
        }
    }
    fn abandon_uncertain_request(&mut self) {
        self.earlier_write_unconfirmed |= self.write_unconfirmed;
        self.write_unconfirmed = false;
    }
    fn close_editor(&mut self) {
        self.editor = None;
        if self.pending.is_none() {
            self.abandon_uncertain_request();
        }
    }
    fn back(&mut self) -> DialogResult<()> {
        if self.actions_help {
            self.actions_help = false;
            return DialogResult::Continue;
        }
        self.view = match self.view {
            View::Diagnostics => self.diagnostic_origin,
            View::Decisions => View::Summary,
            View::Summary => return DialogResult::Cancel,
        };
        self.scroll = 0;
        self.sync_selection();
        DialogResult::Continue
    }
    fn selection_view(&self) -> View {
        if self.view == View::Diagnostics {
            self.diagnostic_origin
        } else {
            self.view
        }
    }
    fn selected_row(&self) -> Option<&review::SelectedWindow> {
        let selection = self.selected_window.as_ref()?;
        self.report
            .as_ref()?
            .selections
            .iter()
            .find(|r| &r.selection == selection)
    }
    fn decision(&self) -> Option<&str> {
        if self.selection_view() == View::Summary {
            return None;
        }
        self.decisions
            .get(self.selected_window.as_ref()?)
            .and_then(Option::as_deref)
    }
    fn label(&self, selection: &Selection, width: u16) -> String {
        let ordinal = self
            .selections
            .iter()
            .position(|s| s == selection)
            .map_or(0, |i| i + 1);
        let prefix = format!("#{ordinal} ");
        let available = width.saturating_sub(prefix.len() as u16);
        let profile_width = (available / 3)
            .clamp(8, 20)
            .min(available.saturating_sub(10));
        let name_width = available.saturating_sub(profile_width + 3);
        format!(
            "{prefix}{} [{}]",
            ellipsize(self.labels.name(selection), name_width),
            ellipsize(&selection.profile, profile_width)
        )
    }
    fn window_label(&self, selection: &Selection, width: u16) -> String {
        let attention = self.report.as_ref().is_some_and(|report| {
            presentation::attention(
                report,
                report.selections.iter().find(|r| &r.selection == selection),
            )
            .any()
        });
        let prefix = if attention { "! " } else { "" };
        format!(
            "{prefix}{}",
            self.label(selection, width.saturating_sub(prefix.len() as u16))
        )
    }
    fn rows(&self) -> Vec<Row> {
        if self.selection_view() == View::Summary {
            return self
                .selections
                .iter()
                .map(|selection| Row {
                    window: selection.clone(),
                    decision: None,
                    label: String::new(),
                })
                .collect();
        }
        let Some(selection) = &self.selected_window else {
            return vec![];
        };
        let mut rows = vec![Row {
            window: selection.clone(),
            decision: None,
            label: "Window context".into(),
        }];
        if let Some(window) = self.selected_row().and_then(|r| r.window.as_ref()) {
            for (i, record) in presentation::records(window).iter().enumerate() {
                if let Some(id) = record["id"].as_str() {
                    rows.push(Row {
                        window: selection.clone(),
                        decision: Some(id.into()),
                        label: format!(
                            "Decision {} | {}",
                            i + 1,
                            review::terminal_text(
                                record["disposition"].as_str().unwrap_or("unavailable")
                            )
                        ),
                    });
                }
            }
        }
        if let Some(id) = self.decision() {
            if !rows.iter().any(|r| r.decision.as_deref() == Some(id)) {
                rows.push(Row {
                    window: selection.clone(),
                    decision: Some(id.into()),
                    label: "Selected decision unavailable".into(),
                });
            }
        }
        rows
    }
    fn sync_selection(&mut self) {
        let rows = self.rows();
        let index = rows.iter().position(|row| {
            Some(&row.window) == self.selected_window.as_ref()
                && row.decision.as_deref() == self.decision()
        });
        self.selected.select(index);
    }
    fn open_decisions(&mut self) {
        let Some(selection) = self.selected_window.clone() else {
            return;
        };
        let first = self
            .selected_row()
            .and_then(|r| r.window.as_ref())
            .and_then(|w| presentation::records(w).first())
            .and_then(|r| r["id"].as_str())
            .map(str::to_owned);
        self.decisions.entry(selection).or_insert(first);
        self.view = View::Decisions;
        self.scroll = 0;
        self.sync_selection();
    }
    fn target(&self) -> Option<annotations::Target> {
        let row = self.selected_row()?;
        if !row.errors.is_empty() {
            return None;
        }
        let window = row.window.as_ref()?;
        let decision = self.decision();
        let hash = match decision {
            Some(id) => presentation::records(window)
                .iter()
                .find(|r| r["id"].as_str() == Some(id))?["payload_hash"]
                .as_str()?,
            None => window["grant_hash"].as_str()?,
        };
        Some(annotations::Target {
            selection: row.selection.clone(),
            decision: decision.map(str::to_owned),
            original_hash: hash.into(),
        })
    }
    fn begin_annotation(&mut self, kind: annotations::Kind) {
        self.actions_help = false;
        self.receipt = None;
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
        let editing_export = matches!(self.editor, Some(Editor::Export { .. }));
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
            if editing_export && !text.is_empty() {
                self.abandon_uncertain_request();
            }
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
                if self.editor.is_some() {
                    self.close_editor();
                    self.message = Some("Submitted operation remains in progress; closing the editor does not cancel it".into());
                } else {
                    return self.back();
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
                KeyCode::Esc => self.close_editor(),
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
                        self.pending_write = Some(match &request {
                            Request::Annotate(request) => WriteKind::Annotation(request.kind),
                            Request::Export(report, _, _) => WriteKind::Export {
                                complete: report.complete,
                            },
                            Request::Read(_) => unreachable!("editors only submit writes"),
                        });
                        self.receipt = None;
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
                            let changed = text.pop().is_some();
                            if changed {
                                self.abandon_uncertain_request();
                            }
                            self.editor_follow_tail = true;
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let changed = !text.is_empty();
                            text.clear();
                            if changed {
                                self.abandon_uncertain_request();
                            }
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
            KeyCode::Esc => return self.back(),
            KeyCode::Char('?') => self.actions_help = !self.actions_help,
            KeyCode::Enter if self.view == View::Summary => self.open_decisions(),
            KeyCode::Char('d') => {
                if self.view == View::Diagnostics {
                    self.view = self.diagnostic_origin;
                } else {
                    self.diagnostic_origin = self.view;
                    self.view = View::Diagnostics;
                }
                self.scroll = 0;
            }
            KeyCode::Char('r') => {
                self.actions_help = false;
                self.refresh();
            }
            KeyCode::Char('j') | KeyCode::Char('m') if self.report.is_some() => {
                self.actions_help = false;
                self.receipt = None;
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
            KeyCode::Char('j') | KeyCode::Char('m') => (),
            KeyCode::Char('n') => self.begin_annotation(annotations::Kind::Note),
            KeyCode::Char('c') => self.begin_annotation(annotations::Kind::Correction),
            KeyCode::Char('v') => self.begin_annotation(annotations::Kind::Reviewed),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(self.scroll_page),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.scroll_page),
            code if self.view != View::Diagnostics => {
                let rows = self.rows();
                let mut selected = self.selected.selected().unwrap_or(0);
                if super::navigate_list(&mut selected, rows.len(), code) {
                    if let Some(row) = rows.get(selected) {
                        self.selected_window = Some(row.window.clone());
                        if self.view == View::Decisions {
                            self.decisions
                                .insert(row.window.clone(), row.decision.clone());
                        }
                        self.selected.select(Some(selected));
                        self.scroll = 0;
                    }
                }
            }
            _ => (),
        }
        DialogResult::Continue
    }
    fn detail(&self) -> String {
        let mut text = match (self.report.as_ref(), self.selected_row()) {
            (Some(report), Some(row)) => match self.view {
                View::Summary => presentation::summary(report, row),
                View::Decisions => presentation::decisions(row, self.decision()),
                View::Diagnostics => presentation::diagnostics(report, row, self.decision()),
            },
            _ => {
                let mut text =
                    "Selected window unavailable; no replacement selected. Annotations disabled."
                        .to_owned();
                if self.view == View::Diagnostics {
                    if let Some(selection) = &self.selected_window {
                        text.push_str(&format!(
                            "\nExact target: {}/{}/{}\nSelected decision: {}",
                            review::terminal_text(&selection.profile),
                            review::terminal_text(&selection.session_id),
                            review::terminal_text(&selection.window_id),
                            review::terminal_text(self.decision().unwrap_or("window context"))
                        ));
                    }
                }
                text
            }
        };
        if self.view == View::Diagnostics {
            if let Some(error) = &self.read_failed {
                text.insert_str(
                    0,
                    &format!("Read failed: {}\n", review::terminal_text(error)),
                );
            }
            if let Some(message) = &self.message {
                text.insert_str(0, &format!("{}\n", review::terminal_text(message)));
            }
        }
        safe_lines(&text)
    }
    fn header(&self) -> String {
        let Some(report) = &self.report else {
            return format!(
                "{:?} | {}",
                self.view,
                if self.worker_unavailable {
                    "Review worker unavailable"
                } else {
                    "No snapshot loaded"
                }
            );
        };
        let complete = report.complete
            && self.selections.iter().all(|selection| {
                report.selections.iter().any(|row| {
                    &row.selection == selection && row.window.is_some() && row.errors.is_empty()
                })
            });
        format!(
            "{:?} | {} windows | {} | names: overview",
            self.view,
            self.selections.len(),
            if complete {
                "COMPLETE COLLECTION"
            } else {
                "INCOMPLETE COLLECTION"
            }
        )
    }
    fn status(&self) -> String {
        let mut lines = Vec::new();
        if self.read_failed.is_some() {
            lines.push(if self.report.is_some() {
                "Read failed; shown snapshot is older"
            } else {
                "Read failed; no snapshot available (r refresh)"
            });
        } else if self.selected_row().is_some_and(|r| !r.errors.is_empty()) {
            lines.push("Selected source INCOMPLETE; annotations disabled (d: errors)");
        }
        if self.pending.is_some() {
            lines.push(if self.pending_write.is_some() {
                "Submitted write still running; closing does not cancel"
            } else if self.report.is_some() {
                "Refreshing stored facts..."
            } else {
                "Reading stored facts..."
            });
        } else if self.write_unconfirmed {
            lines.push(if self.worker_unavailable {
                "Review worker unavailable; write not confirmed; no automatic retry"
            } else {
                "Write not confirmed; inspect destination or retry unchanged request"
            });
        }
        if self.earlier_write_unconfirmed {
            lines.push("Earlier write not confirmed; inspect destination before retry");
        }
        if let Some(receipt) = self.receipt {
            lines.push(receipt);
        }
        lines.join("\n")
    }
    fn footer(&self) -> &'static str {
        if self.pending.is_some() {
            return "Esc back (operation continues); other keys wait";
        }
        if let Some(editor) = &self.editor {
            return if matches!(
                editor,
                Editor::Annotation {
                    submitted: true,
                    ..
                }
            ) {
                "Enter retry unchanged  Esc back  PgUp/PgDn scroll input"
            } else {
                "Enter submit  Esc back  Ctrl+u clear  PgUp/PgDn scroll input"
            };
        }
        if self.actions_help {
            return "j/m export full report  n note  c correction  v reviewed claim\nr refresh  PgUp/PgDn scroll  ? / Esc close help\nNotes are claims, not answers or approvals. Review changes no authority.\nNames are from overview, not historical; exact identities in diagnostics.";
        }
        match self.view {
            View::Summary => {
                "↑↓ select  Enter decisions  d diagnostics  r refresh  ? actions  Esc back"
            }
            View::Decisions => {
                "↑↓ decisions  PgUp/PgDn scroll  d diagnostics  ? actions  Esc summary"
            }
            View::Diagnostics => "PgUp/PgDn scroll  d / Esc return  r refresh  ? actions",
        }
    }
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = super::dialog_block(" AFK return review ", theme);
        let (_, inner) = super::render_dialog_frame(frame, area, 128, 42, block);
        let inner = inner.inner(Margin {
            horizontal: 1,
            vertical: 0,
        });
        let status = self.status();
        let footer = self.footer();
        let layout = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(status.lines().count() as u16),
            Constraint::Min(1),
            Constraint::Length(footer.lines().count() as u16),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(self.header()).style(Style::default().fg(theme.text)),
            layout[0],
        );
        frame.render_widget(
            Paragraph::new(
                status
                    .lines()
                    .map(|line| {
                        Line::styled(
                            line.to_owned(),
                            Style::default().fg(if Some(line) == self.receipt {
                                theme.running
                            } else {
                                theme.waiting
                            }),
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
            layout[1],
        );
        if let Some(editor) = &self.editor {
            let (title,text) = match editor {
                Editor::Export { path, format } => (format!("Private {format:?} export: new file in an owned 0700 directory"),path.clone()),
                Editor::Annotation { request, submitted } => (format!("{:?} annotation | id {}\nExact target: {}/{}/{} | decision {:?}\nOriginal hash: {} | prior {:?}\nAppend-only operator claim, never approval or an answer{}",request.kind,request.id,review::terminal_text(&request.target.selection.profile),review::terminal_text(&request.target.selection.session_id),review::terminal_text(&request.target.selection.window_id),request.target.decision,request.target.original_hash,request.prior,if *submitted { "; submitted content fixed for exact retry" } else { "" }),request.body.clone()),
            };
            let document = format!(
                "{}\n\n{}\n\n{}",
                safe_lines(&title),
                review::terminal_text(&text),
                review::terminal_text(self.message.as_deref().unwrap_or(""))
            );
            let lines = editor_lines(&document, layout[2].width);
            let maximum = u16::try_from(lines.len().saturating_sub(usize::from(layout[2].height)))
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
                layout[2],
            );
        } else {
            let preview = if self.view == View::Diagnostics {
                layout[2]
            } else {
                let compact = inner.width < 100;
                let panes = if compact {
                    Layout::vertical([
                        Constraint::Length(2),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ])
                    .split(layout[2])
                } else {
                    Layout::horizontal([
                        Constraint::Length(34),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ])
                    .split(layout[2])
                };
                let separator = if compact {
                    "─".repeat(usize::from(panes[1].width))
                } else {
                    (0..panes[1].height)
                        .map(|_| "│")
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                frame.render_widget(
                    Paragraph::new(separator).style(Style::default().fg(theme.border)),
                    panes[1],
                );
                let list = panes[0].inner(Margin {
                    horizontal: 1,
                    vertical: 0,
                });
                let rows = self.rows();
                let items: Vec<_> = rows
                    .iter()
                    .map(|row| {
                        ListItem::new(if self.view == View::Summary {
                            self.window_label(&row.window, list.width.saturating_sub(2))
                        } else {
                            ellipsize(&row.label, list.width.saturating_sub(2))
                        })
                    })
                    .collect();
                frame.render_stateful_widget(
                    List::new(items)
                        .style(Style::default().fg(theme.text))
                        .highlight_style(Style::default().fg(theme.accent).bold())
                        .highlight_symbol("> "),
                    list,
                    &mut self.selected,
                );
                panes[2].inner(Margin {
                    horizontal: 1,
                    vertical: 0,
                })
            };
            let mut pinned = vec![self.selected_window.as_ref().map_or_else(
                || "No window selected".into(),
                |s| self.label(s, preview.width),
            )];
            if let Some(report) = &self.report {
                pinned.extend(presentation::pinned(
                    report,
                    self.selected_row(),
                    self.decision(),
                ));
            }
            if self.decision().is_some() && self.target().is_none() {
                pinned.push(
                    "Selected decision unavailable or incomplete; annotations disabled".into(),
                );
            }
            let pinned: Vec<_> = pinned
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    let text = ellipsize(text, preview.width);
                    if index == 0 {
                        Line::styled(text, Style::default().fg(theme.title).bold())
                    } else if text.starts_with("Outcome:") || text.starts_with("Effect:") {
                        let (label, value) = text.split_once(':').unwrap();
                        Line::from(vec![
                            Span::styled(format!("{label}:"), Style::default().bold()),
                            Span::raw(value.to_owned()),
                        ])
                    } else if text == "Needs attention: none recorded" {
                        Line::from(text)
                    } else {
                        Line::styled(text, Style::default().fg(theme.waiting).bold())
                    }
                })
                .collect();
            let content =
                Layout::vertical([Constraint::Length(pinned.len() as u16), Constraint::Min(1)])
                    .split(preview);
            frame.render_widget(
                Paragraph::new(pinned).style(Style::default().fg(theme.text)),
                content[0],
            );
            self.scroll_page = content[1].height.saturating_sub(1).clamp(1, 10);
            let lines = editor_lines(&self.detail(), content[1].width);
            let maximum = u16::try_from(lines.len().saturating_sub(usize::from(content[1].height)))
                .unwrap_or(u16::MAX);
            self.scroll = self.scroll.min(maximum);
            frame.render_widget(
                Paragraph::new(lines)
                    .scroll((self.scroll, 0))
                    .style(Style::default().fg(theme.text)),
                content[1],
            );
        }
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(theme.text)),
            layout[3],
        );
    }
}

fn ellipsize(text: &str, width: u16) -> String {
    let text = review::terminal_text(text);
    let limit = usize::from(width);
    if unicode_width::UnicodeWidthStr::width(text.as_str()) <= limit {
        return text;
    }
    if limit == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut columns = 0;
    for c in text.chars() {
        let size = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if columns + size >= limit {
            break;
        }
        result.push(c);
        columns += size;
    }
    result.push('…');
    result
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
    format!("\nUsage (stored only, not a live observation)\nCoverage: {}\nUnavailable: {}\nReported tokens: {} | complete: {}\nOperator estimate nanoUSD: {} | valuation complete: {}\nNOT an invoice\nSDK-recorded estimate nanoUSD: {} | billing and price basis unknown\nMeasurement gaps: {}\nStored attribution gap: {} | denial: {} | pending evidence: {}\nClosing: {} | source complete: {}\nBaseline: {}\nObserved through: {}\nCutoff: {}\nOperator guard/rate basis: {}\nFreshness: {}\nIn-flight overshoot: unknown; already-admitted work may exceed observed guards, not an account ceiling\nShared usage reference: {} (reported once; do not add to local totals)\n",
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
mod disclosure_tests;
#[cfg(test)]
mod tests;
