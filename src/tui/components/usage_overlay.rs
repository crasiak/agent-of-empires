//! The session's context-reset count in big red digits, with a usage
//! breakdown, drawn translucently over the top-right corner of the preview
//! pane.

use chrono::{DateTime, Duration, Utc};
use ratatui::prelude::*;
use ratatui::widgets::*;
use unicode_width::UnicodeWidthStr;

use crate::tui::styles::{blend, Theme};
use crate::usage::UsageSummary;

/// Opacity of the overlay's own panel color over whatever sits underneath.
const BACKDROP_ALPHA: f32 = 0.25;
/// Opacity of overlay glyph ink over the blended backdrop.
const INK_ALPHA: f32 = 0.9;

/// 3x5 pixel digits, one bit per pixel, top row first, left pixel highest.
const DIGIT_PIXELS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

pub(crate) fn big_digits(n: u32) -> [String; 3] {
    let digits: Vec<usize> = n.to_string().bytes().map(|b| (b - b'0') as usize).collect();
    let pixel =
        |d: usize, row: usize, col: usize| row < 5 && DIGIT_PIXELS[d][row] & (0b100 >> col) != 0;
    std::array::from_fn(|line| {
        digits
            .iter()
            .map(|&d| {
                (0..3)
                    .map(
                        |col| match (pixel(d, line * 2, col), pixel(d, line * 2 + 1, col)) {
                            (true, true) => '█',
                            (true, false) => '▀',
                            (false, true) => '▄',
                            (false, false) => ' ',
                        },
                    )
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

pub(crate) fn format_duration_short(d: Duration) -> String {
    let minutes = d.num_minutes();
    match minutes {
        m if m < 1 => "<1m".to_string(),
        m if m < 60 => format!("{m}m"),
        m if m < 24 * 60 => format!("{}h{}m", m / 60, m % 60),
        m => format!("{}d{}h", m / (24 * 60), (m / 60) % 24),
    }
}

pub(crate) fn overlay_lines(
    summary: &UsageSummary,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let context_age = summary
        .context_started_at
        .map(|at| format_duration_short(now - at))
        .unwrap_or_else(|| "?".to_string());
    vec![
        format!(
            "clr {} cmp {}/{}a rsm {}",
            summary.clears, summary.compactions, summary.compactions_auto, summary.resumes
        ),
        format!(
            "{context_age} {}p {}t age {}",
            summary.context_prompts,
            summary.context_turns,
            format_duration_short(now - created_at)
        ),
    ]
}

/// One right-aligned row of the overlay stack.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OverlayRow {
    pub text: String,
    pub style: Style,
}

/// The reset counter section: big digits, then the breakdown.
pub(crate) fn usage_rows(
    summary: &UsageSummary,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
    theme: &Theme,
) -> Vec<OverlayRow> {
    let number = Style::default().fg(theme.error).bold();
    let detail = Style::default().fg(theme.text);
    big_digits(summary.resets)
        .into_iter()
        .map(|text| OverlayRow {
            text,
            style: number,
        })
        .chain(
            overlay_lines(summary, created_at, now)
                .into_iter()
                .map(|text| OverlayRow {
                    text,
                    style: detail,
                }),
        )
        .collect()
}

pub(crate) fn render_usage_overlay(
    frame: &mut Frame,
    pane: Rect,
    summary: &UsageSummary,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
    theme: &Theme,
) {
    render_overlay_sections(
        frame,
        pane,
        &[usage_rows(summary, created_at, now, theme)],
        theme,
    );
}

/// Stacks sections top-down in the pane's top-right corner. A section joins
/// only while the stack still fits with room to spare; the first that does
/// not fit ends the stack, so nothing draws when the first section is cramped.
pub(crate) fn render_overlay_sections(
    frame: &mut Frame,
    pane: Rect,
    sections: &[Vec<OverlayRow>],
    theme: &Theme,
) {
    let mut rows: Vec<&OverlayRow> = Vec::new();
    let mut width = 0usize;
    for section in sections.iter().filter(|section| !section.is_empty()) {
        let section_width = section
            .iter()
            .map(|row| row.text.width())
            .max()
            .unwrap_or(0);
        let stacked_width = width.max(section_width);
        if (pane.width as usize) < stacked_width + 10
            || (pane.height as usize) < rows.len() + section.len() + 2
        {
            break;
        }
        width = stacked_width;
        rows.extend(section.iter());
    }
    if rows.is_empty() {
        return;
    }
    let width = width as u16;
    let height = rows.len() as u16;
    let area = Rect {
        x: pane.right() - width - 1,
        y: pane.y,
        width,
        height,
    };
    let text: Vec<Line> = rows
        .iter()
        .map(|row| Line::styled(row.text.clone(), row.style))
        .collect();

    // Terminals can't alpha-composite, so render the overlay into a scratch
    // buffer and blend it into the frame buffer by hand.
    let mut scratch = Buffer::empty(area);
    Paragraph::new(text)
        .alignment(Alignment::Right)
        .render(area, &mut scratch);

    let buf = frame.buffer_mut();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let pos = (x, y);
            let under_bg = buf[pos].bg;
            let under_fg = buf[pos].fg;
            let under_bg_is_rgb = matches!(under_bg, Color::Rgb(..));
            let base_bg = if under_bg_is_rgb {
                under_bg
            } else {
                theme.background
            };
            let backdrop = blend(base_bg, theme.background, BACKDROP_ALPHA);
            let over = scratch[pos].clone();
            if over.symbol() != " " {
                let cell = &mut buf[pos];
                cell.set_symbol(over.symbol());
                cell.modifier = over.modifier;
                cell.fg = blend(backdrop, over.fg, INK_ALPHA);
                // aoe never paints `theme.background`, so most preview cells carry
                // the terminal's own non-Rgb background. Only overwrite bg when we
                // know what's underneath, or the overlay paints a solid rectangle.
                if under_bg_is_rgb {
                    cell.bg = backdrop;
                }
            } else {
                let under_fg = match under_fg {
                    Color::Rgb(..) => under_fg,
                    _ => theme.text,
                };
                let cell = &mut buf[pos];
                if under_bg_is_rgb {
                    cell.bg = backdrop;
                }
                cell.fg = blend(under_fg, backdrop, BACKDROP_ALPHA);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn digits_pack_two_pixel_rows_per_line() {
        assert_eq!(big_digits(0), ["█▀█", "█ █", "▀▀▀"]);
        assert_eq!(big_digits(1), ["▄█ ", " █ ", "▀▀▀"]);
        assert_eq!(big_digits(47), ["█ █ ▀▀█", "▀▀█   █", "  ▀   ▀"]);
    }

    #[test]
    fn durations_use_the_two_largest_units() {
        let cases = [
            (Duration::seconds(20), "<1m"),
            (Duration::minutes(42), "42m"),
            (Duration::minutes(125), "2h5m"),
            (Duration::hours(76), "3d4h"),
            (Duration::seconds(-5), "<1m"),
        ];
        for (d, want) in cases {
            assert_eq!(format_duration_short(d), want);
        }
    }

    #[test]
    fn lines_show_breakdown_context_and_age() {
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
        let summary = UsageSummary {
            resets: 17,
            clears: 12,
            compactions: 3,
            compactions_auto: 2,
            compactions_manual: 1,
            resumes: 2,
            context_started_at: Some(now - Duration::minutes(42)),
            context_prompts: 18,
            context_turns: 17,
            ..UsageSummary::default()
        };
        assert_eq!(
            overlay_lines(&summary, now - Duration::hours(76), now),
            vec![
                "clr 12 cmp 3/2a rsm 2".to_string(),
                "42m 18p 17t age 3d4h".to_string(),
            ]
        );
    }

    #[test]
    fn overlay_draws_top_right_and_hides_when_cramped() {
        use ratatui::backend::TestBackend;
        let theme = Theme::default();
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
        let summary = UsageSummary {
            resets: 7,
            ..UsageSummary::default()
        };
        let draw = |w: u16, h: u16| {
            let mut terminal = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| render_usage_overlay(f, f.area(), &summary, now, now, &theme))
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let wide = draw(80, 20);
        let top_right: String = (40..80)
            .map(|x| wide[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            top_right.contains('█') || top_right.contains('▀') || top_right.contains('▄'),
            "{top_right}"
        );
        let cramped = draw(20, 5);
        assert!(cramped.content().iter().all(|c| c.symbol() == " "));
    }

    #[test]
    fn overlay_blends_over_the_backdrop() {
        use ratatui::backend::TestBackend;
        let theme = Theme::default();
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
        let summary = UsageSummary {
            resets: 7,
            ..UsageSummary::default()
        };
        let mut terminal = ratatui::Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                let row = "x".repeat(area.width as usize);
                let fill = Paragraph::new(vec![Line::raw(row); area.height as usize]).style(
                    Style::default()
                        .fg(Color::Rgb(200, 200, 200))
                        .bg(Color::Rgb(0, 0, 0)),
                );
                f.render_widget(fill, area);
                render_usage_overlay(f, area, &summary, now, now, &theme);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let backdrop = blend(Color::Rgb(0, 0, 0), theme.background, BACKDROP_ALPHA);
        let padding = buf
            .content()
            .iter()
            .find(|c| c.symbol() == "x" && c.bg == backdrop)
            .expect("a padding cell should keep the underlying fill");
        assert_eq!(
            padding.fg,
            blend(Color::Rgb(200, 200, 200), backdrop, BACKDROP_ALPHA)
        );
        let digit = buf
            .content()
            .iter()
            .find(|c| matches!(c.symbol(), "█" | "▀" | "▄"))
            .expect("a digit glyph cell should be drawn");
        assert_eq!(digit.bg, backdrop);
        assert_eq!(digit.fg, blend(backdrop, theme.error, INK_ALPHA));
    }

    #[test]
    fn overlay_leaves_a_reset_background_alone() {
        use ratatui::backend::TestBackend;
        let theme = Theme::default();
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
        let summary = UsageSummary {
            resets: 7,
            ..UsageSummary::default()
        };
        let mut terminal = ratatui::Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|f| render_usage_overlay(f, f.area(), &summary, now, now, &theme))
            .unwrap();
        let buf = terminal.backend().buffer();
        // A padding cell over the terminal's own (non-Rgb) background: bg must
        // stay Reset rather than getting painted with the theme's background.
        let padding = buf
            .content()
            .iter()
            .find(|c| c.symbol() == " " && c.bg == Color::Reset && c.fg != Color::Reset)
            .expect("a padding cell over a Reset background should be found");
        assert_eq!(padding.bg, Color::Reset);
        assert_eq!(
            padding.fg,
            blend(theme.text, theme.background, BACKDROP_ALPHA)
        );
        // Same for a digit glyph cell: ink still blends against theme.background,
        // but bg is left untouched.
        let digit = buf
            .content()
            .iter()
            .find(|c| matches!(c.symbol(), "█" | "▀" | "▄"))
            .expect("a digit glyph cell should be drawn");
        assert_eq!(digit.bg, Color::Reset);
        assert_eq!(digit.fg, blend(theme.background, theme.error, INK_ALPHA));
    }

    fn row(text: &str) -> OverlayRow {
        OverlayRow {
            text: text.to_string(),
            style: Style::default(),
        }
    }

    fn draw_sections(w: u16, h: u16, sections: &[Vec<OverlayRow>]) -> Buffer {
        use ratatui::backend::TestBackend;
        let theme = Theme::default();
        let mut terminal = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| render_overlay_sections(f, f.area(), sections, &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn line_at(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
    }

    #[test]
    fn sections_stack_right_aligned_and_skip_empty_ones() {
        let buf = draw_sections(
            60,
            10,
            &[
                vec![],
                vec![row("ledger d861 current")],
                vec![row("hr 2.0k saved")],
            ],
        );
        assert!(line_at(&buf, 0).trim_end().ends_with("ledger d861 current"));
        assert!(line_at(&buf, 1).trim_end().ends_with("hr 2.0k saved"));
    }

    #[test]
    fn a_section_too_wide_for_the_pane_is_dropped_with_the_rest() {
        let wide = "x".repeat(55);
        let buf = draw_sections(
            40,
            10,
            &[vec![row("first")], vec![row(&wide)], vec![row("third")]],
        );
        assert!(line_at(&buf, 0).contains("first"));
        assert!(!line_at(&buf, 1).contains('x'));
        assert!(!(0..10).any(|y| line_at(&buf, y).contains("third")));
    }
}
