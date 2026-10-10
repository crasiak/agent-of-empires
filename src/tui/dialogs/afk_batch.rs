use super::DialogResult;
use crate::{session::afk::batch, tui::styles::Theme};
use crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::{prelude::*, widgets::*};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};
use tui_input::{backend::crossterm::EventHandler, Input};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    Load(PathBuf),
    Apply { path: PathBuf, digest: String },
    Status { profile: String, operation: String },
    Off { profile: String, operation: String },
}
type Reply = anyhow::Result<Value>;
type Dispatch = Box<dyn Fn(Request) -> Receiver<Reply> + Send>;

pub struct AfkBatchDialog {
    path: Input,
    reviewed: Option<(PathBuf, Value)>,
    attempted: bool,
    pending: Option<(Request, Receiver<Reply>)>,
    dispatch: Dispatch,
    message: String,
    scroll: u16,
}
impl Default for AfkBatchDialog {
    fn default() -> Self {
        Self::new()
    }
}
impl AfkBatchDialog {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<(Request, mpsc::Sender<Reply>)>();
        std::thread::spawn(move || {
            for (request, reply) in rx {
                let result = match request {
                    Request::Load(path) => batch::review(&path),
                    Request::Apply { path, digest } => batch::apply(&path, &digest),
                    Request::Status { profile, operation } => batch::status(&profile, &operation),
                    Request::Off { profile, operation } => batch::off(&profile, &operation),
                };
                let _ = reply.send(result);
            }
        });
        Self::with_dispatch(Box::new(move |request| {
            let (reply, result) = mpsc::channel();
            let _ = tx.send((request, reply));
            result
        }))
    }
    fn with_dispatch(dispatch: Dispatch) -> Self {
        Self {
            path: Input::default(),
            reviewed: None,
            attempted: false,
            pending: None,
            dispatch,
            message: "Load a private CLI preview-v2 file. Loading never activates sessions.".into(),
            scroll: 0,
        }
    }
    fn start(&mut self, request: Request) {
        if self.pending.is_some() {
            return;
        }
        self.pending = Some((request.clone(), (self.dispatch)(request)));
        self.scroll = 0;
    }
    pub fn tick(&mut self) -> bool {
        let Some((request, receiver)) = &self.pending else {
            return false;
        };
        match receiver.try_recv() {
            Ok(result) => {
                match result {
                    Ok(value) => {
                        if let Request::Load(path) = request { self.reviewed = Some((path.clone(), value.clone())); }
                        self.message = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "Invalid worker response".into());
                    }
                    Err(error) => self.message = format!("Unavailable: {error:#}\nNo automatic retry. Inspect the original operation before retrying."),
                }
                self.pending = None;
                true
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.message = "Batch worker unavailable; inspect operation state".into();
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
        }
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<()> {
        if key.code == KeyCode::Esc {
            return DialogResult::Cancel;
        }
        if self.pending.is_some() {
            return DialogResult::Continue;
        }
        if let Some((path, preview)) = &self.reviewed {
            match key.code {
                KeyCode::Char('a') if !self.attempted => {
                    if let Some(digest) = preview["digest"].as_str() {
                        let request = Request::Apply {
                            path: path.clone(),
                            digest: digest.into(),
                        };
                        self.attempted = true;
                        self.start(request);
                    }
                }
                KeyCode::Char('r' | 'o') if self.attempted => {
                    if let (Some(profile), Some(operation)) = (
                        preview["profile"].as_str(),
                        preview["operation_id"].as_str(),
                    ) {
                        let request = if key.code == KeyCode::Char('o') {
                            Request::Off {
                                profile: profile.into(),
                                operation: operation.into(),
                            }
                        } else {
                            Request::Status {
                                profile: profile.into(),
                                operation: operation.into(),
                            }
                        };
                        self.start(request);
                    }
                }
                KeyCode::Down | KeyCode::PageDown => {
                    self.scroll =
                        self.scroll
                            .saturating_add(if key.code == KeyCode::Down { 1 } else { 10 })
                }
                KeyCode::Up | KeyCode::PageUp => {
                    self.scroll =
                        self.scroll
                            .saturating_sub(if key.code == KeyCode::Up { 1 } else { 10 })
                }
                _ => {}
            }
        } else if key.code == KeyCode::Enter {
            if self.path.value().is_empty() {
                self.message = "Enter a private preview-v2 path".into();
            } else {
                self.start(Request::Load(self.path.value().into()));
            }
        } else {
            self.path.handle_event(&Event::Key(key));
        }
        DialogResult::Continue
    }
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = super::dialog_block(" AFK batch review / explicit apply ", theme);
        let (_, inner) = super::render_dialog_frame(frame, area, 116, 34, block);
        let layout = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(inner.inner(Margin {
            horizontal: 1,
            vertical: 0,
        }));
        let heading = self.reviewed.as_ref().map_or_else(|| format!("Private preview-v2 path: {}\nEnter loads for review only; inspection-v1 is refused.", self.path.value()), |(_, p)| format!("Profile: {} | frozen members: {} | requested duration: {} minutes\n{} | first-apply expiry is never renewed on retry\nReviewed digest: {}", p["profile"].as_str().unwrap_or("unknown"), p["members"].as_array().map_or(0, Vec::len), p["minutes"], if p["shared_requests"].is_null() { "No shared budget".into() } else { format!("Proposed shared allowance: {} SDK reservations",p["shared_requests"]) }, p["digest"].as_str().unwrap_or("unknown")));
        frame.render_widget(
            Paragraph::new(heading).style(Style::default().fg(theme.text)),
            layout[0],
        );
        frame.render_widget(
            Paragraph::new(self.message.as_str())
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0))
                .style(Style::default().fg(theme.text)),
            layout[1],
        );
        let footer = if self.pending.is_some() {
            "Waiting for scoped service. Closing does not undo a requested operation.\nEsc: close"
        } else if self.reviewed.is_none() {
            "Enter: load without activation   Esc: close"
        } else if self.attempted {
            "r: inspect original operation   o: stop original windows   Up/Down: scroll\nNo automatic apply retry or authority renewal. Esc: close"
        } else {
            "Review exact grants and exclusions above. a: explicitly apply this digest\nEnter does not activate. Up/Down: scroll   Esc: close"
        };
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(theme.text)),
            layout[2],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[test]
    fn loading_and_navigation_cannot_apply_without_distinct_confirmation() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let mut dialog = AfkBatchDialog::with_dispatch(Box::new(move |request| {
            let (tx, rx) = mpsc::channel();
            recorded.lock().unwrap().push((request, tx));
            rx
        }));
        for c in "/private/preview.json".chars() {
            dialog.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        dialog.handle_key(KeyEvent::from(KeyCode::Enter));
        dialog.handle_key(KeyEvent::from(KeyCode::Char('a')));
        assert_eq!(calls.lock().unwrap().len(), 1);
        calls.lock().unwrap()[0].1.send(Ok(serde_json::json!({"version":2,"apply_available":true,"profile":"alpha","operation_id":"op","digest":"a".repeat(64),"minutes":2,"shared_requests":null,"members":[{"session_id":"exact-member","exclusions":[]}]}))).unwrap();
        assert!(dialog.tick());
        dialog.handle_key(KeyEvent::from(KeyCode::Enter));
        dialog.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(calls.lock().unwrap().len(), 1);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 38)).unwrap();
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
        for needed in [
            "Profile: alpha",
            "No shared budget",
            "exact-member",
            "a: explicitly apply",
        ] {
            assert!(text.contains(needed), "{needed}");
        }
        dialog.handle_key(KeyEvent::from(KeyCode::Char('a')));
        dialog.handle_key(KeyEvent::from(KeyCode::Char('a')));
        assert_eq!(calls.lock().unwrap().len(), 2);
        assert_eq!(
            calls.lock().unwrap()[1].0,
            Request::Apply {
                path: "/private/preview.json".into(),
                digest: "a".repeat(64)
            }
        );
        calls.lock().unwrap()[1]
            .1
            .send(Ok(serde_json::json!({"members":[]})))
            .unwrap();
        dialog.tick();
        dialog.handle_key(KeyEvent::from(KeyCode::Char('a')));
        assert_eq!(calls.lock().unwrap().len(), 2);
        dialog.handle_key(KeyEvent::from(KeyCode::Char('o')));
        assert_eq!(
            calls.lock().unwrap()[2].0,
            Request::Off {
                profile: "alpha".into(),
                operation: "op".into()
            }
        );
        assert!(matches!(
            dialog.handle_key(KeyEvent::from(KeyCode::Esc)),
            DialogResult::Cancel
        ));
    }
}
