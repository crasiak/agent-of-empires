//! Profile-scoped publication. Transport files never supply authority.
use super::*;
use delegation::Book;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) mod migration;
pub(super) const NAMESPACE: &str = "afk-authority-v1";
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
    pub book: Book,
    pub evidence: Vec<Evidence>,
}
impl Session {
    pub fn empty() -> Self {
        Self {
            epoch: 0,
            policy: None,
            book: Book::empty(),
            evidence: vec![],
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
}

// Migration 40 hashes this serialized shape; a new schema needs a separate historical decoder.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub version: u32,
    pub profile: String,
    pub revision: u64,
    pub sessions: BTreeMap<String, Session>,
}
impl Snapshot {
    pub fn empty(profile: &str) -> Self {
        Self {
            version: 1,
            profile: profile.into(),
            revision: 1,
            sessions: BTreeMap::new(),
        }
    }
    pub fn validate(&self, profile: &str) -> Result<()> {
        scope(profile, None)?;
        ensure!(
            self.version == 1 && self.profile == profile && self.revision > 0,
            "invalid profile authority identity/revision"
        );
        ensure!(
            serde_json::to_vec(self)?.len() <= PROFILE_BYTES,
            "profile authority exceeds 32 MiB"
        );
        for (id, session) in &self.sessions {
            scope(profile, Some(id))?;
            if let Some(policy) = &session.policy {
                policy.validate()?;
                ensure!(
                    policy.binding.profile == profile && policy.binding.instance_id == *id,
                    "policy scope mismatch"
                );
                ensure!(
                    serde_json::to_vec(policy)?.len() <= MAX_BYTES,
                    "oversized policy"
                );
            }
            session.book.validate_scope(profile, id)?;
            for evidence in &session.evidence {
                evidence.validate(profile, id)?;
            }
        }
        Ok(())
    }
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

pub(super) fn ready(app: &Path) -> Result<()> {
    migration::ready(app).context("AFK authority unavailable; run normal `aoe migrate` in this app namespace, then inspect migration evidence before reenrolling")
}

pub(super) fn validate_existing_snapshot(
    app: &Path,
    profile: &str,
    snapshot: Option<&Snapshot>,
) -> Result<()> {
    if let Some(snapshot) = snapshot {
        snapshot.validate(profile)?;
        migration::validate_import(app, snapshot)
    } else {
        ensure!(
            !migration::expected_profile(app, profile)?,
            "missing imported profile authority; run normal `aoe migrate` and inspect evidence"
        );
        Ok(())
    }
}

impl Store {
    pub(super) fn authority(app: &Path, profile: &str, id: &str, create: bool) -> Result<Self> {
        scope(profile, Some(id))?;
        ready(app)?;
        let mut store = profile_store(app, profile, create)?;
        let _lock = if create {
            Some(store.lock(TIMEOUT)?)
        } else {
            None
        };
        let mut snapshot = match store.read::<Snapshot>("snapshot.json")? {
            Some(snapshot) => snapshot,
            None => {
                ensure!(
                    create && !migration::expected_profile(app, profile)?,
                    "missing imported profile authority"
                );
                Snapshot::empty(profile)
            }
        };
        snapshot.validate(profile)?;
        migration::validate_import(app, &snapshot)?;
        if !snapshot.sessions.contains_key(id) {
            ensure!(
                create,
                "missing session authority; prepare the validated launch first"
            );
            snapshot.sessions.insert(id.into(), Session::empty());
            snapshot.revision = snapshot
                .revision
                .checked_add(1)
                .context("snapshot revision overflow")?;
            snapshot.validate(profile)?;
            store.write("snapshot.json", &snapshot)?;
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
        let snapshot: Snapshot = self
            .read_raw("snapshot.json")?
            .context("missing profile authority")?;
        snapshot.validate(profile)?;
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
        let mut snapshot = self.authority_snapshot()?;
        snapshot.sessions.insert(id.clone(), session);
        snapshot.revision = snapshot
            .revision
            .checked_add(1)
            .context("snapshot revision overflow")?;
        snapshot.validate(profile)?;
        self.write_raw("snapshot.json", &snapshot)
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
