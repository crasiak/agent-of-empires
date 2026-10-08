use super::*;

fn setup(requests: u8) -> (tempfile::TempDir, tempfile::TempDir, Store, Window) {
    let app = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = Store::runtime(app.path(), "test", true).unwrap();
    let (dev, ino) = AnchoredDir::open(root.path()).unwrap().identity().unwrap();
    let w = Window { version:2,binding:Binding {instance_id:"test".into(),profile:"default".into(),native_id:"native".into(),launch_id:uuid::Uuid::new_v4().to_string()},generation:uuid::Uuid::new_v4().to_string(),id:uuid::Uuid::new_v4().to_string(),revision:1,root:root.path().into(),control_root:app.path().into(),root_identity:(dev as u64,ino),grant:serde_json::from_value(json!({"version":2,"task":"scratch","scope":"create disposable scratch text","files":[{"path":"scratch.txt","capability":"create"}],"requests":requests,"assurance":COVERAGE})).unwrap(),issued_at_ms:1000,expires_at_ms:61000,state:"pending".into(),confirmed:true,reservations:vec![],reads:0,records:vec![],permit:None };
    Book {
        version: 2,
        windows: vec![w.clone()],
    }
    .save(&store)
    .unwrap();
    (app, root, store, w)
}
fn decision() -> Value {
    json!({"id":uuid::Uuid::new_v4().to_string(),"question":"Which scratch content?","alternatives":[{"name":"hello","pros":"simple","cons":"minimal","scores":[5]},{"name":"other","pros":"longer","cons":"unneeded","scores":[2]}],"criteria":[{"name":"clarity","weight":4}],"evidence":["operator scratch grant"],"assumptions":[],"uncertainties":[],"recommendation":"hello","rationale":"A small reversible scratch change","rollback":"remove scratch manually","reversible":true,"risk":"low","human_required":false,"depends_on":[],"action":{"path":"scratch.txt","expected_hash":null,"content":"hello\n"}})
}
fn reserve(store: &Store, w: &Window) -> String {
    let request = uuid::Uuid::new_v4().to_string();
    execute(
        store,
        &w.binding,
        &w.generation,
        Request::Reserve {
            window: w.id.clone(),
            request: request.clone(),
        },
        2000,
    )
    .unwrap();
    request
}
fn record(store: &Store, w: &Window, request: &str, d: Value) -> Result<Value> {
    execute(
        store,
        &w.binding,
        &w.generation,
        Request::Record {
            window: w.id.clone(),
            request: request.into(),
            decision: d,
        },
        2000,
    )
}
fn apply(store: &Store, w: &Window, request: &str, d: &Value, now: i64) -> Result<Value> {
    execute(
        store,
        &w.binding,
        &w.generation,
        Request::Apply {
            window: w.id.clone(),
            request: request.into(),
            decision: d["id"].as_str().unwrap().into(),
        },
        now,
    )
}

