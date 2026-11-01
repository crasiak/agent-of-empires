//! Deterministic scope checks that can turn an allowed answer into a stop (or a warning under `yolo`).
//!
//! These are a best-effort deny-list behind the rule's own allow pattern, not a sandbox: a rule
//! should only allow commands it means to allow, and these checks catch common ways out of scope.
use super::decide::{Check, CheckResult, RunContext};
use super::menu::{Kind, Menu};
use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

const MANIFEST_PATTERN: &str = r"\b(Cargo\.(toml|lock)|package(-lock)?\.json|pnpm-lock\.yaml|yarn\.lock|bun\.lockb?|go\.(mod|sum)|pyproject\.toml|uv\.lock|poetry\.lock|Gemfile(\.lock)?|composer\.(json|lock)|requirements[^/\s]*\.txt)\b";
static MANIFEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(MANIFEST_PATTERN).unwrap());
/// A redirect into a manifest: the one way a read-only command writes one.
static MANIFEST_REDIRECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r">>?\s*[^\s;&|]*{MANIFEST_PATTERN}")).unwrap());
/// The quoted message of `git commit -m '…'`: prose, not a command. A double-quoted message that
/// could substitute (`$`, backtick) is left in.
static COMMIT_MESSAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(\bgit\b[^\n;&|'"]*\bcommit\b[^\n;&|'"]*?\s)(--message|-[a-zA-Z]*m)(\s*|=)(?:'[^']*'|"[^"$`]*")"#,
    )
    .unwrap()
});

/// A tool that adds dependencies through some of its subcommands.
struct DependencyTool {
    tools: &'static [&'static str],
    /// The subcommands that add dependencies.
    verbs: &'static [&'static str],
    /// Whether a package argument must follow the verb.
    needs_package: bool,
    /// Whether only the first subcommand counts, as in `cargo test update`. Otherwise any word
    /// before a bare `--` can be the verb.
    first_word: bool,
    /// Subcommands that only group others: `go mod tidy`, `uv pip install`.
    groups: &'static [&'static str],
}

/// Subcommands that hand everything after `--` to another program.
static RUNNERS: &[&str] = &[
    "run",
    "run-script",
    "exec",
    "x",
    "dlx",
    "test",
    "t",
    "start",
];

