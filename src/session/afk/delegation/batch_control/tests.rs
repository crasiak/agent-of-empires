use super::*;
use crate::session::{ActiveExecution, ExecutionBinding, Status, Storage};

struct Fixture {
    app: PathBuf,
    _roots: Vec<tempfile::TempDir>,
    rows: Vec<Instance>,
    preview: batch::Artifact,
}
fn fixture(shared: Option<u16>) -> Fixture {
    let app = crate::session::get_app_dir().unwrap();
    authority::migration::initialize_test(&app);
    let mut roots = Vec::new();
    let mut rows = Vec::new();
    let mut generations = BTreeMap::new();
    for id in ["first", "second"] {
        let root = tempfile::Builder::new()
            .prefix("afk-batch-")
            .tempdir()
            .unwrap();
        let mut row = Instance::new(id, root.path().to_str().unwrap());
        row.id = id.into();
        row.tool = "pi".into();
        row.status = Status::Running;
        row.source_profile = "batch-control".into();
        row.agent_session_id = Some(uuid::Uuid::new_v4().to_string());
        row.active_execution = Some(ActiveExecution {
            launch_id: uuid::Uuid::new_v4().to_string(),
            capture: None,
            container: None,
            binding: ExecutionBinding {
                agent: "pi".into(),
                stores: vec![],
                configuration: vec![],
                cwd: root.path().into(),
                cwd_filesystem: "host".into(),
                filesystem: "host".into(),
                exported_default_store: None,
            },
        });
        Store::authority(&app, &row.source_profile, id, true).unwrap();
        Store::transport(&app, &row.source_profile, id, true).unwrap();
        generations.insert(id.to_owned(), uuid::Uuid::new_v4().to_string());
        roots.push(root);
        rows.push(row);
    }
    Storage::new_unwatched("batch-control")
        .unwrap()
        .update(|stored, _| {
            *stored = rows.clone();
            Ok(())
        })
        .unwrap();
    let manifest: batch::Manifest = serde_json::from_value(json!({"version":1,"profile":"batch-control","minutes":1,"shared_requests":shared,"members":rows.iter().map(|r|json!({"session_id":r.id,"grant":{"version":4,"task":"scratch","scope":"create scratch only","files":[{"path":"scratch.txt","capability":"create"}],"requests":2,"assurance":COVERAGE}})).collect::<Vec<_>>()})).unwrap();
    let preview = batch::inspect_at(&app, manifest, |_, b| {
        Ok(generations[&b.instance_id].clone())
    })
    .unwrap();
    Fixture {
        app,
        _roots: roots,
        rows,
        preview,
    }
}
#[test]
#[serial_test::serial]
fn apply_is_atomic_report_only_and_late_failure_cannot_end_replacement() {
    for case in ["confirm", "fail-second", "replacement", "off"] {
        let _home = crate::session::test_support::isolate_app_dir();
        let f = fixture(Some(2));
        let mut qualified = 0;
        let mut replacement = None;
        let result = apply_with(
            &f.app,
            f.preview.clone(),
            |transport, binding, window, budgeted| {
                let generation = f
                    .preview
                    .content
                    .members
                    .iter()
                    .find(|m| m.session_id == binding.instance_id)
                    .unwrap()
                    .observations[0]
                    .generation
                    .clone();
                if let Some(window) = window {
                    assert!(budgeted);
                    let store = authority::profile_store(&f.app, "batch-control", false).unwrap();
                    let _lock = store
                        .lock_existing(Duration::ZERO)
                        .expect("no profile lock across acknowledgement");
                    let mut snapshot: authority::Snapshot =
                        store.read_raw("snapshot.json").unwrap().unwrap();
                    assert_eq!(snapshot.batches.len(), 1);
                    if qualified == 0 {
                        assert!(snapshot
                            .sessions
                            .values()
                            .all(|s| s.book.windows.len() == 1));
                        assert!(snapshot
                            .sessions
                            .values()
                            .all(|s| !s.book.windows[0].confirmed));
                    }
                    qualified += 1;
                    if case == "fail-second" && binding.instance_id == "second" {
                        anyhow::bail!("injected lost acknowledgement");
                    }
                    if binding.instance_id == "first" && matches!(case, "replacement" | "off") {
                        let session = snapshot.sessions.get_mut("first").unwrap();
                        session.revoke().unwrap();
                        if case == "off" {
                            session.preference(Preference::Excluded).unwrap();
                        } else {
                            let mut newer = window.clone();
                            newer.id = uuid::Uuid::new_v4().to_string();
                            newer.revision += 1;
                            newer.confirmed = true;
                            replacement = Some(newer.id.clone());
                            session.book.windows.push(newer);
                        }
                        snapshot.revision += 1;
                        store.write_raw("snapshot.json", &snapshot).unwrap();
                    }
                    assert!(transport.dir.path().ends_with(&binding.instance_id));
                }
                Ok(generation)
            },
        )
        .unwrap();
        assert_eq!(qualified, 2);
        let op = f.preview.content.operation_id.clone();
        let before = status(&f.app, "batch-control", &op).unwrap();
        let retry = apply_with(&f.app, f.preview.clone(), |_, _, _, _| {
            panic!("retry must not acknowledge or activate")
        })
        .unwrap();
        assert_eq!(before, retry);
        assert_eq!(result["expires_at_ms"], retry["expires_at_ms"]);
        let mut conflict = f.preview.clone();
        conflict.digest = "b".repeat(64);
        assert!(apply_with(&f.app, conflict, |_, _, _, _| panic!(
            "conflict must not acknowledge"
        ))
        .is_err());
        let store = Store::authority(&f.app, "batch-control", "first", false).unwrap();
        let epoch = store.session().unwrap().epoch;
        off(&f.app, "batch-control", &op).unwrap();
        let first = store.session().unwrap();
        if let Some(id) = replacement {
            assert_eq!(first.book.windows.last().unwrap().id, id);
            assert!(first.book.windows.last().unwrap().confirmed);
            assert_ne!(first.book.windows.last().unwrap().state, "ended");
            assert_eq!(first.epoch, epoch);
        }
        if case == "off" {
            assert_eq!(first.preference, Preference::Excluded);
        }
        assert!(store.authority_snapshot().unwrap().batches[&op]
            .tickets
            .is_empty());
    }
}
#[test]
#[serial_test::serial]
fn reviewed_individual_controls_revoke_confirmed_batch_history() {
    use crate::session::afk::{control, Operation as Control};
    for (replace, action) in [
        (false, Control::Off),
        (false, Control::On { minutes: 1 }),
        (true, Control::Off),
        (true, Control::On { minutes: 1 }),
    ] {
        let _home = crate::session::test_support::isolate_app_dir();
        let f = fixture(Some(2));
        apply_with(&f.app, f.preview.clone(), |_, b, _, _| {
            Ok(f.preview
                .content
                .members
                .iter()
                .find(|m| m.session_id == b.instance_id)
                .unwrap()
                .observations[0]
                .generation
                .clone())
        })
        .unwrap();
        let store = Store::authority(&f.app, "batch-control", "first", false).unwrap();
        let w = store.session().unwrap().book.windows[0].clone();
        execute(
            &store,
            &w.binding,
            &w.generation,
            Request::Reserve {
                window: w.id.clone(),
                request: uuid::Uuid::new_v4().to_string(),
                question: None,
                continuation: None,
            },
            now_ms(),
        )
        .unwrap();
        if replace {
            let _lock = store.lock(TIMEOUT).unwrap();
            let mut session = store.session().unwrap();
            session.book.windows[0].end(TerminalReason::Interrupted);
            let mut next = w.clone();
            next.id = uuid::Uuid::new_v4().to_string();
            next.revision += 1;
            session.book.windows.push(next);
            session.epoch += 1;
            store.publish_session(session).unwrap();
        }
        let before = store.authority_snapshot().unwrap();
        control(&f.rows[0], action).unwrap();
        let after = store.authority_snapshot().unwrap();
        let member = &after.sessions["first"];
        assert!(member
            .book
            .windows
            .iter()
            .all(|w| w.state == "ended" && !w.confirmed));
        assert_eq!(
            member.preference,
            if action == Control::Off {
                Preference::Excluded
            } else {
                Preference::Unrecorded
            }
        );
        assert_eq!(
            serde_json::to_value(&before.batches).unwrap(),
            serde_json::to_value(&after.batches).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&before.sessions["second"]).unwrap(),
            serde_json::to_value(&after.sessions["second"]).unwrap()
        );
        control(&f.rows[0], Control::Off).unwrap();
        assert_eq!(store.session().unwrap().preference, Preference::Excluded);
        let overview = crate::session::afk::overview::snapshot_at(&f.app, now_ms()).unwrap();
        let profile = overview
            .profiles
            .iter()
            .find(|p| p.profile == "batch-control")
            .unwrap();
        assert_eq!(profile.batches[0]["shared_charges"], 1);
        let row = profile.sessions.iter().find(|s| s.id == "first").unwrap();
        assert_eq!(row.bulk_preference, "excluded");
        assert_eq!(row.batch.is_some(), !replace);
        assert!(
            !serde_json::to_string(&overview)
                .unwrap()
                .contains("scratch"),
            "overview leaked grant task or file payload"
        );
    }
}

