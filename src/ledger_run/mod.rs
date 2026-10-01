//! The Ledger run overlay's data: `ledger run show` for each Ledger run of a
//! session, folded into one view that the TUI and web overlays render.
//! Contract: docs/development/ledger-run.md.

mod runner;

use serde::{Deserialize, Serialize};

pub use runner::load_view;

pub const SHOW_SCHEMA: &str = "ledger.run.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation {
    pub state: String,
    pub launched: String,
    pub current: String,
    pub current_since: String,
    pub drift: Vec<Change>,
    pub runtime_drift: Vec<RuntimeChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub kind: String,
    pub name: String,
    pub change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeChange {
    pub kind: String,
    pub change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunHeadroom {
    pub requests: u64,
    pub input_tokens_before: u64,
    pub input_tokens_after: u64,
    pub saved_tokens: u64,
    pub model: String,
}

/// The fields of `ledger run show --json` the overlay reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunShow {
    pub schema: String,
    pub run_id: String,
    pub finished_at: Option<String>,
    pub generation: Option<Generation>,
    pub headroom: Option<RunHeadroom>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadroomTotals {
    pub requests: u64,
    pub input_tokens_before: u64,
    pub input_tokens_after: u64,
    pub saved_tokens: u64,
    /// The most recent run's model.
    pub model: String,
    /// Sum of the runs that have a known price; `None` when none do.
    pub estimated_cents: Option<u64>,
}

impl HeadroomTotals {
    fn from_run(run: &RunHeadroom) -> Self {
        Self {
            requests: run.requests,
            input_tokens_before: run.input_tokens_before,
            input_tokens_after: run.input_tokens_after,
            saved_tokens: run.saved_tokens,
            model: run.model.clone(),
            estimated_cents: crate::usage::prices::estimate_cents(run.saved_tokens, &run.model),
        }
    }

    fn plus(self, later: &Self) -> Self {
        let estimated_cents = match (self.estimated_cents, later.estimated_cents) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        Self {
            requests: self.requests + later.requests,
            input_tokens_before: self.input_tokens_before + later.input_tokens_before,
            input_tokens_after: self.input_tokens_after + later.input_tokens_after,
            saved_tokens: self.saved_tokens + later.saved_tokens,
            model: later.model.clone(),
            estimated_cents,
        }
    }
}

/// One session's overlay data. Drift comes from the current (last) run; the
/// Headroom total spans every run the session has had.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerRunView {
    pub run_id: String,
    pub runs: usize,
    pub generation: Option<Generation>,
    pub headroom_total: Option<HeadroomTotals>,
    pub headroom_now: Option<HeadroomTotals>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowOutcome {
    Ok(RunShow),
    /// No `ledger` binary on PATH.
    Missing,
    Timeout,
    UnknownRun,
    Failed,
}

impl ShowOutcome {
    pub fn reason(&self) -> &'static str {
        match self {
            ShowOutcome::Ok(_) => "",
            ShowOutcome::Missing => "no ledger",
            ShowOutcome::Timeout => "timeout",
            ShowOutcome::UnknownRun => "unknown run",
            ShowOutcome::Failed => "ledger error",
        }
    }
}

/// Exit 0 carries the view; exit 2 is Ledger's "unknown run".
pub fn parse_output(code: Option<i32>, stdout: &[u8]) -> ShowOutcome {
    match code {
        Some(0) => serde_json::from_slice::<RunShow>(stdout)
            .ok()
            .filter(|show| show.schema == SHOW_SCHEMA)
            .map_or(ShowOutcome::Failed, ShowOutcome::Ok),
        Some(2) => ShowOutcome::UnknownRun,
        _ => ShowOutcome::Failed,
    }
}

/// The recorded runs with the pane's current run last.
pub fn merge_runs(mut history: Vec<String>, current: &str) -> Vec<String> {
    history.retain(|id| id != current);
    history.push(current.to_string());
    history
}

