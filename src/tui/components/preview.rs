//! Preview panel component

use std::time::Duration;

use ansi_to_tui::IntoText;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::session::Instance;
use crate::tui::styles::Theme;

/// Light value type the renderers consume in place of a raw `&str`.
/// The caller is expected to hand over the cached parse from
/// `PreviewCache::ensure_parsed`; we then read it directly for the
/// actual render.
///
/// Passing a pre-parsed `Text` is the whole point of the
/// optimisation: it lets the cache update once per content change
/// rather than re-running the full `ansi-to-tui` pipeline on every
/// frame. See `PreviewCache::ensure_parsed` for the parse-and-cache
/// contract.
pub struct CachedPreview<'a> {
    /// `None` means the source `content` was empty.
    pub text: Option<&'a Text<'static>>,
    /// No frame has landed for the displayed session yet, so an empty
    /// `text` says nothing about its pane: paint nothing rather than the
    /// "No output available" hint, which would blink while a just-selected
    /// session fills.
    pub pending: bool,
}

impl<'a> CachedPreview<'a> {
    pub fn new(text: Option<&'a Text<'static>>, pending: bool) -> Self {
        Self { text, pending }
    }
}

/// Metadata rows before resources. `PreviewLayout` adds the shared info chrome.
///
/// The height is deliberately independent of the instance's data: launch
/// identity, worktree, sandbox and tool all reserve their rows (drawn as dim
/// placeholders when absent). Otherwise moving between sessions, or a runtime
/// field appearing, would shift the output banner and resize the previewed
/// tmux window. The only variation is the user's work-arc toggle.
pub fn agent_info_height(instance: &Instance) -> u16 {
    // Tool/Path/Status + Launch + Sandbox + Arc, then the Worktree block:
    // blank, header, Branch, Main, Base.
    const FIXED: u16 = 3 + 1 + 1 + 1 + 5;
    FIXED + 4 * u16::from(instance.work_arc_enabled)
}

/// Metadata rows for Terminal and Tool views, before resources.
pub fn terminal_info_height(instance: &Instance) -> u16 {
    let base: u16 = 3; // title / path / status
    let sandbox_lines: u16 = if instance.sandbox_info.as_ref().is_some_and(|s| s.enabled) {
        1
    } else {
        0
    };
    base + sandbox_lines
}

/// The geometry of the preview body, computed once so every consumer agrees on
/// where the output goes and how many rows it spans.
///
/// Historically the info-header / banner / output split was re-derived
/// independently in the renderers (`Layout` + `Borders`), in `render_preview`
/// (for the tmux pane size and the scroll clamp), and in the live `[offset/max]`
/// footer. Each derivation drifted by a row at some point, which is the bug
/// #1521, #1570, and #1604 each chased in turn. `PreviewLayout::compute` is the
/// single definition; `output.height` is THE visible-row count. Anyone needing
/// preview geometry calls this rather than re-counting rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreviewLayout {
    /// Info header when enabled and the viewport leaves space for output.
    pub info: Option<Rect>,
    /// The inner ` Output ` / ` Terminal Output ` banner row, present exactly
    /// when `info` is (it visually separates the header from the body).
    pub banner: Option<Rect>,
    /// Where captured agent/terminal output paints. `output.height` is the
    /// authoritative visible-row count for scrolling and pane sizing.
    pub output: Rect,
}

impl PreviewLayout {
    /// Split metadata, resources and badges from output. Collapse optional info
    /// when hidden, compact, or too short to leave an output row.
    pub(crate) fn compute(area: Rect, compact: bool, show_info: bool, info_height: u16) -> Self {
        let info_height = info_height.saturating_add(3).max(8);
        if compact || !show_info || area.height <= info_height.saturating_add(1) {
            return Self {
                info: None,
                banner: None,
                output: area,
            };
        }
        let chrome = info_height.saturating_add(1).min(area.height);
        let info_h = info_height.min(area.height);
        let info = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: info_h,
        };
        // Keep the banner within the allocated chrome.
        let banner = if chrome > info_h {
            Some(Rect {
                x: area.x,
                y: area.y + info_h,
                width: area.width,
                height: 1,
            })
        } else {
            None
        };
        let output = Rect {
            x: area.x,
            y: area.y + chrome,
            width: area.width,
            height: area.height - chrome,
        };
        Self {
            info: Some(info),
            banner,
            output,
        }
    }
}

/// Metadata and its resource traces sit beside the usage/Ledger badge column.
pub(crate) struct InfoAreas {
    pub metadata: Rect,
    pub resources: Rect,
    pub badges: Rect,
}

impl InfoAreas {
    pub(crate) fn compute(area: Rect, metadata_height: u16) -> Self {
        let badge_width = (area.width / 2).min(48);
        let left_width = area.width.saturating_sub(badge_width + 2);
        let metadata_height = metadata_height.min(area.height);
        Self {
            metadata: Rect::new(area.x, area.y, left_width, metadata_height),
            resources: Rect::new(
                area.x,
                area.y + metadata_height,
                left_width,
                area.height.saturating_sub(metadata_height).min(3),
            ),
            badges: Rect::new(area.right() - badge_width, area.y, badge_width, area.height),
        }
    }
}

