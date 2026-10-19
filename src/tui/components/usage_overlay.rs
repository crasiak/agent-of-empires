//! Usage/reset and Ledger rows in the session info panel.

use chrono::{DateTime, Duration, Utc};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::styles::Theme;
use crate::usage::UsageSummary;

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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OverlayRow {
    pub text: String,
    pub style: Style,
}

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

pub(crate) fn render_info_sections(frame: &mut Frame, area: Rect, sections: &[Vec<OverlayRow>]) {
    let sections: Vec<_> = sections.iter().filter(|rows| !rows.is_empty()).collect();
    let mut lines = Vec::new();
    for (index, section) in sections.iter().enumerate() {
        // Keep a long drift description from displacing Headroom entirely.
        let remaining = (area.height as usize).saturating_sub(lines.len());
        let budget = remaining.saturating_sub(sections.len() - index - 1);
        for (row_index, row) in section.iter().take(budget).enumerate() {
            let text = if row_index + 1 == budget && section.len() > budget {
                format!("{} …", row.text)
            } else {
                row.text.clone()
            };
            lines.push(Line::styled(
                super::truncate_to_width(&text, area.width as usize),
                row.style,
            ));
        }
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Right), area);
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
        for (d, want) in [
            (Duration::seconds(20), "<1m"),
            (Duration::minutes(42), "42m"),
            (Duration::minutes(125), "2h5m"),
            (Duration::hours(76), "3d4h"),
            (Duration::seconds(-5), "<1m"),
        ] {
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
            vec!["clr 12 cmp 3/2a rsm 2", "42m 18p 17t age 3d4h",]
        );
    }

    #[test]
    fn info_sections_stay_in_their_allocated_rect() {
        use ratatui::backend::TestBackend;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(40, 12)).unwrap();
        let area = Rect::new(15, 1, 20, 6);
        let sections = vec![
            vec![
                OverlayRow {
                    text: "ledger d861 current".to_string(),
                    style: Style::default().fg(Color::Red),
                };
                8
            ],
            vec![OverlayRow {
                text: "hr 2.0k saved and a long suffix".to_string(),
                style: Style::default(),
            }],
        ];
        terminal
            .draw(|f| {
                f.render_widget(
                    Paragraph::new(vec![Line::raw("x".repeat(40)); 12]),
                    f.area(),
                );
                render_info_sections(f, area, &sections);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        for y in 0..12 {
            for x in 0..40 {
                if !area.contains(Position::new(x, y)) {
                    assert_eq!(buffer[(x, y)].symbol(), "x");
                }
            }
        }
        let row = |y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        };
        assert!(row(1).contains("ledger d861 current"));
        assert!(row(5).contains('…'));
        assert!(row(6).contains("hr 2.0k saved"));
        assert_eq!(buffer[(16, 1)].fg, Color::Red);
    }
}
