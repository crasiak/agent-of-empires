//! Deterministic scope checks that can turn an allowed answer into a stop (or a warning under `yolo`).
//!
//! These are a best-effort deny-list behind the rule's own allow pattern, not a sandbox: a rule
//! should only allow commands it means to allow, and these checks catch common ways out of scope.
use super::decide::{Check, CheckResult, RunContext};
use super::menu::{Kind, Menu};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static MANIFEST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"\b(Cargo\.(toml|lock)|package(-lock)?\.json|pnpm-lock\.yaml|yarn\.lock|bun\.lockb?|go\.(mod|sum)|pyproject\.toml|uv\.lock|poetry\.lock|Gemfile(\.lock)?|composer\.(json|lock)|requirements[^/\s]*\.txt)\b"
).unwrap()
});
/// (tool words, verbs that add dependencies, whether a package argument must follow the verb).
static DEPENDENCY_TOOLS: &[(&[&str], &[&str], bool)] = &[
    (
        &["npm", "pnpm", "yarn", "bun"],
        &["install", "i", "add"],
        true,
    ),
    (&["cargo"], &["add", "update"], false),
    (&["pip", "pip3", "uv"], &["install", "add"], false),
    (&["go"], &["get"], false),
    (&["poetry", "pdm", "bundle"], &["add"], false),
    (&["composer"], &["require"], false),
];
// A bare `git push` (no refspec) can't be judged without branch context, so it passes.
static PUSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r#"\bgit\b[^\n;&|]*\bpush\b[^\n;&|]*(\s-[a-zA-Z]*[fd][a-zA-Z]*\b|\s--(force|force-with-lease|force-if-includes|delete|mirror|prune)\b|\s\+\S|\s:\S|[\s:/+'"](main|master)($|[^\w./-]))|\bgit\b[^\n;&|]*\bbranch\b[^\n;&|]*\s(-[a-zA-Z]*[dD][a-zA-Z]*|--delete)\b"#
).unwrap()
});
static SEGMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[;&|\n`]|\$\(").unwrap());
static READ_ONLY: &[&str] = &[
    "cat", "ls", "grep", "rg", "head", "tail", "wc", "stat", "file", "less", "diff",
];
static SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "eval"];
static SHELL_META: &[&str] = &[";", "&", "|", ">", "<", "`", "$(", "\n"];

/// The command part of a subject such as `Bash command: cargo test`.
fn command_of(subject: &str) -> &str {
    subject.split_once(": ").map_or(subject, |(_, c)| c)
}

/// Words of a command segment, with quotes and grouping characters trimmed.
fn words(segment: &str) -> Vec<&str> {
    segment
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| "\"'(){}".contains(c)))
        .filter(|w| !w.is_empty())
        .collect()
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(k, _)| {
        !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Each simple command in `command`, split on `;`, `&`, `|`, newline, backtick and `$(`.
fn segments(command: &str) -> impl Iterator<Item = Vec<&str>> + '_ {
    SEGMENT.split(command).map(words).filter(|w| !w.is_empty())
}

/// The segment's command word (after any `VAR=x` assignments) and the words after it.
fn command_word<'a, 'b>(words: &'b [&'a str]) -> Option<(&'a str, &'b [&'a str])> {
    let i = words.iter().position(|w| !is_assignment(w))?;
    Some((basename(words[i]), &words[i + 1..]))
}

fn adds_dependency(subject: &str) -> bool {
    let command = command_of(subject);
    MANIFEST.is_match(command)
        || segments(command).any(|w| {
            w.iter().enumerate().any(|(i, tool)| {
                let tool = basename(tool);
                DEPENDENCY_TOOLS
                    .iter()
                    .any(|(tools, verbs, needs_package)| {
                        tools.contains(&tool)
                            && w[i + 1..].iter().enumerate().any(|(j, verb)| {
                                verbs.contains(verb)
                                    && (!needs_package
                                        || w[i + j + 2..].iter().any(|p| !p.starts_with('-')))
                            })
                    })
            })
        })
}

fn under(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| path.starts_with(r))
}

/// A segment that runs code we can't inspect (a shell or `eval`) or moves to an unresolvable
/// directory (`cd`, `cd ~`, `cd -`, `cd $X`).
fn unresolvable_segment(words: &[&str]) -> bool {
    match command_word(words) {
        Some(("cd", args)) => args
            .first()
            .is_none_or(|t| *t == "~" || *t == "-" || t.starts_with('$')),
        Some((word, _)) => SHELLS.contains(&word),
        None => false,
    }
}

/// Unresolvable paths (`$VAR`, `~user`, `~` without `HOME`, backslash escapes) and
/// unresolvable segments count as outside.
pub(super) fn outside_repo(subject: &str, roots: &[PathBuf], home: Option<&Path>) -> bool {
    let command = command_of(subject);
    let first = command.split_whitespace().next().unwrap_or("");
    if READ_ONLY.contains(&first) && !SHELL_META.iter().any(|m| command.contains(m)) {
        return false;
    }
    if segments(command).any(|w| unresolvable_segment(&w)) {
        return true;
    }
    command
        .split(|c: char| c.is_whitespace() || "\"'=><;&|()`{},".contains(c))
        .any(|t| {
            // A path glued to a short flag: `-o/etc/x`, `-C/etc`.
            let t = match t.strip_prefix('-') {
                Some(rest) if !rest.starts_with('-') => {
                    rest.trim_start_matches(|c: char| c.is_ascii_alphabetic())
                }
                _ => t,
            };
            if t.starts_with('$') || t.contains('\\') {
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
            result: fail(!allow_dependencies && adds_dependency(subject)),
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