static DEPENDENCY_TOOLS: &[DependencyTool] = &[
    DependencyTool {
        tools: &["npm", "pnpm", "yarn", "bun"],
        verbs: &["install", "i", "add"],
        needs_package: true,
        first_word: false,
        groups: &[],
    },
    DependencyTool {
        tools: &["npm", "pnpm", "yarn", "bun"],
        verbs: &["update", "upgrade", "up"],
        needs_package: false,
        first_word: false,
        groups: &[],
    },
    DependencyTool {
        tools: &["pip", "pip3"],
        verbs: &["install"],
        needs_package: false,
        first_word: false,
        groups: &[],
    },
    DependencyTool {
        tools: &["cargo"],
        verbs: &["add", "update"],
        needs_package: false,
        first_word: true,
        groups: &[],
    },
    DependencyTool {
        tools: &["uv"],
        verbs: &["install", "add"],
        needs_package: false,
        first_word: true,
        groups: &["pip"],
    },
    // `go mod tidy` rewrites go.mod and go.sum.
    DependencyTool {
        tools: &["go"],
        verbs: &["get", "tidy"],
        needs_package: false,
        first_word: true,
        groups: &["mod"],
    },
    DependencyTool {
        tools: &["poetry", "pdm", "bundle"],
        verbs: &["add", "update"],
        needs_package: false,
        first_word: true,
        groups: &[],
    },
    DependencyTool {
        tools: &["composer"],
        verbs: &["require"],
        needs_package: false,
        first_word: true,
        groups: &[],
    },
];
// A bare `git push` (no refspec) can't be judged without branch context, so it passes.
fn push_regex(protected: &[String]) -> Regex {
    let branches = protected
        .iter()
        .map(|b| regex::escape(b))
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!(
        r#"\bgit\b[^\n;&|]*\bpush\b[^\n;&|]*(\s-[a-zA-Z]*[fd][a-zA-Z]*\b|\s--(force|force-with-lease|force-if-includes|delete|mirror|prune)\b|\s\+\S|\s:\S|[\s:/+'"]({branches})($|[^\w./-]))|\bgit\b[^\n;&|]*\bbranch\b[^\n;&|]*\s(-[a-zA-Z]*[dD][a-zA-Z]*|--delete)\b"#
    ))
    .expect("an escaped branch list is a valid regex")
}
static SEGMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[;&|\n`]|\$\(").unwrap());
// No `rg` (`--pre` runs a command) and no `less` (`!` runs one).
static READ_ONLY: &[&str] = &[
    "cat", "ls", "grep", "head", "tail", "wc", "stat", "file", "diff",
];
/// The one `$(…)` form accepted in a path: the repo's own root.
const REPO_TOPLEVEL: &str = "$(git rev-parse --show-toplevel)";
static SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "eval"];
static FIND_EXEC: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];

/// A command that runs another command: its short and long options that take a separate
/// argument, the options that make it run a shell, and its positional arguments before the command.
struct Wrapper {
    name: &'static str,
    arg_short: &'static str,
    arg_long: &'static [&'static str],
    shell_short: &'static str,
    shell_long: &'static [&'static str],
    positional: usize,
}

static WRAPPERS: &[Wrapper] = &[
    Wrapper {
        name: "timeout",
        arg_short: "sk",
        arg_long: &["--signal", "--kill-after"],
        shell_short: "",
        shell_long: &[],
        positional: 1,
    },
    Wrapper {
        name: "env",
        arg_short: "uC",
        arg_long: &["--unset", "--chdir"],
        shell_short: "S",
        shell_long: &["--split-string"],
        positional: 0,
    },
    Wrapper {
        name: "sudo",
        arg_short: "ugCDhprtUTR",
        arg_long: &[
            "--user",
            "--group",
            "--close-from",
            "--chdir",
            "--host",
            "--prompt",
            "--role",
            "--type",
            "--other-user",
            "--command-timeout",
            "--chroot",
        ],
        shell_short: "si",
        shell_long: &["--shell", "--login"],
        positional: 0,
    },
    Wrapper {
        name: "nice",
        arg_short: "n",
        arg_long: &["--adjustment"],
        shell_short: "",
        shell_long: &[],
        positional: 0,
    },
    Wrapper {
        name: "nohup",
        arg_short: "",
        arg_long: &[],
        shell_short: "",
        shell_long: &[],
        positional: 0,
    },
    Wrapper {
        name: "xargs",
        arg_short: "ILnPsdEa",
        arg_long: &[
            "--max-args",
            "--max-procs",
            "--max-chars",
            "--delimiter",
            "--arg-file",
            "--process-slot-var",
        ],
        shell_short: "",
        shell_long: &[],
        positional: 0,
    },
    Wrapper {
        name: "stdbuf",
        arg_short: "ioe",
        arg_long: &["--input", "--output", "--error"],
        shell_short: "",
        shell_long: &[],
        positional: 0,
    },
];
static SHELL_META: &[&str] = &[";", "&", "|", ">", "<", "`", "$(", "\n"];

/// The command part of a subject such as `Bash command: cargo test`.
fn command_of(subject: &str) -> &str {
    subject.split_once(": ").map_or(subject, |(_, c)| c)
}

/// Words of a command segment, with quotes and grouping characters trimmed. A word ending in
/// `{}` (the `find`/`xargs` placeholder, alone or as in `-I{}`) keeps its braces, so it still
/// counts as an option's argument.
fn words(segment: &str) -> Vec<&str> {
    segment
        .split_whitespace()
        .map(|w| {
            let w = w.trim_matches(|c: char| "\"'".contains(c));
            if w.ends_with("{}") {
                w
            } else {
                w.trim_matches(|c: char| "\"'(){}".contains(c))
            }
        })
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

/// `command` without the messages of `git commit -m '…'`.
fn without_commit_messages(command: &str) -> String {
    let mut command = command.to_string();
    loop {
        let shorter = COMMIT_MESSAGE
            .replace(&command, "${1}${2}${3}X")
            .into_owned();
        if shorter == command {
            return command;
        }
        command = shorter;
    }
}

/// Index in `args` of the word that makes `tool` add dependencies, if any. Only words before a
/// bare `--` count; for `first_word` tools only the first subcommand does. A word right after
/// an option may be that option's value, so it is checked and the scan goes on.
fn dependency_verb(tool: &DependencyTool, args: &[&str]) -> Option<usize> {
    if !tool.first_word {
        // `npm run build -- install` passes the rest to the script; any other `--` only ends
        // the options, so the verb can follow it (`npm -- install x`).
        let runner = args
            .iter()
            .position(|a| *a == "--")
            .filter(|&i| args[..i].iter().any(|a| RUNNERS.contains(a)));
        let scanned = &args[..runner.unwrap_or(args.len())];
        return scanned.iter().position(|a| tool.verbs.contains(a));
    }
    let mut after_option = false;
    for (j, a) in args.iter().enumerate() {
        if *a == "--" {
            // Reached only before the subcommand, where it ends the options: `go -- mod tidy`.
            after_option = false;
            continue;
        }
        if a.starts_with('+') {
            continue; // `cargo +nightly`
        }
        if a.starts_with('-') {
            after_option = !a.contains('=');
            continue;
        }
        if tool.verbs.contains(a) {
            return Some(j);
        }
        if !tool.groups.contains(a) {
            if !after_option {
                return None;
            }
            after_option = false;
        }
    }
    None
}

/// A command segment that writes a manifest: any word naming one, unless the whole command is one
/// plain read-only command (`plain`: no pipe, redirect or chaining) or the word is the value of
/// `--manifest-path`.
fn names_manifest(words: &[&str], plain: bool) -> bool {
    if plain && command_word(words).is_some_and(|(command, _)| READ_ONLY.contains(&command)) {
        return false;
    }
    let mut value = false;
    for w in words {
        if std::mem::take(&mut value) || w.starts_with("--manifest-path=") {
            continue;
        }
        if *w == "--manifest-path" {
            value = true;
        } else if MANIFEST.is_match(w) {
            return true;
        }
    }
    false
}

fn adds_dependency(subject: &str) -> bool {
    // An edit's subject is the file it edits.
    if let Some((tool, path)) = subject.split_once(": ") {
        if matches!(tool, "Edit file" | "Write file") {
            return MANIFEST.is_match(path);
        }
    }
    let command = without_commit_messages(command_of(subject));
    let plain = !SHELL_META.iter().any(|m| command.contains(m));
    MANIFEST_REDIRECT.is_match(&command)
        || segments(&command).any(|w| {
            names_manifest(&w, plain)
                || w.iter().enumerate().any(|(i, tool)| {
                    let tool = basename(tool);
                    DEPENDENCY_TOOLS.iter().any(|t| {
                        t.tools.contains(&tool)
                            && dependency_verb(t, &w[i + 1..]).is_some_and(|j| {
                                !t.needs_package
                                    || w[i + j + 2..].iter().any(|p| !p.starts_with('-'))
                            })
                    })
                })
        })
}

fn under(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| path.starts_with(r))
}

/// The words a wrapper runs as its command, after its options, their arguments and its
/// positionals; `None` when its options make it run a shell (`sudo -s`, `env -S`).
fn wrapped<'a, 'b>(w: &Wrapper, args: &'b [&'a str]) -> Option<&'b [&'a str]> {
    let (mut i, mut positional, mut options) = (0, w.positional, true);
    while let Some(&a) = args.get(i) {
        i += 1;
        if options && a == "--" {
            options = false;
        } else if options && a.starts_with("--") {
            let (name, value) = a.split_once('=').map_or((a, None), |(n, v)| (n, Some(v)));
            if w.shell_long.contains(&name) {
                return None;
            }
            if value.is_none() && w.arg_long.contains(&name) {
                i += 1;
            }
        } else if let Some(cluster) = a.strip_prefix('-').filter(|_| options) {
            // Short options cluster (`-Eu root`); one that takes an argument ends the cluster.
            for (j, c) in cluster.char_indices() {
                if w.shell_short.contains(c) {
                    return None;
                }
                if w.arg_short.contains(c) {
                    if j + c.len_utf8() == cluster.len() {
                        i += 1;
                    }
                    break;
                }
            }
        } else if positional > 0 {
            positional -= 1;
        } else {
            return Some(&args[i - 1..]);
        }
    }
    Some(&[])
}

