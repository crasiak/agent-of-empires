use super::*;
use serde_json::json;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};

fn grant() -> Value {
    json!({"version":4,"task":"scratch","scope":"inspect one scratch file","files":[{"path":"scratch.txt","capability":"create"}],"requests":2,"assurance":"stock-sdk-main-loop-reservations"})
}
fn manifest(ids: &[&str]) -> Manifest {
    serde_json::from_value(json!({"version":1,"profile":"alpha","minutes":10,"members":ids.iter().map(|id| json!({"session_id":id,"grant":grant()})).collect::<Vec<_>>()})).unwrap()
}
fn private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn instance(id: &str, workspace: &Path) -> Instance {
    let mut row = Instance::new(id, workspace.to_str().unwrap());
    row.id = id.into();
    row.tool = "pi".into();
    row.source_profile = "alpha".into();
    row.agent_session_id = Some(uuid::Uuid::new_v4().to_string());
    row.active_execution = Some(serde_json::from_value(json!({"launch_id":uuid::Uuid::new_v4().to_string(),"binding":{"agent":"pi","stores":[],"cwd":workspace,"cwd_filesystem":"host","filesystem":"host"}})).unwrap());
    row
}
fn registry(app: &Path, rows: &[Instance]) {
    let dir = app.join("profiles/alpha");
    fs::create_dir_all(&dir).unwrap();
    private(
        &dir.join("sessions.json"),
        &serde_json::to_vec(rows).unwrap(),
    );
}
fn prepare(app: &Path, id: &str) {
    Store::authority(app, "alpha", id, true).unwrap();
    Store::transport(app, "alpha", id, true).unwrap();
}

#[test]
fn manifest_validation_is_private_bounded_exact_and_all_or_nothing() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let input = root.path().join("manifest.json");
    let valid = serde_json::to_value(manifest(&["one"])).unwrap();
    for budget in [Value::Null, json!(1), json!(128)] {
        let mut value = valid.clone();
        value["shared_requests"] = budget;
        private(&input, &serde_json::to_vec(&value).unwrap());
        read_manifest(&input).unwrap();
    }
    for case in [
        "duplicate",
        "profile",
        "cross-profile",
        "title",
        "empty",
        "too-many",
        "budget0",
        "budget129",
        "duration0",
        "duration1441",
        "grant",
        "grant-size",
        "unknown",
        "malformed",
        "oversized",
        "mode",
        "link",
        "hardlink",
    ] {
        let mut value = valid.clone();
        match case {
            "duplicate" => value["members"] = json!([valid["members"][0], valid["members"][0]]),
            "profile" => value["profile"] = json!("../beta"),
            "cross-profile" => value["members"][0]["profile"] = json!("beta"),
            "title" => value["members"][0]["session_id"] = json!("not an exact id"),
            "empty" => value["members"] = json!([]),
            "too-many" => {
                value["members"] = json!((0..17)
                    .map(|i| json!({"session_id":format!("s{i}"),"grant":grant()}))
                    .collect::<Vec<_>>())
            }
            "budget0" => value["shared_requests"] = json!(0),
            "budget129" => value["shared_requests"] = json!(129),
            "duration0" => value["minutes"] = json!(0),
            "duration1441" => value["minutes"] = json!(1441),
            "grant" => value["members"][0]["grant"]["files"][0]["path"] = json!("../escape.txt"),
            "grant-size" => {
                value["members"][0]["grant"]["scope"] = json!("x".repeat(delegation::RECORD_BYTES))
            }
            "unknown" => value["apply"] = json!(true),
            _ => {}
        }
        private(&input, &serde_json::to_vec(&value).unwrap());
        match case {
            "malformed" => private(&input, b"{"),
            "oversized" => private(&input, &vec![b' '; ARTIFACT_BYTES + 1]),
            "mode" => fs::set_permissions(&input, fs::Permissions::from_mode(0o644)).unwrap(),
            "link" => {
                fs::rename(&input, root.path().join("target")).unwrap();
                symlink("target", &input).unwrap();
            }
            "hardlink" => fs::hard_link(&input, root.path().join("hardlink")).unwrap(),
            _ => {}
        }
        assert!(read_manifest(&input).is_err(), "{case}");
        fs::remove_file(&input).unwrap();
    }
}

