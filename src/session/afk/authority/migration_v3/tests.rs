use super::*;
use std::{fs, os::unix::fs::PermissionsExt};
struct App(tempfile::TempDir);
impl App {
    fn path(&self) -> &Path {
        self.0.path()
    }
}
impl Drop for App {
    fn drop(&mut self) {
        for p in [
            "afk",
            "afk-runtime-v2",
            "afk-runtime-v3",
            "afk-runtime-v4",
            "afk-authority-v1/profiles",
            "afk-authority-v2/profiles",
        ] {
            let _ = fs::set_permissions(self.path().join(p), fs::Permissions::from_mode(0o700));
        }
    }
}
fn fixture() -> App {
    let app = App(tempfile::tempdir().unwrap());
    for (namespace, leaf, bytes) in [
        ("afk", "policy.json", migration::golden::POLICY),
        ("afk-runtime-v4", "ledger.json", migration::golden::BOOK),
    ] {
        Store::open_namespace(app.path(), "fixture", true, namespace, PROFILE_BYTES)
            .unwrap()
            .write_bytes(leaf, bytes.as_bytes())
            .unwrap();
    }
    previous::initialize_test(app.path());
    let old = frozen::profile_store(app.path(), "alpha", false).unwrap();
    assert_eq!(
        hash(old.bytes("snapshot.json").unwrap().unwrap()),
        "644787c82e846a27579003d47c37add375bdcebf9edf08d4a9d0457a25084c53"
    );
    let runtime = frozen::profile_store(app.path(), "main", true).unwrap();
    runtime
        .write_bytes("snapshot.json", previous::golden::QUESTION.as_bytes())
        .unwrap();
    frozen::profile_store(app.path(), "prepared", true).unwrap();
    app
}
#[test]
fn every_v42_boundary_preserves_originals_tickets_and_guards_without_authority() {
    for phase in [
        Phase::Started,
        Phase::Sealed,
        Phase::Inventory,
        Phase::Originals,
        Phase::Fences,
        Phase::Imports,
        Phase::Verified,
        Phase::Complete,
    ] {
        let app = fixture();
        let held = old_store(app.path(), "main").unwrap();
        let root = ingress(app.path()).unwrap();
        let inode = held.dir.identity().unwrap();
        let old: frozen::Snapshot = held.read_raw("snapshot.json").unwrap().unwrap();
        INTERRUPT.with(|at| at.set(Some(phase)));
        assert!(run_at(app.path()).is_err(), "{phase:?}");
        INTERRUPT.with(|at| at.set(None));
        assert!(ready(app.path()).is_err());
        assert!(previous::run_at(app.path()).is_err());
        run_at(app.path()).unwrap();
        run_at(app.path()).unwrap();
        assert!(held.read::<frozen::Snapshot>("snapshot.json").is_err());
        assert_eq!(held.dir.identity().unwrap(), inode);
        assert!(root.create_child(Path::new("delayed-creator")).is_err());
        assert_eq!(
            state_original(app.path(), "main"),
            previous::golden::QUESTION.as_bytes()
        );
        fs::write(
            app.path().join(".schema_version"),
            cursor::USAGE.to_string(),
        )
        .unwrap();
        ready(app.path()).unwrap();
        let main = profile_store(app.path(), "main", false).unwrap();
        let next: Snapshot = main.read_raw("snapshot.json").unwrap().unwrap();
        next.validate("main").unwrap();
        assert_eq!(
            serde_json::to_value(&next.batches).unwrap(),
            serde_json::to_value(&old.batches).unwrap()
        );
        for (id, before) in &old.sessions {
            let after = &next.sessions[id];
            assert_eq!(after.preference, before.preference);
            assert_eq!(after.exception_epoch, before.exception_epoch);
            assert_eq!(after.epoch, before.epoch + 1);
            let windows = serde_json::to_value(&after.book).unwrap();
            assert!(windows["windows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["state"] == "ended" && w["confirmed"] == false));
        }
        assert!(next.usage.sources.is_empty() && next.usage.windows.is_empty());
        let alpha = Store::authority(app.path(), "alpha", "fixture", false).unwrap();
        let book = serde_json::to_value(alpha.session().unwrap().book).unwrap();
        let window = &book["windows"][0];
        let binding = serde_json::from_value(window["binding"].clone()).unwrap();
        let identity = serde_json::from_value(window["question"]["identity"].clone()).unwrap();
        assert_eq!(
            delegation::questions::guards(&alpha, &binding)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        delegation::questions::recover(
            &alpha,
            &binding,
            window["id"].as_str().unwrap(),
            &identity,
            || Ok(()),
        )
        .unwrap();
        let advanced = alpha.bytes("snapshot.json").unwrap();
        run_at(app.path()).unwrap();
        assert_eq!(alpha.bytes("snapshot.json").unwrap(), advanced);
        assert!(!expected_profile(app.path(), "prepared").unwrap());
        let before = crate::session::afk::overview::tests::tree(app.path());
        let overview = crate::session::afk::overview::snapshot_at(app.path(), 1000).unwrap();
        assert!(overview.complete(), "{overview:?}");
        assert_eq!(
            before,
            crate::session::afk::overview::tests::tree(app.path())
        );
    }
}
fn state_original(app: &Path, profile: &str) -> Vec<u8> {
    let m = manifest(app).unwrap();
    let i = m.entries.iter().position(|e| e.profile == profile).unwrap();
    store(app, false)
        .unwrap()
        .bytes(&original_name(i))
        .unwrap()
        .unwrap()
}
#[test]
fn v42_retries_reestablish_successor_file_and_directory_durability() {
    let app = fixture();
    INTERRUPT.with(|at| at.set(Some(Phase::Fences)));
    assert!(run_at(app.path()).is_err());
    INTERRUPT.with(|at| at.set(None));
    let target = profile_store(app.path(), "alpha", true).unwrap();
    drop(target.lock(TIMEOUT).unwrap());
    for _ in 0..2 {
        crate::session::anchored_fs::FAIL_SYNC_ONCE
            .with(|p| *p.borrow_mut() = Some(target.dir.path().to_owned()));
        let result = run_at(app.path());
        crate::session::anchored_fs::FAIL_SYNC_ONCE.with(|p| p.borrow_mut().take());
        assert!(result.is_err());
        assert!(target.bytes("snapshot.json").unwrap().is_some());
        assert!(manifest(app.path()).unwrap().phase < Phase::Imports);
    }
    run_at(app.path()).unwrap();
    fs::write(
        app.path().join(".schema_version"),
        cursor::USAGE.to_string(),
    )
    .unwrap();
    ready(app.path()).unwrap();
}

#[test]
fn pending_journals_and_changed_originals_are_not_upgrade_repair_opportunities() {
    for case in ["pending-old", "pending-new", "original", "missing-import"] {
        let app = fixture();
        if case == "pending-old" {
            let old = old_store(app.path(), "main").unwrap();
            let before = old.bytes("snapshot.json").unwrap();
            old.write_exclusive(journal::PENDING, b"unknown prior publication")
                .unwrap();
            assert!(run_at(app.path()).is_err());
            assert_eq!(old.bytes("snapshot.json").unwrap(), before);
            assert!(old.bytes(journal::PENDING).unwrap().is_some());
            continue;
        }
        INTERRUPT.with(|p| p.set(Some(Phase::Imports)));
        assert!(run_at(app.path()).is_err());
        INTERRUPT.with(|p| p.set(None));
        match case {
            "pending-new" => {
                profile_store(app.path(), "main", false)
                    .unwrap()
                    .write_exclusive(journal::PENDING, b"pending")
                    .unwrap();
            }
            "original" => {
                let m = manifest(app.path()).unwrap();
                let i = m.entries.iter().position(|e| e.profile == "main").unwrap();
                store(app.path(), false)
                    .unwrap()
                    .write_bytes(&original_name(i), b"changed")
                    .unwrap();
            }
            _ => {
                fs::remove_file(
                    app.path()
                        .join("afk-authority-v3/profiles/main/snapshot.json"),
                )
                .unwrap();
            }
        }
        assert!(run_at(app.path()).is_err(), "{case}");
        assert!(ready(app.path()).is_err());
    }
}
