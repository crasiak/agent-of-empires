//! Read-only host summaries. Stored acknowledgements never establish current authority.
use super::*;
use crate::session::AnchoredDir;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

const MAX_PROFILES: usize = 128;
const MAX_DIRECTORY_ENTRIES: usize = 256;
const MAX_SESSIONS: usize = 4096;
const PROFILE_BYTES: usize = 8 * 1024 * 1024;
const SCAN_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct Overview {
    pub captured_at_ms: i64,
    pub profiles: Vec<ProfileOverview>,
    pub shared_budget: &'static str,
    pub batch_membership: &'static str,
    pub bulk_exclusions: &'static str,
    pub account_coverage: &'static str,
    pub observation: &'static str,
}
impl Overview {
    pub fn complete(&self) -> bool {
        self.profiles
            .iter()
            .all(|p| p.errors.is_empty() && p.sessions.iter().all(|s| s.errors.is_empty()))
    }
}

#[derive(Debug, Serialize)]
pub struct ProfileOverview {
    pub profile: String,
    pub sessions: Vec<SessionOverview>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionOverview {
    pub id: String,
    pub title: String,
    pub tool: String,
    pub activity: String,
    pub archived: bool,
    pub trashed: bool,
    pub support: String,
    pub requested_presence: String,
    pub presence_expires_at_ms: Option<i64>,
    pub presence_acknowledgement: String,
    pub delegation: Option<DelegationSummary>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DelegationSummary {
    pub window: String,
    pub state: String,
    pub host_confirmed: bool,
    pub expires_at_ms: i64,
    pub reservations_used: usize,
    pub requests: u8,
    pub nudges_used: usize,
    pub question_deferrals_used: usize,
    pub unresolved_question_guards: usize,
    pub questions_requiring_review: usize,
    pub latest_question_outcome: Option<String>,
    pub question_answer_observation: &'static str,
    pub human_required_records: usize,
    pub action_outcomes: Vec<String>,
    pub runtime_acknowledgement: String,
    pub coverage: &'static str,
}

pub fn snapshot() -> Result<Overview> {
    snapshot_at(&super::super::get_app_dir_path()?, now_ms())
}

fn escaped(value: &str) -> String {
    value.chars().flat_map(char::escape_debug).collect()
}

fn shown(value: &str) -> String {
    let mut text = escaped(&value.chars().take(160).collect::<String>());
    if value.chars().count() > 160 {
        text.push_str("...");
    }
    text
}

fn snapshot_at(app_path: &Path, now: i64) -> Result<Overview> {
    let mut result = Overview {
        captured_at_ms: now,
        profiles: Vec::new(),
        shared_budget: "No shared budget; batch budget controls are not implemented",
        batch_membership: "Outside batches; only individual windows are implemented",
        bulk_exclusions:
            "Not recorded by this storage version; Off is not a persistent bulk opt-out",
        account_coverage: "unknown; not an account-wide or profile-wide budget",
        observation:
            "Stored host summaries only; activity and acknowledgements are not live observations",
    };
    match std::fs::symlink_metadata(app_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        other => {
            other?;
        }
    }
    let app = AnchoredDir::open(app_path)?;
    if app.entry_stat(Path::new("profiles"))?.is_none() {
        return Ok(result);
    }
    let profiles = app.child(Path::new("profiles"))?;
    let mut names = Vec::new();
    for (visited, entry) in std::fs::read_dir(profiles.path())?.enumerate() {
        ensure!(
            visited < MAX_DIRECTORY_ENTRIES,
            "overview directory entry capacity exceeded (256); no complete fleet snapshot available"
        );
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() || kind.is_symlink() {
            ensure!(
                names.len() < MAX_PROFILES,
                "overview profile capacity exceeded (128); no complete fleet snapshot available"
            );
            names.push(entry.file_name().into_string().map_err(|_| {
                anyhow::anyhow!("non-UTF8 profile name; fleet cannot be enumerated")
            })?);
        }
    }
    names.sort();
    let mut remaining_bytes = SCAN_BYTES;
    let mut remaining_sessions = MAX_SESSIONS;
    for name in names {
        let mut profile = ProfileOverview {
            profile: escaped(&name),
            sessions: vec![],
            errors: vec![],
        };
        let rows = (|| -> Result<Vec<Value>> {
            let dir = profiles.child(Path::new(&name))?;
            if dir.entry_stat(Path::new("sessions.json"))?.is_none() {
                return Ok(Vec::new());
            }
            let file = dir
                .open_regular(Path::new("sessions.json"), PROFILE_BYTES)?
                .context("unsafe or oversized sessions.json")?;
            let size = usize::try_from(file.metadata()?.len())?;
            ensure!(
                size <= remaining_bytes,
                "overview read capacity exceeded (64 MiB)"
            );
            remaining_bytes -= size;
            let mut bytes = Vec::new();
            file.take((size + 1) as u64).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= size, "sessions.json changed during read");
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return Ok(vec![]);
            }
            serde_json::from_slice(&bytes).context("invalid sessions.json")
        })();
        match rows {
            Err(error) => profile.errors.push(shown(&error.to_string())),
            Ok(rows) if rows.len() > remaining_sessions => profile.errors.push(
                "overview session capacity exceeded (4096); this profile was not scanned".into(),
            ),
            Ok(rows) => {
                remaining_sessions -= rows.len();
                let mut ids = std::collections::HashSet::new();
                for (index, row) in rows.into_iter().enumerate() {
                    let Ok(mut instance) = serde_json::from_value::<Instance>(row) else {
                        profile.errors.push(format!(
                            "Invalid session row {index}; not omitted as an empty profile"
                        ));
                        continue;
                    };
                    instance.source_profile = name.clone();
                    let mut summary =
                        session_summary(app_path, &instance, now, &mut remaining_bytes);
                    if !ids.insert(instance.id.clone()) {
                        summary
                            .errors
                            .push("Duplicate session ID in profile".into());
                    }
                    profile.sessions.push(summary);
                }
            }
        }
        result.profiles.push(profile);
    }
    Ok(result)
}

