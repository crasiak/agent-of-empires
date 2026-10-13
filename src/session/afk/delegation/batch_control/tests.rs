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
                        snapshot
                            .usage
                            .sync_lifetime(&mut snapshot.sessions)
                            .unwrap();
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
fn shared_usage_denial_stays_latched_but_local_denial_does_not_stop_siblings() {
    use crate::session::afk::usage as meter;
    struct Reporter {
        runtime: Store,
        transport: Store,
        server: meter::transport::Server,
        binding: Binding,
        generation: String,
        file: String,
        parent: Option<String>,
        reply: Value,
        entries: Vec<meter::Entry>,
    }
    impl Reporter {
        fn refresh(
            &mut self,
            window: Option<String>,
            boundary: meter::Boundary,
            new: Vec<meter::Entry>,
        ) {
            self.entries.extend(new);
            let after: meter::Cursor =
                serde_json::from_value(self.reply["challenge"]["after"].clone()).unwrap();
            let head = self
                .entries
                .iter()
                .try_fold(meter::Cursor::default(), |c, e| c.append(e))
                .unwrap();
            let report = meter::Report {
                version: 1,
                challenge: self.reply["challenge"]["id"].as_str().unwrap().into(),
                identity: meter::Identity {
                    native_id: self.binding.native_id.clone(),
                    file: self.file.clone(),
                    parent: self.parent.clone(),
                },
                after: after.clone(),
                entries: self.entries[after.count..].to_vec(),
                head,
            };
            self.reply = self
                .server
                .observe(
                    &self.transport,
                    &self.runtime,
                    &self.binding,
                    &self.generation,
                    Some(meter::Observation {
                        window,
                        boundary,
                        report: Some(report),
                        gap: None,
                    }),
                    || Ok(()),
                )
                .unwrap();
        }
    }
    let mut missing = Vec::new();
    for mode in [
        "shared-stale",
        "local",
        "closed-rejected",
        "closed-gap",
        "closed-gap-unpublished",
        "closed-unavailable",
        "closed-corrupt",
        "closed-foreign-report",
        "closed-invalid-report",
        "closed-envelope-window",
        "closed-envelope-foreign-window",
        "closed-envelope-gap",
        "closed-envelope-window-unguarded",
        "closed-envelope-foreign-window-unguarded",
        "closed-envelope-gap-unguarded",
        "closed-semantic-corrupt",
        "closed-semantic-corrupt-unguarded",
        "closed-other-native",
        "closed-other-native-parent",
        "closed-rejected-unguarded",
        "closed-gap-unguarded",
        "closed-gap-unpublished-unguarded",
        "closed-unavailable-unguarded",
    ] {
        let local_only = mode == "local";
        let _home = crate::session::test_support::isolate_app_dir();
        let mut f = fixture(None);
        let policy = |tokens| {
            serde_json::from_value::<meter::Policy>(
                json!({"version":1,"basis":"observed-native-entries","tokens":tokens,"usd":null}),
            )
            .unwrap()
        };
        f.preview.content.shared_usage_guard = (!mode.ends_with("unguarded")).then(|| policy(100));
        if local_only {
            f.preview.content.members[1].usage_guard = Some(policy(1));
        }
        let mut reporters = Vec::new();
        for (row, member) in f.rows.iter().zip(&f.preview.content.members) {
            let binding = Binding::for_instance(row).unwrap();
            let generation = member.observations[0].generation.clone();
            let runtime = Store::authority(&f.app, "batch-control", &row.id, false).unwrap();
            let transport = Store::transport(&f.app, "batch-control", &row.id, false).unwrap();
            let mut server = meter::transport::Server::new();
            let reply = server
                .observe(&transport, &runtime, &binding, &generation, None, || Ok(()))
                .unwrap();
            let mut reporter = Reporter {
                runtime,
                transport,
                server,
                file: hash(&binding.instance_id),
                parent: None,
                binding,
                generation,
                reply,
                entries: vec![],
            };
            reporter.refresh(None, meter::Boundary::Poll, vec![]);
            assert_eq!(reporter.reply["fresh"], true);
            reporters.push(reporter);
        }
        apply_with(&f.app, f.preview.clone(), |_, b, _, _| {
            Ok(reporters
                .iter()
                .find(|r| r.binding.instance_id == b.instance_id)
                .unwrap()
                .generation
                .clone())
        })
        .unwrap();
        let windows: Vec<_> = reporters
            .iter()
            .map(|r| r.runtime.session().unwrap().book.windows[0].id.clone())
            .collect();
        if mode.starts_with("closed-") {
            let a = &reporters[0];
            execute(
                &a.runtime,
                &a.binding,
                &a.generation,
                Request::Stop {
                    window: windows[0].clone(),
                    reason: TerminalReason::Interrupted,
                },
                now_ms(),
            )
            .unwrap();
            reporters[0].refresh(Some(windows[0].clone()), meter::Boundary::Settled, vec![]);
            let closed = reporters[0].runtime.authority_snapshot().unwrap();
            assert!(closed.usage.windows[&windows[0]].cutoff.is_some());
            assert!(closed.usage.windows[&windows[0]].gap.is_none());
            reporters[0]
                .transport
                .write_raw("usage-lease.json", &Value::Null)
                .unwrap();
            let reserve_b = || Request::Reserve {
                window: windows[1].clone(),
                request: uuid::Uuid::new_v4().to_string(),
                question: None,
                continuation: None,
            };
            let b = &reporters[1];
            execute(&b.runtime, &b.binding, &b.generation, reserve_b(), now_ms()).unwrap();
            if mode.starts_with("closed-semantic-corrupt") {
                let a = &reporters[0];
                a.transport.write_raw(meter::transport::UNRESOLVED,&json!({"version":1,"identity":{"native_id":a.binding.native_id,"file":a.file,"parent":null},"high_water":0,"heads":{},"parked":false})).unwrap();
                let before = a.transport.bytes(meter::transport::UNRESOLVED).unwrap();
                if !mode.ends_with("unguarded") {
                    let snapshot = a.runtime.authority_snapshot().unwrap();
                    assert_eq!(
                        meter::check(&snapshot, &f.app, &windows[1]).map(|d| d.reason),
                        Some(meter::Denial::Unavailable)
                    );
                    assert!(
                        snapshot.usage.batches[&f.preview.content.operation_id]
                            .blocked
                            .is_none(),
                        "read-only check did not latch denial"
                    );
                }
                reporters[0].refresh(Some(windows[0].clone()), meter::Boundary::Poll, vec![]);
                if reporters[0]
                    .transport
                    .bytes(meter::transport::UNRESOLVED)
                    .unwrap()
                    != before
                {
                    missing.push(format!(
                        "{mode}: malformed evidence repaired or removed by heartbeat"
                    ));
                }
                if reporters[0].reply["fresh"] != false {
                    missing.push(format!("{mode}: malformed evidence renewed freshness"));
                }
            } else if ["closed-foreign-report", "closed-invalid-report"].contains(&mode)
                || mode.starts_with("closed-envelope-")
            {
                let a = &mut reporters[0];
                let malformed = mode.starts_with("closed-envelope-");
                let foreign = mode.contains("foreign");
                if !malformed {
                    a.server.limit_publication_for_test(1);
                }
                let after: meter::Cursor =
                    serde_json::from_value(a.reply["challenge"]["after"].clone()).unwrap();
                let entry = meter::Entry {
                    id: "invalid-origin".into(),
                    parent_id: None,
                    kind: meter::Kind::Assistant,
                    model: None,
                    counters: Some(meter::Counters {
                        input: 2,
                        ..Default::default()
                    }),
                    gap: None,
                    sdk_cost_nano: None,
                };
                let mut head = after.append(&entry).unwrap();
                if mode == "closed-invalid-report" {
                    head.hash = "invalid".into();
                }
                let identity = meter::Identity {
                    native_id: if foreign {
                        uuid::Uuid::new_v4().to_string()
                    } else {
                        a.binding.native_id.clone()
                    },
                    file: if foreign {
                        "f".repeat(64)
                    } else {
                        a.file.clone()
                    },
                    parent: None,
                };
                let report = meter::Report {
                    version: 1,
                    challenge: a.reply["challenge"]["id"].as_str().unwrap().into(),
                    identity,
                    after,
                    entries: vec![entry],
                    head,
                };
                let reply = a.server.observe(
                    &a.transport,
                    &a.runtime,
                    &a.binding,
                    &a.generation,
                    Some(meter::Observation {
                        window: Some(if mode.contains("window") {
                            "not-a-uuid".into()
                        } else {
                            windows[0].clone()
                        }),
                        boundary: meter::Boundary::Poll,
                        report: Some(report),
                        gap: mode
                            .starts_with("closed-envelope-gap")
                            .then_some(meter::Gap::Unavailable),
                    }),
                    || Ok(()),
                );
                if malformed {
                    assert!(reply.is_err(), "malformed envelope accepted: {mode}");
                    let pending = a.transport.bytes(meter::transport::UNRESOLVED).unwrap();
                    if pending.is_none() {
                        missing.push(format!("{mode}: rejected report lost negative custody"));
                    }
                    a.refresh(Some(windows[0].clone()), meter::Boundary::Poll, vec![]);
                    if a.reply["fresh"] != false {
                        missing.push(format!("{mode}: old-prefix heartbeat became fresh"));
                    }
                    assert_eq!(
                        a.transport.bytes(meter::transport::UNRESOLVED).unwrap(),
                        pending,
                        "{mode}: heartbeat changed rejected evidence"
                    );
                } else {
                    a.reply = reply.unwrap();
                }
            } else if mode == "closed-corrupt" {
                reporters[0]
                    .transport
                    .write_raw(meter::transport::UNRESOLVED, &json!({"malformed":true}))
                    .unwrap();
            } else if mode.starts_with("closed-rejected") || mode.starts_with("closed-other-native")
            {
                if mode.starts_with("closed-other-native") {
                    let a = &mut reporters[0];
                    if mode.ends_with("parent") {
                        a.parent = Some(a.file.clone());
                    }
                    a.binding.native_id = uuid::Uuid::new_v4().to_string();
                    a.binding.launch_id = uuid::Uuid::new_v4().to_string();
                    a.generation = uuid::Uuid::new_v4().to_string();
                    a.file = hash(&a.binding.native_id);
                    a.entries.clear();
                    a.server = meter::transport::Server::new();
                    a.reply = a
                        .server
                        .observe(
                            &a.transport,
                            &a.runtime,
                            &a.binding,
                            &a.generation,
                            None,
                            || Ok(()),
                        )
                        .unwrap();
                }
                reporters[0].server.limit_publication_for_test(1);
                reporters[0].refresh(
                    (!mode.starts_with("closed-other-native")).then(|| windows[0].clone()),
                    meter::Boundary::Poll,
                    vec![meter::Entry {
                        id: "late".into(),
                        parent_id: None,
                        kind: meter::Kind::Assistant,
                        model: None,
                        counters: Some(meter::Counters {
                            input: 2,
                            ..Default::default()
                        }),
                        gap: None,
                        sdk_cost_nano: None,
                    }],
                );
                assert!(reporters[0]
                    .transport
                    .bytes(meter::transport::UNRESOLVED)
                    .unwrap()
                    .is_some());
                assert!(
                    reporters[0]
                        .runtime
                        .authority_snapshot()
                        .unwrap()
                        .usage
                        .windows[&windows[0]]
                        .gap
                        .is_none(),
                    "unpublished gap is only negative transport evidence"
                );
            } else {
                let a = &mut reporters[0];
                if mode.starts_with("closed-gap-unpublished") {
                    a.server.limit_publication_for_test(1);
                }
                a.reply = a
                    .server
                    .observe(
                        &a.transport,
                        &a.runtime,
                        &a.binding,
                        &a.generation,
                        Some(meter::Observation {
                            window: Some(windows[0].clone()),
                            boundary: meter::Boundary::Poll,
                            report: None,
                            gap: Some(if mode.starts_with("closed-unavailable") {
                                meter::Gap::Unavailable
                            } else {
                                meter::Gap::HistoryLimit
                            }),
                        }),
                        || Ok(()),
                    )
                    .unwrap();
            }
            if mode.starts_with("closed-gap") {
                let before = reporters[0]
                    .transport
                    .bytes(meter::transport::UNRESOLVED)
                    .unwrap();
                reporters[0]
                    .server
                    .limit_publication_for_test(authority::PROFILE_BYTES);
                reporters[0].refresh(Some(windows[0].clone()), meter::Boundary::Poll, vec![]);
                assert_eq!(
                    reporters[0].reply["fresh"], false,
                    "old-prefix heartbeat erased unknown source coverage"
                );
                if before.is_some() {
                    assert_eq!(
                        reporters[0]
                            .transport
                            .bytes(meter::transport::UNRESOLVED)
                            .unwrap(),
                        before
                    );
                }
            }
            reporters[1].refresh(Some(windows[1].clone()), meter::Boundary::Poll, vec![]);
            let b = &reporters[1];
            let before_status = b.runtime.bytes("snapshot.json").unwrap();
            let report = status(&f.app, "batch-control", &f.preview.content.operation_id).unwrap();
            assert_eq!(
                b.runtime.bytes("snapshot.json").unwrap(),
                before_status,
                "status changed authority"
            );
            if mode != "closed-other-native"
                && report["usage"]["coverage"] != "partial_or_unavailable"
            {
                missing.push(format!("{mode}: status falsely complete"));
            }
            if mode != "closed-other-native"
                && report["members"][0]["usage"]["measurement"]["tokens_complete"] != false
            {
                missing.push(format!("{mode}: member falsely complete"));
            }
            if mode == "closed-other-native" {
                assert_eq!(report["usage"]["coverage"], "stored_reported_interval");
                assert_eq!(
                    report["members"][0]["usage"]["measurement"]["tokens_complete"],
                    true
                );
            }
            let retry = apply_with(&f.app, f.preview.clone(), |_, _, _, _| {
                anyhow::bail!("report-only retry probed runtime")
            })
            .unwrap();
            assert_eq!(retry["usage"], report["usage"]);
            assert_eq!(retry["members"], report["members"]);
            assert_eq!(b.runtime.bytes("snapshot.json").unwrap(), before_status);
            let result = execute(&b.runtime, &b.binding, &b.generation, reserve_b(), now_ms());
            assert_eq!(
                b.runtime.authority_snapshot().unwrap().usage.windows[&windows[0]].cutoff,
                closed.usage.windows[&windows[0]].cutoff
            );
            if mode.ends_with("unguarded") || mode == "closed-other-native" {
                assert!(result.is_ok(), "NoGuard gained a spending veto: {result:?}");
            } else {
                if result
                    .as_ref()
                    .err()
                    .and_then(|e| e.downcast_ref::<meter::Denied>())
                    .map(|e| e.0)
                    != Some(meter::Denial::Unavailable)
                {
                    missing.push(format!(
                        "{mode}: closed negative evidence bypassed guard: {result:?}"
                    ));
                }
                let count = b.runtime.session().unwrap().book.windows[0]
                    .reservations
                    .len();
                if count != 1 {
                    missing.push(format!("{mode}: reserved {count} instead of retaining1"));
                }
            }
            let stopped = off(&f.app, "batch-control", &f.preview.content.operation_id).unwrap();
            let after_off = b.runtime.bytes("snapshot.json").unwrap();
            let repeated = off(&f.app, "batch-control", &f.preview.content.operation_id).unwrap();
            assert_eq!(repeated["usage"], stopped["usage"]);
            assert_eq!(repeated["members"], stopped["members"]);
            assert_eq!(b.runtime.bytes("snapshot.json").unwrap(), after_off);
            continue;
        }
        if local_only {
            reporters[1].refresh(
                Some(windows[1].clone()),
                meter::Boundary::BeforeTool,
                vec![meter::Entry {
                    id: "priced".into(),
                    parent_id: None,
                    kind: meter::Kind::Assistant,
                    model: None,
                    counters: Some(meter::Counters {
                        input: 2,
                        ..Default::default()
                    }),
                    gap: None,
                    sdk_cost_nano: None,
                }],
            );
        } else {
            let mut lease: Value = reporters[0]
                .transport
                .read_raw("usage-lease.json")
                .unwrap()
                .unwrap();
            lease["clock"]["boot"] = json!("previous-boot");
            reporters[0]
                .transport
                .write_raw("usage-lease.json", &lease)
                .unwrap();
        }
        let request = |id: &str| Request::Reserve {
            window: id.into(),
            request: uuid::Uuid::new_v4().to_string(),
            question: None,
            continuation: None,
        };
        let b = &reporters[1];
        let error = execute(
            &b.runtime,
            &b.binding,
            &b.generation,
            request(&windows[1]),
            now_ms(),
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<meter::Denied>().unwrap().0,
            if local_only {
                meter::Denial::Exhausted
            } else {
                meter::Denial::Stale
            }
        );
        reporters[1].refresh(Some(windows[1].clone()), meter::Boundary::Settled, vec![]);
        reporters[0].refresh(Some(windows[0].clone()), meter::Boundary::Poll, vec![]);
        let a = &reporters[0];
        let result = execute(
            &a.runtime,
            &a.binding,
            &a.generation,
            request(&windows[0]),
            now_ms(),
        );
        assert_eq!(
            result.is_ok(),
            local_only,
            "shared guard silently resumed after previously observed denial: {result:?}"
        );
        if !local_only {
            let before = a.runtime.bytes("snapshot.json").unwrap();
            for _ in 0..2 {
                poll(&a.runtime, &a.binding, &a.generation, now_ms(), || Ok(())).unwrap();
            }
            assert_eq!(
                a.runtime.bytes("snapshot.json").unwrap(),
                before,
                "latched shared denial republished unchanged authority"
            );
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
#[serial_test::serial]
fn ambiguous_native_or_file_identity_is_not_independent_shared_usage() {
    use crate::session::afk::usage as meter;
    for same_native in [true, false] {
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
        let store = Store::authority(&f.app, "batch-control", "first", false).unwrap();
        let mut snapshot = store.authority_snapshot().unwrap();
        let shared_native = snapshot.sessions["first"].book.windows[0]
            .binding
            .native_id
            .clone();
        let mut first_window = None;
        for (index, id) in ["first", "second"].into_iter().enumerate() {
            let session = snapshot.sessions.get_mut(id).unwrap();
            session.book.end_all();
            let w = &mut session.book.windows[0];
            if index == 0 {
                first_window = Some(w.id.clone());
            }
            if same_native {
                w.binding.native_id = shared_native.clone();
                snapshot
                    .batches
                    .get_mut(&f.preview.content.operation_id)
                    .unwrap()
                    .members
                    .get_mut(id)
                    .unwrap()
                    .binding = Some(w.binding.clone());
            }
            let identity = meter::Identity {
                native_id: w.binding.native_id.clone(),
                file: hash(if same_native { id } else { "same-file" }),
                parent: None,
            };
            let mut source =
                meter::Source::new(w.binding.clone(), w.generation.clone(), identity.clone());
            let row = meter::Entry {
                id: "observed".into(),
                parent_id: None,
                kind: meter::Kind::Assistant,
                model: None,
                counters: Some(meter::Counters {
                    input: 2,
                    ..Default::default()
                }),
                gap: None,
                sdk_cost_nano: None,
            };
            let head = meter::Cursor::default().append(&row).unwrap();
            source
                .reconcile(
                    &w.binding,
                    &w.generation,
                    &meter::Report {
                        version: 1,
                        challenge: uuid::Uuid::new_v4().to_string(),
                        identity: identity.clone(),
                        after: meter::Cursor::default(),
                        entries: vec![row],
                        head: head.clone(),
                    },
                )
                .unwrap();
            let key = identity.key(&w.binding).unwrap();
            snapshot.usage.sources.insert(key.clone(), source);
            let measured = snapshot.usage.windows.get_mut(&w.id).unwrap();
            measured.binding = w.binding.clone();
            measured.source = Some(key);
            measured.baseline = Some(meter::Cursor::default());
            measured.observed_through = Some(head.clone());
            measured.cutoff = Some(head);
            measured.closing = true;
        }
        snapshot.usage.batches.insert(
            f.preview.content.operation_id.clone(),
            meter::SharedGuard::new(
                serde_json::from_value(
                    json!({"version":1,"basis":"observed-native-entries","tokens":100,"usd":null}),
                )
                .unwrap(),
            ),
        );
        snapshot.validate("batch-control").unwrap();
        assert_eq!(
            meter::check(&snapshot, &f.app, first_window.as_deref().unwrap()).map(|d| d.reason),
            Some(meter::Denial::Unavailable),
            "aliased native/file identities appeared independent"
        );
        let summary = meter::batch_view(&snapshot, &f.preview.content.operation_id);
        assert_eq!(summary["coverage"], "partial_or_unavailable");
        assert!(summary["reported_tokens"].is_null());
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
