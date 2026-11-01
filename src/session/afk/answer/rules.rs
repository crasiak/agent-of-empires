//! Standing answer rules: TOML files that let AFK answer menus under the `rules` policy.
use super::menu::{Kind, Menu, Role, Source};
use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const RULE_ID_MAX: usize = 128;
const INLINE_RULE_MAX: usize = 1024;
const RULES_FILE_MAX: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleAnswer {
    Role(Role),
    Agent,
    Stop,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    #[serde(default)]
    kind: Option<Kind>,
    #[serde(default)]
    source: Option<Source>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    question_matches: Vec<String>,
    #[serde(default)]
    subject_matches: Vec<String>,
    #[serde(default)]
    has_role: Option<Role>,
    answer: String,
    #[serde(default)]
    allow_dependencies: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    rule: Vec<RawRule>,
}

#[derive(Debug)]
pub struct Rule {
    pub id: String,
    pub kind: Option<Kind>,
    pub source: Option<Source>,
    pub format: Option<String>,
    pub question: Vec<Regex>,
    pub subject: Vec<Regex>,
    pub has_role: Option<Role>,
    pub answer: RuleAnswer,
    pub allow_dependencies: bool,
    pub origin: String,
}

#[derive(Debug, Default)]
pub struct Rules {
    pub rules: Vec<Rule>,
    pub sources: Vec<String>,
    /// SHA-256 over every rule source as loaded: each inline rule, then the user and repo files'
    /// contents or their absence. Consent pins it, so a source edited mid-period is detected.
    pub digest: String,
}

/// Feed one rule source to the digest, length-framed so sources cannot run together.
fn digest_source(digest: &mut Sha256, label: &str, contents: Option<&str>) {
    digest.update(label.as_bytes());
    match contents {
        Some(text) => {
            digest.update(b"\0present\0");
            digest.update((text.len() as u64).to_le_bytes());
            digest.update(text.as_bytes());
        }
        None => digest.update(b"\0absent\0"),
    }
}

pub fn user_rules_file() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config/aoe/afk/rules.toml"))
}

fn parse_answer(s: &str) -> Result<RuleAnswer> {
    Ok(match s {
        "approve" => RuleAnswer::Role(Role::Approve),
        "deny" => RuleAnswer::Role(Role::Deny),
        "revise" => RuleAnswer::Role(Role::Revise),
        "other" => RuleAnswer::Role(Role::Other),
        "agent" => RuleAnswer::Agent,
        "stop" => RuleAnswer::Stop,
        other => bail!("answer {other:?} is not allowed (approve|deny|revise|other|agent|stop)"),
    })
}

impl Rule {
    fn matches(&self, m: &Menu) -> bool {
        self.kind.is_none_or(|k| k == m.kind)
            && self.source.is_none_or(|s| s == m.source)
            && self.format.as_ref().is_none_or(|f| *f == m.format)
            && (self.question.is_empty() || self.question.iter().any(|r| r.is_match(&m.question)))
            && (self.subject.is_empty()
                || m.subject
                    .as_ref()
                    .is_some_and(|s| self.subject.iter().any(|r| r.is_match(s))))
            && self
                .has_role
                .is_none_or(|r| m.options.iter().any(|o| o.role == r))
    }
}

/// Append `rules` to `out`; the first definition of an id wins.
fn add(out: &mut Rules, origin: String, rules: Vec<Rule>) {
    out.sources.push(origin);
    for r in rules {
        if !out.rules.iter().any(|o| o.id == r.id) {
            out.rules.push(r);
        }
    }
}

impl Rules {
    pub fn empty() -> Rules {
        Rules::default()
    }

    pub fn parse(origin: &str, text: &str) -> Result<Vec<Rule>> {
        let file: RawFile =
            toml::from_str(text).with_context(|| format!("{origin}: invalid rules file"))?;
        file.rule
            .into_iter()
            .map(|r| {
                ensure!(
                    !r.id.is_empty() && r.id.len() <= RULE_ID_MAX,
                    "{origin}: rule id required"
                );
                let regexes = |xs: &[String]| {
                    xs.iter()
                        .map(|p| Regex::new(p))
                        .collect::<Result<Vec<_>, _>>()
                        .with_context(|| format!("{origin}: rule {}: invalid regex", r.id))
                };
                Ok(Rule {
                    question: regexes(&r.question_matches)?,
                    subject: regexes(&r.subject_matches)?,
                    answer: parse_answer(&r.answer)
                        .with_context(|| format!("{origin}: rule {}", r.id))?,
                    id: r.id.clone(),
                    kind: r.kind,
                    source: r.source,
                    format: r.format,
                    has_role: r.has_role,
                    allow_dependencies: r.allow_dependencies,
                    origin: origin.to_string(),
                })
            })
            .collect()
    }

    /// Inline first, then user, then repo; the first definition of an id wins.
    pub fn load(
        repo_root: Option<&Path>,
        user_file: Option<&Path>,
        inline: &[String],
    ) -> Result<Rules> {
        let mut out = Rules::default();
        let mut digest = Sha256::new();
        for (i, body) in inline.iter().enumerate() {
            ensure!(
                body.len() <= INLINE_RULE_MAX,
                "inline rule {} too long",
                i + 1
            );
            let origin = format!("inline rule {}", i + 1);
            digest_source(&mut digest, "inline", Some(body));
            // `rule = [{ … }]` is the same array-of-tables as `[[rule]]`, written inline.
            let rules = Self::parse(&origin, &format!("rule = [{{ {body} }}]"))?;
            add(&mut out, origin, rules);
        }
        let files = [
            ("user", user_file.map(Path::to_path_buf)),
            ("repo", repo_root.map(|r| r.join("afk-rules.toml"))),
        ];
        for (label, path) in files {
            let Some(path) = path.filter(|p| p.exists()) else {
                digest_source(&mut digest, label, None);
                continue;
            };
            let origin = path.display().to_string();
            let text =
                std::fs::read_to_string(&path).with_context(|| format!("{origin}: unreadable"))?;
            ensure!(
                text.len() <= RULES_FILE_MAX,
                "{origin}: rules file too large"
            );
            digest_source(&mut digest, label, Some(&text));
            let rules = Self::parse(&origin, &text)?;
            add(&mut out, origin, rules);
        }
        out.digest = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(out)
    }

    /// Stop rules win regardless of order; otherwise the first matching rule.
    pub fn first_match(&self, m: &Menu) -> Option<&Rule> {
        let stop = |r: &&Rule| r.answer == RuleAnswer::Stop;
        self.rules
            .iter()
            .filter(stop)
            .find(|r| r.matches(m))
            .or_else(|| {
                self.rules
                    .iter()
                    .filter(|r| !stop(r))
                    .find(|r| r.matches(m))
            })
    }
}
