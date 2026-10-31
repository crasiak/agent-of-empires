//! Preserve legacy orientation, but require consent to the new Session lifetime.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

pub fn run() -> Result<()> {
    run_in(&crate::session::get_app_dir()?)
}

fn run_in(root: &Path) -> Result<()> {
    for path in super::sessions_file::session_files(root)? {
        let content = match std::fs::read(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut rows: Vec<Value> = serde_json::from_slice(&content)
            .with_context(|| format!("reading {} for work arc migration", path.display()))?;
        let mut changed = false;
        for row in &mut rows {
            let state = match row.get("work_arc") {
                Some(state) if state.is_object() => state.clone(),
                None | Some(Value::Null)
                    if row.get("work_arc_enabled").and_then(Value::as_bool) == Some(true) =>
                {
                    let mut state =
                        serde_json::to_value(crate::session::work_arc::ArcState::default())?;
                    state["consent_version"] = 0.into();
                    state
                }
                None | Some(Value::Null) => continue,
                Some(_) => anyhow::bail!("invalid legacy work arc in {}", path.display()),
            };
            if state
                .get("consent_version")
                .and_then(Value::as_u64)
                .is_some_and(|version| version >= 2)
            {
                continue;
            }
            if !changed {
                crate::session::backup_before_migration(&path)?;
            }
            let mut state = state;
            let record = state
                .as_object_mut()
                .unwrap()
                .remove("record")
                .filter(|record| !record.is_null());
            state["consent_version"] = 2.into();
            let mut state: crate::session::work_arc::ArcState = serde_json::from_value(state)?;
            if let Some(record) = record {
                let record: crate::session::work_arc::StoredArc = serde_json::from_value(record)?;
                let id = row
                    .get("id")
                    .and_then(Value::as_str)
                    .context("legacy work arc owner is missing")?;
                anyhow::ensure!(
                    record.identity.session_id == id,
                    "legacy work arc has foreign owner"
                );
                let document = crate::session::work_arc::ArcDocument::observe(record, None, true);
                crate::session::work_arc::publish_document(root, &document)?;
                state.revision = Some(document.revision.clone());
            }
            state.invalidate();
            row["work_arc"] = serde_json::to_value(state)?;
            row["work_arc_enabled"] = false.into();
            changed = true;
        }
        if changed {
            crate::session::atomic_write(&path, &serde_json::to_vec_pretty(&rows)?)?;
            tracing::info!(path = %path.display(), "Preserved legacy work arcs; paused until Session-level re-consent");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn legacy(id: &str) -> Value {
        json!({
            "id":id,"work_arc_enabled":true,
            "work_arc":{
                "record":{"identity":{"session_id":id,"pi_id":"old-pi","launch_id":"old-launch","generation":1,"leaf":"old-turn"},
                    "at":"2026-10-20T01:02:03Z","arc":{"goal":"Keep goal","now":"Old progress","remaining":"Checks","next":"Review"},
                    "selected_route":"fake/route","provider":"fake","model":"test","usage":null},
                "attempts":["2026-10-20T01:02:03Z"],"pending":"old","failed":false,"unavailable":false,"cleared":false,"epoch":7,"cost":"Unknown"
            }
        })
    }

    #[test]
    fn legacy_import_preserves_unknown_dates_budgets_and_newer_consent() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profiles/test");
        std::fs::create_dir_all(&profile).unwrap();
        let row = legacy("session-one");
        let mut newer = legacy("newer-session");
        newer["work_arc"]["consent_version"] = 3.into();
        let empty = json!({"id":"empty-enabled","work_arc_enabled":true});
        let untouched = json!({"id":"untouched","work_arc_enabled":false});
        let path = profile.join("sessions.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!([row, newer, empty, untouched])).unwrap(),
        )
        .unwrap();
        run_in(root.path()).unwrap();
        let mut rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(rows[0]["work_arc_enabled"], false);
        assert_eq!(rows[0]["work_arc"]["attempts"], row["work_arc"]["attempts"]);
        assert_eq!(rows[0]["work_arc"]["pending"], Value::Null);
        assert_eq!(rows[0]["work_arc"]["epoch"], 8);
        let document = crate::session::work_arc::read_document(
            root.path(),
            "session-one",
            rows[0]["work_arc"]["revision"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(document.record.arc.goal, "Keep goal");
        assert!(document
            .fields
            .iter()
            .all(|field| field.observed_at.is_none() && field.changed_at.is_none()));
        assert_eq!(rows[1], newer, "newer authority is never retyped or paused");
        assert_eq!(rows[2]["work_arc_enabled"], false);
        assert_eq!(rows[3], untouched);
        rows[0]["work_arc_enabled"] = true.into();
        std::fs::write(&path, serde_json::to_vec(&rows).unwrap()).unwrap();
        let once = std::fs::read(&path).unwrap();
        run_in(root.path()).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            once,
            "re-consent survives repeat migration"
        );
    }

    #[test]
    fn invalid_or_foreign_legacy_state_never_rewrites_the_registry() {
        for malformed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("sessions.json");
            let mut row = legacy("session-one");
            if malformed {
                row["work_arc"] = json!("damaged");
            } else {
                row["work_arc"]["record"]["identity"]["session_id"] = json!("foreign");
            }
            let before = serde_json::to_vec(&json!([row])).unwrap();
            std::fs::write(&path, &before).unwrap();
            assert!(run_in(root.path()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
}
