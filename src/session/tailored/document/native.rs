//! Read-only Ledger qualification for the two trusted document launch shapes.
use super::*;
use std::{collections::BTreeMap, path::PathBuf};

pub(crate) struct Runtime {
    pub agent_dir: PathBuf,
    pub sessions: PathBuf,
}

#[derive(Deserialize)]
struct Description {
    profile: String,
    harness: String,
    target: String,
    cwd: PathBuf,
    ready: bool,
    state: String,
    #[serde(default)]
    prerequisites: Vec<Vec<String>>,
    executable: PathBuf,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
}

fn bounded(command: &mut std::process::Command) -> Result<Vec<u8>> {
    let output =
        crate::process::run_with_bounded_stdout(command, Duration::from_secs(10), 128 * 1024)?
            .context("Ledger document qualification timed out or exceeded its output limit")?;
    ensure!(
        output.status.success(),
        "Document Ledger profile is unavailable or stale; provision it explicitly before starting"
    );
    Ok(output.stdout)
}

fn core(bytes: &[u8], account: CaptureAccount, store_root: &Path) -> Result<PathBuf> {
    validate_lock(bytes, account)?;
    let lock: serde_json::Value = serde_json::from_slice(bytes)?;
    ensure!(
        lock["binding"]["launch"]["share"] == serde_json::json!(["conversation"]),
        "Document Ledger profile requires explicit conversation-only native history sharing"
    );
    ensure!(
        lock["binding"]["core"]["mode"] == "immutable",
        "Document Ledger profile requires an immutable pinned Pi core"
    );
    let path = PathBuf::from(
        lock["binding"]["core"]["path"]
            .as_str()
            .context("Missing pinned document Pi core")?,
    );
    ensure!(
        path.is_absolute() && path.starts_with(store_root),
        "Document Pi core must be an absolute immutable store object"
    );
    Ok(path)
}

fn validate_description(
    description: &Description,
    account: CaptureAccount,
    cwd: &Path,
    core: &Path,
) -> Result<Runtime> {
    ensure!(
        description.ready
            && description.state == "current"
            && description.prerequisites.is_empty()
            && description.profile == profile(account)
            && description.harness == "pi"
            && description.target == "direct"
            && description.cwd == cwd,
        "Ledger document launch description is unavailable or mismatched"
    );
    let entry = core
        .join("bin/pi")
        .canonicalize()
        .context("Pinned document Pi entry is unavailable")?;
    let node = core
        .join("bin/node")
        .canonicalize()
        .context("Pinned document Node is unavailable")?;
    ensure!(
        entry.starts_with(core)
            && description.executable == node
            && description.arguments == [entry.to_str().context("Invalid pinned Pi path")?],
        "Ledger document launch does not match its pinned Pi core"
    );
    let agent_dir = PathBuf::from(
        description
            .environment
            .get("PI_CODING_AGENT_DIR")
            .context("Ledger did not describe the document native home")?,
    );
    ensure!(
        agent_dir.is_absolute()
            && agent_dir.file_name().is_some_and(|n| n == "runtime")
            && description
                .environment
                .get("PI_CODING_AGENT_SESSION_DIR")
                .is_some_and(String::is_empty)
            && description
                .environment
                .get("NODE_OPTIONS")
                .is_some_and(String::is_empty)
            && description
                .environment
                .get("PI_SKIP_VERSION_CHECK")
                .is_some_and(|s| s == "1"),
        "Ledger document environment is not a protected Pi runtime"
    );
    let sessions = agent_dir
        .join("sessions")
        .canonicalize()
        .context("Ledger document native session store is unavailable")?;
    ensure!(
        sessions.is_dir(),
        "Ledger document native session store is not a directory"
    );
    Ok(Runtime {
        agent_dir,
        sessions,
    })
}

