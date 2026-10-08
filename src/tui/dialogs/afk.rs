use super::DialogResult;
use crate::{
    session::{
        afk::{self, Operation},
        Instance,
    },
    tui::styles::Theme,
};
use crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::{prelude::*, widgets::*};
use std::sync::mpsc::{self, Receiver};
use tui_input::{backend::crossterm::EventHandler, Input};

type Reply = anyhow::Result<afk::Report>;
type Dispatch = Box<dyn Fn(Operation) -> Receiver<Reply> + Send>;

pub struct AfkDialog {
    title: String,
    minutes: Input,
    message: String,
    dispatch: Dispatch,
    pending: Option<Receiver<Reply>>,
    pending_operation: Option<Operation>,
    observed: Option<std::time::Instant>,
    stale: bool,
}
impl AfkDialog {
    pub fn new(instance: Instance) -> Self {
        let title = instance.title.clone();
        let (tx, rx) = mpsc::channel::<(Operation, mpsc::Sender<Reply>)>();
        // Closing the dialog closes the queue; an already-requested Off still runs.
        std::thread::spawn(move || {
            for (operation, reply) in rx {
                let _ = reply.send(afk::control(&instance, operation));
            }
        });
        let dispatch = Box::new(move |operation| {
            let (reply, result) = mpsc::channel();
            let _ = tx.send((operation, reply));
            result
        });
        let mut dialog = Self::with_dispatch(title, dispatch);
        dialog.start(Operation::Status);
        dialog
    }
    fn with_dispatch(title: String, dispatch: Dispatch) -> Self {
        Self {
            title,
            minutes: Input::default(),
            message: String::new(),
            dispatch,
            pending: None,
            pending_operation: None,
            observed: None,
            stale: false,
        }
    }
    fn start(&mut self, operation: Operation) {
        if self.pending.is_some()
            && (operation != Operation::Off || self.pending_operation == Some(Operation::Off))
        {
            return;
        }
        self.pending = Some((self.dispatch)(operation));
        self.pending_operation = Some(operation);
        self.observed = None;
        self.stale = false;
        self.message = if operation == Operation::Off {
            "Off queued; waiting for durable publication and acknowledgement..."
        } else {
            "Waiting for a fresh acknowledgement..."
        }
        .into();
    }
    pub fn tick(&mut self) -> bool {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.message = match result {
                        Ok(report) => {
                            self.observed =
                                report.observed_at_ms.map(|_| std::time::Instant::now());
                            report.describe()
                        }
                        Err(error) => format!("Unavailable: {error:#}"),
                    };
                    self.pending = None;
                    self.pending_operation = None;
                    return true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.pending_operation = None;
                    self.message = "AFK control worker unavailable".into();
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !self.stale
            && self
                .observed
                .is_some_and(|at| at.elapsed() >= afk::FRESHNESS)
        {
            self.stale = true;
            return true;
        }
        false
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<()> {
        match key.code {
            KeyCode::Esc => return DialogResult::Cancel,
            KeyCode::Char('r') => self.start(Operation::Status),
            KeyCode::Char('o') => self.start(Operation::Off),
            KeyCode::Enter => match self.minutes.value().parse::<u32>() {
                Ok(minutes) if (1..=1440).contains(&minutes) => {
                    self.start(Operation::On { minutes })
                }
                _ => self.message = "Enter an explicit expiry from 1 to 1440 minutes".into(),
            },
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                self.minutes.handle_event(&Event::Key(key));
            }
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End => {
                self.minutes.handle_event(&Event::Key(key));
            }
            _ => {}
        }
        DialogResult::Continue
    }
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = super::toned_dialog_block(
            " AFK control-only / delegation status ",
            theme.border,
            theme.title,
        );
        let (_, inner) = super::render_dialog_frame(frame, area, 76, 19, block);
        let stale = if self.stale {
            "STALE observation: press r to inspect again.\n"
        } else {
            ""
        };
        let text = format!("{}\n\n{}{}\n\nExpiry in minutes: {}\nEnter: control-only on   o: off   r: inspect   Esc: close\nQuestions remain open; delegation requires a CLI grant.", self.title, stale, self.message, self.minutes.value());
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text)),
            inner.inner(Margin {
                horizontal: 1,
                vertical: 1,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[test]
    fn explicit_duration_dispatch_off_priority_and_visible_controls() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let mut dialog = AfkDialog::with_dispatch(
            "test-session".into(),
            Box::new(move |operation| {
                let (tx, rx) = mpsc::channel();
                recorded.lock().unwrap().push((operation, tx));
                rx
            }),
        );
        dialog.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(calls.lock().unwrap().is_empty());
        assert!(dialog.message.contains("explicit expiry"));
        for code in [
            KeyCode::Char('1'),
            KeyCode::Char('5'),
            KeyCode::Enter,
            KeyCode::Char('o'),
            KeyCode::Char('r'),
            KeyCode::Enter,
            KeyCode::Char('o'),
        ] {
            dialog.handle_key(KeyEvent::from(code));
        }
        {
            let calls = calls.lock().unwrap();
            assert_eq!(
                calls.iter().map(|(op, _)| *op).collect::<Vec<_>>(),
                [Operation::On { minutes: 15 }, Operation::Off]
            );
            assert!(calls[0]
                .1
                .send(Err(anyhow::anyhow!("old on result")))
                .is_err());
            calls[1].1.send(Err(anyhow::anyhow!("off result"))).unwrap();
        }
        assert!(dialog.tick());
        assert!(dialog.message.contains("off result"));
        dialog.handle_key(KeyEvent::from(KeyCode::Char('r')));
        assert_eq!(calls.lock().unwrap().last().unwrap().0, Operation::Status);
        calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .1
            .send(Err(anyhow::anyhow!("probe failed")))
            .unwrap();
        assert!(dialog.tick());
        dialog.observed = Some(std::time::Instant::now() - afk::FRESHNESS);
        assert!(dialog.tick());
        assert!(dialog.stale);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| dialog.render(frame, frame.area(), &Theme::default()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        for required in [
            "AFK control-only",
            "Expiry in minutes: 15",
            "o: off",
            "r: inspect",
            "Questions remain open",
            "STALE observation",
        ] {
            assert!(text.contains(required), "missing {required}");
        }
        dialog.message = afk::Report {
            state: afk::State::Owned, requested_on: false, revision: 2,
            expires_at_ms: Some(1000), observed_at_ms: Some(1), detail: "fresh capability acknowledgement".into(),
            delegation: Some(serde_json::json!({"reservations_used":1,"reads_used":2,"grant":{"requests":4},"expires_at_ms":1000})),
        }.describe();
        dialog.stale = false;
        terminal
            .draw(|frame| dialog.render(frame, frame.area(), &Theme::default()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("Delegation: Owned"));
        assert!(text.contains("SDK reservations: 1/4"));
        assert!(!text.contains("no autonomy"));
        assert!(!text.contains("Autonomous allowance: 0"));
    }
}