fn clip_info_text(text: &str, width: u16) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if text.width() <= usize::from(width) {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut columns = 0;
    for ch in text.chars() {
        let next = columns + ch.width().unwrap_or(0);
        if next >= usize::from(width) {
            break;
        }
        out.push(ch);
        columns = next;
    }
    out.push('…');
    out
}

pub struct Preview;

impl Preview {
    #[allow(clippy::too_many_arguments)]
    pub fn render_terminal_preview(
        frame: &mut Frame,
        area: Rect,
        instance: &Instance,
        terminal_presence: crate::session::PanePresence,
        cached_output: CachedPreview<'_>,
        scroll_offset: u16,
        theme: &Theme,
        compact: bool,
        show_info: bool,
    ) {
        use crate::session::PanePresence;
        // One source of truth for the header / banner / output split. Compact
        // viewports and the hidden-header toggle both collapse to "output owns
        // the whole area" inside `PreviewLayout::compute`.
        let layout =
            PreviewLayout::compute(area, compact, show_info, terminal_info_height(instance));

        if let Some(info_area) = layout.info {
            let info_area = InfoAreas::compute(info_area, terminal_info_height(instance)).metadata;
            let mut info_lines = vec![
                Line::from(vec![
                    Span::styled("Title:   ", Style::default().fg(theme.dimmed)),
                    Span::styled(&instance.title, Style::default().fg(theme.text).bold()),
                ]),
                Line::from(vec![
                    Span::styled("Path:    ", Style::default().fg(theme.dimmed)),
                    Span::styled(
                        shorten_path(&instance.project_path),
                        Style::default().fg(theme.text),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Status:  ", Style::default().fg(theme.dimmed)),
                    Span::styled(
                        match terminal_presence {
                            PanePresence::Alive => "Running",
                            PanePresence::Dead => "Exited",
                            PanePresence::Absent => "Not started",
                            PanePresence::Unknown => "Unknown",
                        },
                        Style::default().fg(if terminal_presence == PanePresence::Alive {
                            theme.terminal_active
                        } else {
                            theme.dimmed
                        }),
                    ),
                ]),
            ];
            if let Some(sandbox) = &instance.sandbox_info {
                if sandbox.enabled {
                    info_lines.push(Line::from(vec![
                        Span::styled("Sandbox: ", Style::default().fg(theme.dimmed)),
                        Span::styled(&sandbox.container_name, Style::default().fg(theme.sandbox)),
                    ]));
                }
            }
            frame.render_widget(Paragraph::new(info_lines), info_area);
        }

        // `output.height` is the authoritative visible-row count; no separate
        // banner subtraction (that lives entirely in `PreviewLayout::compute`).
        let visible_height = layout.output.height as usize;
        // Preserve captured output for exited or temporarily unobservable panes.
        let parsed_output = if terminal_presence != PanePresence::Absent {
            cached_output.text
        } else {
            None
        };
        let line_count = parsed_output.map_or(0, |t| t.lines.len());

        // The inner ` Terminal Output ` banner is present exactly when the info
        // section is: with it hidden the outer block title already names the
        // view, so the body claims the freed row.
        if let Some(banner) = layout.banner {
            let mut block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border))
                .title(" Terminal Output ")
                .title_style(Style::default().fg(theme.dimmed));
            if let Some(indicator) =
                format_scroll_indicator(line_count, visible_height, scroll_offset)
            {
                block = block.title_top(
                    Line::from(indicator)
                        .right_aligned()
                        .style(Style::default().fg(theme.dimmed)),
                );
            }
            frame.render_widget(block, banner_block_area(layout.output, banner));
        }