#[test]
#[serial_test::serial]
fn reviewed_confirmation_failures_preserve_original_publication_classification() {
    use authority::journal::{fail_next, Fault, Parked};
    for batch in [true, false] {
        for (fault, committed) in [(Fault::Journal, false), (Fault::Unlink, true)] {
            let _home = crate::session::test_support::isolate_app_dir();
            let f = fixture(None);
            let store = Store::authority(&f.app, "batch-control", "first", false).unwrap();
            let error = if batch {
                apply_with(&f.app, f.preview.clone(), |_, b, w, _| {
                    if w.is_some() {
                        fail_next(fault);
                    }
                    Ok(f.preview
                        .content
                        .members
                        .iter()
                        .find(|m| m.session_id == b.instance_id)
                        .unwrap()
                        .observations[0]
                        .generation
                        .clone())
                })
                .unwrap_err()
            } else {
                let candidate = &f.preview.content.members[0];
                let now = now_ms();
                let window = pending_window(
                    &f.rows[0],
                    &f.app,
                    candidate.grant.clone(),
                    candidate.host.as_ref().unwrap().binding.clone().unwrap(),
                    candidate.observations[0].generation.clone(),
                    now,
                    now + 60_000,
                )
                .unwrap();
                let error = {
                    let _lock = store.lock(TIMEOUT).unwrap();
                    let mut session = store.session().unwrap();
                    session.book.windows.push(window.clone());
                    store.publish_session(session).unwrap();
                    fail_next(fault);
                    confirm_attempt(&store, &window, 0).unwrap_err()
                };
                failed_attempt(&store, &window.id, 0, error)
            };
            assert_eq!(
                error.downcast_ref::<Parked>().unwrap().committed,
                Some(committed),
                "batch={batch}: {error:#}"
            );
            let profile = authority::profile_store(&f.app, "batch-control", false).unwrap();
            let current: authority::Snapshot = profile.read_raw("snapshot.json").unwrap().unwrap();
            assert_eq!(
                current.sessions["first"].book.windows[0].confirmed,
                committed
            );
            assert!(profile.bytes("publication.pending.json").unwrap().is_some());
            assert!(status(&f.app, "batch-control", &f.preview.content.operation_id).is_err());
        }
    }
}

