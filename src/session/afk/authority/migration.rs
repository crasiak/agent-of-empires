//! Restartable, evidence-preserving transition from per-instance authority.
use super::super::delegation::Book;
use super::*;
pub(in crate::session::afk) mod frozen;
use crate::session::{afk::schema as cursor, backup_before_migration, AnchoredDir};
use frozen::{profile_store, Session, Snapshot, NAMESPACE, PROFILE_BYTES};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const INVENTORY_LIMIT: usize = 4096;
const ORIGINAL_BYTES: usize = 256 * 1024 * 1024;
const NAMESPACES: [&str; 4] = ["afk", "afk-runtime-v2", "afk-runtime-v3", "afk-runtime-v4"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Roots,
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
struct RootSeal {
    namespace: String,
    device: u64,
    inode: u64,
    original_mode: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Absent {
    namespace: String,
    instance: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    phase: Phase,
    roots: Vec<RootSeal>,
    absent: Vec<Absent>,
    entries: Vec<Evidence>,
    imports: BTreeMap<String, String>,
}
impl Manifest {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 40
                && self.entries.len() + self.absent.len() <= INVENTORY_LIMIT
                && serde_json::to_vec(self)?.len() <= MANIFEST_BYTES,
            "invalid migration 40 manifest"
        );
        ensure!(
            self.roots.len() == NAMESPACES.len(),
            "incomplete legacy root provenance"
        );
        for (root, namespace) in self.roots.iter().zip(NAMESPACES) {
            ensure!(
                root.namespace == namespace && root.original_mode == 0o700,
                "invalid legacy root provenance"
            );
        }
        let mut seen = BTreeSet::new();
        for absent in &self.absent {
            ensure!(
                NAMESPACES.contains(&absent.namespace.as_str())
                    && seen.insert((&absent.namespace, &absent.instance)),
                "duplicate/unknown absent legacy source"
            );
            scope("migration", Some(&absent.instance))?;
        }
        for (index, entry) in self.entries.iter().enumerate() {
            ensure!(
                NAMESPACES.contains(&entry.namespace.as_str())
                    && seen.insert((&entry.namespace, &entry.instance)),
                "duplicate/unknown legacy source"
            );
            scope("migration", Some(&entry.instance))?;
            ensure!(
                entry.version == version(&entry.namespace)
                    && entry.original == original_path(index),
                "invalid original provenance"
            );
            digest_valid(&entry.sha256)?;
            let mut profiles = BTreeSet::new();
            for profile in &entry.profiles {
                scope(profile, None)?;
                ensure!(
                    profiles.insert(profile),
                    "duplicate original profile ownership"
                );
            }
        }
        let owned: BTreeSet<_> = self.entries.iter().flat_map(|e| &e.profiles).collect();
        ensure!(
            owned == self.imports.keys().collect(),
            "manifest ownership/import plan mismatch"
        );
        for (profile, digest) in &self.imports {
            scope(profile, None)?;
            digest_valid(digest)?;
        }
        Ok(())
    }
}
fn digest_valid(hash: &str) -> Result<()> {
    ensure!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid evidence digest"
    );
    Ok(())
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn version(namespace: &str) -> u32 {
    match namespace {
        "afk" => 1,
        "afk-runtime-v2" => 2,
        "afk-runtime-v3" => 3,
        _ => 4,
    }
}
fn leaf(namespace: &str) -> &'static str {
    if namespace == "afk" {
        "policy.json"
    } else {
        "ledger.json"
    }
}
fn original_leaf(index: usize) -> String {
    format!("original-{index:04}.json")
}
fn original_path(index: usize) -> String {
    format!("{NAMESPACE}/migration/{}", original_leaf(index))
}
fn migration_store(app: &Path, create: bool) -> Result<Store> {
    Store::open_namespace(
        app,
        "migration",
        create,
        NAMESPACE,
        MANIFEST_BYTES.max(delegation::SESSION_BYTES),
    )
}
fn legacy_store(app: &Path, namespace: &str, id: &str) -> Result<Store> {
    scope("migration", Some(id))?;
    let root = AnchoredDir::open(app)?.child(Path::new(namespace))?;
    let stat = root.metadata()?;
    ensure!(
        stat.st_uid == unsafe { libc::geteuid() } && stat.st_mode & 0o777 == 0o500,
        "legacy root is not sealed"
    );
    let dir = root.child(Path::new(id))?;
    super::super::store::private_dir(&dir)?;
    Ok(Store {
        dir,
        scope: None,
        limit: if namespace == "afk" {
            MAX_BYTES
        } else {
            delegation::SESSION_BYTES
        },
    })
}
fn verify_roots(app: &Path, m: &Manifest) -> Result<()> {
    let app = AnchoredDir::open(app)?;
    for recorded in &m.roots {
        let root = app.child(Path::new(&recorded.namespace))?;
        let stat = root.metadata()?;
        ensure!(
            stat.st_uid == unsafe { libc::geteuid() }
                && stat.st_mode & 0o777 == 0o500
                && stat.st_dev as u64 == recorded.device
                && stat.st_ino == recorded.inode,
            "legacy root seal identity/mode changed"
        );
    }
    Ok(())
}
fn manifest(app: &Path) -> Result<Manifest> {
    let m: Manifest = migration_store(app, false)?
        .read("manifest.json")?
        .context("missing migration 40 manifest")?;
    m.validate()?;
    Ok(m)
}
pub(super) fn ready(app: &Path) -> Result<()> {
    ready_manifest(app, &manifest(app)?)
}
fn ready_manifest(app: &Path, m: &Manifest) -> Result<()> {
    ensure!(
        cursor::read_cursor(app)? == cursor::AUTHORITY,
        "AFK authority import requires global schema {}",
        cursor::AUTHORITY
    );
    ensure!(m.phase == Phase::Complete, "migration 40 import incomplete");
    verify_roots(app, m)?;
    Ok(())
}
pub(super) fn validate_import(app: &Path, snapshot: &Snapshot) -> Result<()> {
    let m = manifest(app)?;
    if let Some(expected) = m.imports.get(&snapshot.profile) {
        if snapshot.revision == 1 {
            ensure!(
                hash(&serde_json::to_vec(snapshot)?) == *expected,
                "imported profile differs from completed manifest"
            );
        }
        for entry in m
            .entries
            .iter()
            .filter(|e| e.profiles.contains(&snapshot.profile))
        {
            let session = snapshot
                .sessions
                .get(&entry.instance)
                .context("missing imported session authority; repair required")?;
            ensure!(
                session.evidence.contains(entry),
                "missing imported evidence reference"
            );
        }
    }
    Ok(())
}
pub(super) fn expected_profile(app: &Path, profile: &str) -> Result<bool> {
    Ok(manifest(app)?.imports.contains_key(profile))
}
pub(super) fn historical_metadata(
    app: &Path,
    remaining: &mut usize,
) -> Result<(Vec<Evidence>, Vec<String>)> {
    ensure!(*remaining >= 16, "overview read capacity exceeded");
    *remaining -= 16;
    let m: Manifest = migration_store(app, false)?
        .read_budgeted("manifest.json", remaining)?
        .context("missing migration 40 manifest")?;
    m.validate()?;
    ensure!(
        m.phase == Phase::Complete,
        "incomplete migration 40 provenance"
    );
    verify_roots(app, &m)?;
    Ok((m.entries, m.imports.into_keys().collect()))
}
#[cfg(test)]
pub(in crate::session::afk) fn unassigned(app: &Path) -> Result<Vec<Evidence>> {
    Ok(manifest(app)?
        .entries
        .into_iter()
        .filter(|e| e.profiles.is_empty())
        .collect())
}

