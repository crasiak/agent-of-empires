//! Read-only host summaries. Stored acknowledgements never establish current authority.
use super::*;
use crate::session::AnchoredDir;
use serde_json::{json, Value};
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
    pub migration_evidence: Vec<Value>,
    pub historical_authority: Vec<HistoricalAuthority>,
}
impl Overview {
    pub fn complete(&self) -> bool {
        self.profiles
            .iter()
            .all(|p| p.errors.is_empty() && p.sessions.iter().all(|s| s.errors.is_empty()))
    }
}

#[derive(Debug, Serialize)]
pub struct HistoricalAuthority {
    pub profile: String,
    pub instance_id: String,
    pub observation: &'static str,
    pub evidence: Vec<Value>,
    pub windows: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct ProfileOverview {
    pub profile: String,
    pub sessions: Vec<SessionOverview>,
    pub batches: Vec<Value>,
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
    pub legacy_evidence: Vec<Value>,
    pub bulk_preference: String,
    pub exception_epoch: Option<u64>,
    pub batch: Option<Value>,
    pub batch_membership: &'static str,
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

pub(super) fn snapshot_at(app_path: &Path, now: i64) -> Result<Overview> {
    let mut result = Overview {
        captured_at_ms: now,
        migration_evidence: vec![],
        historical_authority: vec![],
        profiles: Vec::new(),
        shared_budget: "No shared budget",
        batch_membership: "Exact original-window membership; other windows remain outside batches",
        bulk_exclusions:
            "Per-session unrecorded/excluded/allowed preference; allow-bulk grants no authority",
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
    let mut remaining_bytes = SCAN_BYTES;
    let metadata = authority::migration_v2::overview_metadata(app_path, &mut remaining_bytes);
    let readiness = metadata.as_ref().map(|_| ());
    if let Ok(metadata) = &metadata {
        result.migration_evidence = metadata
            .evidence
            .iter()
            .filter(|e| e.profiles.is_empty())
            .map(serde_json::to_value)
            .collect::<std::result::Result<_, _>>()?;
        result
            .migration_evidence
            .extend(metadata.absent_profiles.iter().map(|profile| {
                serde_json::json!({
                    "kind":"prepared_profile_absence", "profile":profile,
                    "original_absent":true, "authority_imported":false,
                    "observation":"No v1 snapshot existed; no session or v2 import was fabricated"
                })
            }));
    }
    let profiles = if app.entry_stat(Path::new("profiles"))?.is_some() {
        Some(app.child(Path::new("profiles"))?)
    } else {
        None
    };
    let mut names = std::collections::BTreeSet::new();
    let authority_profiles = (|| -> Result<Option<AnchoredDir>> {
        if app.entry_stat(Path::new(authority::NAMESPACE))?.is_none() {
            return Ok(None);
        }
        let root = app.child(Path::new(authority::NAMESPACE))?;
        if root.entry_stat(Path::new("profiles"))?.is_none() {
            return Ok(None);
        }
        Ok(Some(root.child(Path::new("profiles"))?))
    })();
    for directory in [
        profiles.as_ref(),
        authority_profiles.as_ref().ok().and_then(Option::as_ref),
    ]
    .into_iter()
    .flatten()
    {
        for (visited, entry) in std::fs::read_dir(directory.path())?.enumerate() {
            ensure!(
                visited < MAX_DIRECTORY_ENTRIES,
                "overview directory entry capacity exceeded (256)"
            );
            let entry = entry?;
            if entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
                names.insert(
                    entry
                        .file_name()
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("non-UTF8 profile name"))?,
                );
                ensure!(
                    names.len() <= MAX_PROFILES,
                    "overview profile capacity exceeded (128)"
                );
            }
        }
    }
    if let Ok(metadata) = &metadata {
        names.extend(metadata.profiles.iter().cloned());
        ensure!(
            names.len() <= MAX_PROFILES,
            "overview profile capacity exceeded (128)"
        );
    }
    if names.is_empty() {
        authority_profiles.context("authority discovery unavailable; no empty fleet inferred")?;
        if let Err(error) = &readiness {
            anyhow::bail!(
                "AFK migration metadata unavailable; no empty fleet inferred: {}; run normal `aoe migrate` in this app namespace",
                error.root_cause()
            );
        }
    }
    let mut remaining_sessions = MAX_SESSIONS;
    for name in names {
        let mut profile = ProfileOverview {
            profile: escaped(&name),
            sessions: vec![],
            batches: vec![],
            errors: vec![],
        };
        let authority = (|| -> Result<SummaryProfile> {
            if let Err(error) = &readiness {
                anyhow::bail!(
                    "{}; run normal `aoe migrate` in this app namespace",
                    error.root_cause()
                );
            }
            let store = authority::profile_store(app_path, &name, false)?;
            authority::unparked(&store)?;
            let snapshot: SummaryProfile = store
                .read_budgeted("snapshot.json", &mut remaining_bytes)?
                .context("missing profile authority")?;
            authority::unparked(&store)?;
            ensure!(
                snapshot.version == 2 && snapshot.profile == name && snapshot.revision > 0,
                "invalid profile authority identity"
            );
            if let Ok(metadata) = &metadata {
                if let Some(expected) = metadata.imports.get(&name) {
                    ensure!(
                        snapshot.origin == expected.origin
                            && Some(snapshot.revision) >= expected.revision,
                        "missing or changed imported authority provenance"
                    );
                    for id in &expected.sessions {
                        ensure!(
                            snapshot.sessions.contains_key(id),
                            "missing imported session authority; repair required"
                        );
                    }
                }
                for expected in metadata
                    .evidence
                    .iter()
                    .filter(|e| e.profiles.contains(&name))
                {
                    let session = snapshot
                        .sessions
                        .get(&expected.instance)
                        .context("missing imported session authority; repair required")?;
                    ensure!(
                        session.evidence.contains(expected),
                        "missing imported evidence reference; repair required"
                    );
                }
            }
            Ok(snapshot)
        })();
        if let Err(error) = &authority {
            profile
                .errors
                .push(format!("Authority: {}", shown(&error.to_string())));
        }
        let mut membership_available = false;
        if let Ok(snapshot) = &authority {
            match snapshot.batch_summaries(now) {
                Ok(summaries) => {
                    membership_available = true;
                    if snapshot
                        .batches
                        .values()
                        .any(|b| b.shared_requests.is_some())
                    {
                        result.shared_budget =
                            "Recorded per-batch shared allowances; not a global or account budget";
                    }
                    profile.batches = summaries;
                }
                Err(error) => profile.errors.push(format!(
                    "Batch membership unavailable: {}",
                    shown(&error.to_string())
                )),
            }
        }
        let rows = (|| -> Result<Vec<Value>> {
            let Some(profiles) = &profiles else {
                return Ok(vec![]);
            };
            if profiles.entry_stat(Path::new(&name))?.is_none() {
                return Ok(vec![]);
            }
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
        let mut ids = std::collections::HashSet::new();
        match rows {
            Err(error) => profile.errors.push(shown(&error.to_string())),
            Ok(rows) if rows.len() > remaining_sessions => profile.errors.push(
                "overview session capacity exceeded (4096); this profile was not scanned".into(),
            ),
            Ok(rows) => {
                remaining_sessions -= rows.len();
                for (index, row) in rows.into_iter().enumerate() {
                    let Ok(mut instance) = serde_json::from_value::<Instance>(row) else {
                        profile.errors.push(format!(
                            "Invalid session row {index}; not omitted as an empty profile"
                        ));
                        continue;
                    };
                    instance.source_profile = name.clone();
                    let mut summary = session_summary(
                        app_path,
                        &instance,
                        authority
                            .as_ref()
                            .ok()
                            .and_then(|s| s.sessions.get(&instance.id)),
                        now,
                        &mut remaining_bytes,
                    );
                    if let Some(window) = summary.delegation.as_ref().map(|w| &w.window) {
                        summary.batch = profile
                            .batches
                            .iter()
                            .find(|b| {
                                b["members"].as_array().is_some_and(|members| {
                                    members.iter().any(|m| {
                                        m["session_id"] == instance.id
                                            && m["window"].as_str() == Some(window)
                                    })
                                })
                            })
                            .cloned();
                    }
                    summary.batch_membership = if !membership_available {
                        "unavailable; profile membership not verified"
                    } else if summary.batch.is_some() {
                        "stored original-window membership; not a fresh observation"
                    } else {
                        "outside original batch windows"
                    };
                    if !ids.insert(instance.id.clone()) {
                        summary
                            .errors
                            .push("Duplicate session ID in profile".into());
                    }
                    profile.sessions.push(summary);
                }
            }
        }
        if let Ok(authority) = &authority {
            for (id, session) in &authority.sessions {
                if ids.contains(id) {
                    continue;
                }
                if remaining_sessions == 0 {
                    profile
                        .errors
                        .push("overview historical session capacity exceeded (4096)".into());
                    break;
                }
                remaining_sessions -= 1;
                if session.book.version != 4
                    || session
                        .book
                        .windows
                        .iter()
                        .any(|w| w.validate(&name, id).is_err())
                {
                    profile
                        .errors
                        .push("Invalid historical authority summary".into());
                    continue;
                }
                let evidence = session
                    .evidence
                    .iter()
                    .map(|e| {
                        e.validate(&name, id)?;
                        Ok(serde_json::to_value(e)?)
                    })
                    .collect::<Result<Vec<_>>>();
                match evidence {
                    Err(error) => profile.errors.push(format!("Historical authority: {}", shown(&error.to_string()))),
                    Ok(evidence) => result.historical_authority.push(HistoricalAuthority {
                        profile: escaped(&name), instance_id: shown(id),
                        observation: "Historical authority only; no validated registry row, no live eligibility inferred",
                        evidence,
                        windows: session.book.windows.iter().map(|w| serde_json::json!({
                            "window":w.id,"generation":w.generation,"state":w.state,
                            "unresolved_question_guard": w.question.as_ref().is_some_and(|q| q.outcome.as_deref() != Some("ordinary") && q.observed_request.is_none() && q.recovered_at_ms.is_none()),
                            "question_requires_review":w.question.is_some(),
                            "reservations_used":w.reservations.len(),"host_confirmed":w.confirmed
                        })).collect(),
                    }),
                }
            }
        }
        result.profiles.push(profile);
    }
    if !result.complete() && result.shared_budget == "No shared budget" {
        result.shared_budget = "Shared budgets unobserved; incomplete snapshot";
    }
    Ok(result)
}

fn session_summary(
    app: &Path,
    instance: &Instance,
    authority: Option<&SummarySession>,
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
        legacy_evidence: authority.map_or_else(Vec::new, |s| {
            s.evidence
                .iter()
                .filter(|e| e.validate(&instance.source_profile, &instance.id).is_ok())
                .map(|e| serde_json::to_value(e).expect("evidence serializes"))
                .collect()
        }),
        bulk_preference: authority
            .map_or("unknown", |s| match s.preference {
                batch::state::Preference::Unrecorded => "unrecorded",
                batch::state::Preference::Excluded => "excluded",
                batch::state::Preference::Allowed => "allowed",
            })
            .into(),
        exception_epoch: authority.map(|s| s.exception_epoch),
        batch: None,
        batch_membership: "unavailable",
        errors: vec![],
    };
    let presence = (|| -> Result<()> {
        let session =
            authority.context("missing/unavailable session authority; no authority inferred")?;
        for evidence in &session.evidence {
            evidence.validate(&instance.source_profile, &instance.id)?;
        }
        let policy = session.policy.as_ref();
        if let Some(p) = policy {
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
        let Some(store) = Store::existing_transport(app, &instance.source_profile, &instance.id)?
        else {
            return Ok(());
        };
        let ack = store.read_budgeted::<Ack>("ack.json", remaining)?;
        let probe = store.read_budgeted::<Probe>("request.json", remaining)?;
        if let Some(ack) = ack {
            row.presence_acknowledgement = if probe.as_ref().is_some_and(|probe| {
                binding.as_ref().is_ok_and(|b| *b == probe.binding)
                    && valid_ack(&ack, probe, policy, now)
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
        let session = authority.context("missing/unavailable session authority")?;
        let book = &session.book;
        ensure!(book.version == 4, "unsupported ledger version");
        for w in &book.windows {
            w.validate(&instance.source_profile, &instance.id)?;
        }
        let Some(w) = book.windows.last() else {
            return Ok(None);
        };
        let current = binding.as_ref().is_ok_and(|b| *b == w.binding);
        let mut acknowledgement = "unobserved";
        if let Some(control) =
            Store::existing_transport(app, &instance.source_profile, &instance.id)?
        {
            if let Some(ack) = control.read_budgeted::<SummaryAck>("runtime-ack.json", remaining)? {
                acknowledgement = if ack.version == BRIDGE_VERSION
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

#[derive(Deserialize)]
struct SummaryProfile {
    version: u32,
    profile: String,
    revision: u64,
    origin: Option<authority::Origin>,
    sessions: std::collections::BTreeMap<String, SummarySession>,
    batches: std::collections::BTreeMap<String, SummaryBatch>,
}
impl SummaryProfile {
    fn batch_summaries(&self, now: i64) -> Result<Vec<Value>> {
        ensure!(self.batches.len() <= 64, "batch summary capacity exceeded");
        let mut membership = std::collections::BTreeSet::new();
        let mut summaries = Vec::new();
        for (id, operation) in &self.batches {
            let summary = operation.summary(id, now)?;
            for (session, member) in &operation.members {
                let Some(window) = &member.window else {
                    continue;
                };
                let original = self
                    .sessions
                    .get(session)
                    .and_then(|s| s.book.windows.iter().find(|w| w.id == *window))
                    .context("original member window missing")?;
                original.validate(&self.profile, session)?;
                ensure!(
                    membership.insert((session, window)),
                    "duplicate original-window membership"
                );
                if operation.shared_requests.is_some() {
                    let tickets: Vec<_> = operation
                        .tickets
                        .iter()
                        .filter(|t| t.session == *session && t.window == *window)
                        .collect();
                    let mut requests = std::collections::BTreeSet::new();
                    for ticket in &tickets {
                        if let Some(request) = &ticket.request {
                            ensure!(
                                original.reservations.contains(request) && requests.insert(request),
                                "ticket request is absent or repeated"
                            );
                        }
                    }
                    ensure!(
                        requests.len() == original.reservations.len(),
                        "reservation lacks shared ticket"
                    );
                    ensure!(
                        original.reservations.len()
                            + tickets.iter().filter(|t| t.request.is_none()).count()
                            <= usize::from(original.grant.requests),
                        "held and reserved slots exceed local limit"
                    );
                }
            }
            summaries.push(summary);
        }
        Ok(summaries)
    }
}
#[derive(Deserialize)]
struct SummarySession {
    policy: Option<Policy>,
    book: SummaryBook,
    evidence: Vec<authority::Evidence>,
    preference: batch::state::Preference,
    exception_epoch: u64,
}

#[derive(Deserialize)]
struct SummaryBatch {
    id: String,
    preview_digest: String,
    expires_at_ms: i64,
    shared_requests: Option<u16>,
    stopped: bool,
    members: std::collections::BTreeMap<String, SummaryMember>,
    tickets: Vec<SummaryTicket>,
}
#[derive(Deserialize)]
struct SummaryMember {
    window: Option<String>,
    outcome: batch::state::Outcome,
    reason: Option<String>,
}
#[derive(Deserialize)]
struct SummaryTicket {
    session: String,
    window: String,
    request: Option<String>,
}
impl SummaryBatch {
    fn summary(&self, key: &str, now: i64) -> Result<Value> {
        ensure!(
            self.id == key && uuid::Uuid::parse_str(key).is_ok(),
            "invalid operation identity"
        );
        ensure!(
            self.preview_digest.len() == 64
                && self.preview_digest.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid operation digest"
        );
        ensure!(
            !self.members.is_empty() && self.members.len() <= 16,
            "invalid operation members"
        );
        ensure!(
            self.shared_requests
                .is_none_or(|cap| (1..=128).contains(&cap))
                && self.tickets.len() <= self.shared_requests.map_or(0, usize::from),
            "invalid shared counters"
        );
        for (id, member) in &self.members {
            super::super::validate_instance_id(id)?;
            if let Some(window) = &member.window {
                uuid::Uuid::parse_str(window)?;
            }
        }
        for ticket in &self.tickets {
            ensure!(
                self.members
                    .get(&ticket.session)
                    .and_then(|m| m.window.as_ref())
                    == Some(&ticket.window),
                "ticket outside original membership"
            );
        }
        Ok(
            json!({"operation_id":self.id,"preview_digest":self.preview_digest,"expires_at_ms":self.expires_at_ms,"expired":now>=self.expires_at_ms,"stopped":self.stopped,"shared_requests":self.shared_requests,"shared_charges":self.shared_requests.map(|_|self.tickets.len()),"held_unbound":self.shared_requests.map(|_|self.tickets.iter().filter(|t|t.request.is_none()).count()),"members":self.members.iter().map(|(id,m)|json!({"session_id":id,"window":m.window,"outcome":m.outcome,"reason":m.reason.as_ref().map(|r|shown(r))})).collect::<Vec<_>>(),"observation":"stored host summary; no fresh acknowledgement"}),
        )
    }
}

// Discard task text, decision bodies and file payloads.
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
    fn validate(&self, profile: &str, id: &str) -> Result<()> {
        ensure!(
            self.version == 4 && self.grant.version == 4,
            "unsupported window/grant version"
        );
        ensure!(
            self.binding.instance_id == id && self.binding.profile == profile,
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
pub(super) mod tests;
