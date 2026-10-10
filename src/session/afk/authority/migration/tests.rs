use super::*;
use serde_json::json;
use std::os::unix::fs::MetadataExt;

pub(super) struct Isolated(tempfile::TempDir);
impl Isolated {
    pub(super) fn path(&self) -> &Path {
        self.0.path()
    }
}
impl Drop for Isolated {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        // Discard the isolated fixture only after its legacy handles have been dropped.
        for ns in NAMESPACES.into_iter().chain(["afk-authority-v1/profiles"]) {
            let _ = std::fs::set_permissions(
                self.path().join(ns),
                std::fs::Permissions::from_mode(0o700),
            );
        }
    }
}
pub(super) fn isolated() -> Isolated {
    Isolated(tempfile::tempdir().unwrap())
}

fn legacy(app: &Path, namespace: &str, id: &str, value: &Value) -> Store {
    let store = Store::open_namespace(app, id, true, namespace, delegation::SESSION_BYTES).unwrap();
    store.write(leaf(namespace), value).unwrap();
    store
}
fn window(profile: &str) -> Value {
    json!({"version":4,"binding":{"instance_id":"test","profile":profile,"native_id":"native","launch_id":"launch"},
        "generation":uuid::Uuid::new_v4().to_string(),"id":uuid::Uuid::new_v4().to_string(),"revision":1,
        "root":"/workspace","control_root":"/app","root_identity":[1,2],
        "grant":{"version":4,"task":"scratch","scope":"create scratch","files":[{"path":"scratch.txt","capability":"create"}],"requests":2,"question_deferrals":1,"assurance":"stock-sdk-main-loop-reservations"},
        "issued_at_ms":1000,"expires_at_ms":61000,"state":"pending","confirmed":true,
        "reservations":[],"reads":0,"records":[],"permit":null,"read_facts":[],"checkpoints":[],"nudges":[],"terminal_reason":null,
        "question":{"identity":{"operation":uuid::Uuid::new_v4().to_string(),"interaction_id":uuid::Uuid::new_v4().to_string(),"tool_call_id":"call","context_ref":"context","kind":"input","arguments_hash":"a".repeat(64)},"admitted_at_ms":2000,"emission_intent_at_ms":2001,"outcome":"unknown","outcome_at_ms":2002,"observed_request":null,"recovered_at_ms":null}})
}
fn fixture(app: &Path) -> (Store, Store, Value) {
    let mut policy = super::super::super::tests::policy(super::super::super::tests::binding());
    policy.binding.instance_id = "test".into();
    policy.binding.profile = "alpha".into();
    policy.revision = 7;
    let control = legacy(app, "afk", "test", &serde_json::to_value(policy).unwrap());
    let book = json!({"version":4,"windows":[window("alpha")]});
    let runtime = legacy(app, "afk-runtime-v4", "test", &book);
    (control, runtime, book)
}