fn session_summary(
    app: &Path,
    instance: &Instance,
    now: i64,
    remaining: &mut usize,
) -> SessionOverview {
    let binding = Binding::for_instance(instance);
    let mut row = SessionOverview {
        id: shown(&instance.id),
        title: shown(&instance.title),
        tool: shown(&instance.tool),
        activity: instance.status.as_str().into(),
        archived: instance.is_archived(),
        trashed: instance.is_trashed(),
        support: binding.as_ref().map_or_else(
            |e| shown(&e.to_string()),
            |_| "eligible host binding; runtime support unobserved".into(),
        ),
        requested_presence: "off (no stored request)".into(),
        presence_expires_at_ms: None,
        presence_acknowledgement: "unobserved".into(),
        delegation: None,
        errors: vec![],
    };
    let presence = (|| -> Result<()> {
        let Some(store) = Store::existing(app, &instance.id, false)? else {
            return Ok(());
        };
        let policy = store.read_budgeted::<Policy>("policy.json", remaining)?;
        if let Some(p) = &policy {
            p.validate().context("invalid policy summary")?;
            ensure!(
                p.binding.instance_id == instance.id
                    && p.binding.profile == instance.source_profile,
                "policy belongs to a different session/profile"
            );
            row.requested_presence = if p.enabled {
                "on (control-only)"
            } else {
                "off"
            }
            .into();
            row.presence_expires_at_ms = p.expires_at_ms;
            if p.enabled && p.expires_at_ms.is_some_and(|at| at <= now) {
                row.requested_presence.push_str("; expired");
            }
        }
        let ack = store.read_budgeted::<Ack>("ack.json", remaining)?;
        let probe = store.read_budgeted::<Probe>("request.json", remaining)?;
        if let Some(ack) = ack {
            row.presence_acknowledgement = if probe.as_ref().is_some_and(|probe| {
                binding.as_ref().is_ok_and(|b| *b == probe.binding)
                    && valid_ack(&ack, probe, policy.as_ref(), now)
            }) {
                "stored matching acknowledgement; freshness unknown"
            } else {
                "stale or unmatched stored acknowledgement"
            }
            .into();
        }
        Ok(())
    })();
    if let Err(error) = presence {
        row.errors
            .push(format!("Presence: {}", shown(&error.to_string())));
        row.requested_presence = "unknown; storage error".into();
    }
    let delegated = (|| -> Result<Option<DelegationSummary>> {
        let Some(store) = Store::existing(app, &instance.id, true)? else {
            return Ok(None);
        };
        let Some(book) = store.read_budgeted::<SummaryBook>("ledger.json", remaining)? else {
            return Ok(None);
        };
        ensure!(book.version == 4, "unsupported ledger version");
        for w in &book.windows {
            w.validate(instance)?;
        }
        let Some(w) = book.windows.last() else {
            return Ok(None);
        };
        let current = binding.as_ref().is_ok_and(|b| *b == w.binding);
        let mut acknowledgement = "unobserved";
        if let Some(control) = Store::existing(app, &instance.id, false)? {
            if let Some(ack) = control.read_budgeted::<SummaryAck>("runtime-ack.json", remaining)? {
                acknowledgement = if ack.version == 4
                    && current
                    && ack.binding == w.binding
                    && ack.generation == w.generation
                    && ack.window.as_deref() == Some(&w.id)
                {
                    "stored identity match only; freshness and grant acknowledgement unverified"
                } else {
                    "stale or unmatched stored acknowledgement"
                };
            }
        }
        Ok(Some(DelegationSummary {
            window: w.id.clone(),
            state: if now >= w.expires_at_ms { "expired" } else if w.state == "ended" { "ended" } else if !current { "invalidated" } else { &w.state }.into(),
            host_confirmed: w.confirmed, expires_at_ms: w.expires_at_ms,
            reservations_used: w.reservations.len(), requests: w.grant.requests,
            nudges_used: w.nudges.len(), question_deferrals_used: usize::from(w.question.is_some()),
            unresolved_question_guards: book.windows.iter().filter(|w| w.question.as_ref().is_some_and(|q| q.outcome.as_deref() != Some("ordinary") && q.observed_request.is_none() && q.recovered_at_ms.is_none())).count(),
            questions_requiring_review: book.windows.iter().filter(|w| w.question.is_some()).count(),
            latest_question_outcome: w.question.as_ref().and_then(|q| q.outcome.clone()),
            question_answer_observation: "AFK does not record human answers; observed return or recovery does not resolve human review",
            human_required_records: w.records.iter().filter(|r| r.disposition == "human_required").count(),
            action_outcomes: w.permit.as_ref().map_or_else(Vec::new, |p| p.outcomes.clone()),
            runtime_acknowledgement: acknowledgement.into(),
            coverage: "SDK main-loop reservations only; provider delivery, background requests and billing unknown",
        }))
    })();
    match delegated {
        Ok(summary) => row.delegation = summary,
        Err(error) => row
            .errors
            .push(format!("Delegation: {}", shown(&error.to_string()))),
    }
    row
}

