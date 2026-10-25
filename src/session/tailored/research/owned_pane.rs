//! Exclusive research panes, never adopted through the ordinary Session lookup.
use crate::process::{process_birth_identity, ProcessBirthIdentity};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OwnedPane {
    pane_id: String,
    session_name: String,
    generation: String,
    pub process_id: u32,
    process_birth: ProcessBirthIdentity,
    server_pid: u32,
    server_birth: ProcessBirthIdentity,
}
pub(super) fn session_name(id: &str) -> String {
    format!("{}research_{id}", crate::tmux::SESSION_PREFIX)
}
fn run(cmd: &mut Command) -> Result<String> {
    let output = crate::tmux::TmuxCommandDeadline::new().run(cmd)?;
    if !output.status.success() {
        bail!(
            "Owned research pane unavailable: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?
        .trim_end_matches('\n')
        .into())
}
pub(super) fn create(id: &str, cwd: &str, command: &str) -> Result<OwnedPane> {
    create_with_shell(id, cwd, command, "/bin/sh")
}
fn create_with_shell(id: &str, cwd: &str, command: &str, shell: &str) -> Result<OwnedPane> {
    let name = session_name(id);
    let startup = tempfile::tempdir()?;
    let ready = startup.path().join("ready");
    // Keep a stable supervisor; preserve stdin before dash backgrounds the child.
    let script = format!(
        "exec 3<&0; printf '%s' \"$$\" > {}; ( {command} ) <&3 3<&- & wait",
        shell_words::quote(ready.to_str().context("Non-UTF8 startup path")?)
    );
    // new-session is exclusive unless -A is passed. Never resolve or adopt an existing name.
    let identity = run(crate::tmux::tmux_command().args([
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}\t#{pane_pid}\t#{pid}",
        "-s",
        &name,
        "-c",
        cwd,
        shell,
        "-c",
        &script,
    ]))?;
    let fields: Vec<_> = identity.split('\t').collect();
    if fields.len() != 3 || !fields[0].starts_with('%') {
        bail!("tmux did not return an immutable pane identity");
    }
    let process_id = fields[1].parse()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if std::fs::read_to_string(&ready).ok().as_deref() == Some(fields[1]) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            bail!("Discovery supervisor did not establish its process identity");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let server_pid = fields[2].parse()?;
    let pane = OwnedPane {
        pane_id: fields[0].into(),
        session_name: name,
        generation: uuid::Uuid::new_v4().to_string(),
        process_id,
        process_birth: process_birth_identity(process_id)?
            .context("Pane process exited during startup")?,
        server_pid,
        server_birth: process_birth_identity(server_pid)?
            .context("tmux server exited during startup")?,
    };
    pane.check_birth()?;
    run(crate::tmux::tmux_command().args([
        "set-option",
        "-p",
        "-t",
        &pane.pane_id,
        "@aoe_research_owner",
        &pane.generation,
    ]))?;
    pane.checked("display-message -p owned")?;
    Ok(pane)
}
impl OwnedPane {
    fn check_birth(&self) -> Result<()> {
        if process_birth_identity(self.process_id)?.as_ref() != Some(&self.process_birth)
            || process_birth_identity(self.server_pid)?.as_ref() != Some(&self.server_birth)
        {
            bail!("Owned discovery process generation changed or stopped; no replay or input is allowed");
        }
        Ok(())
    }
    fn condition(&self) -> String {
        format!("#{{&&:#{{==:#{{pane_id}},{}}},#{{&&:#{{==:#{{pane_pid}},{}}},#{{&&:#{{==:#{{session_name}},{}}},#{{==:#{{@aoe_research_owner}},{}}}}}}}}}", self.pane_id, self.process_id, self.session_name, self.generation)
    }
    fn checked(&self, action: &str) -> Result<String> {
        self.check_birth()?;
        let output = run(crate::tmux::tmux_command().args([
            "if-shell",
            "-F",
            "-t",
            &self.pane_id,
            &self.condition(),
            action,
            "display-message -p AOE_RESEARCH_OWNERSHIP_LOST",
        ]))?;
        if output == "AOE_RESEARCH_OWNERSHIP_LOST" {
            bail!("Owned discovery pane changed; no replay or input is allowed");
        }
        Ok(output)
    }
    pub fn output(&self) -> Result<String> {
        self.checked(&format!("capture-pane -p -S -200 -t {}", self.pane_id))
    }
    pub fn reply(&self, answer: &str) -> Result<()> {
        self.check_birth()?;
        let buffer = format!("research-{}", uuid::Uuid::new_v4());
        let mut child = crate::tmux::tmux_command()
            .args(["load-buffer", "-b", &buffer, "-"])
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        use std::io::Write;
        child
            .stdin
            .take()
            .context("Missing tmux buffer pipe")?
            .write_all(answer.as_bytes())?;
        if !child.wait()?.success() {
            bail!("Cannot load discovery answer");
        }
        let result = self.checked(&format!(
            "paste-buffer -d -p -b {buffer} -t {} ; send-keys -t {} Enter",
            self.pane_id, self.pane_id
        ));
        let _ = crate::tmux::tmux_command()
            .args(["delete-buffer", "-b", &buffer])
            .output();
        result.map(|_| ())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[serial_test::serial]
    fn supervisor_preserves_terminal_input_across_posix_background_rules() {
        crate::tmux::test_helpers::require_tmux!();
        let dir = tempfile::tempdir().unwrap();
        let socket = format!("research-stdin-{}", uuid::Uuid::new_v4());
        let _env =
            crate::session::test_support::EnvGuard::set(&[("AOE_TMUX_SOCKET", socket.as_str())]);
        struct Cleanup;
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = crate::tmux::tmux_command().arg("kill-server").output();
            }
        }
        let _cleanup = Cleanup;
        for shell in ["/bin/sh", "/bin/dash"] {
            if !std::path::Path::new(shell).exists() {
                continue;
            }
            let id = uuid::Uuid::new_v4().to_string();
            let pane = create_with_shell(&id, dir.path().to_str().unwrap(), "exec /bin/cat", shell)
                .unwrap_or_else(|error| panic!("{shell}: {error:#}"));
            pane.reply("literal input survives background startup")
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if pane
                    .output()
                    .unwrap()
                    .contains("literal input survives background startup")
                {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{shell} lost input");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn exclusive_creation_and_generation_checks_refuse_collisions_respawns_and_replacements() {
        crate::tmux::test_helpers::require_tmux!();
        let dir = tempfile::tempdir().unwrap();
        let socket = format!("research-test-{}", uuid::Uuid::new_v4());
        let _env =
            crate::session::test_support::EnvGuard::set(&[("AOE_TMUX_SOCKET", socket.as_str())]);
        struct Cleanup;
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = crate::tmux::tmux_command().arg("kill-server").output();
            }
        }
        let _cleanup = Cleanup;
        let id = uuid::Uuid::new_v4().to_string();
        let pane = create(&id, dir.path().to_str().unwrap(), "exec /bin/cat").unwrap();
        assert!(create(&id, dir.path().to_str().unwrap(), "exec /bin/cat").is_err());
        assert!(pane.output().is_ok());
        pane.reply("literal discovery target").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if pane.output().unwrap().contains("literal discovery target") {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut wrong_marker = pane.clone();
        wrong_marker.generation = uuid::Uuid::new_v4().to_string();
        assert!(wrong_marker.reply("MUST NOT ARRIVE").is_err());
        let mut wrong_server = pane.clone();
        wrong_server.server_birth.start_id.push('x');
        assert!(wrong_server.reply("MUST NOT ARRIVE").is_err());

        let other_id = format!("{}{}", &id[..8], &uuid::Uuid::new_v4().to_string()[8..]);
        let _other = create(&other_id, dir.path().to_str().unwrap(), "exec /bin/cat").unwrap();
        let mut wrong = pane.clone();
        wrong.process_birth.start_id.push('x');
        assert!(wrong.reply("MUST NOT ARRIVE").is_err());
        assert!(!pane.output().unwrap().contains("MUST NOT ARRIVE"));
        run(crate::tmux::tmux_command().args([
            "respawn-pane",
            "-k",
            "-t",
            &pane.pane_id,
            "exec /bin/cat",
        ]))
        .unwrap();
        assert!(pane.reply("MUST NOT ARRIVE").is_err());
        assert!(pane.output().is_err());
        run(crate::tmux::tmux_command().args(["kill-session", "-t", &pane.session_name])).unwrap();
        let replacement = create(&id, dir.path().to_str().unwrap(), "exec /bin/cat").unwrap();
        assert!(pane.reply("MUST NOT ARRIVE").is_err());
        assert!(!replacement.output().unwrap().contains("MUST NOT ARRIVE"));
    }
}
