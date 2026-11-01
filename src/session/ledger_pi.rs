//! Native conversation binding for canonical Ledger Pi launches.

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) struct Runtime {
    pub agent_dir: PathBuf,
    pub sessions: PathBuf,
    pub plan: Option<Plan>,
}

const GUARD_CONTRACT: &str = "ledger.run-guard/v1";

#[derive(Clone, Deserialize)]
struct RunGuard {
    contract: String,
    expected_lock: String,
    digest: String,
    conversation_namespace: PathBuf,
}

pub(crate) struct Plan {
    ledger: PathBuf,
    profile: String,
    cwd: PathBuf,
    core: PathBuf,
    guard: RunGuard,
}

type Environment = [(std::ffi::OsString, std::ffi::OsString)];

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn ledger_command(ledger: &Path, environment: &Environment, cwd: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(ledger);
    command
        .env_clear()
        .envs(environment.iter().cloned())
        .current_dir(cwd);
    command
}

fn describe(
    ledger: &Path,
    environment: &Environment,
    profile: &str,
    cwd: &Path,
    arguments: &[String],
) -> Result<Description> {
    let bytes = bounded(
        ledger_command(ledger, environment, cwd)
            .args([
                "launch",
                "describe",
                "--profile",
                profile,
                "--harness",
                "pi",
                "--target",
                "direct",
                "--cwd",
            ])
            .arg(cwd)
            .args(["--run-guard-contract", GUARD_CONTRACT, "--json", "--"])
            .args(arguments),
    )
    .context("Ledger Pi guarded launch inspection failed")?;
    serde_json::from_slice(&bytes).context("Ledger cannot describe a guarded Pi runtime")
}

impl Runtime {
    pub(crate) fn guarded_command(
        &self,
        command: &str,
        environment: &Environment,
    ) -> Result<String> {
        let plan = self
            .plan
            .as_ref()
            .context("Ledger Pi launch has no exact-runtime guard")?;
        let mut words = shell_words::split(command)?;
        ensure!(
            words.len() >= 6
                && words[0] == plan.ledger.to_str().context("Ledger path is not UTF-8")?
                && words[1..5] == ["run", "pi", "--profile", plan.profile.as_str()]
                && words[5] == "--",
            "Ledger Pi command changed after qualification"
        );
        let arguments = &words[6..];
        let description = describe(
            &plan.ledger,
            environment,
            &plan.profile,
            &plan.cwd,
            arguments,
        )?;
        let runtime = validate_description(
            &description,
            &plan.profile,
            &plan.cwd,
            &plan.core,
            arguments,
        )?;
        let guard = description
            .run_guard
            .as_ref()
            .context("Ledger omitted the native execution guard")?;
        ensure!(
            guard.expected_lock == plan.guard.expected_lock
                && runtime.sessions == self.sessions
                && guard.conversation_namespace == plan.guard.conversation_namespace,
            "Ledger Pi selection changed after native identity qualification"
        );
        words.splice(5..5, ["--expect-launch".to_owned(), guard.digest.clone()]);
        Ok(words
            .iter()
            .map(|word| super::environment::shell_escape(word))
            .collect::<Vec<_>>()
            .join(" "))
    }
}

pub(crate) fn profile(tool: &str, command: &str) -> Option<String> {
    if tool != "pi" || super::Instance::contains_active_shell_syntax(command) {
        return None;
    }
    let words = shell_words::split(command).ok()?;
    let [program, run, harness, flag, name, separator] = words.as_slice() else {
        return None;
    };
    let path = Path::new(program);
    if (program != "ledger" && !path.is_absolute())
        || path.file_name().and_then(|s| s.to_str()) != Some("ledger")
        || run != "run"
        || harness != "pi"
        || flag != "--profile"
        || separator != "--"
        || name.is_empty()
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return None;
    }
    Some(name.clone())
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
    #[serde(default)]
    run_guard: Option<RunGuard>,
}

fn bounded(command: &mut std::process::Command) -> Result<Vec<u8>> {
    let output =
        crate::process::run_with_bounded_stdout(command, Duration::from_secs(10), 128 * 1024)?
            .context("Ledger Pi qualification timed out or exceeded its output limit")?;
    ensure!(
        output.status.success(),
        "Ledger Pi qualification helper failed ({})",
        output.status
    );
    Ok(output.stdout)
}

