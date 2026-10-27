//! Fence older writers before introducing authority-bearing Workshop requests.
use anyhow::Result;
use std::path::Path;

pub fn run() -> Result<()> {
    run_in(&crate::session::get_app_dir()?)
}
fn run_in(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    crate::session::locked_update(
        &dir.join("workshop.json"),
        |s| {
            Ok(serde_json::from_str::<
                std::collections::BTreeMap<String, crate::session::tailored::workshop::Request>,
            >(s)?)
        },
        |rows| Ok(serde_json::to_string_pretty(rows)?),
        |_| -> Result<()> { Ok(()) },
    )??;
    tracing::info!("v053: initialized Workshop requests and fenced older schema writers");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workshop_initialization_is_idempotent_and_leaves_other_domains_untouched() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("research.json"), b"unrelated bytes").unwrap();
        run_in(dir.path()).unwrap();
        let first = std::fs::read(dir.path().join("workshop.json")).unwrap();
        run_in(dir.path()).unwrap();
        assert_eq!(
            first,
            std::fs::read(dir.path().join("workshop.json")).unwrap()
        );
        assert_eq!(
            std::fs::read(dir.path().join("research.json")).unwrap(),
            b"unrelated bytes"
        );
    }
}
