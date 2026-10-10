//! Frozen migration-40 field order, defaults, revocation serialization and namespace.
use super::super::super::{delegation::Book, Policy, Store, MAX_BYTES};
use super::super::{scope, Evidence};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub(in crate::session::afk::authority) const NAMESPACE: &str = "afk-authority-v1";
pub(in crate::session::afk::authority) const PROFILE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Session {
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
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Snapshot {
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

pub(in crate::session::afk::authority) fn profile_store(
    app: &Path,
    profile: &str,
    create: bool,
) -> Result<Store> {
    scope(profile, None)?;
    Store::open_namespace(
        app,
        profile,
        create,
        &format!("{NAMESPACE}/profiles"),
        PROFILE_BYTES,
    )
}
