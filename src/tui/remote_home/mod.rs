//! Remote home screen for cross-machine structured-session attach.
//!
//! Each session row carries its daemon-derived context-resume state.
mod render;

use crate::session::repo_appearance::{RepoAppearances, MULTI_REPO_ID, SCRATCH_REPO_ID};
use crate::tui::dialogs::DialogResult;
use crate::tui::highlight::{HighlightFilterDialog, HighlightFilters};
use std::io::Stdout;

use crate::acp::client::discovery::DaemonEndpoint;
use crate::acp::client::HttpClient;
use crate::daemon::{ContextResumeAvailability, SessionResponse};
use crate::plugin::ui_state::UiSnapshot;
use crate::session::config::{resolve_theme_name, resolve_theme_palette_mode};
use crate::tui::styles::Theme;
use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEventKind};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

#[derive(Debug, Clone)]
pub struct RemoteSession {
    pub id: String,
    pub title: String,
    pub project_path: String,
    pub repo_id: String,
    pub color: Option<String>,
    pub trashed: bool,
    pub status: String,
    pub context_resume: Option<ContextResumeAvailability>,
}

pub struct RemoteHomeState {
    pub endpoint: DaemonEndpoint,
    client: HttpClient,
    pub sessions: Vec<RemoteSession>,
    all_sessions: Vec<RemoteSession>,
    pub appearances: RepoAppearances,
    pub filters: HighlightFilters,
    pub filter_dialog: Option<HighlightFilterDialog>,
    pub cursor: usize,
    pub status_text: Option<String>,
    pub last_error: Option<String>,
    pub loading: bool,
    pub plugin_ui: UiSnapshot,
}

impl RemoteHomeState {
    pub fn new(endpoint: DaemonEndpoint) -> Result<Self> {
        let client = HttpClient::new(endpoint.clone())?;
        Ok(Self {
            endpoint,
            client,
            sessions: Vec::new(),
            all_sessions: Vec::new(),
            appearances: Default::default(),
            filters: Default::default(),
            filter_dialog: None,
            cursor: 0,
            status_text: None,
            last_error: None,
            loading: true,
            plugin_ui: UiSnapshot::default(),
        })
    }

    fn rebuild(&mut self) {
        let selected = self.sessions.get(self.cursor).map(|s| s.id.clone());
        self.sessions = self
            .all_sessions
            .iter()
            .filter(|s| {
                s.trashed
                    || self.filters.matches(
                        s.color.as_deref(),
                        self.appearances.get(&s.repo_id).and_then(|a| a.color),
                    )
            })
            .cloned()
            .collect();
        self.cursor = selected
            .and_then(|id| self.sessions.iter().position(|s| s.id == id))
            .unwrap_or_else(|| self.cursor.min(self.sessions.len().saturating_sub(1)));
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let len = self.sessions.len();
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let cur = self.cursor as i32;
        let next = (cur + delta).rem_euclid(len as i32);
        self.cursor = next as usize;
    }
}

