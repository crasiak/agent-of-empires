//! fzf-style session search overlay: one line per hit with the best hit nearest
//! the prompt, an info line, and a preview pane on wide terminals that shows
//! the exchange around the hit, read from its transcript.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

use super::{paste_into_input, DialogResult};
use crate::session_search::context::{self, Role, Turn};
use crate::session_search::{
    group, supervisor, Hit, QueryError, Request, Row, Scope, Session, Target,
};
use crate::tui::components::{fixed_width, set_prefixed_input_cursor_position, truncate_to_width};
use crate::tui::styles::Theme;
use crate::tui::worker::Worker;

const DEBOUNCE: Duration = Duration::from_millis(300);
const MIN_QUERY_CHARS: usize = 3;
const MAX_ROWS: usize = 50;
/// Below this width the preview is off by default and the folder column drops.
const WIDE: u16 = 110;
/// Below this width the date column drops too.
const NARROW: u16 = 80;
const FOLDER_COLS: usize = 16;
const LABEL_COLS: usize = 28;
const HARNESS_COLS: usize = 6;
const DATE_COLS: usize = 5;
const MARKER_COLS: usize = 4;
/// Turns read either side of a hit for the preview.
const CONTEXT_TURNS: usize = 3;
/// Other matches listed under the preview's exchange.
const ALSO: usize = 2;
/// Preview gutter: hit bar, role, time.
const GUTTER: usize = 11;
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub enum SearchOutcome {
    Open(Target),
    Copy(String),
    OpenTranscript(PathBuf),
}

type Reply = (u64, Result<Vec<Hit>, QueryError>);
type Runner = Box<dyn FnMut(Request) -> Reply + Send>;
type Resolver = Box<dyn Fn(&Session) -> Target + Send>;

/// A visible list entry: a row index, plus a child index when expanded.
type Entry = (usize, Option<usize>);

/// Debounce state. An edit is stamped by the next `tick`, which knows the time.
enum Pending {
    Idle,
    Edited,
    Due(Instant),
}

pub struct SessionSearchDialog {
    input: Input,
    scope: Scope,
    here_repo: Option<String>,
    rows: Vec<Row>,
    /// Session keys whose target is a jump, resolved once per result.
    jumps: HashSet<String>,
    /// Rows whose other contexts are listed inline.
    expanded: HashSet<usize>,
    /// Index into `visible()`; 0 is nearest the prompt.
    cursor: usize,
    /// Unset until the first render picks a default from the width.
    preview_on: Option<bool>,
    error: Option<QueryError>,
    /// Latest generation requested; older results are dropped.
    generation: u64,
    /// `generation`, shared with the worker so it skips queued stale requests.
    latest: Arc<AtomicU64>,
    in_flight: usize,
    pending: Pending,
    spin: usize,
    worker: Worker<Request, Reply>,
    resolver: Resolver,
    /// Turns around the selected hit, keyed by transcript and offset.
    context: Option<((String, u64), Vec<Turn>)>,
}

