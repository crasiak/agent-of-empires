use crate::{
    session::tailored::{
        research::{self, Backend, Draft, Inputs, Operation, SourceScope, Store},
        CaptureAccount,
    },
    tui::styles::Theme,
};
use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::{TextArea, WrapMode};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

const LABELS: [&str; 10] = [
    "Original quick prompt (retained)",
    "Explicit context",
    "Sources: one category: explicit scope per line",
    "Developed prompt",
    "Workspace (existing absolute path)",
    "Permitted scope",
    "Evaluation criteria",
    "Public disclosure limits",
    "Consequential unknowns resolution",
    "Literal discovery answer (F9 sends)",
];
enum ResultMessage {
    Draft(Box<Draft>),
    Reply,
}
pub(super) struct ResearchWindow {
    pub(super) id: String,
    pub(super) new_draft: bool,
    draft: Option<Draft>,
    inputs: Inputs,
    fields: Vec<TextArea<'static>>,
    focus: usize,
    checking: Option<mpsc::Receiver<Result<ResultMessage, String>>>,
    output_check: Option<mpsc::Receiver<Result<String, String>>>,
    output_at: Instant,
    output: String,
    show_output: bool,
    output_scroll: u16,
    error: Option<String>,
}
impl ResearchWindow {
    pub fn new(account: CaptureAccount, draft: Option<Draft>) -> Self {
        let mut window = Self {
            new_draft: draft.is_none(),
            id: draft
                .as_ref()
                .map_or_else(|| uuid::Uuid::new_v4().to_string(), |d| d.id.clone()),
            draft: None,
            inputs: Inputs::new(account),
            fields: (0..10)
                .map(|_| {
                    let mut field = TextArea::default();
                    field.set_wrap_mode(WrapMode::WordOrGlyph);
                    field
                })
                .collect(),
            focus: 0,
            checking: None,
            output_check: None,
            output_at: Instant::now() - Duration::from_secs(3),
            output: String::new(),
            show_output: false,
            output_scroll: 0,
            error: None,
        };
        window.fields[7].insert_str(&window.inputs.public_disclosure);
        if let Some(draft) = draft {
            window.accept(draft);
        }
        window
    }
    fn accept(&mut self, draft: Draft) {
        self.new_draft = false;
        self.inputs = draft.inputs.clone();
        let sources = draft
            .inputs
            .sources
            .iter()
            .map(|s| {
                format!(
                    "{}: {}",
                    serde_json::to_value(s.category).unwrap().as_str().unwrap(),
                    s.scope
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        for (index, value) in [
            &draft.original_prompt,
            &self.inputs.context,
            &sources,
            &self.inputs.developed_prompt,
            &self.inputs.workspace,
            &self.inputs.permitted_scope,
            &self.inputs.evaluation,
            &self.inputs.public_disclosure,
            &self.inputs.unknowns_resolution,
        ]
        .into_iter()
        .enumerate()
        {
            self.fields[index] = TextArea::from(value.split('\n'));
            self.fields[index].set_wrap_mode(WrapMode::WordOrGlyph);
        }
        self.draft = Some(draft);
    }
    fn locked(&self) -> bool {
        self.draft.as_ref().is_some_and(|d| {
            matches!(
                d.state,
                research::State::Developing
                    | research::State::Running
                    | research::State::Submitted
                    | research::State::Uncertain
            )
        })
    }
    fn values(&self) -> Result<Inputs> {
        let value = |i: usize| self.fields[i].lines().join("\n");
        let mut inputs = self.inputs.clone();
        inputs.context = value(1);
        inputs.developed_prompt = value(3);
        inputs.workspace = value(4);
        inputs.permitted_scope = value(5);
        inputs.evaluation = value(6);
        inputs.public_disclosure = value(7);
        inputs.unknowns_resolution = value(8);
        inputs.sources = value(2)
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(|line| -> Result<_> {
                let (category, scope) = line
                    .split_once(':')
                    .context("Sources need category: scope per line")?;
                Ok(SourceScope {
                    category: serde_json::from_value(serde_json::json!(category
                        .trim()
                        .to_lowercase()))
                    .context(
                        "Use grafana, otel, zulip (Personal), slack (Work), supplied or public",
                    )?,
                    scope: scope.trim().into(),
                })
            })
            .collect::<Result<_>>()?;
        inputs.validate()?;
        Ok(inputs)
    }
    pub(super) fn account(&self) -> CaptureAccount {
        self.inputs.account
    }
    pub(super) fn can_switch_account(&self) -> bool {
        self.draft.is_none() && self.checking.is_none()
    }
    fn can_edit(&self) -> bool {
        self.checking.is_none()
            && !self.show_output
            && ((!self.locked() && (self.focus != 0 || self.draft.is_none())) || self.focus == 9)
    }
    fn edit_key(&mut self, key: KeyEvent) {
        if self.can_edit() {
            self.fields[self.focus].input(key);
            self.inputs.consequences_reviewed = false;
        }
    }
    pub(super) fn repeat(&mut self, key: KeyEvent) {
        if !(key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL)) {
            self.edit_key(key);
        }
    }
    pub fn key(&mut self, key: KeyEvent) {
        if self.checking.is_some() {
            return;
        }
        if key.code == KeyCode::F(12) {
            if let Some(draft) = self
                .draft
                .as_ref()
                .filter(|d| d.state == research::State::Developing)
            {
                let id = self.id.clone();
                let revision = draft.revision;
                self.start(move || {
                    Ok(ResultMessage::Draft(Box::new(Store::open()?.operate(
                        Operation {
                            operation: "recover-development".into(),
                            id,
                            revision: Some(revision),
                            original_prompt: None,
                            inputs: None,
                            hidden: None,
                        },
                    )?)))
                });
            }
            return;
        }
        if key.code == KeyCode::F(11) {
            self.show_output = !self.show_output;
            return;
        }
        if self.show_output {
            match key.code {
                KeyCode::Up | KeyCode::PageUp => {
                    self.output_scroll = self.output_scroll.saturating_add(5)
                }
                KeyCode::Down | KeyCode::PageDown => {
                    self.output_scroll = self.output_scroll.saturating_sub(5)
                }
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::F(10) {
            let id = self.id.clone();
            self.start(move || Ok(ResultMessage::Draft(Box::new(Store::open()?.get(&id)?))));
            return;
        }
        if key.code == KeyCode::F(9) {
            let Some(draft) = self.draft.clone() else {
                return;
            };
            let answer = self.fields[9].lines().join("\n");
            self.start(move || {
                research::reply(&draft, &answer)?;
                Ok(ResultMessage::Reply)
            });
            return;
        }
        if self.locked() {
            match key.code {
                KeyCode::Tab => self.focus = (self.focus + 1) % self.fields.len(),
                KeyCode::BackTab => {
                    self.focus = (self.focus + self.fields.len() - 1) % self.fields.len()
                }
                _ => self.edit_key(key),
            }
            return;
        }
        if key.modifiers == (KeyModifiers::CONTROL | KeyModifiers::ALT) {
            let operation = match key.code {
                KeyCode::Char('d') => Some("develop"),
                KeyCode::Char('s') => Some("save"),
                KeyCode::Char('r') => Some("run"),
                _ => None,
            };
            if let Some(operation) = operation {
                self.submit(operation);
                return;
            }
        }
        match key.code {
            KeyCode::Tab => self.focus = (self.focus + 1) % self.fields.len(),
            KeyCode::BackTab => {
                self.focus = (self.focus + self.fields.len() - 1) % self.fields.len()
            }
            KeyCode::F(6) if self.draft.is_none() => {
                self.inputs.account = match self.inputs.account {
                    CaptureAccount::Personal => CaptureAccount::Work,
                    CaptureAccount::Work => CaptureAccount::Personal,
                };
                self.inputs.consequences_reviewed = false;
            }
            KeyCode::F(7) => {
                self.inputs.backend = match self.inputs.backend {
                    None => Some(Backend::Orx),
                    Some(Backend::Orx) => Some(Backend::Evo),
                    Some(Backend::Evo) => Some(Backend::Orx),
                };
                self.inputs.consequences_reviewed = false;
            }
            KeyCode::F(8) => self.inputs.consequences_reviewed = !self.inputs.consequences_reviewed,
            _ => self.edit_key(key),
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.can_edit() {
            self.fields[self.focus].insert_str(text);
            self.inputs.consequences_reviewed = false;
        }
    }
    fn start(&mut self, f: impl FnOnce() -> Result<ResultMessage> + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        self.checking = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let _ = tx.send(f().map_err(|e| format!("{e:#}")));
        });
    }
    fn submit(&mut self, operation: &'static str) {
        let inputs = match self.values() {
            Ok(inputs) => inputs,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let id = self.id.clone();
        let revision = self.draft.as_ref().map(|d| d.revision);
        let original_prompt = self.fields[0].lines().join("\n");
        self.start(move || {
            let store = Store::open()?;
            let saved = store.operate(Operation {
                operation: if revision.is_some() { "edit" } else { "create" }.into(),
                id: id.clone(),
                revision,
                original_prompt: Some(original_prompt),
                inputs: Some(inputs),
                hidden: None,
            })?;
            if operation == "save" {
                return Ok(ResultMessage::Draft(Box::new(saved)));
            }
            match store.operate(Operation {
                operation: operation.into(),
                id: id.clone(),
                revision: Some(saved.revision),
                original_prompt: None,
                inputs: None,
                hidden: None,
            }) {
                Ok(draft) => Ok(ResultMessage::Draft(Box::new(draft))),
                Err(error) => {
                    let mut draft = store.get(&id)?;
                    draft.error = Some(format!("{error:#}"));
                    Ok(ResultMessage::Draft(Box::new(draft)))
                }
            }
        });
    }
    pub fn tick(&mut self, visible: bool) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(result) => {
                    self.checking = None;
                    changed = true;
                    match result {
                        Ok(ResultMessage::Draft(draft)) => self.accept(*draft),
                        Ok(ResultMessage::Reply) => self.fields[9] = TextArea::default(),
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.checking = None;
                    self.error = Some("Outcome uncertain; F10 reopens this exact request. Never start a replacement run.".into());
                    changed = true;
                }
                _ => {}
            }
        }
        if let Some(rx) = &self.output_check {
            match rx.try_recv() {
                Ok(result) => {
                    self.output = result.unwrap_or_else(|e| e);
                    self.output_check = None;
                    changed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.output_check = None,
                _ => {}
            }
        }
        if visible
            && self.output_check.is_none()
            && self.output_at.elapsed() >= Duration::from_secs(2)
        {
            if let Some(draft) = self
                .draft
                .as_ref()
                .filter(|d| d.inputs.backend == Some(Backend::Evo) && d.receipt.is_some())
                .cloned()
            {
                let (tx, rx) = mpsc::channel();
                self.output_check = Some(rx);
                self.output_at = Instant::now();
                std::thread::spawn(move || {
                    let _ = tx.send(research::output(&draft).map_err(|e| format!("{e:#}")));
                });
            }
        }
        changed
    }
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(4),
            Constraint::Length(3),
        ])
        .split(area);
        let status = self.draft.as_ref().map_or("draft".into(), |d| {
            format!("{:?} · revision {}", d.state, d.revision)
        });
        frame.render_widget(Paragraph::new(format!("Research {} · {status}\nAccount {:?} (F6) · Backend {:?} (F7 choose/override) · Reviewed [{}] (F8)", self.id, self.inputs.account, self.inputs.backend, if self.inputs.consequences_reviewed { "x" } else { " " })), rows[0]);
        let recommendation = self.draft.as_ref().map_or(
            "Develop first, then choose ORX/EVO, review and explicitly Run.".into(),
            |d| {
                format!(
                    "Recommendation {:?}: {} · Unknowns: {}",
                    d.recommendation,
                    d.reason,
                    d.unknowns.join("; ")
                )
            },
        );
        frame.render_widget(
            Paragraph::new(recommendation).wrap(Wrap { trim: true }),
            rows[1],
        );
        frame.render_widget(Paragraph::new("No connectors/crawling. Added categories: Personal Grafana/OTel/Zulip; Work Slack. Other account-appropriate excerpts use supplied; existing Work tenants are not reclassified. Scope must be explicit. EVO Run explores repo, initializes .evo, builds baseline worktree and first experiment ONLY. No optimize/ship/merge/push; later optimize needs new explicit review (budget = per-branch serial depth). ORX permits shell research, workspace setup/inspection and starter model calls. Return address is daemon-local (remote needs forwarding). Privacy/scope are instructions, not an OS sandbox.").wrap(Wrap { trim: true }).style(Style::default().fg(theme.dimmed)), rows[2]);
        self.fields[self.focus].set_block(
            Block::bordered()
                .title(format!(
                    " {}/10 {} · Tab next ",
                    self.focus + 1,
                    LABELS[self.focus]
                ))
                .border_style(Style::default().fg(theme.accent)),
        );
        if self.show_output {
            let max = self
                .output
                .lines()
                .count()
                .saturating_sub(rows[3].height.saturating_sub(2) as usize)
                .min(u16::MAX as usize) as u16;
            frame.render_widget(
                Paragraph::new(self.output.as_str())
                    .scroll((max.saturating_sub(self.output_scroll), 0))
                    .block(
                        Block::bordered()
                            .title(" Discovery / baseline output · Up/Down scroll · F11 back "),
                    ),
                rows[3],
            );
        } else {
            frame.render_widget(&self.fields[self.focus], rows[3]);
        }
        let detail = if let Some(receipt) = self.draft.as_ref().and_then(|d| d.receipt.as_ref()) {
            format!(
                "Route {} · backend {} · PID {} · {}\n{}",
                receipt.profile,
                receipt.backend_id,
                receipt.process_id,
                receipt.return_path,
                self.output
                    .lines()
                    .rev()
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        } else {
            "Original is retained separately. Edits do not run anything. Run freezes the saved revision. Interrupted/uncertain runs cannot be replayed; reopen this request with F10.".into()
        };
        frame.render_widget(Paragraph::new(detail).wrap(Wrap { trim: true }), rows[4]);
        let error = self
            .error
            .as_deref()
            .or_else(|| self.draft.as_ref().and_then(|d| d.error.as_deref()))
            .unwrap_or("");
        frame.render_widget(Paragraph::new(format!("{}\n{}", if self.checking.is_some() { "Working; no second submission" } else { "Ctrl+Alt+S save · Ctrl+Alt+D develop · Ctrl+Alt+R RUN · F9 send literal answer · F10 refresh · F11 output · F12 abandon development · F2 history" }, error)).wrap(Wrap { trim: true }).style(Style::default().fg(if error.is_empty() { theme.dimmed } else { theme.error })), rows[5]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    #[test]
    #[serial_test::serial]
    fn saved_completion_retires_only_new_slot_and_reentry_keeps_editor() {
        use crate::tui::home::tailored::{history::HistoryAction, HomeView, WindowAction};
        let root = tempfile::tempdir().unwrap();
        let _home = crate::session::test_support::isolate_app_dir_at(root.path());
        let mut view = HomeView::new_for_test(
            Some("test".into()),
            crate::tmux::AvailableTools::with_tools(&["pi"]),
            crate::file_watch::FileWatchService::noop(),
        )
        .unwrap();
        view.open_tailored();
        let overlay = view.tailored_window.as_mut().unwrap();
        overlay.picker_action(HistoryAction::Research);
        let form = overlay.research.as_mut().unwrap();
        let id = form.id.clone();
        let draft = Draft::new(id.clone(), "saved original".into(), form.inputs.clone()).unwrap();
        let (tx, rx) = mpsc::channel();
        form.checking = Some(rx);
        tx.send(Ok(ResultMessage::Draft(Box::new(draft)))).unwrap();
        assert!(form.tick(false));
        overlay.handle_key(key(KeyCode::Tab));
        overlay.handle_paste("unsaved context");
        overlay.handle_key(key(KeyCode::F(2)));
        overlay.picker_action(HistoryAction::Research);
        let fresh = overlay.research.as_ref().unwrap();
        assert_ne!(fresh.id, id);
        assert_eq!(fresh.fields[0].lines().join("\n"), "");
        overlay.handle_key(key(KeyCode::F(2)));
        view.apply_tailored_action(WindowAction::Open(id));
        let overlay = view.tailored_window.as_mut().unwrap();
        let retained = overlay.research.as_ref().unwrap();
        assert_eq!(retained.fields[1].lines().join("\n"), "unsaved context");
        overlay.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(
            overlay.research.as_ref().unwrap().fields[1]
                .lines()
                .join("\n"),
            ""
        );
    }

    #[test]
    fn repeat_press_and_paste_share_research_edit_fences() {
        use crate::tui::home::tailored::{history::HistoryAction, TailoredWindow};
        let mut overlay = TailoredWindow::new(CaptureAccount::Personal);
        overlay.picker_action(HistoryAction::Research);
        let form = overlay.research.as_mut().unwrap();
        form.accept(Draft::new(form.id.clone(), "immutable".into(), form.inputs.clone()).unwrap());
        let repeat = KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            crossterm::event::KeyEventKind::Repeat,
        );
        overlay.handle_key(repeat);
        assert_eq!(
            overlay.research.as_ref().unwrap().fields[0]
                .lines()
                .join("\n"),
            "immutable"
        );
        overlay.handle_key(key(KeyCode::Tab));
        overlay.handle_key(key(KeyCode::F(8)));
        overlay.handle_key(repeat);
        let form = overlay.research.as_ref().unwrap();
        assert_eq!(form.fields[1].lines().join("\n"), "x");
        assert!(!form.inputs.consequences_reviewed);
        overlay.handle_key(key(KeyCode::F(8)));
        overlay.handle_key(key(KeyCode::F(11)));
        overlay.handle_key(repeat);
        overlay.handle_key(key(KeyCode::Char('p')));
        overlay.handle_paste("hidden");
        let form = overlay.research.as_ref().unwrap();
        assert_eq!(form.fields[1].lines().join("\n"), "x");
        assert!(form.inputs.consequences_reviewed);
        assert!(form.checking.is_none());
    }

    #[test]
    fn multiline_paste_undo_redo_and_delete_remain_editor_commands() {
        let mut window = ResearchWindow::new(CaptureAccount::Personal, None);
        window.paste("leftRIGHT");
        window.key(key(KeyCode::Home));
        for _ in 0..4 {
            window.key(key(KeyCode::Right));
        }
        let paste = format!("🌱日本語{}\n\nlast\n", "long".repeat(500));
        window.paste(&paste);
        let exact = format!("left{paste}RIGHT");
        assert_eq!(window.fields[0].lines().join("\n"), exact);
        window.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(window.fields[0].lines().join("\n"), "leftRIGHT");
        window.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(window.fields[0].lines().join("\n"), exact);
        window.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(window.fields[0].lines().join("\n").ends_with("IGHT"));
        window.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        assert!(window.checking.is_none());
    }

    #[test]
    fn review_controls_do_not_launch_and_edits_invalidate_acknowledgement() {
        let mut window = ResearchWindow::new(CaptureAccount::Work, None);
        window.paste("original question");
        window.key(key(KeyCode::F(7)));
        assert_eq!(window.inputs.backend, Some(Backend::Orx));
        window.key(key(KeyCode::F(7)));
        assert_eq!(window.inputs.backend, Some(Backend::Evo));
        window.key(key(KeyCode::F(8)));
        assert!(window.inputs.consequences_reviewed);
        window.key(key(KeyCode::Tab));
        window.paste("supplied work context");
        assert!(!window.inputs.consequences_reviewed);
        assert!(window.checking.is_none());
        window.focus = 2;
        window.paste("slack: team channel A");
        assert_eq!(
            window.values().unwrap().sources[0].category,
            research::SourceCategory::Slack
        );
        window.key(key(KeyCode::F(6)));
        assert!(window.values().is_err());
    }
    #[test]
    fn submitted_drafts_can_be_read_but_cannot_be_edited_or_resubmitted() {
        let mut draft = Draft::new(
            uuid::Uuid::new_v4().to_string(),
            "original".into(),
            Inputs::new(CaptureAccount::Personal),
        )
        .unwrap();
        draft.state = research::State::Uncertain;
        let mut window = ResearchWindow::new(CaptureAccount::Personal, Some(draft));
        window.paste("not appended");
        assert_eq!(window.fields[0].lines().join("\n"), "original");
        window.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(window.checking.is_none());
        window.key(key(KeyCode::Tab));
        assert_eq!(window.focus, 1);
    }
}
