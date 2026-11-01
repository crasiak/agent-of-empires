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

/// The time all git calls of one decide share: it has a 5 s budget and git must not eat it.
pub struct GitBudget {
    deadline: Instant,
}

impl GitBudget {
    pub fn new(total: Duration) -> GitBudget {
        GitBudget {
            deadline: Instant::now() + total,
        }
    }

    pub fn standard() -> GitBudget {
        GitBudget::new(Duration::from_secs(2))
    }
}

/// How a command ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Output(String),
    Failed,
    /// Ran past the deadline and was killed (or the deadline had already passed).
    TimedOut,
}

/// Run `command`; it is killed if it is still running at `deadline`.
pub(super) fn output_by(mut command: Command, deadline: Instant) -> Outcome {
    if Instant::now() >= deadline {
        return Outcome::TimedOut;
    }
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Outcome::Failed;
    };
    let Some(mut stdout) = child.stdout.take() else {
        return Outcome::Failed;
    };
    // Drain stdout on its own thread so a large result cannot fill the pipe and stall the child.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
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
    let bytes = reader.join().unwrap_or_default();
    match status {
        None => Outcome::TimedOut,
        Some(s) if s.success() => {
            Outcome::Output(String::from_utf8_lossy(&bytes).trim().to_string())
        }
        Some(_) => Outcome::Failed,
    }
}

/// Read-only git. `--no-optional-locks` means these reads never take the index lock, so they cannot block the agent's own git commands.
fn git_outcome(dir: &Path, args: &[&str], budget: &GitBudget) -> Outcome {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args);
    output_by(command, budget.deadline)
}

fn git(dir: &Path, args: &[&str], budget: &GitBudget) -> Option<String> {
    match git_outcome(dir, args, budget) {
        Outcome::Output(out) => Some(out),
        _ => None,
    }
}

/// The git toplevel containing `cwd`, else `cwd` itself.
pub fn repo_root(cwd: &Path, budget: &GitBudget) -> PathBuf {
    git(cwd, &["rev-parse", "--show-toplevel"], budget)
        .map(PathBuf::from)
        .or_else(|| cwd.canonicalize().ok())
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// Lines added plus deleted in the PR so far: since the branch left `origin/HEAD`, else
/// `origin/main`, else its upstream (else since `HEAD`). `None` if git fails or runs out of time,
/// including while looking for the base: a `HEAD` base would under-count.
pub(super) fn diff_lines_with(git: &dyn Fn(&[&str]) -> Outcome) -> Option<u64> {
    let mut base = "HEAD".to_string();
    for candidate in ["origin/HEAD", "origin/main", "@{upstream}"] {
        match git(&["merge-base", "HEAD", candidate]) {
            Outcome::Output(found) => {
                base = found;
                break;
            }
            Outcome::Failed => {}
            Outcome::TimedOut => return None,
        }
    }
    let Outcome::Output(numstat) = git(&["diff", "--numstat", &base]) else {
        return None;
    };
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
fn protected_branches(root: &Path, budget: &GitBudget) -> Vec<String> {
    let mut branches = vec!["main".to_string(), "master".to_string()];
    let default = git(root, &["symbolic-ref", "refs/remotes/origin/HEAD"], budget)
        .and_then(|r| r.strip_prefix("refs/remotes/origin/").map(str::to_string));
    if let Some(default) = default.filter(|d| !d.is_empty() && !branches.contains(d)) {
        branches.push(default);
    }
    branches
}

pub fn context(input: &RunInput, budget: &GitBudget) -> RunContext {
    let root = repo_root(&input.cwd, budget);
    let protected_branches = protected_branches(&root, budget);
    RunContext {
        goal: input.goal.clone(),
        policy: input.policy,
        diff_lines: diff_lines_with(&|args| git_outcome(&root, args, budget)),
        repo_roots: vec![root],
        requests_used: input.requests_used,
        request_limit: input.request_limit,
        max_diff_lines: MAX_DIFF_LINES,
        estimate_diff_lines: None,
        home: std::env::var_os("HOME").map(PathBuf::from),
        protected_branches,
    }
}
