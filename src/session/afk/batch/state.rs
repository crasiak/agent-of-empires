use super::super::*;
use delegation::{hash, questions::QuestionIdentity, settlement::Continuation};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::session::afk) enum Preference {
    Unrecorded,
    Excluded,
    Allowed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::session::afk) enum Outcome {
    Skipped,
    Pending,
    Confirmed,
    Failed,
    Stopped,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Member {
    pub binding: Option<Binding>,
    pub generation: Option<String>,
    pub window: Option<String>,
    pub grant_hash: String,
    pub control_epoch: u64,
    pub exception_epoch: u64,
    pub outcome: Outcome,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "identity",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(in crate::session::afk) enum Origin {
    Request(String),
    Question(QuestionIdentity),
    Nudge(Continuation),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Ticket {
    pub session: String,
    pub window: String,
    pub origin: Origin,
    pub request: Option<String>,
    pub charged_at_ms: i64,
}
pub(in crate::session::afk) use authority::migration_v2::frozen::Operation;
#[derive(Debug)]
pub(in crate::session::afk) struct SharedExhausted;
impl fmt::Display for SharedExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "shared request admission exhausted; existing held and reserved rights are unchanged",
        )
    }
}
impl std::error::Error for SharedExhausted {}

pub(in crate::session::afk) struct Facts {
    pub binding: Binding,
    pub generation: String,
    pub grant_hash: String,
    pub expires_at_ms: i64,
    pub confirmed: bool,
    pub ended: bool,
    pub requests: usize,
    pub reservations: Vec<String>,
    pub admissions: Vec<(Origin, Option<String>)>,
}
fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
impl Operation {
    pub fn validate(
        &self,
        profile: &str,
        sessions: &BTreeMap<String, authority::Session>,
    ) -> Result<()> {
        ensure!(
            uuid(&self.id) && digest(&self.preview_digest),
            "invalid batch identity/digest"
        );
        ensure!(
            self.issued_at_ms > 0
                && self
                    .expires_at_ms
                    .checked_sub(self.issued_at_ms)
                    .is_some_and(|d| (60_000..=86_400_000).contains(&d)),
            "invalid batch lifetime"
        );
        ensure!(
            (1..=16).contains(&self.members.len()),
            "batch membership exceeds bounds"
        );
        ensure!(
            self.shared_requests.is_none_or(|n| (1..=128).contains(&n)),
            "invalid shared allowance"
        );
        ensure!(
            self.shared_requests
                .map_or(self.tickets.is_empty(), |n| self.tickets.len()
                    <= usize::from(n)),
            "invalid shared ticket count"
        );
        let mut origins = BTreeSet::new();
        let mut requests = BTreeSet::new();
        for ticket in &self.tickets {
            ensure!(
                ticket.charged_at_ms >= self.issued_at_ms
                    && ticket.charged_at_ms < self.expires_at_ms,
                "ticket outside batch lifetime"
            );
            let member = self
                .members
                .get(&ticket.session)
                .context("ticket outside membership")?;
            ensure!(
                member.window.as_deref() == Some(ticket.window.as_str()),
                "ticket window mismatch"
            );
            let key = (
                ticket.session.clone(),
                ticket.window.clone(),
                hash(serde_json::to_vec(&ticket.origin)?),
            );
            ensure!(origins.insert(key), "duplicate ticket admission");
            if let Some(request) = &ticket.request {
                ensure!(
                    uuid(request)
                        && requests.insert((
                            ticket.session.clone(),
                            ticket.window.clone(),
                            request.clone()
                        )),
                    "duplicate/invalid ticket request"
                );
            }
        }
        for (id, member) in &self.members {
            authority::scope(profile, Some(id))?;
            ensure!(
                digest(&member.grant_hash) && member.reason.as_ref().is_none_or(|s| s.len() <= 512),
                "invalid batch member metadata"
            );
            let Some(window) = &member.window else {
                ensure!(
                    member.outcome == Outcome::Skipped,
                    "uncreated member is not skipped"
                );
                continue;
            };
            ensure!(
                uuid(window) && member.outcome != Outcome::Skipped,
                "invalid created batch member"
            );
            let session = sessions.get(id).context("missing batch session")?;
            ensure!(
                session.epoch >= member.control_epoch
                    && session.exception_epoch >= member.exception_epoch,
                "batch epochs regressed"
            );
            let facts = session
                .book
                .batch_facts(window)
                .context("missing original batch window")?;
            ensure!(
                member.binding.as_ref() == Some(&facts.binding)
                    && member.generation.as_ref() == Some(&facts.generation)
                    && member.grant_hash == facts.grant_hash,
                "batch/window qualification mismatch"
            );
            ensure!(
                facts.expires_at_ms <= self.expires_at_ms,
                "member outlives batch"
            );
            if self.stopped || session.preference == Preference::Excluded {
                ensure!(facts.ended, "revoked batch member still live");
            }
            match member.outcome {
                Outcome::Pending => ensure!(!facts.confirmed, "pending member is confirmed"),
                Outcome::Confirmed => {
                    ensure!(
                        facts.confirmed || facts.ended,
                        "live confirmed member lacks confirmation"
                    )
                }
                Outcome::Failed | Outcome::Stopped => {
                    ensure!(facts.ended, "terminal member still live")
                }
                Outcome::Skipped => unreachable!(),
            }
            if self.shared_requests.is_none() {
                continue;
            }
            let tickets: Vec<_> = self
                .tickets
                .iter()
                .filter(|t| t.session == *id && t.window == *window)
                .collect();
            let held = tickets.iter().filter(|t| t.request.is_none()).count();
            ensure!(
                facts.reservations.len() + held <= facts.requests,
                "member reservations plus held tickets exceed local grant"
            );
            for (origin, observed) in &facts.admissions {
                let ticket = tickets
                    .iter()
                    .find(|t| t.origin == *origin)
                    .context("admission lacks shared ticket")?;
                ensure!(
                    &ticket.request == observed,
                    "ticket/admission observation mismatch"
                );
            }
            for request in &facts.reservations {
                ensure!(
                    tickets
                        .iter()
                        .filter(|t| t.request.as_ref() == Some(request))
                        .count()
                        == 1,
                    "reservation lacks exact shared ticket"
                );
            }
            for ticket in tickets {
                match &ticket.origin {
                    Origin::Request(request) => ensure!(
                        ticket.request.as_ref() == Some(request)
                            && facts.reservations.contains(request)
                            && !facts
                                .admissions
                                .iter()
                                .any(|(_, r)| r.as_ref() == Some(request)),
                        "ordinary ticket does not match reservation"
                    ),
                    _ => ensure!(
                        facts
                            .admissions
                            .iter()
                            .any(|(o, r)| *o == ticket.origin && *r == ticket.request),
                        "ticket lacks matching admission"
                    ),
                }
            }
        }
        Ok(())
    }
    pub fn transition(&self, next: &Self) -> Result<()> {
        ensure!(
            self.id == next.id
                && self.preview_digest == next.preview_digest
                && self.issued_at_ms == next.issued_at_ms
                && self.expires_at_ms == next.expires_at_ms
                && self.shared_requests == next.shared_requests
                && self.members.len() == next.members.len(),
            "immutable batch policy changed"
        );
        ensure!(!self.stopped || next.stopped, "stopped batch reopened");
        for (id, old) in &self.members {
            let member = next.members.get(id).context("batch member removed")?;
            ensure!(
                old.binding == member.binding
                    && old.generation == member.generation
                    && old.window == member.window
                    && old.grant_hash == member.grant_hash
                    && old.control_epoch == member.control_epoch
                    && old.exception_epoch == member.exception_epoch,
                "frozen membership changed"
            );
            ensure!(
                match old.outcome {
                    Outcome::Skipped | Outcome::Failed | Outcome::Stopped => old == member,
                    Outcome::Confirmed =>
                        matches!(member.outcome, Outcome::Confirmed | Outcome::Stopped),
                    Outcome::Pending => member.outcome != Outcome::Skipped,
                },
                "terminal batch member re-enrolled"
            );
        }
        ensure!(
            next.tickets.len() >= self.tickets.len(),
            "shared tickets refunded"
        );
        for (old, ticket) in self.tickets.iter().zip(&next.tickets) {
            ensure!(
                old.session == ticket.session
                    && old.window == ticket.window
                    && old.origin == ticket.origin
                    && old.charged_at_ms == ticket.charged_at_ms
                    && (old.request.is_none() || old.request == ticket.request),
                "shared ticket changed or rebound"
            );
        }
        if next.stopped {
            ensure!(
                self.tickets == next.tickets,
                "stopped batch changed shared consumption"
            );
        }
        Ok(())
    }
    pub fn charge(
        &mut self,
        session: &str,
        window: &str,
        origin: Origin,
        request: Option<String>,
        now: i64,
    ) -> Result<()> {
        ensure!(
            !self.stopped && now < self.expires_at_ms && now >= self.issued_at_ms,
            "batch ended or expired"
        );
        if self.shared_requests.is_none() {
            return Ok(());
        }
        if let Some(ticket) = self
            .tickets
            .iter_mut()
            .find(|t| t.session == session && t.window == window && t.origin == origin)
        {
            if let Some(request) = request {
                ensure!(ticket.request.is_none(), "held ticket already bound");
                ticket.request = Some(request);
            }
            return Ok(());
        }
        ensure!(
            self.tickets.len() < usize::from(self.shared_requests.unwrap()),
            SharedExhausted
        );
        self.tickets.push(Ticket {
            session: session.into(),
            window: window.into(),
            origin,
            request,
            charged_at_ms: now,
        });
        Ok(())
    }
}
