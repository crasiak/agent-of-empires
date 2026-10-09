//! Inspection-only exports. This format is never executable authority.
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
const CAPABILITY_LIMIT: &str = "Two fresh null-window acknowledgements observe base protocol4 SDK coverage and generation only; active work and requested nudge/question capabilities are unproven. No member is apply-eligible.";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    profile: String,
    minutes: u32,
    shared_requests: Option<u16>,
    members: Vec<Member>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    session_id: String,
    grant: Grant,
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
        let mut ids = BTreeSet::new();
        for member in &self.members {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Exclusion {
    Missing,
    Stopped,
    Archived,
    Trashed,
    Unsupported,
    Uninitialized,
    AlreadyDelegating,
    UnresolvedHuman,
    UnsafeWorkspace,
    Unacknowledged,
    StaleAuthority,
    StaleBinding,
    StaleSelection,
    StaleWorkspace,
    StaleGeneration,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
struct Host {
    title: String,
    tool: String,
    command: String,
    activity: String,
    archived: bool,
    trashed: bool,
    project_path: String,
    binding: Option<Binding>,
}
impl Host {
    fn from_instance(row: &Instance) -> Self {
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
#[derive(Clone, PartialEq, Eq, Serialize)]
struct AuthorityObservation {
    profile_revision: u64,
    control_epoch: u64,
    policy_revision: Option<u64>,
    session_hash: String,
}
#[derive(Serialize)]
struct Observation {
    generation: String,
    observed_at_ms: i64,
}
#[derive(Serialize)]
struct InspectedMember {
    session_id: String,
    grant: Grant,
    grant_hash: String,
    host: Option<Host>,
    authority: Option<AuthorityObservation>,
    workspace: Option<Workspace>,
    observations: Vec<Observation>,
    exclusions: Vec<Exclusion>,
}
impl InspectedMember {
    fn exclude(&mut self, reason: Exclusion) {
        if !self.exclusions.contains(&reason) {
            self.exclusions.push(reason);
        }
    }
}
#[derive(Serialize)]
struct Inspection {
    format: &'static str,
    version: u32,
    apply_available: bool,
    operation_id: String,
    captured_at_ms: i64,
    completed_at_ms: i64,
    profile: String,
    minutes: u32,
    shared_requests: Option<u16>,
    shared_budget: &'static str,
    bulk_exclusion: &'static str,
    expiry: &'static str,
    capability_limit: &'static str,
    future_apply: &'static str,
    members: Vec<InspectedMember>,
}
#[derive(Serialize)]
struct Artifact {
    digest: String,
    #[serde(flatten)]
    content: Inspection,
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
fn read_manifest(path: &Path) -> Result<Manifest> {
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
        .open_regular(&leaf, ARTIFACT_BYTES)?
        .context("unsafe or oversized inspection manifest (256 KiB maximum)")?;
    let meta = file.metadata()?;
    ensure!(
        meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o777 == 0o600
            && meta.nlink() == 1
            && meta.ino() == stat.st_ino,
        "manifest changed or is unsafe"
    );
    let mut bytes = Vec::new();
    file.take((ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= ARTIFACT_BYTES,
        "oversized inspection manifest"
    );
    let manifest: Manifest =
        serde_json::from_slice(&bytes).context("invalid inspection manifest")?;
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

struct Capture {
    registry: BTreeMap<String, Instance>,
    snapshot: Option<authority::Snapshot>,
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
                })
            })
            .transpose()
    }
}
fn capture(app: &Path, profile: &str) -> Result<Capture> {
    authority::ready(app)?;
    let mut dir = AnchoredDir::open(app)?;
    let mut existing = true;
    for component in [authority::NAMESPACE, "profiles", profile] {
        if dir.entry_stat(Path::new(component))?.is_none() {
            existing = false;
            break;
        }
        dir = dir.child(Path::new(component))?;
        store::private_dir(&dir)?;
    }
    let store = existing
        .then(|| authority::profile_store(app, profile, false))
        .transpose()?;
    let has_snapshot = store
        .as_ref()
        .map(|s| s.dir.entry_stat(Path::new("snapshot.json")))
        .transpose()?
        .flatten()
        .is_some();
    let _lock = if has_snapshot {
        Some(store.as_ref().unwrap().lock_existing(TIMEOUT)?)
    } else {
        None
    };
    let snapshot: Option<authority::Snapshot> = if has_snapshot {
        Some(
            store
                .as_ref()
                .unwrap()
                .read("snapshot.json")?
                .context("profile authority disappeared during capture")?,
        )
    } else {
        None
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
fn recheck(app: &Path, member: &mut InspectedMember, captured: &Capture) -> Result<()> {
    if captured.observation(&member.session_id)? != member.authority {
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
fn inspect_at(
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
    let content = Inspection {
        format:"aoe-afk-batch-inspection", version:1, apply_available:false,
        operation_id:uuid::Uuid::new_v4().to_string(), captured_at_ms, completed_at_ms:now_ms(),
        profile:manifest.profile, minutes:manifest.minutes, shared_requests:manifest.shared_requests,
        shared_budget:if manifest.shared_requests.is_some() { "Proposed shared request ceiling only; not enforced or charged" } else { "No shared budget" },
        bulk_exclusion:"unrecorded/not implemented", expiry:"No active expiry; duration starts only at a future first actual apply",
        capability_limit:CAPABILITY_LIMIT,
        future_apply:"Inspection-v1 cannot authorize apply; a new executable format and fresh complete exception, budget and grant-specific qualification are required",
        members,
    };
    Ok(Artifact {
        digest: digest(&content)?,
        content,
    })
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
        apply_available: false,
        members: artifact.content.members.len(),
    })
}

#[cfg(test)]
mod tests;