#[test]
fn record_later_apply_outcome_and_last_slot_are_durable() {
    let (_app, root, store, w) = setup(2);
    let first = reserve(&store, &w);
    let d = decision();
    assert_eq!(
        record(&store, &w, &first, d.clone()).unwrap()["disposition"],
        "admitted"
    );
    assert_eq!(
        record(&store, &w, &first, d.clone()).unwrap()["record_request"],
        1
    );
    let mut conflicting = d.clone();
    conflicting["rationale"] = json!("changed");
    assert!(record(&store, &w, &first, conflicting).is_err());
    assert!(apply(&store, &w, &first, &d, 2000).is_err());
    assert!(!root.path().join("scratch.txt").exists());
    let last = reserve(&store, &w);
    assert_eq!(
        apply(&store, &w, &last, &d, 2000).unwrap()["outcome"],
        "observed_applied"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("scratch.txt")).unwrap(),
        "hello\n"
    );
    assert!(apply(&store, &w, &last, &d, 2000).is_err());
    let view = Book::load(&store).unwrap().windows[0]
        .view(2000)
        .to_string();
    assert!(view.contains("observed_applied"));
    assert!(!view.contains("hello\\n"));
    assert!(!view.contains("preimage"));
}
#[test]
fn veto_stop_expiry_generation_and_changed_target_never_apply() {
    for case in [
        "human",
        "dependent",
        "unknown",
        "stop",
        "expiry",
        "generation",
        "changed",
    ] {
        let (_app, root, store, w) = setup(2);
        let first = reserve(&store, &w);
        let mut d = decision();
        match case {
            "human" => d["human_required"] = json!(true),
            "dependent" => d["depends_on"] = json!(["blocked"]),
            "unknown" => d["risk"] = json!("unknown"),
            _ => {}
        }
        record(&store, &w, &first, d.clone()).unwrap();
        let last = reserve(&store, &w);
        if case == "stop" {
            stop(&store).unwrap();
        }
        if case == "changed" {
            std::fs::write(root.path().join("scratch.txt"), "outside writer").unwrap();
        }
        let mut request_w = w.clone();
        if case == "generation" {
            request_w.generation = uuid::Uuid::new_v4().to_string();
        }
        assert!(
            apply(
                &store,
                &request_w,
                &last,
                &d,
                if case == "expiry" { 61000 } else { 2000 }
            )
            .is_err(),
            "{case}"
        );
        assert_ne!(
            std::fs::read_to_string(root.path().join("scratch.txt"))
                .ok()
                .as_deref(),
            Some("hello\n")
        );
    }
}
#[test]
fn finite_requests_reads_privacy_and_unsafe_targets() {
    for limit in [1, 8] {
        let (_app, _root, store, w) = setup(limit);
        let first = reserve(&store, &w);
        let d = decision();
        record(&store, &w, &first, d.clone()).unwrap();
        for _ in 1..limit {
            reserve(&store, &w);
        }
        assert!(execute(
            &store,
            &w.binding,
            &w.generation,
            Request::Reserve {
                window: w.id.clone(),
                request: uuid::Uuid::new_v4().to_string()
            },
            2000
        )
        .is_err());
        if limit == 1 {
            assert!(apply(&store, &w, &first, &d, 2000).is_err());
        }
    }
    let (_app, root, store, w) = setup(8);
    let req = reserve(&store, &w);
    for _ in 0..16 {
        execute(
            &store,
            &w.binding,
            &w.generation,
            Request::Read {
                window: w.id.clone(),
                request: req.clone(),
                path: "scratch.txt".into(),
            },
            2000,
        )
        .unwrap();
    }
    assert!(execute(
        &store,
        &w.binding,
        &w.generation,
        Request::Read {
            window: w.id.clone(),
            request: req.clone(),
            path: "scratch.txt".into()
        },
        2000
    )
    .is_err());
    for path in [
        "../scratch.txt",
        ".git/config",
        "profiles/data.json",
        "secrets.txt",
        "foo.sh",
        "/etc/file.txt",
    ] {
        let mut d = decision();
        d["action"]["path"] = json!(path);
        assert!(record(&store, &w, &req, d).is_err());
    }
    for value in [
        "-----BEGIN PRIVATE KEY-----",
        "key\u{1b}[31m",
        "ghp_example",
    ] {
        let mut d = decision();
        d["rationale"] = json!(value);
        assert!(record(&store, &w, &req, d).is_err());
    }
    std::os::unix::fs::symlink("/etc/passwd", root.path().join("scratch.txt")).unwrap();
    assert!(record(&store, &w, &req, decision()).is_err());
}
#[test]
fn failed_store_ambiguous_commit_and_retention_do_not_refund() {
    let (_app, root, store, w) = setup(2);
    let first = reserve(&store, &w);
    let d = decision();
    record(&store, &w, &first, d.clone()).unwrap();
    let last = reserve(&store, &w);
    FAIL_AFTER_ATTEMPT.with(|flag| flag.set(true));
    assert!(apply(&store, &w, &last, &d, 2000).is_err());
    assert!(!root.path().join("scratch.txt").exists());
    assert!(apply(&store, &w, &last, &d, 2000).is_err());
    let mut book = Book::load(&store).unwrap();
    book.prune(61001 + RETENTION_MS);
    assert!(book.windows[0].permit.as_ref().unwrap().content.is_some());
    assert_eq!(book.windows[0].reservations.len(), 2);
    let mut broken = serde_json::to_value(&book).unwrap();
    broken["windows"][0]["reads"] = json!(17);
    store.write("ledger.json", &broken).unwrap();
    assert!(Book::load(&store).is_err());
}

