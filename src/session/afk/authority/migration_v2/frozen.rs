//! Migration-41 field order, namespace and import serialization.
use super::super::{migration, scope, Evidence, Origin, Store, PROFILE_BYTES};
use crate::session::afk::{
    batch::state::{Member, Preference, Ticket},
    delegation::Book,
    Policy,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(in crate::session::afk) const NAMESPACE: &str = "afk-authority-v2";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Session {
    pub epoch: u64,
    pub policy: Option<Policy>,
    pub book: Book,
    pub evidence: Vec<Evidence>,
    pub preference: Preference,
    pub exception_epoch: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Operation {
    pub id: String,
    pub preview_digest: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub shared_requests: Option<u16>,
    pub stopped: bool,
    pub members: BTreeMap<String, Member>,
    pub tickets: Vec<Ticket>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Snapshot {
    pub version: u32,
    pub profile: String,
    pub revision: u64,
    pub sessions: BTreeMap<String, Session>,
    pub origin: Option<Origin>,
    pub batches: BTreeMap<String, Operation>,
}
impl Snapshot {
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
                .map(|(id, s)| (id.clone(), s.legacy()))
                .collect(),
        }
        .validate(profile)?;
        ensure!(
            self.batches.len() <= 64,
            "retained batch operation capacity exceeded"
        );
        let mut membership = BTreeSet::new();
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
