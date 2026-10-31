//! The answer engine: decides under the run's policy whether AFK answers a recognised menu, and with what.
use super::menu::{Kind, Menu, Role};
use super::rules::{RuleAnswer, Rules};
use super::scope::run_checks;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Rules,
    Yolo,
}

#[derive(Clone, Debug)]
pub struct RunContext {
    pub goal: String,
    pub policy: Policy,
    pub repo_roots: Vec<PathBuf>,
    pub requests_used: u32,
    pub request_limit: u32,
    pub diff_lines: Option<u64>,
    pub max_diff_lines: u64,
    pub estimate_diff_lines: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckResult {
    Pass,
    Fail,
    Warn,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub result: CheckResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Answer {
        choice: Option<String>,
        text: Option<String>,
        rule: Option<String>,
        reason: String,
        warnings: Vec<String>,
        checks: Vec<Check>,
    },
    Stop {
        reason: String,
        checks: Vec<Check>,
    },
}

fn stop(reason: impl Into<String>) -> Decision {
    Decision::Stop {
        reason: reason.into(),
        checks: vec![],
    }
}

fn proposed<'a>(menu: &'a Menu, p: Option<&Proposal>) -> Option<&'a super::menu::MenuOption> {
    let key = p?.key.as_deref()?;
    menu.options.iter().find(|o| o.key == key)
}

pub fn decide(
    menu: &Menu,
    run: &RunContext,
    rules: &Rules,
    proposal: Option<&Proposal>,
) -> Decision {
    let text = proposal
        .and_then(|p| p.text.clone())
        .filter(|t| !t.is_empty() && t.len() <= 1024);
    let reason = proposal.and_then(|p| p.reason.clone()).unwrap_or_default();
    match run.policy {
        Policy::Yolo => {
            if menu.kind == Kind::Input && menu.options.is_empty() {
                return match text {
                    Some(t) => Decision::Answer {
                        choice: None,
                        text: Some(t),
                        rule: None,
                        reason,
                        warnings: vec![],
                        checks: vec![],
                    },
                    None => stop("no_text_answer"),
                };
            }
            let option = proposed(menu, proposal)
                .or_else(|| menu.options.iter().find(|o| o.role == Role::Approve))
                .or_else(|| menu.options.first());
            let Some(option) = option else {
                return stop("no_options");
            };
            let mut checks = if option.role == Role::Deny {
                vec![]
            } else {
                run_checks(menu, run, false)
            };
            let mut warnings: Vec<String> = checks
                .iter_mut()
                .filter(|c| c.result == CheckResult::Fail)
                .map(|c| {
                    c.result = CheckResult::Warn;
                    format!("scope:{}", c.id)
                })
                .collect();
            if option.role == Role::ApproveAlways {
                warnings.push("approve_always_chosen".into());
            }
            Decision::Answer {
                choice: Some(option.key.clone()),
                text: None,
                rule: None,
                reason,
                warnings,
                checks,
            }
        }
        Policy::Rules => {
            let Some(rule) = rules.first_match(menu) else {
                return stop("no_rule");
            };
            let option = match &rule.answer {
                RuleAnswer::Stop => return stop(format!("rule_stop:{}", rule.id)),
                RuleAnswer::Agent if menu.kind == Kind::Input && menu.options.is_empty() => {
                    return match text {
                        Some(t) => Decision::Answer {
                            choice: None,
                            text: Some(t),
                            rule: Some(rule.id.clone()),
                            reason,
                            warnings: vec![],
                            checks: vec![],
                        },
                        None => stop("no_proposal"),
                    };
                }
                RuleAnswer::Agent => match proposed(menu, proposal) {
                    Some(o) => o,
                    None => return stop("no_proposal"),
                },
                RuleAnswer::Role(role) => match menu.options.iter().find(|o| o.role == *role) {
                    Some(o) => o,
                    None => return stop("no_option_for_role"),
                },
            };
            if option.role == Role::ApproveAlways {
                return stop("approve_always_refused");
            }
            let checks = if option.role == Role::Deny {
                vec![]
            } else {
                run_checks(menu, run, rule.allow_dependencies)
            };
            if let Some(failed) = checks.iter().find(|c| c.result == CheckResult::Fail) {
                return Decision::Stop {
                    reason: format!("scope:{}", failed.id),
                    checks,
                };
            }
            Decision::Answer {
                choice: Some(option.key.clone()),
                text: None,
                rule: Some(rule.id.clone()),
                reason,
                warnings: vec![],
                checks,
            }
        }
    }
}
