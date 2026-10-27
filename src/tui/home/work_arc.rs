use super::*;
use crate::session::work_arc::{self as arc, ArcState, ArcStatus, SourceError};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(super) struct ArcRefresh {
    pub(super) task: Option<tokio::task::JoinHandle<()>>,
    target: Option<Instance>,
    updates: mpsc::Receiver<ArcUpdate>,
    sender: mpsc::Sender<ArcUpdate>,
    pub(super) checked: Option<(String, Instant)>,
    pub consent: Option<Instance>,
}

struct ArcUpdate {
    before: Instance,
    state: Option<Box<ArcState>>,
    status: ArcStatus,
}

impl Default for ArcRefresh {
    fn default() -> Self {
        let (sender, updates) = mpsc::channel();
        Self {
            task: None,
            target: None,
            updates,
            sender,
            checked: None,
            consent: None,
        }
    }
}

impl Drop for ArcRefresh {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn unchanged(row: &Instance, before: &Instance) -> bool {
    row.work_arc_enabled
        && row.work_arc.epoch == before.work_arc.epoch
        && row.agent_session_id == before.agent_session_id
        && row.active_execution == before.active_execution
        && row.lifecycle_generation == before.lifecycle_generation
}

fn source_status(error: SourceError) -> ArcStatus {
    match error {
        SourceError::Disabled => ArcStatus::Off,
        SourceError::Missing => ArcStatus::Missing,
        SourceError::Unavailable => ArcStatus::Unavailable,
        SourceError::Stale => ArcStatus::Stale,
        SourceError::Failed => ArcStatus::Failed,
    }
}

fn unique_owner(before: &Instance) -> bool {
    let Ok(profiles) = crate::session::list_profiles() else {
        return false;
    };
    for profile in profiles {
        let Ok(storage) = Storage::open(&profile, crate::file_watch::FileWatchService::noop())
        else {
            return false;
        };
        let Ok(rows) = storage.load() else {
            return false;
        };
        if rows.iter().any(|row| {
            (row.id != before.id || profile != before.source_profile)
                && (row.id == before.id
                    || (row.agent_session_id.is_some()
                        && row.agent_session_id == before.agent_session_id))
        }) {
            return false;
        }
    }
    true
}

async fn refresh(before: Instance, sender: mpsc::Sender<ArcUpdate>) {
    let snapshot = before.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        let storage = Storage::open(
            &snapshot.source_profile,
            crate::file_watch::FileWatchService::noop(),
        )
        .map_err(|_| SourceError::Unavailable)?;
        let rows = storage.load().map_err(|_| SourceError::Unavailable)?;
        let row = rows
            .into_iter()
            .find(|row| row.id == snapshot.id && unchanged(row, &snapshot))
            .ok_or(SourceError::Disabled)?;
        if !unique_owner(&row) {
            return Err(SourceError::Unavailable);
        }
        let bound = arc::read_arc_source(&row)?;
        Ok((storage, bound))
    })
    .await;
    let (storage, bound) = match prepared {
        Ok(Ok(value)) => value,
        result => {
            let error = result
                .ok()
                .and_then(Result::err)
                .unwrap_or(SourceError::Failed);
            let _ = sender.send(ArcUpdate {
                before,
                state: None,
                status: source_status(error),
            });
            return;
        }
    };
    if arc::check_arc_route(&bound).await.is_err() {
        let _ = sender.send(ArcUpdate {
            before,
            state: None,
            status: ArcStatus::Unavailable,
        });
        return;
    }
    let identity = bound.identity();
    let reserved = storage.update(|rows, _| {
        let Some(row) = rows
            .iter_mut()
            .find(|row| row.id == before.id && unchanged(row, &before))
        else {
            return Ok(None);
        };
        if !arc::read_arc_source(row).is_ok_and(|latest| latest.identity() == identity) {
            return Ok(None);
        }
        let nonce = row.work_arc.reserve(&identity, chrono::Utc::now());
        Ok(Some((nonce, row.work_arc.clone())))
    });
    let (nonce, state) = match reserved {
        Ok(Some((Some(nonce), state))) => (nonce, state),
        Ok(Some((None, state))) => {
            let status = if state
                .record
                .as_ref()
                .is_some_and(|r| r.identity == identity)
            {
                ArcStatus::Current
            } else if state.unavailable {
                ArcStatus::Unavailable
            } else if state.failed {
                ArcStatus::Failed
            } else if state.record.is_some() {
                ArcStatus::Stale
            } else {
                ArcStatus::Missing
            };
            let _ = sender.send(ArcUpdate {
                before,
                state: Some(state),
                status,
            });
            return;
        }
        _ => {
            let _ = sender.send(ArcUpdate {
                before,
                state: None,
                status: ArcStatus::Unavailable,
            });
            return;
        }
    };
    let _ = sender.send(ArcUpdate {
        before: before.clone(),
        state: Some(state),
        status: ArcStatus::Loading,
    });
    let result = arc::request_arc(bound).await;
    let snapshot = before.clone();
    let finished = tokio::task::spawn_blocking(move || {
        let rows = storage.load().map_err(|_| SourceError::Failed)?;
        let row = rows
            .into_iter()
            .find(|row| row.id == snapshot.id && unchanged(row, &snapshot))
            .ok_or(SourceError::Disabled)?;
        if !unique_owner(&row) {
            return Err(SourceError::Unavailable);
        }
        let status;
        let attempt_usage;
        let record = match result {
            Ok(generated) => {
                attempt_usage = generated.usage.clone();
                let latest = arc::read_arc_source(&row)?;
                if latest.identity() != identity {
                    return Err(SourceError::Stale);
                }
                status = ArcStatus::Current;
                Some(arc::StoredArc {
                    identity: generated.bound.identity(),
                    at: chrono::Utc::now(),
                    arc: generated.arc,
                    selected_route: generated.selected_route,
                    provider: generated.provider,
                    model: generated.model,
                    usage: generated.usage,
                })
            }
            Err(failure) => {
                status = source_status(failure.error);
                attempt_usage = failure.usage;
                None
            }
        };
        storage
            .update(|rows, _| {
                let Some(row) = rows
                    .iter_mut()
                    .find(|row| row.id == snapshot.id && unchanged(row, &snapshot))
                else {
                    return Ok(None);
                };
                if !identity.matches(row)
                    || !unique_owner(row)
                    || (record.is_some()
                        && !arc::read_arc_source(row)
                            .is_ok_and(|latest| latest.identity() == identity))
                    || !row.work_arc.finish(&nonce, record, attempt_usage)
                {
                    return Ok(None);
                }
                if status == ArcStatus::Unavailable {
                    if row.work_arc.last_attempt_usage.is_none() {
                        row.work_arc.cost = arc::ArcCost::NoRequest;
                    }
                    row.work_arc.unavailable = true;
                }
                Ok(Some((row.work_arc.clone(), status)))
            })
            .map_err(|_| SourceError::Failed)
    })
    .await;
    let (state, status) = match finished {
        Ok(Ok(Some((state, status)))) => (Some(state), status),
        Ok(Err(error)) => (None, source_status(error)),
        _ => (None, ArcStatus::Failed),
    };
    let _ = sender.send(ArcUpdate {
        before,
        state,
        status,
    });
}

