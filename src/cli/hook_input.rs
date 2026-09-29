//! Shared input handling for the hidden hook subcommands.

use std::io::Read;

pub(crate) const STDIN_BYTE_CAP: u64 = 1 << 20;
const MAX_ANCESTORS: usize = 64;

/// Reads at most [`STDIN_BYTE_CAP`] bytes and parses them as JSON.
pub(crate) fn read_json<R: Read>(stdin: R) -> anyhow::Result<serde_json::Value> {
    let mut buf = String::new();
    stdin.take(STDIN_BYTE_CAP).read_to_string(&mut buf)?;
    Ok(serde_json::from_str(&buf)?)
}

pub(crate) fn fired_by_pane_agent() -> bool {
    let agent_pid = std::env::var("AOE_AGENT_PID")
        .ok()
        .and_then(|pid| pid.parse().ok());
    let agent_bin = std::env::var("AOE_AGENT_BIN")
        .ok()
        .filter(|bin| !bin.is_empty());
    let (Some(agent_pid), Some(agent_bin)) = (agent_pid, agent_bin) else {
        return true;
    };
    walk_reaches_single_agent(
        std::os::unix::process::parent_id(),
        agent_pid,
        &agent_bin,
        crate::process::parent_and_argv0,
    )
}

fn walk_reaches_single_agent(
    start: u32,
    agent_pid: u32,
    agent_bin: &str,
    parent_and_argv0: impl Fn(u32) -> Option<(u32, String)>,
) -> bool {
    let mut pid = start;
    let mut agents = 0;
    for hop in 0..MAX_ANCESTORS {
        let Some((ppid, argv0)) = parent_and_argv0(pid) else {
            return hop == 0;
        };
        if std::path::Path::new(&argv0)
            .file_name()
            .and_then(|name| name.to_str())
            == Some(agent_bin)
        {
            agents += 1;
        }
        if pid == agent_pid {
            return agents <= 1;
        }
        if ppid == 0 || ppid == pid {
            return false;
        }
        pid = ppid;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_pane_agent_owns_the_hook() {
        type Chain = &'static [(u32, u32, &'static str)];
        let cases: [(&str, u32, Chain, bool); 7] = [
            ("direct launch", 9, &[(10, 9, "sh"), (9, 1, "claude")], true),
            (
                "wrapper runs the agent as a child",
                8,
                &[(10, 9, "sh"), (9, 8, "/opt/bin/claude"), (8, 1, "/bin/sh")],
                true,
            ),
            (
                "nested agent from the shell tool",
                7,
                &[
                    (10, 9, "sh"),
                    (9, 8, "claude"),
                    (8, 7, "bash"),
                    (7, 1, "claude"),
                ],
                false,
            ),
            (
                "nested agent under a wrapper",
                6,
                &[
                    (10, 9, "sh"),
                    (9, 8, "claude"),
                    (8, 7, "bash"),
                    (7, 6, "claude"),
                    (6, 1, "sh"),
                ],
                false,
            ),
            (
                "detached from the launched pid",
                7,
                &[(10, 9, "sh"), (9, 1, "claude"), (1, 0, "init")],
                false,
            ),
            ("unreadable ancestor", 7, &[(10, 9, "sh")], false),
            ("process table unavailable", 7, &[], true),
        ];
        for (name, agent_pid, chain, owned) in cases {
            let table: std::collections::HashMap<u32, (u32, String)> = chain
                .iter()
                .map(|(pid, ppid, argv0)| (*pid, (*ppid, argv0.to_string())))
                .collect();
            let lookup = |pid| table.get(&pid).cloned();
            assert_eq!(
                walk_reaches_single_agent(10, agent_pid, "claude", lookup),
                owned,
                "{name}"
            );
        }
    }
}
