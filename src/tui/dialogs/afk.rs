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
type NativeReply = anyhow::Result<afk::work_period::Inspection>;
type NativeDispatch = Box<dyn Fn(NativeOperation) -> Receiver<NativeReply> + Send>;
enum NativeOperation {
    Inspect,
    Stop(Box<afk::work_period::Inspection>),
}

pub struct AfkDialog {
    title: String,
    minutes: Input,
    message: String,
    dispatch: Dispatch,
    pending: Option<Receiver<Reply>>,
    pending_operation: Option<Operation>,
    observed: Option<std::time::Instant>,
    stale: bool,
    native_dispatch: Option<NativeDispatch>,
    native_pending: Option<Receiver<NativeReply>>,
    native_report: Option<afk::work_period::Inspection>,
    native_message: String,
    native_observed: Option<std::time::Instant>,
    native_stale: bool,
    scroll: u16,
}
impl AfkDialog {
    pub fn new(instance: Instance) -> Self {
        let title = instance.title.clone();
        let worker = instance.clone();
        let (tx, rx) = mpsc::channel::<(Operation, mpsc::Sender<Reply>)>();
        std::thread::spawn(move || {
            for (operation, reply) in rx {
                let _ = reply.send(afk::control(&worker, operation));
            }
        });
        let off_instance = instance.clone();
        let dispatch = Box::new(move |operation| {
            let (reply, result) = mpsc::channel();
            if operation == Operation::Off {
                // Off publishes independently of an outstanding On/status exchange.
                let instance = off_instance.clone();
                std::thread::spawn(move || {
                    let _ = reply.send(afk::control(&instance, operation));
                });
            } else {
                let _ = tx.send((operation, reply));
            }
            result
        });
        let (native_tx, native_rx) =
            mpsc::channel::<(NativeOperation, mpsc::Sender<NativeReply>)>();
        std::thread::spawn(move || {
            for (operation, reply) in native_rx {
                let result = match operation {
                    NativeOperation::Inspect => afk::work_period::inspect(&instance),
                    NativeOperation::Stop(observed) => afk::work_period::stop(&instance, &observed),
                };
                let _ = reply.send(result);
            }
        });
        let mut dialog = Self::with_dispatch(title, dispatch);
        dialog.native_dispatch = Some(Box::new(move |operation| {
            let (reply, result) = mpsc::channel();
            let _ = native_tx.send((operation, reply));
            result
        }));
        dialog.start(Operation::Status);
        dialog.start_native(NativeOperation::Inspect);
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
            native_dispatch: None,
            native_pending: None,
            native_report: None,
            native_message: "Start native work with /afk-work on in the attached Pi Session".into(),
            native_observed: None,
            native_stale: false,
            scroll: 0,
        }
    }
    fn start_native(&mut self, operation: NativeOperation) {
        if self.native_pending.is_some() {
            return;
        }
        let Some(dispatch) = &self.native_dispatch else {
            return;
        };
        self.native_pending = Some(dispatch(operation));
        self.native_observed = None;
        self.native_stale = false;
        self.scroll = 0;
        self.native_message =
            "Waiting for native inspection/withdrawal; this cannot grant work...".into();
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
        let mut native_changed = false;
        if let Some(rx) = &self.native_pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.native_message = match result {
                        Ok(report) => {
                            self.native_observed = Some(std::time::Instant::now());
                            let text = report.describe();
                            self.native_report = Some(report);
                            text
                        }
                        Err(error) => {
                            self.native_report = None;
                            format!("Native work unconfirmed: {error:#}")
                        }
                    };
                    self.native_pending = None;
                    native_changed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.native_pending = None;
                    self.native_report = None;
                    self.native_message = "Native inspection worker unavailable".into();
                    native_changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !self.native_stale
            && self
                .native_observed
                .is_some_and(|at| at.elapsed() >= afk::FRESHNESS)
        {
            self.native_stale = true;
            native_changed = true;
        }
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
        native_changed
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<()> {
        match key.code {
            KeyCode::Esc => return DialogResult::Cancel,
            KeyCode::Char('r') => {
                self.start(Operation::Status);
                self.start_native(NativeOperation::Inspect);
            }
            KeyCode::Char('o') => self.start(Operation::Off),
            KeyCode::Char('s') => {
                if !self.native_stale && self.native_observed.is_some() {
                    if let Some(report) = self.native_report.clone() {
                        self.start_native(NativeOperation::Stop(Box::new(report)));
                    }
                } else {
                    self.native_message =
                        "Inspect fresh native state before a targeted stop; o remains global Off"
                            .into();
                }
            }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
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
            " AFK / native work / restricted delegation ",
            theme.border,
            theme.title,
        );
        let (_, inner) = super::render_dialog_frame(frame, area, 86, 27, block);
        let stale = if self.stale {
            "STALE observation: press r to inspect again.\n"
        } else {
            ""
        };
        let native_stale = if self.native_stale {
            "STALE native observation: r to inspect again.\n"
        } else {
            ""
        };
        let text = format!(
            "{}\n{}{}\n\nAFK control-only / restricted delegation:\n{}{}",
            self.title, native_stale, self.native_message, stale, self.message
        );
        let content = inner.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(7)]).split(content);
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0))
                .style(Style::default().fg(theme.text)),
            parts[0],
        );
        let controls = format!("Expiry in minutes: {} (control-only)\nEnter: control-only on   o: off + bulk exclude\ns: stop native work   r: inspect both   PgUp/PgDn: scroll\nNative start/consent: /afk-work on in the attached Pi Session\nQuestions remain open. CLI allow-bulk grants no authority.\nEsc: close (not Off)", self.minutes.value());
        frame.render_widget(
            Paragraph::new(controls)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text)),
            parts[1],
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
