//! Reviewed, single-Session intake. The selected profile owns the agent and account.

use anyhow::{ensure, Context, Result};

const POLICY: &str = "AoE reviewed intake v1. This is the task Session, not a concierge or permission to delegate. Refine the supplied brief, task and initial prompt using the selected repository's instructions. Ask about unresolved requirements or access before acting. Keep the selected account, permissions, repository/worktree and group unless the user approves a change. Use supported AoE interfaces: inspect `aoe --help`, `aoe session current`, `aoe session show`, `aoe group --help` and `aoe session color --help` before using their commands. Scope commands to this Session and its AoE profile. Do not create another Session or replay the initial task. No new Ledger integration, profile apply, sandboxing or Fable/Astra orchestration is authorized by intake. Report setup, launch or access failures rather than falling back to another account or broadening permissions.";

#[derive(Clone)]
pub struct IntakeBrief {
    pub task: String,
}

impl IntakeBrief {
    pub fn new(task: String) -> Result<Self> {
        ensure!(
            !task.trim().is_empty(),
            "Describe the task before configuring a Session"
        );
        ensure!(
            task.len() <= 32 * 1024,
            "Intake is limited to 32 KiB; link larger source material"
        );
        ensure!(!task.contains('\0'), "Intake cannot contain NUL bytes");
        Ok(Self { task })
    }

    pub fn prompt(&self, profile: &str, group: &str) -> String {
        format!(
            "# AoE intake brief\n\n## Task (original input)\n{}\n\n## Session setup\nAoE profile: {}\nGroup: {}\nThe launch configuration selected the working directory, worktree and permissions. Confirm the actual context before acting.\n\n## Initial prompt\nTurn the task into a concise working brief: goal, constraints, acceptance checks and next action. Retain the original task; do not invent missing requirements. Clarify consequential unknowns, then carry out the work within the reviewed permissions. This Session owns the task; do not launch a duplicate.\n\n## Intake workflow\n{}",
            self.task,
            serde_json::to_string(profile).expect("string serialization"),
            serde_json::to_string(group).expect("string serialization"),
            POLICY,
        )
    }

    pub fn launch_args(&self, configured: &str, profile: &str, group: &str) -> Result<String> {
        let words = shell_words::split(configured).context("Invalid Pi arguments")?;
        ensure!(
            !words.iter().any(|word| matches!(word.split('=').next().unwrap_or_default(),
                "--resume" | "-r" | "--continue" | "-c" | "--session" | "--session-dir"
                | "--mode" | "--print" | "-p" | "--no-session" | "--")),
            "Intake requires a fresh interactive Pi conversation; remove resume, session, mode or print arguments"
        );
        let mut args = words.iter();
        while let Some(word) = args.next() {
            let (flag, inline) = word
                .split_once('=')
                .map_or((word.as_str(), false), |(flag, _)| (flag, true));
            match flag {
                "--provider" | "--model" | "--thinking" | "--system-prompt" | "--append-system-prompt"
                | "--extension" | "-e" | "--skill" | "--tools" => {
                    ensure!(inline || args.next().is_some(), "Missing value for {flag}");
                }
                "--no-tools" | "--no-extensions" | "--no-skills" | "--no-prompt-templates" | "--offline" if !inline => {}
                _ => anyhow::bail!("Unsupported intake Pi argument {flag:?}; remove it or use the ordinary New Session form"),
            }
        }
        Ok(format!(
            "{} {}",
            configured.trim(),
            shell_words::quote(&serde_json::to_string(&self.prompt(profile, group))?),
        )
        .trim_start()
        .to_string())
    }
}

/// Separate the initial turn before rendering native resume flags. Only our exact trailer qualifies.
pub(crate) fn split_launch_args(args: &str) -> (String, Option<String>) {
    let Ok(mut words) = shell_words::split(args) else {
        return (args.to_string(), None);
    };
    let prompt = words
        .last()
        .and_then(|arg| serde_json::from_str::<String>(arg).ok());
    let Some(prompt) = prompt
        .filter(|prompt| prompt.starts_with("# AoE intake brief\n") && prompt.ends_with(POLICY))
    else {
        return (args.to_string(), None);
    };
    words.pop();
    (shell_words::join(words), Some(prompt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intake_preserves_task_bytes_quotes_setup_and_separates_initial_turn() {
        let task =
            "Fix 'quoted' $(touch /tmp/not-executed)\n\nKeep the original bytes; --resume another";
        let brief = IntakeBrief::new(task.into()).unwrap();
        let args = brief
            .launch_args(
                "--model 'provider/model' --thinking high",
                "personal",
                "aoe",
            )
            .unwrap();
        let (retained, prompt) = split_launch_args(&args);
        assert!(prompt.as_ref().unwrap().contains(task));
        assert!(prompt
            .unwrap()
            .contains("AoE profile: \"personal\"\nGroup: \"aoe\""));
        let words = shell_words::split(&retained).unwrap();
        assert_eq!(words, ["--model", "provider/model", "--thinking", "high"]);
        assert_eq!(split_launch_args("--model normal").1, None);
        assert_eq!(split_launch_args("'unclosed").1, None);
    }

    #[test]
    fn intake_refuses_empty_oversized_and_resume_or_noninteractive_launches() {
        for task in [
            String::new(),
            " \n".into(),
            "a\0b".into(),
            "x".repeat(32 * 1024 + 1),
        ] {
            assert!(IntakeBrief::new(task).is_err());
        }
        let brief = IntakeBrief::new("A task".into()).unwrap();
        for args in [
            "--resume previous",
            "--resume=previous",
            "--session /tmp/s",
            "--mode rpc",
            "--print",
            "-c",
            "--no-session",
            "--",
            "'",
            "@unexpected.md",
            "another prompt",
            "--model",
            "--export /tmp/out",
        ] {
            assert!(brief.launch_args(args, "work", "group").is_err(), "{args}");
        }
    }
}
