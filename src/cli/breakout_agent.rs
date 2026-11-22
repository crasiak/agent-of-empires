//! `aoe breakout-agent` command: open Breakout windows on this machine for aoe running elsewhere.

use anyhow::{bail, Context, Result};
use clap::Args;

#[derive(Args)]
pub struct BreakoutAgentArgs {}

/// The hosts the agent serves, from the global config only. Refuses where this
/// machine cannot open Breakout windows or no host is allowed yet.
fn allowed_hosts() -> Result<Vec<String>> {
    if !crate::breakout::supported_here() {
        bail!("the Breakout agent needs macOS with Ghostty installed");
    }
    let allowed = crate::session::config::Config::load()?.breakout.agent_hosts;
    if allowed.is_empty() {
        bail!(
            "no hosts are allowed yet. Add them to breakout.agent_hosts in the config file:\n\n[breakout]\nagent_hosts = [\"bender\"]"
        );
    }
    Ok(allowed)
}

#[tracing::instrument(target = "cli.breakout", skip_all)]
pub fn run(_args: BreakoutAgentArgs) -> Result<()> {
    let allowed = allowed_hosts()?;
    let path = crate::breakout::agent::socket_path()?;
    let listener = crate::breakout::agent::bind(&path)
        .with_context(|| format!("binding {}", path.display()))?;
    println!("Breakout agent listening on {}", path.display());
    println!("Allowed hosts: {}", allowed.join(", "));
    crate::breakout::agent::serve(listener, allowed, crate::breakout::run_local);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakout::test_support::FORCED;
    use serial_test::serial;

    /// Forces `supported_here` for one test and restores it even when an assertion fails.
    struct Forced;

    impl Forced {
        fn set(supported: bool) -> Self {
            FORCED.with(|f| f.set(Some(supported)));
            Self
        }
    }

    impl Drop for Forced {
        fn drop(&mut self) {
            FORCED.with(|f| f.set(None));
        }
    }

    #[test]
    #[serial]
    fn the_agent_refuses_to_start_without_ghostty() {
        let _home = crate::session::test_support::isolate_app_dir();
        let _forced = Forced::set(false);
        let error = allowed_hosts().unwrap_err().to_string();
        assert!(error.contains("macOS with Ghostty"), "{error}");
    }

    #[test]
    #[serial]
    fn the_agent_refuses_to_start_with_no_allowed_hosts() {
        let _home = crate::session::test_support::isolate_app_dir();
        let _forced = Forced::set(true);
        let error = allowed_hosts().unwrap_err().to_string();
        assert!(error.contains("breakout.agent_hosts"), "{error}");
    }

    #[test]
    #[serial]
    fn the_agent_reads_its_hosts_from_the_global_config() {
        let home = crate::session::test_support::isolate_app_dir();
        let _forced = Forced::set(true);
        let path = crate::session::config::config_path().unwrap();
        std::fs::write(&path, "[breakout]\nagent_hosts = [\"bender\", \"flexo\"]\n").unwrap();
        assert!(path.starts_with(home.path()));
        assert_eq!(allowed_hosts().unwrap(), ["bender", "flexo"]);
    }
}