        let inner = layout.output;
        if terminal_presence == PanePresence::Absent {
            let hint = Paragraph::new("Press Enter to start terminal")
                .style(Style::default().fg(theme.dimmed))
                .alignment(Alignment::Center);
            frame.render_widget(hint, inner);
        } else if let Some(output_text) = parsed_output {
            render_scrolled_output(
                frame,
                inner,
                output_text,
                line_count,
                visible_height,
                scroll_offset,
                Style::default().fg(theme.text),
            );
        } else if !cached_output.pending {
            let hint = Paragraph::new(match terminal_presence {
                PanePresence::Unknown => "Terminal status unavailable",
                PanePresence::Dead => "Terminal exited; press Enter to restart",
                _ => "No output available",
            })
            .style(Style::default().fg(theme.dimmed))
            .alignment(Alignment::Center);
            frame.render_widget(hint, inner);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_with_cache(
        frame: &mut Frame,
        area: Rect,
        instance: &Instance,
        cached_output: CachedPreview<'_>,
        scroll_offset: u16,
        theme: &Theme,
        idle_decay_window: Duration,
        compact: bool,
        show_info: bool,
    ) {
        // One source of truth for the split. With the header hidden or the
        // viewport compact the output claims the whole pane, and the outer block
        // already says "Preview".
        let layout = PreviewLayout::compute(area, compact, show_info, agent_info_height(instance));
        if let Some(info_area) = layout.info {
            Self::render_info(frame, info_area, instance, theme, idle_decay_window);
        }
        Self::render_output_cached(
            frame,
            layout.output,
            layout.banner,
            instance,
            cached_output,
            scroll_offset,
            theme,
        );
    }

    pub(crate) fn render_info(
        frame: &mut Frame,
        area: Rect,
        instance: &Instance,
        theme: &Theme,
        idle_decay_window: Duration,
    ) {
        let area = InfoAreas::compute(area, agent_info_height(instance)).metadata;
        let mut info_lines = Vec::new();

        // Profile and Tool on the same row to save vertical space
        let mut profile_tool_spans = Vec::new();
        if !instance.source_profile.is_empty() {
            profile_tool_spans.push(Span::styled("Profile: ", Style::default().fg(theme.dimmed)));
            profile_tool_spans.push(Span::styled(
                &instance.source_profile,
                Style::default().fg(theme.accent),
            ));
            profile_tool_spans.push(Span::raw("  "));
        }
        profile_tool_spans.push(Span::styled("Tool: ", Style::default().fg(theme.dimmed)));
        profile_tool_spans.push(Span::styled(
            &instance.tool,
            Style::default().fg(theme.accent),
        ));
        info_lines.push(Line::from(profile_tool_spans));
        info_lines.push(match instance.current_launch_identity() {
            Some(identity) => Line::from(vec![
                Span::styled("Launch:  ", Style::default().fg(theme.dimmed)),
                Span::styled(identity.description(), Style::default().fg(theme.text)),
            ]),
            None => Line::styled("Launch:  unknown", Style::default().fg(theme.dimmed)),
        });

        info_lines.extend([
            Line::from(vec![
                Span::styled("Path:    ", Style::default().fg(theme.dimmed)),
                Span::styled(
                    shorten_path(&instance.project_path),
                    Style::default().fg(theme.text),
                ),
            ]),
            Line::from(vec![
                Span::styled("Status:  ", Style::default().fg(theme.dimmed)),
                {
                    // A dormant (idle-reaped, resumable) structured worker
                    // reads "Dormant" in dim amber, distinct from a deliberate
                    // Stop or a live Idle. See #2250.
                    let (label, color) = if instance.is_shown_dormant() {
                        ("Dormant".to_string(), theme.dormant())
                    } else {
                        (
                            format!("{:?}", instance.status),
                            match instance.status {
                                crate::session::Status::Running => theme.running,
                                crate::session::Status::Waiting => theme.waiting,
                                crate::session::Status::Idle => {
                                    theme.idle_color_at_age(instance.idle_age(), idle_decay_window)
                                }
                                crate::session::Status::Unknown => theme.waiting,
                                crate::session::Status::Stopped => theme.dimmed,
                                crate::session::Status::Error => theme.error,
                                crate::session::Status::Starting => theme.dimmed,
                                crate::session::Status::Deleting => theme.waiting,
                                crate::session::Status::Creating => theme.accent,
                            },
                        )
                    };
                    Span::styled(label, Style::default().fg(color))
                },
            ]),
        ]);

        // Sandbox and worktree rows are always reserved; see `agent_info_height`.
        match instance.sandbox_info.as_ref().filter(|s| s.enabled) {
            Some(sandbox) => info_lines.push(Line::from(vec![
                Span::styled("Sandbox: ", Style::default().fg(theme.dimmed)),
                Span::styled(&sandbox.container_name, Style::default().fg(theme.sandbox)),
            ])),
            None => info_lines.push(Line::styled(
                "Sandbox: host",
                Style::default().fg(theme.dimmed),
            )),
        }

        info_lines.push(Line::from(""));
        info_lines.push(Line::from(vec![
            Span::styled("─", Style::default().fg(theme.border)),
            Span::styled(" Worktree ", Style::default().fg(theme.dimmed)),
            Span::styled("─", Style::default().fg(theme.border)),
        ]));
        if let Some(wt_info) = &instance.worktree_info {
            info_lines.push(Line::from(vec![
                Span::styled("Branch:  ", Style::default().fg(theme.dimmed)),
                Span::styled(&wt_info.branch, Style::default().fg(theme.branch)),
            ]));
            info_lines.push(Line::from(vec![
                Span::styled("Main:    ", Style::default().fg(theme.dimmed)),
                Span::styled(
                    shorten_path(&wt_info.main_repo_path),
                    Style::default().fg(theme.text),
                ),
            ]));
            info_lines.push(match wt_info.base_branch.as_deref() {
                Some(base) => Line::from(vec![
                    Span::styled("Base:    ", Style::default().fg(theme.dimmed)),
                    Span::styled(base, Style::default().fg(theme.branch)),
                ]),
                None => Line::from(""),
            });
        } else {
            info_lines.push(Line::styled(
                "Branch:  none",
                Style::default().fg(theme.dimmed),
            ));
            info_lines.push(Line::from(""));
            info_lines.push(Line::from(""));
        }

        if crate::session::work_arc::supported(instance) {
            let record = crate::session::work_arc::visible_record(instance);
            let covered = record
                .map(|r| {
                    format!(
                        ", covered {}",
                        r.at.with_timezone(&chrono::Local).format("%m-%d %H:%M %:z")
                    )
                })
                .unwrap_or_default();
            let status = crate::session::work_arc::projection(instance);
            let prior = if record
                .is_some_and(|r| instance.agent_session_id.as_deref() != Some(&r.identity.pi_id))
            {
                " (carried forward)"
            } else if record.is_some() && status != crate::session::work_arc::ArcStatus::Current {
                " (prior stale)"
            } else {
                ""
            };
            let expand = if instance.work_arc_enabled || record.is_some() {
                " [menu: expand]"
            } else {
                ""
            };
            info_lines.push(Line::styled(
                format!("Arc: {}{prior}{covered}{expand}", status.label()),
                Style::default().fg(theme.dimmed),
            ));
            if instance.work_arc_enabled {
                for (label, value) in [
                    ("Goal: ", record.map(|r| r.arc.goal.as_str())),
                    ("Now: ", record.map(|r| r.arc.now.as_str())),
                    ("Remaining: ", record.map(|r| r.arc.remaining.as_str())),
                    ("Next: ", record.map(|r| r.arc.next.as_str())),
                ] {
                    let label_width = unicode_width::UnicodeWidthStr::width(label) as u16;
                    info_lines.push(Line::from(vec![
                        Span::styled(
                            clip_info_text(label, area.width),
                            Style::default().fg(theme.dimmed),
                        ),
                        Span::styled(
                            clip_info_text(
                                value.unwrap_or("Unknown"),
                                area.width.saturating_sub(label_width),
                            ),
                            Style::default().fg(theme.text),
                        ),
                    ]));
                }
            }
        } else {
            // Tools without work arcs keep the row so the panel height is the
            // same for every tool.
            info_lines.push(Line::styled("Arc: n/a", Style::default().fg(theme.dimmed)));
        }
        let paragraph = Paragraph::new(info_lines);
        frame.render_widget(paragraph, area);
    }

    fn render_output_cached(
        frame: &mut Frame,
        output: Rect,
        banner: Option<Rect>,
        instance: &Instance,
        cached_output: CachedPreview<'_>,
        scroll_offset: u16,
        theme: &Theme,
    ) {
        // `output.height` is the visible-row count straight from
        // `PreviewLayout`. The error path below returns early, so `parsed_output`
        // is the caller's cached parse by the time the Paragraph uses it.
        let visible_height = output.height as usize;
        // The error path below returns early, so by the time we use
        // `parsed_output` for the output Paragraph the error case has
        // been handled. Until then `parsed_output` is just the cached
        // parse passed in by the caller (renamed for readability).
        let parsed_output = cached_output.text;
        let line_count = parsed_output.map_or(0, |t| t.lines.len());

        // The inner ` Output ` banner is drawn only when `PreviewLayout` gave a
        // banner row; otherwise the body claims the freed row.
        if let Some(banner) = banner {
            let mut block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border))
                .title(" Output ")
                .title_style(Style::default().fg(theme.dimmed));
            if let Some(indicator) =
                format_scroll_indicator(line_count, visible_height, scroll_offset)
            {
                block = block.title_top(
                    Line::from(indicator)
                        .right_aligned()
                        .style(Style::default().fg(theme.dimmed)),
                );
            }
            frame.render_widget(block, banner_block_area(output, banner));
        }
        let inner = output;