pub(crate) fn qualify(account: CaptureAccount, cwd: &Path) -> Result<Runtime> {
    #[cfg(test)]
    test_support::qualification_gate(cwd)?;
    let ledger = which::which("ledger").context("Ledger is required for document chat")?;
    let lock = bounded(std::process::Command::new(&ledger).args([
        "lock",
        "show",
        "--profile",
        profile(account),
        "--harness",
        "pi",
    ]))?;
    let store_root = PathBuf::from("/nix/store");
    #[cfg(test)]
    let store_root = test_support::core_root(cwd).unwrap_or(store_root);
    let core = core(&lock, account, &store_root)?;
    let cwd = cwd.canonicalize()?;
    let bytes = bounded(
        std::process::Command::new(&ledger)
            .args([
                "launch",
                "describe",
                "--profile",
                profile(account),
                "--harness",
                "pi",
                "--target",
                "direct",
                "--cwd",
            ])
            .arg(&cwd)
            .arg("--json"),
    )?;
    let description: Description = serde_json::from_slice(&bytes)
        .context("Ledger cannot describe a ready document runtime")?;
    let runtime = validate_description(&description, account, &cwd, &core)?;
    let isolated = tempfile::tempdir()?;
    std::fs::create_dir(isolated.path().join("pi"))?;
    let mut probe = capability_probe(&description, &cwd, isolated.path());
    let help = bounded(&mut probe)?;
    let help = std::str::from_utf8(&help).context("Pinned Pi help was not UTF-8")?;
    for flag in [
        "--session-id",
        "--session",
        "--extension",
        "--no-tools",
        "--no-mcp",
        "--no-context-files",
        "--no-skills",
        "--no-prompt-templates",
        "--no-extensions",
    ] {
        ensure!(
            help.contains(flag),
            "Pinned Pi does not support the document conversation contract"
        );
    }
    Ok(runtime)
}

