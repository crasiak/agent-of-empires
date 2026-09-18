//! Metadata-only bridge to Ledger's restart journal.

use std::process::Command;
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

use super::launch_identity::{valid_pane_id, LaunchReport, Launcher};

pub const INTENT_ENV: &str = "LEDGER_RESTART_INTENT";
const MAX_OUTPUT: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerLaunchReport {
    pub instance_id: String,
    pub pane_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_intent: Option<String>,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_alphanumeric() || i > 0 && matches!(b, b'_' | b'-' | b'.' | b':')
        })
}

impl LedgerLaunchReport {
    pub fn encode(&self) -> anyhow::Result<String> {
        super::validate_instance_id(&self.instance_id)?;
        anyhow::ensure!(
            valid_pane_id(&self.pane_id) && self.pane_id.len() <= 32,
            "invalid tmux pane ID"
        );
        anyhow::ensure!(valid_id(&self.run_id), "invalid Ledger run ID");
        anyhow::ensure!(
            self.restart_intent.as_deref().is_none_or(valid_id),
            "invalid Ledger intent ID"
        );
        Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?))
    }

    fn decode(encoded: &str, instance_id: &str, pane_id: &str) -> Option<Self> {
        if encoded.len() > 1024 {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(encoded).ok()?;
        let report: Self = serde_json::from_slice(&bytes).ok()?;
        report.encode().ok()?;
        (report.instance_id == instance_id && report.pane_id == pane_id).then_some(report)
    }
}

fn decode_prior(line: &str, instance_id: &str) -> Option<(LaunchReport, LedgerLaunchReport)> {
    if line.len() > MAX_OUTPUT {
        return None;
    }
    let mut parts = line.trim_end().split('|');
    let pane = parts.next()?;
    let launch = LaunchReport::decode(parts.next()?, pane)?;
    let ledger = LedgerLaunchReport::decode(parts.next()?, instance_id, pane)?;
    if parts.next().is_some()
        || launch.instance_id != instance_id
        || !matches!(
            launch.identity.launcher,
            Launcher::Ledger | Launcher::LedgerHeadroom
        )
    {
        return None;
    }
    Some((launch, ledger))
}

fn record_arguments(
    launch: &LaunchReport,
    ledger: &LedgerLaunchReport,
    prior: Option<&str>,
    resume: Option<&str>,
) -> Option<Vec<String>> {
    if !valid_id(&launch.identity.profile)
        || !prior.is_none_or(valid_id)
        || !resume.is_none_or(valid_id)
    {
        return None;
    }
    let mut args: Vec<String> = [
        "restart",
        "record",
        &launch.identity.agent,
        "--profile",
        &launch.identity.profile,
        "--prior-run",
        &ledger.run_id,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if let Some(prior) = prior {
        args.extend(["--prior-session".into(), prior.into()]);
    }
    if let Some(resume) = resume {
        args.extend(["--resume".into(), resume.into()]);
    } else {
        args.push("--fresh".into());
    }
    args.extend([
        "--reason".into(),
        "manual".into(),
        "--aoe-instance".into(),
        ledger.instance_id.clone(),
        "--tmux-pane".into(),
        ledger.pane_id.clone(),
        "--json".into(),
    ]);
    Some(args)
}

fn decode_intent(bytes: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        intent_id: String,
    }
    if bytes.len() > MAX_OUTPUT {
        return None;
    }
    let reply: Reply = serde_json::from_slice(bytes).ok()?;
    (valid_id(&reply.intent_id) && reply.intent_id.starts_with("restart_"))
        .then_some(reply.intent_id)
}

fn bounded_output(command: &mut Command, timeout: Duration) -> Option<Vec<u8>> {
    let output = crate::process::run_with_bounded_stdout(command, timeout, MAX_OUTPUT).ok()??;
    output.status.success().then_some(output.stdout)
}

fn owned_prior(instance: &super::Instance) -> Option<(LaunchReport, LedgerLaunchReport)> {
    if instance.is_sandboxed() {
        return None;
    }
    let session = instance.tmux_session().ok()?;
    let target = format!("{}:^.0", session.name());
    let output = bounded_output(
        crate::tmux::tmux_command().args([
            "display-message",
            "-p",
            "-t",
            &target,
            "#{pane_id}|#{@aoe_launch_identity}|#{@aoe_ledger_launch}",
        ]),
        Duration::from_millis(500),
    )?;
    decode_prior(std::str::from_utf8(&output).ok()?, &instance.id)
}

pub(crate) fn seal_before_failed_resume_cleanup(instance: &super::Instance) {
    let seal = || {
        let (_, ledger) = owned_prior(instance)?;
        bounded_output(
            Command::new("ledger").args(["incident", "seal", "--run", &ledger.run_id, "--json"]),
            Duration::from_secs(3),
        )
    };
    if seal().is_some() {
        tracing::info!(target: "session.restart", "event=ledger_resume_cleanup_seal status=request_completed native_identity=unverified");
    } else {
        tracing::warn!(target: "session.restart", "event=ledger_resume_cleanup_seal status=unavailable cleanup_policy=unchanged");
    }
}