        if let Some(error) = &instance.last_error {
            let mut error_lines: Vec<Line> = vec![
                Line::from(Span::styled(
                    "Error:",
                    Style::default().fg(theme.error).bold(),
                )),
                Line::from(""),
            ];
            for line in error.split('\n') {
                error_lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.error),
                )));
            }
            let paragraph = Paragraph::new(error_lines).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, inner);
            return;
        }

        if let Some(output_text) = parsed_output {
            render_scrolled_output(
                frame,
                inner,
                output_text,
                line_count,
                visible_height,
                scroll_offset,
                Style::default().fg(theme.text),
            );
        } else if !cached_output.pending {
            let hint = Paragraph::new("No output available")
                .style(Style::default().fg(theme.dimmed))
                .alignment(Alignment::Center);
            frame.render_widget(hint, inner);
        }
    }
}

/// The `Borders::TOP` block that draws the ` Output ` / ` Terminal Output `
/// banner. It spans the banner row plus the whole output body so its single top
/// border lands on the banner row and `block.inner()` coincides exactly with
/// `output`. Built from `PreviewLayout`'s `output` + `banner` rects so the
/// banner can never be sized independently of the body it caps.
fn banner_block_area(output: Rect, banner: Rect) -> Rect {
    Rect {
        x: output.x,
        y: banner.y,
        width: output.width,
        height: output.height.saturating_add(1),
    }
}

/// Pick the row offset passed to `Paragraph::scroll`. Zero user offset shows
/// the bottom of the cached pane (live-follow). A positive offset scrolls the
/// same number of lines back, saturating at the top of the capture.
///
/// Exposed at crate visibility so the preview drag-select code can map a
/// screen row to the absolute content line under it: the value returned here
/// is the index of the parsed-text line painted on the output pane's top row,
/// so `first_line + (screen_row - pane.y)` is the line beneath any cell.
pub(crate) fn compute_scroll(line_count: usize, visible_height: usize, user_offset: u16) -> u16 {
    if line_count <= visible_height {
        return 0;
    }
    let bottom = (line_count - visible_height) as u16;
    bottom.saturating_sub(user_offset)
}