#[test]
#[serial_test::serial]
fn retained_operation_capacity_refuses_without_eviction_or_enrollment() {
    let _home = crate::session::test_support::isolate_app_dir();
    let f = fixture(None);
    let store = authority::profile_store(&f.app, "batch-control", false).unwrap();
    {
        let mut tx = authority::SessionTransaction::profile(&store, "batch-control").unwrap();
        let mut proposed = tx.snapshot().clone();
        for _ in 0..64 {
            let id = uuid::Uuid::new_v4().to_string();
            let now = now_ms();
            proposed.batches.insert(
                id.clone(),
                Operation {
                    id,
                    preview_digest: f.preview.digest.clone(),
                    issued_at_ms: now,
                    expires_at_ms: now + 60_000,
                    shared_requests: None,
                    stopped: false,
                    members: f
                        .preview
                        .content
                        .members
                        .iter()
                        .map(|m| {
                            (
                                m.session_id.clone(),
                                Member {
                                    binding: m.host.as_ref().unwrap().binding.clone(),
                                    generation: Some(m.observations[0].generation.clone()),
                                    window: None,
                                    grant_hash: m.grant_hash.clone(),
                                    control_epoch: 0,
                                    exception_epoch: 0,
                                    outcome: Outcome::Skipped,
                                    reason: Some("excluded".into()),
                                },
                            )
                        })
                        .collect(),
                    tickets: vec![],
                },
            );
        }
        tx.publish(proposed).unwrap().warn();
    }
    let before = store.bytes("snapshot.json").unwrap();
    let error = apply_with(&f.app, f.preview.clone(), |_, b, w, _| {
        assert!(
            w.is_none(),
            "capacity must refuse before grant qualification"
        );
        Ok(f.preview
            .content
            .members
            .iter()
            .find(|m| m.session_id == b.instance_id)
            .unwrap()
            .observations[0]
            .generation
            .clone())
    })
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("retained batch operation capacity"));
    assert_eq!(store.bytes("snapshot.json").unwrap(), before);
}

