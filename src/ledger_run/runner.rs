//! Runs `ledger run show` with a deadline, caches finished runs for the
//! process lifetime, and backs off when the `ledger` binary is missing.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::{LedgerRunView, RunShow, ShowOutcome};

const SHOW_TIMEOUT: Duration = Duration::from_secs(2);
const SHOW_MAX_BYTES: usize = 64 * 1024;
const MISSING_BACKOFF: Duration = Duration::from_secs(5 * 60);

pub(crate) struct ShowCache {
    finished: HashMap<String, RunShow>,
    missing_until: Option<Instant>,
}

impl ShowCache {
    pub(crate) fn new() -> Self {
        Self {
            finished: HashMap::new(),
            missing_until: None,
        }
    }

    pub(crate) fn show(
        &mut self,
        run_id: &str,
        now: Instant,
        run: &mut dyn FnMut(&str) -> ShowOutcome,
    ) -> ShowOutcome {
        if let Some(show) = self.finished.get(run_id) {
            return ShowOutcome::Ok(show.clone());
        }
        if self.missing_until.is_some_and(|until| now < until) {
            return ShowOutcome::Missing;
        }
        let outcome = run(run_id);
        match &outcome {
            ShowOutcome::Ok(show) if show.finished_at.is_some() => {
                self.finished.insert(run_id.to_string(), show.clone());
            }
            ShowOutcome::Missing => self.missing_until = Some(now + MISSING_BACKOFF),
            _ => {}
        }
        outcome
    }
}

/// Shared by the TUI poller and the web server, so each process asks Ledger
/// about a finished run once.
static CACHE: LazyLock<Mutex<ShowCache>> = LazyLock::new(|| Mutex::new(ShowCache::new()));

fn run_show(run_id: &str) -> ShowOutcome {
    let mut command = std::process::Command::new("ledger");
    command.args(["run", "show", run_id, "--json"]);
    match crate::process::run_with_bounded_stdout(&mut command, SHOW_TIMEOUT, SHOW_MAX_BYTES) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ShowOutcome::Missing,
        Err(_) => ShowOutcome::Failed,
        Ok(None) => ShowOutcome::Timeout,
        Ok(Some(output)) => super::parse_output(output.status.code(), &output.stdout),
    }
}

/// The session's overlay view, or `None` when Ledger did not launch its
/// agent (no `@aoe_ledger_launch` pane option). Blocks on `ledger`; call it
/// off the render loop.
pub fn load_view(instance: &crate::session::Instance) -> Option<LedgerRunView> {
    let current = crate::session::ledger_restart::current_ledger_run(instance)?;
    let history = crate::usage::UsageStore::open_default()
        .and_then(|store| store.events_for_instance(&instance.id))
        .map(|events| crate::usage::ledger_runs(&events))
        .unwrap_or_default();
    let runs = super::merge_runs(history, &current);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    Some(super::build_view(&runs, &mut |id: &str| {
        cache.show(id, now, &mut |id: &str| run_show(id))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn show(run_id: &str, finished: bool) -> ShowOutcome {
        ShowOutcome::Ok(crate::ledger_run::RunShow {
            schema: crate::ledger_run::SHOW_SCHEMA.into(),
            run_id: run_id.into(),
            finished_at: finished.then(|| "2026-10-01T10:00:00Z".to_string()),
            generation: None,
            headroom: None,
        })
    }

    #[test]
    fn finished_runs_are_asked_once_and_live_runs_every_time() {
        let mut cache = ShowCache::new();
        let mut calls = Vec::new();
        let now = Instant::now();
        for _ in 0..3 {
            cache.show("run_done", now, &mut |id: &str| {
                calls.push(id.to_string());
                show(id, true)
            });
            cache.show("run_live", now, &mut |id: &str| {
                calls.push(id.to_string());
                show(id, false)
            });
        }
        assert_eq!(calls.iter().filter(|id| *id == "run_done").count(), 1);
        assert_eq!(calls.iter().filter(|id| *id == "run_live").count(), 3);
    }

    #[test]
    fn a_missing_binary_is_not_retried_for_five_minutes() {
        let mut cache = ShowCache::new();
        let calls = std::cell::Cell::new(0);
        let start = Instant::now();
        let mut missing = |_: &str| {
            calls.set(calls.get() + 1);
            ShowOutcome::Missing
        };
        assert_eq!(
            cache.show("run_a", start, &mut missing),
            ShowOutcome::Missing
        );
        assert_eq!(
            cache.show("run_a", start + Duration::from_secs(299), &mut missing),
            ShowOutcome::Missing
        );
        assert_eq!(calls.get(), 1);
        cache.show("run_a", start + Duration::from_secs(301), &mut missing);
        assert_eq!(calls.get(), 2);
    }
}