impl HomeView {
    pub(super) fn invalidate_work_arc_selection(&mut self) {
        if let Some(task) = self.work_arc_refresh.task.take() {
            task.abort();
        }
        self.work_arc_refresh.target = None;
        self.work_arc_refresh.checked = None;
        for row in self.instances.values_mut() {
            if matches!(
                row.work_arc_status,
                Some(ArcStatus::Current | ArcStatus::Loading)
            ) {
                row.work_arc_status = None;
            }
        }
    }

    pub(super) fn toggle_work_arc(&mut self) {
        let Some(before) = self
            .selected_session
            .as_ref()
            .and_then(|id| self.get_instance(id))
            .cloned()
        else {
            return;
        };
        if !arc::supported(&before) {
            return;
        }
        if before.work_arc_enabled {
            self.invalidate_work_arc_selection();
            if let Err(error) = self.apply_user_action(&before.id, |row| {
                row.work_arc_enabled = false;
                row.work_arc.clear();
                row.work_arc_status = None;
            }) {
                self.info_dialog = Some(InfoDialog::new(
                    "Work arc",
                    &format!("Could not disable: {error}"),
                ));
            }
        } else {
            self.work_arc_refresh.consent = Some(before);
            self.confirm_dialog = Some(ConfirmDialog::new(
                "Enable work arc for this Session?",
                "AoE will read bounded user/assistant text and send it through this Pi process's selected model route. Provider charges may apply; a virtual route may use another physical model. Only derived cues and spend limits are saved locally. No generation while the TUI is closed. At most 1 attempt/5 minutes and 20/day. Disable removes the cue. Enable only this Session?",
                "enable_work_arc",
            ).neutral().buttons("Enable", "Cancel"));
        }
    }

