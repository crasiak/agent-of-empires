//! Deterministic scope checks that can turn an allowed answer into a stop (or a warning under `yolo`).
use super::decide::{Check, CheckResult, RunContext};
use super::menu::{Kind, Menu};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static DEPENDENCY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"\b((npm|pnpm|yarn|bun)\s+(install|i|add)\s+[^-\s]|cargo\s+add\b|pip3?\s+install\b|uv\s+(add|pip\s+install)\b|go\s+get\b|(Cargo\.(toml|lock)|package(-lock)?\.json|pnpm-lock\.yaml|yarn\.lock|go\.(mod|sum)|pyproject\.toml|uv\.lock|requirements[^/\s]*\.txt)\b)"
).unwrap()
});
static PUSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"git\s+push\b.*(\s(-f|--force|--force-with-lease|--delete|-d)\b|\s\+\S|\s:\S|\s(origin\s+)?(main|master)\b)|git\s+branch\s+(-D|-d|--delete)\b"
).unwrap()
});
static READ_ONLY: &[&str] = &[
    "cat", "ls", "grep", "rg", "head", "tail", "wc", "stat", "file", "less", "diff",
];

fn under(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| path.starts_with(r))
}

fn outside_repo(subject: &str, roots: &[PathBuf]) -> bool {
    let command = subject.split_once(": ").map_or(subject, |(_, c)| c);
    if READ_ONLY.contains(&command.split_whitespace().next().unwrap_or("")) {
        return false;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    command
        .split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '=' || c == '>')
        .any(|t| {
            let p = if let (Some(rest), Some(h)) = (t.strip_prefix("~/"), &home) {
                h.join(rest)
            } else {
                PathBuf::from(t)
            };
            (p.is_absolute() && !under(&p, roots)) || t.split('/').any(|c| c == "..")
        })
}

pub fn run_checks(menu: &Menu, run: &RunContext, allow_dependencies: bool) -> Vec<Check> {
    let subject = menu.subject.as_deref().unwrap_or("");
    let fail = |b: bool| {
        if b {
            CheckResult::Fail
        } else {
            CheckResult::Pass
        }
    };
    let limit = run.max_diff_lines.max(run.estimate_diff_lines.unwrap_or(0));
    vec![
        Check {
            id: "harness_warning",
            result: fail(!menu.warnings.is_empty()),
        },
        Check {
            id: "outside_repo",
            result: fail(!subject.is_empty() && outside_repo(subject, &run.repo_roots)),
        },
        Check {
            id: "dependencies",
            result: fail(!allow_dependencies && DEPENDENCY.is_match(subject)),
        },
        Check {
            id: "protected_push",
            result: fail(PUSH.is_match(subject)),
        },
        Check {
            id: "request_share",
            result: fail(
                menu.kind == Kind::Plan
                    && u64::from(run.requests_used) * 100 > u64::from(run.request_limit) * 80,
            ),
        },
        Check {
            id: "diff_limit",
            result: fail(run.diff_lines.is_some_and(|d| d > limit)),
        },
    ]
}