pub(crate) fn record_before_teardown(
    instance: &super::Instance,
    prior: Option<&str>,
    resume: Option<&str>,
) -> Option<String> {
    let observe = || {
        let (launch, ledger) = owned_prior(instance)?;
        let args = record_arguments(&launch, &ledger, prior, resume)?;
        let output = bounded_output(Command::new("ledger").args(args), Duration::from_secs(3))?;
        decode_intent(&output)
    };
    let intent = observe();
    if let Some(id) = intent.as_ref() {
        tracing::info!(target: "session.restart", intent_id = %id, "event=ledger_restart_intent status=recorded native_identity=unverified");
    } else {
        tracing::warn!(target: "session.restart", "event=ledger_restart_intent status=unavailable restart_policy=unchanged");
    }
    intent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::launch_identity::{LaunchAccount, LaunchIdentity, LaunchReport, Launcher};

    fn prior() -> (LaunchReport, LedgerLaunchReport) {
        (
            LaunchReport {
                instance_id: "1234567890abcdef".into(),
                pane_id: "%17".into(),
                identity: LaunchIdentity {
                    agent: "codex".into(),
                    account: LaunchAccount::Work,
                    launcher: Launcher::LedgerHeadroom,
                    profile: "work-headroom".into(),
                },
            },
            LedgerLaunchReport {
                instance_id: "1234567890abcdef".into(),
                pane_id: "%17".into(),
                run_id: "run-prior".into(),
                restart_intent: None,
            },
        )
    }

    #[test]
    fn ledger_reports_preserve_legacy_report_and_require_matching_ownership() {
        let (launch, mut ledger) = prior();
        let legacy = launch.encode().unwrap();
        assert_eq!(LaunchReport::decode(&legacy, "%17"), Some(launch.clone()));
        let encoded = ledger.encode().unwrap();
        let line = format!("%17|{legacy}|{encoded}");
        assert!(decode_prior(&line, &launch.instance_id).is_some());
        assert!(decode_prior(&line, "another-instance").is_none());
        assert!(decode_prior(&format!("%18|{legacy}|{encoded}"), &launch.instance_id).is_none());
        assert!(decode_prior(&format!("%17|{legacy}|"), &launch.instance_id).is_none());
        ledger.run_id = "bad\nsecret".into();
        assert!(ledger.encode().is_err());
        assert!(decode_prior(&"x".repeat(4097), &launch.instance_id).is_none());
    }

    #[test]
    fn optional_report_command_does_not_change_legacy_cli() {
        use clap::Parser;
        assert!(crate::cli::Cli::try_parse_from([
            "aoe",
            "session",
            "report-launch",
            "--agent",
            "codex",
            "--account",
            "work",
            "--launcher",
            "ledger-headroom",
            "--launch-profile",
            "work"
        ])
        .is_ok());
        assert!(crate::cli::Cli::try_parse_from([
            "aoe",
            "session",
            "report-ledger-launch",
            "--supervised-exec",
            "--run-id",
            "run-next",
            "--restart-intent",
            "restart_abc"
        ])
        .is_ok());
        assert!(
            crate::cli::Cli::try_parse_from(["aoe", "session", "report-ledger-launch"]).is_err()
        );
    }

    #[test]
    fn intent_args_preserve_requested_resume_without_claiming_success() {
        let (launch, ledger) = prior();
        let args = record_arguments(
            &launch,
            &ledger,
            Some("prior-native"),
            Some("desired-native"),
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "restart",
                "record",
                "codex",
                "--profile",
                "work-headroom",
                "--prior-run",
                "run-prior",
                "--prior-session",
                "prior-native",
                "--resume",
                "desired-native",
                "--reason",
                "manual",
                "--aoe-instance",
                "1234567890abcdef",
                "--tmux-pane",
                "%17",
                "--json"
            ]
        );
        let fresh = record_arguments(&launch, &ledger, None, None).unwrap();
        assert!(fresh.contains(&"--fresh".into()));
        assert!(!fresh.contains(&"--resume".into()));
        assert!(record_arguments(&launch, &ledger, None, Some("bad;target")).is_none());
        assert_eq!(
            decode_intent(br#"{"intent_id":"restart_abc"}"#).as_deref(),
            Some("restart_abc")
        );
        assert!(decode_intent(br#"{"intent_id":"bad/path"}"#).is_none());
        assert!(decode_intent(br#"{"intent_id":"restart_abc","body":"secret"}"#).is_none());
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;

    #[test]
    fn helper_kills_descendants_on_inherited_pipe_timeout_and_live_overflow() {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        for script in [
            "sleep 10 & echo $! > \"$1\"; exit 0",
            "sleep 10 & echo $! > \"$1\"; while :; do printf '%01024d' 0; done",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let pid_path = directory.path().join("child.pid");
            let started = std::time::Instant::now();
            let result = bounded_output(
                Command::new("/bin/sh")
                    .args(["-c", script, "fixture"])
                    .arg(&pid_path),
                Duration::from_millis(200),
            );
            let pid = Pid::from_raw(
                std::fs::read_to_string(pid_path)
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap(),
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while kill(pid, None).is_ok() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let gone = kill(pid, None).is_err();
            if !gone {
                let _ = kill(pid, Signal::SIGKILL);
            }
            assert!(gone, "helper left descendant running");
            assert!(result.is_none());
            assert!(started.elapsed() < Duration::from_secs(3));
        }
    }

    #[test]
    fn helper_bounds_output_time_and_discards_private_errors() {
        let output = bounded_output(
            Command::new("/bin/sh").args([
                "-c",
                "printf SECRET_CANARY >&2; printf '{\"intent_id\":\"restart_test\"}'",
            ]),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(decode_intent(&output).as_deref(), Some("restart_test"));
        assert!(!String::from_utf8_lossy(&output).contains("SECRET_CANARY"));
        assert!(bounded_output(
            Command::new("/bin/sh").args(["-c", "printf '%05000d' 0"]),
            Duration::from_secs(1)
        )
        .is_none());
        let started = std::time::Instant::now();
        assert!(bounded_output(
            Command::new("/bin/sh").args(["-c", "exec /bin/sleep 10"]),
            Duration::from_millis(100)
        )
        .is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