/// A shell given a repo-relative script file first, rather than code to run (`-c`) or a stream.
/// Where the file is is judged with every other path.
fn runs_script_file(shell: &str, args: &[&str]) -> bool {
    shell != "eval"
        && args.first().is_some_and(|a| {
            !a.starts_with(['-', '+'])
                && a.chars()
                    .all(|c| c.is_alphanumeric() || "._-/+@%".contains(c))
        })
}

/// A segment that runs code we can't inspect (a shell or `eval`, directly, behind a wrapper
/// such as `sudo` or `xargs`, or as a `find -exec` target) or moves to an unresolvable
/// directory (`cd`, `cd ~`, `cd -`, `cd $X`).
fn unresolvable_segment(words: &[&str]) -> bool {
    match command_word(words) {
        Some(("cd", args)) => args
            .first()
            .is_none_or(|t| *t == "~" || *t == "-" || t.starts_with('$')),
        Some(("find", args)) => args
            .iter()
            .enumerate()
            .any(|(i, a)| FIND_EXEC.contains(a) && unresolvable_segment(&args[i + 1..])),
        Some((word, args)) => match WRAPPERS.iter().find(|w| w.name == word) {
            Some(w) => wrapped(w, args).is_none_or(unresolvable_segment),
            None => SHELLS.contains(&word) && !runs_script_file(word, args),
        },
        None => false,
    }
}

