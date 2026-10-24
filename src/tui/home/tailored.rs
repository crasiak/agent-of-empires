mod history;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use history::{HistoryAction, HistoryPicker};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::TextArea;

use super::{live_send, HomeView};
use crate::session::tailored::{CaptureAccount, TailoredRequest};
use crate::tui::styles::Theme;

pub(in crate::tui) struct TailoredWindow {
    picker: HistoryPicker,
    history_refreshed: Option<Instant>,
    draft_committed: bool,
    composing: bool,
    idea: TextArea<'static>,
    account: CaptureAccount,
    immediately: bool,
    focus: usize,
    error: Option<String>,
    checking: Option<mpsc::Receiver<Result<TailoredRequest, String>>>,
    terminal: Option<CaptureTerminal>,
    account_rect: Rect,
    immediately_rect: Rect,
    submit_rect: Rect,
}

pub(super) enum WindowAction {
    Continue,
    Close,
    Open(String),
    Visibility { id: String, hidden: bool },
}

struct CaptureTerminal {
    session_id: String,
    title: String,
    resize_retry_at: Option<std::time::Instant>,
    send: live_send::LiveSendWorker,
    capture: live_send::LiveCaptureWorker,
    text: Text<'static>,
    cursor: Option<crate::tmux::PaneCursor>,
    size: (u16, u16),
}

impl TailoredWindow {
    fn new(account: CaptureAccount) -> Self {
        Self {
            picker: HistoryPicker::default(),
            history_refreshed: None,
            draft_committed: false,
            composing: false,
            idea: TextArea::default(),
            account,
            immediately: false,
            focus: 0,
            error: None,
            checking: None,
            terminal: None,
            account_rect: Rect::default(),
            immediately_rect: Rect::default(),
            submit_rect: Rect::default(),
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> WindowAction {
        if super::bindings::is_tailored_shortcut(&key)
            || (key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('q'))
        {
            return if self.checking.is_none() {
                WindowAction::Close
            } else {
                WindowAction::Continue
            };
        }
        if key.code == KeyCode::F(2) && self.checking.is_none() {
            self.terminal = None;
            self.composing = false;
            self.picker.recent();
            self.history_refreshed = None;
            self.error = None;
            return WindowAction::Continue;
        }
        if let Some(terminal) = &self.terminal {
            if let live_send::LiveDispatch::Send(key) = live_send::translate(key) {
                if !terminal.send.lock_lost() {
                    terminal.send.send(key);
                }
            }
            return WindowAction::Continue;
        }
        if self.checking.is_some() {
            return WindowAction::Continue;
        }
        if key.code == KeyCode::Esc {
            return WindowAction::Close;
        }
        if !self.composing {
            let action = self.picker.handle_key(key);
            return self.picker_action(action);
        }
        match key.code {
            KeyCode::Tab => self.focus = (self.focus + 1) % 4,
            KeyCode::BackTab => self.focus = (self.focus + 3) % 4,
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) && self.focus == 0 => {
                self.idea.insert_newline();
            }
            KeyCode::Enter
                if self.focus == 0
                    || self.focus == 3
                    || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.submit()
            }
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right | KeyCode::Enter
                if self.focus == 1 =>
            {
                self.toggle_account()
            }
            KeyCode::Char(' ') | KeyCode::Enter if self.focus == 2 => {
                self.immediately = !self.immediately
            }
            _ if self.focus == 0 => {
                self.idea.input(key);
            }
            _ => {}
        }
        WindowAction::Continue
    }

    fn picker_action(&mut self, action: HistoryAction) -> WindowAction {
        match action {
            HistoryAction::None => WindowAction::Continue,
            HistoryAction::New => {
                if self.draft_committed {
                    self.idea = TextArea::default();
                    self.immediately = false;
                    self.draft_committed = false;
                }
                self.composing = true;
                self.focus = 0;
                self.error = None;
                WindowAction::Continue
            }
            HistoryAction::Open(id) => WindowAction::Open(id),
            HistoryAction::Visibility { id, hidden } => WindowAction::Visibility { id, hidden },
        }
    }

    pub(super) fn handle_paste(&mut self, text: &str) {
        if let Some(terminal) = &self.terminal {
            if !terminal.send.lock_lost() {
                terminal
                    .send
                    .send(live_send::TmuxKey::Paste(text.to_string()));
            }
        } else if self.checking.is_none() {
            if self.composing && self.focus == 0 {
                self.idea.insert_str(text);
            } else if !self.composing {
                self.picker.handle_paste(text);
            }
        }
    }

