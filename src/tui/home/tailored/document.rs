use crate::session::{
    builder::{BuildResult, InstanceParams},
    tailored::{document::DocumentRequest, CaptureAccount, TailoredRequest},
};
use crate::tui::{dialogs::NewSessionData, styles::Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::{TextArea, WrapMode};
use std::sync::mpsc;

type Prepared = Result<PreparedDocument, String>;

pub(super) struct PreparedDocument {
    built: Option<BuildResult>,
    profile: String,
}

impl PreparedDocument {
    fn new(built: BuildResult, profile: String) -> Self {
        Self {
            built: Some(built),
            profile,
        }
    }

    pub fn accept(mut self) -> (BuildResult, String) {
        (
            self.built.take().expect("uncommitted document"),
            self.profile.clone(),
        )
    }
}

impl Drop for PreparedDocument {
    fn drop(&mut self) {
        if let Some(built) = &self.built {
            let _ = std::fs::remove_dir_all(&built.instance.project_path);
        }
    }
}

pub(super) struct StartWorker {
    pending: mpsc::Receiver<(String, Result<(), String>)>,
}

impl StartWorker {
    #[cfg(test)]
    pub(super) fn from_receiver_for_test(
        pending: mpsc::Receiver<(String, Result<(), String>)>,
    ) -> Self {
        Self { pending }
    }

    pub fn spawn(mut instance: crate::session::Instance) -> Self {
        let (tx, pending) = mpsc::channel();
        std::thread::spawn(move || {
            let result = instance
                .ensure_pane_ready()
                .map_err(|error| error.to_string())
                .and_then(|outcome| match outcome {
                    crate::session::EnsureReadyOutcome::ResumeFailed { sid } => Err(format!(
                        "Resume not confirmed for {sid}; inspect the original conversation"
                    )),
                    _ => Ok(()),
                });
            let _ = tx.send((instance.id, result));
        });
        Self { pending }
    }

    pub fn take_result(&self) -> Option<(String, Result<(), String>)> {
        match self.pending.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some((
                String::new(),
                Err("Document start worker stopped; inspect history before retrying".into()),
            )),
        }
    }
}

pub(super) struct DocumentWindow {
    url: TextArea<'static>,
    question: TextArea<'static>,
    pub(super) account: CaptureAccount,
    pub(super) focus: usize,
    error: Option<String>,
    pending: Option<mpsc::Receiver<Prepared>>,
    start_rect: Rect,
    pub(super) account_rect: Rect,
    url_rect: Rect,
    question_rect: Rect,
}

