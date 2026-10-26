use super::*;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub(super) owner: String,
    pub(super) name: String,
    pub(super) repository_id: u64,
    pub(super) base_branch: String,
}
impl Destination {
    /// Only canonical HTTPS github.com URLs are accepted. SSH/config rewrites are not used.
    pub fn new(remote: &str, repository_id: u64, base_branch: &str) -> Result<Self> {
        let path = remote
            .strip_prefix("https://github.com/")
            .context("Use canonical https://github.com/OWNER/REPO.git")?;
        let mut parts = path.split('/');
        let owner = parts.next().context("Missing owner")?.to_owned();
        let name = parts.next().context("Missing repository")?;
        let name = name.strip_suffix(".git").unwrap_or(name).to_owned();
        ensure!(parts.next().is_none(), "Extra remote path components");
        let value = Self {
            owner,
            name,
            repository_id,
            base_branch: base_branch.into(),
        };
        value.validate()?;
        ensure!(
            remote == value.remote() || remote == value.remote().trim_end_matches(".git"),
            "Noncanonical remote"
        );
        Ok(value)
    }
    pub(super) fn validate(&self) -> Result<()> {
        component(&self.owner)?;
        component(&self.name)?;
        branch(&self.base_branch)?;
        ensure!(
            self.repository_id > 0 && !self.name.ends_with(".git"),
            "Invalid repository identity"
        );
        Ok(())
    }
    pub fn remote(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.name)
    }
    pub fn repository_id(&self) -> u64 {
        self.repository_id
    }
    pub fn base_branch(&self) -> &str {
        &self.base_branch
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Account {
    Personal,
    Work,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Private,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub id: u64,
    pub login: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryIdentity {
    pub id: u64,
    pub node_id: String,
    pub visibility: Visibility,
    pub default_branch: String,
}

/// Input assembled by the host from its frozen Review and reconciled local Commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSource {
    pub repository: PathBuf,
    pub common_dir: PathBuf,
    pub request_id: String,
    pub revision: u64,
    pub review_digest: String,
    pub base_commit: String,
    pub commit_oid: String,
    pub source_tree: String,
    pub skill_tree: String,
    pub skill_path: String,
    /// SHA-256 over the canonical JSON list of (relative path, Git mode, blob OID).
    pub content_digest: String,
    pub account: Account,
    /// Source suitability context, NOT a disclosure grant.
    pub source_scope: String,
}
impl HostSource {
    pub fn branch(&self) -> String {
        format!("workshop/{}", self.request_id)
    }
    pub fn owned_ref(&self) -> String {
        format!("refs/heads/{}", self.branch())
    }
    /// Computes the host review's subtree fingerprint without checkout/index/filter effects.
    pub fn reviewed_content_digest(
        repository: &std::path::Path,
        skill_tree: &str,
    ) -> Result<String> {
        source::content_digest(repository, skill_tree)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishPreview {
    pub source: HostSource,
    pub destination: Destination,
    pub repository: RepositoryIdentity,
    pub actor: Actor,
    pub base_head: String,
    pub title: String,
    pub body: String,
    pub digest: String,
}
impl PublishPreview {
    pub(super) fn computed_digest(&self) -> Result<String> {
        let mut copy = self.clone();
        copy.digest.clear();
        digest(&copy)
    }
}
/// Must come from a separate, explicit host UI action displaying the complete preview.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishApproval {
    pub preview_digest: String,
    pub disclosure_confirmed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCommit {
    pub commit_oid: String,
    pub source_tree: String,
    pub skill_tree: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublishStage {
    Upload,
    CreateRef { commit: RemoteCommit },
    CreatePr { commit: RemoteCommit },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishIntent {
    pub(super) attempt: String,
    pub(super) preview: PublishPreview,
    pub(super) stage: PublishStage,
}
impl PublishIntent {
    pub fn stage(&self) -> &PublishStage {
        &self.stage
    }
    pub fn preview(&self) -> &PublishPreview {
        &self.preview
    }
    pub fn attempt(&self) -> &str {
        &self.attempt
    }
    pub(super) fn next(&self, stage: PublishStage) -> Self {
        Self {
            stage,
            ..self.clone()
        }
    }
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.attempt)?.to_string() == self.attempt
                && self.preview.computed_digest()? == self.preview.digest,
            "Invalid/stale publication intent"
        );
        self.preview.destination.validate()
    }
    pub(super) fn pr_body(&self) -> String {
        format!(
            "{}\n\n<!-- aoe-workshop:{}:{} -->",
            self.preview.body, self.preview.source.request_id, self.preview.digest
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PublishProgress {
    Next(Box<PublishIntent>),
    Published(Box<PublishedPr>),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrIdentity {
    pub repository_id: u64,
    pub id: u64,
    pub node_id: String,
    pub number: u64,
    pub url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedPr {
    pub identity: PrIdentity,
    pub destination: Destination,
    pub head: String,
    pub head_ref: String,
    pub skill_path: String,
    pub skill_tree: String,
    pub content_digest: String,
    pub publish_digest: String,
    pub actor: Actor,
    pub publication_content_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Outcome<T> {
    Confirmed(T),
    Refused(String),
    Unknown(String),
}

/// Observed policy, not a client-invented readiness score. GitHub enforces current rules.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyObservation {
    pub published: PublishedPr,
    pub base_head: String,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub rules: Vec<Value>,
    pub classic: Value,
    pub digest: String,
}
impl PolicyObservation {
    pub(super) fn computed_digest(&self) -> Result<String> {
        let mut value = self.clone();
        value.digest.clear();
        digest(&value)
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    Squash,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandApproval {
    pub policy_digest: String,
    pub method: MergeMethod,
    /// Exact reviewed head under current GitHub base/rules, not a base/policy CAS.
    pub current_base_and_rules: bool,
    /// The human exclusively controls this owned PR during Land. GitHub provides
    /// no stack-membership or target-ref CAS; this is an explicit user assumption.
    pub exclusive_pr_custody: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandIntent {
    pub(super) attempt: String,
    pub(super) observation: PolicyObservation,
    pub(super) approval: LandApproval,
    pub(super) async_uuid: Option<String>,
}
impl LandIntent {
    pub fn attempt(&self) -> &str {
        &self.attempt
    }
    pub fn async_uuid(&self) -> Option<&str> {
        self.async_uuid.as_deref()
    }
    pub fn observation(&self) -> &PolicyObservation {
        &self.observation
    }
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.attempt)?.to_string() == self.attempt
                && self.observation.computed_digest()? == self.observation.digest
                && self.approval.policy_digest == self.observation.digest
                && self.approval.current_base_and_rules
                && self.approval.exclusive_pr_custody,
            "Stale/invalid Land intent"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LandProgress {
    Pending(Box<LandIntent>),
    Landed(Box<LandedSource>),
}
/// Only actual merged PR and Git commit/tree evidence can produce this receipt.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandedSource {
    pub identity: PrIdentity,
    pub approved_head: String,
    pub merged_commit: String,
    pub source_tree: String,
    pub skill_path: String,
    pub skill_tree: String,
    pub content_digest: String,
    pub land_intent_digest: String,
    pub policy_digest: String,
    pub api_version: String,
}
