//! Profile-scoped publication. Transport files never supply authority.
use super::*;
use serde_json::Value;
use std::path::Path;

pub(super) mod journal;
mod transaction;
pub(super) use journal::collect as publication_context;
pub(super) use transaction::SessionTransaction;

pub(super) fn publication_result(run: impl FnOnce() -> Result<Value>) -> Result<Value> {
    let (result, warnings) = publication_context(run);
    let mut value = result?;
    if !warnings.is_empty() {
        value
            .as_object_mut()
            .context("publication result is not an object")?
            .insert(
                "publication_warnings".into(),
                serde_json::to_value(warnings)?,
            );
    }
    Ok(value)
}
pub(crate) mod migration;
pub(crate) mod migration_v2;
pub(super) const NAMESPACE: &str = "afk-authority-v2";
pub(super) const PROFILE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Evidence {
    pub namespace: String,
    pub instance: String,
    pub version: u32,
    pub sha256: String,
    pub original: String,
    pub profiles: Vec<String>,
}

impl Evidence {
    pub(super) fn validate(&self, profile: &str, id: &str) -> Result<()> {
        ensure!(
            self.instance == id && self.profiles.iter().any(|p| p == profile),
            "evidence scope mismatch"
        );
        for profile in &self.profiles {
            scope(profile, Some(id))?;
        }
        let version = match self.namespace.as_str() {
            "afk" => 1,
            "afk-runtime-v2" => 2,
            "afk-runtime-v3" => 3,
            "afk-runtime-v4" => 4,
            _ => anyhow::bail!("unknown evidence namespace"),
        };
        ensure!(
            self.version == version
                && self.sha256.len() == 64
                && self.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid evidence version/hash"
        );
        let index = self
            .original
            .strip_prefix("afk-authority-v1/migration/original-")
            .and_then(|s| s.strip_suffix(".json"))
            .context("invalid private evidence reference")?;
        ensure!(
            index.len() == 4 && index.bytes().all(|b| b.is_ascii_digit()),
            "invalid private evidence reference"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Session {
    pub epoch: u64,
    pub policy: Option<Policy>,
    pub book: delegation::Book,
    pub evidence: Vec<Evidence>,
    pub preference: batch::state::Preference,
    pub exception_epoch: u64,
}
impl From<migration::frozen::Session> for Session {
    fn from(old: migration::frozen::Session) -> Self {
        Self {
            epoch: old.epoch,
            policy: old.policy,
            book: old.book,
            evidence: old.evidence,
            preference: batch::state::Preference::Unrecorded,
            exception_epoch: 0,
        }
    }
}
impl Session {
    pub fn empty() -> Self {
        migration::frozen::Session::empty().into()
    }
    pub fn legacy(&self) -> migration::frozen::Session {
        migration::frozen::Session {
            epoch: self.epoch,
            policy: self.policy.clone(),
            book: self.book.clone(),
            evidence: self.evidence.clone(),
        }
    }
    pub fn revoke(&mut self) -> Result<()> {
        self.epoch = self
            .epoch
            .checked_add(1)
            .context("control epoch overflow")?;
        self.book.end_all();
        Ok(())
    }
    pub fn preference(&mut self, preference: batch::state::Preference) -> Result<()> {
        self.exception_epoch = self
            .exception_epoch
            .checked_add(1)
            .context("exception epoch overflow")?;
        self.preference = preference;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Origin {
    pub profile: String,
    pub sha256: String,
    pub original: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub version: u32,
    pub profile: String,
    pub revision: u64,
    pub sessions: std::collections::BTreeMap<String, Session>,
    pub origin: Option<Origin>,
    pub batches: std::collections::BTreeMap<String, batch::state::Operation>,
}
impl Snapshot {
    pub fn empty(profile: &str) -> Self {
        Self {
            version: 2,
            profile: profile.into(),
            revision: 1,
            sessions: Default::default(),
            origin: None,
            batches: Default::default(),
        }
    }
    pub fn validate(&self, profile: &str) -> Result<()> {
        ensure!(
            self.version == 2 && self.profile == profile && self.revision > 0,
            "invalid v2 authority identity/revision"
        );
        ensure!(
            serde_json::to_vec(self)?.len() <= PROFILE_BYTES,
            "profile authority exceeds 32 MiB"
        );
        migration::frozen::Snapshot {
            version: 1,
            profile: self.profile.clone(),
            revision: self.revision,
            sessions: self
                .sessions
                .iter()
                .map(|(id, session)| (id.clone(), session.legacy()))
                .collect(),
        }
        .validate(profile)?;
        ensure!(
            self.batches.len() <= 64,
            "retained batch operation capacity exceeded"
        );
        let mut membership = std::collections::BTreeSet::new();
        for (id, batch) in &self.batches {
            ensure!(*id == batch.id, "batch map identity mismatch");
            batch.validate(profile, &self.sessions)?;
            for (session, member) in &batch.members {
                if let Some(window) = &member.window {
                    ensure!(
                        membership.insert((session, window)),
                        "window belongs to multiple batches"
                    );
                }
            }
        }
        if let Some(origin) = &self.origin {
            ensure!(
                origin.profile == profile
                    && origin.sha256.len() == 64
                    && origin.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid v1 authority provenance"
            );
            let index = origin
                .original
                .strip_prefix("afk-authority-v2/migration/original-")
                .and_then(|s| s.strip_suffix(".json"))
                .context("invalid v1 original reference")?;
            ensure!(
                index.len() == 4 && index.bytes().all(|b| b.is_ascii_digit()),
                "invalid v1 original index"
            );
        }
        Ok(())
    }
    pub fn transition(&self, next: &Self) -> Result<()> {
        ensure!(
            self.profile == next.profile && self.origin == next.origin,
            "profile provenance changed"
        );
        for (id, old) in &self.batches {
            old.transition(next.batches.get(id).context("batch operation removed")?)?;
        }
        for (id, old) in &self.sessions {
            let session = next.sessions.get(id).context("session authority removed")?;
            ensure!(
                session.epoch >= old.epoch && session.exception_epoch >= old.exception_epoch,
                "session epochs regressed"
            );
            ensure!(
                session.preference == old.preference
                    || session.exception_epoch > old.exception_epoch,
                "bulk preference changed without epoch"
            );
        }
        Ok(())
    }
}

pub(super) fn unparked(store: &Store) -> Result<()> {
    journal::clear(store)
}

pub(super) fn scope(profile: &str, id: Option<&str>) -> Result<()> {
    super::super::validate_profile_name(profile)?;
    if let Some(id) = id {
        super::super::validate_instance_id(id)?;
    }
    Ok(())
}

pub(super) fn profile_store(app: &Path, profile: &str, create: bool) -> Result<Store> {
    scope(profile, None)?;
    Store::open_namespace(
        app,
        profile,
        create,
        &format!("{NAMESPACE}/profiles"),
        PROFILE_BYTES,
    )
}

pub(super) fn existing_profile_store(app: &Path, profile: &str) -> Result<Option<Store>> {
    scope(profile, None)?;
    let mut dir = crate::session::AnchoredDir::open(app)?;
    for component in [NAMESPACE, "profiles", profile] {
        if dir.entry_stat(Path::new(component))?.is_none() {
            return Ok(None);
        }
        dir = dir.child(Path::new(component))?;
        store::private_dir(&dir)?;
    }
    Ok(Some(Store {
        dir,
        scope: None,
        limit: PROFILE_BYTES,
    }))
}

pub(super) fn ready(app: &Path) -> Result<()> {
    migration_v2::ready(app).context("AFK authority unavailable; run normal `aoe migrate` in this app namespace, then inspect migration evidence before reenrolling")
}

pub(super) fn validate_existing_snapshot(
    app: &Path,
    profile: &str,
    snapshot: Option<&Snapshot>,
) -> Result<()> {
    if let Some(snapshot) = snapshot {
        snapshot.validate(profile)?;
        migration_v2::validate_import(app, snapshot)
    } else {
        ensure!(
            !migration_v2::expected_profile(app, profile)?,
            "missing imported profile authority; run normal `aoe migrate` and inspect evidence"
        );
        Ok(())
    }
}

impl Store {
    pub(super) fn authority(app: &Path, profile: &str, id: &str, create: bool) -> Result<Self> {
        scope(profile, Some(id))?;
        ready(app)?;
        let existing = existing_profile_store(app, profile)?;
        let initialized = existing.is_some();
        ensure!(create || initialized, "missing profile authority");
        let mut store = match existing {
            Some(store) => store,
            None => profile_store(app, profile, true)?,
        };
        let _lock = if create {
            store.lock(TIMEOUT)?
        } else {
            store.lock_existing(TIMEOUT)?
        };
        journal::clear(&store)?;
        let prior = store.read::<Snapshot>("snapshot.json")?;
        let mut snapshot = match prior.clone() {
            Some(snapshot) => snapshot,
            None => {
                ensure!(
                    create && !initialized && !migration_v2::expected_profile(app, profile)?,
                    "missing imported profile authority"
                );
                Snapshot::empty(profile)
            }
        };
        snapshot.validate(profile)?;
        migration_v2::validate_import(app, &snapshot)?;
        if !snapshot.sessions.contains_key(id) {
            ensure!(
                create,
                "missing session authority; prepare the validated launch first"
            );
            snapshot.sessions.insert(id.into(), Session::empty());
            if prior.is_some() {
                snapshot.revision = snapshot
                    .revision
                    .checked_add(1)
                    .context("snapshot revision overflow")?;
            }
            snapshot.validate(profile)?;
            journal::publish(&store, prior.as_ref(), &snapshot)?.warn();
        }
        store.scope = Some((profile.into(), id.into()));
        Ok(store)
    }
    pub(super) fn transport(app: &Path, profile: &str, id: &str, create: bool) -> Result<Self> {
        scope(profile, Some(id))?;
        ready(app)?;
        Self::open_namespace(
            app,
            id,
            create,
            &format!("{NAMESPACE}/profiles/{profile}/transport"),
            MAX_BYTES,
        )
    }
    pub(super) fn authority_snapshot(&self) -> Result<Snapshot> {
        let (profile, _) = self.scope.as_ref().context("not an authority store")?;
        journal::clear(self)?;
        let snapshot: Snapshot = self
            .read_raw("snapshot.json")?
            .context("missing profile authority")?;
        snapshot.validate(profile)?;
        journal::clear(self)?;
        Ok(snapshot)
    }
    pub(super) fn session(&self) -> Result<Session> {
        let (_, id) = self.scope.as_ref().context("not an authority store")?;
        self.authority_snapshot()?
            .sessions
            .remove(id)
            .context("missing session authority")
    }
    // Caller holds the stable profile lock throughout read, effect commitment and publication.
    pub(super) fn publish_session(&self, session: Session) -> Result<()> {
        let (profile, id) = self.scope.as_ref().context("not an authority store")?;
        let prior = self.authority_snapshot()?;
        let mut snapshot = prior.clone();
        snapshot.sessions.insert(id.clone(), session);
        snapshot.revision = snapshot
            .revision
            .checked_add(1)
            .context("snapshot revision overflow")?;
        snapshot.validate(profile)?;
        journal::publish(self, Some(&prior), &snapshot)?.warn();
        Ok(())
    }
    pub(super) fn authority_read<T: serde::de::DeserializeOwned>(
        &self,
        leaf: &str,
    ) -> Result<Option<T>> {
        let session = self.session()?;
        let value = match leaf {
            "policy.json" => serde_json::to_value(session.policy)?,
            "ledger.json" => serde_json::to_value(session.book)?,
            _ => anyhow::bail!("transport leaf is not authority"),
        };
        if value == Value::Null {
            Ok(None)
        } else {
            Ok(Some(serde_json::from_value(value)?))
        }
    }
    pub(super) fn authority_write(&self, leaf: &str, value: &impl Serialize) -> Result<()> {
        let mut session = self.session()?;
        match leaf {
            "policy.json" => {
                session.policy = Some(serde_json::from_value(serde_json::to_value(value)?)?)
            }
            "ledger.json" => session.book = serde_json::from_value(serde_json::to_value(value)?)?,
            _ => anyhow::bail!("transport leaf is not authority"),
        }
        self.publish_session(session)
    }
}

#[cfg(test)]
impl Store {
    pub(super) fn corrupt_authority_for_test(
        &self,
        leaf: &str,
        value: &impl Serialize,
    ) -> Result<()> {
        let (_, id) = self.scope.as_ref().context("not authority")?;
        let mut snapshot: Value = self.read_raw("snapshot.json")?.unwrap();
        snapshot["sessions"][id][match leaf {
            "policy.json" => "policy",
            _ => "book",
        }] = serde_json::to_value(value)?;
        self.write_raw("snapshot.json", &snapshot)
    }
}

#[cfg(test)]
mod tests;