fn core(bytes: &[u8], profile: &str, store_root: &Path) -> Result<PathBuf> {
    let lock: serde_json::Value = serde_json::from_slice(bytes)?;
    ensure!(
        matches!(
            lock["schema"].as_str(),
            Some("ledger-profile-lock-v4" | "ledger-profile-lock-v5")
        ) && lock["profile"]["name"] == profile
            && lock["binding"]["harness"] == "pi"
            && lock["binding"]["account_class"] == "pi-home"
            && matches!(
                lock["binding"]["launcher"].as_str(),
                Some("direct" | "headroom-isolated")
            )
            && lock["binding"]["launch"]["share"] == serde_json::json!(["conversation"])
            && lock["binding"]["core"]["mode"] == "immutable",
        "Ledger Pi requires a current direct or Headroom-isolated pi-home profile with conversation-only sharing and an immutable core"
    );
    let path = PathBuf::from(
        lock["binding"]["core"]["path"]
            .as_str()
            .context("Ledger Pi core is missing")?,
    );
    ensure!(
        path.is_absolute() && path.parent() == Some(store_root) && path.canonicalize()? == path,
        "Ledger Pi core must name an immutable store object"
    );
    Ok(path)
}

fn validate_description(
    d: &Description,
    profile: &str,
    cwd: &Path,
    core: &Path,
    forwarded: &[String],
) -> Result<Runtime> {
    ensure!(
        d.ready
            && d.state == "current"
            && d.prerequisites.is_empty()
            && d.profile == profile
            && d.harness == "pi"
            && d.target == "direct"
            && d.cwd == cwd,
        "Ledger Pi launch description is unavailable or mismatched"
    );
    let entry = core
        .join("bin/pi")
        .canonicalize()
        .context("Pinned Ledger Pi entry is unavailable")?;
    let node = core
        .join("bin/node")
        .canonicalize()
        .context("Pinned Ledger Node is unavailable")?;
    ensure!(
        entry.starts_with(core)
            && d.executable == node
            && d.arguments.first().map(String::as_str)
                == Some(entry.to_str().context("Invalid pinned Pi entry")?)
            && d.arguments[1..] == *forwarded,
        "Ledger Pi launch does not match its pinned core"
    );
    let agent_dir = PathBuf::from(
        d.environment
            .get("PI_CODING_AGENT_DIR")
            .context("Ledger Pi native home is missing")?,
    );
    ensure!(
        agent_dir.is_absolute()
            && agent_dir.file_name().is_some_and(|name| name == "runtime")
            && d.environment
                .get("PI_CODING_AGENT_SESSION_DIR")
                .is_some_and(String::is_empty)
            && d.environment
                .get("NODE_OPTIONS")
                .is_some_and(String::is_empty)
            && d.environment
                .get("PI_SKIP_VERSION_CHECK")
                .is_some_and(|s| s == "1"),
        "Ledger Pi environment is not a protected native runtime"
    );
    let sessions = agent_dir
        .join("sessions")
        .canonicalize()
        .context("Ledger Pi native session store is unavailable")?;
    ensure!(
        sessions.is_dir(),
        "Ledger Pi native session store is not a directory"
    );
    let guard = d.run_guard.as_ref().context(
        "Ledger lacks the exact native launch guard; ordinary Describe is not a launch lease",
    )?;
    ensure!(
        guard.contract == GUARD_CONTRACT
            && hash(&guard.digest)
            && hash(&guard.expected_lock)
            && guard.conversation_namespace == sessions,
        "Ledger native execution guard does not match the observed conversation namespace"
    );
    Ok(Runtime {
        agent_dir,
        sessions,
        plan: None,
    })
}

