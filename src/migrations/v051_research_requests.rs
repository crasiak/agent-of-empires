//! Introduce native research drafts without changing capture records.
use anyhow::Result;
use std::path::Path;

pub fn run() -> Result<()> {
    run_in(&crate::session::get_app_dir()?)
}
fn run_in(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("research.json");
    crate::session::locked_update(
        &path,
        |s| {
            Ok(serde_json::from_str::<
                std::collections::BTreeMap<String, crate::session::tailored::research::Draft>,
            >(s)?)
        },
        |s| Ok(serde_json::to_string_pretty(s)?),
        |_| -> Result<()> { Ok(()) },
    )??;
    tracing::info!("v051: initialized native research request store");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn introduces_store_without_changing_capture_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sessions.json"), "capture bytes").unwrap();
        run_in(dir.path()).unwrap();
        let first = std::fs::read(dir.path().join("research.json")).unwrap();
        run_in(dir.path()).unwrap();
        assert_eq!(
            first,
            std::fs::read(dir.path().join("research.json")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sessions.json")).unwrap(),
            "capture bytes"
        );
    }
}