impl DocumentWindow {
    pub fn new(account: CaptureAccount) -> Self {
        let mut url = TextArea::default();
        url.set_wrap_mode(WrapMode::WordOrGlyph);
        let mut question = TextArea::default();
        question.set_wrap_mode(WrapMode::WordOrGlyph);
        Self {
            url,
            question,
            account,
            focus: 0,
            error: None,
            pending: None,
            start_rect: Rect::default(),
            account_rect: Rect::default(),
            url_rect: Rect::default(),
            question_rect: Rect::default(),
        }
    }
    #[cfg(test)]
    pub(super) fn prepared_for_test(built: BuildResult, profile: String) -> Self {
        let mut window = Self::new(CaptureAccount::Work);
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(PreparedDocument::new(built, profile)))
            .ok()
            .unwrap();
        window.pending = Some(rx);
        window
    }
    #[cfg(test)]
    pub(super) fn url_text_for_test(&self) -> String {
        self.url.lines().join("\n")
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn paste(&mut self, text: &str) {
        if self.busy() {
            return;
        }
        match self.focus {
            0 => {
                self.url.insert_str(text);
            }
            1 => {
                self.question.insert_str(text);
            }
            _ => {}
        }
    }
    pub(super) fn repeat(&mut self, key: KeyEvent) {
        if !self.busy()
            && !(key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            match self.focus {
                0 => {
                    self.url.input(key);
                }
                1 => {
                    self.question.input(key);
                }
                _ => {}
            }
        }
    }
    pub fn key(&mut self, key: KeyEvent, profile: &str) {
        if self.busy() {
            return;
        }
        match key.code {
            KeyCode::Tab => self.focus = (self.focus + 1) % 4,
            KeyCode::BackTab => self.focus = (self.focus + 3) % 4,
            KeyCode::Enter
                if key.modifiers == KeyModifiers::CONTROL
                    || self.focus == 3 && key.modifiers.is_empty() =>
            {
                self.start(profile)
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
                if self.focus == 2 && key.modifiers.is_empty() =>
            {
                self.account = match self.account {
                    CaptureAccount::Personal => CaptureAccount::Work,
                    CaptureAccount::Work => CaptureAccount::Personal,
                }
            }
            _ if self.focus == 0 => {
                self.url.input(key);
            }
            _ if self.focus == 1 => {
                self.question.input(key);
            }
            _ => {}
        }
    }
    pub fn click(&mut self, col: u16, row: u16, profile: &str) {
        if self.busy() {
            return;
        }
        let point = Position::new(col, row);
        if self.start_rect.contains(point) {
            self.focus = 3;
            self.start(profile);
        } else if self.account_rect.contains(point) {
            self.focus = 2;
            self.key(KeyEvent::from(KeyCode::Enter), profile);
        } else if self.url_rect.contains(point) {
            self.focus = 0;
        } else if self.question_rect.contains(point) {
            self.focus = 1;
        }
    }
    fn start(&mut self, profile: &str) {
        let document = DocumentRequest {
            url: self.url.lines().join("\n"),
            opening_question: self.question.lines().join("\n"),
        };
        let request = TailoredRequest {
            preset: "document-chat".into(),
            account: self.account,
            idea: String::new(),
            immediately: false,
            document: Some(document.clone()),
        };
        let launch = match request.launch() {
            Ok(launch) => launch,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let profile = profile.to_owned();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<_> {
                request.verify_profile()?;
                let params = InstanceParams::from(NewSessionData {
                    profile: profile.clone(),
                    title: format!("Document chat {}", &uuid::Uuid::new_v4().to_string()[..8]),
                    title_typed: false,
                    path: String::new(),
                    group: "Tailored".into(),
                    tool: "pi".into(),
                    worktree_enabled: false,
                    worktree_branch: None,
                    create_new_branch: false,
                    base_branch: None,
                    extra_repo_paths: Vec::new(),
                    sandbox: false,
                    sandbox_image: String::new(),
                    yolo_mode: false,
                    extra_env: Vec::new(),
                    extra_args: launch.extra_args,
                    command_override: launch.command,
                    scratch: true,
                    fork_seed: None,
                    structured: false,
                });
                let built = crate::session::tailored::document::build(
                    params,
                    &[],
                    &[],
                    &profile,
                    &document,
                )?;
                Ok(PreparedDocument::new(built, profile))
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        });
    }
    pub fn take_result(&mut self) -> Option<Prepared> {
        let result = match self.pending.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Document preparation stopped unexpectedly".into())
            }
        };
        self.pending = None;
        Some(result)
    }
    pub fn error(&mut self, error: String) {
        self.error = Some(error);
    }
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(area);
        frame.render_widget(Paragraph::new("Document chat · public direct Markdown/PDF\nDownload and extract before starting a tool-free Pi conversation."), rows[0]);
        self.url_rect = rows[1];
        self.question_rect = rows[2];
        self.account_rect = rows[3];
        self.start_rect = rows[5];
        for (index, (input, rect, title)) in [
            (&mut self.url, rows[1], " Public URL "),
            (&mut self.question, rows[2], " Opening question (optional) "),
        ]
        .into_iter()
        .enumerate()
        {
            input.set_block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::default().fg(if self.focus == index {
                        theme.accent
                    } else {
                        theme.dimmed
                    })),
            );
            frame.render_widget(&*input, rect);
        }
        frame.render_widget(
            Paragraph::new(format!(
                "Account: {:?} · Tab to select, Space to change",
                self.account
            ))
            .style(Style::default().fg(if self.focus == 2 {
                theme.accent
            } else {
                theme.text
            })),
            rows[3],
        );
        frame.render_widget(Paragraph::new(self.error.as_deref().unwrap_or("No credentials, redirects, OCR or remote extraction. 10 MiB download / 256 KiB text.")).wrap(Wrap { trim: false }), rows[4]);
        frame.render_widget(
            Paragraph::new(if self.busy() {
                "Preparing source; no model started…"
            } else {
                "[ Start ] · Tab to Start, Enter to confirm · F2 history"
            })
            .style(Style::default().fg(if self.focus == 3 {
                theme.accent
            } else {
                theme.dimmed
            })),
            rows[5],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_document_result_drop_cleans_only_uncommitted_preparation() {
        for accept in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let scratch = root.path().join("prepared");
            std::fs::create_dir(&scratch).unwrap();
            let built = BuildResult {
                instance: crate::session::Instance::new("Document", scratch.to_str().unwrap()),
                created_worktree: None,
                created_workspace_worktrees: vec![],
                warnings: vec![],
            };
            let (tx, rx) = mpsc::channel();
            tx.send(PreparedDocument::new(built, "test".into()))
                .ok()
                .unwrap();
            if accept {
                let (_built, _profile) = rx.recv().unwrap().accept();
                // Acceptance may have committed even when its caller later reports failure.
            }
            drop(rx);
            assert_eq!(scratch.exists(), accept);
        }
    }

    #[test]
    fn wrapped_document_input_preserves_exact_bytes_and_enter_does_not_start() {
        let mut window = DocumentWindow::new(CaptureAccount::Personal);
        let url = format!("https://example.org/{}", "long".repeat(80));
        window.paste(&url);
        for width in [35, 70, 25] {
            let backend = ratatui::backend::TestBackend::new(width, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal
                .draw(|f| window.render(f, f.area(), &Theme::default()))
                .unwrap();
            assert_eq!(window.url.lines().join("\n"), url);
        }
        window.key(KeyEvent::from(KeyCode::Enter), "default");
        assert!(!window.busy());
        window.key(KeyEvent::from(KeyCode::Tab), "default");
        window.paste("leftRIGHT");
        window.key(KeyEvent::from(KeyCode::Home), "default");
        for _ in 0..4 {
            window.key(KeyEvent::from(KeyCode::Right), "default");
        }
        let paste = format!("🌱日本語{}\n\nlast\n", "long".repeat(500));
        window.paste(&paste);
        let exact = format!("left{paste}RIGHT");
        assert_eq!(window.question.lines().join("\n"), exact);
        window.key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            "default",
        );
        assert_eq!(window.question.lines().join("\n"), "leftRIGHT");
        window.key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            "default",
        );
        assert_eq!(window.question.lines().join("\n"), exact);
        assert!(!window.busy());
    }
}