pub(crate) fn qualify(
    ledger: &Path,
    environment: &Environment,
    profile: &str,
    cwd: &Path,
) -> Result<Runtime> {
    ensure!(
        ledger.is_absolute(),
        "Ledger qualification requires the actual selected executable"
    );
    let cwd = cwd.canonicalize()?;
    let lock = bounded(ledger_command(ledger, environment, &cwd).args([
        "lock",
        "show",
        "--profile",
        profile,
        "--harness",
        "pi",
    ]))
    .context("Ledger Pi lock inspection failed")?;
    let store_root = PathBuf::from("/nix/store");
    #[cfg(test)]
    let store_root = test_support::core_root(&cwd).unwrap_or(store_root);
    let core = core(&lock, profile, &store_root)?;
    let lock_value: serde_json::Value = serde_json::from_slice(&lock)?;
    let lock_hash = lock_value["hash"]
        .as_str()
        .filter(|value| hash(value))
        .context("Ledger omitted its immutable Lock hash")?;
    let description = describe(ledger, environment, profile, &cwd, &[])?;
    let mut runtime = validate_description(&description, profile, &cwd, &core, &[])?;
    let guard = description
        .run_guard
        .as_ref()
        .context("Ledger native execution guard is missing")?;
    ensure!(
        guard.expected_lock == lock_hash,
        "Ledger Lock changed during qualification"
    );
    runtime.plan = Some(Plan {
        ledger: ledger.to_owned(),
        profile: profile.to_owned(),
        cwd: cwd.clone(),
        core,
        guard: guard.clone(),
    });
    let isolated = tempfile::tempdir()?;
    std::fs::create_dir(isolated.path().join("pi"))?;
    let help = bounded(
        std::process::Command::new(&description.executable)
            .env_clear()
            .env("HOME", isolated.path())
            .env("NODE_OPTIONS", "")
            .env("PI_SKIP_VERSION_CHECK", "1")
            .env("PI_CODING_AGENT_DIR", isolated.path().join("pi"))
            .env("PI_CODING_AGENT_SESSION_DIR", "")
            .current_dir(&cwd)
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
            ]),
    )
    .context("Ledger Pi pinned core help inspection failed")?;
    let help = std::str::from_utf8(&help).context("Pinned Pi help was not UTF-8")?;
    validate_help(help)?;
    Ok(runtime)
}

