use super::*;
use crate::session::afk::batch::{
    self,
    state::{Member, Operation, Outcome, Preference},
};
use std::collections::BTreeMap;
#[cfg(test)]
mod tests;

fn same_operation(snapshot: &authority::Snapshot, id: &str, digest: &str) -> Result<bool> {
    if let Some(op) = snapshot.batches.get(id) {
        ensure!(
            op.preview_digest == digest,
            "operation identity has another preview digest"
        );
        return Ok(true);
    }
    Ok(false)
}
fn view(app: &Path, snapshot: &authority::Snapshot, id: &str) -> Result<Value> {
    let op = snapshot
        .batches
        .get(id)
        .context("unknown batch operation in this profile")?;
    let mut negative = false;
    let members: Vec<_> = op.members.iter().map(|(id, member)| {
        let facts = member.window.as_ref().and_then(|window| snapshot.sessions.get(id)?.book.batch_facts(window));
        let pending = member.window.as_ref().and_then(|w| snapshot.usage.windows.get(w)).and_then(|w|w.source.as_ref()).and_then(|key|snapshot.usage.sources.get(key)).is_some_and(|source|usage::transport::negative_evidence(app,source).unwrap_or(true));
        let mut measured = member.window.as_ref().map(|w|snapshot.usage.window_view(w));
        if pending {negative = true; if let Some(value)=&mut measured {usage::pending_projection(value);}}
        json!({"session_id":id,"window":member.window,"outcome":member.outcome,"reason":member.reason,"host_confirmed":facts.as_ref().is_some_and(|f| f.confirmed),"ended":facts.as_ref().is_none_or(|f| f.ended),"reservations_used":facts.as_ref().map_or(0,|f|f.reservations.len()),"bulk_preference":snapshot.sessions.get(id).map(|s|s.preference),"usage":measured})
    }).collect();
    let mut total = usage::batch_view(snapshot, id);
    if negative {
        usage::pending_projection(&mut total);
    }
    let observed: usize = op
        .members
        .iter()
        .filter_map(|(id, m)| {
            snapshot
                .sessions
                .get(id)?
                .book
                .batch_facts(m.window.as_ref()?)
        })
        .map(|f| f.reservations.len())
        .sum();
    Ok(
        json!({"operation_id":id,"profile":snapshot.profile,"preview_digest":op.preview_digest,"issued_at_ms":op.issued_at_ms,"expires_at_ms":op.expires_at_ms,"stopped":op.stopped,"expired":now_ms()>=op.expires_at_ms,"shared_requests":op.shared_requests,"shared_budget":if op.shared_requests.is_some(){"Configured per-batch SDK request allowance; not delivery or billing"}else if snapshot.usage.batches.contains_key(id){"No shared request budget; usage guard is separate"}else{"No shared budget"},"charged":op.shared_requests.map(|_|op.tickets.len()),"held_unobserved":op.shared_requests.map(|_|op.tickets.iter().filter(|t|t.request.is_none()).count()),"recorded_sdk_reservations":observed,"usage":total,"members":members,"observation":"Stored host authority; acknowledgements and model delivery are not fresh observations"}),
    )
}
pub(in crate::session::afk) fn budget_view(
    snapshot: &authority::Snapshot,
    session: &str,
    window: &str,
) -> Value {
    snapshot.batches.values().find(|op| op.members.get(session).and_then(|m|m.window.as_deref())==Some(window)).map_or(Value::Null, |op| json!({"operation_id":op.id,"shared_requests":op.shared_requests,"charged":op.shared_requests.map(|_|op.tickets.len()),"held_unobserved":op.shared_requests.map(|_|op.tickets.iter().filter(|t|t.request.is_none()).count()),"stopped":op.stopped,"expires_at_ms":op.expires_at_ms}))
}
pub(in crate::session::afk) fn status(app: &Path, profile: &str, id: &str) -> Result<Value> {
    ensure!(
        uuid::Uuid::parse_str(id).is_ok(),
        "invalid operation identity"
    );
    authority::ready(app)?;
    let store = authority::profile_store(app, profile, false)?;
    let transaction = authority::SessionTransaction::profile(&store, profile)?;
    authority::validate_existing_snapshot(app, profile, Some(transaction.snapshot()))?;
    view(app, transaction.snapshot(), id)
}
pub(in crate::session::afk) fn apply(app: &Path, artifact: batch::Artifact) -> Result<Value> {
    apply_with(app, artifact, |store, binding, window, budgeted| {
        Ok(exchange_runtime(store, binding, window, budgeted)?.generation)
    })
}
fn apply_with(
    app: &Path,
    artifact: batch::Artifact,
    mut acknowledge: impl FnMut(&Store, &Binding, Option<&Window>, bool) -> Result<String>,
) -> Result<Value> {
    let preview = &artifact.content;
    authority::ready(app)?;
    let initial = batch::capture(app, &preview.profile)?;
    let initial_snapshot = initial
        .snapshot
        .as_ref()
        .context("no initialized profile authority")?;
    if same_operation(initial_snapshot, &preview.operation_id, &artifact.digest)? {
        return view(app, initial_snapshot, &preview.operation_id);
    }
    let mut candidates = preview.members.clone();
    for candidate in &mut candidates {
        if !candidate.exclusions.is_empty() {
            continue;
        }
        batch::recheck(app, candidate, &initial)?;
        if !candidate.exclusions.is_empty() {
            continue;
        }
        let observed = (|| -> Result<()> {
            let binding = candidate
                .host
                .as_ref()
                .and_then(|h| h.binding.as_ref())
                .context("missing candidate binding")?;
            let transport =
                Store::existing_transport(app, &preview.profile, &candidate.session_id)?
                    .context("missing candidate transport")?;
            let _lock = transport.lock(TIMEOUT)?;
            let generation = acknowledge(&transport, binding, None, false)?;
            ensure!(
                candidate
                    .observations
                    .first()
                    .is_some_and(|o| o.generation == generation),
                "runtime generation changed"
            );
            Ok(())
        })();
        if observed.is_err() {
            candidate.exclusions.push(batch::Exclusion::Unacknowledged);
        }
    }
    let store = authority::profile_store(app, &preview.profile, false)?;
    let mut created = Vec::new();
    {
        let mut transaction = authority::SessionTransaction::profile(&store, &preview.profile)?;
        authority::validate_existing_snapshot(app, &preview.profile, Some(transaction.snapshot()))?;
        if same_operation(
            transaction.snapshot(),
            &preview.operation_id,
            &artifact.digest,
        )? {
            return view(app, transaction.snapshot(), &preview.operation_id);
        }
        ensure!(
            transaction.snapshot().batches.len() < 64,
            "retained batch operation capacity exceeded; no operation was created"
        );
        let captured = batch::Capture {
            registry: registry::load(app, &preview.profile)?,
            snapshot: Some(transaction.snapshot().clone()),
        };
        let issued = now_ms();
        let expires = issued
            .checked_add(i64::from(preview.minutes) * 60_000)
            .context("batch expiry overflow")?;
        let mut proposal = transaction.snapshot().clone();
        let mut members = BTreeMap::new();
        for candidate in &mut candidates {
            if candidate.exclusions.is_empty() {
                batch::recheck(app, candidate, &captured)?;
            }
            let binding = candidate.host.as_ref().and_then(|h| h.binding.clone());
            let generation = candidate.observations.first().map(|o| o.generation.clone());
            let expected = candidate.authority.as_ref();
            let mut member = Member {
                binding: binding.clone(),
                generation: generation.clone(),
                window: None,
                grant_hash: candidate.grant_hash.clone(),
                control_epoch: expected.map_or(0, |a| a.control_epoch),
                exception_epoch: expected.map_or(0, |a| a.exception_epoch),
                outcome: Outcome::Skipped,
                reason: Some(format!("{:?}", candidate.exclusions)),
            };
            if candidate.exclusions.is_empty() {
                let row = captured
                    .registry
                    .get(&candidate.session_id)
                    .context("candidate disappeared")?;
                let session = proposal
                    .sessions
                    .get_mut(&candidate.session_id)
                    .context("candidate authority disappeared")?;
                ensure!(
                    session.preference != Preference::Excluded
                        && !session.book.inspection_flags().0
                        && !session.book.inspection_flags().1,
                    "candidate no longer eligible"
                );
                let mut window = pending_window(
                    row,
                    app,
                    candidate.grant.clone(),
                    binding.context("candidate binding missing")?,
                    generation.context("candidate generation missing")?,
                    issued,
                    expires,
                )?;
                if let Some(old) = session.book.windows.last() {
                    window.revision = old
                        .revision
                        .checked_add(1)
                        .context("window revision overflow")?;
                }
                if proposal
                    .usage
                    .enroll(
                        app,
                        &window.id,
                        &window.binding,
                        &window.generation,
                        candidate.usage_guard.clone(),
                        candidate.usage_guard.is_some() || preview.shared_usage_guard.is_some(),
                    )
                    .is_err()
                {
                    member.reason = Some(format!("{:?}", batch::Exclusion::UsageUnavailable));
                    members.insert(candidate.session_id.clone(), member);
                    continue;
                }
                session.book.prune(issued);
                session.epoch = session
                    .epoch
                    .checked_add(1)
                    .context("control epoch overflow")?;
                member.window = Some(window.id.clone());
                member.control_epoch = session.epoch;
                member.exception_epoch = session.exception_epoch;
                member.outcome = Outcome::Pending;
                member.reason = None;
                session.book.windows.push(window.clone());
                created.push((candidate.session_id.clone(), window));
            }
            members.insert(candidate.session_id.clone(), member);
        }
        proposal.batches.insert(
            preview.operation_id.clone(),
            Operation {
                id: preview.operation_id.clone(),
                preview_digest: artifact.digest.clone(),
                issued_at_ms: issued,
                expires_at_ms: expires,
                shared_requests: preview.shared_requests,
                stopped: false,
                members,
                tickets: vec![],
            },
        );
        if let Some(policy) = &preview.shared_usage_guard {
            proposal.usage.batches.insert(
                preview.operation_id.clone(),
                usage::SharedGuard::new(policy.clone()),
            );
        }
        transaction.publish(proposal)?.warn();
    }
    for (session_id, window) in created {
        let confirm = (|| -> Result<()> {
            let transport = Store::existing_transport(app, &preview.profile, &session_id)?
                .context("transport disappeared")?;
            let _transport_lock = transport.lock(TIMEOUT)?;
            current_instance(&window.binding)?;
            ensure!(
                acknowledge(
                    &transport,
                    &window.binding,
                    Some(&window),
                    preview.shared_requests.is_some()
                )? == window.generation,
                "runtime generation changed during confirmation"
            );
            let mut transaction = authority::SessionTransaction::profile(&store, &preview.profile)?;
            current_instance(&window.binding)?;
            if let Some(denial) = usage::check(transaction.snapshot(), app, &window.id) {
                let reason = denial.reason;
                transaction.deny_usage(&window.id, denial)?.warn();
                return Err(usage::Denied(reason).into());
            }
            let mut proposal = transaction.snapshot().clone();
            let op = proposal
                .batches
                .get_mut(&preview.operation_id)
                .context("operation disappeared")?;
            ensure!(
                !op.stopped && now_ms() < op.expires_at_ms,
                "operation stopped or expired"
            );
            let member = op
                .members
                .get_mut(&session_id)
                .context("member disappeared")?;
            let session = proposal
                .sessions
                .get_mut(&session_id)
                .context("session disappeared")?;
            let latest = session
                .book
                .windows
                .last_mut()
                .context("window disappeared")?;
            ensure!(
                member.outcome == Outcome::Pending
                    && member.window.as_deref() == Some(window.id.as_str())
                    && session.epoch == member.control_epoch
                    && session.exception_epoch == member.exception_epoch
                    && session.preference != Preference::Excluded
                    && latest.id == window.id
                    && latest.binding == window.binding
                    && latest.generation == window.generation
                    && hash(serde_json::to_vec(&latest.grant)?) == member.grant_hash
                    && latest.state == "pending"
                    && now_ms() < latest.expires_at_ms,
                "batch confirmation changed or ended"
            );
            latest.confirmed = true;
            member.outcome = Outcome::Confirmed;
            transaction.publish(proposal)?.warn();
            Ok(())
        })();
        if let Err(error) = confirm {
            if error.is::<authority::journal::Parked>() {
                return Err(error);
            }
            let mut transaction = authority::SessionTransaction::profile(&store, &preview.profile)?;
            let mut proposal = transaction.snapshot().clone();
            let member = proposal
                .batches
                .get_mut(&preview.operation_id)
                .context("operation disappeared")?
                .members
                .get_mut(&session_id)
                .context("member disappeared")?;
            if member.outcome == Outcome::Pending {
                let session = proposal
                    .sessions
                    .get_mut(&session_id)
                    .context("session disappeared")?;
                if let Some(original) = session
                    .book
                    .windows
                    .iter_mut()
                    .find(|w| w.id == window.id && !w.confirmed)
                {
                    original.end(TerminalReason::Interrupted);
                }
                member.outcome = Outcome::Failed;
                member.reason = Some(
                    "Original attempt unacknowledged, changed or ended; no retry activation".into(),
                );
                transaction.publish(proposal)?.warn();
            }
        }
    }
    status(app, &preview.profile, &preview.operation_id)
}
pub(in crate::session::afk) fn off(app: &Path, profile: &str, id: &str) -> Result<Value> {
    ensure!(
        uuid::Uuid::parse_str(id).is_ok(),
        "invalid operation identity"
    );
    authority::ready(app)?;
    let store = authority::profile_store(app, profile, false)?;
    let mut transaction = authority::SessionTransaction::profile(&store, profile)?;
    authority::validate_existing_snapshot(app, profile, Some(transaction.snapshot()))?;
    let mut proposal = transaction.snapshot().clone();
    let mut op = proposal
        .batches
        .get(id)
        .context("unknown batch operation in this profile")?
        .clone();
    if op.stopped {
        return view(app, transaction.snapshot(), id);
    }
    for (session_id, member) in &mut op.members {
        let Some(window) = &member.window else {
            continue;
        };
        let session = proposal
            .sessions
            .get_mut(session_id)
            .context("batch session missing")?;
        if let Some(latest) = session
            .book
            .windows
            .last_mut()
            .filter(|w| w.id == *window && w.state != "ended")
        {
            latest.end(TerminalReason::Interrupted);
            session.epoch = session
                .epoch
                .checked_add(1)
                .context("control epoch overflow")?;
        }
        if matches!(member.outcome, Outcome::Pending | Outcome::Confirmed) {
            member.outcome = Outcome::Stopped;
            member.reason = Some("Operator stopped original batch window".into());
        }
    }
    op.stopped = true;
    proposal.batches.insert(id.into(), op);
    transaction.publish(proposal)?.warn();
    view(app, transaction.snapshot(), id)
}