    pub(super) fn handle_click(&mut self, col: u16, row: u16) -> WindowAction {
        if self.checking.is_some() || self.terminal.is_some() {
            return WindowAction::Continue;
        }
        if !self.composing {
            let action = self.picker.handle_click(col, row);
            return self.picker_action(action);
        }
        let pos = Position::new(col, row);
        if self.account_rect.contains(pos) {
            self.focus = 1;
            self.toggle_account();
        } else if self.immediately_rect.contains(pos) {
            self.focus = 2;
            self.immediately = !self.immediately;
        } else if self.submit_rect.contains(pos) {
            self.submit();
        } else {
            self.focus = 0;
        }
        WindowAction::Continue
    }

    fn toggle_account(&mut self) {
        self.account = match self.account {
            CaptureAccount::Personal => CaptureAccount::Work,
            CaptureAccount::Work => CaptureAccount::Personal,
        };
    }

    fn submit(&mut self) {
        let request = TailoredRequest {
            preset: "capture-idea".into(),
            account: self.account,
            idea: self.idea.lines().join("\n"),
            immediately: self.immediately,
        };
        if let Err(error) = request.launch() {
            self.error = Some(error.to_string());
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.checking = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let result = request
                .verify_profile()
                .map(|()| request)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        });
    }

    pub(super) fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let width = area
            .width
            .saturating_sub(4)
            .min(if self.terminal.is_some() { 140 } else { 84 });
        let height = area
            .height
            .saturating_sub(4)
            .min(if self.terminal.is_some() {
                48
            } else if self.composing {
                20
            } else {
                28
            });
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(" Tailored Sessions ")
            .border_style(Style::default().fg(theme.accent))
            .style(Style::default().bg(theme.background).fg(theme.text))
            .padding(Padding::horizontal(1));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        self.account_rect = Rect::default();
        self.immediately_rect = Rect::default();
        self.submit_rect = Rect::default();
        if let Some(terminal) = &mut self.terminal {
            let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
            let output = rows[0];
            let size = (output.width.max(1), output.height.max(1));
            let retry = super::render::live_resize_retry_due(
                &mut terminal.resize_retry_at,
                terminal.send.take_resize_failed(),
                std::time::Instant::now(),
            );
            if terminal.size != size || retry {
                terminal.size = size;
                terminal.send.resize(size.0, size.1);
            }
            terminal.capture.set_capture_lines(size.1 as usize);
            if let Some(captured) = terminal.capture.take_latest() {
                if terminal.capture.frame_is_current(&captured) {
                    terminal.text =
                        crate::tui::components::preview::parse_output_text(&captured.content);
                    terminal.cursor = captured.cursor;
                }
            }
            let scroll = terminal
                .text
                .lines
                .len()
                .saturating_sub(output.height as usize)
                .min(u16::MAX as usize) as u16;
            frame.render_widget(
                Paragraph::new(terminal.text.clone()).scroll((scroll, 0)),
                output,
            );
            if let Some(cursor) = terminal.cursor.filter(|cursor| {
                cursor.visible
                    && cursor.position_reliable
                    && cursor.x < output.width
                    && cursor.y < output.height
            }) {
                frame.set_cursor_position((output.x + cursor.x, output.y + cursor.y));
            }
            let hint = if terminal.send.lock_lost() {
                "Input paused: this Session is active elsewhere. Ctrl+Y closes."
            } else {
                "F2 history · Ctrl+Y close · Session keeps running · Esc goes to agent"
            };
            frame.render_widget(
                Paragraph::new(hint).style(Style::default().fg(theme.dimmed)),
                rows[1],
            );
            return;
        }
        if !self.composing {
            self.picker
                .render(frame, inner, theme, self.error.as_deref());
            return;
        }
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);
        self.account_rect = rows[0];
        self.immediately_rect = rows[1];
        self.submit_rect = rows[5];
        let style = |focused| Style::default().fg(if focused { theme.accent } else { theme.text });
        frame.render_widget(
            Paragraph::new(format!(
                "Account: {}  (Space to switch)",
                match self.account {
                    CaptureAccount::Personal => "Personal",
                    CaptureAccount::Work => "Work",
                }
            ))
            .style(style(self.focus == 1)),
            rows[0],
        );
        frame.render_widget(
            Paragraph::new(format!(
                "[{}] Immediately apply inferred ticket / Atlas / KB changes",
                if self.immediately { "x" } else { " " }
            ))
            .style(style(self.focus == 2)),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new("Unchecked: save idea, then review destinations before writing.")
                .style(Style::default().fg(theme.dimmed)),
            rows[2],
        );
        self.idea.set_block(
            Block::bordered()
                .title(" Idea ")
                .border_style(style(self.focus == 0)),
        );
        self.idea.set_cursor_line_style(Style::default());
        self.idea.set_style(Style::default().fg(theme.text));
        frame.render_widget(&self.idea, rows[3]);
        if let Some(error) = &self.error {
            frame.render_widget(
                Paragraph::new(error.as_str())
                    .wrap(Wrap { trim: true })
                    .style(Style::default().fg(theme.error)),
                rows[4],
            );
        }
        frame.render_widget(
            Paragraph::new(if self.checking.is_some() {
                "Checking minimal Ledger profile…"
            } else {
                "Enter capture · Shift+Enter newline · Tab controls · F2 history · Esc close"
            })
            .style(style(self.focus == 3)),
            rows[5],
        );
    }
}