#[test]
fn inspection_freezes_selection_and_never_creates_authority_or_claims_apply() {
    let app = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    authority::migration::initialize_test(app.path());
    let mut rows = vec![
        instance("one", workspace.path()),
        instance("stopped", workspace.path()),
        instance("unsupported", workspace.path()),
        instance("archived", workspace.path()),
        instance("uninitialized", workspace.path()),
    ];
    rows[0].title = "missing".into();
    rows[1].status = crate::session::Status::Stopped;
    rows[2].tool = "claude".into();
    rows[3].archive();
    registry(app.path(), &rows);
    prepare(app.path(), "one");
    let other_profile = Store::authority(app.path(), "beta", "one", true).unwrap();
    let other_bytes = other_profile.bytes("snapshot.json").unwrap();
    let snapshot = app
        .path()
        .join("afk-authority-v1/profiles/alpha/snapshot.json");
    let before = fs::read(&snapshot).unwrap();
    let generation = uuid::Uuid::new_v4().to_string();
    let mut calls = 0;
    let artifact = inspect_at(
        app.path(),
        manifest(&[
            "one",
            "stopped",
            "unsupported",
            "archived",
            "uninitialized",
            "missing",
        ]),
        |_, binding| {
            assert_eq!(binding.instance_id, "one");
            assert_eq!(binding.profile, "alpha");
            calls += 1;
            if calls == 1 {
                rows.push(instance("newly-created", workspace.path()));
                registry(app.path(), &rows);
            }
            Ok(generation.clone())
        },
    )
    .unwrap();
    assert_eq!(calls, 2);
    assert!(!artifact.content.apply_available);
    assert_eq!(artifact.content.shared_budget, "No shared budget");
    assert_eq!(
        artifact.content.bulk_exclusion,
        "unrecorded/not implemented"
    );
    assert_eq!(artifact.content.members.len(), 6);
    assert!(artifact.content.members[0].exclusions.is_empty());
    assert_eq!(artifact.content.members[0].observations.len(), 2);
    for (index, reason) in [
        (1, Exclusion::Stopped),
        (2, Exclusion::Unsupported),
        (3, Exclusion::Archived),
        (4, Exclusion::Uninitialized),
        (5, Exclusion::Missing),
    ] {
        assert!(artifact.content.members[index].exclusions.contains(&reason));
    }
    assert_eq!(fs::read(snapshot).unwrap(), before);
    assert_eq!(other_profile.bytes("snapshot.json").unwrap(), other_bytes);
    assert_eq!(artifact.digest, digest(&artifact.content).unwrap());
    let mut changed = serde_json::to_value(&artifact.content).unwrap();
    changed["minutes"] = json!(11);
    assert_ne!(artifact.digest, canonical_digest(changed).unwrap());
}

#[test]
fn probe_rechecks_generation_epoch_binding_and_workspace_without_holding_profile_lock() {
    for changed in [
        "generation",
        "epoch",
        "binding",
        "workspace",
        "workspace-identity",
        "timeout",
    ] {
        let app = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let moved = tempfile::tempdir().unwrap();
        authority::migration::initialize_test(app.path());
        let mut rows = vec![instance("one", workspace.path())];
        registry(app.path(), &rows);
        prepare(app.path(), "one");
        let generation = uuid::Uuid::new_v4().to_string();
        let mut calls = 0;
        let artifact = inspect_at(app.path(), manifest(&["one"]), |_, _| {
            calls += 1;
            let store = Store::authority(app.path(), "alpha", "one", false).unwrap();
            let _lock = store
                .lock_existing(Duration::ZERO)
                .expect("no authority lock during transport observation");
            if calls == 1 {
                match changed {
                    "epoch" => {
                        let mut session = store.session().unwrap();
                        session.revoke().unwrap();
                        store.publish_session(session).unwrap();
                    }
                    "binding" => {
                        rows[0].agent_session_id = Some(uuid::Uuid::new_v4().to_string());
                        registry(app.path(), &rows);
                    }
                    "workspace" => {
                        rows[0].project_path = moved.path().to_str().unwrap().into();
                        registry(app.path(), &rows);
                    }
                    "workspace-identity" => {
                        fs::rename(workspace.path(), moved.path().join("old-root")).unwrap();
                        fs::create_dir(workspace.path()).unwrap();
                    }
                    "timeout" => anyhow::bail!("test timeout"),
                    _ => {}
                }
            }
            Ok(if changed == "generation" && calls == 2 {
                uuid::Uuid::new_v4().to_string()
            } else {
                generation.clone()
            })
        })
        .unwrap();
        let exclusions = &artifact.content.members[0].exclusions;
        assert!(
            exclusions.contains(&match changed {
                "generation" => Exclusion::StaleGeneration,
                "epoch" => Exclusion::StaleAuthority,
                "binding" => Exclusion::StaleBinding,
                "workspace" | "workspace-identity" => Exclusion::StaleWorkspace,
                _ => Exclusion::Unacknowledged,
            }),
            "{changed}: {exclusions:?}"
        );
        if changed == "timeout" {
            assert!(!exclusions.contains(&Exclusion::Unsupported));
        }
    }
}

