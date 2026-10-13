use super::*;
use crate::session::afk::delegation::Book;
use std::fs::File;

pub(in crate::session::afk) struct SessionTransaction<'a> {
    store: &'a Store,
    _lock: File,
    snapshot: Snapshot,
    session: Option<String>,
}
impl<'a> SessionTransaction<'a> {
    pub fn open(store: &'a Store) -> Result<Self> {
        let (_, session) = store
            .scope
            .as_ref()
            .context("transaction requires scoped authority")?;
        let lock = store.lock_existing(TIMEOUT)?;
        let snapshot = store.authority_snapshot()?;
        ensure!(
            snapshot.sessions.contains_key(session),
            "missing transaction session"
        );
        Ok(Self {
            store,
            _lock: lock,
            snapshot,
            session: Some(session.clone()),
        })
    }
    pub fn profile(store: &'a Store, profile: &str) -> Result<Self> {
        let lock = store.lock_existing(TIMEOUT)?;
        journal::clear(store)?;
        let snapshot: Snapshot = store
            .read_raw("snapshot.json")?
            .context("missing profile authority")?;
        snapshot.validate(profile)?;
        Ok(Self {
            store,
            _lock: lock,
            snapshot,
            session: None,
        })
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn book(&self) -> Result<&Book> {
        Ok(&self.snapshot.sessions[self
            .session
            .as_ref()
            .context("transaction has no selected session")?]
        .book)
    }
    pub fn commit_admission(
        &mut self,
        book: Book,
        admission: delegation::Admission,
        now: i64,
    ) -> Result<journal::Committed> {
        let mut proposal = self.snapshot.clone();
        let session = self
            .session
            .as_ref()
            .context("transaction has no selected session")?;
        let (window, origin, request) = admission.ticket(&book)?;
        if let Some(operation) = proposal
            .batches
            .values_mut()
            .find(|op| op.members.get(session).and_then(|m| m.window.as_ref()) == Some(&window))
        {
            ensure!(
                operation.members[session].outcome == batch::state::Outcome::Confirmed,
                "batch member not confirmed"
            );
            if operation.shared_requests.is_some()
                && request.is_some()
                && !matches!(origin, batch::state::Origin::Request(_))
            {
                ensure!(
                    operation
                        .tickets
                        .iter()
                        .any(|t| t.session == *session && t.window == window && t.origin == origin),
                    "follow-up lacks its original held ticket"
                );
            }
            operation.charge(session, &window, origin, request, now)?;
        }
        proposal
            .sessions
            .get_mut(session)
            .context("missing transaction session")?
            .book = book;
        self.publish(proposal)
    }
    pub fn deny_usage(&mut self, id: &str, denial: usage::Decision) -> Result<journal::Committed> {
        let mut proposal = self.snapshot.clone();
        usage::latch(&mut proposal, id, &denial)?;
        if proposal.usage == self.snapshot.usage
            && proposal
                .sessions
                .iter()
                .all(|(id, s)| s.epoch == self.snapshot.sessions[id].epoch)
        {
            return Ok(journal::Committed {
                cleanup_durable: true,
            });
        }
        self.publish(proposal)
    }
    pub fn publish(&mut self, mut proposal: Snapshot) -> Result<journal::Committed> {
        proposal.usage.sync_lifetime(&mut proposal.sessions)?;
        proposal.revision = self
            .snapshot
            .revision
            .checked_add(1)
            .context("profile revision overflow")?;
        let committed = journal::publish(self.store, Some(&self.snapshot), &proposal)?;
        self.snapshot = proposal;
        Ok(committed)
    }
    pub fn commit_book(&mut self, book: Book) -> Result<journal::Committed> {
        let mut proposal = self.snapshot.clone();
        proposal
            .sessions
            .get_mut(
                self.session
                    .as_ref()
                    .context("transaction has no selected session")?,
            )
            .context("missing transaction session")?
            .book = book;
        self.publish(proposal)
    }
}
