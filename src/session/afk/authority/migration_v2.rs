//! Restartable transition from frozen v1 profiles to current authority.
use super::*;
use crate::session::{backup_before_migration, AnchoredDir};
use delegation::hash;
use migration::frozen;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

const MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const TOTAL_ORIGINAL_BYTES: usize = 256 * 1024 * 1024;
const MAX_PROFILES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Started,
    Sealed,
    Inventory,
    Originals,
    Fences,
    Imports,
    Verified,
    Complete,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Entry {
    pub profile: String,
    pub origin: Option<Origin>,
    pub sessions: Vec<String>,
    pub revision: Option<u64>,
    pub imported_hash: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    phase: Phase,
    root_identity: (u64, u64),
    entries: Vec<Entry>,
}
impl Manifest {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 41
                && self.entries.len() <= MAX_PROFILES
                && serde_json::to_vec(self)?.len() <= MANIFEST_BYTES,
            "invalid migration 41 manifest"
        );
        let mut profiles = BTreeSet::new();
        for (index, entry) in self.entries.iter().enumerate() {
            scope(&entry.profile, None)?;
            ensure!(
                profiles.insert(&entry.profile),
                "duplicate v1 profile provenance"
            );
            match &entry.origin {
                Some(origin) => {
                    ensure!(
                        origin.profile == entry.profile && origin.original == original_path(index),
                        "invalid v1 original scope/path"
                    );
                    digest(&origin.sha256)?;
                    digest(
                        entry
                            .imported_hash
                            .as_deref()
                            .context("missing import hash")?,
                    )?;
                    ensure!(
                        entry.revision.is_some_and(|r| r > 1),
                        "invalid import revision"
                    );
                    let mut sessions = BTreeSet::new();
                    for id in &entry.sessions {
                        scope(&entry.profile, Some(id))?;
                        ensure!(sessions.insert(id), "duplicate imported session");
                    }
                }
                None => ensure!(
                    entry.imported_hash.is_none()
                        && entry.revision.is_none()
                        && entry.sessions.is_empty(),
                    "absent v1 source fabricated an import"
                ),
            }
        }
        Ok(())
    }
}
fn digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid authority digest"
    );
    Ok(())
}
fn original_name(index: usize) -> String {
    format!("original-{index:04}.json")
}
fn original_path(index: usize) -> String {
    format!("{NAMESPACE}/migration/{}", original_name(index))
}
fn store(app: &Path, create: bool) -> Result<Store> {
    Store::open_namespace(app, "migration", create, NAMESPACE, PROFILE_BYTES)
}
pub(super) fn started(app: &Path) -> Result<bool> {
    let mut dir = AnchoredDir::open(app)?;
    for component in [NAMESPACE, "migration"] {
        if dir.entry_stat(Path::new(component))?.is_none() {
            return Ok(false);
        }
        dir = dir.child(Path::new(component))?;
        super::super::store::private_dir(&dir)?;
    }
    Ok(dir.entry_stat(Path::new("manifest.json"))?.is_some())
}
fn read_manifest(store: &Store, remaining: &mut usize) -> Result<Option<Manifest>> {
    let mut budget = (*remaining).min(MANIFEST_BYTES);
    let before = budget;
    let result = store.read_budgeted("manifest.json", &mut budget);
    *remaining -= before - budget;
    result
}
fn manifest(app: &Path) -> Result<Manifest> {
    let s = store(app, false)?;
    let mut budget = MANIFEST_BYTES;
    let m: Manifest = read_manifest(&s, &mut budget)?.context("missing migration 41 manifest")?;
    m.validate()?;
    Ok(m)
}
fn ingress(app: &Path) -> Result<AnchoredDir> {
    AnchoredDir::open(app)?.child(Path::new("afk-authority-v1/profiles"))
}
fn verify_seal(app: &Path, m: &Manifest) -> Result<()> {
    let dir = ingress(app)?;
    let stat = dir.metadata()?;
    ensure!(
        stat.st_uid == unsafe { libc::geteuid() }
            && stat.st_mode & 0o777 == 0o500
            && (stat.st_dev as u64, stat.st_ino) == m.root_identity,
        "v1 profile ingress seal changed"
    );
    Ok(())
}
fn schema(app: &Path) -> Result<String> {
    let file = AnchoredDir::open(app)?
        .open_regular(Path::new(".schema_version"), 16)?
        .context("missing schema version")?;
    let mut version = String::new();
    file.take(17).read_to_string(&mut version)?;
    Ok(version.trim().to_owned())
}
fn ready_manifest(app: &Path, m: &Manifest) -> Result<()> {
    ensure!(
        schema(app)? == "41" && m.phase == Phase::Complete,
        "completed migration 41 and schema41 required"
    );
    verify_seal(app, m)
}
pub(super) fn ready(app: &Path) -> Result<()> {
    ready_manifest(app, &manifest(app)?)?;
    migration::historical_metadata(app, &mut (MANIFEST_BYTES + 16))?;
    Ok(())
}
pub(super) fn expected_profile(app: &Path, profile: &str) -> Result<bool> {
    Ok(manifest(app)?
        .entries
        .iter()
        .any(|e| e.profile == profile && e.origin.is_some()))
}
pub(super) fn validate_import(app: &Path, snapshot: &Snapshot) -> Result<()> {
    let m = manifest(app)?;
    if let Some(entry) = m
        .entries
        .iter()
        .find(|e| e.profile == snapshot.profile && e.origin.is_some())
    {
        ensure!(
            snapshot.origin == entry.origin,
            "missing or changed v1 authority provenance"
        );
        ensure!(
            Some(snapshot.revision) >= entry.revision,
            "v2 import revision regressed"
        );
        if Some(snapshot.revision) == entry.revision {
            ensure!(
                Some(hash(serde_json::to_vec(snapshot)?)) == entry.imported_hash,
                "v2 import differs from completed manifest"
            );
        }
        for id in &entry.sessions {
            ensure!(
                snapshot.sessions.contains_key(id),
                "missing imported session; explicit repair required"
            );
        }
    } else {
        ensure!(
            snapshot.origin.is_none(),
            "unexpected v1 authority provenance"
        );
    }
    migration::validate_import(
        app,
        &frozen::Snapshot {
            version: 1,
            profile: snapshot.profile.clone(),
            revision: snapshot.revision.max(2),
            sessions: snapshot
                .sessions
                .iter()
                .map(|(id, session)| (id.clone(), session.legacy()))
                .collect(),
        },
    )
}

