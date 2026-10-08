use super::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct QuestionIdentity {
    operation: String,
    tool_call_id: String,
    interaction_id: String,
    kind: String,
    arguments_hash: String,
    context_ref: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Question {
    identity: QuestionIdentity,
    admitted_at_ms: i64,
    emission_intent_at_ms: Option<i64>,
    outcome: Option<QuestionOutcome>,
    outcome_at_ms: Option<i64>,
    observed_request: Option<String>,
    recovered_at_ms: Option<i64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::session::afk) enum QuestionOutcome {
    Accepted,
    Ordinary,
    Unknown,
}
impl QuestionIdentity {
    fn validate(&self) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.operation).is_ok()
                && uuid::Uuid::parse_str(&self.interaction_id).is_ok()
                && matches!(self.kind.as_str(), "input" | "select" | "custom-overlay")
                && self.arguments_hash.len() == 64
                && self.arguments_hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid question identity"
        );
        for id in [&self.tool_call_id, &self.context_ref] {
            ensure!(
                !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control),
                "invalid question reference"
            );
        }
        Ok(())
    }
}
impl Question {
    fn unresolved(&self) -> bool {
        self.outcome != Some(QuestionOutcome::Ordinary)
            && self.observed_request.is_none()
            && self.recovered_at_ms.is_none()
    }
}
impl Window {
    pub(super) fn unresolved_question(&self) -> bool {
        self.question.as_ref().is_some_and(Question::unresolved)
    }
    pub(super) fn validate_question(&self) -> Result<()> {
        if let Some(q) = &self.question {
            q.identity.validate()?;
            ensure!(
                self.grant.question_deferrals == 1
                    && q.admitted_at_ms >= self.issued_at_ms
                    && q.admitted_at_ms < self.expires_at_ms
                    && q.outcome.is_some() == q.outcome_at_ms.is_some()
                    && (q.outcome.is_none() || q.emission_intent_at_ms.is_some())
                    && q.observed_request
                        .as_ref()
                        .is_none_or(|r| self.reservations.contains(r)
                            && q.outcome == Some(QuestionOutcome::Accepted)),
                "corrupt question lifecycle"
            );
        }
        Ok(())
    }
    fn question_mut(&mut self, identity: &QuestionIdentity) -> Result<&mut Question> {
        let q = self.question.as_mut().context("question not admitted")?;
        ensure!(q.identity == *identity, "conflicting question identity");
        Ok(q)
    }
    pub(super) fn admit_question(&mut self, identity: QuestionIdentity, now: i64) -> Result<Value> {
        identity.validate()?;
        if let Some(q) = &self.question {
            ensure!(
                q.identity == identity,
                "question attempt consumed/conflicting identity"
            );
            return Ok(json!({"identity":q.identity,"admitted_at_ms":q.admitted_at_ms}));
        }
        ensure!(
            self.state == "pending"
                && self.grant.question_deferrals == 1
                && self.reservations.len() < usize::from(self.grant.requests),
            "question deferral not granted or request allowance exhausted"
        );
        self.question = Some(Question {
            identity: identity.clone(),
            admitted_at_ms: now,
            emission_intent_at_ms: None,
            outcome: None,
            outcome_at_ms: None,
            observed_request: None,
            recovered_at_ms: None,
        });
        Ok(json!({"identity":identity,"admitted_at_ms":now}))
    }
    pub(super) fn question_intent(
        &mut self,
        identity: &QuestionIdentity,
        now: i64,
    ) -> Result<Value> {
        let q = self.question_mut(identity)?;
        ensure!(
            q.emission_intent_at_ms.is_none() && q.unresolved(),
            "question intent cannot be replayed"
        );
        q.emission_intent_at_ms = Some(now);
        Ok(json!({"identity":identity,"guarded_return":true}))
    }
    pub(super) fn question_outcome(
        &mut self,
        identity: &QuestionIdentity,
        outcome: QuestionOutcome,
        now: i64,
    ) -> Result<Value> {
        let q = self.question_mut(identity)?;
        ensure!(q.emission_intent_at_ms.is_some(), "question not emitted");
        ensure!(
            q.outcome.is_none() || q.outcome == Some(outcome),
            "contradictory question outcome"
        );
        if q.outcome.is_none() {
            q.outcome = Some(outcome);
            q.outcome_at_ms = Some(now);
        }
        Ok(json!({"identity":identity,"outcome":outcome}))
    }
    pub(super) fn observe_question(
        &mut self,
        identity: Option<&QuestionIdentity>,
        request: &str,
    ) -> Result<()> {
        let Some(q) = &mut self.question else {
            ensure!(identity.is_none(), "unknown question return");
            return Ok(());
        };
        if q.observed_request.is_some() {
            ensure!(identity.is_none(), "question return already observed");
            return Ok(());
        }
        ensure!(
            identity == Some(&q.identity)
                && q.outcome == Some(QuestionOutcome::Accepted)
                && q.emission_intent_at_ms.is_some()
                && q.recovered_at_ms.is_none(),
            "unproven question return"
        );
        q.observed_request = Some(request.to_owned());
        Ok(())
    }
}

pub(in crate::session::afk) fn guards(store: &Store, binding: &Binding) -> Result<Value> {
    let book = Book::load(store)?;
    Ok(json!(book
        .windows
        .iter()
        .filter(|w| w.binding.native_id == binding.native_id
            && w.binding.instance_id == binding.instance_id
            && w.binding.profile == binding.profile)
        .filter_map(
            |w| w.question.as_ref().filter(|q| q.unresolved()).map(|q| json!({
                "window":w.id,"binding":w.binding,"generation":w.generation,"identity":q.identity,"emission_intent":q.emission_intent_at_ms.is_some()
            }))
        )
        .collect::<Vec<_>>()))
}
pub(in crate::session::afk) fn recover(
    store: &Store,
    binding: &Binding,
    window: &str,
    identity: &QuestionIdentity,
    validate: impl Fn() -> Result<()>,
) -> Result<Value> {
    let _lock = store.lock(TIMEOUT)?;
    validate()?;
    let mut book = Book::load(store)?;
    let w = book
        .windows
        .iter_mut()
        .find(|w| w.id == window)
        .context("unknown question recovery")?;
    ensure!(
        w.binding.native_id == binding.native_id
            && w.binding.instance_id == binding.instance_id
            && w.binding.profile == binding.profile,
        "stale question recovery"
    );
    let q = w.question_mut(identity)?;
    if q.unresolved() {
        q.recovered_at_ms = Some(now_ms());
    }
    w.end(TerminalReason::Interrupted);
    book.save(store)?;
    Ok(json!({"recovered":true,"authority_restored":false}))
}