#[test]
fn output_is_private_exclusive_and_unready_inspection_does_not_initialize() {
    let app = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    fs::set_permissions(files.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let input = files.path().join("manifest.json");
    let output = files.path().join("preview.json");
    private(
        &input,
        &serde_json::to_vec(&manifest(&["missing"])).unwrap(),
    );
    assert!(preview_at(app.path(), &input, &output).is_err());
    assert_eq!(fs::read_dir(app.path()).unwrap().count(), 0);
    assert!(!output.exists());
    authority::migration::initialize_test(app.path());
    let migration = app.path().join("afk-authority-v1/migration/manifest.json");
    let original = fs::read(&migration).unwrap();
    for state in ["older", "partial"] {
        if state == "older" {
            fs::write(app.path().join(".schema_version"), "39").unwrap();
        } else {
            let mut value: Value = serde_json::from_slice(&original).unwrap();
            value["phase"] = json!("imports");
            fs::write(&migration, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let before = super::super::overview::tests::tree(app.path());
        assert!(preview_at(app.path(), &input, &output)
            .err()
            .unwrap()
            .to_string()
            .contains("aoe migrate"));
        assert_eq!(before, super::super::overview::tests::tree(app.path()));
        assert!(!output.exists());
        fs::write(app.path().join(".schema_version"), "40").unwrap();
        fs::write(&migration, &original).unwrap();
    }
    preview_at(app.path(), &input, &output).unwrap();
    let before = fs::read(&output).unwrap();
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(preview_at(app.path(), &input, &output).is_err());
    assert_eq!(fs::read(&output).unwrap(), before);
    assert!(before.len() <= ARTIFACT_BYTES);
    let mut value: Value = serde_json::from_slice(&before).unwrap();
    let expected = value.as_object_mut().unwrap().remove("digest").unwrap();
    assert_eq!(expected, canonical_digest(value).unwrap());
}

#[test]
fn output_parent_changes_during_observation_refuse_publication() {
    for changed in ["moved-under-app", "replaced", "mode"] {
        let app = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        fs::set_permissions(files.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let export = files.path().join("export");
        fs::create_dir(&export).unwrap();
        fs::set_permissions(&export, fs::Permissions::from_mode(0o700)).unwrap();
        let moved = app.path().join("moved-export");
        let input = files.path().join("manifest.json");
        let output = export.join("preview.json");
        private(&input, &serde_json::to_vec(&manifest(&["one"])).unwrap());
        authority::migration::initialize_test(app.path());
        registry(app.path(), &[instance("one", workspace.path())]);
        prepare(app.path(), "one");
        let generation = uuid::Uuid::new_v4().to_string();
        let mut observed = 0;
        let result = preview_with(app.path(), &input, &output, |_, _| {
            if observed == 0 {
                match changed {
                    "mode" => {
                        fs::set_permissions(&export, fs::Permissions::from_mode(0o755)).unwrap()
                    }
                    _ => {
                        fs::rename(&export, &moved).unwrap();
                        if changed == "replaced" {
                            fs::create_dir(&export).unwrap();
                            fs::set_permissions(&export, fs::Permissions::from_mode(0o700))
                                .unwrap();
                        }
                    }
                }
            }
            observed += 1;
            Ok(generation.clone())
        });
        assert_eq!(observed, 2);
        assert!(result.is_err(), "changed output parent accepted: {changed}");
        assert!(!output.exists());
        assert!(!moved.join("preview.json").exists());
        for parent in [&export, &moved] {
            if parent.exists() {
                assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
            }
        }
    }
}

#[test]
fn blocked_transport_acquisition_leaves_authority_available_for_off() {
    let app = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    authority::migration::initialize_test(app.path());
    registry(app.path(), &[instance("one", workspace.path())]);
    prepare(app.path(), "one");
    let transport = Store::transport(app.path(), "alpha", "one", false).unwrap();
    let held = transport.lock(TIMEOUT).unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let path = app.path().to_owned();
    let peer = std::thread::spawn(move || {
        store::LOCK_CONTENDED.with(|slot| *slot.borrow_mut() = Some(sent));
        inspect_at(&path, manifest(&["one"]), |_, _| {
            panic!("revoked selection must not probe")
        })
        .unwrap()
    });
    assert_eq!(
        received.recv_timeout(TIMEOUT).unwrap(),
        transport.dir.path()
    );
    let authority = Store::authority(app.path(), "alpha", "one", false).unwrap();
    {
        let _lock = authority.lock_existing(Duration::ZERO).unwrap();
        let mut session = authority.session().unwrap();
        session.revoke().unwrap();
        authority.publish_session(session).unwrap();
    }
    drop(held);
    let result = peer.join().unwrap();
    assert!(result.content.members[0]
        .exclusions
        .contains(&Exclusion::StaleAuthority));
}

#[test]
fn active_and_unresolved_history_are_not_probed_replaced_or_recovered() {
    let app = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    authority::migration::initialize_test(app.path());
    let mut trashed = instance("trashed", workspace.path());
    trashed.trash();
    let rows = vec![
        instance("active", workspace.path()),
        instance("guard", workspace.path()),
        trashed,
    ];
    registry(app.path(), &rows);
    for (index, id) in ["active", "guard"].into_iter().enumerate() {
        prepare(app.path(), id);
        let store = Store::authority(app.path(), "alpha", id, false).unwrap();
        let generation = uuid::Uuid::new_v4().to_string();
        let mut window = json!({"version":4,"binding":Binding::for_instance(&rows[index]).unwrap(),"generation":generation,"id":uuid::Uuid::new_v4().to_string(),"revision":1,
            "root":workspace.path(),"control_root":app.path(),"root_identity":[1,2],"grant":grant(),
            "issued_at_ms":1000,"expires_at_ms":61000,"state":"pending","confirmed":false,"reservations":[],"reads":0,"records":[],"permit":null,"read_facts":[],"checkpoints":[],"nudges":[],"terminal_reason":null,"question":null});
        if id == "guard" {
            window["state"] = json!("ended");
            window["terminal_reason"] = json!("delivery_unknown");
            window["grant"]["question_deferrals"] = json!(1);
            window["question"] = json!({"identity":{"operation":uuid::Uuid::new_v4().to_string(),"interaction_id":uuid::Uuid::new_v4().to_string(),"tool_call_id":"call","context_ref":"context","kind":"input","arguments_hash":"a".repeat(64)},"admitted_at_ms":2000,"emission_intent_at_ms":2001,"outcome":"unknown","outcome_at_ms":2002,"observed_request":null,"recovered_at_ms":null});
        }
        let _lock = store.lock(TIMEOUT).unwrap();
        store
            .write("ledger.json", &json!({"version":4,"windows":[window]}))
            .unwrap();
    }
    let before = super::super::overview::tests::tree(app.path());
    let artifact = inspect_at(
        app.path(),
        manifest(&["active", "guard", "trashed"]),
        |_, _| panic!("excluded rows must not probe"),
    )
    .unwrap();
    assert!(artifact.content.members[0]
        .exclusions
        .contains(&Exclusion::AlreadyDelegating));
    assert!(artifact.content.members[1]
        .exclusions
        .contains(&Exclusion::UnresolvedHuman));
    assert!(artifact.content.members[2]
        .exclusions
        .contains(&Exclusion::Trashed));
    assert_eq!(before, super::super::overview::tests::tree(app.path()));
}

#[test]
fn proposed_budget_and_publication_limits_never_invent_charges_or_partial_output() {
    let app = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    fs::set_permissions(files.path(), fs::Permissions::from_mode(0o700)).unwrap();
    authority::migration::initialize_test(app.path());
    let mut proposed = manifest(&["missing"]);
    proposed.shared_requests = Some(128);
    let before = super::super::overview::tests::tree(app.path());
    let artifact = inspect_at(app.path(), proposed, |_, _| {
        panic!("missing member must not probe")
    })
    .unwrap();
    assert_eq!(artifact.content.shared_requests, Some(128));
    assert!(artifact
        .content
        .shared_budget
        .contains("not enforced or charged"));
    assert!(!artifact.content.apply_available);
    assert_eq!(before, super::super::overview::tests::tree(app.path()));

    let mut row = Instance::new(&"x".repeat(ARTIFACT_BYTES), "/workspace");
    row.id = "large".into();
    registry(app.path(), &[row]);
    let input = files.path().join("manifest.json");
    let output = files.path().join("preview.json");
    private(&input, &serde_json::to_vec(&manifest(&["large"])).unwrap());
    assert!(preview_at(app.path(), &input, &output)
        .err()
        .unwrap()
        .to_string()
        .contains("256 KiB"));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(files.path()).unwrap().count(), 1);
    let unsafe_output = app.path().join("new-authority.json");
    fs::set_permissions(app.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(preview_at(app.path(), &input, &unsafe_output).is_err());
    assert!(!unsafe_output.exists());
}