#[test]
#[serial_test::serial]
fn reviewed_overview_refuses_orphan_and_duplicate_membership_without_writes() {
    for duplicate in [false, true] {
        let _home = crate::session::test_support::isolate_app_dir();
        let f = fixture(Some(2));
        apply_with(&f.app, f.preview.clone(), |_, b, _, _| {
            Ok(f.preview
                .content
                .members
                .iter()
                .find(|m| m.session_id == b.instance_id)
                .unwrap()
                .observations[0]
                .generation
                .clone())
        })
        .unwrap();
        let store = authority::profile_store(&f.app, "batch-control", false).unwrap();
        let mut value: Value = store.read_raw("snapshot.json").unwrap().unwrap();
        let id = &f.preview.content.operation_id;
        if duplicate {
            let other = uuid::Uuid::new_v4().to_string();
            let mut op = value["batches"][id].clone();
            op["id"] = json!(other);
            value["batches"][&other] = op;
        } else {
            value["batches"][id]["members"]["first"]["window"] =
                json!(uuid::Uuid::new_v4().to_string());
        }
        store.write_raw("snapshot.json", &value).unwrap();
        let before = crate::session::afk::overview::tests::tree(&f.app);
        let overview = crate::session::afk::overview::snapshot_at(&f.app, now_ms()).unwrap();
        assert!(
            !overview.complete(),
            "inconsistent membership reported complete: duplicate={duplicate}"
        );
        let profile = overview
            .profiles
            .iter()
            .find(|p| p.profile == "batch-control")
            .unwrap();
        assert!(!profile.errors.is_empty());
        assert!(profile.batches.is_empty());
        assert!(profile.sessions.iter().all(|s| s.batch.is_none()));
        let json = serde_json::to_value(profile).unwrap();
        assert!(json["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["batch_membership"]
                .as_str()
                .is_some_and(|v| v.contains("unavailable"))));
        assert!(!serde_json::to_string(&overview)
            .unwrap()
            .contains("scratch"));
        assert_eq!(before, crate::session::afk::overview::tests::tree(&f.app));
    }
}

#[tokio::test]
#[serial_test::serial]
async fn reviewed_cli_uses_configured_default_for_batch_status_and_off() {
    use clap::Parser;
    let _home = crate::session::test_support::isolate_app_dir();
    let f = fixture(None);
    apply_with(&f.app, f.preview.clone(), |_, b, _, _| {
        Ok(f.preview
            .content
            .members
            .iter()
            .find(|m| m.session_id == b.instance_id)
            .unwrap()
            .observations[0]
            .generation
            .clone())
    })
    .unwrap();
    crate::session::set_default_profile("batch-control").unwrap();
    for operation in ["status", "off"] {
        let cli = crate::cli::Cli::try_parse_from([
            "aoe",
            "session",
            "afk",
            "batch",
            operation,
            &f.preview.content.operation_id,
        ])
        .unwrap();
        let Some(crate::cli::Commands::Session {
            command: crate::cli::session::SessionCommands::Afk(args),
        }) = cli.command
        else {
            panic!("wrong command")
        };
        crate::cli::afk::run("", args).await.unwrap();
    }
    assert_eq!(
        status(&f.app, "batch-control", &f.preview.content.operation_id).unwrap()["stopped"],
        true
    );
}

#[test]
#[serial_test::serial]
fn reset_changes_only_preference_and_old_previews_cannot_enroll_new_eligibility() {
    let _home = crate::session::test_support::isolate_app_dir();
    let f = fixture(None);
    let store = Store::authority(&f.app, "batch-control", "first", false).unwrap();
    {
        let _lock = store.lock_existing(TIMEOUT).unwrap();
        let mut session = store.session().unwrap();
        session.preference(Preference::Excluded).unwrap();
        store.publish_session(session).unwrap();
    }
    let before = store.session().unwrap();
    batch::allow_bulk(&f.rows[0]).unwrap();
    let after = store.session().unwrap();
    assert_eq!(after.epoch, before.epoch);
    assert_eq!(
        serde_json::to_value(after.book).unwrap(),
        serde_json::to_value(before.book).unwrap()
    );
    assert_eq!(after.preference, Preference::Allowed);
    assert!(after.exception_epoch > before.exception_epoch);
    let result = apply_with(&f.app, f.preview.clone(), |_, binding, _, _| {
        assert_eq!(binding.instance_id, "second");
        Ok(f.preview.content.members[1].observations[0]
            .generation
            .clone())
    })
    .unwrap();
    assert_eq!(result["members"][0]["outcome"], "skipped");
    assert_eq!(result["members"][1]["outcome"], "confirmed");
    assert_eq!(result["charged"], Value::Null);
    assert!(store.session().unwrap().book.windows.is_empty());
}
