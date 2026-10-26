//! Durable, separately approved skill authoring, publication and targeted adoption.
mod artifacts;
#[cfg(test)]
mod composition_tests;
mod deployment;
mod developer;
mod publication;
pub use deployment::{DeployIntent, DeployStage, DeployTarget, Deployment, RolloutAttempt};
mod source;
pub use publication::Publication;
#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod source_tests;
#[cfg(test)]
mod tests;

use super::CaptureAccount;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, thiserror::Error)]
#[error("Effect outcome uncertain; retained intent must be reopened, never replayed: {0}")]
pub struct EffectOutcomeUncertain(#[source] anyhow::Error);
impl EffectOutcomeUncertain {
    pub(crate) fn new(error: anyhow::Error) -> Self {
        Self(error)
    }
}
fn uncertain(error: anyhow::Error) -> anyhow::Error {
    EffectOutcomeUncertain::new(error).into()
}

pub(crate) fn text(value: &str, max: usize) -> Result<()> {
    ensure!(
        value.len() <= max && !value.contains('\0'),
        "Text exceeds {max} bytes or contains NUL"
    );
    Ok(())
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(Sha256::digest(serde_json::to_vec(value)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
fn slug(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            && !value.starts_with('-')
            && !value.ends_with('-'),
        "Use a lowercase skill slug of 1 to 64 letters, digits and internal hyphens"
    );
    Ok(())
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateEnvelope {
    pub account: CaptureAccount,
    pub idea: String,
    pub repository: PathBuf,
    pub slug: String,
    pub source_scope: String,
}
impl CreateEnvelope {
    fn validate(&self) -> Result<()> {
        slug(&self.slug)?;
        text(&self.idea, 16_384)?;
        text(&self.source_scope, 4096)?;
        ensure!(
            !self.idea.trim().is_empty() && !self.source_scope.trim().is_empty(),
            "Enter the idea and explicit account/source suitability for this repository"
        );
        source::repository(&self.repository)?;
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub files: BTreeMap<String, String>,
}
impl Bundle {
    pub fn validate(&self, name: &str) -> Result<()> {
        slug(name)?;
        ensure!(
            !self.files.is_empty() && self.files.len() <= 32,
            "A bundle needs 1 to 32 files"
        );
        let folded: std::collections::BTreeSet<_> =
            self.files.keys().map(|p| p.to_ascii_lowercase()).collect();
        ensure!(
            folded.len() == self.files.len(),
            "Case-folded bundle paths collide"
        );
        let mut spellings = BTreeMap::new();
        let mut total = 0;
        for (path, content) in &self.files {
            ensure!(
                !path.is_empty()
                    && path.len() <= 240
                    && path.is_ascii()
                    && path.split('/').all(|p| !p.is_empty()
                        && p != "."
                        && p != ".."
                        && !p.eq_ignore_ascii_case(".git")
                        && !p.starts_with('.')
                        && p.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))),
                "Unsafe relative bundle path: {path:?}"
            );
            ensure!(path.split('/').count() <= 8, "Bundle path is too deep");
            text(content, 65_536)?;
            total += content.len();
            let mut component_path = String::new();
            for part in path.split('/') {
                if !component_path.is_empty() {
                    component_path.push('/');
                }
                component_path.push_str(part);
                if let Some(old) =
                    spellings.insert(component_path.to_ascii_lowercase(), component_path.clone())
                {
                    ensure!(
                        old == component_path,
                        "Bundle directory components have case aliases"
                    );
                }
            }
            let mut prefix = String::new();
            for part in path.split('/').take(path.split('/').count() - 1) {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                ensure!(
                    !folded.contains(&prefix.to_ascii_lowercase()),
                    "Bundle file/directory conflict"
                );
            }
        }
        ensure!(total <= 256 * 1024, "Bundle exceeds 256 KiB");
        let parsed = crate::session::skills_model::parse_skill_md(
            self.files
                .get("SKILL.md")
                .context("Bundle needs SKILL.md")?,
        )?;
        ensure!(
            parsed.name == name,
            "SKILL.md name must equal the selected slug"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub reply: String,
    pub bundle: Bundle,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub user: String,
    pub proposal: Option<Proposal>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Draft,
    Refining,
    Reviewing,
    Review,
    Committing,
    Committed,
    PublishPreview,
    Publishing,
    PublishReady,
    PublishUnknown,
    Published,
    LandPreview,
    Landing,
    LandPending,
    LandUnknown,
    Landed,
    ImportPreview,
    Importing,
    Imported,
    CandidatePreview,
    Applying,
    PublishedConfig,
    Materializing,
    Materialized,
    RolloutPreview,
    RollingOut,
    RolloutComplete,
    DeployUnknown,
    Uncertain,
    Failed,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub revision: u64,
    pub repository: PathBuf,
    pub base_commit: String,
    pub source_tree: String,
    pub skill_tree: String,
    pub files: Bundle,
    pub previous_files: BTreeMap<String, String>,
    pub diff: String,
    pub evidence: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub digest: String,
}
impl Review {
    fn computed_digest(&self) -> Result<String> {
        let mut copy = self.clone();
        copy.digest.clear();
        digest(&copy)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommitIntent {
    pub review_digest: String,
    pub commit_oid: String,
    pub branch: String,
    pub worktree: PathBuf,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommitReceipt {
    pub commit_oid: String,
    pub source_tree: String,
    pub skill_tree: String,
    pub branch: String,
    pub worktree: PathBuf,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub envelope: CreateEnvelope,
    pub revision: u64,
    pub state: State,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub hidden: bool,
    pub turns: Vec<Turn>,
    pub bundle: Option<Bundle>,
    pub review: Option<Review>,
    pub commit_intent: Option<CommitIntent>,
    pub commit: Option<CommitReceipt>,
    #[serde(default)]
    pub publication: Option<Publication>,
    #[serde(default)]
    pub deployment: Option<Deployment>,
    pub error: Option<String>,
}
impl Request {
    fn revision(&self, revision: u64) -> Result<()> {
        ensure!(
            revision == self.revision,
            "Workshop revision changed; reopen this request"
        );
        Ok(())
    }
    fn editable(&self, revision: u64) -> Result<()> {
        self.revision(revision)?;
        ensure!(matches!(self.state, State::Draft | State::Review | State::Failed), "Request is busy or already committed; reconcile existing intent instead of replaying it");
        Ok(())
    }
    pub fn history(&self) -> super::history::HistoryEntry {
        super::history::HistoryEntry {
            id: self.id.clone(),
            profile: "workshop".into(),
            title: format!("Skill workshop · {}", self.envelope.slug),
            account: self.envelope.account,
            input_preview: self
                .envelope
                .idea
                .chars()
                .filter(|c| {
                    !c.is_control()
                        && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                })
                .take(160)
                .collect(),
            status: serde_json::to_value(self.state)
                .expect("state")
                .as_str()
                .expect("state string")
                .into(),
            created_at: self.created_at,
            hidden: self.hidden,
            research: false,
            workshop: true,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Operation {
    Create {
        id: String,
        envelope: CreateEnvelope,
    },
    Propose {
        id: String,
        revision: u64,
        bundle: Bundle,
    },
    Refine {
        id: String,
        revision: u64,
        message: String,
    },
    RecoverRefinement {
        id: String,
        revision: u64,
    },
    Review {
        id: String,
        revision: u64,
    },
    Commit {
        id: String,
        revision: u64,
        review_digest: String,
    },
    Reconcile {
        id: String,
        revision: u64,
    },
    Visibility {
        id: String,
        revision: u64,
        hidden: bool,
    },
    PreviewPublish {
        id: String,
        revision: u64,
        remote: String,
        repository_id: u64,
        base_branch: String,
        title: String,
        body: String,
    },
    Publish {
        id: String,
        revision: u64,
        preview_digest: String,
        disclosure_confirmed: bool,
    },
    ContinuePublish {
        id: String,
        revision: u64,
    },
    PreviewLand {
        id: String,
        revision: u64,
    },
    Land {
        id: String,
        revision: u64,
        policy_digest: String,
        current_base_and_rules: bool,
        exclusive_pr_custody: bool,
    },
    ReconcilePublication {
        id: String,
        revision: u64,
    },
    PreviewImport {
        id: String,
        revision: u64,
        source_name: String,
        targets: Vec<DeployTarget>,
    },
    ApproveImport {
        id: String,
        revision: u64,
        preview_digest: String,
    },
    PreviewDeploy {
        id: String,
        revision: u64,
    },
    Deploy {
        id: String,
        revision: u64,
        candidate_digest: String,
    },
    Materialize {
        id: String,
        revision: u64,
        candidate_digest: String,
    },
    PreviewRollout {
        id: String,
        revision: u64,
    },
    ApplyRollout {
        id: String,
        revision: u64,
        plan_digest: String,
    },
    ReconcileDeploy {
        id: String,
        revision: u64,
    },
}
impl Operation {
    fn id(&self) -> &str {
        match self {
            Self::Create { id, .. }
            | Self::Propose { id, .. }
            | Self::Refine { id, .. }
            | Self::RecoverRefinement { id, .. }
            | Self::Review { id, .. }
            | Self::Commit { id, .. }
            | Self::Reconcile { id, .. }
            | Self::Visibility { id, .. }
            | Self::PreviewPublish { id, .. }
            | Self::Publish { id, .. }
            | Self::ContinuePublish { id, .. }
            | Self::PreviewLand { id, .. }
            | Self::Land { id, .. }
            | Self::ReconcilePublication { id, .. }
            | Self::PreviewImport { id, .. }
            | Self::ApproveImport { id, .. }
            | Self::PreviewDeploy { id, .. }
            | Self::Deploy { id, .. }
            | Self::Materialize { id, .. }
            | Self::PreviewRollout { id, .. }
            | Self::ApplyRollout { id, .. }
            | Self::ReconcileDeploy { id, .. } => id,
        }
    }
}

#[derive(Clone)]
pub struct Store {
    path: PathBuf,
    #[cfg(test)]
    fail_settle: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    fail_directory_sync: std::sync::Arc<std::sync::atomic::AtomicU8>,
}
impl Store {
    pub fn open() -> Result<Self> {
        Ok(Self::at(
            crate::session::get_app_dir()?.join("workshop.json"),
        ))
    }
    pub(crate) fn at(path: PathBuf) -> Self {
        Self {
            path,
            #[cfg(test)]
            fail_settle: Default::default(),
            #[cfg(test)]
            fail_directory_sync: Default::default(),
        }
    }
    fn operation_lock(&self, id: &str) -> Result<crate::session::StorageFlock> {
        ensure!(
            uuid::Uuid::parse_str(id)?.to_string() == id,
            "Canonical Workshop UUID required"
        );
        crate::session::try_acquire_storage_flock(
            self.path
                .parent()
                .context("Missing Workshop store parent")?,
            &format!(".workshop-{id}.operation.lock"),
        )?
        .context("This Workshop request has an admitted host operation; reopen after it settles")
    }

    fn update<R>(&self, f: impl FnOnce(&mut BTreeMap<String, Request>) -> Result<R>) -> Result<R> {
        let path = crate::session::resolve_symlink_chain(&self.path)?;
        let before = std::cell::RefCell::new(None::<String>);
        let changed = std::cell::Cell::new(false);
        let result = crate::session::locked_update_if_changed(
            &path,
            |s| Ok(serde_json::from_str(s)?),
            |rows| {
                let serialized = serde_json::to_string_pretty(rows)?;
                let mut before = before.borrow_mut();
                if let Some(old) = before.as_ref() {
                    changed.set(old != &serialized);
                } else {
                    *before = Some(serialized.clone());
                }
                Ok(serialized)
            },
            f,
        )??;
        if changed.get() {
            #[cfg(test)]
            let fault = self
                .fail_directory_sync
                .swap(0, std::sync::atomic::Ordering::SeqCst);
            #[cfg(test)]
            ensure!(
                fault != 1,
                "Injected Workshop parent-directory open failure after rename"
            );
            let parent =
                std::fs::File::open(path.parent().context("Missing Workshop store parent")?)
                    .context("Opening Workshop parent after rename")?;
            #[cfg(test)]
            ensure!(
                fault != 2,
                "Injected Workshop parent-directory sync failure after rename"
            );
            parent
                .sync_all()
                .context("Durably publishing Workshop rename")?;
        }
        Ok(result)
    }
    pub fn list(&self) -> Result<Vec<Request>> {
        self.update(|rows| Ok(rows.values().cloned().collect()))
    }
    pub fn get(&self, id: &str) -> Result<Request> {
        let request =
            self.update(|rows| rows.get(id).cloned().context("Workshop request not found"))?;
        if let Some(deployment) = &request.deployment {
            deployment.projection.verify().context("Retained landed projection changed or unavailable; deployment halted, never regenerate against prior approval")?;
        }
        Ok(request)
    }
    fn change<R>(&self, id: &str, f: impl FnOnce(&mut Request) -> Result<R>) -> Result<R> {
        self.update(|rows| f(rows.get_mut(id).context("Workshop request not found")?))
    }
    fn settle<R>(&self, id: &str, f: impl FnOnce(&mut Request) -> Result<R>) -> Result<R> {
        #[cfg(test)]
        if self
            .fail_settle
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(uncertain(anyhow::anyhow!(
                "Injected completion persistence failure after actual effect"
            )));
        }
        self.change(id, f).map_err(uncertain)
    }
    pub fn create(&self, id: String, envelope: CreateEnvelope) -> Result<Request> {
        ensure!(
            uuid::Uuid::parse_str(&id)?.to_string() == id,
            "Workshop ID must be a canonical UUID"
        );
        envelope.validate()?;
        self.update(|rows| {
            if let Some(old) = rows.get(&id) {
                ensure!(
                    old.envelope == envelope,
                    "Workshop ID already belongs to a different complete creation envelope"
                );
                return Ok(old.clone());
            }
            let request = Request {
                id: id.clone(),
                envelope,
                revision: 0,
                state: State::Draft,
                created_at: chrono::Utc::now(),
                hidden: false,
                turns: vec![],
                bundle: None,
                review: None,
                commit_intent: None,
                commit: None,
                publication: None,
                deployment: None,
                error: None,
            };
            rows.insert(id, request.clone());
            Ok(request)
        })
    }
    fn begin_refinement(&self, id: &str, revision: u64, message: String) -> Result<Request> {
        text(&message, 8192)?;
        ensure!(!message.trim().is_empty(), "Enter a refinement message");
        self.change(id, |d| {
            d.editable(revision)?;
            ensure!(d.turns.len() < 12, "Workshop conversation is limited to 12 turns; review or create a new request explicitly");
            ensure!(serde_json::to_vec(&d.turns)?.len() + message.len() < 512 * 1024, "Conversation exceeds its bounded context");
            d.turns.push(Turn { user: message, proposal: None });
            d.review = None; d.state = State::Refining; d.revision += 1; d.error = None; Ok(d.clone())
        })
    }
    fn finish_refinement(&self, id: &str, revision: u64, result: Result<Proposal>) -> Result<()> {
        self.change(id, |d| {
            if d.state != State::Refining || d.revision != revision {
                return Ok(());
            }
            let result = result.and_then(|p| {
                text(&p.reply, 8192)?;
                p.bundle.validate(&d.envelope.slug)?;
                Ok(p)
            });
            match result {
                Ok(proposal) => {
                    d.bundle = Some(proposal.bundle.clone());
                    d.turns
                        .last_mut()
                        .context("Missing refinement turn")?
                        .proposal = Some(proposal);
                    d.state = State::Draft;
                    d.error = None;
                }
                Err(error) => {
                    d.state = State::Failed;
                    d.error = Some(format!("Refinement failed: {error:#}"));
                }
            }
            d.revision += 1;
            Ok(())
        })
    }
    pub fn operate(&self, op: Operation) -> Result<Request> {
        let _admission = if matches!(
            op,
            Operation::Visibility { .. } | Operation::Commit { .. } | Operation::Reconcile { .. }
        ) {
            Some(self.operation_lock(op.id())?)
        } else {
            None
        };
        let id = match op {
            Operation::Create { id, envelope } => return self.create(id, envelope),
            operation @ (Operation::PreviewPublish { .. }
            | Operation::Publish { .. }
            | Operation::ContinuePublish { .. }
            | Operation::PreviewLand { .. }
            | Operation::Land { .. }
            | Operation::ReconcilePublication { .. }) => {
                return self.publication_operation(
                    operation,
                    &crate::github::forge::Forge::authenticated()?,
                )
            }
            operation @ (Operation::PreviewImport { .. }
            | Operation::ApproveImport { .. }
            | Operation::PreviewDeploy { .. }
            | Operation::Deploy { .. }
            | Operation::Materialize { .. }
            | Operation::PreviewRollout { .. }
            | Operation::ApplyRollout { .. }
            | Operation::ReconcileDeploy { .. }) => return self.deployment_operation(operation),
            Operation::Propose {
                id,
                revision,
                bundle,
            } => {
                self.change(&id, |d| {
                    d.editable(revision)?;
                    bundle.validate(&d.envelope.slug)?;
                    d.bundle = Some(bundle);
                    d.review = None;
                    d.state = State::Draft;
                    d.revision += 1;
                    d.error = None;
                    Ok(())
                })?;
                id
            }
            Operation::Refine {
                id,
                revision,
                message,
            } => {
                let draft = self.begin_refinement(&id, revision, message)?;
                self.finish_refinement(&id, draft.revision, developer::refine(&draft))?;
                id
            }
            Operation::RecoverRefinement { id, revision } => {
                self.change(&id, |d| { d.revision(revision)?; ensure!(d.state == State::Refining, "Only pending refinement can be abandoned"); d.state = State::Draft; d.revision += 1; d.error = Some("Refinement abandoned; late output is fenced. No model proposal authorizes effects.".into()); Ok(()) })?;
                id
            }
            Operation::Review { id, revision } => {
                let draft = self.change(&id, |d| {
                    d.editable(revision)?;
                    d.bundle
                        .as_ref()
                        .context("Propose a bundle before Review")?
                        .validate(&d.envelope.slug)?;
                    d.state = State::Reviewing;
                    d.review = None;
                    d.revision += 1;
                    d.error = None;
                    Ok(d.clone())
                })?;
                let result = source::review(&draft);
                self.change(&id, |d| {
                    ensure!(
                        d.revision == draft.revision && d.state == State::Reviewing,
                        "Review intent changed"
                    );
                    match result {
                        Ok(review) => {
                            d.review = Some(review);
                            d.state = State::Review;
                        }
                        Err(error) => {
                            d.state = State::Failed;
                            d.error = Some(format!("Review failed: {error:#}"));
                        }
                    }
                    d.revision += 1;
                    Ok(())
                })?;
                id
            }
            Operation::Commit {
                id,
                revision,
                review_digest,
            } => {
                let draft = self.get(&id)?;
                if matches!(
                    draft.state,
                    State::Committing | State::Committed | State::Uncertain
                ) {
                    ensure!(
                        draft
                            .commit_intent
                            .as_ref()
                            .is_some_and(|i| i.review_digest == review_digest),
                        "Commit intent already exists for another review"
                    );
                    return Ok(draft);
                }
                draft.revision(revision)?;
                ensure!(
                    draft.state == State::Review,
                    "Freeze a fresh review before approving a local commit"
                );
                let review = draft.review.as_ref().context("Missing review")?;
                ensure!(
                    review.digest == review_digest && review.computed_digest()? == review_digest,
                    "Approval does not match the exact frozen review"
                );
                source::validate_review(&draft)?;
                let intent = source::intent(
                    &draft,
                    self.path
                        .parent()
                        .context("Missing Workshop store directory")?,
                )?;
                let pending = self.change(&id, |d| {
                    d.revision(revision)?;
                    ensure!(d.state == State::Review, "Commit already attempted");
                    d.state = State::Committing;
                    d.commit_intent = Some(intent);
                    d.revision += 1;
                    Ok(d.clone())
                })?;
                let result = source::commit(&pending);
                self.settle(&id, |d| {
                    ensure!(
                        d.revision == pending.revision && d.state == State::Committing,
                        "Commit intent changed"
                    );
                    match result {
                        Ok(receipt) => {
                            d.commit = Some(receipt);
                            d.state = State::Committed;
                            d.error = None;
                        }
                        Err(error) => {
                            d.state = State::Uncertain;
                            d.error = Some(format!(
                                "Commit outcome uncertain; reconcile, do not retry: {error:#}"
                            ));
                        }
                    }
                    d.revision += 1;
                    Ok(())
                })?;
                id
            }
            Operation::Reconcile { id, revision } => {
                let draft = self.get(&id)?;
                draft.revision(revision)?;
                ensure!(
                    matches!(
                        draft.state,
                        State::Committing | State::Committed | State::Uncertain | State::Reviewing
                    ),
                    "No effect intent to reconcile"
                );
                if draft.state == State::Reviewing {
                    self.change(&id, |d| { d.revision(revision)?; d.state = State::Draft; d.revision += 1; d.error = Some("Interrupted static review abandoned; request a fresh Review explicitly".into()); Ok(()) })?;
                } else {
                    let receipt = source::reconcile(&draft);
                    self.change(&id, |d| { d.revision(revision)?; match receipt { Ok(receipt) => { d.commit = Some(receipt); d.state = State::Committed; d.error = None; }, Err(error) => { d.state = State::Uncertain; d.error = Some(format!("Reconciliation could not prove the exact commit/worktree; no writes retried: {error:#}")); } } d.revision += 1; Ok(()) })?;
                }
                id
            }
            Operation::Visibility {
                id,
                revision,
                hidden,
            } => {
                self.change(&id, |d| {
                    d.revision(revision)?;
                    ensure!(
                        !matches!(
                            d.state,
                            State::Refining
                                | State::Reviewing
                                | State::Committing
                                | State::Publishing
                                | State::Landing
                                | State::Importing
                                | State::Applying
                                | State::Materializing
                                | State::RollingOut
                        ),
                        "Wait for the current operation before changing visibility"
                    );
                    d.hidden = hidden;
                    d.revision += 1;
                    Ok(())
                })?;
                id
            }
        };
        self.get(&id)
    }
}