/// Set up alternate-screen terminal, run the remote home loop, tear it
/// down. Invoked from `tui::run` when `AOE_DAEMON_URL` is set.
pub async fn run_standalone(endpoint: DaemonEndpoint) -> Result<()> {
    use crossterm::event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    };
    use crossterm::execute;
    use crossterm::terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    };
    use std::io;
    use std::io::IsTerminal;

    if !io::stdin().is_terminal() {
        anyhow::bail!("stdin is not a terminal; `aoe` needs an interactive TTY");
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    // Push the kitty enhancement stack so the remote picker and the
    // structured-view it hands off to see `Shift+Enter` as a distinct
    // KeyEvent (#2362). Best-effort like `TerminalGuard::enter`; the
    // `AOE_DAEMON_URL` flow never enters via `TerminalGuard`.
    #[cfg(unix)]
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    );
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut event_stream = EventStream::new();
    let theme_name = resolve_theme_name();
    let palette_mode = resolve_theme_palette_mode();
    let theme = crate::tui::styles::load_theme_with_mode(&theme_name, palette_mode);

    let result = run(&mut terminal, &mut event_stream, &theme, endpoint).await;

    #[cfg(unix)]
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    event_stream: &mut EventStream,
    theme: &Theme,
    endpoint: DaemonEndpoint,
) -> Result<()> {
    let mut state = RemoteHomeState::new(endpoint)?;
    let mut pending = Some(start_refresh(&state));
    terminal.draw(|f| render::render(f, f.area(), theme, &state))?;
    #[cfg(feature = "e2e-tests")]
    crate::tui::app::e2e_render_ack(true)?;

    let mut refresh_timer = tokio::time::interval(std::time::Duration::from_secs(3));
    refresh_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    refresh_timer.tick().await;
    loop {
        let evt = match next_event(event_stream, &mut pending, &mut refresh_timer).await {
            RemoteEvent::Input(Some(event)) => event,
            RemoteEvent::Input(None) => return Ok(()),
            RemoteEvent::Refreshed(snapshot) => {
                pending = None;
                apply_snapshot(&mut state, snapshot);
                terminal.draw(|f| render::render(f, f.area(), theme, &state))?;
                continue;
            }
            RemoteEvent::Tick => {
                if pending.is_none() {
                    pending = Some(start_refresh(&state));
                }
                continue;
            }
        };
        let Ok(evt) = evt else { return Ok(()) };
        let CrosstermEvent::Key(key) = evt else {
            continue;
        };
        #[cfg(feature = "e2e-tests")]
        if key.code == KeyCode::F(12) && std::env::var_os("AOE_E2E_INPUT_BARRIER").is_some() {
            terminal.draw(|f| render::render(f, f.area(), theme, &state))?;
            crate::tui::app::e2e_render_ack(false)?;
            continue;
        }
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if let Some(dialog) = &mut state.filter_dialog {
            match dialog.handle_key(key) {
                DialogResult::Continue => {}
                DialogResult::Cancel => state.filter_dialog = None,
                DialogResult::Submit(filters) => {
                    state.filter_dialog = None;
                    state.filters = filters;
                    state.rebuild();
                }
            }
            terminal.draw(|f| render::render(f, f.area(), theme, &state))?;
            continue;
        }
        match key.code {
            KeyCode::F(4) => state.filter_dialog = Some(HighlightFilterDialog::new(&state.filters)),
            KeyCode::Esc if state.filters.active() => {
                state.filters = Default::default();
                state.rebuild();
            }
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('r') => {
                state.loading = true;
                state.status_text = Some("refreshing…".to_string());
                if pending.is_none() {
                    pending = Some(start_refresh(&state));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => state.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => state.move_cursor(-1),
            KeyCode::Enter => {
                if let Some(session) = state.sessions.get(state.cursor).cloned() {
                    let endpoint = state.endpoint.clone();
                    super::structured_view::run_for_endpoint(
                        terminal,
                        event_stream,
                        theme,
                        endpoint,
                        &session.id,
                    )
                    .await?;
                    // Avoid a cursor query that races the live event stream.
                    crate::tui::clear_terminal(terminal)?;
                }
            }
            _ => {}
        }
        terminal.draw(|f| render::render(f, f.area(), theme, &state))?;
    }
}

fn sessions_from_snapshot(wire_sessions: Vec<SessionResponse>) -> Vec<RemoteSession> {
    let mut sessions: Vec<_> = wire_sessions
        .into_iter()
        .filter(|session| session.view == crate::session::View::Structured)
        .map(|session| RemoteSession {
            id: session.id,
            title: session.title,
            repo_id: if session.scratch {
                SCRATCH_REPO_ID.into()
            } else if session.workspace_repos.len() > 1 {
                MULTI_REPO_ID.into()
            } else {
                session
                    .main_repo_path
                    .clone()
                    .unwrap_or_else(|| session.project_path.clone())
            },
            project_path: session.project_path,
            color: session.color,
            trashed: session.trashed_at.is_some(),
            status: session.status,
            context_resume: session.context_resume,
        })
        .collect();
    sessions.sort_by(|a, b| a.title.cmp(&b.title));
    sessions
}

fn apply_session_result(state: &mut RemoteHomeState, result: Result<Vec<RemoteSession>, String>) {
    match result {
        Ok(sessions) => {
            state.all_sessions = sessions;
            state.rebuild();
            state.status_text = Some(format!("{} session(s)", state.sessions.len()));
        }
        Err(error) => {
            state.sessions.clear();
            state.all_sessions.clear();
            state.cursor = 0;
            state.last_error = Some(error);
            state.status_text = None;
        }
    }
}
struct RemoteSnapshot {
    sessions: Result<Vec<RemoteSession>, String>,
    appearances: Result<RepoAppearances, crate::acp::client::HttpError>,
    plugin_ui: Result<UiSnapshot, crate::acp::client::HttpError>,
}
type RefreshFuture = std::pin::Pin<Box<dyn std::future::Future<Output = RemoteSnapshot> + Send>>;

enum RemoteEvent {
    Input(Option<std::io::Result<CrosstermEvent>>),
    Refreshed(RemoteSnapshot),
    Tick,
}

async fn next_event<S>(
    events: &mut S,
    pending: &mut Option<RefreshFuture>,
    timer: &mut tokio::time::Interval,
) -> RemoteEvent
where
    S: futures_util::Stream<Item = std::io::Result<CrosstermEvent>> + Unpin,
{
    tokio::select! {
        event = events.next() => RemoteEvent::Input(event),
        snapshot = async { pending.as_mut().expect("guarded refresh").await }, if pending.is_some() => RemoteEvent::Refreshed(snapshot),
        _ = timer.tick() => RemoteEvent::Tick,
    }
}

fn start_refresh(state: &RemoteHomeState) -> RefreshFuture {
    let endpoint = state.endpoint.clone();
    let client = state.client.clone();
    Box::pin(async move {
        let sessions = async {
            match endpoint.daemon_client() {
                Ok(client) => client
                    .list_sessions(None)
                    .await
                    .map(|envelope| sessions_from_snapshot(envelope.sessions))
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            }
        };
        let (sessions, appearances, plugin_ui) = tokio::join!(
            sessions,
            client.repo_appearances(),
            client.plugin_ui_state()
        );
        RemoteSnapshot {
            sessions,
            appearances,
            plugin_ui,
        }
    })
}

fn apply_snapshot(state: &mut RemoteHomeState, snapshot: RemoteSnapshot) {
    state.last_error = None;
    match snapshot.appearances {
        Ok(map) => state.appearances = map,
        Err(error) => {
            tracing::warn!(target: "tui.remote_home", "repository appearance fetch failed: {error}")
        }
    }
    apply_session_result(state, snapshot.sessions);
    state.plugin_ui = match snapshot.plugin_ui {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::debug!(target: "tui.remote_home", "plugin ui-state fetch failed: {error}");
            UiSnapshot::default()
        }
    };
    state.loading = false;
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::client::discovery::Source;

    fn session(id: &str) -> RemoteSession {
        RemoteSession {
            id: id.to_string(),
            title: id.to_string(),
            project_path: format!("/tmp/{id}"),
            repo_id: format!("/tmp/{id}"),
            color: None,
            trashed: false,
            status: "Stopped".to_string(),
            context_resume: Some(ContextResumeAvailability::Available),
        }
    }

    fn wire(id: &str, view: crate::session::View) -> SessionResponse {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": id,
            "project_path": format!("/tmp/{id}"),
            "status": "Stopped",
            "view": view,
            "context_resume": { "state": "available" },
        }))
        .unwrap()
    }

    fn state() -> RemoteHomeState {
        RemoteHomeState::new(DaemonEndpoint::new(
            "http://127.0.0.1:8080".to_string(),
            None,
            Source::Env,
        ))
        .unwrap()
    }

    #[test]
    fn snapshot_failure_clears_stale_sessions() {
        let mut state = state();
        state.sessions = vec![session("stale")];
        state.cursor = 4;
        state.status_text = Some("stale status".to_string());

        apply_session_result(&mut state, Err("daemon unavailable".to_string()));

        assert!(state.sessions.is_empty());
        assert_eq!(state.cursor, 0);
        assert_eq!(state.last_error.as_deref(), Some("daemon unavailable"));
        assert!(state.status_text.is_none());
    }

    #[test]
    fn successful_snapshot_restores_sorted_structured_scope() {
        let sessions = sessions_from_snapshot(vec![
            wire("second", crate::session::View::Structured),
            wire("terminal", crate::session::View::Terminal),
            wire("first", crate::session::View::Structured),
        ]);

        let mut state = state();
        apply_session_result(&mut state, Ok(sessions));
        assert_eq!(
            state
                .sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }
}

