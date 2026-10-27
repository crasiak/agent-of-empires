//! Host-owned, approval-bound GitHub publication and non-bypass landing.
//!
//! Persist every returned intent before executing it. An uncertain write is only
//! reconciled, never replayed. Call these blocking operations off the UI thread.
mod export;
mod policy;
mod source;
#[cfg(test)]
mod tests;
mod transport;
mod types;

pub use export::ProjectionReceipt;
pub use transport::{GhTransport, HttpRequest, HttpResponse, Method, Transport};
pub use types::*;

use anyhow::{bail, ensure, Context};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

type Result<T> = anyhow::Result<T>;
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 10;
const API_VERSION: &str = "2026-03-10";

fn digest(value: &impl Serialize) -> Result<String> {
    Ok(Sha256::digest(serde_json::to_vec(value)?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn oid(value: &str) -> Result<git2::Oid> {
    ensure!(
        value.len() == 40
            && value
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "Expected a canonical Git SHA-1"
    );
    Ok(git2::Oid::from_str(value)?)
}
fn bounded_text(value: &str, max: usize) -> Result<()> {
    ensure!(
        value.len() <= max && !value.contains('\0'),
        "Text exceeds bound or contains NUL"
    );
    Ok(())
}
fn component(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 100
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)),
        "Unsafe GitHub path component"
    );
    Ok(())
}
fn branch(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 200
            && !value.starts_with('-')
            && git2::Reference::is_valid_name(&format!("refs/heads/{value}"))
            && value.split('/').all(|s| component(s).is_ok()),
        "Unsafe branch"
    );
    Ok(())
}
fn segment(value: &str) -> String {
    value
        .bytes()
        .map(|c| {
            if c.is_ascii_alphanumeric() || b"-_.".contains(&c) {
                (c as char).to_string()
            } else {
                format!("%{c:02X}")
            }
        })
        .collect()
}