fn directories(app: &Path) -> Result<Vec<(String, String)>> {
    let root = AnchoredDir::open(app)?;
    let mut result = Vec::new();
    for namespace in NAMESPACES {
        if root.entry_stat(Path::new(namespace))?.is_none() {
            continue;
        }
        let dir = root.child(Path::new(namespace))?;
        let stat = dir.metadata()?;
        ensure!(
            stat.st_uid == unsafe { libc::geteuid() } && stat.st_mode & 0o777 == 0o500,
            "legacy root not sealed"
        );
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(dir.path())? {
            ensure!(
                result.len() + ids.len() < INVENTORY_LIMIT,
                "legacy inventory capacity exceeded"
            );
            let entry = entry?;
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF8 legacy instance"))?;
            super::super::super::validate_instance_id(&id)?;
            let child = dir
                .child(Path::new(&id))
                .with_context(|| format!("unsafe legacy store {namespace}/{id}"))?;
            super::super::store::private_dir(&child)?;
            ids.push(id);
        }
        ids.sort();
        result.extend(ids.into_iter().map(|id| (namespace.into(), id)));
    }
    Ok(result)
}

fn owners(bytes: &[u8], namespace: &str, id: &str) -> Result<Vec<String>> {
    let mut profiles = BTreeSet::new();
    if namespace == "afk" {
        let policy: Policy = serde_json::from_slice(bytes)?;
        policy.validate()?;
        ensure!(
            policy.binding.instance_id == id,
            "policy instance provenance mismatch"
        );
        scope(&policy.binding.profile, Some(id))?;
        profiles.insert(policy.binding.profile);
    } else {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyBook {
            version: u32,
            windows: Vec<Value>,
        }
        let book: LegacyBook = serde_json::from_slice(bytes)?;
        ensure!(
            book.version == version(namespace),
            "legacy ledger version mismatch"
        );
        let mut ids = BTreeSet::new();
        let mut previous = 0;
        for window in book.windows {
            ensure!(
                serde_json::to_vec(&window)?.len() <= delegation::WINDOW_BYTES,
                "oversized legacy window"
            );
            ensure!(
                window["version"].as_u64() == Some(u64::from(book.version)),
                "legacy window version mismatch"
            );
            let binding: Binding = serde_json::from_value(window["binding"].clone())?;
            scope(&binding.profile, Some(id))?;
            ensure!(
                binding.instance_id == id
                    && !binding.native_id.is_empty()
                    && !binding.launch_id.is_empty(),
                "invalid legacy binding"
            );
            let window_id = window["id"].as_str().context("missing legacy window ID")?;
            ensure!(
                uuid::Uuid::parse_str(window_id).is_ok() && ids.insert(window_id.to_owned()),
                "duplicate/invalid legacy window ID"
            );
            let revision = window["revision"]
                .as_u64()
                .context("missing legacy revision")?;
            ensure!(
                revision > previous && revision <= 9_007_199_254_740_991,
                "invalid legacy revision"
            );
            previous = revision;
            profiles.insert(binding.profile);
        }
        if namespace == "afk-runtime-v4" {
            Book::import(bytes, id)?;
        }
    }
    Ok(profiles.into_iter().collect())
}
fn imports(entries: &[Evidence], originals: &[Vec<u8>]) -> Result<BTreeMap<String, Snapshot>> {
    let mut snapshots: BTreeMap<String, Snapshot> = BTreeMap::new();
    for (entry, bytes) in entries.iter().zip(originals) {
        let books = if entry.version == 4 {
            Book::import(bytes, &entry.instance)?
        } else {
            BTreeMap::new()
        };
        for profile in &entry.profiles {
            let snapshot = snapshots
                .entry(profile.clone())
                .or_insert_with(|| Snapshot::empty(profile));
            let session = snapshot
                .sessions
                .entry(entry.instance.clone())
                .or_insert_with(Session::empty);
            session.epoch = 1;
            session.evidence.push(entry.clone());
            if entry.version == 1 {
                let mut policy: Policy = serde_json::from_slice(bytes)?;
                policy.revision = next_revision(Some(&policy))?;
                policy.enabled = false;
                policy.expires_at_ms = None;
                policy.duration_ms = 0;
                session.policy = Some(policy);
            }
            if let Some(book) = books.get(profile) {
                session.book = book.clone();
            }
        }
    }
    for (profile, snapshot) in &snapshots {
        snapshot.validate(profile)?;
    }
    Ok(snapshots)
}
fn fence(entry: &Evidence) -> Value {
    serde_json::json!({"version":40,"afk_authority_fence":"non-authorizing; do not restore executable legacy authority", "original":entry.original,"sha256":entry.sha256})
}
fn absent_fence(entry: &Absent) -> Value {
    serde_json::json!({"version":40,"afk_authority_fence":"non-authorizing", "original_absent":true,"namespace":entry.namespace,"instance":entry.instance})
}
fn save_phase(store: &Store, m: &mut Manifest, phase: Phase) -> Result<()> {
    m.phase = phase;
    ensure!(
        serde_json::to_vec(m)?.len() <= MANIFEST_BYTES,
        "migration manifest capacity exceeded"
    );
    store.write("manifest.json", m)?;
    #[cfg(test)]
    if INTERRUPT.with(|at| at.get() == Some(phase)) {
        anyhow::bail!("injected interruption at {phase:?}");
    }
    Ok(())
}
#[cfg(test)]
thread_local! { static INTERRUPT: std::cell::Cell<Option<Phase>> = const { std::cell::Cell::new(None) }; }