pub(in crate::session::afk) struct Metadata {
    pub evidence: Vec<Evidence>,
    pub profiles: Vec<String>,
    pub absent_profiles: Vec<String>,
    pub imports: BTreeMap<String, Entry>,
}
pub(in crate::session::afk) fn overview_metadata(
    app: &Path,
    remaining: &mut usize,
) -> Result<Metadata> {
    let m: Manifest =
        read_manifest(&store(app, false)?, remaining)?.context("missing migration 41 manifest")?;
    m.validate()?;
    ready_manifest(app, &m)?;
    let (evidence, _) = migration::historical_metadata(app, remaining)?;
    let profiles = m
        .entries
        .iter()
        .filter(|e| e.origin.is_some())
        .map(|e| e.profile.clone())
        .collect();
    let absent_profiles = m
        .entries
        .iter()
        .filter(|e| e.origin.is_none())
        .map(|e| e.profile.clone())
        .collect();
    Ok(Metadata {
        evidence,
        profiles,
        absent_profiles,
        imports: m
            .entries
            .into_iter()
            .filter(|e| e.origin.is_some())
            .map(|e| (e.profile.clone(), e))
            .collect(),
    })
}
fn old_store(app: &Path, profile: &str) -> Result<Store> {
    scope(profile, None)?;
    let dir = ingress(app)?.child(Path::new(profile))?;
    super::super::store::private_dir(&dir)?;
    Ok(Store {
        dir,
        scope: None,
        limit: PROFILE_BYTES,
    })
}
fn profiles(app: &Path, m: &Manifest) -> Result<Vec<String>> {
    verify_seal(app, m)?;
    let root = ingress(app)?;
    let mut profiles = Vec::new();
    for entry in std::fs::read_dir(root.path())? {
        ensure!(
            profiles.len() < MAX_PROFILES,
            "v1 profile inventory capacity exceeded"
        );
        let profile = entry?
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF8 v1 profile"))?;
        old_store(app, &profile)?;
        profiles.push(profile);
    }
    profiles.sort();
    Ok(profiles)
}
fn import(bytes: &[u8], origin: &Origin) -> Result<Snapshot> {
    let old: frozen::Snapshot = serde_json::from_slice(bytes)?;
    old.validate(&origin.profile)?;
    let mut sessions: BTreeMap<String, Session> = old
        .sessions
        .into_iter()
        .map(|(id, session)| (id, session.into()))
        .collect();
    for session in sessions.values_mut() {
        session.epoch = session
            .epoch
            .checked_add(1)
            .context("control epoch overflow")?;
        session.book.retire_for_upgrade();
        if let Some(policy) = &mut session.policy {
            policy.revision = next_revision(Some(policy))?;
            policy.enabled = false;
            policy.expires_at_ms = None;
            policy.duration_ms = 0;
        }
    }
    let snapshot = Snapshot {
        version: 2,
        profile: origin.profile.clone(),
        revision: old
            .revision
            .checked_add(1)
            .context("profile revision overflow")?,
        sessions,
        origin: Some(origin.clone()),
        batches: Default::default(),
    };
    snapshot.validate(&origin.profile)?;
    Ok(snapshot)
}
fn fence(entry: &Entry) -> Value {
    serde_json::json!({"version":41,"afk_profile_fence":"non-authorizing; preserve originals", "profile":entry.profile,"origin":entry.origin,"original_absent":entry.origin.is_none()})
}
fn save(store: &Store, m: &mut Manifest, phase: Phase) -> Result<()> {
    m.phase = phase;
    m.validate()?;
    store.write_raw("manifest.json", m)?;
    #[cfg(test)]
    if INTERRUPT.with(|at| at.get() == Some(phase)) {
        anyhow::bail!("injected migration41 interruption {phase:?}");
    }
    Ok(())
}
#[cfg(test)]
thread_local! { static INTERRUPT: std::cell::Cell<Option<Phase>> = const { std::cell::Cell::new(None) }; }

