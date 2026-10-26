//! Initialize shared history visibility without rewriting Session records.

use anyhow::Result;
use std::path::Path;

pub fn run() -> Result<()> {
    run_in(&crate::session::get_app_dir()?)
}

fn run_in(dir: &Path) -> Result<()> {
    let path = dir.join("state.toml");
    if !path.exists() {
        return Ok(());
    }
    crate::session::locked_update_if_changed(
        &path,
        |content| Ok(content.parse::<toml::Table>()?),
        |state| Ok(toml::to_string_pretty(state)?),
        |state| -> Result<()> {
            if let Some(value) = state.get("tailored_hidden_sessions") {
                let _: crate::session::tailored::history::HiddenSessions =
                    value.clone().try_into()?;
                return Ok(());
            }
            crate::session::backup_before_migration(&path)?;
            state.insert(
                "tailored_hidden_sessions".into(),
                toml::Value::Table(toml::Table::new()),
            );
            tracing::info!("v050: initialized shared Tailored Session history visibility");
            Ok(())
        },
    )??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_state_and_sessions_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let old = "has_seen_welcome = true\nunknown_peer_field = 'kept'\n[web_ui_state]\nother = 'value'\n";
        std::fs::write(&path, old).unwrap();
        let sessions = dir.path().join("sessions.json");
        std::fs::write(&sessions, "unchanged session bytes").unwrap();
        run_in(dir.path()).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let state: toml::Table = first.parse().unwrap();
        assert_eq!(state["unknown_peer_field"].as_str(), Some("kept"));
        assert_eq!(state["web_ui_state"]["other"].as_str(), Some("value"));
        assert!(state["tailored_hidden_sessions"]
            .as_table()
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::read_to_string(&sessions).unwrap(),
            "unchanged session bytes"
        );
        assert!(std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .any(|file| {
                file.path() != path
                    && std::fs::read_to_string(file.path()).ok().as_deref() == Some(old)
            }));
        run_in(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        std::fs::write(
            &path,
            "[tailored_hidden_sessions]\nalpha = ['session-id']\n",
        )
        .unwrap();
        run_in(dir.path()).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("session-id"));
        let invalid = "tailored_hidden_sessions = 'not a profile map'\n";
        std::fs::write(&path, invalid).unwrap();
        assert!(run_in(dir.path()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
    }
}
