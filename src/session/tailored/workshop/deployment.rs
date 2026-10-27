use super::*;
use serde_json::{json, Value};
use std::{path::Path, process::Command, time::Duration};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeployTarget {
    pub profile: String,
    pub harness: String,
    pub account: String,
    pub sessions: Vec<String>,
}
impl DeployTarget {
    fn validate_all(targets: &[Self]) -> Result<()> {
        use std::collections::BTreeSet;
        ensure!(
            !targets.is_empty() && targets.len() <= 64,
            "Select explicit profile/harness/account pairs; no common or all-profile fallback"
        );
        let mut pairs = BTreeSet::new();
        let mut sessions = BTreeSet::new();
        for t in targets {
            for name in [&t.profile, &t.account] {
                ensure!(
                    !name.is_empty()
                        && name.len() <= 63
                        && name.as_bytes()[0].is_ascii_lowercase()
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                    "Invalid explicit profile/account name"
                );
            }
            ensure!(
                matches!(t.harness.as_str(), "pi" | "codex" | "claude-code")
                    && pairs.insert((&t.profile, &t.harness)),
                "Unsupported or duplicate profile/harness pair"
            );
            ensure!(t.sessions.len() <= 64, "Too many explicit session targets");
            for id in &t.sessions {
                ensure!(id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) && sessions.insert(id), "Use unique exact AoE session IDs; empty sessions means publish-only, never all sessions");
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeployStage {
    ImportPreview,
    ImportApproval,
    Import,
    CandidatePreview,
    CandidateApproval,
    Apply,
    Materialize,
    RolloutPreview,
    Rollout,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeployIntent {
    pub attempt: String,
    pub stage: DeployStage,
    pub digest: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RolloutAttempt {
    pub directory: PathBuf,
    pub plan_digest: String,
    pub plan: Value,
    pub result: Option<Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub directory: PathBuf,
    pub import_directory: PathBuf,
    pub candidate_directory: PathBuf,
    pub ledger_binary: PathBuf,
    pub fleet_binary: PathBuf,
    pub targets: Vec<DeployTarget>,
    pub landed: Value,
    pub projection: crate::github::forge::ProjectionReceipt,
    pub intent: DeployIntent,
    pub import_preview: Option<Value>,
    pub import_receipt: Option<Value>,
    pub candidate: Option<Value>,
    pub publication_receipt: Option<Value>,
    pub materialize_receipt: Option<Value>,
    pub rollouts: Vec<RolloutAttempt>,
}

pub(super) trait Companion {
    fn binaries(&self) -> (&Path, &Path);
    fn ledger(&self, args: &[String]) -> Result<Value>;
    fn fleet(&self, args: &[String]) -> Result<Value>;
}
pub(super) struct CliCompanion {
    ledger: PathBuf,
    fleet: PathBuf,
}
impl CliCompanion {
    pub fn resolve() -> Result<Self> {
        Ok(Self {
            ledger: which::which("ledger")?.canonicalize()?,
            fleet: which::which("aoe-fleet")?.canonicalize()?,
        })
    }
    fn pinned(deployment: &Deployment) -> Result<Self> {
        let current = Self::resolve()?;
        ensure!(current.ledger == deployment.ledger_binary && current.fleet == deployment.fleet_binary, "Companion binary identity changed; retained intent cannot fall back to another executable");
        Ok(current)
    }
    fn run(binary: &Path, args: &[String]) -> Result<Value> {
        let cwd = tempfile::tempdir()?;
        let mut cmd = Command::new(binary);
        cmd.args(args).current_dir(cwd.path()).env_clear();
        for key in [
            "HOME",
            "PATH",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "LEDGER_HOME",
            "AOE_INSTANCE_ID",
        ] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        cmd.env("NO_COLOR", "1").env("PYTHONDONTWRITEBYTECODE", "1");
        let output = crate::process::run_with_bounded_stdout(
            &mut cmd,
            Duration::from_secs(180),
            8 * 1024 * 1024,
        )?
        .context("Companion outcome unknown: deadline/output bound reached; do not replay")?;
        ensure!(output.status.success(), "Companion refused or outcome unknown; inspect retained private receipts, do not replay");
        if output.stdout.is_empty() {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_slice(&output.stdout)?)
        }
    }
}
impl Companion for CliCompanion {
    fn binaries(&self) -> (&Path, &Path) {
        (&self.ledger, &self.fleet)
    }
    fn ledger(&self, args: &[String]) -> Result<Value> {
        Self::run(&self.ledger, args)
    }
    fn fleet(&self, args: &[String]) -> Result<Value> {
        Self::run(&self.fleet, args)
    }
}
pub(super) fn hash(value: &Value, field: &str) -> Result<String> {
    let value = value[field]
        .as_str()
        .context("Missing producer digest/revision")?;
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Invalid producer digest/revision"
    );
    Ok(value.into())
}
pub(super) fn schema(value: &Value, expected: &str) -> Result<()> {
    ensure!(
        value["schema"] == expected,
        "Unsupported producer schema; provision the current companion explicitly"
    );
    Ok(())
}
pub(super) fn read_artifact(directory: &Path, name: &str) -> Result<Value> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Artifact directory changed"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.mode() & 0o077 == 0 && metadata.uid() == nix::unistd::geteuid().as_raw(),
            "Artifact directory is not private and owned"
        );
    }
    let path = directory.join(name);
    let meta = std::fs::symlink_metadata(&path)?;
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 8 * 1024 * 1024,
        "Receipt is not a bounded regular file"
    );
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 8 * 1024 * 1024, "Receipt exceeds bound");
    Ok(serde_json::from_slice(&bytes)?)
}
fn write_private(path: &Path, value: &Value) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    std::fs::File::open(path.parent().context("Missing artifact parent")?)?.sync_all()?;
    Ok(())
}
fn private_directory(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}