/// The half-open range of line indices `[top, end)` that a `visible_height`
/// viewport shows at `scroll_offset` into a `line_count`-line snapshot. Pure so
/// the slice math is unit-tested without a `Frame`.
pub(crate) fn visible_line_range(
    line_count: usize,
    visible_height: usize,
    scroll_offset: u16,
) -> (usize, usize) {
    let top = (compute_scroll(line_count, visible_height, scroll_offset) as usize).min(line_count);
    let end = top.saturating_add(visible_height).min(line_count);
    (top, end)
}

/// Render only the visible window of `text` into `area`.
///
/// `Paragraph::new(text).scroll((n, 0))` clones and lays out the WHOLE parsed
/// `Text` every frame, so a deep frozen snapshot (the reading capture can run to
/// thousands of lines) made a fast wheel flick stutter: each frame paid an
/// O(total-lines) clone plus layout, and at a high event rate the renders
/// couldn't keep up. Slicing to just the `visible_height` rows at the current
/// offset first keeps every frame O(visible), so scroll cost is flat no matter
/// how much scrollback is held. Rendered at scroll 0 because the slice already
/// starts at the top visible row.
fn render_scrolled_output(
    frame: &mut Frame,
    area: Rect,
    text: &Text<'static>,
    line_count: usize,
    visible_height: usize,
    scroll_offset: u16,
    style: Style,
) {
    let (top, end) = visible_line_range(line_count, visible_height, scroll_offset);
    let visible: Vec<Line<'static>> = text.lines[top..end].to_vec();
    frame.render_widget(Paragraph::new(Text::from(visible)).style(style), area);
}

/// Render a tmux-style ` [offset/max] ` indicator when the user has scrolled
/// back. Returns `None` while live-following or when the content fits in view.
pub fn format_scroll_indicator(
    line_count: usize,
    visible_height: usize,
    user_offset: u16,
) -> Option<String> {
    if user_offset == 0 || line_count <= visible_height {
        return None;
    }
    let max_offset = (line_count - visible_height) as u16;
    let clamped = user_offset.min(max_offset);
    Some(format!(" [{}/{}] ", clamped, max_offset))
}

/// Parse a captured ANSI string into a ratatui `Text`.
///
/// Visible at the module level so `PreviewCache::ensure_parsed` can
/// call it from `src/tui/home/preview.rs` to drive the cache.
///
/// OSC 8 is stripped rather than carried: `ansi-to-tui` drops the visible text
/// around an ST-terminated OSC (#1181), and a ratatui cell has no attribute to
/// hold a hyperlink target anyway. The targets are kept beside the text in
/// `PreviewCache::links` and re-anchored by `crate::tui::links`.
pub fn parse_output_text(content: &str) -> Text<'static> {
    let cleaned = crate::tmux::utils::strip_osc_st(content);
    cleaned.into_text().unwrap_or_else(|_| Text::from(cleaned))
}