// Project only host fields; decision bodies, grants' task text and file payloads are discarded.
#[derive(Deserialize)]
struct SummaryBook {
    version: u32,
    windows: Vec<SummaryWindow>,
}
#[derive(Deserialize)]
struct SummaryWindow {
    version: u32,
    binding: Binding,
    generation: String,
    id: String,
    state: String,
    confirmed: bool,
    expires_at_ms: i64,
    reservations: Vec<String>,
    grant: SummaryGrant,
    nudges: Vec<serde::de::IgnoredAny>,
    question: Option<SummaryQuestion>,
    records: Vec<SummaryRecord>,
    permit: Option<SummaryPermit>,
}
#[derive(Deserialize)]
struct SummaryGrant {
    version: u32,
    requests: u8,
}
#[derive(Deserialize)]
struct SummaryQuestion {
    outcome: Option<String>,
    observed_request: Option<String>,
    recovered_at_ms: Option<i64>,
}
#[derive(Deserialize)]
struct SummaryRecord {
    disposition: String,
}
#[derive(Deserialize)]
struct SummaryPermit {
    outcomes: Vec<String>,
}
#[derive(Deserialize)]
struct SummaryAck {
    version: u32,
    binding: Binding,
    generation: String,
    window: Option<String>,
}
impl SummaryWindow {
    fn validate(&self, instance: &Instance) -> Result<()> {
        ensure!(
            self.version == 4 && self.grant.version == 4,
            "unsupported window/grant version"
        );
        ensure!(
            self.binding.instance_id == instance.id
                && self.binding.profile == instance.source_profile,
            "ledger belongs to a different session/profile"
        );
        ensure!(
            uuid::Uuid::parse_str(&self.id).is_ok()
                && uuid::Uuid::parse_str(&self.generation).is_ok(),
            "invalid ledger identity"
        );
        ensure!(
            (1..=8).contains(&self.grant.requests)
                && self.reservations.len() <= usize::from(self.grant.requests)
                && self.nudges.len() <= 2
                && self.records.len() <= 4,
            "invalid summary counters"
        );
        ensure!(
            matches!(self.state.as_str(), "pending" | "owned" | "ended"),
            "invalid ledger state"
        );
        if let Some(question) = &self.question {
            ensure!(
                question
                    .outcome
                    .as_deref()
                    .is_none_or(|outcome| matches!(outcome, "accepted" | "ordinary" | "unknown")),
                "invalid question outcome"
            );
        }
        if let Some(permit) = &self.permit {
            ensure!(
                permit.outcomes.is_empty()
                    || permit.outcomes == ["unknown"]
                    || permit.outcomes == ["unknown", "observed_applied"],
                "invalid action outcomes"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
