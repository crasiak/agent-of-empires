use super::{
    deployment::{Companion, DeployStage},
    publication::HostForge,
    *,
};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Fixture(Child);
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        if crate::process::wait_with_timeout(&mut self.0, Duration::from_secs(10))
            .ok()
            .flatten()
            .is_none()
        {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
struct PublicCli {
    ledger: PathBuf,
    fleet: PathBuf,
    environment: BTreeMap<String, String>,
    store: Store,
    id: String,
    audit_directory: PathBuf,
    import_calls: std::sync::atomic::AtomicUsize,
}
impl PublicCli {
    fn run(&self, binary: &Path, args: &[String]) -> Result<Value> {
        if !args.iter().any(|arg| arg == "capabilities") {
            let row = self.store.get(&self.id)?;
            let d = row
                .deployment
                .as_ref()
                .context("Store intent must precede every helper effect")?;
            d.projection.verify()?;
            let stage = match args[1].as_str() {
                "import-preview" => DeployStage::ImportPreview,
                "import-approve" => DeployStage::ImportApproval,
                "import" => DeployStage::Import,
                "prepare-import" => DeployStage::CandidatePreview,
                "approve" => DeployStage::CandidateApproval,
                "apply" if args[0] == "workshop-rollout" => DeployStage::Rollout,
                "apply" => DeployStage::Apply,
                "materialize" => DeployStage::Materialize,
                "prepare" => DeployStage::RolloutPreview,
                _ => bail!("Unexpected actual public helper verb"),
            };
            ensure!(
                d.intent.stage == stage,
                "Helper called without its own persisted intent"
            );
        }
        let audit = self
            .audit_directory
            .join(format!("backend-{}.json", uuid::Uuid::new_v4()));
        let mut command = if binary == self.fleet {
            let mut command = Command::new(which::which("python3")?);
            command.arg(binary).args(&args[1..]);
            command
        } else {
            let mut command = Command::new(binary);
            command.args(args);
            command
        };
        command
            .env_clear()
            .envs(&self.environment)
            .current_dir(&self.audit_directory);
        if binary == self.fleet {
            command.env("CRA1260_WORKSHOP_FAKE_STATE", &audit);
        }
        let out = crate::process::run_with_bounded_stdout(
            &mut command,
            Duration::from_secs(30),
            8 * 1024 * 1024,
        )?
        .context("Actual fixture CLI timed out")?;
        ensure!(
            out.status.success(),
            "Actual public CLI refused {:?} (exit {:?})",
            args,
            out.status.code()
        );
        if binary == self.fleet {
            let audit: Value = serde_json::from_slice(&std::fs::read(&audit)?)?;
            ensure!(
                audit["backend_calls"] == serde_json::json!([]),
                "Publish-only must never access runtime/native backend"
            );
        }
        if args.get(1).is_some_and(|verb| verb == "import") {
            self.import_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            bail!(
                "Fixture lost host acknowledgement AFTER actual guarded import receipt persisted"
            );
        }
        if out.stdout.is_empty() {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_slice(&out.stdout)?)
        }
    }
}
impl Companion for PublicCli {
    fn binaries(&self) -> (&Path, &Path) {
        (&self.ledger, &self.fleet)
    }
    fn ledger(&self, args: &[String]) -> Result<Value> {
        self.run(&self.ledger, args)
    }
    fn fleet(&self, args: &[String]) -> Result<Value> {
        self.run(&self.fleet, args)
    }
}

