//! Bounded identity reads without registry repair, quarantine or initialization.
use super::*;
use crate::session::AnchoredDir;
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const REGISTRY_BYTES: usize = 8 * 1024 * 1024;
const REGISTRY_ROWS: usize = 4096;

pub(super) fn load(app: &Path, profile: &str) -> Result<BTreeMap<String, Instance>> {
    authority::scope(profile, None)?;
    let mut dir = AnchoredDir::open(app)?;
    for component in ["profiles", profile] {
        if dir.entry_stat(Path::new(component))?.is_none() {
            return Ok(BTreeMap::new());
        }
        dir = dir.child(Path::new(component))?;
    }
    if dir.entry_stat(Path::new("sessions.json"))?.is_none() {
        return Ok(BTreeMap::new());
    }
    let file = dir
        .open_regular(Path::new("sessions.json"), REGISTRY_BYTES)?
        .context("unsafe or oversized session registry")?;
    let meta = file.metadata()?;
    ensure!(
        meta.uid() == unsafe { libc::geteuid() } && meta.nlink() <= 1,
        "unsafe session registry ownership"
    );
    let mut bytes = Vec::new();
    file.take((REGISTRY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= REGISTRY_BYTES, "oversized session registry");
    let rows: Vec<Instance> = serde_json::from_slice(&bytes)
        .context("invalid session registry; inspection does not repair it")?;
    ensure!(
        rows.len() <= REGISTRY_ROWS,
        "session registry exceeds 4096 rows"
    );
    let mut result = BTreeMap::new();
    for mut row in rows {
        authority::scope(profile, Some(&row.id))?;
        row.source_profile = profile.to_owned();
        ensure!(
            result.insert(row.id.clone(), row).is_none(),
            "duplicate session identity in registry"
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_duplicate_and_unsafe_registry_never_quarantine_or_initialize() {
        let app = tempfile::tempdir().unwrap();
        assert!(load(app.path(), "alpha").unwrap().is_empty());
        assert_eq!(std::fs::read_dir(app.path()).unwrap().count(), 0);
        let dir = app.path().join("profiles/alpha");
        std::fs::create_dir_all(&dir).unwrap();
        let row = Instance::new("visible", "/workspace");
        for bytes in [
            b"[{}]".to_vec(),
            serde_json::to_vec(&vec![&row, &row]).unwrap(),
        ] {
            std::fs::write(dir.join("sessions.json"), &bytes).unwrap();
            assert!(load(app.path(), "alpha").is_err());
            assert_eq!(std::fs::read(dir.join("sessions.json")).unwrap(), bytes);
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        }
        std::fs::remove_file(dir.join("sessions.json")).unwrap();
        std::os::unix::fs::symlink("/missing", dir.join("sessions.json")).unwrap();
        assert!(load(app.path(), "alpha").is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_file(dir.join("sessions.json")).unwrap();
        std::fs::File::create(dir.join("sessions.json"))
            .unwrap()
            .set_len((REGISTRY_BYTES + 1) as u64)
            .unwrap();
        assert!(load(app.path(), "alpha").is_err());
        std::fs::write(
            dir.join("sessions.json"),
            serde_json::to_vec(&vec![&row; REGISTRY_ROWS + 1]).unwrap(),
        )
        .unwrap();
        assert!(load(app.path(), "alpha")
            .err()
            .unwrap()
            .to_string()
            .contains("4096"));
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1);
    }
}