fn capability_probe(
    description: &Description,
    cwd: &Path,
    isolated: &Path,
) -> std::process::Command {
    let mut probe = std::process::Command::new(&description.executable);
    probe
        .env_clear()
        .env("HOME", isolated)
        .env("NODE_OPTIONS", "")
        .env("PI_SKIP_VERSION_CHECK", "1")
        .env("PI_CODING_AGENT_DIR", isolated.join("pi"))
        .env("PI_CODING_AGENT_SESSION_DIR", "")
        .current_dir(cwd)
        .args(&description.arguments)
        .args([
            "--no-tools",
            "--no-mcp",
            "--no-context-files",
            "--no-skills",
            "--no-prompt-templates",
            "--no-extensions",
            "--no-session",
            "--help",
        ]);
    probe
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::{mpsc, LazyLock, Mutex};
    type Gate = (mpsc::Sender<std::thread::ThreadId>, mpsc::Receiver<()>);
    static GATES: LazyLock<Mutex<BTreeMap<PathBuf, Gate>>> =
        LazyLock::new(|| Mutex::new(BTreeMap::new()));

    static CORE_ROOTS: LazyLock<Mutex<BTreeMap<PathBuf, PathBuf>>> =
        LazyLock::new(|| Mutex::new(BTreeMap::new()));

    pub(crate) struct CoreRootFixture(PathBuf);
    impl CoreRootFixture {
        pub fn new(cwd: &Path, store_root: &Path) -> Self {
            let cwd = cwd.canonicalize().unwrap();
            CORE_ROOTS
                .lock()
                .unwrap()
                .insert(cwd.clone(), store_root.to_owned());
            Self(cwd)
        }
    }
    impl Drop for CoreRootFixture {
        fn drop(&mut self) {
            CORE_ROOTS.lock().unwrap().remove(&self.0);
        }
    }
    pub(super) fn core_root(cwd: &Path) -> Option<PathBuf> {
        CORE_ROOTS
            .lock()
            .unwrap()
            .get(&cwd.canonicalize().ok()?)
            .cloned()
    }

    pub(crate) struct HoldQualification {
        cwd: PathBuf,
        release: Option<mpsc::Sender<()>>,
        pub arrived: mpsc::Receiver<std::thread::ThreadId>,
    }
    impl HoldQualification {
        pub fn new(cwd: &Path) -> Self {
            let (tx, arrived) = mpsc::channel();
            let (release, rx) = mpsc::channel();
            GATES
                .lock()
                .unwrap()
                .insert(cwd.canonicalize().unwrap(), (tx, rx));
            Self {
                cwd: cwd.canonicalize().unwrap(),
                release: Some(release),
                arrived,
            }
        }
        pub fn release(&mut self) {
            self.release.take().unwrap().send(()).unwrap();
        }
    }
    impl Drop for HoldQualification {
        fn drop(&mut self) {
            self.release.take();
            GATES.lock().unwrap().remove(&self.cwd);
        }
    }
    pub(super) fn qualification_gate(cwd: &Path) -> Result<()> {
        let gate = GATES.lock().unwrap().remove(&cwd.canonicalize()?);
        if let Some((arrived, release)) = gate {
            let _ = arrived.send(std::thread::current().id());
            let _ = release.recv_timeout(Duration::from_secs(10));
            anyhow::bail!("injected launch-time qualification failure");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_capability_probe_cannot_read_runtime_settings_or_auth() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real/runtime");
        let isolated = root.path().join("isolated");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&isolated).unwrap();
        std::fs::write(real.join("auth.json"), "must not read").unwrap();
        std::fs::write(real.join("settings.json"), "must not read").unwrap();
        let description = Description {
            profile: profile(CaptureAccount::Work).into(), harness:"pi".into(), target:"direct".into(),
            cwd:root.path().into(), ready:true, state:"current".into(), prerequisites:vec![],
            executable:"/bin/sh".into(), arguments: vec!["-c".into(),
                "test ! -e \"$PI_CODING_AGENT_DIR/auth.json\" && test ! -e \"$PI_CODING_AGENT_DIR/settings.json\" && printf '%s\n%s\n' \"$HOME\" \"$PI_CODING_AGENT_DIR\"".into()],
            environment:BTreeMap::new(),
        };
        let mut probe = capability_probe(&description, root.path(), &isolated);
        let output = probe.output().unwrap();
        assert!(output.status.success(), "probe accessed the real runtime");
        let output = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            output.lines().collect::<Vec<_>>(),
            vec![
                isolated.to_str().unwrap(),
                isolated.join("pi").to_str().unwrap()
            ]
        );
        assert_eq!(
            std::fs::read_to_string(real.join("auth.json")).unwrap(),
            "must not read"
        );
    }

    #[test]
    fn description_requires_exact_pinned_runtime_identity_and_native_history_share() {
        let root = tempfile::tempdir().unwrap();
        let core = root.path().canonicalize().unwrap().join("core");
        std::fs::create_dir_all(core.join("bin")).unwrap();
        std::fs::write(core.join("bin/pi"), "fixture").unwrap();
        std::fs::write(core.join("bin/node"), "fixture").unwrap();
        let runtime = root.path().canonicalize().unwrap().join("profile/runtime");
        std::fs::create_dir_all(runtime.join("sessions")).unwrap();
        let mut d = Description {
            profile: profile(CaptureAccount::Work).into(),
            harness: "pi".into(),
            target: "direct".into(),
            cwd: root.path().into(),
            ready: true,
            state: "current".into(),
            prerequisites: vec![],
            executable: core.join("bin/node"),
            arguments: vec![core.join("bin/pi").to_str().unwrap().into()],
            environment: [
                (
                    "PI_CODING_AGENT_DIR".into(),
                    runtime.to_str().unwrap().into(),
                ),
                ("PI_CODING_AGENT_SESSION_DIR".into(), "".into()),
                ("NODE_OPTIONS".into(), "".into()),
                ("PI_SKIP_VERSION_CHECK".into(), "1".into()),
            ]
            .into(),
        };
        let qualified = validate_description(&d, CaptureAccount::Work, root.path(), &core).unwrap();
        assert_eq!(qualified.sessions, runtime.join("sessions"));
        assert!(validate_description(&d, CaptureAccount::Personal, root.path(), &core).is_err());
        d.ready = false;
        assert!(validate_description(&d, CaptureAccount::Work, root.path(), &core).is_err());
        d.ready = true;
        d.arguments.push("--tools".into());
        assert!(validate_description(&d, CaptureAccount::Work, root.path(), &core).is_err());
        let mut lock = serde_json::json!({"schema":"ledger-profile-lock-v4", "profile":{"name":"aoe-document-work"}, "binding":{"harness":"pi", "account_class":"pi-home", "launcher":"direct", "core":{"mode":"immutable","path":"/nix/store/test-core"}, "launch":{"share":["conversation"]}},"resources":[{"key":"models/work"}]});
        assert!(super::core(
            &serde_json::to_vec(&lock).unwrap(),
            CaptureAccount::Work,
            Path::new("/nix/store")
        )
        .is_ok());
        lock["binding"]["launch"]["share"] = serde_json::json!(["conversation", "laya"]);
        assert!(super::core(
            &serde_json::to_vec(&lock).unwrap(),
            CaptureAccount::Work,
            Path::new("/nix/store")
        )
        .is_err());
    }
}