fn capture_target_available(instance: Option<&crate::session::Instance>, title: &str) -> bool {
    instance.is_some_and(|instance| {
        instance.title == title
            && !instance.is_trashed()
            && !instance.is_archived()
            && !matches!(
                instance.status,
                crate::session::Status::Deleting | crate::session::Status::Stopped
            )
    })
}

impl HomeView {
    pub(in crate::tui) fn routes_tailored_key(&self, key: &KeyEvent) -> bool {
        self.tailored_window.is_some()
            || (super::bindings::is_tailored_shortcut(key) && !self.has_non_live_send_overlay())
    }

    pub(super) fn open_tailored(&mut self) {
        self.tailored_window = Some(TailoredWindow::new(self.tailored_account));
        self.refresh_tailored_history();
    }

    pub(super) fn handle_tailored_key(&mut self, key: KeyEvent) {
        if let Some(window) = &mut self.tailored_window {
            let action = window.handle_key(key);
            self.apply_tailored_action(action);
        }
    }

    pub(super) fn handle_tailored_click(&mut self, col: u16, row: u16) {
        if let Some(window) = &mut self.tailored_window {
            let action = window.handle_click(col, row);
            self.apply_tailored_action(action);
        }
    }

    fn apply_tailored_action(&mut self, action: WindowAction) {
        match action {
            WindowAction::Continue => {}
            WindowAction::Close => self.tailored_window = None,
            WindowAction::Open(id) => {
                let result = self.open_capture_terminal(&id);
                if let Some(window) = &mut self.tailored_window {
                    match result {
                        Ok(terminal) => {
                            window.terminal = Some(terminal);
                            window.composing = false;
                            window.error = None;
                        }
                        Err(error) => window.error = Some(format!("{error:#}")),
                    }
                }
            }
            WindowAction::Visibility { id, hidden } => {
                let result = self
                    .get_instance(&id)
                    .filter(|instance| {
                        crate::session::tailored::history::capture_account(instance).is_some()
                    })
                    .ok_or_else(|| anyhow::anyhow!("Capture Session disappeared"))
                    .and_then(|instance| {
                        crate::session::tailored::history::set_hidden(
                            &instance.effective_profile(),
                            &id,
                            hidden,
                        )
                    });
                if let Some(window) = &mut self.tailored_window {
                    window.error = result.err().map(|error| format!("{error:#}"));
                }
                self.refresh_tailored_history();
            }
        }
    }

    fn refresh_tailored_history(&mut self) -> bool {
        let result = crate::session::tailored::history::load_hidden()
            .map(|hidden| crate::session::tailored::history::entries(self.instances(), &hidden));
        let Some(window) = &mut self.tailored_window else {
            return false;
        };
        window.history_refreshed = Some(Instant::now());
        match result {
            Ok(rows) => window.picker.replace_rows(rows),
            Err(error) => {
                let message = format!("Cannot read capture history: {error:#}");
                let changed = window.error.as_deref() != Some(&message);
                window.error = Some(message);
                changed
            }
        }
    }