    pub(super) fn confirm_work_arc(&mut self) {
        let Some(before) = self.work_arc_refresh.consent.take() else {
            return;
        };
        if !self.get_instance(&before.id).is_some_and(|row| {
            row.agent_session_id == before.agent_session_id
                && row.active_execution == before.active_execution
                && row.work_arc.epoch == before.work_arc.epoch
                && arc::supported(row)
        }) {
            return;
        }
        if let Err(error) = self.apply_user_action(&before.id, |row| {
            row.work_arc_enabled = true;
            row.work_arc.clear();
            row.work_arc.cleared = false;
            row.work_arc_status = None;
        }) {
            self.info_dialog = Some(InfoDialog::new(
                "Work arc",
                &format!("Could not enable: {error}"),
            ));
        }
        if let Some(disk) = self
            .storages
            .get(&before.source_profile)
            .and_then(|storage| storage.load().ok())
            .and_then(|rows| rows.into_iter().find(|row| row.id == before.id))
        {
            if let Some(row) = self.instances.get_mut(&before.id) {
                row.work_arc_enabled = disk.work_arc_enabled;
                row.work_arc = disk.work_arc;
            }
        }
        self.work_arc_refresh.checked = None;
    }

    pub(super) fn expand_work_arc(&mut self) {
        let Some(row) = self
            .selected_session
            .as_ref()
            .and_then(|id| self.get_instance(id))
        else {
            return;
        };
        let mut text = format!(
            "Work arc: {}\nDerived orientation, not independent verification.\nLast attempt: {}\n",
            arc::projection(row).label(),
            row.work_arc.cost.label()
        );
        if let Some(usage) = &row.work_arc.last_attempt_usage {
            text.push_str(&format!("Latest attempt usage: {usage}\nCosts are catalog estimates, not provider billing.\n"));
        }
        if let Some(record) = arc::visible_record(row) {
            text.push_str(&format!("\nGoal: {}\n\nNow: {}\n\nRemaining: {}\n\nNext: {}\n\nCovered: {}\nSelected route: {}\nActual: {}/{}\nUsage: {}\nLeaf: {}",
                record.arc.goal, record.arc.now, record.arc.remaining, record.arc.next,
                record.at.format("%Y-%m-%d %H:%M UTC"), record.selected_route,
                record.provider, record.model,
                record.usage.as_ref().map_or("unknown".into(), |v| v.to_string()), record.identity.leaf));
        }
        self.info_dialog = Some(
            InfoDialog::new("Work arc (Up/Down to scroll)", &text)
                .with_size(90, 30)
                .scrollable(),
        );
    }

    pub(crate) fn tick_work_arc(&mut self) -> bool {
        let invalidated = self.work_arc_refresh.target.as_ref().is_some_and(|before| {
            self.selected_session.as_deref() != Some(&before.id)
                || !self
                    .get_instance(&before.id)
                    .is_some_and(|row| unchanged(row, before))
        });
        if invalidated {
            self.invalidate_work_arc_selection();
        }
        let mut changed = invalidated;
        while let Ok(update) = self.work_arc_refresh.updates.try_recv() {
            if let Some(row) = self
                .instances
                .get_mut(&update.before.id)
                .filter(|row| unchanged(row, &update.before))
            {
                if let Some(state) = update.state {
                    row.work_arc = state;
                }
                row.work_arc_status = Some(
                    if self.selected_session.as_deref() != Some(&row.id)
                        && update.status == ArcStatus::Current
                    {
                        ArcStatus::Stale
                    } else {
                        update.status
                    },
                );
                changed = true;
            }
        }
        if self
            .work_arc_refresh
            .task
            .as_ref()
            .is_some_and(|task| !task.is_finished())
        {
            return changed;
        }
        self.work_arc_refresh.task = None;
        let Some(row) = self
            .selected_session
            .as_ref()
            .and_then(|id| self.get_instance(id))
        else {
            return changed;
        };
        if !row.work_arc_enabled || !arc::supported(row) {
            return changed;
        }
        if self
            .work_arc_refresh
            .checked
            .as_ref()
            .is_some_and(|(id, at)| *id == row.id && at.elapsed() < Duration::from_secs(5))
        {
            return changed;
        }
        let row = row.clone();
        if let Some(current) = self.instances.get_mut(&row.id) {
            current.work_arc_status = Some(if arc::visible_record(current).is_some() {
                ArcStatus::Stale
            } else {
                ArcStatus::Loading
            });
        }
        changed = true;
        self.work_arc_refresh.target = Some(row.clone());
        self.work_arc_refresh.checked = Some((row.id.clone(), Instant::now()));
        self.work_arc_refresh.task = Some(tokio::spawn(refresh(
            row,
            self.work_arc_refresh.sender.clone(),
        )));
        changed
    }
}
