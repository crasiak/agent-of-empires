mod document;
mod history;
mod research;
mod workshop;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use history::{HistoryAction, HistoryPicker};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::{TextArea, WrapMode};

use super::{live_send, HomeView};
use crate::session::tailored::{CaptureAccount, TailoredRequest};
use crate::tui::styles::Theme;

pub(in crate::tui) struct TailoredWindow {
    picker: HistoryPicker,
    research: Option<research::ResearchWindow>,
    workshop: Option<workshop::WorkshopWindow>,
    document: Option<document::DocumentWindow>,
    starting_document: Option<document::StartWorker>,
    #[cfg(test)]
    document_starter: Option<Box<dyn Fn(crate::session::Instance) -> document::StartWorker + Send>>,
    starting_document_foreground: bool,
    research_drafts: Vec<research::ResearchWindow>,
    workshop_drafts: Vec<workshop::WorkshopWindow>,
    document_drafts: Vec<document::DocumentWindow>,
    other_capture: Option<(TextArea<'static>, bool, bool)>,
    research_account: CaptureAccount,
    workshop_account: CaptureAccount,
    document_account: CaptureAccount,
    hidden: bool,
    suspended_terminal: Option<String>,
    profile: String,
    workshop_history: Option<(
        u64,
        mpsc::Receiver<Result<Vec<crate::session::tailored::history::HistoryEntry>, String>>,
    )>,
    workshop_history_generation: u64,
    workshop_visibility: Option<mpsc::Receiver<Result<(), String>>>,
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

pub(super) fn load_workshop_history(
) -> Result<Vec<crate::session::tailored::history::HistoryEntry>, String> {
    crate::session::tailored::workshop::Store::open()
        .and_then(|store| store.list())
        .map(|requests| requests.iter().map(|r| r.history()).collect())
        .map_err(|e| format!("{e:#}"))
}

fn other_account(account: CaptureAccount) -> CaptureAccount {
    match account {
        CaptureAccount::Personal => CaptureAccount::Work,
        CaptureAccount::Work => CaptureAccount::Personal,
    }
}

fn idea_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    textarea
}

impl TailoredWindow {
    pub(super) fn new(account: CaptureAccount) -> Self {
        Self {
            picker: HistoryPicker::default(),
            research: None,
            workshop: None,
            document: None,
            starting_document: None,
            #[cfg(test)]
            document_starter: None,
            starting_document_foreground: false,
            research_drafts: Vec::new(),
            workshop_drafts: Vec::new(),
            document_drafts: Vec::new(),
            other_capture: None,
            research_account: account,
            workshop_account: account,
            document_account: account,
            hidden: false,
            suspended_terminal: None,
            profile: String::new(),
            workshop_history: None,
            workshop_history_generation: 0,
            workshop_visibility: None,
            history_refreshed: None,
            draft_committed: false,
            composing: false,
            idea: idea_textarea(),
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

    #[cfg(test)]
    pub(super) fn set_history_worker_for_test(
        &mut self,
        receiver: mpsc::Receiver<
            Result<Vec<crate::session::tailored::history::HistoryEntry>, String>,
        >,
    ) {
        self.workshop_history = Some((self.workshop_history_generation, receiver));
        self.history_refreshed = Some(Instant::now());
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> WindowAction {
        if key.kind == KeyEventKind::Release || self.hidden {
            return WindowAction::Continue;
        }
        let editing = self.terminal.is_none()
            && (self.document.is_some()
                || self.workshop.is_some()
                || self.research.is_some()
                || self.composing && self.focus == 0);
        if key.kind == KeyEventKind::Repeat {
            if let Some(terminal) = &self.terminal {
                if let live_send::LiveDispatch::Send(key) = live_send::translate(key) {
                    if !terminal.send.lock_lost() {
                        terminal.send.send(key);
                    }
                }
                return WindowAction::Continue;
            }
            if editing
                && key.code != KeyCode::Tab
                && key.code != KeyCode::BackTab
                && !matches!(key.code, KeyCode::F(_) | KeyCode::Esc)
                && (key.modifiers.is_empty()
                    || key.modifiers == KeyModifiers::SHIFT
                    || key.modifiers == KeyModifiers::CONTROL)
            {
                if let Some(form) = &mut self.document {
                    form.repeat(key);
                } else if let Some(form) = &mut self.workshop {
                    form.repeat(key);
                } else if let Some(form) = &mut self.research {
                    form.repeat(key);
                } else if self.focus == 0 && self.checking.is_none() {
                    self.idea.input(key);
                }
            }
            return WindowAction::Continue;
        }
        if (super::bindings::is_tailored_shortcut(&key) && !editing)
            || (key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('q'))
        {
            return WindowAction::Close;
        }
        if key.code == KeyCode::F(2) && self.checking.is_none() {
            self.terminal = None;
            self.starting_document_foreground = false;
            self.park_forms();
            self.composing = false;
            self.picker.recent();
            self.history_refreshed = None;
            self.error = None;
            return WindowAction::Continue;
        }
        if key.modifiers.is_empty()
            && self
                .document
                .as_ref()
                .is_some_and(|d| !d.busy() && d.focus == 2)
            && matches!(
                key.code,
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            )
        {
            let account = self.document.as_ref().unwrap().account;
            self.park_forms();
            self.open_document(other_account(account));
            return WindowAction::Continue;
        }
        if key.code == KeyCode::F(6) {
            if self
                .research
                .as_ref()
                .is_some_and(|r| r.can_switch_account())
            {
                let account = self.research.as_ref().unwrap().account();
                self.park_forms();
                self.open_research(other_account(account));
                return WindowAction::Continue;
            }
            if self
                .workshop
                .as_ref()
                .is_some_and(|w| w.can_switch_account())
            {
                let account = self.workshop.as_ref().unwrap().account();
                self.park_forms();
                self.open_workshop(other_account(account));
                return WindowAction::Continue;
            }
        }
        if let Some(document) = &mut self.document {
            if key.code == KeyCode::Esc {
                return WindowAction::Close;
            }
            document.key(key, &self.profile);
            return WindowAction::Continue;
        }
        if let Some(workshop) = &mut self.workshop {
            if key.code == KeyCode::Esc {
                return WindowAction::Close;
            }
            workshop.key(key);
            return WindowAction::Continue;
        }
        if let Some(research) = &mut self.research {
            if key.code == KeyCode::Esc {
                return WindowAction::Close;
            }
            research.key(key);
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
            KeyCode::Enter if key.modifiers == KeyModifiers::CONTROL => self.submit(),
            KeyCode::Enter
                if self.focus == 0
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.idea.insert_newline();
            }
            KeyCode::Enter if self.focus == 3 && key.modifiers.is_empty() => self.submit(),
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
            HistoryAction::Document => {
                self.open_document(self.document_account);
                WindowAction::Continue
            }
            HistoryAction::Workshop => {
                self.open_workshop(self.workshop_account);
                WindowAction::Continue
            }
            HistoryAction::Research => {
                self.open_research(self.research_account);
                WindowAction::Continue
            }
            HistoryAction::New => {
                if self.draft_committed {
                    self.idea = idea_textarea();
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
        if self.hidden {
            return;
        }
        if let Some(workshop) = &mut self.workshop {
            workshop.paste(text);
            return;
        }
        if let Some(document) = &mut self.document {
            document.paste(text);
            return;
        }
        if let Some(research) = &mut self.research {
            research.paste(text);
            return;
        }
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
        if self.hidden {
            return WindowAction::Continue;
        }
        if self
            .document
            .as_ref()
            .is_some_and(|d| !d.busy() && d.account_rect.contains(Position::new(col, row)))
        {
            let account = self.document.as_ref().unwrap().account;
            self.park_forms();
            self.open_document(other_account(account));
            return WindowAction::Continue;
        }
        if let Some(document) = &mut self.document {
            document.click(col, row, &self.profile);
            return WindowAction::Continue;
        }
        if self.checking.is_some()
            || self.terminal.is_some()
            || self.research.is_some()
            || self.workshop.is_some()
        {
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
        let (idea, immediately, committed) = self
            .other_capture
            .take()
            .unwrap_or_else(|| (idea_textarea(), false, false));
        self.other_capture = Some((
            std::mem::replace(&mut self.idea, idea),
            std::mem::replace(&mut self.immediately, immediately),
            std::mem::replace(&mut self.draft_committed, committed),
        ));
        self.account = other_account(self.account);
        self.error = None;
    }

    fn park_forms(&mut self) {
        if let Some(form) = self.research.take() {
            self.research_drafts.push(form);
        }
        if let Some(form) = self.workshop.take() {
            self.workshop_drafts.push(form);
        }
        if let Some(form) = self.document.take() {
            self.document_drafts.push(form);
        }
    }
    fn open_document(&mut self, account: CaptureAccount) {
        self.document_account = account;
        self.document = Some(
            self.document_drafts
                .iter()
                .position(|d| d.account == account)
                .map(|i| self.document_drafts.remove(i))
                .unwrap_or_else(|| document::DocumentWindow::new(account)),
        );
    }
    fn open_research(&mut self, account: CaptureAccount) {
        self.research_account = account;
        self.research = Some(
            self.research_drafts
                .iter()
                .position(|d| d.new_draft && d.account() == account)
                .map(|i| self.research_drafts.remove(i))
                .unwrap_or_else(|| research::ResearchWindow::new(account, None)),
        );
    }
    fn open_workshop(&mut self, account: CaptureAccount) {
        self.workshop_account = account;
        self.workshop = Some(
            self.workshop_drafts
                .iter()
                .position(|d| d.new_draft && d.account() == account)
                .map(|i| self.workshop_drafts.remove(i))
                .unwrap_or_else(|| workshop::WorkshopWindow::new(account, None)),
        );
    }

    fn submit(&mut self) {
        let request = TailoredRequest {
            preset: "capture-idea".into(),
            account: self.account,
            idea: self.idea.lines().join("\n"),
            immediately: self.immediately,
            document: None,
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
        let width = area.width.saturating_sub(4).min(
            if self.terminal.is_some() || self.research.is_some() || self.workshop.is_some() {
                140
            } else {
                84
            },
        );
        let height = area.height.saturating_sub(4).min(
            if self.terminal.is_some() || self.research.is_some() || self.workshop.is_some() {
                48
            } else if self.composing || self.document.is_some() {
                20
            } else {
                28
            },
        );
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
        if let Some(document) = &mut self.document {
            document.render(frame, inner, theme);
            return;
        }
        if let Some(workshop) = &mut self.workshop {
            workshop.render(frame, inner, theme);
            return;
        }
        if let Some(research) = &mut self.research {
            research.render(frame, inner, theme);
            return;
        }
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
                "Enter newline · Ctrl+Enter capture · Tab controls · F2 history · Esc/Ctrl+Q close"
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
        if self.tailored_window.is_some() {
            return;
        }
        let profile = self.config_profile();
        let mut window = self
            .retained_tailored_windows
            .iter()
            .position(|window| window.profile == profile)
            .map(|index| self.retained_tailored_windows.remove(index))
            .unwrap_or_else(|| TailoredWindow::new(self.tailored_account));
        window.hidden = false;
        window.profile = profile;
        let terminal = window.suspended_terminal.take();
        self.tailored_window = Some(window);
        if let Some(id) = terminal {
            self.apply_tailored_action(WindowAction::Open(id));
        }
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

    pub(super) fn close_tailored(&mut self) {
        self.apply_tailored_action(WindowAction::Close);
    }

    fn apply_tailored_action(&mut self, action: WindowAction) {
        match action {
            WindowAction::Continue => {}
            WindowAction::Close => {
                if let Some(mut window) = self.tailored_window.take() {
                    window.hidden = true;
                    if let Some(terminal) = window.terminal.take() {
                        window.suspended_terminal = Some(terminal.session_id);
                    }
                    self.retained_tailored_windows.push(window);
                }
            }
            WindowAction::Open(id) => {
                if let Some(window) = &mut self.tailored_window {
                    if let Some(index) = window.workshop_drafts.iter().position(|d| d.id == id) {
                        window.workshop = Some(window.workshop_drafts.remove(index));
                        return;
                    }
                    if let Some(index) = window.research_drafts.iter().position(|d| d.id == id) {
                        window.research = Some(window.research_drafts.remove(index));
                        return;
                    }
                }
                if let Some(window) = &mut self.tailored_window {
                    if let Some(entry) =
                        window.picker.rows.iter().find(|r| r.id == id && r.workshop)
                    {
                        window.workshop = Some(workshop::WorkshopWindow::load(entry.account, id));
                        return;
                    }
                }
                if let Ok(draft) = crate::session::tailored::research::Store::open()
                    .and_then(|store| store.get(&id))
                {
                    if let Some(window) = &mut self.tailored_window {
                        window.research = Some(research::ResearchWindow::new(
                            draft.inputs.account,
                            Some(draft),
                        ));
                    }
                    return;
                }
                if self.get_instance(&id).is_some_and(|instance| {
                    crate::session::tailored::document::account(instance).is_some()
                }) {
                    if let Err(error) = self.start_document_terminal(&id, true) {
                        if let Some(window) = &mut self.tailored_window {
                            window.error = Some(format!("{error:#}"));
                        }
                    }
                    return;
                }
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
                if let Some(window) = &mut self.tailored_window {
                    if window.picker.rows.iter().any(|r| r.id == id && r.workshop) {
                        if window.workshop_visibility.is_some() {
                            return;
                        }
                        window.workshop_history_generation += 1;
                        let (tx, rx) = mpsc::channel();
                        window.workshop_visibility = Some(rx);
                        std::thread::spawn(move || {
                            let result = (|| -> anyhow::Result<()> {
                                let store = crate::session::tailored::workshop::Store::open()?;
                                let request = store.get(&id)?;
                                store.operate(
                                    crate::session::tailored::workshop::Operation::Visibility {
                                        id,
                                        revision: request.revision,
                                        hidden,
                                    },
                                )?;
                                Ok(())
                            })()
                            .map_err(|e| format!("{e:#}"));
                            let _ = tx.send(result);
                        });
                        return;
                    }
                }
                if let Ok(store) = crate::session::tailored::research::Store::open() {
                    if store.get(&id).is_ok() {
                        let result = store.change(&id, |d| {
                            d.hidden = hidden;
                            Ok(())
                        });
                        if let Some(window) = &mut self.tailored_window {
                            window.error = result.err().map(|e| e.to_string());
                        }
                        self.refresh_tailored_history();
                        return;
                    }
                }
                let result = self
                    .get_instance(&id)
                    .filter(|instance| {
                        crate::session::tailored::history::tailored_account(instance).is_some()
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
        let result = crate::session::tailored::history::load_hidden().and_then(|hidden| {
            let mut rows = crate::session::tailored::history::entries(self.instances(), &hidden);
            rows.extend(crate::session::tailored::research::Store::open()?.history()?);
            rows.sort_by(crate::session::tailored::history::compare);
            Ok(rows)
        });
        let Some(window) = &mut self.tailored_window else {
            return false;
        };
        if window.workshop_history.is_none() && window.workshop_visibility.is_none() {
            let (tx, rx) = mpsc::channel();
            window.workshop_history = Some((window.workshop_history_generation, rx));
            std::thread::spawn(move || {
                let result = load_workshop_history();
                let _ = tx.send(result);
            });
        }
        window.history_refreshed = Some(Instant::now());
        match result {
            Ok(mut rows) => {
                rows.extend(window.picker.rows.iter().filter(|r| r.workshop).cloned());
                rows.sort_by(crate::session::tailored::history::compare);
                window.picker.replace_rows(rows)
            }
            Err(error) => {
                let message = format!("Cannot read capture history: {error:#}");
                let changed = window.error.as_deref() != Some(&message);
                window.error = Some(message);
                changed
            }
        }
    }

    pub(super) fn tick_tailored(&mut self) -> bool {
        let visible = self.tailored_window.take();
        for window in std::mem::take(&mut self.retained_tailored_windows) {
            self.tailored_window = Some(window);
            self.tick_tailored_operations();
            if let Some(mut window) = self.tailored_window.take() {
                if let Some(terminal) = window.terminal.take() {
                    window.suspended_terminal = Some(terminal.session_id);
                }
                self.retained_tailored_windows.push(window);
            }
        }
        self.tailored_window = visible;
        self.tick_tailored_operations()
    }

    fn tick_tailored_operations(&mut self) -> bool {
        let mut workshop_changed = false;
        if let Some(window) = &mut self.tailored_window {
            for form in &mut window.workshop_drafts {
                workshop_changed |= form.tick();
            }
            for form in &mut window.research_drafts {
                workshop_changed |= form.tick(false);
            }
        }
        if let Some(window) = &mut self.tailored_window {
            if let Some((generation, result)) = window
                .workshop_history
                .as_ref()
                .map(|(generation, rx)| (*generation, rx.try_recv()))
                .filter(|(_, r)| !matches!(r, Err(mpsc::TryRecvError::Empty)))
            {
                window.workshop_history = None;
                if generation != window.workshop_history_generation {
                    window.history_refreshed = None;
                } else {
                    match result {
                        Ok(Ok(workshop)) => {
                            let mut rows: Vec<_> = window
                                .picker
                                .rows
                                .iter()
                                .filter(|r| !r.workshop)
                                .cloned()
                                .collect();
                            rows.extend(workshop);
                            rows.sort_by(crate::session::tailored::history::compare);
                            workshop_changed |= window.picker.replace_rows(rows);
                        }
                        _ => {
                            window.error = Some(
                                "Workshop history unavailable; previous snapshot retained".into(),
                            );
                            workshop_changed = true;
                        }
                    }
                }
            }
            if let Some(result) = window
                .workshop_visibility
                .as_ref()
                .map(|rx| rx.try_recv())
                .filter(|r| !matches!(r, Err(mpsc::TryRecvError::Empty)))
            {
                window.workshop_visibility = None;
                window.error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    _ => {
                        Some("Workshop visibility outcome unknown; refresh the same history".into())
                    }
                };
                window.history_refreshed = None;
                workshop_changed = true;
            }
        }
        let refresh = self.tailored_window.as_ref().is_some_and(|window| {
            !window.hidden
                && window
                    .history_refreshed
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(2))
        });
        let mut refreshed = (refresh && self.refresh_tailored_history()) || workshop_changed;
        let started = self
            .tailored_window
            .as_ref()
            .and_then(|window| window.starting_document.as_ref())
            .and_then(|worker| worker.take_result());
        if let Some((id, outcome)) = started {
            let foreground = self.tailored_window.as_ref().is_some_and(|w| {
                w.starting_document_foreground
                    && !w.hidden
                    && w.research.is_none()
                    && w.workshop.is_none()
                    && w.document.is_none()
                    && !w.composing
            });
            let profile = self
                .tailored_window
                .as_ref()
                .expect("document owner")
                .profile
                .clone();
            let storage = if self.storages.contains_key(&profile) {
                Ok(())
            } else {
                crate::session::Storage::new(&profile, self.file_watch.clone()).map(|storage| {
                    self.storages.insert(profile, storage);
                })
            };
            let result = storage
                .and_then(|()| self.reload())
                .and_then(|()| outcome.map_err(anyhow::Error::msg))
                .and_then(|()| {
                    if foreground {
                        self.ready_capture_terminal(&id).map(Some)
                    } else {
                        Ok(None)
                    }
                });
            if let Some(window) = &mut self.tailored_window {
                window.starting_document = None;
                window.history_refreshed = None;
                match result {
                    Ok(terminal) => { if let Some(terminal) = terminal { window.terminal = Some(terminal); } window.error = None; if window.hidden && window.starting_document_foreground && window.research.is_none() && window.workshop.is_none() && window.document.is_none() { window.suspended_terminal = Some(id); } }
                    Err(error) => window.error = Some(format!("Document start not confirmed: {error:#}. Inspect this Session in history; do not create another copy.")),
                }
            }
            return true;
        }
        if let Some(workshop) = self
            .tailored_window
            .as_mut()
            .and_then(|w| w.workshop.as_mut())
        {
            refreshed |= workshop.tick();
        }
        let visible = self.tailored_window.as_ref().is_some_and(|w| !w.hidden);
        if let Some(research) = self
            .tailored_window
            .as_mut()
            .and_then(|w| w.research.as_mut())
        {
            refreshed |= research.tick(visible);
        }
        let mut document_owner = None;
        let document_result = self.tailored_window.as_mut().and_then(|w| {
            if w.starting_document.is_some() {
                return None;
            }
            if let Some(result) = w.document.as_mut().and_then(|d| d.take_result()) {
                return Some(result);
            }
            for (index, form) in w.document_drafts.iter_mut().enumerate() {
                if let Some(result) = form.take_result() {
                    document_owner = Some(index);
                    return Some(result);
                }
            }
            None
        });
        if let Some(result) = document_result {
            let previous = self.selected_session.clone();
            let prepared = result.is_ok();
            let result = result.map_err(anyhow::Error::msg).and_then(|prepared| {
                let (built, profile) = prepared.accept();
                let id = self.accept_built_session(built, profile, false)?;
                self.start_document_terminal(&id, document_owner.is_none())
            });
            if let Some(id) = previous {
                self.select_and_reveal_session(&id);
            }
            if let Some(window) = &mut self.tailored_window {
                if prepared {
                    if let Some(index) = document_owner {
                        window.document_drafts.remove(index);
                    } else {
                        window.document = None;
                    }
                    window.history_refreshed = None;
                }
                match result {
                    Ok(()) => {}
                    Err(error) => {
                        if let Some(document) = if prepared {
                            None
                        } else {
                            match document_owner {
                                Some(i) => window.document_drafts.get_mut(i),
                                None => window.document.as_mut(),
                            }
                        } {
                            document.error(format!("{error:#}"));
                        } else {
                            window.error = Some(format!("Document create/start not confirmed: {error:#}. Check this Session in history; do not start another copy."));
                        }
                    }
                }
            }
            return true;
        }
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
        let result = self.launch_capture(request).and_then(|id| {
            if self.tailored_window.as_ref().is_some_and(|w| w.hidden) {
                Ok((id, None))
            } else {
                self.ready_capture_terminal(&id)
                    .map(|terminal| (id, Some(terminal)))
            }
        });
        if let Some(id) = previous {
            self.select_and_reveal_session(&id);
        }
        if let Some(window) = &mut self.tailored_window {
            window.checking = None;
            match result {
                Ok((id, terminal)) => {
                    window.terminal = terminal;
                    if window.hidden {
                        window.suspended_terminal = Some(id);
                    }
                    window.draft_committed = true;
                }
                Err(error) => window.error = Some(format!("{error:#}")),
            }
        }
        true
    }

    fn launch_capture(&mut self, request: TailoredRequest) -> anyhow::Result<String> {
        let launch = request.launch()?;
        let id = self.create_session(crate::tui::dialogs::NewSessionData {
            profile: self
                .tailored_window
                .as_ref()
                .expect("capture owner")
                .profile
                .clone(),
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
        self.try_mutate_instance_writeback_on_err(&id, |instance| {
            instance.ensure_pane_ready().map_err(Into::into)
        })?;
        Ok(id)
    }

    fn start_document_terminal(&mut self, id: &str, foreground: bool) -> anyhow::Result<()> {
        let instance = self
            .get_instance(id)
            .filter(|instance| crate::session::tailored::document::account(instance).is_some())
            .ok_or_else(|| anyhow::anyhow!("Document Session disappeared"))?
            .clone();
        let window = self
            .tailored_window
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Document window closed"))?;
        anyhow::ensure!(
            window.starting_document.is_none(),
            "A document start is already in progress"
        );
        window.starting_document_foreground = foreground;
        if foreground {
            window.terminal = None;
            window.composing = false;
            window.error = Some(
                "Starting document conversation; launch-time Ledger verification is running…"
                    .into(),
            );
        }
        #[cfg(test)]
        if let Some(start) = &window.document_starter {
            window.starting_document = Some(start(instance));
            return Ok(());
        }
        window.starting_document = Some(document::StartWorker::spawn(instance));
        Ok(())
    }

    fn open_capture_terminal(&mut self, id: &str) -> anyhow::Result<CaptureTerminal> {
        anyhow::ensure!(
            self.get_instance(id).is_some_and(|instance| {
                crate::session::tailored::history::tailored_account(instance).is_some()
            }),
            "This is not a capture Session"
        );
        anyhow::ensure!(
            self.get_instance(id).is_none_or(|instance| {
                crate::session::tailored::document::account(instance).is_none()
            }),
            "Document launch requires its background start worker"
        );
        self.try_mutate_instance_writeback_on_err(id, |instance| {
            instance.ensure_pane_ready().map_err(Into::into)
        })?;
        self.ready_capture_terminal(id)
    }

    fn ready_capture_terminal(&self, id: &str) -> anyhow::Result<CaptureTerminal> {
        let instance = self
            .get_instance(id)
            .ok_or_else(|| anyhow::anyhow!("Capture Session disappeared"))?;
        instance.ensure_startable()?;
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
    fn capture_enter_edits_and_editor_redo_does_not_close() {
        let mut window = TailoredWindow::new(CaptureAccount::Personal);
        window.picker_action(HistoryAction::New);
        window.handle_paste("seed\0");
        window.handle_key(key(KeyCode::Enter));
        assert_eq!(window.idea.lines().join("\n"), "seed\0\n");
        assert!(window.checking.is_none());
        assert!(matches!(
            window.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)),
            WindowAction::Continue
        ));
    }

    #[test]
    fn capture_pending_check_preserves_buffer_cursor_undo_and_request() {
        let mut window = TailoredWindow::new(CaptureAccount::Work);
        window.picker_action(HistoryAction::New);
        window.handle_paste("leftRIGHT\nnext");
        window.handle_key(key(KeyCode::Home));
        window.handle_key(key(KeyCode::Right));
        window.handle_paste("🌱");
        window.immediately = true;
        let mut expected = window.idea.clone();
        let request = TailoredRequest {
            preset: "capture-idea".into(),
            account: window.account,
            idea: window.idea.lines().join("\n"),
            immediately: window.immediately,
            document: None,
        };
        let original = serde_json::to_value(&request).unwrap();
        let (tx, rx) = mpsc::channel();
        window.checking = Some(rx);
        for (code, modifiers) in [
            (KeyCode::Char('x'), KeyModifiers::NONE),
            (KeyCode::Enter, KeyModifiers::NONE),
            (KeyCode::Backspace, KeyModifiers::NONE),
            (KeyCode::Delete, KeyModifiers::NONE),
            (KeyCode::Left, KeyModifiers::NONE),
            (KeyCode::Char('u'), KeyModifiers::CONTROL),
            (KeyCode::Char('r'), KeyModifiers::CONTROL),
        ] {
            for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
                window.handle_key(KeyEvent::new_with_kind(code, modifiers, kind));
                assert_eq!(window.idea.lines(), expected.lines(), "{code:?} {kind:?}");
                assert_eq!(window.idea.cursor(), expected.cursor(), "{code:?} {kind:?}");
            }
        }
        window.handle_paste("not submitted");
        assert_eq!(window.idea.lines(), expected.lines());
        assert_eq!(window.idea.cursor(), expected.cursor());
        assert!(matches!(
            window.checking.as_ref().unwrap().try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        tx.send(Ok(request)).unwrap();
        let checked = window.checking.take().unwrap().try_recv().unwrap().unwrap();
        assert_eq!(serde_json::to_value(checked).unwrap(), original);
        for code in ['u', 'r'] {
            let event = KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL);
            expected.input(event);
            window.handle_key(event);
            assert_eq!(window.idea.lines(), expected.lines());
            assert_eq!(window.idea.cursor(), expected.cursor());
        }
    }

    #[test]
    fn preset_history_roundtrip_preserves_document_editor() {
        let mut window = TailoredWindow::new(CaptureAccount::Personal);
        window.picker_action(HistoryAction::Document);
        window.handle_paste("https://example.org/source.md\n");
        window.handle_key(key(KeyCode::F(2)));
        window.picker_action(HistoryAction::Document);
        // Undo must address the original editor, not a newly constructed one.
        window.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(window.document.is_some());
        window.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(
            window.document.as_ref().unwrap().url_text_for_test(),
            "https://example.org/source.md\n"
        );
    }

    #[test]
    fn capture_multiline_middle_paste_and_account_roundtrip_keep_native_undo() {
        let mut window = TailoredWindow::new(CaptureAccount::Personal);
        window.picker_action(HistoryAction::New);
        window.handle_paste("leftRIGHT");
        window.handle_key(key(KeyCode::Home));
        for _ in 0..4 {
            window.handle_key(key(KeyCode::Right));
        }
        let paste = format!("🌱日本語{}\n\nlast\n", "long".repeat(1000));
        window.handle_paste(&paste);
        let exact = format!("left{paste}RIGHT");
        assert_eq!(window.idea.lines().join("\n"), exact);
        assert!(window.checking.is_none());
        window.toggle_account();
        assert_eq!(window.idea.lines().join("\n"), "");
        window.handle_paste("work only");
        window.toggle_account();
        assert_eq!(window.idea.lines().join("\n"), exact);
        window.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(window.idea.lines().join("\n"), "leftRIGHT");
        window.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(window.idea.lines().join("\n"), exact);
        let before = window.idea.lines().join("\n");
        window.handle_key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ));
        assert_ne!(window.idea.lines().join("\n"), before);
        window.handle_key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::CONTROL,
            KeyEventKind::Release,
        ));
        assert!(window.checking.is_none());
    }

    #[test]
    #[serial_test::serial]
    fn home_close_reopen_retains_exact_editor_without_hidden_history_polling() {
        let root = tempfile::tempdir().unwrap();
        let _home = crate::session::test_support::isolate_app_dir_at(root.path());
        let mut view = HomeView::new_for_test(
            Some("test".into()),
            crate::tmux::AvailableTools::with_tools(&["pi"]),
            crate::file_watch::FileWatchService::noop(),
        )
        .unwrap();
        view.open_tailored();
        let window = view.tailored_window.as_mut().unwrap();
        window.picker_action(HistoryAction::New);
        window.handle_paste("private 🌱\n\n");
        window.handle_key(key(KeyCode::Left));
        let cursor = window.idea.cursor();
        view.apply_tailored_action(WindowAction::Close);
        assert!(view.tailored_window.is_none());
        view.retained_tailored_windows
            .first_mut()
            .unwrap()
            .history_refreshed = None;
        view.tick_tailored();
        assert!(view
            .retained_tailored_windows
            .first()
            .unwrap()
            .history_refreshed
            .is_none());
        view.open_tailored();
        let window = view.tailored_window.as_mut().unwrap();
        assert_eq!(window.idea.lines().join("\n"), "private 🌱\n\n");
        assert_eq!(window.idea.cursor(), cursor);
        window.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(window.idea.lines().join("\n"), "");
    }

    #[test]
    #[serial_test::serial]
    fn home_profile_switch_does_not_relabel_or_copy_private_drafts() {
        let root = tempfile::tempdir().unwrap();
        let _home = crate::session::test_support::isolate_app_dir_at(root.path());
        let mut view = HomeView::new_for_test(
            Some("first".into()),
            crate::tmux::AvailableTools::with_tools(&["pi"]),
            crate::file_watch::FileWatchService::noop(),
        )
        .unwrap();
        view.open_tailored();
        let window = view.tailored_window.as_mut().unwrap();
        window.picker_action(HistoryAction::New);
        window.handle_paste("first profile private 🌱\n");
        let (tx, rx) = mpsc::channel();
        window.checking = Some(rx);
        tx.send(Err("first owner check failed".into())).unwrap();
        view.switch_profile(Some("second".into())).unwrap();
        assert!(view.tailored_window.is_none());
        view.open_tailored();
        let window = view.tailored_window.as_mut().unwrap();
        window.picker_action(HistoryAction::New);
        assert_eq!(window.idea.lines().join("\n"), "");
        window.handle_paste("second profile private");
        view.tick_tailored();
        assert!(view.tailored_window.as_ref().unwrap().error.is_none());
        assert_eq!(
            view.retained_tailored_windows
                .iter()
                .find(|w| w.profile == "first")
                .unwrap()
                .error
                .as_deref(),
            Some("first owner check failed")
        );
        view.apply_tailored_action(WindowAction::Close);
        view.switch_profile(Some("first".into())).unwrap();
        view.open_tailored();
        assert_eq!(
            view.tailored_window
                .as_ref()
                .unwrap()
                .idea
                .lines()
                .join("\n"),
            "first profile private 🌱\n"
        );
    }

    #[test]
    fn idea_input_wraps_without_changing_text_and_keeps_new_typing_visible() {
        use ratatui::backend::TestBackend;

        fn assert_visible(window: &TailoredWindow, width: u16, height: u16, tail: &str) -> usize {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| frame.render_widget(&window.idea, frame.area()))
                .unwrap();
            let cells = terminal.backend().buffer().content();
            let visible: String = cells
                .iter()
                .flat_map(|cell| cell.symbol().chars())
                .filter(|ch| !ch.is_whitespace())
                .collect();
            assert!(
                visible.ends_with(tail),
                "latest input disappeared at {width}x{height}"
            );
            assert_eq!(
                cells
                    .iter()
                    .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
                    .count(),
                1,
                "editing cursor disappeared at {width}x{height}"
            );
            cells
                .chunks(width as usize)
                .filter(|row| row.iter().any(|cell| !cell.symbol().trim().is_empty()))
                .count()
        }

        // Cross the old horizontal viewport's u16 range without submitting.
        for text in [
            format!("https://example.org/{} TAIL", "x".repeat(70_000)),
            format!("{} TAIL", "é界🌱e\u{301} words ".repeat(150)),
            format!("{} TAIL", "A thought with words ".repeat(100)),
        ] {
            let mut window = TailoredWindow::new(CaptureAccount::Personal);
            window.composing = true;
            window.handle_paste(&text);
            let mut expected = text;
            for (index, (width, height)) in [(72, 8), (19, 6), (95, 10)].into_iter().enumerate() {
                window.handle_key(key(KeyCode::Char('!')));
                expected.push('!');
                let displayed_rows = assert_visible(
                    &window,
                    width,
                    height,
                    &format!("TAIL{}", "!".repeat(index + 1)),
                );
                // Reflow may anchor the cursor row at the top after a wider resize.
                if index == 0 {
                    assert!(
                        displayed_rows > 1,
                        "long input still occupies one horizontal row"
                    );
                }
                assert_eq!(window.idea.lines(), std::slice::from_ref(&expected));
            }
            window.handle_key(key(KeyCode::Backspace));
            expected.pop();
            assert_visible(&window, 33, 7, "TAIL!!");
            assert_eq!(window.idea.lines(), std::slice::from_ref(&expected));
            window.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
            window.handle_key(key(KeyCode::Char('n')));
            assert_eq!(window.idea.lines(), &[expected, "n".into()]);
            assert!(window.checking.is_none());

            window.draft_committed = true;
            window.picker_action(HistoryAction::New);
            let fresh = format!("{} TAIL", "Fresh capture ".repeat(50));
            window.handle_paste(&fresh);
            assert!(assert_visible(&window, 33, 7, "TAIL") > 1);
            assert_eq!(window.idea.lines(), &[fresh]);
        }
    }

    #[test]
    #[serial_test::serial]
    fn document_initial_start_and_history_reopen_qualify_off_the_tick_thread() {
        use crate::session::tailored::document::native::test_support::HoldQualification;
        use crate::session::{builder::BuildResult, Instance, Storage};
        let root = tempfile::tempdir().unwrap();
        let _home = crate::session::test_support::isolate_app_dir_at(root.path());
        let _tmux = crate::session::test_support::install_login_shell_path_command(
            root.path(),
            "tmux",
            "#!/bin/sh\nexit 1\n",
        );
        let _ledger = crate::session::test_support::install_login_shell_path_command(
            root.path(),
            "ledger",
            "#!/bin/sh\nexit 99\n",
        );
        for reopen in [false, true] {
            let scratch = root.path().join(if reopen { "reopen" } else { "initial" });
            std::fs::create_dir(&scratch).unwrap();
            let mut instance = Instance::new("Document", scratch.to_str().unwrap());
            instance.source_profile = "test".into();
            instance.tool = "pi".into();
            instance.scratch = true;
            instance.command = "ledger run pi --profile aoe-document-work --".into();
            instance.extra_args = crate::session::tailored::document::restrictive_args();
            let id = instance.id.clone();
            let storage = Storage::new_unwatched("test").unwrap();
            storage
                .update(|rows, _| {
                    *rows = if reopen {
                        vec![instance.clone()]
                    } else {
                        vec![]
                    };
                    Ok(())
                })
                .unwrap();
            let mut view = HomeView::new_for_test(
                Some("test".into()),
                crate::tmux::AvailableTools::with_tools(&["pi"]),
                crate::file_watch::FileWatchService::noop(),
            )
            .unwrap();
            view.open_tailored();
            let mut gate = HoldQualification::new(&scratch);
            if reopen {
                view.apply_tailored_action(WindowAction::Open(id.clone()));
            } else {
                view.tailored_window.as_mut().unwrap().document =
                    Some(document::DocumentWindow::prepared_for_test(
                        BuildResult {
                            instance,
                            created_worktree: None,
                            created_workspace_worktrees: vec![],
                            warnings: vec![],
                        },
                        "test".into(),
                    ));
                assert!(view.tick_tailored());
            }
            let worker = gate
                .arrived
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|error| {
                    panic!(
                        "qualification gate {error:?}; worker result: {:?}",
                        view.tailored_window
                            .as_ref()
                            .unwrap()
                            .starting_document
                            .as_ref()
                            .unwrap()
                            .take_result()
                    )
                });
            assert_ne!(worker, std::thread::current().id());
            assert!(view
                .tailored_window
                .as_ref()
                .unwrap()
                .starting_document
                .is_some());
            view.tick_tailored();
            assert!(
                view.tailored_window
                    .as_ref()
                    .unwrap()
                    .starting_document
                    .as_ref()
                    .unwrap()
                    .take_result()
                    .is_none(),
                "tick returned without waiting for qualification"
            );
            view.handle_tailored_key(key(KeyCode::Char('x')));
            let rendered = {
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
                terminal
                    .draw(|frame| {
                        view.tailored_window.as_mut().unwrap().render(
                            frame,
                            frame.area(),
                            &Theme::default(),
                        )
                    })
                    .unwrap();
                terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
            };
            assert!(
                rendered.contains('x'),
                "history search input continues while qualification waits"
            );
            gate.release();
            let deadline = Instant::now() + Duration::from_secs(5);
            while view
                .tailored_window
                .as_ref()
                .unwrap()
                .starting_document
                .is_some()
            {
                view.tick_tailored();
                assert!(
                    Instant::now() < deadline,
                    "start worker must finish after release"
                );
                std::thread::yield_now();
            }
            assert!(view
                .tailored_window
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("not confirmed"));
            assert!(storage.load().unwrap().iter().any(|row| row.id == id));
        }
    }

    #[test]
    #[serial_test::serial]
    fn parked_document_preparation_waits_for_start_slot_without_losing_owner() {
        use crate::session::{builder::BuildResult, Instance, Storage};
        let root = tempfile::tempdir().unwrap();
        let _home = crate::session::test_support::isolate_app_dir_at(root.path());
        let mut view = HomeView::new_for_test(
            Some("test".into()),
            crate::tmux::AvailableTools::with_tools(&["pi"]),
            crate::file_watch::FileWatchService::noop(),
        )
        .unwrap();
        view.open_tailored();
        let (started, arrivals) = mpsc::channel();
        view.tailored_window.as_mut().unwrap().document_starter = Some(Box::new(move |instance| {
            let (done, pending) = mpsc::channel();
            started.send((instance.id, done)).unwrap();
            document::StartWorker::from_receiver_for_test(pending)
        }));
        let mut ids = vec![];
        for account in [CaptureAccount::Personal, CaptureAccount::Work] {
            let scratch = root.path().join(format!("{account:?}"));
            std::fs::create_dir(&scratch).unwrap();
            let mut instance = Instance::new("Document", scratch.to_str().unwrap());
            instance.source_profile = "test".into();
            instance.tool = "pi".into();
            instance.scratch = true;
            instance.command = format!(
                "ledger run pi --profile aoe-document-{} --",
                if account == CaptureAccount::Personal {
                    "personal"
                } else {
                    "work"
                }
            );
            ids.push(instance.id.clone());
            let mut form = document::DocumentWindow::prepared_for_test(
                BuildResult {
                    instance,
                    created_worktree: None,
                    created_workspace_worktrees: vec![],
                    warnings: vec![],
                },
                "test".into(),
            );
            form.account = account;
            view.tailored_window.as_mut().unwrap().document = Some(form);
            view.handle_tailored_key(key(KeyCode::F(2)));
            view.tick_tailored();
        }
        let (first, finish_first) = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first, ids[0]);
        let window = view.tailored_window.as_ref().unwrap();
        assert_eq!(
            window.document_drafts.len(),
            1,
            "second prepared editor must remain owned while the first start waits"
        );
        assert!(window.document_drafts[0].busy());
        assert!(
            root.path().join("Work").exists(),
            "prepared guard must remain alive"
        );
        assert_eq!(
            Storage::new_unwatched("test")
                .unwrap()
                .load()
                .unwrap()
                .len(),
            1
        );
        assert!(arrivals.try_recv().is_err());
        view.switch_profile(Some("second".into())).unwrap();
        view.open_tailored();
        let active = view.tailored_window.as_mut().unwrap();
        active.picker_action(HistoryAction::New);
        active.handle_paste("second profile private 🌱");
        let active_cursor = active.idea.cursor();
        finish_first.send((first, Ok(()))).unwrap();
        view.tick_tailored();
        view.tick_tailored();
        let (second, finish_second) = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(second, ids[1]);
        finish_second.send((second, Ok(()))).unwrap();
        view.tick_tailored();
        let active = view.tailored_window.as_ref().unwrap();
        assert_eq!(active.profile, "second");
        assert_eq!(active.idea.lines().join("\n"), "second profile private 🌱");
        assert_eq!(active.idea.cursor(), active_cursor);
        assert!(active.terminal.is_none());
        assert!(active.error.is_none());
        assert!(Storage::new_unwatched("second")
            .unwrap()
            .load()
            .unwrap()
            .is_empty());
        let window = view
            .retained_tailored_windows
            .iter()
            .find(|w| w.profile == "test")
            .unwrap();
        assert!(window.terminal.is_none());
        assert!(window.document_drafts.is_empty());
        assert!(window.starting_document.is_none());
        assert!(window.error.is_none());
        assert_eq!(
            Storage::new_unwatched("test")
                .unwrap()
                .load()
                .unwrap()
                .len(),
            2
        );
        assert!(
            arrivals.try_recv().is_err(),
            "each explicitly requested start happens once"
        );
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
        window.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
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
