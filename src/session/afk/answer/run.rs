//! The run an answer belongs to, as the extension describes it, and the repo facts checks need.
use super::decide::{Policy, RunContext};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    pub cwd: PathBuf,
    pub requests_used: u32,
    pub request_limit: u32,
}

/// Read-only git. `--no-optional-locks` means these reads never take the index lock, so they cannot block the agent's own git commands.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The git toplevel containing `cwd`, else `cwd` itself.
pub fn repo_root(cwd: &Path) -> PathBuf {
    git(cwd, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .or_else(|| cwd.canonicalize().ok())
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// Lines added plus deleted since the branch left `origin/HEAD` (else since `HEAD`); `None` if git fails.
fn diff_lines(root: &Path) -> Option<u64> {
    let base = git(root, &["merge-base", "HEAD", "origin/HEAD"]).unwrap_or_else(|| "HEAD".into());
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

pub fn context(input: &RunInput) -> RunContext {
    let root = repo_root(&input.cwd);
    RunContext {
        goal: input.goal.clone(),
        policy: input.policy,
        diff_lines: diff_lines(&root),
        repo_roots: vec![root],
        requests_used: input.requests_used,
        request_limit: input.request_limit,
        max_diff_lines: MAX_DIFF_LINES,
        estimate_diff_lines: None,
    }
}
