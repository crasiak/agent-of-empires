//! Retype web-owned repository appearance JSON into shared state.
use crate::session::repo_appearance::{valid_repo_id, RepoAppearance, RepoColor};
use anyhow::Result;
use std::path::Path;

const LEGACY_KEY: &str = "aoe-repo-appearance-v1";

pub fn run() -> Result<()> {
    run_in(&crate::session::get_app_dir()?)
}

fn run_in(dir: &Path) -> Result<()> {
    let path = dir.join("state.toml");
    if !path.exists() {
        return Ok(());
    }
    crate::session::locked_update(
        &path,
        |content| Ok(content.parse::<toml::Table>()?),
        |state| Ok(toml::to_string_pretty(state)?),
        |state| -> Result<()> {
            let Some(legacy) = state
                .get_mut("web_ui_state")
                .and_then(toml::Value::as_table_mut)
                .and_then(|web| web.remove(LEGACY_KEY))
            else {
                return Ok(());
            };
            crate::session::backup_before_migration(&path)?;
            let parsed = legacy
                .as_str()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
            let entries = parsed.as_ref().and_then(serde_json::Value::as_object);
            if let Some(entries) = entries {
                let target = state
                    .entry("repo_appearances")
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                    .as_table_mut()
                    .ok_or_else(|| anyhow::anyhow!("repo_appearances is not a table"))?;
                for (id, raw) in entries {
                    if !valid_repo_id(id) || !raw.is_object() {
                        tracing::warn!(
                            "v036: skipping invalid repository appearance key/entry {id:?}"
                        );
                        continue;
                    }
                    let alias = raw
                        .get("alias")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    let color = raw
                        .get("color")
                        .and_then(|v| serde_json::from_value::<RepoColor>(v.clone()).ok());
                    if alias.is_none() && color.is_none() {
                        continue;
                    }
                    // A peer's already-typed entry wins on a resumed upgrade.
                    if !target.contains_key(id) {
                        target.insert(
                            id.clone(),
                            toml::Value::try_from(RepoAppearance { alias, color })?,
                        );
                    }
                }
            } else {
                tracing::warn!("v036: invalid legacy repository appearance JSON; original retained in migration backup");
            }
            tracing::info!("v036: moved repository appearances to shared typed state");
            Ok(())
        },
    )??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrates_legacy_fields_preserves_peers_and_is_idempotent() {
        for legacy in [
            r#"{"/a":{"alias":" Alpha ","color":"sky"},"/b":{"alias":"B","color":"bad"},"/c":{"color":"green"},"relative":{"color":"rose"},"__scratch__":{"color":"teal"},"/peer":{"color":"rose"},"/invalid":2}"#,
            "broken json",
            "[]",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.toml");
            let old = format!("has_seen_welcome = true\n[web_ui_state]\nother = 'kept'\n{LEGACY_KEY} = '{}'\n[repo_appearances.'/peer']\nalias = 'Peer'\n", legacy);
            std::fs::write(&path, &old).unwrap();
            run_in(dir.path()).unwrap();
            let first = std::fs::read_to_string(&path).unwrap();
            let state: toml::Table = first.parse().unwrap();
            assert_eq!(state["has_seen_welcome"].as_bool(), Some(true));
            assert_eq!(state["web_ui_state"]["other"].as_str(), Some("kept"));
            assert!(!state["web_ui_state"]
                .as_table()
                .unwrap()
                .contains_key(LEGACY_KEY));
            assert_eq!(
                state["repo_appearances"]["/peer"]["alias"].as_str(),
                Some("Peer")
            );
            if legacy.starts_with('{') {
                assert_eq!(
                    state["repo_appearances"]["/a"]["alias"].as_str(),
                    Some("Alpha")
                );
                assert_eq!(
                    state["repo_appearances"]["/a"]["color"].as_str(),
                    Some("sky")
                );
                assert_eq!(state["repo_appearances"]["/b"]["alias"].as_str(), Some("B"));
                assert!(!state["repo_appearances"]
                    .as_table()
                    .unwrap()
                    .contains_key("/c"));
                assert!(!state["repo_appearances"]
                    .as_table()
                    .unwrap()
                    .contains_key("relative"));
            }
            assert!(std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .any(|f| f.path() != path
                    && std::fs::read_to_string(f.path()).ok().as_deref() == Some(&old)));
            run_in(dir.path()).unwrap();
            assert_eq!(std::fs::read_to_string(path).unwrap(), first);
        }
    }
}