#[test]
fn real_pi_sdk_records_then_applies_through_host_bridge() {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    if std::env::var_os("AOE_PI_ROOT").is_none() {
        eprintln!("Set AOE_PI_ROOT to pinned Pi 0.87.1 to run the offline SDK/host integration");
        return;
    }
    let (_app, root, store, mut w) = setup(2);
    let control_root = tempfile::tempdir().unwrap();
    let control = Store::open(control_root.path(), "test", true).unwrap();
    let mut child = Command::new("node")
        .args(["assets/session/aoe-afk-sdk.test.mjs", "--host"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut initialized = false;
    let mut observed = false;
    for line in output.lines() {
        let value: Value = serde_json::from_str(&line.unwrap()).unwrap();
        if value.get("result").is_some() {
            assert_eq!(value["result"]["calls"], 3);
            observed = true;
            break;
        }
        if !initialized {
            w.binding = serde_json::from_value(value["binding"].clone()).unwrap();
            w.generation = value["generation"].as_str().unwrap().into();
            w.issued_at_ms = now_ms();
            w.expires_at_ms = w.issued_at_ms + 60000;
            Book {
                version: 2,
                windows: vec![w.clone()],
            }
            .save(&store)
            .unwrap();
            initialized = true;
        }
        let packet = serde_json::to_string(&value["packet"]).unwrap() + "\n";
        let mut reply = Vec::new();
        super::super::bridge::serve(
            &control,
            Some(&store),
            &w.binding,
            &w.generation,
            Cursor::new(packet),
            &mut reply,
            || Ok(()),
        )
        .unwrap();
        input.write_all(&reply).unwrap();
        input.flush().unwrap();
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    assert!(observed);
    assert_eq!(
        std::fs::read_to_string(root.path().join("scratch.txt")).unwrap(),
        "hello\n"
    );
    let book = Book::load(&store).unwrap();
    let w = &book.windows[0];
    assert_eq!(w.reservations.len(), 2);
    assert_eq!(w.records[0].disposition, "admitted");
    assert_eq!(
        w.permit.as_ref().unwrap().outcomes,
        ["unknown", "observed_applied"]
    );
}

#[test]
fn replacement_preimage_store_failure_caps_and_expired_body_pruning() {
    let (app, root, store, mut w) = setup(8);
    std::fs::write(root.path().join("scratch.txt"), "before").unwrap();
    w.grant.files[0].capability = Capability::Replace;
    Book {
        version: 2,
        windows: vec![w.clone()],
    }
    .save(&store)
    .unwrap();
    let first = reserve(&store, &w);
    let mut d = decision();
    d["action"]["expected_hash"] = json!(hash("before"));
    // An unsafe store must deny record and therefore deny all dependent effects.
    let ledger = app.path().join("afk-runtime-v2/test/ledger.json");
    std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(record(&store, &w, &first, d.clone()).is_err());
    std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Book::load(&store).unwrap().windows[0].permit.is_none());
    let mut large = d.clone();
    large["rationale"] = json!("é".repeat(RECORD_BYTES / 2));
    assert!(record(&store, &w, &first, large).is_err());
    record(&store, &w, &first, d.clone()).unwrap();
    let second = reserve(&store, &w);
    apply(&store, &w, &second, &d, 2000).unwrap();
    stop(&store).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("scratch.txt")).unwrap(),
        "hello\n"
    );
    let mut book = Book::load(&store).unwrap();
    assert_eq!(
        book.windows[0].permit.as_ref().unwrap().preimage.as_deref(),
        Some("before")
    );
    book.prune(61001 + RETENTION_MS);
    book.save(&store).unwrap();
    assert!(book.windows[0].records[0].claims.is_none());
    assert!(book.windows[0].permit.as_ref().unwrap().preimage.is_none());
    assert_eq!(book.windows[0].reservations.len(), 2);
    assert!(apply(&store, &w, &second, &d, 2000).is_err());
    let (_app, _root, store, w) = setup(2);
    let request = reserve(&store, &w);
    for _ in 0..4 {
        let mut deferred = decision();
        deferred["action"] = Value::Null;
        record(&store, &w, &request, deferred).unwrap();
    }
    assert!(record(&store, &w, &request, decision()).is_err());
    let mut book = Book::load(&store).unwrap();
    let mut window = book.windows[0].clone();
    window.grant.scope = "x".repeat(RECORD_BYTES / 2);
    book.windows = (0..2000)
        .map(|index| {
            let mut next = window.clone();
            next.id = uuid::Uuid::new_v4().to_string();
            next.revision = index + 1;
            next
        })
        .collect();
    assert!(book.save(&store).is_err());
    assert_eq!(Book::load(&store).unwrap().windows.len(), 1);
}

#[test]
fn executable_large_and_hardlinked_targets_and_unconfirmed_grants_are_rejected() {
    let (_app, root, store, mut w) = setup(2);
    w.confirmed = false;
    Book {
        version: 2,
        windows: vec![w.clone()],
    }
    .save(&store)
    .unwrap();
    assert!(execute(
        &store,
        &w.binding,
        &w.generation,
        Request::Reserve {
            window: w.id.clone(),
            request: uuid::Uuid::new_v4().to_string()
        },
        2000
    )
    .is_err());
    w.confirmed = true;
    Book {
        version: 2,
        windows: vec![w.clone()],
    }
    .save(&store)
    .unwrap();
    let request = reserve(&store, &w);
    let file = root.path().join("scratch.txt");
    std::fs::write(&file, "plain").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(record(&store, &w, &request, decision()).is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&file, vec![b'x'; FILE_BYTES + 1]).unwrap();
    assert!(w.file("scratch.txt").is_err());
    std::fs::write(&file, "plain").unwrap();
    std::fs::hard_link(&file, root.path().join("other.txt")).unwrap();
    assert!(w.file("scratch.txt").is_err());
    let result = execute_checked(
        &store,
        &w.binding,
        &w.generation,
        Request::Read {
            window: w.id.clone(),
            request,
            path: "scratch.txt".into(),
        },
        2000,
        || anyhow::bail!("binding changed after lock"),
    );
    assert!(result.is_err());
    assert_eq!(Book::load(&store).unwrap().windows[0].reads, 0);
}

