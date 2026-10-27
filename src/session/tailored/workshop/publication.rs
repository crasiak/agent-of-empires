use super::*;
use crate::github::forge::{self, Forge, Transport};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub preview: Option<forge::PublishPreview>,
    pub approval: Option<forge::PublishApproval>,
    pub intent: Option<forge::PublishIntent>,
    pub published: Option<forge::PublishedPr>,
    pub policy: Option<forge::PolicyObservation>,
    pub land_approval: Option<forge::LandApproval>,
    pub land_intent: Option<forge::LandIntent>,
    pub land_execution: Option<forge::LandExecution>,
    pub land_observation_pending: bool,
    pub landed: Option<forge::LandedSource>,
}

pub(super) trait HostForge {
    fn export(
        &self,
        landed: &forge::LandedSource,
        destination: &forge::Destination,
        parent: &std::path::Path,
    ) -> Result<forge::ProjectionReceipt>;
    fn preview(
        &self,
        source: forge::HostSource,
        destination: forge::Destination,
        title: String,
        body: String,
    ) -> Result<forge::PublishPreview>;
    fn prepare_publish(
        &self,
        preview: forge::PublishPreview,
        approval: forge::PublishApproval,
    ) -> Result<forge::PublishIntent>;
    fn publish(
        &self,
        intent: &forge::PublishIntent,
        reconcile: bool,
    ) -> forge::Outcome<forge::PublishProgress>;
    fn policy(&self, published: &forge::PublishedPr) -> Result<forge::PolicyObservation>;
    fn prepare_land(
        &self,
        policy: forge::PolicyObservation,
        approval: forge::LandApproval,
    ) -> Result<forge::LandIntent>;
    fn begin_land(&self, intent: &forge::LandIntent) -> Result<forge::LandExecution>;
    fn land(&self, execution: &forge::LandExecution, reconcile: bool) -> forge::LandOutcome;
}
impl<T: Transport> HostForge for Forge<T> {
    fn export(
        &self,
        landed: &forge::LandedSource,
        destination: &forge::Destination,
        parent: &std::path::Path,
    ) -> Result<forge::ProjectionReceipt> {
        self.export_landed(landed, destination, parent)
    }
    fn preview(
        &self,
        source: forge::HostSource,
        destination: forge::Destination,
        title: String,
        body: String,
    ) -> Result<forge::PublishPreview> {
        self.preview_publish(source, destination, title, body)
    }
    fn prepare_publish(
        &self,
        preview: forge::PublishPreview,
        approval: forge::PublishApproval,
    ) -> Result<forge::PublishIntent> {
        self.prepare_publish(preview, approval)
    }
    fn publish(
        &self,
        intent: &forge::PublishIntent,
        reconcile: bool,
    ) -> forge::Outcome<forge::PublishProgress> {
        if reconcile {
            self.reconcile_publish(intent)
        } else {
            self.publish(intent)
        }
    }
    fn policy(&self, published: &forge::PublishedPr) -> Result<forge::PolicyObservation> {
        self.observe_policy(published)
    }
    fn prepare_land(
        &self,
        policy: forge::PolicyObservation,
        approval: forge::LandApproval,
    ) -> Result<forge::LandIntent> {
        self.prepare_land(policy, approval)
    }
    fn begin_land(&self, intent: &forge::LandIntent) -> Result<forge::LandExecution> {
        self.begin_land(intent)
    }
    fn land(&self, execution: &forge::LandExecution, reconcile: bool) -> forge::LandOutcome {
        if reconcile {
            self.poll_land(execution)
        } else {
            self.land(execution)
        }
    }
}

