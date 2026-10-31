use super::*;
use crate::session::work_arc::{self as arc, ArcState, ArcStatus, SourceError};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(super) struct ArcRefresh {
    pub(super) task: Option<tokio::task::JoinHandle<()>>,
    invalidated: tokio_util::sync::CancellationToken,
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
            invalidated: tokio_util::sync::CancellationToken::new(),
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
        self.invalidated.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn consent_unchanged(row: &Instance, before: &Instance) -> bool {
    row.work_arc_enabled
        && row.work_arc.consent_version == arc::CONSENT_VERSION
        && row.work_arc.consent_version == before.work_arc.consent_version
        && row.work_arc.epoch == before.work_arc.epoch
        && row.agent_session_id == before.agent_session_id
}

fn unchanged(row: &Instance, before: &Instance) -> bool {
    consent_unchanged(row, before)
        && row.agent_session_binding == before.agent_session_binding
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

fn arc_storages(before: &Instance) -> Result<Vec<Storage>, SourceError> {
    let profiles = crate::session::list_profiles().map_err(|_| SourceError::Unavailable)?;
    let storages = profiles
        .into_iter()
        .map(|profile| {
            Storage::open(&profile, crate::file_watch::FileWatchService::noop())
                .map_err(|_| SourceError::Unavailable)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !storages
        .iter()
        .any(|store| store.profile() == before.source_profile)
    {
        return Err(SourceError::Unavailable);
    }
    Ok(storages)
}

fn unique_owner_in(
    before: &Instance,
    storages: &[Storage],
    cancel: &tokio_util::sync::CancellationToken,
) -> bool {
    let mut occurrences = 0;
    for storage in storages {
        let Ok(rows) = storage.load_work_arc_owners(before, cancel) else {
            return false;
        };
        for row in rows {
            if cancel.is_cancelled() {
                return false;
            }
            if row.id == before.id && storage.profile() == before.source_profile {
                occurrences += 1;
            } else if row.id == before.id
                || (row.agent_session_id.is_some()
                    && row.agent_session_id == before.agent_session_id)
            {
                return false;
            }
        }
    }
    occurrences == 1
}

fn locked_arc<R>(
    before: &Instance,
    cancel: &tokio_util::sync::CancellationToken,
    f: impl FnOnce(&Storage) -> Result<R, SourceError>,
) -> Result<R, SourceError> {
    if cancel.is_cancelled() {
        return Err(SourceError::Unavailable);
    }
    let storages = arc_storages(before)?;
    crate::session::with_storages_locked_cancellable(&storages, cancel, || {
        if !unique_owner_in(before, &storages, cancel) {
            return Err(SourceError::Unavailable);
        }
        let storage = storages
            .iter()
            .find(|store| store.profile() == before.source_profile)
            .ok_or(SourceError::Unavailable)?;
        f(storage)
    })
    .map_err(|_| SourceError::Unavailable)?
}

struct ArcReceipt {
    before: Instance,
    identity: arc::ArcIdentity,
    nonce: String,
    result: Result<arc::GeneratedArc, arc::ArcFailure>,
}

async fn request_refresh(
    mut before: Instance,
    sender: &mpsc::Sender<ArcUpdate>,
    invalidated: &tokio_util::sync::CancellationToken,
) -> Option<ArcReceipt> {
    let snapshot = before.clone();
    let prepared = arc::bounded_source_cancellable(
        move |cancel| {
            locked_arc(&snapshot, &cancel, |storage| {
                let row = storage
                    .load_work_arc_owners(&snapshot, &cancel)
                    .map_err(|_| SourceError::Unavailable)?
                    .into_iter()
                    .find(|row| row.id == snapshot.id && unchanged(row, &snapshot))
                    .ok_or(SourceError::Disabled)?;
                arc::read_arc_source_cancellable(&row, &cancel)
            })
        },
        Duration::from_secs(2),
        invalidated,
    )
    .await;
    let bound = match prepared {
        Ok(value) => value,
        Err(error) => {
            let _ = sender.send(ArcUpdate {
                before,
                state: None,
                status: source_status(error),
            });
            return None;
        }
    };
    if arc::check_arc_route_cancellable(&bound, invalidated)
        .await
        .is_err()
    {
        let _ = sender.send(ArcUpdate {
            before,
            state: None,
            status: ArcStatus::Unavailable,
        });
        return None;
    }
    let identity = bound.identity();
    let reserved_identity = identity.clone();
    let snapshot = before.clone();
    let reserved = arc::bounded_source_cancellable(
        move |cancel| {
            locked_arc(&snapshot, &cancel, |storage| {
                storage
                    .update_changed_cancellable_under_lock(&cancel, |rows, _| {
                        let Some(row) = rows
                            .iter_mut()
                            .find(|row| row.id == snapshot.id && unchanged(row, &snapshot))
                        else {
                            return Ok((None, false));
                        };
                        if !arc::read_arc_source_cancellable(row, &cancel)
                            .is_ok_and(|latest| latest.identity() == reserved_identity)
                        {
                            return Ok((None, false));
                        }
                        let original = row.work_arc.clone();
                        let nonce = row.work_arc.reserve(&reserved_identity, chrono::Utc::now());
                        let changed = row.work_arc != original;
                        Ok((Some((nonce, row.work_arc.clone())), changed))
                    })
                    .map_err(|_| SourceError::Failed)
            })
        },
        Duration::from_secs(2),
        invalidated,
    )
    .await;
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
            return None;
        }
        _ => {
            let _ = sender.send(ArcUpdate {
                before,
                state: None,
                status: ArcStatus::Unavailable,
            });
            return None;
        }
    };
    let _ = sender.send(ArcUpdate {
        before: before.clone(),
        state: Some(state.clone()),
        status: ArcStatus::Loading,
    });
    // Receipt projection is relative to this reservation, not a newer canonical reload.
    before.work_arc = state;
    #[cfg(test)]
    send_hook(&before).await;
    let snapshot = before.clone();
    let send_nonce = nonce.clone();
    let result = arc::request_arc_guarded(
        bound,
        Box::new(move |bound, stream, request, cancel| {
            locked_arc(&snapshot, cancel, |storage| {
                let row = storage
                    .load_work_arc_owners(&snapshot, cancel)
                    .map_err(|_| SourceError::Unavailable)?
                    .into_iter()
                    .find(|row| row.id == snapshot.id && unchanged(row, &snapshot))
                    .ok_or(SourceError::Disabled)?;
                if !row.work_arc.is_pending(&send_nonce) {
                    return Err(SourceError::Stale);
                }
                let latest = arc::read_arc_source_cancellable(&row, cancel)?;
                if &latest != bound {
                    return Err(SourceError::Stale);
                }
                bound.write_request(stream, request, cancel)
            })
        }),
        invalidated,
    )
    .await;
    Some(ArcReceipt {
        before,
        identity,
        nonce,
        result,
    })
}

async fn refresh(
    before: Instance,
    sender: mpsc::Sender<ArcUpdate>,
    invalidated: tokio_util::sync::CancellationToken,
) {
    let received = tokio::select! {
        biased;
        _ = invalidated.cancelled() => return,
        received = request_refresh(before, &sender, &invalidated) => received,
    };
    let Some(ArcReceipt {
        before,
        identity,
        nonce,
        result,
    }) = received
    else {
        return;
    };
    // UI invalidation stops source work and IPC, not bounded receipt accounting.
    let source_cancel = invalidated.child_token();
    let _source_guard = source_cancel.clone().drop_guard();
    let snapshot = before.clone();
    let finished = arc::bounded_source(
        move |cancel| {
            locked_arc(&snapshot, &cancel, |storage| {
                storage
                    .update_changed_cancellable_under_lock(&cancel, |rows, _| {
                        let Some(row) = rows
                            .iter_mut()
                            .find(|row| row.id == snapshot.id && consent_unchanged(row, &snapshot))
                        else {
                            return Ok((None, false));
                        };
                        if !row.work_arc.is_pending(&nonce) {
                            return Ok((None, false));
                        }
                        let fresh = !invalidated.is_cancelled()
                            && unchanged(row, &snapshot)
                            && row.work_arc.revision == snapshot.work_arc.revision
                            && identity.matches(row);
                        let route_refused =
                            result.as_ref().is_err_and(|failure| failure.route_refused);
                        let (status, record, usage) = match result {
                            Ok(generated) if !fresh => (ArcStatus::Stale, None, generated.usage),
                            Ok(generated) => {
                                match arc::read_arc_source_cancellable(row, &source_cancel) {
                                    Ok(latest)
                                        if !invalidated.is_cancelled()
                                            && latest.identity() == identity =>
                                    {
                                        (
                                            ArcStatus::Current,
                                            Some(arc::StoredArc {
                                                identity: generated.bound.identity(),
                                                at: chrono::Utc::now(),
                                                arc: generated.arc,
                                                selected_route: generated.selected_route,
                                                provider: generated.provider,
                                                model: generated.model,
                                                usage: generated.usage.clone(),
                                            }),
                                            generated.usage,
                                        )
                                    }
                                    Ok(_) => (ArcStatus::Stale, None, generated.usage),
                                    Err(error) => (source_status(error), None, generated.usage),
                                }
                            }
                            Err(failure) if !fresh => (ArcStatus::Stale, None, failure.usage),
                            Err(failure) => (source_status(failure.error), None, failure.usage),
                        };
                        if !row.work_arc.finish(&nonce, record, usage) {
                            return Ok((None, false));
                        }
                        if status == ArcStatus::Unavailable {
                            if route_refused && row.work_arc.last_attempt_usage.is_none() {
                                row.work_arc.cost = arc::ArcCost::NoRequest;
                            }
                            row.work_arc.unavailable = true;
                        }
                        Ok((Some((row.work_arc.clone(), status)), true))
                    })
                    .map_err(|_| SourceError::Failed)
            })
        },
        Duration::from_secs(2),
    )
    .await;
    let (state, status) = match finished {
        Ok(Some((state, status))) => (Some(state), status),
        Err(error) => (None, source_status(error)),
        _ => (None, ArcStatus::Failed),
    };
    let _ = sender.send(ArcUpdate {
        before,
        state,
        status,
    });
}

#[cfg(test)]
type SendHook = (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);
#[cfg(test)]
pub(super) static SEND_HOOK: std::sync::Mutex<Option<SendHook>> = std::sync::Mutex::new(None);
#[cfg(test)]
async fn send_hook(row: &Instance) {
    let hook = {
        let mut slot = SEND_HOOK.lock().unwrap();
        if slot.as_ref().is_some_and(|(id, _, _)| id == &row.id) {
            slot.take()
        } else {
            None
        }
    };
    if let Some((_, ready, release)) = hook {
        ready.send(()).unwrap();
        release.await.unwrap();
    }
}

impl HomeView {
    pub(super) fn invalidate_work_arc_selection(&mut self) {
        self.work_arc_refresh.invalidated.cancel();
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
                row.work_arc.invalidate();
                row.work_arc_status = None;
            }) {
                self.info_dialog = Some(InfoDialog::new(
                    "Work arc",
                    &format!("Could not pause: {error}"),
                ));
            }
        } else {
            self.work_arc_refresh.consent = Some(before);
            self.confirm_dialog = Some(ConfirmDialog::new(
                "Enable work arc for this Session?",
                "For this AoE Session, including successor Pi conversations, AoE will read bounded current user/assistant text and send it through the guarded Pi process's selected model route. Provider charges may apply; a virtual route may use another physical model. Local documents/history retain derived fields, provenance and spend limits. No prior arc values, raw history or field timestamps are sent; no remote upload. Refresh runs while the TUI is open, at most 1 attempt/5 minutes and 20/day per Session. Pause retains documents/history/budgets. Carried-forward text is not fresh evidence. Enable this Session?",
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
            row.work_arc.consent_version = arc::CONSENT_VERSION;
            row.work_arc.invalidate();
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
            let time = |at: Option<chrono::DateTime<chrono::Utc>>| {
                at.map_or("unknown".into(), |at| {
                    at.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S %:z")
                        .to_string()
                })
            };
            for (index, (label, value)) in [
                ("Goal", &record.arc.goal),
                ("Now", &record.arc.now),
                ("Remaining", &record.arc.remaining),
                ("Next", &record.arc.next),
            ]
            .into_iter()
            .enumerate()
            {
                text.push_str(&format!("\n{label}: {value}\n"));
                if let Some(document) = &row.work_arc.document {
                    let field = &document.fields[index];
                    let carried =
                        if row.agent_session_id.as_deref() != Some(field.source.pi_id.as_str()) {
                            " (carried forward)"
                        } else {
                            ""
                        };
                    text.push_str(&format!(
                        "Observed: {} | Changed: {}\nSource: {} / {}{carried}\n",
                        time(field.observed_at),
                        time(field.changed_at),
                        field.source.pi_id,
                        field.source.leaf
                    ));
                }
            }
            text.push_str(&format!(
                "\nCovered: {}\nSelected route: {}\nActual: {}/{}\nUsage: {}\nLeaf: {}",
                time(Some(record.at)),
                record.selected_route,
                record.provider,
                record.model,
                record
                    .usage
                    .as_ref()
                    .map_or("unknown".into(), |v| v.to_string()),
                record.identity.leaf
            ));
            if let Some(storage) = self.storages.get(&row.source_profile) {
                match storage.work_arc_history(row, 8) {
                    Ok(history) => {
                        text.push_str("\n\nRecent history (latest first, at most 8; older revisions retained):");
                        for document in history {
                            let record = document.record;
                            text.push_str(&format!(
                                "\n{} | {} / {}\nGoal: {} | Now: {}\nRemaining: {} | Next: {}",
                                time(Some(record.at)),
                                record.identity.pi_id,
                                record.identity.leaf,
                                record.arc.goal,
                                record.arc.now,
                                record.arc.remaining,
                                record.arc.next
                            ));
                        }
                    }
                    Err(error) => text.push_str(&format!("\nHistory unavailable: {error}")),
                }
            }
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
            if let Some(row) = self.instances.get_mut(&update.before.id).filter(|row| {
                unchanged(row, &update.before)
                    && (row.work_arc == update.before.work_arc
                        || update
                            .state
                            .as_ref()
                            .is_some_and(|state| state == &row.work_arc))
            }) {
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
        if !row.work_arc_enabled
            || row.work_arc.consent_version != arc::CONSENT_VERSION
            || !arc::supported(row)
        {
            return changed;
        }
        if self
            .work_arc_refresh
            .checked
            .as_ref()
            .is_some_and(|(id, at)| id == &row.id && at.elapsed() < Duration::from_secs(5))
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
        self.work_arc_refresh.invalidated = tokio_util::sync::CancellationToken::new();
        self.work_arc_refresh.task = Some(tokio::spawn(refresh(
            row,
            self.work_arc_refresh.sender.clone(),
            self.work_arc_refresh.invalidated.clone(),
        )));
        changed
    }
}