impl SessionSearchDialog {
    pub fn new(here_repo: Option<String>, mut runner: Runner, resolver: Resolver) -> Self {
        let latest = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&latest);
        // The worker runs one request at a time; anything superseded while it
        // waited answers empty at once rather than running to its timeout.
        let worker = Worker::spawn("aoe-session-search", move |req: Request| {
            if req.generation < seen.load(Ordering::Relaxed) {
                return (req.generation, Ok(Vec::new()));
            }
            runner(req)
        });
        Self {
            input: Input::default(),
            scope: Scope::All,
            here_repo,
            rows: Vec::new(),
            jumps: HashSet::new(),
            expanded: HashSet::new(),
            cursor: 0,
            preview_on: None,
            error: None,
            generation: 0,
            latest,
            in_flight: 0,
            pending: Pending::Idle,
            spin: 0,
            worker,
            resolver,
            context: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<SearchOutcome> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return DialogResult::Cancel,
            KeyCode::Up => self.step(1),
            KeyCode::Down => self.step(-1),
            KeyCode::Char('k') if ctrl => self.step(1),
            KeyCode::Char('j') if ctrl => self.step(-1),
            KeyCode::Right => self.set_expanded(true),
            KeyCode::Left => self.set_expanded(false),
            KeyCode::Tab => self.cycle_scope(),
            // Legacy terminals send Ctrl-/ as Ctrl-7.
            KeyCode::Char('/') | KeyCode::Char('7') if ctrl => {
                self.preview_on = Some(!self.preview_on.unwrap_or(false));
            }
            KeyCode::Enter => {
                if let Some(hit) = self.selected() {
                    return DialogResult::Submit(SearchOutcome::Open((self.resolver)(
                        &hit.session,
                    )));
                }
            }
            KeyCode::Char('y') if ctrl => {
                if let Some(resume) = self.selected().and_then(|h| h.session.resume.as_ref()) {
                    return DialogResult::Submit(SearchOutcome::Copy(resume.command.clone()));
                }
            }
            KeyCode::Char('o') if ctrl => {
                if let Some(path) = self
                    .selected()
                    .and_then(|h| h.session.transcript_path.as_ref())
                {
                    return DialogResult::Submit(SearchOutcome::OpenTranscript(PathBuf::from(
                        path,
                    )));
                }
            }
            _ => {
                let before = self.input.value().to_string();
                self.input.handle_event(&crossterm::event::Event::Key(key));
                if self.input.value() != before {
                    self.edited();
                }
            }
        }
        DialogResult::Continue
    }

    pub fn handle_paste(&mut self, text: &str) {
        paste_into_input(&mut self.input, text);
        self.edited();
    }

    /// Fires a due debounced query and drains results; true when a repaint is needed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut repaint = false;
        if matches!(self.pending, Pending::Edited) {
            self.pending = Pending::Due(now + DEBOUNCE);
        }
        if matches!(self.pending, Pending::Due(at) if now >= at) {
            self.send();
            repaint = true;
        }
        loop {
            match self.worker.try_recv() {
                Ok((generation, result)) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    if generation >= self.generation {
                        self.apply(result);
                        repaint = true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.in_flight > 0 {
                        self.in_flight = 0;
                        self.error = Some(QueryError::Failed("search worker stopped".into()));
                        repaint = true;
                    }
                    break;
                }
            }
        }
        if self.in_flight > 0 {
            self.spin = self.spin.wrapping_add(1);
            repaint = true;
        }
        repaint
    }

    /// A query is still running, for tests that wait on the worker.
    #[cfg(test)]
    pub(crate) fn busy(&self) -> bool {
        self.in_flight > 0
    }

    fn query(&self) -> &str {
        self.input.value().trim()
    }

    fn query_ready(&self) -> bool {
        self.query().chars().count() >= MIN_QUERY_CHARS
    }

    fn edited(&mut self) {
        if self.query_ready() {
            self.pending = Pending::Edited;
            return;
        }
        // Too short to search: clear, and orphan any query still running.
        self.pending = Pending::Idle;
        self.bump();
        self.rows.clear();
        self.expanded.clear();
        self.cursor = 0;
        self.error = None;
    }

    fn bump(&mut self) {
        self.generation += 1;
        self.latest.store(self.generation, Ordering::Relaxed);
    }

    fn send(&mut self) {
        self.pending = Pending::Idle;
        self.bump();
        self.in_flight += 1;
        self.worker.request(Request {
            generation: self.generation,
            text: self.query().to_string(),
            scope: self.scope.clone(),
        });
    }

    fn apply(&mut self, result: Result<Vec<Hit>, QueryError>) {
        self.expanded.clear();
        self.cursor = 0;
        match result {
            Ok(hits) => {
                self.rows = group(hits, MAX_ROWS);
                self.jumps = self
                    .rows
                    .iter()
                    .flat_map(|row| std::iter::once(&row.best).chain(&row.others))
                    .filter(|hit| matches!((self.resolver)(&hit.session), Target::Jump(_)))
                    .map(|hit| hit.session.session_key.clone())
                    .collect();
                self.error = None;
            }
            Err(e) => {
                self.rows.clear();
                self.error = Some(e);
            }
        }
    }

    fn cycle_scope(&mut self) {
        self.scope = match (&self.scope, &self.here_repo) {
            (Scope::All, Some(repo)) => Scope::Here(repo.clone()),
            (Scope::All, None) | (Scope::Here(_), _) => Scope::Recent,
            (Scope::Recent, _) => Scope::All,
        };
        if self.query_ready() {
            self.send();
        }
    }

    /// Entries in display order from the prompt upward. Children sit between
    /// their parent and the prompt, so they read top-down under it.
    fn visible(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for (r, row) in self.rows.iter().enumerate() {
            if self.expanded.contains(&r) {
                out.extend((0..row.others.len()).rev().map(|c| (r, Some(c))));
            }
            out.push((r, None));
        }
        out
    }

    fn hit(&self, (r, c): Entry) -> &Hit {
        let row = &self.rows[r];
        c.map_or(&row.best, |c| &row.others[c])
    }

    fn selected(&self) -> Option<&Hit> {
        let entry = *self.visible().get(self.cursor)?;
        Some(self.hit(entry))
    }

    fn step(&mut self, delta: isize) {
        let last = self.visible().len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    fn set_expanded(&mut self, on: bool) {
        let Some(&(r, _)) = self.visible().get(self.cursor) else {
            return;
        };
        if on && !self.rows[r].others.is_empty() {
            self.expanded.insert(r);
        } else if !on {
            self.expanded.remove(&r);
        }
        // Keep the group's own row selected as entries shift around it.
        self.cursor = self
            .visible()
            .iter()
            .position(|e| *e == (r, None))
            .unwrap_or(0);
    }

    fn scope_label(&self) -> String {
        match &self.scope {
            Scope::All => "all·30d".to_string(),
            Scope::Here(repo) => format!("here:{repo}"),
            Scope::Recent => "recent·7d".to_string(),
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (window, area) = window(area);
        frame.render_widget(Clear, window);
        let block = Block::default()
            .style(Style::default().bg(theme.background).fg(theme.text))
            .border_style(Style::default().fg(theme.accent));
        let block = if window == area {
            block
        } else {
            block
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Search sessions ")
        };
        frame.render_widget(block, window);
        let preview_on = *self.preview_on.get_or_insert(area.width >= WIDE);
        let (main, preview) = if preview_on {
            let [main, sep, preview] = Layout::horizontal([
                Constraint::Percentage(55),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(area);
            let bar = vec![Line::from("│"); sep.height as usize];
            frame.render_widget(
                Paragraph::new(bar).style(Style::default().fg(theme.border)),
                sep,
            );
            (main, Some(preview))
        } else {
            (area, None)
        };
        let [list, info, prompt] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(main);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("> ", Style::default().fg(theme.accent).bold()),
                Span::styled(self.input.value(), Style::default().fg(theme.text)),
            ])),
            prompt,
        );
        set_prefixed_input_cursor_position(frame, prompt, "> ", &self.input);

        let shown = self.render_list(frame, list, area.width, theme);
        let spinner = if self.in_flight > 0 {
            SPINNER[self.spin % SPINNER.len()]
        } else {
            ' '
        };
        let info_text = format!(
            "{spinner} {shown}/{}  {}",
            self.rows.len(),
            self.scope_label()
        );
        frame.render_widget(
            Paragraph::new(info_text).style(Style::default().fg(theme.dimmed)),
            info,
        );

        if let Some(preview) = preview {
            self.load_context();
            self.render_preview(frame, preview, theme);
        } else if let Some(e) = &self.error {
            let bottom = Rect {
                y: list.bottom().saturating_sub(1),
                height: list.height.min(1),
                ..list
            };
            frame.render_widget(error_line(e, theme), bottom);
        }
    }

    /// Draws the list bottom-up; returns how many groups are on screen.
    fn render_list(&self, frame: &mut Frame, list: Rect, width: u16, theme: &Theme) -> usize {
        let height = list.height as usize;
        if height == 0 {
            return 0;
        }
        let visible = self.visible();
        let start = self.cursor.saturating_sub(height - 1);
        let window = &visible[start..visible.len().min(start + height)];
        let show_folder = width >= WIDE;
        let show_date = width >= NARROW;
        let label_cols = window
            .iter()
            .map(|&e| label(&self.hit(e).session, show_folder).chars().count())
            .max()
            .unwrap_or(0)
            .min(LABEL_COLS);
        let terms = self.terms();
        let today = Local::now().date_naive();
        let stripe = theme.highlight_background(theme.dimmed);

        for (i, &entry) in window.iter().enumerate() {
            let session = &self.hit(entry).session;
            let selected = start + i == self.cursor;
            let base = if selected {
                Style::default()
                    .fg(theme.text)
                    .add_modifier(Modifier::REVERSED)
            } else if (start + i) % 2 == 1 {
                Style::default().fg(theme.text).bg(stripe)
            } else {
                Style::default().fg(theme.text)
            };
            let meta = if selected {
                base
            } else {
                base.fg(theme.dimmed)
            };
            let name = match entry.1 {
                Some(_) => "  └".to_string(),
                None => label(session, show_folder),
            };
            let mut cols = format!(
                "{} {}",
                fixed_width(&name, label_cols),
                fixed_width(&session.harness, HARNESS_COLS)
            );
            if show_date {
                cols.push(' ');
                cols.push_str(&fixed_width(
                    &short_date(&session.last_ts, today),
                    DATE_COLS,
                ));
            }
            let live = self.jumps.contains(&session.session_key);
            let others = match entry.1 {
                None => self.rows[entry.0].others.len(),
                Some(_) => 0,
            };
            let dot = if live { "●" } else { " " };
            let marker = match others {
                0 => dot.to_string(),
                n => format!("{dot} {n}"),
            };
            cols.push(' ');
            cols.push_str(&fixed_width(&marker, MARKER_COLS));
            cols.push(' ');

            let bar = if selected {
                Span::styled("▌", Style::default().fg(theme.accent).bg(theme.background))
            } else {
                Span::styled(" ", base)
            };
            let used = 1 + cols.chars().count();
            let room = (list.width as usize).saturating_sub(used);
            let title = session
                .title
                .as_deref()
                .or_else(|| self.hit(entry).snippets.first().map(|s| s.text.as_str()))
                .unwrap_or("");
            let title = fixed_width(title, room);
            let mut spans = vec![bar, Span::styled(cols, meta)];
            spans.extend(highlight(&title, &terms, base));
            let y = list.bottom() - 1 - i as u16;
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect {
                    y,
                    height: 1,
                    ..list
                },
            );
        }
        window.iter().filter(|e| e.1.is_none()).count()
    }

    /// Reads the turns around the selected hit's best snippet, once per selection.
    fn load_context(&mut self) {
        let key = self.selected().and_then(|hit| {
            let path = hit.session.transcript_path.clone()?;
            let offset = hit.snippets.first()?.offset?;
            Some((path, offset))
        });
        let Some(key) = key else {
            self.context = None;
            return;
        };
        if self.context.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let turns = context::around(
            std::path::Path::new(&key.0),
            key.1,
            CONTEXT_TURNS,
            CONTEXT_TURNS,
        )
        .unwrap_or_default();
        self.context = Some((key, turns));
    }

    fn render_preview(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default().padding(Padding::horizontal(1));
        let inner = block.inner(area);
        if let Some(e) = &self.error {
            frame.render_widget(
                error_line(e, theme),
                Rect {
                    height: inner.height.min(1),
                    ..inner
                },
            );
            return;
        }
        let Some(hit) = self.selected() else {
            return;
        };
        let s = &hit.session;
        let width = inner.width as usize;
        let dim = Style::default().fg(theme.dimmed);
        let terms = self.terms();
        let today = Local::now().date_naive();
        let mut lines = Vec::new();
        match &s.resume {
            Some(r) => lines.extend(
                clamp(wrap(&r.command, width), 2)
                    .into_iter()
                    .map(|l| Line::styled(l, Style::default().fg(theme.text))),
            ),
            None => lines.push(Line::styled("no resume command", dim)),
        }
        let place: Vec<String> = [
            s.cwd.as_deref().map(tilde),
            s.branch.clone(),
            s.profile.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        lines.push(Line::styled(fixed_width(&place.join(" · "), width), dim));
        lines.push(Line::styled("─".repeat(width), dim));

        let also: Vec<_> = hit.snippets.iter().skip(1).take(ALSO).collect();
        let room = (inner.height as usize).saturating_sub(lines.len() + also.len());
        let turns = self
            .context
            .as_ref()
            .map(|(_, turns)| turns.as_slice())
            .filter(|turns| turns.iter().any(|t| t.hit));
        match turns {
            Some(turns) => {
                lines.extend(exchange(turns, width, room, &terms, today, theme));
            }
            // No transcript to read: the snippets are all there is.
            None => {
                for snippet in hit.snippets.iter().take(1) {
                    let turn = Turn {
                        role: snippet_role(&snippet.role),
                        ts: Some(snippet.ts.clone()),
                        text: snippet.text.clone(),
                        hit: true,
                    };
                    lines.extend(exchange(&[turn], width, room, &terms, today, theme));
                }
            }
        }
        if !also.is_empty() {
            let pad = (inner.height as usize).saturating_sub(lines.len() + also.len());
            lines.extend(std::iter::repeat_n(Line::default(), pad));
        }
        for snippet in also {
            let head = format!("also {:<9} ", stamp(&snippet.ts));
            let text = fixed_width(&one_line(&snippet.text), width.saturating_sub(head.len()));
            let mut spans = vec![Span::styled(head, dim)];
            spans.extend(highlight(&text, &terms, dim));
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn terms(&self) -> Vec<String> {
        self.query()
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .collect()
    }
}

/// The overlay's window and its inner area: a bordered window with a margin
/// that shows the screen behind it, as the tailored-sessions window does.
/// Small terminals get the whole area, unbordered.
fn window(area: Rect) -> (Rect, Rect) {
    if area.width < 60 || area.height < 12 {
        return (area, area);
    }
    let mx = (area.width / 16).max(2);
    let my = (area.height / 12).max(1);
    let outer = Rect {
        x: area.x + mx,
        y: area.y + my,
        width: area.width - 2 * mx,
        height: area.height - 2 * my,
    };
    let inner = Rect {
        x: outer.x + 1,
        y: outer.y + 1,
        width: outer.width - 2,
        height: outer.height - 2,
    };
    (outer, inner)
}

fn snippet_role(role: &str) -> Role {
    match role {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => Role::ToolResult,
    }
}

/// The hit's turn and its neighbours in `room` lines, in transcript order.
/// The nearest turn either side goes in first, then the hit from its first
/// match, then further turns while they fit. Neighbours get one line (tool
/// calls) or two; the hit keeps its line breaks.
fn exchange(
    turns: &[Turn],
    width: usize,
    room: usize,
    terms: &[String],
    today: chrono::NaiveDate,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let text_cols = width.saturating_sub(GUTTER).max(1);
    let Some(first_hit) = turns.iter().position(|t| t.hit) else {
        return Vec::new();
    };
    let last_hit = turns.iter().rposition(|t| t.hit).unwrap_or(first_hit);
    let body: Vec<Vec<String>> = turns
        .iter()
        .map(|t| {
            if t.hit {
                hit_lines(&t.text, terms, text_cols)
            } else {
                let cap = if t.role == Role::Tool { 1 } else { 2 };
                clamp(wrap(&one_line(&t.text), text_cols), cap)
            }
        })
        .collect();

    // Nearest-first neighbour order: before, after, before, after...
    let mut order = Vec::new();
    let (mut b, mut a) = (first_hit, last_hit + 1);
    while b > 0 || a < turns.len() {
        if b > 0 {
            b -= 1;
            order.push(b);
        }
        if a < turns.len() {
            order.push(a);
            a += 1;
        }
    }
    let mut take = vec![0usize; turns.len()];
    let mut left = room;
    // Keep at least three lines for the hit when reserving its first neighbours.
    for &i in order.iter().take(2) {
        if left >= body[i].len() + 3 {
            take[i] = body[i].len();
            left -= take[i];
        }
    }
    for i in first_hit..=last_hit {
        take[i] = body[i].len().min(left);
        left -= take[i];
    }
    for &i in order.iter().skip(2) {
        if left >= body[i].len() {
            take[i] = body[i].len();
            left -= take[i];
        }
    }

    let mut out = Vec::new();
    for (i, turn) in turns.iter().enumerate() {
        if take[i] == 0 {
            continue;
        }
        let (label, label_style) = match turn.role {
            Role::User => ("you", Style::default().fg(theme.accent)),
            Role::Assistant => ("ai", Style::default().fg(theme.dimmed)),
            Role::Tool => ("▸", Style::default().fg(theme.dimmed)),
            Role::ToolResult => ("↳", Style::default().fg(theme.dimmed)),
        };
        let text_style = if turn.hit {
            Style::default().fg(theme.text)
        } else {
            Style::default().fg(theme.dimmed)
        };
        let label_style = if turn.hit {
            label_style.fg(theme.accent).bold()
        } else {
            label_style
        };
        let when = turn
            .ts
            .as_deref()
            .map(|ts| short_date(ts, today))
            .unwrap_or_default();
        for (n, text) in body[i].iter().take(take[i]).enumerate() {
            let bar = if turn.hit {
                Span::styled("▌", Style::default().fg(theme.accent))
            } else {
                Span::raw(" ")
            };
            let head = if n == 0 {
                format!(
                    "{} {} ",
                    fixed_width(label, 3),
                    fixed_width(&when, DATE_COLS)
                )
            } else {
                " ".repeat(GUTTER - 1)
            };
            let mut spans = vec![bar, Span::styled(head, label_style)];
            spans.extend(highlight(text, terms, text_style));
            out.push(Line::from(spans));
        }
    }
    out
}

/// The hit's text wrapped to `cols`, starting a line above the first match
/// so the matching part shows even in a long message.
fn hit_lines(text: &str, terms: &[String], cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut blank = false;
    for raw in text.lines() {
        let raw = raw.trim_end();
        if raw.is_empty() {
            if !blank && !lines.is_empty() {
                lines.push(String::new());
            }
            blank = true;
            continue;
        }
        blank = false;
        lines.extend(wrap(raw, cols));
    }
    let first = lines.iter().position(|l| {
        let l = l.to_ascii_lowercase();
        terms
            .iter()
            .any(|t| !t.is_empty() && l.contains(t.as_str()))
    });
    match first {
        Some(i) if i > 1 => {
            let mut rest = lines.split_off(i - 1);
            rest.insert(0, "…".to_string());
            rest
        }
        _ => lines,
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keeps `cap` lines, marking a cut with `…`.
fn clamp(mut lines: Vec<String>, cap: usize) -> Vec<String> {
    if lines.len() > cap {
        lines.truncate(cap);
        if let Some(last) = lines.last_mut() {
            last.push('…');
        }
    }
    lines
}

/// Greedy word wrap by display width; words wider than `cols` are split.
fn wrap(text: &str, cols: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split(' ') {
        let w: usize = word.chars().map(|c| c.width().unwrap_or(0)).sum();
        if used > 0 && used + 1 + w > cols {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        if used > 0 {
            line.push(' ');
            used += 1;
        }
        for c in word.chars() {
            let cw = c.width().unwrap_or(0);
            if used + cw > cols && used > 0 {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(c);
            used += cw;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// `repo·folder`, folder being the last segment of `cwd`; just `repo` when narrow.
fn label(s: &Session, show_folder: bool) -> String {
    let folder = s
        .cwd
        .as_deref()
        .and_then(|cwd| cwd.trim_end_matches('/').rsplit('/').next())
        .filter(|f| show_folder && !f.is_empty());
    match folder {
        Some(f) => format!("{}·{}", s.repo, truncate_to_width(f, FOLDER_COLS)),
        None => s.repo.clone(),
    }
}

fn local_time(ts: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&Local))
}

/// `HH:MM` for today, else `MM-DD`, in local time.
fn short_date(ts: &str, today: chrono::NaiveDate) -> String {
    match local_time(ts) {
        Some(t) if t.date_naive() == today => t.format("%H:%M").to_string(),
        Some(t) => t.format("%m-%d").to_string(),
        None => String::new(),
    }
}

fn stamp(ts: &str) -> String {
    local_time(ts).map_or_else(String::new, |t| t.format("%m-%d %H:%M").to_string())
}

fn tilde(path: &str) -> String {
    let home = dirs::home_dir();
    match home
        .as_deref()
        .and_then(|h| std::path::Path::new(path).strip_prefix(h).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.to_string(),
    }
}

/// Splits `text` into spans, bolding case-insensitive matches of `terms`.
fn highlight(text: &str, terms: &[String], base: Style) -> Vec<Span<'static>> {
    // ASCII lowercasing keeps byte offsets, so marks index `text` directly.
    let lower = text.to_ascii_lowercase();
    let mut marked = vec![false; text.len()];
    for term in terms.iter().filter(|t| !t.is_empty()) {
        for (at, _) in lower.match_indices(term.as_str()) {
            marked[at..at + term.len()].fill(true);
        }
    }
    let flips = text
        .char_indices()
        .map(|(i, _)| i)
        .filter(|&i| i > 0 && marked[i] != marked[i - 1]);
    let mut spans = Vec::new();
    let mut start = 0;
    for end in flips.chain([text.len()]) {
        if end > start {
            let style = if marked[start] {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            };
            spans.push(Span::styled(text[start..end].to_string(), style));
        }
        start = end;
    }
    spans
}

fn error_line(e: &QueryError, theme: &Theme) -> Paragraph<'static> {
    let mut text = match e {
        QueryError::NotInstalled => "session-search is not installed".to_string(),
        QueryError::NotRunning(m) => format!("session-search is not running: {m}"),
        QueryError::Timeout => "search timed out".to_string(),
        QueryError::BadOutput(m) => format!("unexpected session-search output: {m}"),
        QueryError::Failed(m) => format!("search failed: {m}"),
    };
    if matches!(e, QueryError::NotInstalled | QueryError::NotRunning(_)) {
        if let Some(last) = supervisor::last_error() {
            text.push_str(&format!(" ({last})"));
        }
    }
    Paragraph::new(text).style(Style::default().fg(theme.error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_search::parse;
    use crate::tui::dialogs::test_keys::{ctrl_key, key};
    use crate::tui::dialogs::test_render::draw;
    use crossterm::event::KeyCode;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type Reply = Box<dyn FnMut(&Request) -> (u64, Result<Vec<Hit>, QueryError>) + Send>;
    type Sent = Arc<Mutex<Vec<(String, Scope)>>>;

    fn fixture() -> Vec<Hit> {
        parse(include_bytes!(
            "../../session_search/testdata/query_v1.json"
        ))
        .unwrap()
    }

    fn honest() -> Reply {
        Box::new(|req| (req.generation, Ok(fixture())))
    }

    fn dialog(here: Option<&str>, mut reply: Reply) -> (SessionSearchDialog, Sent) {
        let sent: Sent = Arc::default();
        let log = Arc::clone(&sent);
        let runner: Runner = Box::new(move |req: Request| {
            log.lock()
                .unwrap()
                .push((req.text.clone(), req.scope.clone()));
            reply(&req)
        });
        // A live pane jumps; anything else copies its key, so tests can tell rows apart.
        let resolver: Resolver = Box::new(|s: &Session| match &s.aoe_instance_id {
            Some(id) => Target::Jump(id.clone()),
            None => Target::CopyOnly(s.session_key.clone()),
        });
        (
            SessionSearchDialog::new(here.map(String::from), runner, resolver),
            sent,
        )
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn type_text(d: &mut SessionSearchDialog, text: &str) {
        for c in text.chars() {
            assert!(matches!(
                d.handle_key(key(KeyCode::Char(c))),
                DialogResult::Continue
            ));
        }
    }

    /// Drain the worker until no query is in flight.
    fn settle(d: &mut SessionSearchDialog, now: Instant) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while d.in_flight > 0 {
            assert!(Instant::now() < deadline, "query never answered");
            d.tick(now);
            std::thread::yield_now();
        }
    }

    /// A dialog holding the fixture's results for `abc`.
    fn loaded() -> SessionSearchDialog {
        let (mut d, _) = dialog(None, honest());
        let t = Instant::now();
        type_text(&mut d, "abc");
        d.tick(t);
        d.tick(t + ms(301));
        settle(&mut d, t + ms(301));
        d
    }

    /// The rows inside the overlay's window, cropped to it.
    fn screen(d: &mut SessionSearchDialog, width: u16, height: u16) -> Vec<String> {
        let buf = draw(width, height, |f, theme| d.render(f, f.area(), theme));
        let (_, inner) = window(Rect::new(0, 0, width, height));
        (inner.y..inner.bottom())
            .map(|y| {
                (inner.x..inner.right())
                    .map(|x| buf[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// The fixture with its first hit pointed at `transcript`, offset `at`.
    fn loaded_with_transcript(transcript: &std::path::Path, at: u64) -> SessionSearchDialog {
        let path = transcript.to_string_lossy().into_owned();
        let (mut d, _) = dialog(
            None,
            Box::new(move |req| {
                let mut hits = fixture();
                hits[0].session.transcript_path = Some(path.clone());
                hits[0].snippets[0].offset = Some(at);
                (req.generation, Ok(hits))
            }),
        );
        let t = Instant::now();
        type_text(&mut d, "margin");
        d.tick(t);
        d.tick(t + ms(301));
        settle(&mut d, t + ms(301));
        d
    }

    #[test]
    fn queries_fire_300ms_after_the_last_edit_from_three_chars() {
        let (mut d, sent) = dialog(None, honest());
        let t = Instant::now();
        type_text(&mut d, "ab");
        d.tick(t);
        d.tick(t + ms(400));
        // Each query takes a generation; the worker may answer within a tick.
        let sent_before = d.generation;
        assert_eq!(d.in_flight, 0);

        let t = t + ms(400);
        type_text(&mut d, "c");
        d.tick(t);
        d.tick(t + ms(250));
        assert_eq!(d.generation, sent_before);
        d.tick(t + ms(301));
        assert_eq!(d.generation, sent_before + 1);
        settle(&mut d, t + ms(301));
        d.tick(t + ms(900));
        assert_eq!(*sent.lock().unwrap(), [("abc".to_string(), Scope::All)]);
        assert_eq!(d.rows.len(), 2);

        // Back under three chars clears the rows.
        d.handle_key(key(KeyCode::Backspace));
        assert!(d.rows.is_empty());
    }

    #[test]
    fn a_result_from_an_older_generation_is_dropped() {
        let (mut d, _) = dialog(None, Box::new(|req| (req.generation - 1, Ok(fixture()))));
        let t = Instant::now();
        type_text(&mut d, "abc");
        d.tick(t);
        d.tick(t + ms(301));
        settle(&mut d, t + ms(301));
        assert!(d.rows.is_empty());
    }

    #[test]
    fn a_newer_query_skips_the_ones_queued_behind_a_slow_one() {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let (started, running) = std::sync::mpsc::channel::<()>();
        let mut calls = 0;
        let (mut d, sent) = dialog(
            Some("attic"),
            Box::new(move |req| {
                calls += 1;
                if calls == 1 {
                    started.send(()).unwrap();
                    blocked.recv().unwrap();
                    let mut one = fixture();
                    one.truncate(1);
                    return (req.generation, Ok(one));
                }
                (req.generation, Ok(fixture()))
            }),
        );
        let t = Instant::now();
        type_text(&mut d, "abc");
        d.tick(t);
        d.tick(t + ms(301));
        running.recv_timeout(Duration::from_secs(10)).unwrap();
        for _ in 0..3 {
            d.handle_key(key(KeyCode::Tab));
        }
        release.send(()).unwrap();
        settle(&mut d, t + ms(301));
        assert_eq!(
            *sent.lock().unwrap(),
            [
                ("abc".to_string(), Scope::All),
                ("abc".to_string(), Scope::All)
            ]
        );
        assert_eq!(d.rows.len(), 2);
    }

    #[test]
    fn tab_cycles_the_scope_and_requeries() {
        let (mut d, sent) = dialog(Some("attic"), honest());
        let t = Instant::now();
        type_text(&mut d, "abc");
        let before = d.generation;
        for want in [Scope::Here("attic".into()), Scope::Recent, Scope::All] {
            d.handle_key(key(KeyCode::Tab));
            assert_eq!(d.scope, want);
        }
        // Each Tab sends at once and absorbs the pending edit. Requests a
        // later Tab superseded may be skipped, so only the last is certain.
        assert_eq!(d.generation, before + 3);
        settle(&mut d, t);
        d.tick(t + ms(900));
        assert_eq!(d.generation, before + 3);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.last().map(|s| &s.1), Some(&Scope::All));

        let (mut d, _) = dialog(None, honest());
        for want in [Scope::Recent, Scope::All] {
            d.handle_key(key(KeyCode::Tab));
            assert_eq!(d.scope, want);
        }
    }

    #[test]
    fn letters_type_into_the_query() {
        let mut d = loaded();
        d.handle_key(key(KeyCode::Up));
        type_text(&mut d, "jkyo");
        assert_eq!(d.input.value(), "abcjkyo");
    }

    #[test]
    fn keys_move_expand_and_act_on_the_selected_row() {
        let mut d = loaded();
        assert_eq!(d.visible().len(), 2);
        d.handle_key(key(KeyCode::Up));
        d.handle_key(key(KeyCode::Right));
        assert_eq!(d.visible().len(), 3);

        // The child sits between its parent and the prompt.
        d.handle_key(key(KeyCode::Down));
        match d.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(SearchOutcome::Open(Target::CopyOnly(k))) => assert_eq!(k, "L2b"),
            _ => panic!("expected the child's target"),
        }
        match d.handle_key(ctrl_key(KeyCode::Char('y'))) {
            DialogResult::Submit(SearchOutcome::Copy(c)) => {
                assert!(c.ends_with("ledger run pi --profile personal -- --session p-222"))
            }
            _ => panic!("expected Copy"),
        }
        // The child has no transcript of its own.
        assert!(matches!(
            d.handle_key(ctrl_key(KeyCode::Char('o'))),
            DialogResult::Continue
        ));

        d.handle_key(ctrl_key(KeyCode::Char('k')));
        match d.handle_key(ctrl_key(KeyCode::Char('o'))) {
            DialogResult::Submit(SearchOutcome::OpenTranscript(p)) => {
                assert!(p.ends_with("t_p-222.jsonl"))
            }
            _ => panic!("expected OpenTranscript"),
        }
        d.handle_key(key(KeyCode::Left));
        assert_eq!(d.visible().len(), 2);

        d.handle_key(ctrl_key(KeyCode::Char('j')));
        match d.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(SearchOutcome::Open(Target::Jump(id))) => assert_eq!(id, "aoe1"),
            _ => panic!("expected the jump"),
        }
        assert!(matches!(
            d.handle_key(key(KeyCode::Esc)),
            DialogResult::Cancel
        ));
    }

    #[test]
    fn renders_prompt_info_rows_and_preview() {
        let mut d = loaded();
        let lines = screen(&mut d, 150, 24);
        let n = lines.len();
        assert!(lines[n - 1].starts_with("> abc"), "{:?}", lines[n - 1]);
        assert!(lines[n - 2].contains("2/2"), "{:?}", lines[n - 2]);
        assert!(lines[n - 3].starts_with('▌'), "{:?}", lines[n - 3]);
        assert!(lines[n - 3].contains("attic·deploy"), "{:?}", lines[n - 3]);
        assert!(lines.iter().any(|l| l.contains('│')));
        assert!(lines.iter().any(|l| l.contains("ledger run")));
        // No transcript to read: the best snippet stands in for the exchange.
        assert!(lines.iter().any(|l| l.contains("kickstart the daemon")));

        let mut d = loaded();
        let lines = screen(&mut d, 90, 20);
        let n = lines.len();
        assert!(!lines.iter().any(|l| l.contains('│')));
        assert!(!lines.iter().any(|l| l.contains("attic·")));
        assert!(lines[n - 3].starts_with('▌'), "{:?}", lines[n - 3]);
    }

    #[test]
    fn the_window_leaves_a_margin_showing_the_screen_behind() {
        let mut d = loaded();
        let buf = draw(150, 24, |f, theme| {
            f.render_widget(
                Paragraph::new("behind".repeat(200)).wrap(ratatui::widgets::Wrap { trim: false }),
                f.area(),
            );
            d.render(f, f.area(), theme)
        });
        let (outer, _) = window(Rect::new(0, 0, 150, 24));
        assert!(outer.x > 0 && outer.y > 0);
        assert_ne!(buf[(0, 0)].symbol(), " ");
        assert_eq!(buf[(outer.x, outer.y)].symbol(), "╭");
        let title: String = (outer.x..outer.right())
            .map(|x| buf[(x, outer.y)].symbol())
            .collect();
        assert!(title.contains("Search sessions"), "{title}");
        // Too small for a margin: the whole area, unbordered.
        assert_eq!(window(Rect::new(0, 0, 50, 10)).0, Rect::new(0, 0, 50, 10));
    }

    #[test]
    fn the_preview_shows_the_exchange_around_the_hit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let records = [
            r#"{"type":"user","timestamp":"2026-10-27T13:30:00Z","message":{"role":"user","content":"hitting ctrl-f shows nothing"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-27T13:31:00Z","message":{"role":"assistant","content":[{"type":"text","text":"the old binary is running"},{"type":"tool_use","name":"Bash","input":{"command":"ls -la ~/.local/bin/aoe"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-27T13:37:00Z","message":{"role":"user","content":"the window can overlay with a margin to show the background"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-27T13:38:00Z","message":{"role":"assistant","content":[{"type":"text","text":"making it a bordered window"}]}}"#,
        ];
        let mut body = String::new();
        let mut at = 0;
        for (i, r) in records.iter().enumerate() {
            if i == 2 {
                at = body.len() as u64;
            }
            body.push_str(r);
            body.push('\n');
        }
        std::fs::write(&path, body).unwrap();

        let mut d = loaded_with_transcript(&path, at);
        let lines = screen(&mut d, 150, 30);
        let find = |needle: &str| {
            lines
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle} not shown: {lines:#?}"))
        };
        let ask = find("hitting ctrl-f");
        let tool = find("Bash ls -la");
        let hit = find("overlay with a margin");
        let reply = find("making it a bordered window");
        assert!(ask < tool && tool < hit && hit < reply);
        assert!(lines[hit].contains("▌you"), "{:?}", lines[hit]);
    }

    #[test]
    fn stripes_stay_on_their_rows_when_the_list_scrolls() {
        let mut d = loaded();
        d.handle_key(key(KeyCode::Up));
        d.handle_key(key(KeyCode::Right));
        d.cursor = 2;
        let theme = crate::tui::styles::load_theme("empire");
        let stripe = theme.highlight_background(theme.dimmed);
        // Row 1 of three sits second from the bottom; a two-row list scrolls it to the bottom.
        let tall = draw(90, 20, |f, theme| d.render(f, f.area(), theme));
        let short = draw(90, 4, |f, theme| d.render(f, f.area(), theme));
        let (_, inner) = window(Rect::new(0, 0, 90, 20));
        // Above the prompt and info lines, one row up from the list's bottom.
        assert_eq!(tall[(inner.x + 2, inner.bottom() - 4)].bg, stripe);
        assert_eq!(short[(2, 1)].bg, stripe);
    }

    #[test]
    fn an_error_renders_its_message() {
        let (mut d, _) = dialog(
            None,
            Box::new(|req| (req.generation, Err(QueryError::Timeout))),
        );
        let t = Instant::now();
        type_text(&mut d, "abc");
        d.tick(t);
        d.tick(t + ms(301));
        settle(&mut d, t + ms(301));
        for width in [140, 90] {
            let lines = screen(&mut d, width, 20);
            assert!(lines.iter().any(|l| l.contains("search timed out")));
        }
    }

    #[test]
    fn hit_lines_start_just_above_the_first_match() {
        let text = "one\ntwo\nthree\nfour has the margin\nfive";
        let lines = hit_lines(text, &["margin".into()], 40);
        assert_eq!(lines, ["…", "three", "four has the margin", "five"]);
        assert_eq!(hit_lines("a\n\n\nb", &[], 40), ["a", "", "b"]);
    }

    #[test]
    fn wrap_and_clamp_fit_the_width() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(clamp(wrap("aa bb cc dd", 2), 2), ["aa", "bb…"]);
    }
}
