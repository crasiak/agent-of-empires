//! Operator claims only. This namespace never publishes execution authority.
use super::*;
use std::collections::BTreeSet;

const NAMESPACE: &str = "afk-review-v1";
const PROFILE_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_PROFILES: usize = 128;
const INDEX_BYTES: usize = 64 * 1024;

#[cfg(test)]
thread_local! {
    pub(in crate::session::afk) static AFTER_PROFILE_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub selection: Selection,
    /// None targets the window grant; Some targets the original decision record.
    pub decision: Option<String>,
    pub original_hash: String,
}
impl Target {
    fn validate(&self) -> Result<()> {
        self.selection.validate()?;
        if let Some(id) = &self.decision {
            canonical_uuid(id)?;
        }
        ensure!(
            self.original_hash.len() == 64
                && self.original_hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid original target hash"
        );
        Ok(())
    }
    pub(super) fn validate_snapshot(&self, snapshot: &authority::Snapshot) -> Result<()> {
        ensure!(
            snapshot.profile == self.selection.profile,
            "annotation profile mismatch"
        );
        let facts = snapshot
            .sessions
            .get(&self.selection.session_id)
            .context("unknown review session")?
            .book
            .review_target(&self.selection.window_id, self.decision.as_deref())?;
        ensure!(
            facts.hash == self.original_hash,
            "original target hash mismatch"
        );
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Note,
    Correction,
    Reviewed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub target: Target,
    pub kind: Kind,
    pub body: String,
    pub prior: Option<String>,
}
impl Request {
    fn validate(&self) -> Result<()> {
        canonical_uuid(&self.id)?;
        self.target.validate()?;
        ensure!(self.body.len() <= 2048, "annotation body exceeds 2 KiB");
        delegation::text(&self.body)?;
        if let Some(prior) = &self.prior {
            canonical_uuid(prior)?;
            ensure!(
                self.kind == Kind::Correction && prior != &self.id,
                "only corrections can reference a prior annotation"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    pub id: String,
    pub target: Target,
    pub kind: Kind,
    pub body: Option<String>,
    pub prior: Option<String>,
    pub created_at_ms: i64,
    pub request_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Index {
    version: u32,
    profiles: BTreeSet<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    version: u32,
    profile: String,
    entries: Vec<Annotation>,
}

fn namespace(app: &Path, create: bool) -> Result<Store> {
    Store::open_namespace(app, "annotations", create, NAMESPACE, PROFILE_BYTES)
}
fn validate_index(index: &Index) -> Result<()> {
    ensure!(
        index.version == 1 && index.profiles.len() <= MAX_PROFILES,
        "invalid review index"
    );
    for profile in &index.profiles {
        validate_profile(profile)?;
    }
    Ok(())
}
fn index_at(store: &Store, budget: &mut usize) -> Result<Index> {
    let mut allowance = (*budget).min(INDEX_BYTES);
    let before = allowance;
    let result = store.read_budgeted("index.json", &mut allowance);
    *budget -= before - allowance;
    let index: Index = result?.context("missing review index; explicit repair required")?;
    validate_index(&index)?;
    Ok(index)
}

pub(crate) fn initialize(app: &Path) -> Result<()> {
    let schema = authority::migration_v3::schema(app)?;
    ensure!(
        matches!(schema.as_str(), "42" | "43"),
        "review migration requires schema42 or schema43"
    );
    authority::ready(app)?;
    let store = namespace(app, schema == "42")?;
    let _lock = if schema == "42" {
        store.lock(TIMEOUT)?
    } else {
        store.lock_existing(TIMEOUT)?
    };
    if store.dir.entry_stat(Path::new("index.json"))?.is_some() {
        index_at(&store, &mut { INDEX_BYTES })?;
    } else {
        ensure!(
            schema == "42",
            "missing review index; explicit repair required"
        );
        store.write_exclusive(
            "index.json",
            &serde_json::to_vec(&Index {
                version: 1,
                profiles: BTreeSet::new(),
            })?,
        )?;
    }
    store.dir.sync()?;
    tracing::info!("Migration 43 initialized separate private AFK operator annotations");
    Ok(())
}

pub(super) fn read_index(app: &Path, budget: &mut usize) -> Result<(Store, Index)> {
    ensure!(
        authority::migration_v3::schema(app)? == "43",
        "review annotations require migration43; run aoe migrate explicitly"
    );
    let store = namespace(app, false)?;
    let index = index_at(&store, budget)?;
    Ok((store, index))
}
fn leaf(profile: &str) -> String {
    format!("profile-{profile}.json")
}
impl Profile {
    fn validate(&self, profile: &str) -> Result<()> {
        ensure!(
            self.version == 1 && self.profile == profile && self.entries.len() <= MAX_ENTRIES,
            "invalid annotation profile metadata"
        );
        let mut ids = BTreeSet::new();
        for (position, entry) in self.entries.iter().enumerate() {
            canonical_uuid(&entry.id)?;
            entry.target.validate()?;
            ensure!(
                entry.target.selection.profile == profile
                    && ids.insert(&entry.id)
                    && entry.created_at_ms >= 0,
                "invalid annotation identity"
            );
            ensure!(
                entry.request_hash.len() == 64
                    && entry.request_hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid annotation request hash"
            );
            if let Some(body) = &entry.body {
                let request = Request {
                    id: entry.id.clone(),
                    target: entry.target.clone(),
                    kind: entry.kind,
                    body: body.clone(),
                    prior: entry.prior.clone(),
                };
                request.validate()?;
                ensure!(
                    delegation::hash(serde_json::to_vec(&request)?) == entry.request_hash,
                    "annotation content changed"
                );
            }
            if let Some(prior) = &entry.prior {
                canonical_uuid(prior)?;
                ensure!(
                    entry.kind == Kind::Correction
                        && self.entries[..position]
                            .iter()
                            .any(|e| &e.id == prior && e.target == entry.target),
                    "invalid prior annotation"
                );
            }
            ensure!(
                self.entries[..=position]
                    .iter()
                    .filter(|e| e.target == entry.target)
                    .count()
                    <= 16,
                "annotation target capacity exceeded"
            );
        }
        Ok(())
    }
    fn suppress_expired(&mut self, snapshot: &authority::Snapshot, now: i64) {
        for entry in &mut self.entries {
            let target = &entry.target;
            let facts = snapshot
                .sessions
                .get(&target.selection.session_id)
                .and_then(|s| {
                    s.book
                        .review_target(&target.selection.window_id, target.decision.as_deref())
                        .ok()
                });
            if facts.is_some_and(|f| {
                f.hash == target.original_hash
                    && !f.ambiguous
                    && now.saturating_sub(f.expires_at_ms.max(entry.created_at_ms))
                        > delegation::RETENTION_MS
            }) {
                entry.body = None;
            }
        }
    }
}
pub(super) fn read_profile(
    store: &Store,
    index: &Index,
    profile: &str,
    budget: &mut usize,
) -> Result<Profile> {
    validate_profile(profile)?;
    let value: Option<Profile> = store.read_budgeted(&leaf(profile), budget).map_err(|_| {
        anyhow::anyhow!("annotation profile unreadable, corrupt, unsafe or over read capacity")
    })?;
    match value {
        Some(value) => {
            ensure!(
                index.profiles.contains(profile),
                "unregistered annotation profile; explicit repair required"
            );
            value.validate(profile)?;
            Ok(value)
        }
        None => {
            ensure!(
                !index.profiles.contains(profile),
                "missing expected annotation profile; explicit repair required"
            );
            Ok(Profile {
                version: 1,
                profile: profile.into(),
                entries: vec![],
            })
        }
    }
}
pub(super) fn projected(
    mut profile: Profile,
    snapshot: &authority::Snapshot,
    now: i64,
) -> Vec<Annotation> {
    profile.suppress_expired(snapshot, now);
    profile.entries
}

pub fn append(request: Request) -> Result<Annotation> {
    append_at(&crate::session::get_app_dir_path()?, request, now_ms())
}
pub(in crate::session::afk) fn append_at(
    app: &Path,
    request: Request,
    now: i64,
) -> Result<Annotation> {
    request.validate()?;
    ensure!(now >= 0, "invalid annotation host timestamp");
    let mut budget = SOURCE_BYTES;
    let metadata = authority::migration_v3::overview_metadata(app, &mut budget)?;
    let store = namespace(app, false)?;
    let _lock = store.lock_existing(TIMEOUT)?;
    ensure!(
        authority::migration_v3::schema(app)? == "43",
        "annotation writes require migration43"
    );
    let mut index = index_at(&store, &mut budget)?;
    let selection = &request.target.selection;
    let snapshot = read_authority(app, &selection.profile, &metadata, &mut budget)?;
    request.target.validate_snapshot(&snapshot)?;
    let mut profile = read_profile(&store, &index, &selection.profile, &mut budget)?;
    let request_hash = delegation::hash(serde_json::to_vec(&request)?);
    profile.suppress_expired(&snapshot, now);
    if let Some(prior) = profile.entries.iter().find(|entry| entry.id == request.id) {
        ensure!(
            prior.request_hash == request_hash,
            "conflicting annotation UUID"
        );
        return Ok(prior.clone());
    }
    if let Some(prior) = &request.prior {
        ensure!(
            profile
                .entries
                .iter()
                .any(|entry| &entry.id == prior && entry.target == request.target),
            "unknown or mismatched prior annotation"
        );
    }
    ensure!(
        profile
            .entries
            .iter()
            .filter(|entry| entry.target == request.target)
            .count()
            < 16,
        "annotation target capacity exhausted"
    );
    ensure!(
        profile.entries.len() < MAX_ENTRIES,
        "annotation metadata capacity exhausted"
    );
    ensure!(
        index.profiles.contains(&selection.profile) || index.profiles.len() < MAX_PROFILES,
        "annotation profile capacity exhausted"
    );
    let entry = Annotation {
        id: request.id,
        target: request.target.clone(),
        kind: request.kind,
        body: Some(request.body),
        prior: request.prior,
        created_at_ms: now,
        request_hash,
    };
    profile.entries.push(entry.clone());
    profile.validate(&selection.profile)?;
    let bytes = serde_json::to_vec(&profile)?;
    ensure!(
        bytes.len() <= PROFILE_BYTES,
        "annotation profile exceeds 1 MiB; no entries evicted"
    );
    let index_bytes = if index.profiles.insert(selection.profile.clone()) {
        let bytes = serde_json::to_vec(&index)?;
        ensure!(
            bytes.len() <= INDEX_BYTES,
            "annotation index exceeds 64 KiB; nothing published"
        );
        Some(bytes)
    } else {
        None
    };
    store.write_bytes(&leaf(&selection.profile), &bytes)?;
    #[cfg(test)]
    if let Some(after_profile) = AFTER_PROFILE_PUBLICATION.with(|slot| slot.borrow_mut().take()) {
        after_profile();
    }
    if let Some(bytes) = index_bytes {
        // An interrupted first publication remains unregistered and fails closed.
        store.write_bytes("index.json", &bytes)?;
    }
    Ok(entry)
}
