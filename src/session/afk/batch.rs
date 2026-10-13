//! Bounded batch manifests and operator exports.
pub(super) mod state;
use super::*;
use crate::session::{anchored_fs::FilePublication, AnchoredDir};
use delegation::{Grant, Workspace};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::Permissions;
use std::io::{Cursor, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

const ARTIFACT_BYTES: usize = 256 * 1024;
const MAX_MEMBERS: usize = 16;
const CAPABILITY_LIMIT: &str = "Two fresh null-window acknowledgements observe base SDK coverage and generation only; active work and requested optional capabilities remain unproven until grant-specific apply qualification.";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Manifest {
    version: u32,
    profile: String,
    minutes: u32,
    shared_requests: Option<u16>,
    shared_usage_guard: Option<usage::Policy>,
    members: Vec<Member>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    session_id: String,
    grant: Grant,
    usage_guard: Option<usage::Policy>,
}
impl Manifest {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported inspection manifest version");
        authority::scope(&self.profile, None)?;
        ensure!(
            (1..=1440).contains(&self.minutes),
            "inspection duration must be 1..1440 minutes"
        );
        ensure!(
            self.shared_requests.is_none_or(|n| (1..=128).contains(&n)),
            "proposed shared_requests must be omitted/null or 1..128"
        );
        ensure!(
            (1..=MAX_MEMBERS).contains(&self.members.len()),
            "select 1..16 exact session IDs"
        );
        if let Some(policy) = &self.shared_usage_guard {
            policy.validate()?;
        }
        let mut ids = BTreeSet::new();
        for member in &self.members {
            if let Some(policy) = &member.usage_guard {
                policy.validate()?;
            }
            authority::scope(&self.profile, Some(&member.session_id))?;
            ensure!(
                ids.insert(&member.session_id),
                "duplicate selected session ID"
            );
            member
                .grant
                .validate()
                .with_context(|| format!("invalid grant for {}", member.session_id))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::session::afk) enum Exclusion {
    Missing,
    Stopped,
    Archived,
    Trashed,
    Unsupported,
    Uninitialized,
    BulkExcluded,
    AlreadyDelegating,
    UnresolvedHuman,
    UnsafeWorkspace,
    Unacknowledged,
    StaleAuthority,
    StaleBinding,
    StaleSelection,
    StaleWorkspace,
    StaleGeneration,
    UsageUnavailable,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Host {
    pub title: String,
    pub tool: String,
    pub command: String,
    pub activity: String,
    pub archived: bool,
    pub trashed: bool,
    pub project_path: String,
    pub binding: Option<Binding>,
}
impl Host {
    pub(in crate::session::afk) fn from_instance(row: &Instance) -> Self {
        Self {
            title: row.title.clone(),
            tool: row.tool.clone(),
            command: row.command.clone(),
            activity: row.status.as_str().into(),
            archived: row.is_archived(),
            trashed: row.is_trashed(),
            project_path: row.project_path.clone(),
            binding: Binding::for_instance(row).ok(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct AuthorityObservation {
    pub profile_revision: u64,
    pub control_epoch: u64,
    pub policy_revision: Option<u64>,
    pub session_hash: String,
    pub exception_epoch: u64,
    pub preference: state::Preference,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Observation {
    pub generation: String,
    pub observed_at_ms: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct InspectedMember {
    pub session_id: String,
    pub grant: Grant,
    pub usage_guard: Option<usage::Policy>,
    pub grant_hash: String,
    pub host: Option<Host>,
    pub authority: Option<AuthorityObservation>,
    pub workspace: Option<Workspace>,
    pub observations: Vec<Observation>,
    pub exclusions: Vec<Exclusion>,
}
impl InspectedMember {
    fn exclude(&mut self, reason: Exclusion) {
        if !self.exclusions.contains(&reason) {
            self.exclusions.push(reason);
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Inspection {
    pub format: String,
    pub version: u32,
    pub apply_available: bool,
    pub operation_id: String,
    pub captured_at_ms: i64,
    pub completed_at_ms: i64,
    pub profile: String,
    pub minutes: u32,
    pub shared_requests: Option<u16>,
    pub shared_usage_guard: Option<usage::Policy>,
    pub shared_budget: String,
    pub bulk_exclusion: String,
    pub expiry: String,
    pub capability_limit: String,
    pub future_apply: String,
    pub members: Vec<InspectedMember>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(in crate::session::afk) struct Artifact {
    pub digest: String,
    #[serde(flatten)]
    pub content: Inspection,
}
#[derive(Serialize)]
pub struct Receipt {
    output: PathBuf,
    operation_id: String,
    digest: String,
    apply_available: bool,
    members: usize,
}

fn private_parent(path: &Path) -> Result<(AnchoredDir, PathBuf)> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let leaf = path.file_name().context("a file name is required")?;
    let dir = AnchoredDir::open(parent)?;
    store::private_dir(&dir)
        .context("inspection files require an existing private owned directory (0700)")?;
    Ok((dir, PathBuf::from(leaf)))
}
fn read_private<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    read_private_bounded(path, ARTIFACT_BYTES)
}
pub(in crate::session::afk) fn read_private_bounded<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: usize,
) -> Result<T> {
    let (dir, leaf) = private_parent(path)?;
    let stat = dir
        .entry_stat(&leaf)?
        .context("missing inspection manifest")?;
    ensure!(
        stat.st_mode & libc::S_IFMT == libc::S_IFREG
            && stat.st_mode & 0o777 == 0o600
            && stat.st_uid == unsafe { libc::geteuid() }
            && stat.st_nlink == 1,
        "manifest must be an owned private regular file (0600), not a link"
    );
    let file = dir
        .open_regular(&leaf, limit)?
        .context("unsafe or oversized private AFK document")?;
    let meta = file.metadata()?;
    ensure!(
        meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o777 == 0o600
            && meta.nlink() == 1
            && meta.ino() == stat.st_ino,
        "manifest changed or is unsafe"
    );
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "oversized private AFK document");
    serde_json::from_slice(&bytes).context("invalid private AFK document")
}
fn read_manifest(path: &Path) -> Result<Manifest> {
    let manifest: Manifest = read_private(path)?;
    manifest.validate()?;
    Ok(manifest)
}
fn canonical_digest(mut content: Value) -> Result<String> {
    content.sort_all_objects();
    Ok(delegation::hash(serde_json::to_vec(&content)?))
}
fn digest(content: &Inspection) -> Result<String> {
    canonical_digest(serde_json::to_value(content)?)
}

pub(in crate::session::afk) struct Capture {
    pub registry: BTreeMap<String, Instance>,
    pub snapshot: Option<authority::Snapshot>,
}
impl Capture {
    fn observation(&self, id: &str) -> Result<Option<AuthorityObservation>> {
        let Some(snapshot) = &self.snapshot else {
            return Ok(None);
        };
        snapshot
            .sessions
            .get(id)
            .map(|session| {
                Ok(AuthorityObservation {
                    profile_revision: snapshot.revision,
                    control_epoch: session.epoch,
                    policy_revision: session.policy.as_ref().map(|p| p.revision),
                    session_hash: delegation::hash(serde_json::to_vec(session)?),
                    exception_epoch: session.exception_epoch,
                    preference: session.preference,
                })
            })
            .transpose()
    }
}
pub(in crate::session::afk) fn capture(app: &Path, profile: &str) -> Result<Capture> {
    authority::ready(app)?;
    let store = authority::existing_profile_store(app, profile)?;
    let _lock = store
        .as_ref()
        .map(|s| s.lock_existing(TIMEOUT))
        .transpose()?;
    let snapshot: Option<authority::Snapshot> = match &store {
        Some(store) => {
            authority::unparked(store)?;
            Some(
                store
                    .read("snapshot.json")?
                    .context("profile authority disappeared during capture")?,
            )
        }
        None => None,
    };
    authority::validate_existing_snapshot(app, profile, snapshot.as_ref())?;
    Ok(Capture {
        registry: registry::load(app, profile)?,
        snapshot,
    })
}
fn member_at(app: &Path, member: &Member, captured: &Capture) -> Result<InspectedMember> {
    let row = captured.registry.get(&member.session_id);
    let mut result = InspectedMember {
        session_id: member.session_id.clone(),
        grant: member.grant.clone(),
        usage_guard: member.usage_guard.clone(),
        grant_hash: delegation::hash(serde_json::to_vec(&member.grant)?),
        host: row.map(Host::from_instance),
        authority: captured.observation(&member.session_id)?,
        workspace: None,
        observations: vec![],
        exclusions: vec![],
    };
    let Some(row) = row else {
        result.exclude(Exclusion::Missing);
        return Ok(result);
    };
    if row.is_trashed() {
        result.exclude(Exclusion::Trashed);
    }
    if row.is_archived() {
        result.exclude(Exclusion::Archived);
    }
    if matches!(
        row.status,
        crate::session::Status::Stopped | crate::session::Status::Error
    ) {
        result.exclude(Exclusion::Stopped);
    }
    if result.host.as_ref().unwrap().binding.is_none() && result.exclusions.is_empty() {
        result.exclude(Exclusion::Unsupported);
    }
    if result.authority.is_none() {
        result.exclude(Exclusion::Uninitialized);
    }
    if let Some(session) = captured
        .snapshot
        .as_ref()
        .and_then(|s| s.sessions.get(&member.session_id))
    {
        if session.preference == state::Preference::Excluded {
            result.exclude(Exclusion::BulkExcluded);
        }
        let (active, unresolved) = session.book.inspection_flags();
        if active {
            result.exclude(Exclusion::AlreadyDelegating);
        }
        if unresolved {
            result.exclude(Exclusion::UnresolvedHuman);
        }
    }
    if result.exclusions.is_empty() {
        match Workspace::inspect(row, app, &member.grant) {
            Ok(workspace) => result.workspace = Some(workspace),
            Err(_) => result.exclude(Exclusion::UnsafeWorkspace),
        }
    }
    Ok(result)
}
pub(in crate::session::afk) fn recheck(
    app: &Path,
    member: &mut InspectedMember,
    captured: &Capture,
) -> Result<()> {
    let mut current_authority = captured.observation(&member.session_id)?;
    if let (Some(current), Some(expected)) = (&mut current_authority, &member.authority) {
        current.profile_revision = expected.profile_revision;
    }
    if current_authority != member.authority {
        member.exclude(Exclusion::StaleAuthority);
    }
    let current = captured.registry.get(&member.session_id);
    let host = current.map(Host::from_instance);
    if host != member.host {
        if host.as_ref().map(|h| &h.project_path) != member.host.as_ref().map(|h| &h.project_path) {
            member.exclude(Exclusion::StaleWorkspace);
        }
        member.exclude(
            if host.as_ref().and_then(|h| h.binding.as_ref())
                != member.host.as_ref().and_then(|h| h.binding.as_ref())
            {
                Exclusion::StaleBinding
            } else {
                Exclusion::StaleSelection
            },
        );
    }
    if let Some(expected) = &member.workspace {
        if current
            .and_then(|row| Workspace::inspect(row, app, &member.grant).ok())
            .as_ref()
            != Some(expected)
        {
            member.exclude(Exclusion::StaleWorkspace);
        }
    }
    Ok(())
}
pub(in crate::session::afk) fn inspect_at(
    app: &Path,
    manifest: Manifest,
    mut observe: impl FnMut(&Store, &Binding) -> Result<String>,
) -> Result<Artifact> {
    manifest.validate()?;
    let captured_at_ms = now_ms();
    let initial = capture(app, &manifest.profile)?;
    let mut members = manifest
        .members
        .iter()
        .map(|m| member_at(app, m, &initial))
        .collect::<Result<Vec<_>>>()?;
    for member in &mut members {
        if !member.exclusions.is_empty() {
            continue;
        }
        let binding = member
            .host
            .as_ref()
            .and_then(|h| h.binding.clone())
            .context("missing inspection binding")?;
        let observation = (|| -> Result<()> {
            let transport = Store::existing_transport(app, &manifest.profile, &member.session_id)?
                .context("no existing transport")?;
            // Profile capture/recheck locks are released before transport acquisition or waits.
            let _transport_lock = transport.lock(TIMEOUT)?;
            recheck(app, member, &capture(app, &manifest.profile)?)?;
            if !member.exclusions.is_empty() {
                return Ok(());
            }
            for _ in 0..2 {
                let generation = observe(&transport, &binding)?;
                ensure!(
                    uuid::Uuid::parse_str(&generation).is_ok(),
                    "invalid observed generation"
                );
                member.observations.push(Observation {
                    generation,
                    observed_at_ms: now_ms(),
                });
                recheck(app, member, &capture(app, &manifest.profile)?)?;
                if !member.exclusions.is_empty() {
                    break;
                }
            }
            Ok(())
        })();
        if observation.is_err() {
            member.exclude(Exclusion::Unacknowledged);
        }
        if member.observations.len() == 2
            && member.observations[0].generation != member.observations[1].generation
        {
            member.exclude(Exclusion::StaleGeneration);
        }
    }
    let final_capture = capture(app, &manifest.profile)?;
    for member in &mut members {
        recheck(app, member, &final_capture)?;
    }
    let shared_budget = if manifest.shared_requests.is_some() {
        "Proposed shared request ceiling; enforced only after explicit apply"
    } else if manifest.shared_usage_guard.is_some() {
        "No shared request budget; observed usage guard is separate"
    } else {
        "No shared budget"
    }
    .into();
    let content = Inspection {
        format:"aoe-afk-batch-preview".into(), version:3, apply_available:true,
        operation_id:uuid::Uuid::new_v4().to_string(), captured_at_ms, completed_at_ms:now_ms(),
        profile:manifest.profile, minutes:manifest.minutes, shared_requests:manifest.shared_requests,shared_usage_guard:manifest.shared_usage_guard,
        shared_budget,
        bulk_exclusion:"Per-member preference and exception epoch are frozen; unrecorded is not an inherited opt-in or opt-out".into(), expiry:"No active expiry; duration starts at first explicit apply".into(),
        capability_limit:CAPABILITY_LIMIT.into(),
        future_apply:"Explicit apply must revalidate frozen members and obtain exact grant-specific acknowledgement; this preview itself grants nothing".into(),
        members,
    };
    Ok(Artifact {
        digest: digest(&content)?,
        content,
    })
}

pub(in crate::session::afk) fn executable(path: &Path, expected_digest: &str) -> Result<Artifact> {
    let mut value: Value = read_private(path)?;
    ensure!(
        value["format"] == "aoe-afk-batch-preview"
            && value["version"] == 3
            && value["apply_available"] == true,
        "inspection-v1, preview-v2 and unqualified formats cannot authorize apply; create a fresh preview-v3"
    );
    let recorded = value
        .as_object_mut()
        .context("invalid preview")?
        .remove("digest")
        .context("missing preview digest")?;
    ensure!(
        recorded.as_str() == Some(expected_digest)
            && canonical_digest(value.clone())? == expected_digest,
        "reviewed preview digest changed"
    );
    let content: Inspection = serde_json::from_value(value)?;
    ensure!(
        uuid::Uuid::parse_str(&content.operation_id).is_ok()
            && content.captured_at_ms > 0
            && content.completed_at_ms >= content.captured_at_ms,
        "invalid preview identity/time"
    );
    let manifest = Manifest {
        version: 1,
        profile: content.profile.clone(),
        minutes: content.minutes,
        shared_requests: content.shared_requests,
        shared_usage_guard: content.shared_usage_guard.clone(),
        members: content
            .members
            .iter()
            .map(|m| Member {
                session_id: m.session_id.clone(),
                grant: m.grant.clone(),
                usage_guard: m.usage_guard.clone(),
            })
            .collect(),
    };
    manifest.validate()?;
    for member in &content.members {
        ensure!(
            member.grant_hash == delegation::hash(serde_json::to_vec(&member.grant)?),
            "preview grant hash changed"
        );
        if member.exclusions.is_empty() {
            let binding = member
                .host
                .as_ref()
                .and_then(|h| h.binding.as_ref())
                .context("candidate missing binding")?;
            ensure!(
                binding.profile == content.profile
                    && binding.instance_id == member.session_id
                    && member.authority.is_some()
                    && member.workspace.is_some(),
                "incomplete candidate scope"
            );
            ensure!(
                member.observations.len() == 2
                    && member.observations[0].generation == member.observations[1].generation
                    && uuid::Uuid::parse_str(&member.observations[0].generation).is_ok(),
                "candidate generation unobserved"
            );
        }
    }
    Ok(Artifact {
        digest: expected_digest.into(),
        content,
    })
}

pub fn review(preview: &Path) -> Result<Value> {
    let raw: Value = read_private(preview)?;
    let digest = raw["digest"].as_str().context("missing preview digest")?;
    let artifact = executable(preview, digest)?;
    serde_json::to_value(artifact).context("preview cannot be displayed")
}

pub fn apply(preview: &Path, digest: &str) -> Result<Value> {
    let artifact = executable(preview, digest)?;
    authority::publication_result(|| {
        delegation::batch_control::apply(&super::super::get_app_dir_path()?, artifact)
    })
}
pub fn status(profile: &str, operation: &str) -> Result<Value> {
    delegation::batch_control::status(&super::super::get_app_dir_path()?, profile, operation)
}
pub fn off(profile: &str, operation: &str) -> Result<Value> {
    authority::publication_result(|| {
        delegation::batch_control::off(&super::super::get_app_dir_path()?, profile, operation)
    })
}

pub fn allow_bulk(instance: &Instance) -> Result<Value> {
    authority::publication_result(|| allow_bulk_inner(instance))
}
fn allow_bulk_inner(instance: &Instance) -> Result<Value> {
    let app = super::super::get_app_dir_path()?;
    let profile = instance.effective_profile();
    ensure!(
        registry::load(&app, &profile)?.contains_key(&instance.id),
        "session no longer exists"
    );
    let store = Store::authority(&app, &profile, &instance.id, true)?;
    let _lock = store.lock_existing(TIMEOUT)?;
    let mut session = store.session()?;
    session.preference(state::Preference::Allowed)?;
    let epoch = session.exception_epoch;
    store.publish_session(session)?;
    Ok(
        serde_json::json!({"session_id":instance.id,"bulk_preference":"allowed","exception_epoch":epoch,"authority_created":false,"window_changed":false}),
    )
}

pub fn preview(manifest: &Path, output: &Path) -> Result<Receipt> {
    preview_at(&super::super::get_app_dir_path()?, manifest, output)
}
fn preview_at(app: &Path, manifest: &Path, output: &Path) -> Result<Receipt> {
    preview_with(app, manifest, output, delegation::observe_base_runtime)
}
fn preview_with(
    app: &Path,
    manifest: &Path,
    output: &Path,
    observe: impl FnMut(&Store, &Binding) -> Result<String>,
) -> Result<Receipt> {
    let manifest = read_manifest(manifest)?;
    let (parent, leaf) = private_parent(output)?;
    ensure!(
        parent.entry_stat(&leaf)?.is_none(),
        "inspection output already exists; never overwrite a reviewed artifact"
    );
    authority::ready(app)?;
    let expected_parent = std::fs::canonicalize(parent.path())?;
    ensure!(
        !expected_parent.starts_with(std::fs::canonicalize(app)?),
        "inspection output must be outside the application state directory"
    );
    let validate_output = || -> Result<()> {
        let (current, _) = private_parent(output)?;
        ensure!(
            current.identity()? == parent.identity()?,
            "inspection output parent identity changed"
        );
        let location = std::fs::canonicalize(current.path())?;
        ensure!(
            location == expected_parent && !location.starts_with(std::fs::canonicalize(app)?),
            "inspection output parent location changed"
        );
        Ok(())
    };
    let artifact = inspect_at(app, manifest, observe)?;
    let mut bytes = serde_json::to_vec_pretty(&artifact)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= ARTIFACT_BYTES,
        "inspection artifact exceeds 256 KiB; nothing published"
    );
    validate_output().context("inspection output directory changed; nothing published")?;
    ensure!(
        parent.publish_file(
            &leaf,
            &mut Cursor::new(bytes),
            Permissions::from_mode(0o600),
            false,
            Some(FilePublication {
                staging: &parent,
                validate: &validate_output,
            })
        )?,
        "inspection output already exists; never overwrite a reviewed artifact"
    );
    validate_output().context("inspection output directory changed during publication; a private artifact may exist, inspect before retrying")?;
    Ok(Receipt {
        output: output.into(),
        operation_id: artifact.content.operation_id,
        digest: artifact.digest,
        apply_available: true,
        members: artifact.content.members.len(),
    })
}

#[cfg(test)]
mod tests;