#[cfg(test)]
mod highlight_tests {
    use super::*;
    use crate::session::repo_appearance::{RepoAppearance, RepoColor};
    #[tokio::test]
    async fn slow_highlight_refresh_does_not_block_terminal_input() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let mut pending: Option<RefreshFuture> = Some(Box::pin(async move {
            entered_tx.send(()).unwrap();
            finish_rx.await.unwrap()
        }));
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut events = futures_util::stream::poll_fn(|cx| input_rx.poll_recv(cx));
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(3600));
        timer.tick().await;
        let producer = tokio::spawn(async move {
            entered_rx.await.unwrap();
            input_tx
                .send(Ok(CrosstermEvent::Key(KeyCode::Char('q').into())))
                .unwrap();
            input_tx
        });
        let event = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            next_event(&mut events, &mut pending, &mut timer),
        )
        .await
        .unwrap();
        assert!(
            matches!(event,RemoteEvent::Input(Some(Ok(CrosstermEvent::Key(key)))) if key.code == KeyCode::Char('q'))
        );
        let _input_tx = producer.await.unwrap();
        assert!(pending.is_some());
        assert!(finish_tx
            .send(RemoteSnapshot {
                sessions: Ok(vec![]),
                appearances: Ok(Default::default()),
                plugin_ui: Ok(Default::default())
            })
            .is_ok());
        assert!(matches!(
            next_event(&mut events, &mut pending, &mut timer).await,
            RemoteEvent::Refreshed(_)
        ));
    }

    #[test]
    fn projection_filters_shared_paths_and_preserves_selection_on_refresh() {
        let row = |id: &str, extra: serde_json::Value| {
            let mut v = serde_json::json!({"id":id,"title":id,"project_path":"/worktree","status":"Stopped","view":"structured","main_repo_path":"/repo","color":"red"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value(v).unwrap()
        };
        let rows = sessions_from_snapshot(vec![
            row("a", serde_json::json!({})),
            row("b", serde_json::json!({"scratch":true})),
            row(
                "c",
                serde_json::json!({"workspace_repos":[{"name":"a","source_path":"/a","branch":"main"},{"name":"b","source_path":"/b","branch":"main"}]}),
            ),
        ]);
        assert_eq!(
            rows.iter().map(|s| s.repo_id.as_str()).collect::<Vec<_>>(),
            ["/repo", SCRATCH_REPO_ID, MULTI_REPO_ID]
        );
        let mut state = RemoteHomeState::new(DaemonEndpoint::new(
            "http://127.0.0.1:8080".into(),
            None,
            crate::acp::client::discovery::Source::Env,
        ))
        .unwrap();
        state.appearances.insert(
            "/repo".into(),
            RepoAppearance {
                color: Some(RepoColor::Sky),
                alias: None,
            },
        );
        apply_session_result(&mut state, Ok(rows.clone()));
        state.cursor = 1;
        apply_session_result(&mut state, Ok(vec![rows[2].clone(), rows[1].clone()]));
        assert_eq!(state.sessions[state.cursor].id, "b");
        apply_session_result(&mut state, Ok(rows));
        state.filters.projects = vec![Some(RepoColor::Sky)];
        state.filters.sessions = vec![Some("red".into())];
        state.rebuild();
        assert_eq!(
            state
                .sessions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["a"]
        );
        state.appearances.clear();
        state.rebuild();
        assert!(state.sessions.is_empty());
        state.filters = Default::default();
        state.rebuild();
        assert_eq!(state.sessions.len(), 3);
    }
}