/// `path` with `.` and `..` resolved by name alone, so a symlink is not followed.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A token as the shell sees it unquoted: `\x` is `x`, and a lone `\` (a line continuation) is nothing.
fn unescaped(token: &str) -> String {
    let mut out = String::new();
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a `$` in `token` starts an expansion (`a/$X/b`) rather than ending a regex (`s/x$//`).
fn expands(token: &str) -> bool {
    token.match_indices('$').any(|(i, _)| {
        token[i + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || "_@*#?!$-".contains(c))
    })
}

/// Unresolvable paths (`$VAR`, `~user`, `~` without `home`) and unresolvable segments count as
/// outside. A relative path is judged from the repo root, `..` resolved by name.
pub(super) fn outside_repo(subject: &str, roots: &[PathBuf], home: Option<&Path>) -> bool {
    let command = command_of(subject).replace(REPO_TOPLEVEL, ".");
    let first = command.split_whitespace().next().unwrap_or("");
    if READ_ONLY.contains(&first) && !SHELL_META.iter().any(|m| command.contains(m)) {
        return false;
    }
    if segments(&command).any(|w| unresolvable_segment(&w)) {
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
            let t = unescaped(t);
            if t.starts_with('$') || expands(&t) {
                return true;
            }
            let p = match t.strip_prefix('~') {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => match home {
                    Some(h) => h.join(rest.trim_start_matches('/')),
                    None => return true,
                },
                Some(_) => return true,
                None => PathBuf::from(&t),
            };
            let p = match roots.first() {
                Some(root) if !p.is_absolute() => root.join(p),
                _ => p,
            };
            let p = lexical(&p);
            if p.is_absolute() {
                p != Path::new("/dev/null") && !under(&p, roots)
            } else {
                p.components().any(|c| c == Component::ParentDir)
            }
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
    // `main` and `master` are protected whatever the run says.
    let protected: Vec<String> = ["main", "master"]
        .iter()
        .map(|b| b.to_string())
        .chain(run.protected_branches.iter().cloned())
        .collect();
    let limit = run.max_diff_lines.max(run.estimate_diff_lines.unwrap_or(0));
    vec![
        Check {
            id: "harness_warning",
            result: fail(!menu.warnings.is_empty()),
        },
        Check {
            id: "outside_repo",
            result: fail(
                !subject.is_empty() && outside_repo(subject, &run.repo_roots, run.home.as_deref()),
            ),
        },
        Check {
            id: "dependencies",
            result: fail(!allow_dependencies && adds_dependency(subject)),
        },
        Check {
            id: "protected_push",
            result: fail(push_regex(&protected).is_match(subject)),
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
