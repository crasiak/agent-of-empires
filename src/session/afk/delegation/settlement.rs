use super::*;

const CHECKPOINT_BYTES: usize = 2048;
const CHECKPOINTS: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::session::afk) enum TerminalReason {
    Completed,
    Deferred,
    Exhausted,
    Interrupted,
    Failed,
    DeliveryUnknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Progress {
    Unfinished,
    Completed,
    Blocked,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum NextStep {
    Read { path: String },
    Record { path: String },
    Apply { decision: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct CheckpointClaims {
    status: Progress,
    next_step: Option<NextStep>,
    rationale: String,
    evidence: Vec<String>,
    depends_on: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Checkpoint {
    sequence: usize,
    request: u8,
    recorded_at_ms: i64,
    claims: Option<CheckpointClaims>,
    facts: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct ReadFact {
    path: String,
    hash: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Continuation {
    window: String,
    generation: String,
    revision: u64,
    checkpoint: usize,
    admission: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Nudge {
    identity: Continuation,
    facts: String,
    admitted_at_ms: i64,
    contribution_intent_at_ms: Option<i64>,
    observed_request: Option<String>,
}

impl Window {
    pub(super) fn prune_checkpoint_bodies(&mut self) {
        for checkpoint in &mut self.checkpoints {
            checkpoint.claims = None;
        }
    }
    pub(super) fn unresolved_nudge(&self) -> bool {
        self.nudges.iter().any(|n| n.observed_request.is_none())
    }
    pub(in crate::session::afk) fn end(&mut self, reason: TerminalReason) {
        self.state = "ended".into();
        if self.terminal_reason.is_none() {
            self.terminal_reason = Some(if self.unresolved_nudge() {
                TerminalReason::DeliveryUnknown
            } else {
                reason
            });
        }
    }
    pub(in crate::session::afk) fn read_fact(
        &mut self,
        path: String,
        body: &Option<String>,
    ) -> String {
        let fact = ReadFact {
            path,
            hash: body.as_ref().map(hash),
        };
        let key = hash(serde_json::to_vec(&fact).unwrap());
        self.read_facts.retain(|f| f.path != fact.path);
        self.read_facts.push(fact);
        self.read_facts.sort_by(|a, b| a.path.cmp(&b.path));
        key
    }
    pub(in crate::session::afk) fn evidence(&self) -> Vec<String> {
        let mut keys: Vec<_> = self
            .read_facts
            .iter()
            .map(|f| hash(serde_json::to_vec(f).unwrap()))
            .collect();
        if let Some(p) = &self.permit {
            keys.push(hash(serde_json::to_vec(&json!({"path":p.path,"expected_hash":p.expected_hash,"payload_hash":p.payload_hash,"outcomes":p.outcomes})).unwrap()));
        }
        keys.sort();
        keys
    }
    fn facts(&self) -> String {
        hash(serde_json::to_vec(&self.evidence()).unwrap())
    }
    pub(super) fn checkpoint_retry(
        &self,
        request: &str,
        claims: &CheckpointClaims,
    ) -> Option<Value> {
        self.checkpoints
            .last()
            .filter(|c| {
                self.reservations
                    .get(usize::from(c.request) - 1)
                    .is_some_and(|r| r == request)
                    && c.claims.as_ref() == Some(claims)
                    && c.facts == self.facts()
            })
            .map(|c| json!(c))
    }
    pub(in crate::session::afk) fn checkpoint(
        &mut self,
        request: &str,
        claims: CheckpointClaims,
        now: i64,
    ) -> Result<Value> {
        let number = self.request(request)?;
        bounded(&claims, CHECKPOINT_BYTES)?;
        private_text(&serde_json::to_value(&claims)?)?;
        ensure!(
            !claims.rationale.is_empty(),
            "checkpoint rationale required"
        );
        let evidence = self.evidence();
        ensure!(
            claims.evidence.iter().all(|key| evidence.contains(key)),
            "unobserved checkpoint evidence"
        );
        let facts = self.facts();
        if let Some(c) = self.checkpoints.last() {
            if c.claims.as_ref() == Some(&claims) && c.facts == facts {
                return Ok(json!(c));
            }
        }
        ensure!(self.checkpoints.len() < CHECKPOINTS, "checkpoint limit");
        let terminal = match claims.status {
            Progress::Completed => Some(TerminalReason::Completed),
            Progress::Blocked => Some(TerminalReason::Deferred),
            Progress::Unfinished if !claims.depends_on.is_empty() => Some(TerminalReason::Deferred),
            _ => None,
        };
        let c = Checkpoint {
            sequence: self.checkpoints.len() + 1,
            request: number,
            recorded_at_ms: now,
            claims: Some(claims),
            facts,
        };
        let result = json!(c);
        self.checkpoints.push(c);
        if let Some(reason) = terminal {
            self.end(reason);
        }
        Ok(result)
    }
    fn viable(&self, c: &Checkpoint) -> Result<()> {
        let claims = c.claims.as_ref().context("checkpoint body pruned")?;
        ensure!(
            claims.status == Progress::Unfinished && claims.depends_on.is_empty(),
            "completed or blocked checkpoint"
        );
        ensure!(
            !claims.evidence.is_empty() && c.facts == self.facts(),
            "missing or stale checkpoint evidence"
        );
        ensure!(
            !self
                .records
                .iter()
                .any(|r| r.disposition == "human_required"),
            "known human prerequisite"
        );
        for fact in &self.read_facts {
            let (_, _, body, _) = self.file(&fact.path)?;
            ensure!(
                body.as_ref().map(hash) == fact.hash,
                "observed target changed"
            );
        }
        match claims.next_step.as_ref().context("missing next step")? {
            NextStep::Read { path } => {
                ensure!(self.reads < READ_LIMIT, "read operations exhausted");
                self.file(path)?;
            }
            NextStep::Record { path } => {
                ensure!(
                    self.permit.is_none() && self.records.len() < 4,
                    "no further decision permit"
                );
                let (_, _, body, _) = self.file(path)?;
                let capability = if body.is_some() {
                    Capability::Replace
                } else {
                    Capability::Create
                };
                ensure!(
                    self.grant
                        .files
                        .iter()
                        .any(|f| f.path == *path && f.capability == capability),
                    "next action outside grant"
                );
            }
            NextStep::Apply { decision } => {
                let p = self.permit.as_ref().context("no admitted action")?;
                ensure!(
                    p.decision == *decision && p.attempted_at_ms.is_none(),
                    "action unavailable"
                );
                let (_, _, body, _) = self.file(&p.path)?;
                ensure!(
                    body.as_ref().map(hash) == p.expected_hash,
                    "action target changed"
                );
            }
        }
        Ok(())
    }
    pub(in crate::session::afk) fn admit_nudge(
        &mut self,
        request: &str,
        checkpoint: usize,
        now: i64,
    ) -> Result<Value> {
        self.request(request)?;
        let c = self.checkpoints.last().context("missing checkpoint")?;
        ensure!(c.sequence == checkpoint, "stale checkpoint revision");
        self.viable(c)?;
        if let Some(n) = self
            .nudges
            .iter()
            .find(|n| n.identity.checkpoint == checkpoint)
        {
            return Ok(json!({"identity":n.identity}));
        }
        ensure!(
            self.reservations.len() < usize::from(self.grant.requests),
            "requests exhausted"
        );
        ensure!(
            self.nudges.len() < usize::from(self.grant.settlement_nudges),
            "nudges exhausted"
        );
        ensure!(
            !self
                .nudges
                .iter()
                .any(|n| n.facts == c.facts || n.observed_request.is_none()),
            "repeated progress or unresolved admission"
        );
        let identity = Continuation {
            window: self.id.clone(),
            generation: self.generation.clone(),
            revision: self.revision,
            checkpoint,
            admission: uuid::Uuid::new_v4().to_string(),
        };
        self.nudges.push(Nudge {
            identity: identity.clone(),
            facts: c.facts.clone(),
            admitted_at_ms: now,
            contribution_intent_at_ms: None,
            observed_request: None,
        });
        Ok(json!({"identity":identity}))
    }
    pub(in crate::session::afk) fn contribution_intent(
        &mut self,
        identity: &Continuation,
        now: i64,
    ) -> Result<Value> {
        let c = self.checkpoints.last().context("missing checkpoint")?;
        ensure!(c.sequence == identity.checkpoint, "changed checkpoint");
        self.viable(c)?;
        let n = self.nudges.last_mut().context("no nudge admission")?;
        ensure!(
            n.identity == *identity && n.observed_request.is_none(),
            "invalid contribution intent"
        );
        n.contribution_intent_at_ms.get_or_insert(now);
        Ok(json!({"identity":identity}))
    }
    pub(in crate::session::afk) fn observe_nudge(
        &mut self,
        identity: Option<&Continuation>,
        request: &str,
    ) -> Result<()> {
        if identity.is_some() {
            self.viable(self.checkpoints.last().context("missing checkpoint")?)?;
        }
        if let Some(n) = self
            .nudges
            .last_mut()
            .filter(|n| n.observed_request.is_none())
        {
            ensure!(
                Some(&n.identity) == identity && n.contribution_intent_at_ms.is_some(),
                "unmatched continuation"
            );
            n.observed_request = Some(request.into());
        } else {
            ensure!(
                identity.is_none(),
                "continuation already observed or unknown"
            );
        }
        Ok(())
    }
    pub(in crate::session::afk) fn settlement_reason(&self) -> TerminalReason {
        let claims = self.checkpoints.last().and_then(|c| c.claims.as_ref());
        match claims.map(|c| &c.status) {
            Some(Progress::Completed) => TerminalReason::Completed,
            Some(Progress::Blocked) | None => TerminalReason::Deferred,
            _ if self.reservations.len() == usize::from(self.grant.requests)
                || self.nudges.len() == usize::from(self.grant.settlement_nudges)
                || (self.reads == READ_LIMIT
                    && matches!(
                        claims.and_then(|c| c.next_step.as_ref()),
                        Some(NextStep::Read { .. })
                    )) =>
            {
                TerminalReason::Exhausted
            }
            _ => TerminalReason::Deferred,
        }
    }
    pub(in crate::session::afk) fn validate_settlement(&self) -> Result<()> {
        ensure!(
            self.checkpoints.len() <= CHECKPOINTS
                && self.read_facts.len() <= usize::from(READ_LIMIT)
                && self.nudges.len() <= usize::from(self.grant.settlement_nudges),
            "corrupt settlement counters"
        );
        ensure!(
            (self.state == "ended") == self.terminal_reason.is_some(),
            "missing terminal reason"
        );
        for (index, c) in self.checkpoints.iter().enumerate() {
            bounded(&c.claims, CHECKPOINT_BYTES)?;
            ensure!(
                self.state == "ended" || c.claims.is_some(),
                "live checkpoint body missing"
            );
            ensure!(
                c.sequence == index + 1
                    && c.request > 0
                    && usize::from(c.request) <= self.reservations.len(),
                "corrupt checkpoint identity"
            );
        }
        for (index, n) in self.nudges.iter().enumerate() {
            ensure!(
                n.identity.window == self.id
                    && n.identity.generation == self.generation
                    && n.identity.revision == self.revision
                    && uuid::Uuid::parse_str(&n.identity.admission).is_ok(),
                "corrupt nudge identity"
            );
            ensure!(
                self.checkpoints
                    .iter()
                    .any(|c| c.sequence == n.identity.checkpoint && c.facts == n.facts)
                    && !self.nudges[..index].iter().any(|old| old.facts == n.facts
                        || old.identity.admission == n.identity.admission),
                "corrupt nudge progress"
            );
            ensure!(
                n.observed_request.as_ref().is_none_or(
                    |r| n.contribution_intent_at_ms.is_some() && self.reservations.contains(r)
                ),
                "corrupt continuation observation"
            );
        }
        Ok(())
    }
}

pub(in crate::session::afk) fn reconcile(
    store: &Store,
    binding: &Binding,
    generation: &str,
    now: i64,
) -> Result<Value> {
    let _lock = store.lock(TIMEOUT)?;
    let mut book = Book::load(store)?;
    if let Some(w) = book.windows.last_mut() {
        if w.state != "ended"
            && (w.binding != *binding || w.generation != generation || now >= w.expires_at_ms)
        {
            w.end(TerminalReason::Interrupted);
            book.save(store)?;
        }
    }
    Ok(book.windows.last().map_or(Value::Null, |w| w.view(now)))
}
