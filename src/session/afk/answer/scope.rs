//! Deterministic scope checks that can turn an allowed answer into a stop (or a warning under `yolo`).
use super::decide::{Check, CheckResult, RunContext};
use super::menu::{Kind, Menu};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static DEPENDENCY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"\b((npm|pnpm|yarn|bun)\s+(install|i|add)(\s+-\S+)*\s+[^-\s]|cargo\s+(add|update)\b|pip3?\s+install\b|uv\s+(add|pip\s+install)\b|go\s+get\b|(poetry|pdm|bundle)\s+add\b|composer\s+require\b|(Cargo\.(toml|lock)|package(-lock)?\.json|pnpm-lock\.yaml|yarn\.lock|bun\.lockb?|go\.(mod|sum)|pyproject\.toml|uv\.lock|poetry\.lock|Gemfile(\.lock)?|composer\.(json|lock)|requirements[^/\s]*\.txt)\b)"
).unwrap()
});
// A bare `git push` (no refspec) can't be judged without branch context, so it passes.
static PUSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"\bgit\b[^\n;&|]*\bpush\b[^\n;&|]*(\s-[a-zA-Z]*[fd][a-zA-Z]*\b|\s--(force|force-with-lease|force-if-includes|delete|mirror|prune)\b|\s\+\S|\s:\S|[\s:/+](main|master)($|[\s:]))|\bgit\b[^\n;&|]*\bbranch\b[^\n;&|]*\s(-[a-zA-Z]*[dD][a-zA-Z]*|--delete)\b"
).unwrap()
});
static READ_ONLY: &[&str] = &[
    "cat", "ls", "grep", "rg", "head", "tail", "wc", "stat", "file", "less", "diff",
];
static SHELL_META: &[&str] = &[";", "&", "|", ">", "<", "`", "$(", "\n"];

fn under(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| path.starts_with(r))
}

/// Unresolvable paths (`$VAR`, `~user`, `~` without `HOME`) count as outside.
pub(super) fn outside_repo(subject: &str, roots: &[PathBuf], home: Option<&Path>) -> bool {
    let command = subject.split_once(": ").map_or(subject, |(_, c)| c);
    let first = command.split_whitespace().next().unwrap_or("");
    if READ_ONLY.contains(&first) && !SHELL_META.iter().any(|m| command.contains(m)) {
        return false;
    }
    command
        .split(|c: char| c.is_whitespace() || "\"'=><;&|()`".contains(c))
        .any(|t| {
            if t.starts_with('$') {
                return true;
            }
            let p = match t.strip_prefix('~') {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => match home {
                    Some(h) => h.join(rest.trim_start_matches('/')),
                    None => return true,
                },
                Some(_) => return true,
                None => PathBuf::from(t),
            };
            (p.is_absolute() && p != Path::new("/dev/null") && !under(&p, roots))
                || t.split('/').any(|c| c == "..")
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
            result: fail(
                !subject.is_empty()
                    && outside_repo(
                        subject,
                        &run.repo_roots,
                        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
                    ),
            ),
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
