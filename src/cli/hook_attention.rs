//! Hidden `aoe __hook-attention` subcommand: records what a waiting agent asks
//! in the instance's `attention` sidecar.

use std::io::Read;

use anyhow::{anyhow, bail, Result};
use clap::Args;
use serde_json::Value;

use super::hook_input::{fired_by_pane_agent, publisher_is_pane_agent};

#[derive(Args)]
pub struct HookAttentionArgs {
    /// Binary of the agent whose hook fired this. Refused when the pane runs a different agent.
    #[arg(long)]
    agent: String,
}

/// Hook payloads that carry a question are small; a larger one is refused, not truncated.
const STDIN_BYTE_CAP: u64 = 64 << 10;

pub async fn run(args: HookAttentionArgs) -> Result<()> {
    let Some(instance_id) = pane_instance_id(&args.agent) else {
        return Ok(());
    };
    if let Err(e) = run_inner(std::io::stdin().lock(), &instance_id, &args.agent) {
        tracing::debug!(target: "hooks.attention", "attention not written: {e}");
    }
    Ok(())
}

/// The instance this hook may write for, or `None` when it fired outside an AoE
/// pane, for another agent's pane, or from a nested agent.
fn pane_instance_id(agent: &str) -> Option<String> {
    let instance_id = std::env::var("AOE_INSTANCE_ID").ok()?;
    crate::session::validate_instance_id(&instance_id).ok()?;
    let pane_agent = std::env::var("AOE_AGENT_BIN").ok();
    if !publisher_is_pane_agent(Some(agent), pane_agent.as_deref()) || !fired_by_pane_agent() {
        tracing::debug!(target: "hooks.attention", "ignoring hook from another agent");
        return None;
    }
    Some(instance_id)
}