impl Store {
    pub(super) fn deployment_operation(&self, operation: Operation) -> Result<Request> {
        let id = match &operation {
            Operation::PreviewImport { id, .. }
            | Operation::ApproveImport { id, .. }
            | Operation::PreviewDeploy { id, .. }
            | Operation::Deploy { id, .. }
            | Operation::Materialize { id, .. }
            | Operation::PreviewRollout { id, .. }
            | Operation::ApplyRollout { id, .. }
            | Operation::ReconcileDeploy { id, .. } => id,
            _ => bail!("Not a deployment operation"),
        };
        let request = self.get(id)?;
        if matches!(operation, Operation::ReconcileDeploy { .. }) {
            return self.deployment_with(operation, None::<&CliCompanion>, None);
        }
        if let Some(d) = &request.deployment {
            d.projection.verify()?;
        }
        let cli = match &request.deployment {
            Some(d) => CliCompanion::pinned(d)?,
            None => CliCompanion::resolve()?,
        };
        let forge = if matches!(operation, Operation::PreviewImport { .. }) {
            Some(crate::github::forge::Forge::authenticated()?)
        } else {
            None
        };
        self.deployment_with(
            operation,
            Some(&cli),
            forge.as_ref().map(|f| f as &dyn publication::HostForge),
        )
    }
    pub(super) fn deployment_with(
        &self,
        operation: Operation,
        companion: Option<&impl Companion>,
        projector: Option<&dyn publication::HostForge>,
    ) -> Result<Request> {
        let _admission = self.operation_lock(operation.id())?;
        match operation {
            Operation::PreviewImport {
                id,
                revision,
                source_name,
                targets,
            } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(request.state == State::Landed && request.deployment.is_none(), "Only verified actual Landed source can start Deploy; existing attempts cannot be replaced");
                DeployTarget::validate_all(&targets)?;
                text(&source_name, 100)?;
                ensure!(
                    !source_name.trim().is_empty(),
                    "Explicit Ledger source name required"
                );
                let companion = companion.context("Companion unavailable")?;
                qualify(companion)?;
                // The projection contains actual remote object IDs, not a locally reviewed substitute.
                let parent = self
                    .path
                    .parent()
                    .context("Missing store root")?
                    .join("workshop-artifacts");
                std::fs::create_dir_all(&parent)?;
                let directory = parent.join(uuid::Uuid::new_v4().to_string());
                private_directory(&directory)?;
                let directory = directory.canonicalize()?;
                let publication = request
                    .publication
                    .as_ref()
                    .context("Missing publication")?;
                let landed = publication
                    .landed
                    .as_ref()
                    .context("Actual landed receipt required")?;
                let published = publication
                    .published
                    .as_ref()
                    .context("Missing remote identity")?;
                let projection = projector
                    .context("Authenticated landed projection capability unavailable")?
                    .export(landed, &published.destination, &directory)?;
                let landed_value = projection_landed(&projection, landed)?;
                let source_root = projection.source_root();
                let import_directory = directory.join("import");
                let candidate_directory = directory.join("candidate");
                let input = json!({"schema":"cra1260.workshop-import-request.v1", "landed":landed_value, "source_root":source_root, "source_name":source_name, "targets":targets});
                write_private(&directory.join("import-request.json"), &input)?;
                let (ledger_binary, fleet_binary) = companion.binaries();
                let pending = self.change(&id, |d| {
                    d.revision(revision)?;
                    ensure!(
                        d.state == State::Landed && d.deployment.is_none(),
                        "Deployment already started"
                    );
                    d.deployment = Some(Deployment {
                        directory: directory.clone(),
                        import_directory,
                        candidate_directory,
                        ledger_binary: ledger_binary.into(),
                        fleet_binary: fleet_binary.into(),
                        targets,
                        landed: landed_value,
                        projection,
                        intent: DeployIntent {
                            attempt: uuid::Uuid::new_v4().to_string(),
                            stage: DeployStage::ImportPreview,
                            digest: String::new(),
                        },
                        import_preview: None,
                        import_receipt: None,
                        candidate: None,
                        publication_receipt: None,
                        materialize_receipt: None,
                        rollouts: vec![],
                    });
                    d.state = State::Importing;
                    d.revision += 1;
                    d.error = None;
                    Ok(d.clone())
                })?;
                let deploy = pending.deployment.as_ref().context("Missing deployment")?;
                self.run_deploy_stage(
                    pending.clone(),
                    companion,
                    args(&[
                        "workshop-deploy",
                        "import-preview",
                        &directory.join("import-request.json").display().to_string(),
                        &deploy.import_directory.display().to_string(),
                    ]),
                )
            }
            Operation::ApproveImport {
                id,
                revision,
                preview_digest,
            } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(
                    request.state == State::ImportPreview,
                    "Read all current consumers and approve the exact import preview separately"
                );
                let d = request.deployment.as_ref().context("Missing deployment")?;
                ensure!(
                    hash(
                        d.import_preview.as_ref().context("Missing preview")?,
                        "digest"
                    )? == preview_digest,
                    "Import approval digest changed"
                );
                let companion = companion.context("Companion unavailable")?;
                qualify(companion)?;
                let pending = self.begin_deploy(
                    &request,
                    DeployStage::ImportApproval,
                    &preview_digest,
                    State::Importing,
                )?;
                let d = pending.deployment.as_ref().context("Missing deployment")?;
                let approval = companion.ledger(&args(&[
                    "workshop-deploy",
                    "import-approve",
                    &d.import_directory.display().to_string(),
                    &preview_digest,
                ]));
                if let Err(error) = approval {
                    return self.deploy_unknown(&pending, error);
                }
                let pending = self
                    .begin_deploy(
                        &pending,
                        DeployStage::Import,
                        &preview_digest,
                        State::Importing,
                    )
                    .map_err(uncertain)?;
                let d = pending.deployment.as_ref().context("Missing deployment")?;
                self.run_deploy_stage(
                    pending.clone(),
                    companion,
                    args(&[
                        "workshop-deploy",
                        "import",
                        &d.import_directory.display().to_string(),
                        &preview_digest,
                    ]),
                )
            }
            Operation::PreviewDeploy { id, revision } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(
                    request.state == State::Imported,
                    "A confirmed guarded import is required before complete candidate preview"
                );
                let companion = companion.context("Companion unavailable")?;
                qualify(companion)?;
                let pending = self.begin_deploy(
                    &request,
                    DeployStage::CandidatePreview,
                    "",
                    State::Importing,
                )?;
                let d = pending.deployment.as_ref().context("Missing deployment")?;
                self.run_deploy_stage(
                    pending.clone(),
                    companion,
                    args(&[
                        "workshop-deploy",
                        "prepare-import",
                        &d.import_directory.display().to_string(),
                        &d.candidate_directory.display().to_string(),
                    ]),
                )
            }
            Operation::Deploy {
                id,
                revision,
                candidate_digest,
            } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(request.state == State::CandidatePreview, "Approve the complete original post-import config candidate, not a moving checkout");
                let d = request.deployment.as_ref().context("Missing deployment")?;
                ensure!(
                    hash(d.candidate.as_ref().context("Missing candidate")?, "digest")?
                        == candidate_digest,
                    "Candidate approval changed"
                );
                let companion = companion.context("Companion unavailable")?;
                qualify(companion)?;
                let pending = self.begin_deploy(
                    &request,
                    DeployStage::CandidateApproval,
                    &candidate_digest,
                    State::Applying,
                )?;
                let d = pending.deployment.as_ref().context("Missing deployment")?;
                if let Err(error) = companion.ledger(&args(&[
                    "workshop-deploy",
                    "approve",
                    &d.candidate_directory.display().to_string(),
                    &candidate_digest,
                ])) {
                    return self.deploy_unknown(&pending, error);
                }
                let pending = self
                    .begin_deploy(
                        &pending,
                        DeployStage::Apply,
                        &candidate_digest,
                        State::Applying,
                    )
                    .map_err(uncertain)?;
                let d = pending.deployment.as_ref().context("Missing deployment")?;
                self.run_deploy_stage(
                    pending.clone(),
                    companion,
                    args(&[
                        "workshop-deploy",
                        "apply",
                        &d.candidate_directory.display().to_string(),
                        &candidate_digest,
                    ]),
                )
            }
            Operation::Materialize {
                id,
                revision,
                candidate_digest,
            } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(request.state == State::PublishedConfig, "Confirmed original atomic publication acknowledgement required before materialization");
                let d = request.deployment.as_ref().context("Missing deployment")?;
                ensure!(
                    hash(d.candidate.as_ref().context("Missing candidate")?, "digest")?
                        == candidate_digest,
                    "Candidate digest changed"
                );
                let companion = companion.context("Companion unavailable")?;
                qualify(companion)?;
                let pending = self.begin_deploy(
                    &request,
                    DeployStage::Materialize,
                    &candidate_digest,
                    State::Materializing,
                )?;
                let d = pending.deployment.as_ref().context("Missing deployment")?;
                self.run_deploy_stage(
                    pending.clone(),
                    companion,
                    args(&[
                        "workshop-deploy",
                        "materialize",
                        &d.candidate_directory.display().to_string(),
                        &candidate_digest,
                    ]),
                )
            }
            Operation::PreviewRollout { id, revision } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(
                    matches!(
                        request.state,
                        State::Materialized | State::RolloutComplete | State::RolloutPreview
                    ),
                    "Materialized v2 receipt required; unknown attempts cannot retry"
                );
                let companion = companion.context("Companion unavailable")?;
                qualify(companion)?;
                let d = request.deployment.as_ref().context("Missing deployment")?;
                ensure!(d.rollouts.len() < 32, "Rollout history limit reached");
                let digest = hash(d.candidate.as_ref().context("Missing candidate")?, "digest")?;
                let pending = self.begin_deploy(
                    &request,
                    DeployStage::RolloutPreview,
                    &digest,
                    State::RollingOut,
                )?;
                let d = pending.deployment.as_ref().context("Missing deployment")?;
                let output = d.directory.join(format!("rollout-{}", d.intent.attempt));
                self.run_deploy_stage(
                    pending.clone(),
                    companion,
                    args(&[
                        "workshop-rollout",
                        "prepare",
                        "--aoe-dir",
                        &crate::session::get_app_dir()?.display().to_string(),
                        "--ledger",
                        &d.ledger_binary.display().to_string(),
                        "--candidate",
                        &d.candidate_directory
                            .join("candidate.json")
                            .display()
                            .to_string(),
                        "--materialized",
                        &d.candidate_directory
                            .join("materialize-receipt.json")
                            .display()
                            .to_string(),
                        "--digest",
                        &digest,
                        "--output",
                        &output.display().to_string(),
                    ]),
                )
            }
            Operation::ApplyRollout {
                id,
                revision,
                plan_digest,
            } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(request.state == State::RolloutPreview, "Preview and separately approve the exact native plan; waiting idle needs a new explicit preview");
                let d = request.deployment.as_ref().context("Missing deployment")?;
                let rollout = d.rollouts.last().context("Missing native plan")?;
                ensure!(
                    rollout.plan_digest == plan_digest && rollout.result.is_none(),
                    "Native plan changed or already attempted"
                );
                let companion = companion.context("Companion unavailable")?;
                qualify(companion)?;
                let pending = self.begin_deploy(
                    &request,
                    DeployStage::Rollout,
                    &plan_digest,
                    State::RollingOut,
                )?;
                let d = pending.deployment.as_ref().context("Missing deployment")?;
                let rollout = d.rollouts.last().context("Missing native plan")?;
                self.run_deploy_stage(
                    pending.clone(),
                    companion,
                    args(&[
                        "workshop-rollout",
                        "apply",
                        "--aoe-dir",
                        &crate::session::get_app_dir()?.display().to_string(),
                        "--ledger",
                        &d.ledger_binary.display().to_string(),
                        "--queue",
                        &rollout.directory.display().to_string(),
                        "--plan-digest",
                        &plan_digest,
                        "--limit",
                        "1",
                    ]),
                )
            }
            Operation::ReconcileDeploy { id, revision } => {
                let request = self.get(&id)?;
                request.revision(revision)?;
                ensure!(
                    matches!(
                        request.state,
                        State::DeployUnknown
                            | State::Importing
                            | State::Applying
                            | State::Materializing
                            | State::RollingOut
                    ),
                    "No pending deployment intent to reconcile"
                );
                self.finish_deploy(&request)
                    .or_else(|error| self.deploy_unknown(&request, error))
            }
            _ => bail!("Not a deployment operation"),
        }
    }
    fn begin_deploy(
        &self,
        request: &Request,
        stage: DeployStage,
        digest: &str,
        state: State,
    ) -> Result<Request> {
        self.change(&request.id, |d| {
            d.revision(request.revision)?;
            ensure!(d.state == request.state, "Deployment worker superseded");
            d.deployment.as_mut().context("Missing deployment")?.intent = DeployIntent {
                attempt: uuid::Uuid::new_v4().to_string(),
                stage,
                digest: digest.into(),
            };
            d.state = state;
            d.revision += 1;
            d.error = None;
            Ok(d.clone())
        })
    }
    fn deploy_unknown(&self, request: &Request, error: anyhow::Error) -> Result<Request> {
        self.settle(&request.id, |d| { d.revision(request.revision)?; d.state = State::DeployUnknown; d.revision += 1; d.error = Some(format!("Deployment not proven; retained intent/artifacts, read-only reconcile only: {error:#}")); Ok(d.clone()) })
    }
    fn run_deploy_stage(
        &self,
        request: Request,
        companion: &impl Companion,
        args: Vec<String>,
    ) -> Result<Request> {
        let stage = request
            .deployment
            .as_ref()
            .context("Missing deployment")?
            .intent
            .stage;
        let result = if matches!(stage, DeployStage::RolloutPreview | DeployStage::Rollout) {
            companion.fleet(&args)
        } else {
            companion.ledger(&args)
        };
        match result {
            Ok(_) => self
                .finish_deploy(&request)
                .or_else(|e| self.deploy_unknown(&request, e)),
            Err(e) => self.deploy_unknown(&request, e),
        }
    }
    fn finish_deploy(&self, request: &Request) -> Result<Request> {
        let mut d = request.deployment.clone().context("Missing deployment")?;
        d.projection.verify()?;
        let state = super::artifacts::accept_artifact(&mut d)?;
        self.settle(&request.id, |row| {
            row.revision(request.revision)?;
            ensure!(row.state == request.state, "Deployment worker superseded");
            row.deployment = Some(d);
            row.state = state;
            row.revision += 1;
            row.error = None;
            Ok(row.clone())
        })
    }
}