pub fn build_view(runs: &[String], show: &mut dyn FnMut(&str) -> ShowOutcome) -> LedgerRunView {
    let Some((current, earlier)) = runs.split_last() else {
        return LedgerRunView::default();
    };
    let mut view = LedgerRunView {
        run_id: current.clone(),
        runs: runs.len(),
        ..LedgerRunView::default()
    };
    let current_show = match show(current) {
        ShowOutcome::Ok(current_show) => current_show,
        failed => {
            view.error = Some(failed.reason().to_string());
            return view;
        }
    };
    view.generation = current_show.generation;
    view.headroom_now = current_show.headroom.as_ref().map(HeadroomTotals::from_run);
    let mut total: Option<HeadroomTotals> = None;
    for id in earlier {
        if let ShowOutcome::Ok(earlier_show) = show(id) {
            if let Some(headroom) = &earlier_show.headroom {
                let run = HeadroomTotals::from_run(headroom);
                total = Some(match total {
                    Some(sum) => sum.plus(&run),
                    None => run,
                });
            }
        }
    }
    if let Some(now) = &view.headroom_now {
        total = Some(match total {
            Some(sum) => sum.plus(now),
            None => now.clone(),
        });
    }
    view.headroom_total = total;
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ledger-run")
    }

    fn read_show(name: &str) -> RunShow {
        let text = std::fs::read_to_string(fixtures().join(name)).unwrap();
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        shows: Vec<String>,
        #[serde(default)]
        run_ids: Vec<String>,
        outcome: Option<String>,
        view: serde_json::Value,
    }

    #[test]
    fn fixture_cases_build_their_views() {
        let text = std::fs::read_to_string(fixtures().join("cases.json")).unwrap();
        let cases: Vec<Case> = serde_json::from_str(&text).unwrap();
        assert!(cases.len() >= 7);
        for case in cases {
            let shows: Vec<RunShow> = case.shows.iter().map(|name| read_show(name)).collect();
            let runs: Vec<String> = if shows.is_empty() {
                case.run_ids.clone()
            } else {
                shows.iter().map(|show| show.run_id.clone()).collect()
            };
            let mut show = |id: &str| match case.outcome.as_deref() {
                Some("timeout") => ShowOutcome::Timeout,
                Some(other) => panic!("unknown outcome {other}"),
                None => ShowOutcome::Ok(shows.iter().find(|s| s.run_id == id).unwrap().clone()),
            };
            let view = build_view(&runs, &mut show);
            assert_eq!(
                serde_json::to_value(&view).unwrap(),
                case.view,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn output_maps_exit_codes_and_schema() {
        let good = std::fs::read(fixtures().join("behind.json")).unwrap();
        assert!(matches!(parse_output(Some(0), &good), ShowOutcome::Ok(_)));
        let other_schema = String::from_utf8(good.clone())
            .unwrap()
            .replace("ledger.run.v1", "ledger.run.v9");
        assert_eq!(
            parse_output(Some(0), other_schema.as_bytes()),
            ShowOutcome::Failed
        );
        assert_eq!(parse_output(Some(0), b"not json"), ShowOutcome::Failed);
        assert_eq!(
            parse_output(Some(2), br#"{"error":"unknown_run"}"#),
            ShowOutcome::UnknownRun
        );
        assert_eq!(parse_output(Some(1), b""), ShowOutcome::Failed);
        assert_eq!(parse_output(None, b""), ShowOutcome::Failed);
    }

    #[test]
    fn the_current_run_goes_last_once() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(merge_runs(s(&["a", "b"]), "b"), s(&["a", "b"]));
        assert_eq!(merge_runs(s(&["a"]), "c"), s(&["a", "c"]));
        assert_eq!(merge_runs(s(&["a", "b"]), "a"), s(&["b", "a"]));
        assert_eq!(merge_runs(Vec::new(), "c"), s(&["c"]));
    }

    #[test]
    fn a_failed_earlier_run_is_left_out_and_a_failed_current_run_is_the_error() {
        let finished = read_show("finished.json");
        let live = read_show("live-now.json");
        let runs = vec![finished.run_id.clone(), live.run_id.clone()];
        let view = build_view(&runs, &mut |id: &str| {
            if id == live.run_id {
                ShowOutcome::Ok(live.clone())
            } else {
                ShowOutcome::Timeout
            }
        });
        assert_eq!(view.error, None);
        assert_eq!(view.headroom_total.as_ref().unwrap().saved_tokens, 26_500);
        let view = build_view(&runs, &mut |_: &str| ShowOutcome::Missing);
        assert_eq!(view.error.as_deref(), Some("no ledger"));
        assert_eq!((view.generation, view.headroom_total), (None, None));
    }
}