    pub(super) fn tick_tailored(&mut self) -> bool {
        let refresh = self.tailored_window.as_ref().is_some_and(|window| {
            window
                .history_refreshed
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(2))
        });
        let refreshed = refresh && self.refresh_tailored_history();
        let changed_target = self
            .tailored_window
            .as_ref()
            .and_then(|window| window.terminal.as_ref())
            .is_some_and(|terminal| {
                !capture_target_available(self.get_instance(&terminal.session_id), &terminal.title)
            });
        if changed_target {
            if let Some(window) = &mut self.tailored_window {
                window.terminal = None;
                window.composing = false;
                window.error =
                    Some("Capture Session changed or stopped; select it again from history".into());
                window.history_refreshed = None;
            }
            return true;
        }
        let result = self
            .tailored_window
            .as_ref()
            .and_then(|window| window.checking.as_ref())
            .map(|rx| rx.try_recv());
        let Some(result) = result else {
            return refreshed;
        };
        let request = match result {
            Ok(Ok(request)) => request,
            Ok(Err(error)) => {
                if let Some(window) = &mut self.tailored_window {
                    window.checking = None;
                    window.error = Some(error);
                }
                return true;
            }
            Err(mpsc::TryRecvError::Empty) => return refreshed,
            Err(mpsc::TryRecvError::Disconnected) => {
                if let Some(window) = &mut self.tailored_window {
                    window.checking = None;
                    window.error = Some("Ledger profile check stopped unexpectedly".into());
                }
                return true;
            }
        };
        self.tailored_account = request.account;
        let previous = self.selected_session.clone();
        let result = self.launch_capture(request);
        if let Some(id) = previous {
            self.select_and_reveal_session(&id);
        }
        if let Some(window) = &mut self.tailored_window {
            window.checking = None;
            match result {
                Ok(terminal) => {
                    window.terminal = Some(terminal);
                    window.draft_committed = true;
                }
                Err(error) => window.error = Some(format!("{error:#}")),
            }
        }
        true
    }

    fn launch_capture(&mut self, request: TailoredRequest) -> anyhow::Result<CaptureTerminal> {
        let launch = request.launch()?;
        let id = self.create_session(crate::tui::dialogs::NewSessionData {
            profile: self.config_profile(),
            title: "Capture an idea".into(),
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
        })?;
        self.open_capture_terminal(&id)
    }

    fn open_capture_terminal(&mut self, id: &str) -> anyhow::Result<CaptureTerminal> {
        anyhow::ensure!(
            self.get_instance(id).is_some_and(|instance| {
                crate::session::tailored::history::capture_account(instance).is_some()
            }),
            "This is not a capture Session"
        );
        self.try_mutate_instance_writeback_on_err(id, |instance| {
            instance.ensure_pane_ready().map_err(Into::into)
        })?;
        let instance = self
            .get_instance(id)
            .ok_or_else(|| anyhow::anyhow!("Capture Session disappeared"))?;
        let name = instance.tmux_session()?.name().to_string();
        let title = instance.title.clone();
        let capture = live_send::LiveCaptureWorker::spawn(self.preview_wake.clone());
        capture.set_vt_enabled(self.vt_live_enabled);
        capture.set_target(name.clone());
        capture.set_live(true);
        capture.set_capture_lines(40);
        let send = live_send::LiveSendWorker::spawn(name, Some(capture.waker()));
        Ok(CaptureTerminal {
            session_id: id.to_owned(),
            title,
            resize_retry_at: None,
            send,
            capture,
            text: Text::default(),
            cursor: None,
            size: (0, 0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn changed_or_shelved_capture_targets_are_not_kept_interactive() {
        let mut instance = crate::session::Instance::new("Capture", "/tmp/capture");
        instance.status = crate::session::Status::Running;
        assert!(capture_target_available(Some(&instance), "Capture"));
        assert!(!capture_target_available(None, "Capture"));
        assert!(!capture_target_available(Some(&instance), "Previous title"));
        instance.trashed_at = Some(chrono::Utc::now());
        assert!(!capture_target_available(Some(&instance), "Capture"));
    }

    #[test]
    fn picker_filters_and_capture_controls_do_not_launch_blank_ideas() {
        let mut window = TailoredWindow::new(CaptureAccount::Personal);
        window.handle_paste("shopping");
        window.handle_key(key(KeyCode::Enter));
        assert!(!window.composing);
        let mut window = TailoredWindow::new(CaptureAccount::Personal);
        window.handle_paste("idea");
        window.handle_key(key(KeyCode::Enter));
        assert!(window.composing);
        window.handle_key(key(KeyCode::Enter));
        assert!(window.error.is_some());
        assert!(window.checking.is_none());
        window.handle_key(key(KeyCode::Tab));
        window.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(window.account, CaptureAccount::Work);
        window.handle_key(key(KeyCode::Tab));
        window.handle_key(key(KeyCode::Char(' ')));
        assert!(window.immediately);
        assert!(matches!(
            window.handle_key(key(KeyCode::Esc)),
            WindowAction::Close
        ));
    }
}