#[test]
fn every_phase_resumes_with_exact_originals_stable_legacy_handles_and_question_recovery() {
    for phase in [
        Phase::Roots,
        Phase::Sealed,
        Phase::Inventory,
        Phase::Originals,
        Phase::Fences,
        Phase::Imports,
        Phase::Verified,
        Phase::Complete,
    ] {
        let app = isolated();
        let (control, runtime, book) = fixture(app.path());
        let original_policy = control.bytes("policy.json").unwrap().unwrap();
        let original_book = runtime.bytes("ledger.json").unwrap().unwrap();
        let locks = [
            control.lock(TIMEOUT).unwrap(),
            runtime.lock(TIMEOUT).unwrap(),
        ];
        let lock_inodes = [
            locks[0].metadata().unwrap().ino(),
            locks[1].metadata().unwrap().ino(),
        ];
        drop(locks);
        let directories = [
            control.dir.identity().unwrap(),
            runtime.dir.identity().unwrap(),
        ];
        INTERRUPT.with(|at| at.set(Some(phase)));
        assert!(run_at(app.path()).is_err(), "{phase:?}");
        INTERRUPT.with(|at| at.set(None));
        assert!(ready(app.path()).is_err());
        assert!(Store::authority(app.path(), "alpha", "test", true).is_err());
        run_at(app.path()).unwrap();
        run_at(app.path()).unwrap();
        let m = manifest(app.path()).unwrap();
        assert_eq!(m.phase, Phase::Complete);
        let evidence = migration_store(app.path(), false).unwrap();
        assert_eq!(
            evidence.bytes(&original_leaf(0)).unwrap().unwrap(),
            original_policy
        );
        assert_eq!(
            evidence.bytes(&original_leaf(1)).unwrap().unwrap(),
            original_book
        );
        assert!(control.read::<Policy>("policy.json").is_err());
        assert!(runtime.read::<Book>("ledger.json").is_err());
        assert_eq!(
            [
                control.dir.identity().unwrap(),
                runtime.dir.identity().unwrap()
            ],
            directories
        );
        assert_eq!(
            [
                control.lock(TIMEOUT).unwrap().metadata().unwrap().ino(),
                runtime.lock(TIMEOUT).unwrap().metadata().unwrap().ino()
            ],
            lock_inodes
        );
        std::fs::write(app.path().join(".schema_version"), "40").unwrap();
        super::super::migration_v2::run_at(app.path()).unwrap();
        std::fs::write(app.path().join(".schema_version"), "41").unwrap();
        let authority = Store::authority(app.path(), "alpha", "test", false).unwrap();
        let mut expected = book.clone();
        expected["windows"][0]["state"] = json!("ended");
        expected["windows"][0]["confirmed"] = json!(false);
        expected["windows"][0]["terminal_reason"] = json!("delivery_unknown");
        let session = authority.session().unwrap();
        assert_eq!(
            serde_json::to_value(&session.book).unwrap(),
            expected_with_grant_defaults(expected)
        );
        let policy = session.policy.unwrap();
        assert!(!policy.enabled);
        assert_eq!(policy.revision, 9);
        let binding: Binding =
            serde_json::from_value(book["windows"][0]["binding"].clone()).unwrap();
        let guards = delegation::questions::guards(&authority, &binding).unwrap();
        assert_eq!(
            guards[0]["identity"],
            book["windows"][0]["question"]["identity"]
        );
        delegation::questions::recover(
            &authority,
            &binding,
            book["windows"][0]["id"].as_str().unwrap(),
            &serde_json::from_value(book["windows"][0]["question"]["identity"].clone()).unwrap(),
            || Ok(()),
        )
        .unwrap();
        assert!(delegation::questions::guards(&authority, &binding)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        let advanced = authority.bytes("snapshot.json").unwrap();
        assert!(run_at(app.path()).is_err());
        super::super::migration_v2::run_at(app.path()).unwrap();
        assert_eq!(authority.bytes("snapshot.json").unwrap(), advanced);
    }
}
fn expected_with_grant_defaults(mut book: Value) -> Value {
    book["windows"][0]["grant"]["settlement_nudges"] = json!(0);
    book
}

#[test]
fn legacy_evidence_has_exact_ownership_without_protocol_promotion_or_empty_default() {
    let app = isolated();
    let (control, _, _) = fixture(app.path());
    let mut old = window("beta");
    old["version"] = json!(2);
    old["grant"]["version"] = json!(2);
    let v2 = legacy(
        app.path(),
        "afk-runtime-v2",
        "test",
        &json!({"version":2,"windows":[old]}),
    );
    let original = v2.bytes("ledger.json").unwrap().unwrap();
    legacy(
        app.path(),
        "afk-runtime-v3",
        "test",
        &json!({"version":3,"windows":[]}),
    );
    legacy(
        app.path(),
        "afk-runtime-v3",
        "unassigned",
        &json!({"version":3,"windows":[]}),
    );
    run_at(app.path()).unwrap();
    let m = manifest(app.path()).unwrap();
    assert_eq!(
        m.imports.keys().cloned().collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    let entry = m.entries.iter().find(|e| e.version == 2).unwrap();
    assert_eq!(entry.profiles, ["beta"]);
    assert_eq!(
        std::fs::read(app.path().join(&entry.original)).unwrap(),
        original
    );
    let associated = m
        .entries
        .iter()
        .find(|e| e.version == 3 && e.instance == "test")
        .unwrap();
    assert_eq!(associated.profiles, ["alpha"]);
    assert_eq!(unassigned(app.path()).unwrap()[0].instance, "unassigned");
    std::fs::write(app.path().join(".schema_version"), "40").unwrap();
    super::super::migration_v2::run_at(app.path()).unwrap();
    std::fs::write(app.path().join(".schema_version"), "41").unwrap();
    let beta = Store::authority(app.path(), "beta", "test", false)
        .unwrap()
        .session()
        .unwrap();
    assert_eq!(
        serde_json::to_value(beta.book).unwrap(),
        json!({"version":4,"windows":[]})
    );
    assert_eq!(beta.evidence[0].version, 2);
    assert!(control.read::<Policy>("policy.json").is_err());
    assert!(v2.read::<Book>("ledger.json").is_err());
}

#[test]
fn malformed_originals_ownership_conflicts_and_advanced_imports_fail_closed() {
    for case in [
        "corrupt",
        "foreign",
        "oversized",
        "backup",
        "advanced",
        "changed-source",
    ] {
        let app = isolated();
        let (_, runtime, _) = fixture(app.path());
        let original = runtime.bytes("ledger.json").unwrap().unwrap();
        match case {
            "corrupt" => {
                runtime
                    .write("ledger.json", &json!({"version":4,"windows":[{}]}))
                    .unwrap();
            }
            "foreign" => {
                let mut book: Value = serde_json::from_slice(&original).unwrap();
                book["windows"][0]["binding"]["instance_id"] = json!("other");
                runtime.write("ledger.json", &book).unwrap();
            }
            "oversized" => {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(runtime.dir.path().join("ledger.json"))
                    .unwrap()
                    .set_len((delegation::SESSION_BYTES + 1) as u64)
                    .unwrap();
            }
            _ => {
                INTERRUPT.with(|at| {
                    at.set(Some(if case == "advanced" {
                        Phase::Imports
                    } else {
                        Phase::Originals
                    }))
                });
                assert!(run_at(app.path()).is_err());
                INTERRUPT.with(|at| at.set(None));
                if case == "backup" {
                    migration_store(app.path(), false)
                        .unwrap()
                        .write_bytes(&original_leaf(1), b"changed")
                        .unwrap();
                } else if case == "advanced" {
                    let store = profile_store(app.path(), "alpha", false).unwrap();
                    let mut snapshot: Snapshot = store.read("snapshot.json").unwrap().unwrap();
                    snapshot.revision += 1;
                    store.write("snapshot.json", &snapshot).unwrap();
                } else {
                    runtime
                        .write("ledger.json", &json!({"version":4,"windows":[]}))
                        .unwrap();
                }
            }
        }
        assert!(run_at(app.path()).is_err(), "{case}");
        assert!(ready(app.path()).is_err());
        if ["corrupt", "foreign", "oversized"].contains(&case) {
            assert!(manifest(app.path()).unwrap().phase < Phase::Inventory);
        }
    }
}

#[test]
fn concurrent_migration_attempts_serialize_and_missing_imports_cannot_be_recreated() {
    use std::sync::{Arc, Barrier};
    let app = isolated();
    fixture(app.path());
    let start = Arc::new(Barrier::new(3));
    let peers: Vec<_> = (0..2)
        .map(|_| {
            let path = app.path().to_owned();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                run_at(&path).unwrap();
            })
        })
        .collect();
    start.wait();
    for peer in peers {
        peer.join().unwrap();
    }
    std::fs::write(app.path().join(".schema_version"), "40").unwrap();
    super::super::migration_v2::run_at(app.path()).unwrap();
    std::fs::write(app.path().join(".schema_version"), "41").unwrap();
    let store = super::super::profile_store(app.path(), "alpha", false).unwrap();
    let original = store.bytes("snapshot.json").unwrap().unwrap();
    for missing in ["session", "evidence"] {
        let mut snapshot: super::super::Snapshot = serde_json::from_slice(&original).unwrap();
        snapshot.revision += 1;
        if missing == "session" {
            snapshot.sessions.clear();
        } else {
            snapshot.sessions.get_mut("test").unwrap().evidence.clear();
        }
        store.write("snapshot.json", &snapshot).unwrap();
        let before = super::super::super::overview::tests::tree(app.path());
        let overview = super::super::super::overview::snapshot_at(app.path(), 10000).unwrap();
        assert!(!overview.complete(), "missing imported {missing}");
        assert!(overview.profiles.iter().all(|p| p.sessions.is_empty()));
        assert_eq!(
            before,
            super::super::super::overview::tests::tree(app.path())
        );
        assert!(Store::authority(app.path(), "alpha", "test", true).is_err());
        assert!(super::super::migration_v2::run_at(app.path()).is_err());
        assert_eq!(
            store.bytes("snapshot.json").unwrap().unwrap(),
            serde_json::to_vec(&snapshot).unwrap()
        );
    }
    std::fs::remove_file(store.dir.path().join("snapshot.json")).unwrap();
    assert!(Store::authority(app.path(), "alpha", "test", true).is_err());
    assert!(!store.dir.path().join("snapshot.json").exists());
}