fn validate_help(help: &str) -> Result<()> {
    let options = help
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
        .collect::<std::collections::HashSet<_>>();
    for flag in [
        "--session-id",
        "--session",
        "--session-dir",
        "--extension",
        "--no-extensions",
        "--no-tools",
        "--no-mcp",
        "--no-context-files",
        "--no-skills",
        "--no-prompt-templates",
    ] {
        ensure!(
            options.contains(flag),
            "Pinned Pi lacks the native conversation contract: {flag}"
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::{LazyLock, Mutex};
    static ROOTS: LazyLock<Mutex<BTreeMap<PathBuf, PathBuf>>> =
        LazyLock::new(|| Mutex::new(BTreeMap::new()));
    pub(crate) struct CoreRootFixture(PathBuf);
    impl CoreRootFixture {
        pub(crate) fn new(cwd: &Path, root: &Path) -> Self {
            let cwd = cwd.canonicalize().unwrap();
            ROOTS.lock().unwrap().insert(cwd.clone(), root.to_owned());
            Self(cwd)
        }
    }
    impl Drop for CoreRootFixture {
        fn drop(&mut self) {
            ROOTS.lock().unwrap().remove(&self.0);
        }
    }
    pub(super) fn core_root(cwd: &Path) -> Option<PathBuf> {
        ROOTS
            .lock()
            .unwrap()
            .get(&cwd.canonicalize().ok()?)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_pi_help_requires_the_exact_resume_option() {
        let help = "--session-id --session-dir --extension --no-extensions --no-tools --no-mcp --no-context-files --no-skills --no-prompt-templates";
        assert!(
            validate_help(help).is_err(),
            "Namespace selectors do not prove exact transcript resume"
        );
        assert!(validate_help(&format!("{help} [-s, --session <path>]")).is_ok());
    }

    #[test]
    fn ledger_pi_queries_use_the_selected_program_and_environment() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected");
        let other = root.path().join("other");
        for path in [&selected, &other] {
            std::fs::create_dir(path).unwrap();
        }
        let path = selected.join("ledger");
        std::fs::write(&path, "#!/bin/sh\nprintf 'selected:%s' \"$HOME\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let environment = vec![(
            std::ffi::OsString::from("HOME"),
            other.as_os_str().to_owned(),
        )];
        let bytes = bounded(&mut ledger_command(&path, &environment, root.path())).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            format!("selected:{}", other.display())
        );
        std::fs::write(
            &path,
            "#!/bin/sh\nprintf 'private-helper-detail' >&2\nexit 7\n",
        )
        .unwrap();
        let error = bounded(&mut ledger_command(&path, &environment, root.path())).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("exit status: 7"), "{message}");
        assert!(!message.contains("private-helper-detail"), "{message}");
    }

    #[test]
    fn ledger_pi_descriptions_cannot_redirect_native_identity() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let core = root.join("pi-core");
        let runtime = root.join("profile/runtime");
        std::fs::create_dir_all(core.join("bin")).unwrap();
        std::fs::create_dir_all(runtime.join("sessions")).unwrap();
        std::fs::write(core.join("entry.js"), "").unwrap();
        std::fs::write(core.join("bin/node"), "").unwrap();
        symlink(core.join("entry.js"), core.join("bin/pi")).unwrap();
        let valid = serde_json::json!({"profile":"personal", "harness":"pi", "target":"direct", "cwd":root,
            "ready":true,"state":"current","executable":core.join("bin/node"),"arguments":[core.join("entry.js")],
            "environment":{"PI_CODING_AGENT_DIR":runtime,"PI_CODING_AGENT_SESSION_DIR":"","NODE_OPTIONS":"","PI_SKIP_VERSION_CHECK":"1"},
            "run_guard":{"contract":"ledger.run-guard/v1","digest":"b".repeat(64),"expected_lock":"a".repeat(64),"conversation_namespace":runtime.join("sessions")}});
        for (pointer, replacement) in [
            ("/ready", serde_json::json!(false)),
            ("/state", serde_json::json!("stale")),
            ("/profile", serde_json::json!("work")),
            ("/harness", serde_json::json!("claude")),
            ("/run_guard", serde_json::json!(null)),
            ("/run_guard/contract", serde_json::json!("unknown")),
            ("/run_guard/expected_lock", serde_json::json!("invalid")),
            (
                "/run_guard/conversation_namespace",
                serde_json::json!("/other"),
            ),
            ("/cwd", serde_json::json!("/elsewhere")),
            ("/prerequisites", serde_json::json!([["apply"]])),
            ("/executable", serde_json::json!("/bin/sh")),
            ("/arguments", serde_json::json!(["another.js"])),
            (
                "/environment/NODE_OPTIONS",
                serde_json::json!("--require other.js"),
            ),
            (
                "/environment/PI_CODING_AGENT_SESSION_DIR",
                serde_json::json!("/other"),
            ),
        ] {
            let mut changed = valid.clone();
            if pointer == "/prerequisites" {
                changed["prerequisites"] = replacement;
            } else {
                *changed.pointer_mut(pointer).unwrap() = replacement;
            }
            let description = serde_json::from_value(changed).unwrap();
            assert!(
                validate_description(&description, "personal", &root, &core, &[]).is_err(),
                "{pointer}"
            );
        }
        let description = serde_json::from_value(valid).unwrap();
        let result = validate_description(&description, "personal", &root, &core, &[]).unwrap();
        assert_eq!(result.sessions, runtime.join("sessions"));
        let valid_lock = serde_json::json!({"schema":"ledger-profile-lock-v4","profile":{"name":"personal"},"binding":{"harness":"pi","account_class":"pi-home","launcher":"direct","launch":{"share":["conversation"]},"core":{"mode":"immutable","path":core}}});
        for schema in ["ledger-profile-lock-v4", "ledger-profile-lock-v5"] {
            for launcher in ["direct", "headroom-isolated"] {
                let mut lock = valid_lock.clone();
                lock["schema"] = serde_json::json!(schema);
                lock["binding"]["launcher"] = serde_json::json!(launcher);
                assert_eq!(
                    super::core(&serde_json::to_vec(&lock).unwrap(), "personal", &root).unwrap(),
                    core,
                    "{schema}/{launcher}"
                );
            }
        }
        for (pointer, replacement) in [
            ("/schema", serde_json::json!("ledger-profile-lock-v3")),
            ("/schema", serde_json::json!("ledger-profile-lock-v6")),
            ("/binding/core/mode", serde_json::json!("mutable")),
            ("/binding/launcher", serde_json::json!("shared")),
            ("/binding/launcher", serde_json::json!("headroom")),
            ("/binding/launch/share", serde_json::json!([])),
            (
                "/binding/launch/share",
                serde_json::json!(["conversation", "config"]),
            ),
            ("/binding/harness", serde_json::json!("codex")),
            ("/profile/name", serde_json::json!("work")),
            ("/binding/account_class", serde_json::json!("api-key")),
            ("/binding/core/path", serde_json::json!(root)),
        ] {
            let mut changed = valid_lock.clone();
            *changed.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                super::core(&serde_json::to_vec(&changed).unwrap(), "personal", &root).is_err(),
                "{pointer}"
            );
        }
    }
}