impl Store {
    fn host_source(&self, request: &Request) -> Result<forge::HostSource> {
        let actual = source::reconcile(request)?;
        let review = request.review.as_ref().context("Missing frozen Review")?;
        ensure!(
            review.computed_digest()? == review.digest,
            "Frozen Review changed"
        );
        let recorded = request
            .commit
            .as_ref()
            .context("Missing local Commit receipt")?;
        ensure!(
            actual.commit_oid == recorded.commit_oid
                && actual.source_tree == review.source_tree
                && actual.skill_tree == review.skill_tree,
            "Local Commit and frozen Review differ"
        );
        let repository = source::repository(&request.envelope.repository)?;
        Ok(forge::HostSource {
            repository: request.envelope.repository.clone(),
            common_dir: repository.commondir().canonicalize()?,
            request_id: request.id.clone(),
            revision: request.revision,
            review_digest: review.digest.clone(),
            base_commit: review.base_commit.clone(),
            commit_oid: actual.commit_oid,
            source_tree: actual.source_tree,
            skill_tree: actual.skill_tree,
            skill_path: format!(".agents/skills/{}", request.envelope.slug),
            content_digest: forge::HostSource::reviewed_content_digest(
                &request.envelope.repository,
                &review.skill_tree,
            )?,
            account: match request.envelope.account {
                CaptureAccount::Personal => forge::Account::Personal,
                CaptureAccount::Work => forge::Account::Work,
            },
            source_scope: request.envelope.source_scope.clone(),
        })
    }
    pub(super) fn publication_operation(
        &self,
        operation: Operation,
        host: &impl HostForge,
    ) -> Result<Request> {
        let _admission = self.operation_lock(operation.id())?;
        match operation {
            Operation::PreviewPublish {
                id,
                revision,
                remote,
                repository_id,
                base_branch,
                title,
                body,
            } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(matches!(request.state, State::Committed | State::PublishPreview), "Reconcile a local Commit before destination preview; never replace an attempted publication");
                let source = self.host_source(&request)?;
                let destination = forge::Destination::new(&remote, repository_id, &base_branch)?;
                let preview = host.preview(source, destination, title, body)?;
                self.change(&id, |d| {
                    d.revision(revision)?;
                    ensure!(
                        matches!(d.state, State::Committed | State::PublishPreview),
                        "Publication already attempted"
                    );
                    d.publication = Some(Publication {
                        preview: Some(preview),
                        ..Default::default()
                    });
                    d.state = State::PublishPreview;
                    d.revision += 1;
                    d.error = None;
                    Ok(d.clone())
                })
            }
            Operation::Publish {
                id,
                revision,
                preview_digest,
                disclosure_confirmed,
            } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(
                    request.state == State::PublishPreview,
                    "Preview the actual destination before separate Publish approval"
                );
                let preview = request
                    .publication
                    .as_ref()
                    .and_then(|p| p.preview.clone())
                    .context("Missing publication preview")?;
                self.host_source(&request)?;
                let approval = forge::PublishApproval {
                    preview_digest,
                    disclosure_confirmed,
                };
                let intent = host.prepare_publish(preview, approval.clone())?;
                let pending = self.change(&id, |d| {
                    d.revision(revision)?;
                    ensure!(
                        d.state == State::PublishPreview,
                        "Publish already attempted"
                    );
                    let p = d.publication.as_mut().context("Missing publication")?;
                    p.approval = Some(approval);
                    p.intent = Some(intent);
                    d.state = State::Publishing;
                    d.revision += 1;
                    d.error = None;
                    Ok(d.clone())
                })?;
                self.drive_publish(pending, host, false)
            }
            Operation::ContinuePublish { id, revision } => {
                let pending = self.change(&id, |d| {
                    d.revision(revision)?; ensure!(d.state == State::PublishReady, "Only a confirmed unsent next stage can continue; unknown writes cannot replay");
                    d.state = State::Publishing; d.revision += 1; Ok(d.clone())
                })?;
                self.drive_publish(pending, host, false)
            }
            Operation::PreviewLand { id, revision } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(
                    matches!(request.state, State::Published | State::LandPreview),
                    "Only a confirmed owned PR can preview current Land rules"
                );
                let published = request
                    .publication
                    .as_ref()
                    .and_then(|p| p.published.as_ref())
                    .context("Missing actual remote PR receipt")?;
                let policy = host.policy(published)?;
                self.change(&id, |d| {
                    d.revision(revision)?;
                    ensure!(
                        matches!(d.state, State::Published | State::LandPreview),
                        "Land already attempted"
                    );
                    d.publication
                        .as_mut()
                        .context("Missing publication")?
                        .policy = Some(policy);
                    d.state = State::LandPreview;
                    d.revision += 1;
                    d.error = None;
                    Ok(d.clone())
                })
            }
            Operation::Land {
                id,
                revision,
                policy_digest,
                current_base_and_rules,
                exclusive_pr_custody,
            } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(
                    request.state == State::LandPreview,
                    "Observe actual current rules before separate Land approval"
                );
                let policy = request
                    .publication
                    .as_ref()
                    .and_then(|p| p.policy.clone())
                    .context("Missing policy observation")?;
                let approval = forge::LandApproval {
                    policy_digest,
                    method: forge::MergeMethod::Squash,
                    current_base_and_rules,
                    exclusive_pr_custody,
                };
                let intent = host.prepare_land(policy, approval.clone())?;
                let execution = host.begin_land(&intent)?;
                let pending = self.change(&id, |d| {
                    d.revision(revision)?;
                    ensure!(d.state == State::LandPreview, "Land already attempted");
                    let p = d.publication.as_mut().context("Missing publication")?;
                    p.land_approval = Some(approval);
                    p.land_intent = Some(intent);
                    p.land_execution = Some(execution);
                    p.land_observation_pending = true;
                    d.state = State::Landing;
                    d.revision += 1;
                    d.error = None;
                    Ok(d.clone())
                })?;
                self.drive_land(pending, host, false)
            }
            Operation::ReconcilePublication { id, revision } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                match request.state {
                    State::Publishing | State::PublishUnknown => {
                        self.drive_publish(request, host, true)
                    }
                    State::Landing | State::LandPending | State::LandUnknown => {
                        ensure!(!request.publication.as_ref().context("Missing publication")?.land_observation_pending, "Interrupted Land observation has unsettled execution evidence; remote reconciliation and Deploy are blocked, never fall back to the older execution");
                        let observing = self.change(&id, |d| {
                            d.revision(revision)?;
                            d.publication
                                .as_mut()
                                .context("Missing publication")?
                                .land_observation_pending = true;
                            d.revision += 1;
                            Ok(d.clone())
                        })?;
                        self.drive_land(observing, host, true)
                    }
                    _ => bail!(
                        "No uncertain remote intent to reconcile; no remote write was replayed"
                    ),
                }
            }
            _ => bail!("Not a publication operation"),
        }
    }
    fn drive_publish(
        &self,
        mut pending: Request,
        host: &impl HostForge,
        reconcile: bool,
    ) -> Result<Request> {
        for _ in 0..3 {
            let intent = pending
                .publication
                .as_ref()
                .and_then(|p| p.intent.as_ref())
                .context("Missing persisted Publish intent")?;
            let outcome = host.publish(intent, reconcile);
            pending = self.settle(&pending.id, |d| {
                d.revision(pending.revision)?;
                ensure!(d.state == pending.state, "Publish worker superseded");
                let p = d.publication.as_mut().context("Missing publication")?;
                match outcome {
                    forge::Outcome::Confirmed(forge::PublishProgress::Next(next)) => {
                        p.intent = Some(*next);
                        d.state = if reconcile {
                            State::PublishReady
                        } else {
                            State::Publishing
                        };
                        d.error = None;
                    }
                    forge::Outcome::Confirmed(forge::PublishProgress::Published(receipt)) => {
                        p.published = Some(*receipt);
                        d.state = State::Published;
                        d.error = None;
                    }
                    forge::Outcome::Refused(reason) => {
                        d.state = State::PublishUnknown;
                        d.error = Some(format!(
                            "Publish refused; retained stage, reconcile only: {reason}"
                        ));
                    }
                    forge::Outcome::Unknown(reason) => {
                        d.state = State::PublishUnknown;
                        d.error = Some(format!(
                            "Publish unknown; retained stage, reconcile only: {reason}"
                        ));
                    }
                }
                d.revision += 1;
                Ok(d.clone())
            })?;
            if pending.state != State::Publishing || reconcile {
                return Ok(pending);
            }
        }
        Ok(pending)
    }
    fn drive_land(
        &self,
        pending: Request,
        host: &impl HostForge,
        reconcile: bool,
    ) -> Result<Request> {
        let execution = pending
            .publication
            .as_ref()
            .and_then(|p| p.land_execution.as_ref())
            .context("Missing versioned persisted Land execution; old/unfenced attempts cannot reconcile")?;
        let outcome = host.land(execution, reconcile);
        self.settle(&pending.id, |d| {
            d.revision(pending.revision)?;
            ensure!(d.state == pending.state, "Land worker superseded");
            let p = d.publication.as_mut().context("Missing publication")?;
            ensure!(
                p.land_observation_pending,
                "Missing durable Land observation admission"
            );
            p.land_observation_pending = false;
            match outcome {
                forge::LandOutcome::Confirmed(forge::LandProgress::Pending(next)) => {
                    p.land_execution = Some(*next);
                    d.state = State::LandPending;
                    d.error = None;
                }
                forge::LandOutcome::Confirmed(forge::LandProgress::Landed(receipt)) => {
                    p.landed = Some(*receipt);
                    d.state = State::Landed;
                    d.error = None;
                }
                forge::LandOutcome::Uncertain { execution, reason } => {
                    p.land_execution = Some(*execution);
                    d.state = State::LandUnknown;
                    d.error = Some(format!(
                        "Land not proven; execution evidence retained, Deploy halted: {reason}"
                    ));
                }
                forge::LandOutcome::Refused(reason) => {
                    d.state = State::LandUnknown;
                    d.error = Some(format!(
                        "Land not proven; Deploy halted. Reconcile only: {reason}"
                    ));
                }
            }
            d.revision += 1;
            Ok(d.clone())
        })
    }
}