#[test]
fn mixed_v4_history_partitions_only_by_stored_binding_and_preserves_all_windows() {
    let app = isolated();
    let first = window("alpha");
    let mut second = window("beta");
    second["revision"] = json!(2);
    let mut third = window("alpha");
    third["revision"] = json!(3);
    let original = vec![first, second, third];
    legacy(
        app.path(),
        "afk-runtime-v4",
        "test",
        &json!({"version":4,"windows":original}),
    );
    initialize_test(app.path());
    for (profile, indexes) in [("alpha", vec![0, 2]), ("beta", vec![1])] {
        let store = Store::authority(app.path(), profile, "test", false).unwrap();
        let book = serde_json::to_value(store.session().unwrap().book).unwrap();
        assert_eq!(book["windows"].as_array().unwrap().len(), indexes.len());
        for (w, index) in book["windows"].as_array().unwrap().iter().zip(indexes) {
            assert_eq!(w["id"], original[index]["id"]);
            assert_eq!(w["revision"], original[index]["revision"]);
            assert_eq!(w["question"], original[index]["question"]);
            assert_eq!(w["expires_at_ms"], original[index]["expires_at_ms"]);
            assert_eq!(w["state"], "ended");
            assert_eq!(w["confirmed"], false);
        }
    }
}

#[test]
fn absent_leaves_and_held_root_ingress_are_fenced_across_restart() {
    for phase in [Phase::Roots, Phase::Sealed, Phase::Inventory, Phase::Fences] {
        let app = isolated();
        let control = Store::open(app.path(), "prepared", true).unwrap();
        let runtime = Store::open_namespace(
            app.path(),
            "prepared",
            true,
            "afk-runtime-v4",
            delegation::SESSION_BYTES,
        )
        .unwrap();
        let held_root = AnchoredDir::open(app.path())
            .unwrap()
            .child(Path::new("afk-runtime-v4"))
            .unwrap();
        INTERRUPT.with(|at| at.set(Some(phase)));
        assert!(run_at(app.path()).is_err());
        INTERRUPT.with(|at| at.set(None));
        run_at(app.path()).unwrap();
        assert!(held_root.create_child(Path::new("after-seal")).is_err());
        assert!(!app.path().join("afk-runtime-v4/after-seal").exists());
        let _old_control_lock = control.lock(TIMEOUT).unwrap();
        let _old_runtime_lock = runtime.lock(TIMEOUT).unwrap();
        assert!(control.read::<Policy>("policy.json").is_err());
        assert!(runtime.read::<Book>("ledger.json").is_err());
        let m = manifest(app.path()).unwrap();
        assert_eq!(m.absent.len(), 2);
        assert!(m.entries.is_empty());
        assert!(m.imports.is_empty());
        assert!(!app.path().join(original_path(0)).exists());
    }
}

