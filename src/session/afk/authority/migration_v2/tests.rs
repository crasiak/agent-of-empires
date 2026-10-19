use super::v040 as frozen;
use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

struct App(tempfile::TempDir);
impl App {
    fn path(&self) -> &Path {
        self.0.path()
    }
}
impl Drop for App {
    fn drop(&mut self) {
        for path in [
            "afk",
            "afk-runtime-v2",
            "afk-runtime-v3",
            "afk-runtime-v4",
            "afk-authority-v1/profiles",
            "afk-authority-v2/profiles",
        ] {
            let _ = fs::set_permissions(self.path().join(path), fs::Permissions::from_mode(0o700));
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
    migration::run_at(app.path()).unwrap();
    fs::write(
        app.path().join(".schema_version"),
        cursor::AUTHORITY.to_string(),
    )
    .unwrap();
    assert_eq!(
        frozen::profile_store(app.path(), "alpha", false)
            .unwrap()
            .bytes("snapshot.json")
            .unwrap()
            .unwrap(),
        migration::golden::SNAPSHOT.as_bytes()
    );
    app
}

#[test]
fn every_upgrade_boundary_preserves_frozen_history_and_fences_held_handles() {
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
        let held = frozen::profile_store(app.path(), "alpha", false).unwrap();
        let old_identity = held.dir.identity().unwrap();
        let held_root = ingress(app.path()).unwrap();
        INTERRUPT.with(|at| at.set(Some(phase)));
        assert!(run_at(app.path()).is_err(), "{phase:?}");
        INTERRUPT.with(|at| at.set(None));
        assert!(ready(app.path()).is_err());
        assert!(Store::authority(app.path(), "alpha", "fixture", true).is_err());
        assert!(migration::run_at(app.path()).is_err());
        run_at(app.path()).unwrap();
        run_at(app.path()).unwrap();
        assert!(held.read::<frozen::Snapshot>("snapshot.json").is_err());
        assert_eq!(held.dir.identity().unwrap(), old_identity);
        assert!(held_root.create_child(Path::new("late-profile")).is_err());
        let state = store(app.path(), false).unwrap();
        assert_eq!(
            state.bytes(&original_name(0)).unwrap().unwrap(),
            migration::golden::SNAPSHOT.as_bytes()
        );
        let mut remaining = 64 * 1024 * 1024;
        migration::historical_metadata(app.path(), &mut remaining).unwrap();
        let v040: Value = serde_json::from_slice(
            &fs::read(app.path().join("afk-authority-v1/migration/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            v040["imports"]["alpha"],
            "f6ba50ee5c3f424450f90b4b1a60126d020984db2c8520c4062920ff5492bc08"
        );
        assert_eq!(
            hash(
                profile_store(app.path(), "alpha", false)
                    .unwrap()
                    .bytes("snapshot.json")
                    .unwrap()
                    .unwrap()
            ),
            "644787c82e846a27579003d47c37add375bdcebf9edf08d4a9d0457a25084c53"
        );
        fs::write(
            app.path().join(".schema_version"),
            cursor::BATCHES.to_string(),
        )
        .unwrap();
        super::super::migration_v3::run_at(app.path()).unwrap();
        fs::write(
            app.path().join(".schema_version"),
            cursor::USAGE.to_string(),
        )
        .unwrap();
        let current = Store::authority(app.path(), "alpha", "fixture", false).unwrap();
        let session = current.session().unwrap();
        assert_eq!(session.epoch, 3);
        assert_eq!(session.policy.as_ref().unwrap().revision, 10);
        assert!(!session.policy.unwrap().enabled);
        let book = serde_json::to_value(&session.book).unwrap();
        assert_eq!(book["windows"][0]["confirmed"], false);
        assert_eq!(book["windows"][0]["state"], "ended");
        assert_eq!(book["windows"][0]["expires_at_ms"], 61000);
        let binding = serde_json::from_value(book["windows"][0]["binding"].clone()).unwrap();
        let identity =
            serde_json::from_value(book["windows"][0]["question"]["identity"].clone()).unwrap();
        assert_eq!(
            delegation::questions::guards(&current, &binding)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        delegation::questions::recover(
            &current,
            &binding,
            book["windows"][0]["id"].as_str().unwrap(),
            &identity,
            || Ok(()),
        )
        .unwrap();
        let advanced = current.bytes("snapshot.json").unwrap();
        assert!(run_at(app.path()).is_err());
        super::super::migration_v3::run_at(app.path()).unwrap();
        assert_eq!(current.bytes("snapshot.json").unwrap(), advanced);
    }
}

#[test]
fn prepared_absence_and_runtime_created_profiles_are_included() {
    let app = fixture();
    let prepared = frozen::profile_store(app.path(), "prepared", true).unwrap();
    let runtime = frozen::profile_store(app.path(), "runtime", true).unwrap();
    let mut old = frozen::Snapshot::empty("runtime");
    old.sessions
        .insert("new-session".into(), frozen::Session::empty());
    runtime.write_raw("snapshot.json", &old).unwrap();
    run_at(app.path()).unwrap();
    fs::write(
        app.path().join(".schema_version"),
        cursor::BATCHES.to_string(),
    )
    .unwrap();
    assert!(prepared.read::<frozen::Snapshot>("snapshot.json").is_err());
    assert!(!expected_profile(app.path(), "prepared").unwrap());
    super::super::migration_v3::run_at(app.path()).unwrap();
    fs::write(
        app.path().join(".schema_version"),
        cursor::USAGE.to_string(),
    )
    .unwrap();
    let current = Store::authority(app.path(), "runtime", "new-session", false).unwrap();
    assert_eq!(current.session().unwrap().epoch, 2);
    assert_eq!(manifest(app.path()).unwrap().entries.len(), 3);
    assert!(!app.path().join(original_path(1)).exists());
    let before = crate::session::afk::overview::tests::tree(app.path());
    let overview = crate::session::afk::overview::snapshot_at(app.path(), 10000).unwrap();
    assert!(
        overview.complete(),
        "prepared absence is not missing expected authority: {overview:?}"
    );
    assert!(overview.profiles.iter().all(|p| p.sessions.is_empty()));
    assert!(serde_json::to_string(&overview.migration_evidence)
        .unwrap()
        .contains("prepared"));
    assert_eq!(
        before,
        crate::session::afk::overview::tests::tree(app.path())
    );
}

#[test]
fn missing_corrupt_or_advanced_upgrade_state_never_resets_authority() {
    for case in [
        "missing-legacy-profile",
        "missing-legacy-snapshot",
        "corrupt-original",
        "conflicting-import",
        "quarantined-current",
    ] {
        let app = fixture();
        match case {
            "missing-legacy-profile" => {
                fs::rename(
                    app.path().join("afk-authority-v1/profiles/alpha"),
                    app.path().join("moved-alpha"),
                )
                .unwrap();
            }
            "missing-legacy-snapshot" => {
                fs::remove_file(
                    app.path()
                        .join("afk-authority-v1/profiles/alpha/snapshot.json"),
                )
                .unwrap();
            }
            _ => {
                INTERRUPT.with(|at| at.set(Some(Phase::Imports)));
                assert!(run_at(app.path()).is_err());
                INTERRUPT.with(|at| at.set(None));
                if case == "corrupt-original" {
                    store(app.path(), false)
                        .unwrap()
                        .write_bytes(&original_name(0), b"changed")
                        .unwrap();
                } else if case == "conflicting-import" {
                    let target = profile_store(app.path(), "alpha", false).unwrap();
                    let mut current: Snapshot = target.read_raw("snapshot.json").unwrap().unwrap();
                    current.revision += 1;
                    target.write_raw("snapshot.json", &current).unwrap();
                } else {
                    let target = profile_store(app.path(), "alpha", false).unwrap();
                    target
                        .write_exclusive(journal::PENDING, b"uncertain")
                        .unwrap();
                }
            }
        }
        assert!(run_at(app.path()).is_err(), "{case}");
        assert!(ready(app.path()).is_err());
    }
}

#[test]
fn malformed_manifest_reads_still_consume_the_aggregate_budget() {
    let app = fixture();
    run_at(app.path()).unwrap();
    let state = store(app.path(), false).unwrap();
    for bytes in [b"{invalid}".to_vec(), vec![b' '; MANIFEST_BYTES + 1]] {
        state.write_bytes("manifest.json", &bytes).unwrap();
        let mut remaining = MANIFEST_BYTES + 100;
        assert!(read_manifest(&state, &mut remaining).is_err());
        let consumed = if bytes.len() <= MANIFEST_BYTES {
            bytes.len()
        } else {
            0
        };
        assert_eq!(remaining, MANIFEST_BYTES + 100 - consumed);
    }
}

#[test]
fn import_sync_failure_is_reestablished_before_completion() {
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
}
