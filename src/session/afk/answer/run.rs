//! The run an answer belongs to, as the extension describes it, and the repo facts checks need.
use super::decide::{Policy, RunContext};
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_DIFF_LINES: u64 = 1500;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunInput {
    pub period: String,
    #[serde(default)]
    pub item: Option<String>,
    pub goal: String,
    pub policy: Policy,
    #[serde(default)]
    pub rules: Vec<String>,
    /// The `rules_check` digest the human consented to; `rules` decisions stop if the sources changed.
    #[serde(default)]
    pub rules_digest: Option<String>,
    pub cwd: PathBuf,
    pub requests_used: u32,
    pub request_limit: u32,
}

/// How long one git call may run: a decide makes several inside its 5 s budget.
const GIT_TIMEOUT: Duration = Duration::from_secs(1);

/// Run `command` and return its trimmed stdout if it exits successfully within `timeout`; a
/// command that overruns is killed and counts as failed.
fn output_within(mut command: Command, timeout: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Drain stdout on its own thread so a large result cannot fill the pipe and stall the child.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader.join().ok()?;
    status
        .filter(|s| s.success())
        .map(|_| String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Read-only git. `--no-optional-locks` means these reads never take the index lock, so they cannot block the agent's own git commands.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args);
    output_within(command, GIT_TIMEOUT)
}

/// The git toplevel containing `cwd`, else `cwd` itself.
pub fn repo_root(cwd: &Path) -> PathBuf {
    git(cwd, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .or_else(|| cwd.canonicalize().ok())
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// Lines added plus deleted in the PR so far: since the branch left `origin/HEAD`, else
/// `origin/main`, else its upstream (else since `HEAD`); `None` if git fails.
fn diff_lines(root: &Path) -> Option<u64> {
    let base = ["origin/HEAD", "origin/main", "@{upstream}"]
        .iter()
        .find_map(|base| git(root, &["merge-base", "HEAD", base]))
        .unwrap_or_else(|| "HEAD".into());
    let numstat = git(root, &["diff", "--numstat", &base])?;
    Some(
        numstat
            .lines()
            .filter_map(|line| {
                // Binary files report `-` for both counts.
                let mut fields = line.split('\t');
                Some(
                    fields.next()?.parse::<u64>().unwrap_or(0)
                        + fields.next()?.parse::<u64>().unwrap_or(0),
                )
            })
            .sum(),
    )
}

/// `main` and `master`, plus the repo's default branch when `origin/HEAD` names one.
fn protected_branches(root: &Path) -> Vec<String> {
    let mut branches = vec!["main".to_string(), "master".to_string()];
    let default = git(root, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        .and_then(|r| r.strip_prefix("refs/remotes/origin/").map(str::to_string));
    if let Some(default) = default.filter(|d| !d.is_empty() && !branches.contains(d)) {
        branches.push(default);
    }
    branches
}

#[cfg(test)]
pub(super) fn run_within(command: Command, timeout: Duration) -> Option<String> {
    output_within(command, timeout)
}

pub fn context(input: &RunInput) -> RunContext {
    let root = repo_root(&input.cwd);
    let protected_branches = protected_branches(&root);
    RunContext {
        goal: input.goal.clone(),
        policy: input.policy,
        diff_lines: diff_lines(&root),
        repo_roots: vec![root],
        requests_used: input.requests_used,
        request_limit: input.request_limit,
        max_diff_lines: MAX_DIFF_LINES,
        estimate_diff_lines: None,
        home: std::env::var_os("HOME").map(PathBuf::from),
        protected_branches,
    }
}