#[test]
fn creation_probe_and_sync_failure_refuse_unqualified_completion() {
    let app = isolated();
    let root = AnchoredDir::open(app.path())
        .unwrap()
        .create_child(Path::new("afk"))
        .unwrap();
    assert!(root
        .qualify_child_creation_seal()
        .unwrap_err()
        .to_string()
        .contains("unsupported"));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    crate::session::anchored_fs::FAIL_SYNC_ONCE
        .with(|path| *path.borrow_mut() = Some(root.path().to_owned()));
    assert!(run_at(app.path()).is_err());
    assert!(manifest(app.path()).unwrap().phase < Phase::Sealed);
    assert!(ready(app.path()).is_err());
    run_at(app.path()).unwrap();
    assert!(root.create_child(Path::new("denied")).is_err());
}

#[test]
fn retry_requires_import_directory_sync_after_a_visible_rename() {
    let app = isolated();
    fixture(app.path());
    INTERRUPT.with(|at| at.set(Some(Phase::Fences)));
    assert!(run_at(app.path()).is_err());
    INTERRUPT.with(|at| at.set(None));
    let target = profile_store(app.path(), "alpha", true).unwrap();
    drop(target.lock(TIMEOUT).unwrap());
    for attempt in 0..2 {
        crate::session::anchored_fs::FAIL_SYNC_ONCE
            .with(|path| *path.borrow_mut() = Some(target.dir.path().to_owned()));
        let result = run_at(app.path());
        crate::session::anchored_fs::FAIL_SYNC_ONCE.with(|path| path.borrow_mut().take());
        assert!(
            result.is_err(),
            "attempt {attempt} must establish import durability"
        );
        assert!(target.bytes("snapshot.json").unwrap().is_some());
        assert!(manifest(app.path()).unwrap().phase < Phase::Imports);
        assert!(ready(app.path()).is_err());
    }
    let visible = target.bytes("snapshot.json").unwrap();
    run_at(app.path()).unwrap();
    assert_eq!(target.bytes("snapshot.json").unwrap(), visible);
    assert_eq!(manifest(app.path()).unwrap().phase, Phase::Complete);
}