fn qualify(companion: &impl Companion) -> Result<()> {
    for (component, value) in [
        (
            "ledger.workshop-deploy",
            companion.ledger(&args(&["workshop-deploy", "capabilities"]))?,
        ),
        (
            "aoe-fleet.workshop-rollout",
            companion.fleet(&args(&["workshop-rollout", "capabilities"]))?,
        ),
    ] {
        schema(&value, "cra1260.workshop-capabilities.v1")?;
        ensure!(value["component"] == component
            && value["candidate_schema"] == "cra1260.workshop-deploy-candidate.v2"
            && value["publication_receipt_schema"] == "cra1260.workshop-publication-receipt.v2"
            && value["materialize_receipt_schema"] == "cra1260.workshop-materialize-receipt.v2"
            && value["rollout_schema"] == "cra1260.workshop-rollout.v2"
            && value["import_guard_schema"] == "ledger.skill-import-guard.v1"
            && value["import_receipt_schema"] == "ledger.skill-import-receipt.v1"
            && value["guarded_import"] == true && value["scope_field_presence_fence"] == true
            && value["atomic_publication_revision"] == true && value["live_ready"] == false,
            "Current companion protocol is required; no old receipt/epoch fallback or automatic install");
        if component == "ledger.workshop-deploy" {
            ensure!(
                value["existing_daemon_only"] == true,
                "Preview must not bootstrap a daemon"
            );
        } else {
            ensure!(
                value["native_delegation"] == "CRA931"
                    && value["default_apply_limit"] == 1
                    && value["limit_one_supported"] == true,
                "Native effects must remain bounded and CRA-931-owned"
            );
        }
    }
    Ok(())
}