pub(super) fn optional_real_cli(store: &Store, request: &Request, host: &impl HostForge) {
    let Some(runner) = std::env::var_os("CRA1260_TEST_LEDGER_FIXTURE") else {
        return;
    };
    eprintln!("CRA1260_ACTUAL_CLI_COMPOSITION_ENTERED: private Go Handler/store + public CLI + no-live fleet parser");
    let ledger = PathBuf::from(
        std::env::var_os("CRA1260_TEST_LEDGER_CLI").expect("explicit actual CLI required"),
    )
    .canonicalize()
    .unwrap();
    let fleet = PathBuf::from(
        std::env::var_os("CRA1260_TEST_FLEET_CLI")
            .expect("explicit source fleet entrypoint required"),
    )
    .canonicalize()
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("ready.json");
    let bootstrap_home = directory.path().join("bootstrap-home");
    std::fs::create_dir(&bootstrap_home).unwrap();
    let mut child = Command::new(runner)
        .args([
            "-test.run",
            "^TestWorkshopPrivateDaemonFixture$",
            "-test.timeout=30m",
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", &bootstrap_home)
        .env("XDG_CONFIG_HOME", bootstrap_home.join("config"))
        .env("XDG_DATA_HOME", bootstrap_home.join("data"))
        .current_dir(directory.path())
        .env("CRA1260_WORKSHOP_SERVE_READY", &ready)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
        .env("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let fixture = Fixture(child);
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.is_ok_and(|line| line == "CRA1260_WORKSHOP_FIXTURE_READY") {
                let _ = tx.send(());
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(30))
        .expect("actual Handler/store readiness marker");
    let ready: Value = serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
    assert_eq!(ready["schema"], "cra1260.workshop-test-daemon.v1");
    let bin = directory.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for name in ["aoe", "tmux"] {
        let path = bin.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 99\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let mut environment: BTreeMap<String, String> =
        serde_json::from_value(ready["env"].clone()).unwrap();
    environment.insert(
        "PATH".into(),
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    environment.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
    environment.insert(
        "XDG_CONFIG_HOME".into(),
        format!("{}/config", environment["HOME"]),
    );
    environment.insert(
        "XDG_DATA_HOME".into(),
        format!("{}/data", environment["HOME"]),
    );
    let companion = PublicCli {
        ledger,
        fleet,
        environment,
        store: store.clone(),
        id: request.id.clone(),
        audit_directory: directory.path().into(),
        import_calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let mut row = store
        .deployment_with(
            Operation::PreviewImport {
                id: request.id.clone(),
                revision: request.revision,
                source_name: "fixture".into(),
                targets: serde_json::from_value(ready["targets"].clone()).unwrap(),
            },
            Some(&companion),
            Some(host),
        )
        .unwrap();
    assert_eq!(row.state, State::ImportPreview, "{:?}", row.error);
    let frozen = row.deployment.as_ref().unwrap().projection.clone();
    let digest = row
        .deployment
        .as_ref()
        .unwrap()
        .import_preview
        .as_ref()
        .unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    row = store
        .deployment_with(
            Operation::ApproveImport {
                id: row.id,
                revision: row.revision,
                preview_digest: digest,
            },
            Some(&companion),
            None,
        )
        .unwrap();
    assert_eq!(row.state, State::DeployUnknown, "{:?}", row.error);
    let reopened = store.get(&row.id).unwrap();
    row = store
        .deployment_with(
            Operation::ReconcileDeploy {
                id: reopened.id,
                revision: reopened.revision,
            },
            None::<&PublicCli>,
            None,
        )
        .unwrap();
    assert_eq!(row.state, State::Imported, "{:?}", row.error);
    assert_eq!(
        companion
            .import_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "Reconcile must inspect the real receipt, never invoke import again"
    );
    row = store
        .deployment_with(
            Operation::PreviewDeploy {
                id: row.id,
                revision: row.revision,
            },
            Some(&companion),
            None,
        )
        .unwrap();
    assert_eq!(row.state, State::CandidatePreview, "{:?}", row.error);
    let digest = row.deployment.as_ref().unwrap().candidate.as_ref().unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    row = store
        .deployment_with(
            Operation::Deploy {
                id: row.id,
                revision: row.revision,
                candidate_digest: digest.clone(),
            },
            Some(&companion),
            None,
        )
        .unwrap();
    assert_eq!(row.state, State::PublishedConfig, "{:?}", row.error);
    row = store
        .deployment_with(
            Operation::Materialize {
                id: row.id,
                revision: row.revision,
                candidate_digest: digest,
            },
            Some(&companion),
            None,
        )
        .unwrap();
    assert_eq!(row.state, State::Materialized, "{:?}", row.error);
    row = store
        .deployment_with(
            Operation::PreviewRollout {
                id: row.id,
                revision: row.revision,
            },
            Some(&companion),
            None,
        )
        .unwrap();
    assert_eq!(row.state, State::RolloutPreview, "{:?}", row.error);
    let attempt = row.deployment.as_ref().unwrap().rollouts.last().unwrap();
    assert_eq!(
        attempt.plan["targets"],
        serde_json::json!([]),
        "empty sessions are publish-only, never all"
    );
    let plan_digest = attempt.plan_digest.clone();
    row = store
        .deployment_with(
            Operation::ApplyRollout {
                id: row.id,
                revision: row.revision,
                plan_digest,
            },
            Some(&companion),
            None,
        )
        .unwrap();
    assert_eq!(row.state, State::RolloutComplete, "{:?}", row.error);
    let reopened = store.get(&row.id).unwrap();
    assert_eq!(
        reopened
            .deployment
            .as_ref()
            .unwrap()
            .projection
            .source_root(),
        frozen.source_root()
    );
    frozen.verify().unwrap();
    super::artifacts::check_real_completed_receipt_fences(reopened.deployment.as_ref().unwrap());
    let snapshot = frozen.source_root().join("SKILL.md");
    let original = std::fs::read(&snapshot).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(&snapshot, b"changed after approval").unwrap();
    assert!(
        store.get(&reopened.id).is_err(),
        "Reopen must reject changed durable projection, never regenerate it"
    );
    std::fs::write(&snapshot, original).unwrap();
    drop(fixture);
    reader.join().unwrap();
    eprintln!("CRA1260_ACTUAL_CLI_COMPOSITION_COMPLETED: v2 import/publication/materialization; backend_calls=[]; exact intent/receipt recovery");
}
