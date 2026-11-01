use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const MENU_SCHEMA: &str = "aoe.afk-menu.v1";
pub const RAW_MAX: usize = 4096;
const TEXT_MAX: usize = 2048;
const OPTIONS_MAX: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    AgentText,
    AskUser,
    BashPrompt,
    PiDialog,
    Pane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Plan,
    Choice,
    Permission,
    Confirm,
    Input,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Approve,
    ApproveAlways,
    Deny,
    Revise,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuOption {
    pub key: String,
    pub label: String,
    pub role: Role,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Menu {
    pub schema: String,
    pub source: Source,
    pub format: String,
    pub kind: Kind,
    pub question: String,
    pub subject: Option<String>,
    pub options: Vec<MenuOption>,
    pub free_text: bool,
    pub warnings: Vec<String>,
    pub deadline_ms: Option<u64>,
    pub raw: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputOption {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub role: Option<Role>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuInput {
    pub source: Source,
    pub format: String,
    pub kind: Kind,
    pub question: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub options: Vec<InputOption>,
    #[serde(default)]
    pub free_text: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    #[serde(default)]
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleWords {
    #[serde(default)]
    pub approve: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub revise: Vec<String>,
}
impl Default for RoleWords {
    fn default() -> Self {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
        Self {
            approve: v(&[
                "yes", "y", "approve", "apply", "proceed", "continue", "accept", "ok", "allow",
                "confirm",
            ]),
            deny: v(&[
                "no", "n", "deny", "reject", "skip", "cancel", "abort", "decline", "stop",
            ]),
            revise: v(&[
                "revise",
                "edit",
                "amend",
                "change",
                "keep planning",
                "tell",
                "modify",
            ]),
        }
    }
}

/// "Always"/"all … this session"/"don't ask again" make an approval standing, whatever else the label says.
pub fn role_for(label: &str, words: &RoleWords) -> Role {
    let l = label.trim().to_lowercase();
    let first = l
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or("");
    let has = |ws: &[String]| {
        ws.iter()
            .any(|w| l == *w || first == w || l.starts_with(&format!("{w} ")))
    };
    if has(&words.approve) || l.contains("always") {
        if l.contains("always")
            || l.contains("don't ask")
            || l.contains("do not ask")
            || (l.contains("all") && l.contains("session"))
        {
            return Role::ApproveAlways;
        }
        return Role::Approve;
    }
    if has(&words.deny) {
        return Role::Deny;
    }
    if has(&words.revise) {
        return Role::Revise;
    }
    Role::Other
}

pub(super) fn bounded(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

impl Menu {
    pub fn from_input(input: MenuInput, words: &RoleWords) -> Result<Menu> {
        ensure!(
            !input.format.is_empty() && input.format.len() <= 128,
            "invalid menu format id"
        );
        ensure!(!input.question.trim().is_empty(), "menu question required");
        ensure!(input.options.len() <= OPTIONS_MAX, "too many menu options");
        ensure!(
            input.kind == Kind::Input || !input.options.is_empty(),
            "menu has no options"
        );
        let mut seen = std::collections::HashSet::new();
        for o in &input.options {
            ensure!(
                !o.key.is_empty() && o.key.len() <= 64 && !o.label.trim().is_empty(),
                "invalid menu option"
            );
            ensure!(seen.insert(o.key.clone()), "duplicate menu option key");
        }
        Ok(Menu {
            schema: MENU_SCHEMA.into(),
            source: input.source,
            format: input.format,
            kind: input.kind,
            question: bounded(&input.question, TEXT_MAX),
            subject: input.subject.map(|s| bounded(&s, TEXT_MAX)),
            options: input
                .options
                .into_iter()
                .map(|o| MenuOption {
                    role: o.role.unwrap_or_else(|| role_for(&o.label, words)),
                    label: bounded(&o.label, 256),
                    key: o.key,
                })
                .collect(),
            free_text: input.free_text,
            warnings: input
                .warnings
                .into_iter()
                .take(8)
                .map(|w| bounded(&w, 512))
                .collect(),
            deadline_ms: input.deadline_ms,
            raw: bounded(&input.raw, RAW_MAX),
        })
    }
}