#[test]
#[serial_test::serial]
fn operator_grant_requires_two_fresh_acknowledgements_before_executable_confirmation() {
    use crate::session::{ActiveExecution, ExecutionBinding, Status};
    for refuse in [false, true] {
        let _home = crate::session::test_support::isolate_app_dir();
        let workspace = tempfile::Builder::new()
            .prefix("afk-workspace-")
            .tempdir()
            .unwrap();
        let mut instance = Instance::new("pi", workspace.path().to_str().unwrap());
        instance.tool = "pi".into();
        instance.status = Status::Running;
        instance.source_profile = "afk-runtime-test".into();
        instance.agent_session_id = Some(uuid::Uuid::new_v4().to_string());
        instance.active_execution = Some(ActiveExecution {
            launch_id: uuid::Uuid::new_v4().to_string(),
            capture: None,
            container: None,
            binding: ExecutionBinding {
                agent: "pi".into(),
                stores: vec![],
                configuration: vec![],
                cwd: instance.project_path.clone().into(),
                cwd_filesystem: "host".into(),
                filesystem: "host".into(),
                exported_default_store: None,
            },
        });
        let storage = Storage::new_unwatched("afk-runtime-test").unwrap();
        storage
            .update(|rows, _| {
                rows.push(instance.clone());
                Ok(())
            })
            .unwrap();
        let app = crate::session::get_app_dir().unwrap();
        let control = Store::open(&app, &instance.id, true).unwrap();
        let runtime = Store::runtime(&app, &instance.id, true).unwrap();
        let grant = json!({"version":2,"task":"scratch","scope":"create scratch greeting","files":[{"path":"scratch.txt","capability":"create"}],"requests":2,"assurance":COVERAGE});
        let file = workspace.path().join("grant.json");
        std::fs::write(&file, serde_json::to_vec(&grant).unwrap()).unwrap();
        let binding = Binding::for_instance(&instance).unwrap();
        let generation = uuid::Uuid::new_v4().to_string();
        let peer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut previous = String::new();
            let mut acks = 0;
            while acks < 2 {
                if let Some(probe) = control.read::<RuntimeProbe>("runtime-probe.json").unwrap() {
                    if probe.challenge != previous {
                        if let Some(id) = &probe.window {
                            let book = Book::load(&runtime).unwrap();
                            assert!(!book.windows.last().unwrap().confirmed);
                            assert!(execute(
                                &runtime,
                                &binding,
                                &generation,
                                Request::Reserve {
                                    window: id.clone(),
                                    request: uuid::Uuid::new_v4().to_string()
                                },
                                now_ms()
                            )
                            .is_err());
                            if refuse {
                                stop(&runtime).unwrap();
                            }
                        }
                        previous = probe.challenge.clone();
                        acknowledge(
                            &control,
                            &binding,
                            &generation,
                            RuntimeAck {
                                version: 2,
                                binding: binding.clone(),
                                challenge: probe.challenge,
                                window: probe.window,
                                grant_hash: probe.grant_hash,
                                generation: generation.clone(),
                                sdk: "0.87.1".into(),
                                coverage: COVERAGE.into(),
                            },
                        )
                        .unwrap();
                        acks += 1;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "grant handshake did not progress"
                );
                std::thread::yield_now();
            }
        });
        let result = delegate(&instance, 1, &file);
        peer.join().unwrap();
        let runtime = Store::runtime(&app, &instance.id, false).unwrap();
        if refuse {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("ended or changed before confirmation"));
            let view = snapshot(&runtime).unwrap();
            assert_eq!(view["state"], "ended");
            assert_eq!(view["confirmed"], false);
            assert_eq!(view["reservations_used"], 0);
        } else {
            let result = result.unwrap();
            assert_eq!(result["state"], "pending");
            assert_eq!(result["confirmed"], true);
            assert_eq!(result["reservations_used"], 0);
        }
        assert!(!workspace.path().join("scratch.txt").exists());
        stop(&runtime).unwrap();
        assert_eq!(snapshot(&runtime).unwrap()["state"], "ended");
    }
}
