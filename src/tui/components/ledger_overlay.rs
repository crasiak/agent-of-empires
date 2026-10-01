//! The Ledger run lines under the reset counter: the session's generation
//! drift and its Headroom savings. `web/src/lib/ledgerRun.ts` mirrors this
//! file; both are pinned to `tests/fixtures/ledger-run/cases.json`.

use chrono::{DateTime, Utc};
use ratatui::style::Style;

use super::usage_overlay::{format_duration_short, OverlayRow};
use crate::ledger_run::{Change, LedgerRunView};
use crate::tui::styles::Theme;

/// Skills named on the drift line before the rest become `+N more`.
const MAX_SKILLS: usize = 6;

pub(crate) struct DriftLine {
    pub text: String,
    pub dim: bool,
}

fn marker(change: &str) -> &'static str {
    match change {
        "older" => "↓",
        "newer" => "↑",
        "extra" => "+",
        "missing" => "-",
        _ => "~",
    }
}

fn short(id: &str) -> String {
    id.chars().take(4).collect()
}

pub(crate) fn drift_line(view: &LedgerRunView, now: DateTime<Utc>) -> Option<DriftLine> {
    if let Some(error) = &view.error {
        return Some(DriftLine {
            text: format!("ledger ? {error}"),
            dim: true,
        });
    }
    let generation = view.generation.as_ref()?;
    if generation.state == "current" {
        return Some(DriftLine {
            text: format!("ledger {} current", short(&generation.launched)),
            dim: false,
        });
    }
    let age = DateTime::parse_from_rfc3339(&generation.current_since)
        .map(|since| format_duration_short(now - since.with_timezone(&Utc)))
        .unwrap_or_else(|_| "?".to_string());
    let mut segments = vec![format!(
        "ledger {} {}↔{} {age}",
        generation.state,
        short(&generation.launched),
        short(&generation.current)
    )];

    let skills: Vec<&Change> = generation
        .drift
        .iter()
        .filter(|change| change.kind == "skill")
        .collect();
    let mut named: Vec<String> = skills
        .iter()
        .take(MAX_SKILLS)
        .map(|change| format!("{}{}", marker(&change.change), change.name))
        .collect();
    if skills.len() > MAX_SKILLS {
        named.push(format!("+{} more", skills.len() - MAX_SKILLS));
    }
    if !named.is_empty() {
        segments.push(named.join(" "));
    }

    // Other kinds collapse to a count per kind, marked when they all moved
    // the same way.
    let mut kinds: Vec<&str> = Vec::new();
    for change in &generation.drift {
        if change.kind != "skill" && !kinds.contains(&change.kind.as_str()) {
            kinds.push(&change.kind);
        }
    }
    let mut counted: Vec<String> = kinds
        .iter()
        .map(|kind| {
            let changes: Vec<&str> = generation
                .drift
                .iter()
                .filter(|change| change.kind == *kind)
                .map(|change| change.change.as_str())
                .collect();
            let mark = if changes.iter().all(|change| *change == changes[0]) {
                marker(changes[0])
            } else {
                "~"
            };
            format!("{mark}{} {kind}", changes.len())
        })
        .collect();
    counted.extend(
        generation
            .runtime_drift
            .iter()
            .map(|change| format!("{}{}", marker(&change.change), change.kind)),
    );
    if !counted.is_empty() {
        segments.push(counted.join(" "));
    }
    Some(DriftLine {
        text: segments.join("  "),
        dim: false,
    })
}

/// `n`, `X.Yk` or `X.YM`, rounded down. Integer math keeps the web copy exact.
pub(crate) fn compact_count(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        let tenths = n / 100;
        format!("{}.{}k", tenths / 10, tenths % 10)
    } else {
        let tenths = n / 100_000;
        format!("{}.{}M", tenths / 10, tenths % 10)
    }
}

pub(crate) fn headroom_line(view: &LedgerRunView) -> Option<String> {
    if view.error.is_some() {
        return None;
    }
    let total = view.headroom_total.as_ref()?;
    let percent = (total.saved_tokens * 100)
        .checked_div(total.input_tokens_before)
        .unwrap_or(0);
    let mut parts = vec![
        format!("hr {} saved", compact_count(total.saved_tokens)),
        format!("{percent}%"),
    ];
    if let Some(cents) = total.estimated_cents {
        parts.push(format!("~${}.{:02}", cents / 100, cents % 100));
    }
    let mut line = parts.join(" · ");
    if view.runs > 1 {
        if let Some(now) = &view.headroom_now {
            line.push_str(&format!("  (now {})", compact_count(now.saved_tokens)));
        }
    }
    Some(line)
}

pub(crate) fn drift_rows(
    view: &LedgerRunView,
    now: DateTime<Utc>,
    theme: &Theme,
) -> Vec<OverlayRow> {
    drift_line(view, now)
        .map(|line| OverlayRow {
            style: Style::default().fg(if line.dim { theme.dimmed } else { theme.text }),
            text: line.text,
        })
        .into_iter()
        .collect()
}

pub(crate) fn headroom_rows(view: &LedgerRunView, theme: &Theme) -> Vec<OverlayRow> {
    headroom_line(view)
        .map(|text| OverlayRow {
            text,
            style: Style::default().fg(theme.text),
        })
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        now: DateTime<Utc>,
        view: LedgerRunView,
        lines: Vec<String>,
    }

    fn cases() -> Vec<Case> {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ledger-run/cases.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn fixture_cases_render_their_lines() {
        for case in cases() {
            let lines: Vec<String> = drift_line(&case.view, case.now)
                .map(|line| line.text)
                .into_iter()
                .chain(headroom_line(&case.view))
                .collect();
            assert_eq!(lines, case.lines, "{}", case.name);
        }
    }

    #[test]
    fn only_the_error_line_is_dim() {
        for case in cases() {
            let dim = drift_line(&case.view, case.now).is_some_and(|line| line.dim);
            assert_eq!(dim, case.view.error.is_some(), "{}", case.name);
        }
    }

    #[test]
    fn counts_round_down_to_one_decimal() {
        let cases = [
            (0, "0"),
            (999, "999"),
            (1_000, "1.0k"),
            (26_500, "26.5k"),
            (999_999, "999.9k"),
            (1_250_000, "1.2M"),
            (1_132_000, "1.1M"),
        ];
        for (n, want) in cases {
            assert_eq!(compact_count(n), want);
        }
    }

    #[test]
    fn rows_use_dim_ink_only_for_errors() {
        let theme = crate::tui::styles::Theme::default();
        for case in cases() {
            let drift = drift_rows(&case.view, case.now, &theme);
            if let Some(row) = drift.first() {
                let want = if case.view.error.is_some() {
                    theme.dimmed
                } else {
                    theme.text
                };
                assert_eq!(row.style.fg, Some(want), "{}", case.name);
            }
            let headroom = headroom_rows(&case.view, &theme);
            assert_eq!(
                headroom.len(),
                headroom_line(&case.view).iter().count(),
                "{}",
                case.name
            );
        }
    }
}