fn read_payload<R: Read>(stdin: R) -> Result<Value> {
    let mut buf = Vec::new();
    stdin.take(STDIN_BYTE_CAP + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > STDIN_BYTE_CAP {
        bail!("hook payload exceeds 64 KiB");
    }
    Ok(serde_json::from_slice(&buf)?)
}

/// The text the user is asked. A permission request names only its tool:
/// `tool_input` can hold a command line with secrets in it.
fn summary(agent: &str, payload: &Value) -> Option<String> {
    if agent != "claude" {
        return None;
    }
    let text = |pointer: &str| payload.pointer(pointer).and_then(Value::as_str);
    let summary = match text("/hook_event_name")? {
        "PreToolUse" if text("/tool_name")? == "AskUserQuestion" => {
            text("/tool_input/questions/0/question")?.to_string()
        }
        "Notification" => text("/message")?.to_string(),
        "PermissionRequest" => match text("/tool_name")? {
            "" => return None,
            tool => format!("Allow {tool}?"),
        },
        _ => return None,
    };
    Some(crate::hooks::sanitize_attention_summary(summary.as_bytes()))
}

fn run_inner<R: Read>(stdin: R, instance_id: &str, agent: &str) -> Result<()> {
    let payload = read_payload(stdin)?;
    let summary =
        summary(agent, &payload).ok_or_else(|| anyhow!("payload carries no attention text"))?;
    crate::hooks::write_attention_via_guard(instance_id, &summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::test_support::BaseGuard;
    use crate::session::test_support::EnvGuard;
    use std::os::unix::fs::PermissionsExt;

    fn read_attention(base: &std::path::Path, instance_id: &str) -> Option<String> {
        std::fs::read_to_string(base.join(instance_id).join("attention")).ok()
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn claude_payloads_yield_their_question_and_nothing_else() {
        let (_g, base, _tmp) = BaseGuard::ready();
        let ask = |questions: Value| {
            serde_json::json!({
                "session_id": "s", "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion", "tool_input": {"questions": questions},
            })
            .to_string()
        };
        let padded_ask = {
            let mut payload = ask(serde_json::json!([{"question": "Padded?"}]));
            payload.push_str(&" ".repeat(STDIN_BYTE_CAP as usize - payload.len()));
            payload
        };
        let oversized_ask = format!("{padded_ask} ");
        let cases: Vec<(&str, &str, String, Option<&str>)> = vec![
            (
                "ask_first_question",
                "claude",
                ask(serde_json::json!([
                    {"question": "Which database?", "options": [{"label": "pg"}]},
                    {"question": "Second?"},
                ])),
                Some("Which database?"),
            ),
            (
                "ask_sanitized",
                "claude",
                ask(serde_json::json!([{"question": "Line one\nline\u{202e} two\t"}])),
                Some("Line one line two"),
            ),
            ("ask_at_cap", "claude", padded_ask, Some("Padded?")),
            (
                "notification_message",
                "claude",
                serde_json::json!({
                    "hook_event_name": "Notification",
                    "notification_type": "permission_prompt",
                    "message": "Claude needs your permission to use Bash",
                })
                .to_string(),
                Some("Claude needs your permission to use Bash"),
            ),
            (
                "permission_names_only_the_tool",
                "claude",
                serde_json::json!({
                    "hook_event_name": "PermissionRequest", "tool_name": "Bash",
                    "tool_input": {"command": "export TOKEN=hunter2"},
                })
                .to_string(),
                Some("Allow Bash?"),
            ),
            ("ask_oversized", "claude", oversized_ask, None),
            (
                "other_tool",
                "claude",
                serde_json::json!({
                    "hook_event_name": "PreToolUse", "tool_name": "Bash",
                    "tool_input": {"command": "ls"},
                })
                .to_string(),
                None,
            ),
            (
                "ask_without_question",
                "claude",
                ask(serde_json::json!([{"header": "h"}])),
                None,
            ),
            (
                "ask_question_not_string",
                "claude",
                ask(serde_json::json!([{"question": ["nested"]}])),
                None,
            ),
            (
                "notification_without_message",
                "claude",
                r#"{"hook_event_name":"Notification"}"#.to_string(),
                None,
            ),
            (
                "permission_without_tool",
                "claude",
                r#"{"hook_event_name":"PermissionRequest","tool_name":""}"#.to_string(),
                None,
            ),
            (
                "other_agent",
                "codex",
                r#"{"hook_event_name":"Notification","message":"m"}"#.to_string(),
                None,
            ),
            ("malformed", "claude", "not json {{".to_string(), None),
            ("empty", "claude", String::new(), None),
        ];
        for (name, agent, payload, want) in cases {
            let result = run_inner(payload.as_bytes(), name, agent);
            assert_eq!(result.is_ok(), want.is_some(), "{name}: {result:?}");
            assert_eq!(read_attention(&base, name).as_deref(), want, "{name}");
        }
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn infinite_stdin_is_refused_without_hanging() {
        let (_g, base, _tmp) = BaseGuard::ready();
        let payload = r#"{"hook_event_name":"Notification","message":"m"}"#;
        let input = payload.as_bytes().chain(std::io::repeat(b' '));
        assert!(run_inner(input, "infinite", "claude").is_err());
        assert!(read_attention(&base, "infinite").is_none());
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn write_replaces_a_symlinked_leaf_without_following_it() {
        let (_g, base, tmp) = BaseGuard::ready();
        let decoy = tmp.path().join("decoy");
        std::fs::write(&decoy, b"do not overwrite").unwrap();
        let inst = base.join("decoy_leaf");
        std::fs::create_dir(&inst).unwrap();
        std::fs::set_permissions(&inst, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&decoy, inst.join("attention")).unwrap();
        let payload = r#"{"hook_event_name":"Notification","message":"m"}"#;
        run_inner(payload.as_bytes(), "decoy_leaf", "claude").unwrap();
        assert_eq!(std::fs::read_to_string(&decoy).unwrap(), "do not overwrite");
        assert_eq!(read_attention(&base, "decoy_leaf").as_deref(), Some("m"));
    }

    #[test]
    #[serial_test::serial(hook_base)]
    fn only_the_pane_agent_may_write() {
        let missing_pid = (1u32 << 22) + 12_345;
        for (name, instance, pane_agent, agent_pid, accepted) in [
            ("same_agent", Some("inst"), Some("claude"), None, true),
            ("pane_without_agent_bin", Some("inst"), None, None, true),
            ("other_pane_agent", Some("inst"), Some("codex"), None, false),
            (
                "detached_from_the_pane_agent",
                Some("inst"),
                Some("claude"),
                Some(missing_pid),
                false,
            ),
            ("no_instance", None, Some("claude"), None, false),
            ("unsafe_instance", Some("../x"), Some("claude"), None, false),
        ] {
            let mut env = EnvGuard::unset(&[
                "AOE_INSTANCE_ID",
                "AOE_AGENT_BIN",
                "AOE_AGENT_PID",
                "AOE_AGENT_PROGRAM",
            ]);
            for (key, value) in [
                ("AOE_INSTANCE_ID", instance.map(str::to_string)),
                ("AOE_AGENT_BIN", pane_agent.map(str::to_string)),
                ("AOE_AGENT_PID", agent_pid.map(|pid| pid.to_string())),
            ] {
                if let Some(value) = value {
                    env = env.and_set(key, value);
                }
            }
            assert_eq!(pane_instance_id("claude").is_some(), accepted, "{name}");
            drop(env);
        }
    }
}