pub(crate) fn run_at(app: &Path) -> Result<()> {
    let store = migration_store(app, true)?;
    let _migration_lock = store.lock(Duration::from_secs(30))?;
    ensure!(
        !super::migration_v2::started(app)?,
        "migration 41 has begun; v040 reconstruction is forbidden"
    );
    run_locked(app, &store)
}

pub(super) fn prepare_upgrade(app: &Path, start: impl FnOnce() -> Result<()>) -> Result<()> {
    let store = migration_store(app, true)?;
    let _migration_lock = store.lock(Duration::from_secs(30))?;
    ensure!(
        !super::migration_v2::started(app)?,
        "migration 41 already started"
    );
    run_locked(app, &store)?;
    ready(app)?;
    verify_history(app)?;
    start()
}

pub(super) fn verify_history(app: &Path) -> Result<(Vec<Evidence>, Vec<String>)> {
    let m = manifest(app)?;
    ensure!(
        m.phase == Phase::Complete,
        "incomplete migration 40 provenance"
    );
    verify_roots(app, &m)?;
    let store = migration_store(app, false)?;
    let mut remaining = ORIGINAL_BYTES;
    for (index, entry) in m.entries.iter().enumerate() {
        let bytes = store
            .bytes(&original_leaf(index))?
            .context("missing migration 40 original")?;
        ensure!(
            bytes.len() <= remaining,
            "legacy evidence capacity exceeded"
        );
        remaining -= bytes.len();
        ensure!(
            hash(&bytes) == entry.sha256,
            "migration 40 original hash changed"
        );
        let mut profiles = owners(&bytes, &entry.namespace, &entry.instance)?;
        if profiles.is_empty() {
            profiles = m
                .entries
                .iter()
                .find(|e| e.version == 1 && e.instance == entry.instance)
                .map(|e| e.profiles.clone())
                .unwrap_or_default();
        }
        ensure!(profiles == entry.profiles, "migration 40 ownership changed");
        ensure!(
            legacy_store(app, &entry.namespace, &entry.instance)?.bytes(leaf(&entry.namespace))?
                == Some(serde_json::to_vec(&fence(entry))?),
            "migration 40 fence changed"
        );
    }
    for entry in &m.absent {
        ensure!(
            legacy_store(app, &entry.namespace, &entry.instance)?.bytes(leaf(&entry.namespace))?
                == Some(serde_json::to_vec(&absent_fence(entry))?),
            "migration 40 absence fence changed"
        );
    }
    Ok((m.entries, m.imports.into_keys().collect()))
}