#[test]
fn unassigned_only_migration_errors_cannot_appear_as_a_complete_empty_fleet() {
    for condition in ["partial", "corrupt", "missing"] {
        let app = isolated();
        legacy(
            app.path(),
            "afk-runtime-v3",
            "unassigned",
            &json!({"version":3,"windows":[]}),
        );
        initialize_test(app.path());
        let original = super::super::super::overview::snapshot_at(app.path(), 10000).unwrap();
        assert!(original.complete());
        assert_eq!(original.migration_evidence.len(), 1);
        let store = migration_store(app.path(), false).unwrap();
        match condition {
            "partial" => {
                let mut m = manifest(app.path()).unwrap();
                m.phase = Phase::Imports;
                store.write("manifest.json", &m).unwrap();
            }
            "corrupt" => store.write_bytes("manifest.json", b"not json").unwrap(),
            _ => std::fs::remove_file(store.dir.path().join("manifest.json")).unwrap(),
        }
        let before = super::super::super::overview::tests::tree(app.path());
        let error = super::super::super::overview::snapshot_at(app.path(), 10000).unwrap_err();
        assert!(
            error.to_string().contains("migration"),
            "{condition}: {error}"
        );
        assert_eq!(
            before,
            super::super::super::overview::tests::tree(app.path())
        );
    }
}

#[test]
fn removed_registry_profiles_and_sessions_expose_only_historical_refs_and_guards() {
    let app = isolated();
    fixture(app.path());
    for version in [2, 3] {
        let mut old = window("removed-profile");
        old["version"] = json!(version);
        old["grant"]["task"] = json!("PRIVATE-GRANT-BODY");
        legacy(
            app.path(),
            &format!("afk-runtime-v{version}"),
            "test",
            &json!({"version":version,"windows":[old]}),
        );
    }
    initialize_test(app.path());
    let before = super::super::super::overview::tests::tree(app.path());
    let overview = super::super::super::overview::snapshot_at(app.path(), 10000).unwrap();
    assert!(overview.complete());
    assert!(overview.profiles.iter().all(|p| p.sessions.is_empty()));
    assert_eq!(overview.historical_authority.len(), 2);
    assert_eq!(
        overview
            .historical_authority
            .iter()
            .map(|h| h.evidence.len())
            .sum::<usize>(),
        4
    );
    let alpha = overview
        .historical_authority
        .iter()
        .find(|h| h.profile == "alpha")
        .unwrap();
    assert_eq!(alpha.windows[0]["unresolved_question_guard"], true);
    assert!(!serde_json::to_string(&overview)
        .unwrap()
        .contains("PRIVATE-GRANT-BODY"));
    assert_eq!(
        before,
        super::super::super::overview::tests::tree(app.path())
    );
}