fn shorten_path(path: &str) -> String {
    let path_buf = std::path::PathBuf::from(path);

    if let Some(home) = dirs::home_dir() {
        if let (Ok(canonical_path), Ok(canonical_home)) =
            (path_buf.canonicalize(), home.canonicalize())
        {
            return crate::util::collapse_home(
                &canonical_path.to_string_lossy(),
                &canonical_home.to_string_lossy(),
            );
        }
        return crate::util::collapse_home(path, &home.to_string_lossy());
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins `$HOME` so the read inside `shorten_path` cannot see a value another
    /// test set. `isolate_home` holds the process-global env lock for the guard's
    /// lifetime and restores `$HOME` on Drop, before the tempdir is deleted.
    #[test]
    #[serial_test::serial]
    fn shorten_path_abbreviates_home() {
        let home = tempfile::TempDir::new().expect("temp home");
        let _home = crate::session::test_support::isolate_home(home.path());
        let home_str = home.path().to_str().expect("utf-8 temp home");

        for (case, path, expect) in [
            (
                "a path under home",
                format!("{home_str}/projects/myapp"),
                "~/projects/myapp",
            ),
            ("home itself", home_str.to_string(), "~"),
            // A sibling whose name merely starts with the home path.
            (
                "a similar prefix",
                format!("{home_str}extra/not/home"),
                &*format!("{home_str}extra/not/home"),
            ),
            (
                "a trailing slash",
                format!("{home_str}/projects/"),
                "~/projects/",
            ),
        ] {
            assert_eq!(shorten_path(&path), expect, "{case}");
        }
    }

    #[test]
    fn test_shorten_path_empty() {
        let path = "";
        let shortened = shorten_path(path);
        assert_eq!(shortened, "");
    }

    // Single source of truth for the preview split, pinning the row arithmetic
    // that #1521 / #1570 / #1604 each got wrong in a different derivation.
    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn layout_hidden_info_gives_output_the_whole_area() {
        // Header toggled off (or compact): no header, no banner, output == area.
        let area = rect(0, 0, 80, 40);
        let l = PreviewLayout::compute(area, false, false, 7);
        assert_eq!(l.info, None);
        assert_eq!(l.banner, None);
        assert_eq!(l.output, area);
        // Compact forces the same regardless of the toggle.
        assert_eq!(PreviewLayout::compute(area, true, true, 7).output, area);
    }

    #[test]
    fn layout_shown_info_carves_header_plus_banner_once() {
        let area = rect(2, 3, 80, 40);
        let l = PreviewLayout::compute(area, false, true, 7);
        assert_eq!(l.info, Some(rect(2, 3, 80, 10)));
        assert_eq!(l.banner, Some(rect(2, 13, 80, 1)));
        assert_eq!(l.output, rect(2, 14, 80, 29));
        let block_area = banner_block_area(l.output, l.banner.unwrap());
        assert_eq!(block_area, rect(2, 13, 80, 30));
        let info = InfoAreas::compute(l.info.unwrap(), 7);
        assert_eq!(info.metadata, rect(2, 3, 38, 7));
        assert_eq!(info.resources, rect(2, 10, 38, 3));
        assert_eq!(info.badges, rect(42, 3, 40, 10));
    }

    #[test]
    fn layout_preserves_output_in_shallow_panes() {
        for height in [0, 3, 8, 9] {
            let area = rect(0, 0, 80, height);
            let layout = PreviewLayout::compute(area, false, true, 4);
            assert_eq!(layout.output, area);
            assert!(layout.info.is_none());
            assert!(layout.banner.is_none());
        }
    }

    // End to end: a captured screen exactly as tall as the banner-less output,
    // live-following. Scroll must be 0 so the top row stays on screen (#1604).
    #[test]
    fn full_height_capture_does_not_scroll_when_banner_hidden() {
        let area = rect(0, 0, 80, 40);
        let l = PreviewLayout::compute(area, false, false, 7);
        let visible = l.output.height as usize;
        assert_eq!(visible, 40);
        assert_eq!(compute_scroll(visible, visible, 0), 0);
    }

    #[test]
    fn compute_scroll_live_follow_when_content_fits() {
        assert_eq!(compute_scroll(5, 10, 0), 0);
        assert_eq!(compute_scroll(5, 10, 20), 0);
    }

    #[test]
    fn compute_scroll_sticks_to_bottom_with_zero_offset() {
        assert_eq!(compute_scroll(100, 20, 0), 80);
    }

    #[test]
    fn compute_scroll_walks_back_by_offset() {
        assert_eq!(compute_scroll(100, 20, 15), 65);
    }

    #[test]
    fn compute_scroll_saturates_at_top() {
        assert_eq!(compute_scroll(100, 20, 500), 0);
    }

    #[test]
    fn visible_line_range_slices_only_the_viewport() {
        // Content fits: the whole snapshot is the window.
        assert_eq!(visible_line_range(5, 10, 0), (0, 5));
        // Live edge (offset 0): the last `visible_height` lines.
        assert_eq!(visible_line_range(100, 20, 0), (80, 100));
        // Walked back 15: window slides up by 15, still 20 rows tall.
        assert_eq!(visible_line_range(100, 20, 15), (65, 85));
        // Saturated past the top: pinned to the first 20 rows, never negative.
        assert_eq!(visible_line_range(100, 20, 500), (0, 20));
    }

    #[test]
    fn scroll_indicator_hidden_when_live() {
        assert_eq!(format_scroll_indicator(100, 20, 0), None);
    }

    #[test]
    fn scroll_indicator_hidden_when_content_fits() {
        assert_eq!(format_scroll_indicator(10, 20, 5), None);
    }

    #[test]
    fn scroll_indicator_reports_position_and_max() {
        assert_eq!(
            format_scroll_indicator(100, 20, 15),
            Some(" [15/80] ".to_string())
        );
    }

    #[test]
    fn scroll_indicator_clamps_to_max() {
        assert_eq!(
            format_scroll_indicator(100, 20, 500),
            Some(" [80/80] ".to_string())
        );
    }

    #[test]
    fn info_panel_marks_arc_not_applicable_for_tools_without_work_arcs() {
        use ratatui::{backend::TestBackend, Terminal};
        let theme = crate::tui::styles::load_theme("empire");
        let mut session = Instance::new("plain", "/tmp/plain");
        session.tool = "claude".into();
        assert!(!crate::session::work_arc::supported(&session));
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| {
                Preview::render_info(frame, frame.area(), &session, &theme, Duration::ZERO)
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..buffer.area.height)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Arc: n/a"), "{text}");
        assert!(!text.contains("Arc: off"), "{text}");
    }

    #[test]
    fn work_arc_info_buffer_shows_four_bounded_fields_states_and_expand_hint() {
        use crate::session::work_arc::{ArcIdentity, ArcStatus, StoredArc, WorkArc};
        use ratatui::{backend::TestBackend, Terminal};
        let theme = crate::tui::styles::load_theme("empire");
        let mut session = Instance::new("synthetic", "/tmp/synthetic");
        session.tool = "pi".into();
        session.agent_session_id = Some("synthetic-pi".into());
        session.work_arc_enabled = true;
        for (status, width) in [
            (ArcStatus::Missing, 55),
            (ArcStatus::Loading, 140),
            (ArcStatus::Stale, 140),
            (ArcStatus::Failed, 55),
            (ArcStatus::Failed, 140),
            (ArcStatus::Unavailable, 55),
        ] {
            session.work_arc_status = Some(status);
            if status == ArcStatus::Stale {
                session.work_arc.record = Some(StoredArc {
                    identity: ArcIdentity {
                        session_id: session.id.clone(),
                        pi_id: "synthetic-pi".into(),
                        launch_id: "launch".into(),
                        generation: 0,
                        leaf: "leaf".into(),
                    },
                    at: chrono::Utc::now(),
                    arc: WorkArc {
                        goal: "Goal evidence ".repeat(8),
                        now: "Reported progress".into(),
                        remaining: "Unknown".into(),
                        next: "Review".into(),
                    },
                    selected_route: "fake/route".into(),
                    provider: "fake".into(),
                    model: "model".into(),
                    usage: None,
                });
            }
            if status == ArcStatus::Failed {
                session.agent_session_id = Some("successor-pi".into());
            }
            let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
            terminal
                .draw(|frame| {
                    Preview::render_info(frame, frame.area(), &session, &theme, Duration::ZERO)
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let rows: Vec<String> = (0..buffer.area.height)
                .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            let text = rows.join("\n");
            assert!(
                text.contains(status.label().split_whitespace().next().unwrap()),
                "{text}"
            );
            for field in ["Goal:", "Now:", "Remaining:", "Next:"] {
                assert!(text.contains(field), "{text}");
            }
            if status == ArcStatus::Stale {
                assert!(rows[11].contains('…'), "clipped Goal has a cue: {text}");
                assert!(text.contains("menu: expand"), "{text}");
                let covered = session
                    .work_arc
                    .record
                    .as_ref()
                    .unwrap()
                    .at
                    .with_timezone(&chrono::Local)
                    .format("%m-%d %H:%M %:z")
                    .to_string();
                assert!(
                    text.contains(&covered),
                    "covered time includes local UTC offset: {text}"
                );
            }
            if status == ArcStatus::Failed {
                assert!(
                    text.contains("carried"),
                    "successor cue must be distinguished from same-conversation staleness: {text}"
                );
                if width == 140 {
                    assert!(text.contains("carried forward"), "{text}");
                }
            }
            assert!(
                rows[15].trim().is_empty(),
                "arc cannot spill into resources"
            );
        }
    }

    #[test]
    fn work_arc_geometry_keeps_fields_resources_and_output_separate() {
        let mut session = Instance::new("synthetic", "/tmp/synthetic");
        session.tool = "pi".into();
        session.work_arc_enabled = true;
        for (width, height) in [(140, 40), (55, 30), (30, 8)] {
            let area = Rect::new(0, 0, width, height);
            let metadata = agent_info_height(&session);
            assert_eq!(metadata, 15);
            let layout = PreviewLayout::compute(area, false, true, metadata);
            if height == 8 {
                assert_eq!(layout.output, area);
            }
            if let Some(info) = layout.info {
                let areas = InfoAreas::compute(info, metadata);
                assert_eq!(areas.resources.y, areas.metadata.bottom());
                assert_eq!(areas.resources.height, 3);
                assert!(areas.badges.x > areas.metadata.right());
                assert!(layout.output.y > areas.resources.bottom());
            }
            assert_eq!(
                PreviewLayout::compute(area, true, true, metadata).output,
                area
            );
            assert_eq!(
                PreviewLayout::compute(area, false, false, metadata).output,
                area
            );
        }
    }

    #[test]
    fn output_banner_row_is_identical_across_info_variants() {
        use crate::session::WorktreeInfo;
        let mut rows = Vec::new();
        for tool in ["claude", "pi"] {
            for worktree in [None, Some(None), Some(Some("main".to_string()))] {
                let mut s = Instance::new("synthetic", "/tmp/synthetic");
                s.tool = tool.into();
                s.worktree_info = worktree.map(|base_branch| WorktreeInfo {
                    branch: "feature/x".into(),
                    main_repo_path: "/repo".into(),
                    managed_by_aoe: true,
                    created_at: chrono::Utc::now(),
                    base_branch,
                });
                let area = Rect::new(0, 0, 100, 40);
                let layout = PreviewLayout::compute(area, false, true, agent_info_height(&s));
                rows.push((layout.banner.map(|b| b.y), layout.output));
            }
        }
        assert!(rows[0].0.is_some());
        assert!(rows.iter().all(|r| *r == rows[0]), "{rows:?}");
    }

    // `agent_info_height` drives both the preview layout split and the live-send
    // worker geometry, so each branch of the formula gets a case: a one-row drift
    // brings the shifted-preview bug back.
    mod agent_info_height {
        use super::super::agent_info_height;
        use crate::session::{Instance, SandboxInfo, WorktreeInfo};
        use chrono::Utc;

        fn worktree(base_branch: Option<&str>) -> WorktreeInfo {
            WorktreeInfo {
                branch: "feature/x".into(),
                main_repo_path: "/repo".into(),
                managed_by_aoe: true,
                created_at: Utc::now(),
                base_branch: base_branch.map(str::to_string),
            }
        }

        fn enabled_sandbox() -> SandboxInfo {
            SandboxInfo {
                provider: None,
                enabled: true,
                container_id: None,
                image: "img".into(),
                container_name: "ctr".into(),
                extra_env: None,
                custom_instruction: None,
                before_start_env: Vec::new(),
                before_start_hook_env: Vec::new(),
                container_workdir: None,
            }
        }

        /// Title/Path/Status + Launch + Arc + Worktree block (blank, header,
        /// Branch, Main, Base) + Sandbox.
        const FIXED: u16 = 3 + 1 + 1 + 5 + 1;

        fn variants() -> Vec<(&'static str, Instance)> {
            let mut out = Vec::new();
            for tool in ["claude", "pi", "codex"] {
                for sandboxed in [false, true] {
                    for wt in [None, Some(None), Some(Some("main"))] {
                        let mut inst = Instance::new("v", "/tmp/v");
                        inst.tool = tool.into();
                        if sandboxed {
                            inst.sandbox_info = Some(enabled_sandbox());
                        }
                        inst.worktree_info = wt.map(worktree);
                        out.push((tool, inst));
                    }
                }
            }
            let mut disabled = Instance::new("d", "/tmp/d");
            let mut sb = enabled_sandbox();
            sb.enabled = false;
            disabled.sandbox_info = Some(sb);
            out.push(("disabled-sandbox", disabled));
            out
        }

        #[test]
        fn height_is_fixed_across_tool_sandbox_and_worktree() {
            for (name, inst) in variants() {
                assert_eq!(agent_info_height(&inst), FIXED, "{name}");
            }
        }

        #[test]
        fn arc_enabled_adds_only_the_four_field_rows() {
            for (name, mut inst) in variants() {
                inst.work_arc_enabled = true;
                assert_eq!(agent_info_height(&inst), FIXED + 4, "{name}");
            }
        }
    }

    // Terminal-view counterpart of `agent_info_height`, guarding the same drift:
    // the live-send resize sizes the pane to `inner - terminal_info_height - 1`.
    mod terminal_info_height {
        use super::super::terminal_info_height;
        use crate::session::{Instance, SandboxInfo, WorktreeInfo};
        use chrono::Utc;

        fn enabled_sandbox() -> SandboxInfo {
            SandboxInfo {
                provider: None,
                enabled: true,
                container_id: None,
                image: "img".into(),
                container_name: "ctr".into(),
                extra_env: None,
                custom_instruction: None,
                before_start_env: Vec::new(),
                before_start_hook_env: Vec::new(),
                container_workdir: None,
            }
        }

        #[test]
        fn plain_session_is_three_rows() {
            let inst = Instance::new("plain", "/tmp/plain");
            assert_eq!(terminal_info_height(&inst), 3);
        }

        #[test]
        fn sandboxed_adds_one_row() {
            let mut inst = Instance::new("sandboxed", "/tmp/sandboxed");
            inst.sandbox_info = Some(enabled_sandbox());
            assert_eq!(terminal_info_height(&inst), 4);
        }

        #[test]
        fn disabled_sandbox_does_not_count() {
            let mut inst = Instance::new("disabled", "/tmp/disabled");
            let mut sandbox = enabled_sandbox();
            sandbox.enabled = false;
            inst.sandbox_info = Some(sandbox);
            assert_eq!(terminal_info_height(&inst), 3);
        }

        #[test]
        fn worktree_info_does_not_count() {
            // Worktree info is an Agent-view-only block; the terminal
            // view doesn't render it, so the height stays at 3.
            let mut inst = Instance::new("wt", "/tmp/wt");
            inst.worktree_info = Some(WorktreeInfo {
                branch: "feature/x".into(),
                main_repo_path: "/repo".into(),
                managed_by_aoe: true,
                created_at: Utc::now(),
                base_branch: Some("main".into()),
            });
            assert_eq!(terminal_info_height(&inst), 3);
        }
    }

    /// Rows of a rendered terminal preview, for the hint assertions below.
    fn terminal_preview_rows(pending: bool) -> Vec<String> {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let theme = crate::tui::styles::load_theme("empire");
        let instance = Instance::new("pane", "/tmp/pane");
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal
            .draw(|frame| {
                Preview::render_terminal_preview(
                    frame,
                    frame.area(),
                    &instance,
                    crate::session::PanePresence::Alive,
                    CachedPreview::new(None, pending),
                    0,
                    &theme,
                    false,
                    false,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn empty_preview_hint_waits_for_the_first_frame() {
        let hint = |pending| {
            terminal_preview_rows(pending)
                .iter()
                .any(|row| row.contains("No output available"))
        };
        assert!(
            hint(false),
            "an empty frame for the displayed session paints the hint"
        );
        assert!(
            !hint(true),
            "no frame yet for the displayed session paints nothing"
        );
    }
}