fn run_locked(app: &Path, store: &Store) -> Result<()> {
    let mut m = match store.read::<Manifest>("manifest.json")? {
        Some(m) => {
            m.validate()?;
            m
        }
        None => {
            let anchor = AnchoredDir::open(app)?;
            let mut roots = Vec::new();
            for namespace in NAMESPACES {
                let root = anchor.create_child(Path::new(namespace))?;
                super::super::store::private_dir(&root)?;
                let stat = root.metadata()?;
                roots.push(RootSeal {
                    namespace: namespace.into(),
                    device: stat.st_dev as u64,
                    inode: stat.st_ino,
                    original_mode: 0o700,
                });
            }
            let mut m = Manifest {
                version: 40,
                phase: Phase::Roots,
                roots,
                absent: vec![],
                entries: vec![],
                imports: BTreeMap::new(),
            };
            save_phase(store, &mut m, Phase::Roots)?;
            m
        }
    };
    let anchor = AnchoredDir::open(app)?;
    for recorded in &m.roots {
        let root = anchor.child(Path::new(&recorded.namespace))?;
        let stat = root.metadata()?;
        ensure!(
            stat.st_uid == unsafe { libc::geteuid() }
                && [0o700, 0o500].contains(&(stat.st_mode & 0o777))
                && stat.st_dev as u64 == recorded.device
                && stat.st_ino == recorded.inode,
            "legacy root provenance changed"
        );
        if m.phase >= Phase::Sealed {
            ensure!(
                stat.st_mode & 0o777 == 0o500,
                "legacy root seal was removed"
            );
        }
        root.seal_child_creation()?;
        ensure!(
            anchor.child(Path::new(&recorded.namespace))?.identity()? == root.identity()?,
            "legacy root replaced during sealing"
        );
    }
    verify_roots(app, &m)?;
    if m.phase < Phase::Sealed {
        save_phase(store, &mut m, Phase::Sealed)?;
    }
    let dirs = directories(app)?;
    let stores = dirs
        .iter()
        .map(|(ns, id)| legacy_store(app, ns, id))
        .collect::<Result<Vec<_>>>()?;
    // All control locks precede runtime locks; directory and lock inodes remain in place.
    let _locks = stores
        .iter()
        .map(|s| s.lock(Duration::from_secs(30)))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        directories(app)? == dirs,
        "legacy inventory changed while locking; retry migration"
    );
    if m.phase < Phase::Inventory {
        let mut entries = Vec::new();
        let mut originals = Vec::new();
        let mut remaining = ORIGINAL_BYTES;
        let mut policies = BTreeMap::new();
        for ((namespace, id), source) in dirs.iter().zip(&stores) {
            if let Some(bytes) = source.bytes(leaf(namespace))? {
                ensure!(
                    bytes.len() <= remaining,
                    "legacy evidence capacity exceeded"
                );
                remaining -= bytes.len();
                let mut profiles = owners(&bytes, namespace, id).with_context(|| {
                    format!("legacy provenance {namespace}/{id}/{}", leaf(namespace))
                })?;
                if namespace == "afk" {
                    policies.insert(id.clone(), profiles.clone());
                } else if profiles.is_empty() {
                    profiles = policies.get(id).cloned().unwrap_or_default();
                }
                entries.push(Evidence {
                    namespace: namespace.clone(),
                    instance: id.clone(),
                    version: version(namespace),
                    sha256: hash(&bytes),
                    original: original_path(entries.len()),
                    profiles,
                });
                originals.push(bytes);
            } else {
                m.absent.push(Absent {
                    namespace: namespace.clone(),
                    instance: id.clone(),
                });
            }
        }
        let imports = imports(&entries, &originals)?
            .into_iter()
            .map(|(p, s)| Ok((p, hash(&serde_json::to_vec(&s)?))))
            .collect::<Result<_>>()?;
        m.entries = entries;
        m.imports = imports;
        save_phase(store, &mut m, Phase::Inventory)?;
    }
    let actual: BTreeSet<_> = dirs.iter().cloned().collect();
    let expected: BTreeSet<_> = m
        .entries
        .iter()
        .map(|e| (e.namespace.clone(), e.instance.clone()))
        .chain(
            m.absent
                .iter()
                .map(|e| (e.namespace.clone(), e.instance.clone())),
        )
        .collect();
    ensure!(
        actual == expected,
        "legacy inventory conflicts with migration manifest"
    );
    for entry in &m.absent {
        let current =
            legacy_store(app, &entry.namespace, &entry.instance)?.bytes(leaf(&entry.namespace))?;
        ensure!(
            current.is_none() || current == Some(serde_json::to_vec(&absent_fence(entry))?),
            "previously absent legacy leaf gained conflicting authority"
        );
    }
    let mut originals = Vec::new();
    let mut remaining = ORIGINAL_BYTES;
    for (index, entry) in m.entries.iter().enumerate() {
        let source = legacy_store(app, &entry.namespace, &entry.instance)?;
        let current = source
            .bytes(leaf(&entry.namespace))?
            .context("legacy source disappeared")?;
        let fenced = current == serde_json::to_vec(&fence(entry))?;
        ensure!(
            fenced || hash(&current) == entry.sha256,
            "legacy source advanced/conflicted: {}/{}",
            entry.namespace,
            entry.instance
        );
        let original_name = original_leaf(index);
        let bytes = match store.bytes(&original_name)? {
            Some(bytes) => bytes,
            None => {
                ensure!(
                    !fenced && m.phase == Phase::Inventory,
                    "missing verified original; fence must never be backed up as original"
                );
                backup_before_migration(&source.dir.path().join(leaf(&entry.namespace)))?;
                store.write_bytes(&original_name, &current)?;
                store
                    .bytes(&original_name)?
                    .context("original publication missing")?
            }
        };
        ensure!(
            bytes.len() <= remaining,
            "legacy evidence capacity exceeded"
        );
        remaining -= bytes.len();
        ensure!(
            hash(&bytes) == entry.sha256,
            "corrupt migration original: {}",
            entry.original
        );
        let mut profiles = owners(&bytes, &entry.namespace, &entry.instance)?;
        if profiles.is_empty() {
            profiles = m
                .entries
                .iter()
                .find(|e| e.version == 1 && e.instance == entry.instance)
                .map(|e| e.profiles.clone())
                .unwrap_or_default();
        }
        ensure!(profiles == entry.profiles, "conflicting original ownership");
        // The generic backup helper's directory-sync warning is not the durability boundary.
        store
            .dir
            .open_regular(Path::new(&original_name), delegation::SESSION_BYTES)?
            .context("original missing")?
            .sync_all()?;
        store.dir.sync()?;
        originals.push(bytes);
    }
    let snapshots = imports(&m.entries, &originals)?;
    let digests: BTreeMap<_, _> = snapshots
        .iter()
        .map(|(p, s)| Ok((p.clone(), hash(&serde_json::to_vec(s)?))))
        .collect::<Result<_>>()?;
    ensure!(
        digests == m.imports,
        "import plan conflicts with verified originals"
    );
    if m.phase < Phase::Originals {
        save_phase(store, &mut m, Phase::Originals)?;
    }
    for entry in &m.entries {
        legacy_store(app, &entry.namespace, &entry.instance)?
            .write(leaf(&entry.namespace), &fence(entry))?;
    }
    for entry in &m.absent {
        legacy_store(app, &entry.namespace, &entry.instance)?
            .write(leaf(&entry.namespace), &absent_fence(entry))?;
    }
    if m.phase < Phase::Fences {
        save_phase(store, &mut m, Phase::Fences)?;
    }
    for (profile, snapshot) in &snapshots {
        let target = profile_store(app, profile, true)?;
        let _lock = target.lock(TIMEOUT)?;
        match target.bytes("snapshot.json")? {
            None => {
                ensure!(
                    m.phase < Phase::Imports,
                    "expected imported snapshot disappeared"
                );
                target.write("snapshot.json", snapshot)?;
            }
            Some(bytes) => {
                if m.phase == Phase::Complete && ready(app).is_ok() {
                    let current: Snapshot = serde_json::from_slice(&bytes)?;
                    current.validate(profile)?;
                    ensure!(
                        current.revision >= snapshot.revision,
                        "import revision regressed"
                    );
                } else {
                    ensure!(
                        hash(&bytes) == m.imports[profile],
                        "profile import conflict; refusing to overwrite advanced authority"
                    );
                }
            }
        }
        target
            .dir
            .open_regular(Path::new("snapshot.json"), PROFILE_BYTES)?
            .context("import disappeared before durability verification")?
            .sync_all()?;
        target.dir.sync()?;
    }
    for (profile, expected) in &m.imports {
        let target = profile_store(app, profile, false)?;
        let bytes = target
            .bytes("snapshot.json")?
            .context("import verification missing snapshot")?;
        if m.phase != Phase::Complete || ready(app).is_err() {
            ensure!(hash(&bytes) == *expected, "import verification mismatch");
        } else {
            let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
            snapshot.validate(profile)?;
            validate_import(app, &snapshot)?;
        }
    }
    if m.phase < Phase::Imports {
        save_phase(store, &mut m, Phase::Imports)?;
    }
    for entry in &m.entries {
        ensure!(
            legacy_store(app, &entry.namespace, &entry.instance)?.bytes(leaf(&entry.namespace))?
                == Some(serde_json::to_vec(&fence(entry))?),
            "legacy fence verification failed"
        );
    }
    for entry in &m.absent {
        ensure!(
            legacy_store(app, &entry.namespace, &entry.instance)?.bytes(leaf(&entry.namespace))?
                == Some(serde_json::to_vec(&absent_fence(entry))?),
            "absent leaf fence verification failed"
        );
    }
    verify_roots(app, &m)?;
    ensure!(
        directories(app)? == dirs,
        "legacy stores appeared during migration"
    );
    if m.phase < Phase::Verified {
        save_phase(store, &mut m, Phase::Verified)?;
    }
    if m.phase < Phase::Complete {
        save_phase(store, &mut m, Phase::Complete)?;
    }
    tracing::info!("Migration 40 preserved originals, fenced legacy leaves and imported non-authorizing profile history");
    Ok(())
}

#[cfg(test)]
pub(crate) fn initialize_test(app: &Path) {
    super::migration_v3::initialize_test(app);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) mod golden;