pub(crate) fn run_at(app: &Path) -> Result<()> {
    ensure!(
        matches!(schema(app)?.as_str(), "40" | "41"),
        "migration 41 requires schema40 prerequisite or schema41 replay"
    );
    let state = store(app, true)?;
    let _upgrade_lock = state.lock(Duration::from_secs(30))?;
    let mut manifest_budget = MANIFEST_BYTES;
    let mut m = match read_manifest(&state, &mut manifest_budget)? {
        Some(m) => {
            m.validate()?;
            m
        }
        None => {
            let mut prepared = None;
            migration::prepare_upgrade(app, || {
                let root = AnchoredDir::open(app)?
                    .child(Path::new(frozen::NAMESPACE))?
                    .create_child(Path::new("profiles"))?;
                super::super::store::private_dir(&root)?;
                let (dev, ino) = root.identity()?;
                let mut m = Manifest {
                    version: 41,
                    phase: Phase::Started,
                    root_identity: (dev as u64, ino),
                    entries: vec![],
                };
                save(&state, &mut m, Phase::Started)?;
                prepared = Some(m);
                Ok(())
            })?;
            prepared.context("missing upgrade start")?
        }
    };
    let (legacy_evidence, expected_legacy_profiles) = migration::verify_history(app)?;
    let root = ingress(app)?;
    let stat = root.metadata()?;
    ensure!(
        stat.st_uid == unsafe { libc::geteuid() }
            && (stat.st_dev as u64, stat.st_ino) == m.root_identity
            && [0o700, 0o500].contains(&(stat.st_mode & 0o777)),
        "v1 ingress provenance changed"
    );
    if m.phase >= Phase::Sealed {
        ensure!(stat.st_mode & 0o777 == 0o500, "v1 ingress seal removed");
    }
    root.seal_child_creation()?;
    verify_seal(app, &m)?;
    if m.phase < Phase::Sealed {
        save(&state, &mut m, Phase::Sealed)?;
    }
    let names = profiles(app, &m)?;
    for expected in &expected_legacy_profiles {
        ensure!(
            names.contains(expected),
            "missing expected v040 profile {expected}"
        );
    }
    let sources = names
        .iter()
        .map(|p| old_store(app, p))
        .collect::<Result<Vec<_>>>()?;
    let _locks = sources
        .iter()
        .map(|s| s.lock(Duration::from_secs(30)))
        .collect::<Result<Vec<_>>>()?;
    ensure!(profiles(app, &m)? == names, "v1 profile inventory changed");
    if m.phase < Phase::Inventory {
        let mut remaining = TOTAL_ORIGINAL_BYTES;
        let mut entries = Vec::new();
        for (index, (profile, source)) in names.iter().zip(&sources).enumerate() {
            if let Some(bytes) = source.bytes("snapshot.json")? {
                ensure!(bytes.len() <= remaining, "v1 original capacity exceeded");
                remaining -= bytes.len();
                let old: frozen::Snapshot = serde_json::from_slice(&bytes)?;
                old.validate(profile)?;
                migration::validate_import(app, &old)?;
                ensure!(
                    old.sessions
                        .values()
                        .flat_map(|s| &s.evidence)
                        .all(|e| legacy_evidence.contains(e)),
                    "v1 snapshot references unknown original evidence"
                );
                let origin = Origin {
                    profile: profile.clone(),
                    sha256: hash(&bytes),
                    original: original_path(index),
                };
                let next = import(&bytes, &origin)?;
                entries.push(Entry {
                    profile: profile.clone(),
                    sessions: old.sessions.into_keys().collect(),
                    revision: Some(next.revision),
                    imported_hash: Some(hash(serde_json::to_vec(&next)?)),
                    origin: Some(origin),
                });
            } else {
                ensure!(
                    !migration::expected_profile(app, profile)?,
                    "missing imported v040 snapshot"
                );
                entries.push(Entry {
                    profile: profile.clone(),
                    origin: None,
                    sessions: vec![],
                    revision: None,
                    imported_hash: None,
                });
            }
        }
        m.entries = entries;
        save(&state, &mut m, Phase::Inventory)?;
    }
    ensure!(
        m.entries
            .iter()
            .map(|e| e.profile.clone())
            .collect::<Vec<_>>()
            == names,
        "v1 inventory conflicts with upgrade manifest"
    );
    let mut imports = BTreeMap::new();
    let mut remaining = TOTAL_ORIGINAL_BYTES;
    for (index, (entry, source)) in m.entries.iter().zip(&sources).enumerate() {
        let current = source.bytes("snapshot.json")?;
        let fenced = current.as_deref() == Some(serde_json::to_vec(&fence(entry))?.as_slice());
        if let Some(origin) = &entry.origin {
            let current = current.context("v1 snapshot disappeared")?;
            ensure!(
                fenced || hash(&current) == origin.sha256,
                "v1 snapshot advanced or conflicted"
            );
            let name = original_name(index);
            let original = match state.bytes(&name)? {
                Some(bytes) => bytes,
                None => {
                    ensure!(
                        !fenced && m.phase == Phase::Inventory,
                        "missing original; never back up a fence"
                    );
                    backup_before_migration(&source.dir.path().join("snapshot.json"))?;
                    state.write_bytes(&name, &current)?;
                    state
                        .bytes(&name)?
                        .context("missing published v1 original")?
                }
            };
            ensure!(
                original.len() <= remaining && hash(&original) == origin.sha256,
                "invalid v1 original evidence"
            );
            remaining -= original.len();
            state
                .dir
                .open_regular(Path::new(&name), PROFILE_BYTES)?
                .context("missing original")?
                .sync_all()?;
            state.dir.sync()?;
            let next = import(&original, origin)?;
            ensure!(
                Some(hash(serde_json::to_vec(&next)?)) == entry.imported_hash
                    && Some(next.revision) == entry.revision,
                "v2 import plan changed"
            );
            imports.insert(entry.profile.clone(), next);
        } else {
            ensure!(
                current.is_none() || fenced,
                "prepared v1 snapshot gained conflicting data"
            );
        }
    }
    if m.phase < Phase::Originals {
        save(&state, &mut m, Phase::Originals)?;
    }
    for (entry, source) in m.entries.iter().zip(&sources) {
        source.write_raw("snapshot.json", &fence(entry))?;
    }
    if m.phase < Phase::Fences {
        save(&state, &mut m, Phase::Fences)?;
    }
    for (profile, expected) in &imports {
        let target = profile_store(app, profile, true)?;
        let _lock = target.lock(TIMEOUT)?;
        unparked(&target)?;
        match target.bytes("snapshot.json")? {
            None => {
                ensure!(m.phase < Phase::Imports, "expected v2 import disappeared");
                target.write_raw("snapshot.json", expected)?;
            }
            Some(bytes) => {
                if m.phase == Phase::Complete && ready(app).is_ok() {
                    let current: Snapshot = serde_json::from_slice(&bytes)?;
                    current.validate(profile)?;
                    validate_import(app, &current)?;
                } else {
                    ensure!(
                        hash(&bytes) == hash(serde_json::to_vec(expected)?),
                        "v2 import conflicts with existing state"
                    );
                }
            }
        }
        target
            .dir
            .open_regular(Path::new("snapshot.json"), PROFILE_BYTES)?
            .context("v2 import disappeared")?
            .sync_all()?;
        target.dir.sync()?;
    }
    if m.phase < Phase::Imports {
        save(&state, &mut m, Phase::Imports)?;
    }
    for (entry, source) in m.entries.iter().zip(&sources) {
        ensure!(
            source.bytes("snapshot.json")? == Some(serde_json::to_vec(&fence(entry))?),
            "v1 profile fence changed"
        );
    }
    verify_seal(app, &m)?;
    ensure!(
        profiles(app, &m)? == names,
        "v1 profiles appeared after seal"
    );
    if m.phase < Phase::Verified {
        save(&state, &mut m, Phase::Verified)?;
    }
    if m.phase < Phase::Complete {
        save(&state, &mut m, Phase::Complete)?;
    }
    tracing::info!("Migration 41 preserved v1 profiles, sealed ingress and published non-authorizing v2 history");
    Ok(())
}

#[cfg(test)]
pub(crate) fn initialize_test(app: &Path) {
    if !started(app).unwrap() {
        migration::run_at(app).unwrap();
        std::fs::write(app.join(".schema_version"), "40").unwrap();
    }
    run_at(app).unwrap();
    std::fs::write(app.join(".schema_version"), "41").unwrap();
}
#[cfg(test)]
mod tests;