pub struct Forge<T: Transport> {
    transport: T,
}
impl Forge<GhTransport> {
    /// Resolves gh, but does not read authentication or contact GitHub.
    pub fn authenticated() -> Result<Self> {
        Ok(Self::new(GhTransport::new()?))
    }
}
impl<T: Transport> Forge<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
    fn request(
        &self,
        destination: &Destination,
        method: Method,
        suffix: &str,
        body: Option<Value>,
    ) -> Result<HttpResponse> {
        destination.validate()?;
        let request = HttpRequest::new(
            method,
            format!(
                "/repos/{}/{}{}",
                destination.owner, destination.name, suffix
            ),
            body,
        )?;
        let response = self.transport.send(&request)?;
        ensure!(response.body.len() <= MAX_BODY, "HTTP body exceeded bound");
        Ok(response)
    }
    fn get<R: DeserializeOwned>(&self, destination: &Destination, suffix: &str) -> Result<R> {
        let response = self.request(destination, Method::Get, suffix, None)?;
        ensure!(
            response.status == 200 && !response.next_page,
            "Incomplete/unavailable GitHub response ({})",
            response.status
        );
        Ok(serde_json::from_slice(&response.body)?)
    }
    fn write<R: DeserializeOwned>(
        &self,
        destination: &Destination,
        method: Method,
        suffix: &str,
        body: Value,
    ) -> Result<R> {
        let response = self.request(destination, method, suffix, Some(body))?;
        ensure!(
            matches!(response.status, 200 | 201) && !response.next_page,
            "GitHub write not confirmed ({})",
            response.status
        );
        Ok(serde_json::from_slice(&response.body)?)
    }
    fn pages(&self, destination: &Destination, suffix: &str) -> Result<Vec<Value>> {
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            let sep = if suffix.contains('?') { '&' } else { '?' };
            let response = self.request(
                destination,
                Method::Get,
                &format!("{suffix}{sep}per_page=100&page={page}"),
                None,
            )?;
            ensure!(
                response.status == 200,
                "Incomplete/private GitHub listing ({})",
                response.status
            );
            let items: Vec<Value> = serde_json::from_slice(&response.body)?;
            ensure!(items.len() <= 100, "Oversized page");
            let count = items.len();
            all.extend(items);
            if !response.next_page && count < 100 {
                return Ok(all);
            }
            ensure!(count > 0, "Contradictory pagination");
        }
        bail!("GitHub pagination limit reached; evidence incomplete")
    }
    fn actor(&self) -> Result<Actor> {
        let response =
            self.transport
                .send(&HttpRequest::new(Method::Get, "/user".into(), None)?)?;
        ensure!(
            response.status == 200 && !response.next_page && response.body.len() <= MAX_BODY,
            "Authenticated actor unknown"
        );
        let user: Value = serde_json::from_slice(&response.body)?;
        ensure!(user["type"] == "User", "A human host account is required");
        let actor = Actor {
            id: user["id"].as_u64().context("Missing actor id")?,
            login: user["login"]
                .as_str()
                .context("Missing actor login")?
                .into(),
        };
        component(&actor.login)?;
        ensure!(actor.id > 0, "Missing actor identity");
        Ok(actor)
    }
    fn repository(&self, destination: &Destination) -> Result<RepositoryIdentity> {
        let value: Value = self.get(destination, "")?;
        let visibility = match value["visibility"].as_str() {
            Some("public") => Visibility::Public,
            Some("private") => Visibility::Private,
            _ => bail!("Repository visibility is unknown/unsupported"),
        };
        ensure!(
            value["private"].as_bool() == Some(visibility == Visibility::Private),
            "Contradictory repository visibility"
        );
        ensure!(
            value["id"].as_u64() == Some(destination.repository_id)
                && value["full_name"].as_str()
                    == Some(&format!("{}/{}", destination.owner, destination.name)),
            "Destination repository identity changed"
        );
        ensure!(
            value["clone_url"].as_str() == Some(&destination.remote()),
            "Noncanonical destination remote"
        );
        let node_id = value["node_id"]
            .as_str()
            .context("Missing repository node ID")?
            .to_owned();
        bounded_text(&node_id, 200)?;
        ensure!(
            !node_id.is_empty()
                && value["archived"].as_bool() == Some(false)
                && value["disabled"].as_bool() == Some(false),
            "Unavailable repository"
        );
        let default_branch = value["default_branch"]
            .as_str()
            .context("Missing default branch")?
            .into();
        Ok(RepositoryIdentity {
            id: destination.repository_id,
            node_id,
            visibility,
            default_branch,
        })
    }
    fn base(&self, destination: &Destination) -> Result<String> {
        let reference: RemoteRef = self.get(
            destination,
            &format!("/git/ref/heads/{}", segment(&destination.base_branch)),
        )?;
        ensure!(
            reference.reference == format!("refs/heads/{}", destination.base_branch)
                && reference.object.kind == "commit",
            "Base ref identity differs"
        );
        oid(&reference.object.sha)?;
        Ok(reference.object.sha)
    }
    fn source_reachable(
        &self,
        destination: &Destination,
        parent: &str,
        current_base: &str,
    ) -> Result<()> {
        let value: Value = self.get(destination, &format!("/compare/{parent}...{current_base}"))?;
        ensure!(
            value["merge_base_commit"]["sha"].as_str() == Some(parent)
                && matches!(value["status"].as_str(), Some("ahead" | "identical")),
            "Reviewed parent is not reachable from destination base"
        );
        Ok(())
    }
    /// Read-only. Source scope is displayed/bound, never interpreted as disclosure consent.
    pub fn preview_publish(
        &self,
        source: HostSource,
        destination: Destination,
        title: String,
        body: String,
    ) -> Result<PublishPreview> {
        source::verify(&source)?;
        bounded_text(&title, 256)?;
        bounded_text(&body, 32_768)?;
        ensure!(!title.trim().is_empty(), "PR title is required");
        let repository = self.repository(&destination)?;
        let actor = self.actor()?;
        let base_head = self.base(&destination)?;
        self.source_reachable(&destination, &source.base_commit, &base_head)?;
        let mut preview = PublishPreview {
            source,
            destination,
            repository,
            actor,
            base_head,
            title,
            body,
            digest: String::new(),
        };
        preview.digest = preview.computed_digest()?;
        Ok(preview)
    }
    /// Pure planning. The host must persist this intent before executing it.
    pub fn prepare_publish(
        &self,
        preview: PublishPreview,
        approval: PublishApproval,
    ) -> Result<PublishIntent> {
        ensure!(
            preview.computed_digest()? == preview.digest
                && approval.preview_digest == preview.digest
                && approval.disclosure_confirmed,
            "Explicit destination-aware publication/disclosure approval is missing or stale"
        );
        source::verify(&preview.source)?;
        Ok(PublishIntent {
            attempt: uuid::Uuid::new_v4().to_string(),
            preview,
            stage: PublishStage::Upload,
        })
    }
    fn revalidate_publish(&self, intent: &PublishIntent) -> Result<()> {
        intent.validate()?;
        source::verify(&intent.preview.source)?;
        ensure!(
            self.repository(&intent.preview.destination)? == intent.preview.repository
                && self.actor()? == intent.preview.actor,
            "Destination visibility/account changed; fresh approval required"
        );
        let base = self.base(&intent.preview.destination)?;
        self.source_reachable(
            &intent.preview.destination,
            &intent.preview.source.base_commit,
            &base,
        )
    }
    /// Executes the persisted stage exactly once. Never call again after Unknown.
    pub fn publish(&self, intent: &PublishIntent) -> Outcome<PublishProgress> {
        if let Err(error) = self.revalidate_publish(intent) {
            return Outcome::Refused(error.to_string());
        }
        match self.publish_inner(intent) {
            Ok(value) => Outcome::Confirmed(value),
            Err(error) => Outcome::Unknown(error.to_string()),
        }
    }
    fn publish_inner(&self, intent: &PublishIntent) -> Result<PublishProgress> {
        let preview = &intent.preview;
        let destination = &preview.destination;
        match &intent.stage {
            PublishStage::Upload => {
                let commit = source::upload(self, intent)?;
                Ok(PublishProgress::Next(Box::new(
                    intent.next(PublishStage::CreateRef { commit }),
                )))
            }
            PublishStage::CreateRef { commit } => {
                self.verify_commit(destination, commit, &preview.source)?;
                if let Some(existing) = self.remote_owned_ref(intent)? {
                    ensure!(
                        existing == commit.commit_oid,
                        "Owned remote ref differs; never overwrite"
                    );
                } else {
                    let reference: RemoteRef = self.write(
                        destination,
                        Method::Post,
                        "/git/refs",
                        json!({"ref":preview.source.owned_ref(),"sha":commit.commit_oid}),
                    )?;
                    ensure!(
                        reference.reference == preview.source.owned_ref()
                            && reference.object.kind == "commit"
                            && reference.object.sha == commit.commit_oid,
                        "Create-ref receipt differs"
                    );
                }
                ensure!(
                    self.remote_owned_ref(intent)?.as_deref() == Some(&commit.commit_oid),
                    "Remote ref not confirmed"
                );
                Ok(PublishProgress::Next(Box::new(intent.next(
                    PublishStage::CreatePr {
                        commit: commit.clone(),
                    },
                ))))
            }
            PublishStage::CreatePr { commit } => {
                self.verify_commit(destination, commit, &preview.source)?;
                ensure!(
                    self.remote_owned_ref(intent)?.as_deref() == Some(&commit.commit_oid),
                    "Remote ref changed"
                );
                let existing = self.find_pr(intent, commit)?;
                let pr = if let Some(pr) = existing {
                    pr
                } else {
                    let value: Value = self.write(destination,Method::Post,"/pulls",json!({"head":preview.source.branch(),"base":destination.base_branch,"title":preview.title,"body":intent.pr_body(),"maintainer_can_modify":false}))?;
                    self.decode_pr(intent, commit, value)?
                };
                Ok(PublishProgress::Published(Box::new(pr)))
            }
        }
    }
    fn remote_owned_ref(&self, intent: &PublishIntent) -> Result<Option<String>> {
        let response = self.request(
            &intent.preview.destination,
            Method::Get,
            &format!(
                "/git/ref/heads/{}",
                segment(&intent.preview.source.branch())
            ),
            None,
        )?;
        if response.status == 404 {
            return Ok(None);
        }
        ensure!(
            response.status == 200 && !response.next_page,
            "Owned ref unknown"
        );
        let reference: RemoteRef = serde_json::from_slice(&response.body)?;
        ensure!(
            reference.reference == intent.preview.source.owned_ref()
                && reference.object.kind == "commit",
            "Owned ref identity differs"
        );
        oid(&reference.object.sha)?;
        Ok(Some(reference.object.sha))
    }
    fn find_pr(
        &self,
        intent: &PublishIntent,
        commit: &RemoteCommit,
    ) -> Result<Option<PublishedPr>> {
        let query = format!(
            "/pulls?state=all&head={}:{}",
            intent.preview.destination.owner,
            segment(&intent.preview.source.branch())
        );
        let values = self.pages(&intent.preview.destination, &query)?;
        ensure!(
            values.len() <= 1,
            "Ambiguous duplicate PRs for owned branch"
        );
        values
            .into_iter()
            .next()
            .map(|value| self.decode_pr(intent, commit, value))
            .transpose()
    }
    fn decode_pr(
        &self,
        intent: &PublishIntent,
        commit: &RemoteCommit,
        value: Value,
    ) -> Result<PublishedPr> {
        let destination = &intent.preview.destination;
        let number = value["number"]
            .as_u64()
            .filter(|n| *n > 0)
            .context("Missing PR number")?;
        let url = format!(
            "https://github.com/{}/{}/pull/{number}",
            destination.owner, destination.name
        );
        ensure!(
            value["html_url"].as_str() == Some(&url)
                && value["head"]["repo"]["id"].as_u64() == Some(destination.repository_id)
                && value["base"]["repo"]["id"].as_u64() == Some(destination.repository_id)
                && value["head"]["sha"].as_str() == Some(&commit.commit_oid)
                && value["head"]["ref"].as_str() == Some(&intent.preview.source.branch())
                && value["base"]["ref"].as_str() == Some(&destination.base_branch),
            "PR binding differs"
        );
        ensure!(
            value["title"].as_str() == Some(&intent.preview.title)
                && value["body"].as_str() == Some(&intent.pr_body())
                && value["maintainer_can_modify"].as_bool() == Some(false),
            "Existing PR does not match approved publication"
        );
        ensure!(
            value["state"] == "open" && value["draft"] == false,
            "PR is not open and ready for review"
        );
        ensure!(
            value["user"]["id"].as_u64() == Some(intent.preview.actor.id),
            "PR was not created by approved host account"
        );
        let node_id = value["node_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("Missing PR node ID")?
            .into();
        let id = value["id"]
            .as_u64()
            .filter(|n| *n > 0)
            .context("Missing PR id")?;
        Ok(PublishedPr {
            identity: PrIdentity {
                repository_id: destination.repository_id,
                id,
                node_id,
                number,
                url,
            },
            destination: destination.clone(),
            head: commit.commit_oid.clone(),
            head_ref: intent.preview.source.branch(),
            skill_path: intent.preview.source.skill_path.clone(),
            skill_tree: intent.preview.source.skill_tree.clone(),
            content_digest: intent.preview.source.content_digest.clone(),
            publish_digest: digest(intent)?,
            actor: intent.preview.actor.clone(),
            publication_content_digest: digest(&(intent.preview.title.clone(), intent.pr_body()))?,
        })
    }
    /// No writes, including when absence is observed after a lost acknowledgment.
    pub fn reconcile_publish(&self, intent: &PublishIntent) -> Outcome<PublishProgress> {
        let result = (|| {
            self.revalidate_publish(intent)?;
            match &intent.stage {
                PublishStage::Upload => bail!(
                    "Upload outcome unknown; no branch/PR writes authorized by reconciliation"
                ),
                PublishStage::CreateRef { commit } | PublishStage::CreatePr { commit } => {
                    self.verify_commit(
                        &intent.preview.destination,
                        commit,
                        &intent.preview.source,
                    )?;
                    ensure!(
                        self.remote_owned_ref(intent)?.as_deref() == Some(&commit.commit_oid),
                        "Owned ref absent/different; retain unknown outcome"
                    );
                    if let Some(pr) = self.find_pr(intent, commit)? {
                        Ok(PublishProgress::Published(Box::new(pr)))
                    } else if matches!(intent.stage, PublishStage::CreateRef { .. }) {
                        Ok(PublishProgress::Next(Box::new(intent.next(
                            PublishStage::CreatePr {
                                commit: commit.clone(),
                            },
                        ))))
                    } else {
                        bail!("PR absent; lost create acknowledgment must not be replayed")
                    }
                }
            }
        })();
        match result {
            Ok(value) => Outcome::Confirmed(value),
            Err(error) => Outcome::Unknown(error.to_string()),
        }
    }
    fn verify_commit(
        &self,
        destination: &Destination,
        receipt: &RemoteCommit,
        source: &HostSource,
    ) -> Result<()> {
        oid(&receipt.commit_oid)?;
        let value: Value =
            self.get(destination, &format!("/git/commits/{}", receipt.commit_oid))?;
        ensure!(
            value["sha"].as_str() == Some(&receipt.commit_oid)
                && value["tree"]["sha"].as_str() == Some(&source.source_tree)
                && value["parents"].as_array().is_some_and(
                    |p| p.len() == 1 && p[0]["sha"].as_str() == Some(&source.base_commit)
                ),
            "Server commit does not bind exact reviewed tree and parent"
        );
        ensure!(
            receipt.source_tree == source.source_tree && receipt.skill_tree == source.skill_tree,
            "Remote commit receipt tree differs"
        );
        ensure!(
            self.skill_tree(destination, &source.source_tree, &source.skill_path)?
                == source.skill_tree,
            "Server skill tree differs"
        );
        Ok(())
    }
    fn skill_tree(&self, destination: &Destination, root: &str, path: &str) -> Result<String> {
        oid(root)?;
        let mut current = root.to_owned();
        for part in path.split('/') {
            let value: Value = self.get(destination, &format!("/git/trees/{current}"))?;
            ensure!(
                value["sha"].as_str() == Some(&current)
                    && value["truncated"].as_bool() == Some(false),
                "Incomplete Git tree"
            );
            let entries = value["tree"].as_array().context("Missing tree entries")?;
            ensure!(entries.len() <= 4096, "Git tree exceeds bound");
            let matches: Vec<_> = entries
                .iter()
                .filter(|e| {
                    e["path"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(part))
                })
                .collect();
            ensure!(
                matches.len() == 1
                    && matches[0]["path"] == part
                    && matches[0]["mode"] == "040000"
                    && matches[0]["type"] == "tree",
                "Missing/aliased/non-tree skill path"
            );
            current = matches[0]["sha"]
                .as_str()
                .context("Missing tree OID")?
                .into();
            oid(&current)?;
        }
        Ok(current)
    }
}

#[derive(Deserialize)]
struct RemoteRef {
    #[serde(rename = "ref")]
    reference: String,
    object: RefObject,
}
#[derive(Deserialize)]
struct RefObject {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}