fn projection_landed(
    projection: &crate::github::forge::ProjectionReceipt,
    landed: &crate::github::forge::LandedSource,
) -> Result<Value> {
    projection.verify()?;
    ensure!(
        serde_json::to_value(projection.landed())? == serde_json::to_value(landed)?,
        "Projection authority differs from actual Land receipt"
    );
    let repo = git2::Repository::open_bare(projection.repository())?;
    let mut files = BTreeMap::new();
    fn walk(
        repo: &git2::Repository,
        tree: git2::Oid,
        prefix: &str,
        depth: usize,
        files: &mut BTreeMap<String, (Vec<u8>, bool)>,
    ) -> Result<()> {
        ensure!(depth <= 8, "Landed skill depth exceeds bound");
        for entry in &repo.find_tree(tree)? {
            let name = entry.name().context("Non-UTF8 landed skill path")?;
            let path = format!("{prefix}{name}");
            if entry.kind() == Some(git2::ObjectType::Tree) {
                walk(repo, entry.id(), &format!("{path}/"), depth + 1, files)?;
            } else {
                ensure!(
                    matches!(entry.filemode(), 0o100644 | 0o100755) && files.len() < 32,
                    "Landed skill contains unsupported entries"
                );
                let blob = repo.find_blob(entry.id())?;
                ensure!(blob.size() <= 65_536, "Landed skill blob exceeds bound");
                files.insert(
                    path,
                    (blob.content().to_vec(), entry.filemode() == 0o100755),
                );
            }
        }
        Ok(())
    }
    walk(
        &repo,
        git2::Oid::from_str(&landed.skill_tree)?,
        "",
        0,
        &mut files,
    )?;
    ensure!(
        files.values().map(|(b, _)| b.len()).sum::<usize>() <= 256 * 1024,
        "Landed skill exceeds bound"
    );
    let mut h = Sha256::new();
    let mut part = |bytes: &[u8]| {
        h.update((bytes.len() as u64).to_be_bytes());
        h.update(bytes);
    };
    part(b"resource-tree-v1");
    for (name, (bytes, executable)) in files {
        part(name.as_bytes());
        part(&[u8::from(executable)]);
        part(&bytes);
    }
    let tree_hash: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(
        json!({"repository":projection.repository(),"commit":landed.merged_commit,"source_tree":landed.source_tree,"skill_tree":landed.skill_tree,"path":landed.skill_path,"tree_hash":tree_hash}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Capabilities {
        ledger: Value,
        fleet: Value,
    }
    impl Companion for Capabilities {
        fn binaries(&self) -> (&Path, &Path) {
            (Path::new("/fixture/ledger"), Path::new("/fixture/fleet"))
        }
        fn ledger(&self, args: &[String]) -> Result<Value> {
            ensure!(
                args == ["workshop-deploy", "capabilities"],
                "No effect allowed"
            );
            Ok(self.ledger.clone())
        }
        fn fleet(&self, args: &[String]) -> Result<Value> {
            ensure!(
                args == ["workshop-rollout", "capabilities"],
                "No effect allowed"
            );
            Ok(self.fleet.clone())
        }
    }
    #[test]
    fn actual_public_capability_producers_qualify_and_old_missing_contracts_refuse_before_effects()
    {
        let ledger: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/workshop/ledger-capabilities.json"
        ))
        .unwrap();
        let fleet: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/workshop/fleet-capabilities.json"
        ))
        .unwrap();
        qualify(&Capabilities {
            ledger: ledger.clone(),
            fleet: fleet.clone(),
        })
        .unwrap();
        for field in [
            "schema",
            "candidate_schema",
            "publication_receipt_schema",
            "materialize_receipt_schema",
            "rollout_schema",
            "import_guard_schema",
            "import_receipt_schema",
            "guarded_import",
            "scope_field_presence_fence",
            "atomic_publication_revision",
            "live_ready",
            "existing_daemon_only",
        ] {
            let mut old = ledger.clone();
            old.as_object_mut().unwrap().remove(field);
            assert!(
                qualify(&Capabilities {
                    ledger: old,
                    fleet: fleet.clone()
                })
                .is_err(),
                "{field}"
            );
        }
        for field in [
            "native_delegation",
            "default_apply_limit",
            "limit_one_supported",
        ] {
            let mut old = fleet.clone();
            old.as_object_mut().unwrap().remove(field);
            assert!(
                qualify(&Capabilities {
                    ledger: ledger.clone(),
                    fleet: old
                })
                .is_err(),
                "{field}"
            );
        }
    }
    #[test]
    fn explicit_scope_never_interprets_empty_sessions_as_all_and_rejects_aliases_or_extra_ids() {
        let valid = DeployTarget {
            profile: "selected".into(),
            harness: "pi".into(),
            account: "personal".into(),
            sessions: vec![],
        };
        DeployTarget::validate_all(std::slice::from_ref(&valid)).unwrap();
        assert!(DeployTarget::validate_all(&[]).is_err());
        assert!(DeployTarget::validate_all(&[valid.clone(), valid.clone()]).is_err());
        for sessions in [
            vec!["not-an-id".into()],
            vec!["0123456789abcdef".into(), "0123456789abcdef".into()],
        ] {
            assert!(DeployTarget::validate_all(&[DeployTarget {
                sessions,
                ..valid.clone()
            }])
            .is_err());
        }
        DeployTarget::validate_all(&[DeployTarget {
            sessions: vec!["0123456789abcdef".into()],
            ..valid
        }])
        .unwrap();
    }
}
