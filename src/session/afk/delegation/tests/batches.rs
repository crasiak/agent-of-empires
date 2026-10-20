use super::*;
use crate::session::afk::batch::state::{Member, Operation, Outcome, SharedExhausted};
use std::collections::BTreeMap;

struct Fixture {
    _app: tempfile::TempDir,
    _root: tempfile::TempDir,
    store: Store,
    other: Store,
    first: Window,
    second: Window,
    operation: String,
}
fn fixture(shared: Option<u16>, question: bool, nudge: bool) -> Fixture {
    let (app, root, store, mut first) = setup(4);
    first.grant.question_deferrals = u8::from(question);
    first.grant.settlement_nudges = u8::from(nudge);
    let other = Store::authority(app.path(), "default", "other", true).unwrap();
    let mut second = first.clone();
    second.id = uuid::Uuid::new_v4().to_string();
    second.binding.instance_id = "other".into();
    second.binding.native_id = "other-native".into();
    let operation = uuid::Uuid::new_v4().to_string();
    {
        let _lock = store.lock_existing(TIMEOUT).unwrap();
        let mut snapshot = store.authority_snapshot().unwrap();
        snapshot.sessions.get_mut("test").unwrap().book = Book {
            version: 4,
            windows: vec![first.clone()],
        };
        snapshot.sessions.get_mut("other").unwrap().book = Book {
            version: 4,
            windows: vec![second.clone()],
        };
        let members = [&first, &second]
            .into_iter()
            .map(|w| {
                (
                    w.binding.instance_id.clone(),
                    Member {
                        binding: Some(w.binding.clone()),
                        generation: Some(w.generation.clone()),
                        window: Some(w.id.clone()),
                        grant_hash: hash(serde_json::to_vec(&w.grant).unwrap()),
                        control_epoch: 0,
                        exception_epoch: 0,
                        outcome: Outcome::Confirmed,
                        reason: None,
                    },
                )
            })
            .collect();
        snapshot.batches.insert(
            operation.clone(),
            Operation {
                id: operation.clone(),
                preview_digest: "a".repeat(64),
                issued_at_ms: 1000,
                expires_at_ms: 61000,
                shared_requests: shared,
                stopped: false,
                members,
                tickets: vec![],
            },
        );
        snapshot.validate("default").unwrap();
        store.write_raw("snapshot.json", &snapshot).unwrap();
    }
    Fixture {
        _app: app,
        _root: root,
        store,
        other,
        first,
        second,
        operation,
    }
}
fn call(store: &Store, window: &Window, request: Request) -> Result<Value> {
    execute(store, &window.binding, &window.generation, request, 2000)
}
fn ordinary(store: &Store, window: &Window) -> Result<String> {
    let request = uuid::Uuid::new_v4().to_string();
    call(
        store,
        window,
        Request::Reserve {
            window: window.id.clone(),
            request: request.clone(),
            question: None,
            continuation: None,
        },
    )?;
    Ok(request)
}
fn question() -> QuestionIdentity {
    serde_json::from_value(json!({"operation":uuid::Uuid::new_v4().to_string(),"interaction_id":uuid::Uuid::new_v4().to_string(),"tool_call_id":"call","context_ref":"context","kind":"input","arguments_hash":"a".repeat(64)})).unwrap()
}
#[test]
fn optional_budget_is_atomic_and_denial_preserves_last_reserved_tools() {
    for shared in [None, Some(1)] {
        let f = fixture(shared, false, false);
        let first = ordinary(&f.store, &f.first).unwrap();
        if shared.is_some() {
            assert!(ordinary(&f.other, &f.second)
                .unwrap_err()
                .is::<SharedExhausted>());
            assert!(ordinary(&f.store, &f.first)
                .unwrap_err()
                .is::<SharedExhausted>());
        } else {
            ordinary(&f.other, &f.second).unwrap();
        }
        call(
            &f.store,
            &f.first,
            Request::Read {
                window: f.first.id.clone(),
                request: first,
                path: "scratch.txt".into(),
            },
        )
        .unwrap();
        let snapshot = f.store.authority_snapshot().unwrap();
        assert_eq!(
            snapshot.batches[&f.operation].tickets.len(),
            usize::from(shared.is_some())
        );
        assert_eq!(
            snapshot.sessions["other"].book.windows[0]
                .reservations
                .len(),
            usize::from(shared.is_none())
        );
        assert_eq!(snapshot.sessions["test"].book.windows[0].state, "owned");
    }
}
#[test]
fn question_hold_binds_after_denial_and_outcomes_never_refund() {
    for outcome in [
        QuestionOutcome::Accepted,
        QuestionOutcome::Ordinary,
        QuestionOutcome::Unknown,
    ] {
        let f = fixture(Some(1), true, false);
        let identity = question();
        call(
            &f.store,
            &f.first,
            Request::QuestionAdmit {
                window: f.first.id.clone(),
                identity: identity.clone(),
            },
        )
        .unwrap();
        call(
            &f.store,
            &f.first,
            Request::QuestionIntent {
                window: f.first.id.clone(),
                identity: identity.clone(),
            },
        )
        .unwrap();
        assert!(ordinary(&f.other, &f.second)
            .unwrap_err()
            .is::<SharedExhausted>());
        call(
            &f.store,
            &f.first,
            Request::QuestionOutcome {
                window: f.first.id.clone(),
                identity: identity.clone(),
                outcome,
            },
        )
        .unwrap();
        if outcome == QuestionOutcome::Accepted {
            let request = uuid::Uuid::new_v4().to_string();
            call(
                &f.store,
                &f.first,
                Request::Reserve {
                    window: f.first.id.clone(),
                    request: request.clone(),
                    question: Some(identity.clone()),
                    continuation: None,
                },
            )
            .unwrap();
            assert!(call(
                &f.store,
                &f.first,
                Request::Reserve {
                    window: f.first.id.clone(),
                    request,
                    question: Some(identity),
                    continuation: None
                }
            )
            .is_err());
        }
        call(
            &f.store,
            &f.first,
            Request::Stop {
                window: f.first.id.clone(),
                reason: TerminalReason::Interrupted,
            },
        )
        .unwrap();
        let snapshot = f.store.authority_snapshot().unwrap();
        assert_eq!(snapshot.batches[&f.operation].tickets.len(), 1);
        assert_eq!(
            snapshot.batches[&f.operation].tickets[0].request.is_some(),
            outcome == QuestionOutcome::Accepted
        );
    }
}
#[test]
fn nudge_hold_binds_once_when_the_shared_budget_is_already_full() {
    let f = fixture(Some(2), false, true);
    let first = ordinary(&f.store, &f.first).unwrap();
    let read = call(
        &f.store,
        &f.first,
        Request::Read {
            window: f.first.id.clone(),
            request: first.clone(),
            path: "scratch.txt".into(),
        },
    )
    .unwrap();
    let claims = serde_json::from_value(json!({"status":"unfinished","next_step":{"kind":"record","path":"scratch.txt"},"rationale":"granted next step","evidence":[read["evidence"]],"depends_on":[]})).unwrap();
    call(
        &f.store,
        &f.first,
        Request::Checkpoint {
            window: f.first.id.clone(),
            request: first.clone(),
            claims,
        },
    )
    .unwrap();
    let receipt = call(
        &f.store,
        &f.first,
        Request::Nudge {
            window: f.first.id.clone(),
            request: first.clone(),
            checkpoint: 1,
        },
    )
    .unwrap();
    let identity: Continuation = serde_json::from_value(receipt["identity"].clone()).unwrap();
    let repeat = call(
        &f.store,
        &f.first,
        Request::Nudge {
            window: f.first.id.clone(),
            request: first,
            checkpoint: 1,
        },
    )
    .unwrap();
    assert_eq!(receipt, repeat);
    assert!(ordinary(&f.other, &f.second)
        .unwrap_err()
        .is::<SharedExhausted>());
    call(
        &f.store,
        &f.first,
        Request::ContributionIntent {
            window: f.first.id.clone(),
            identity: identity.clone(),
        },
    )
    .unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    call(
        &f.store,
        &f.first,
        Request::Reserve {
            window: f.first.id.clone(),
            request: request.clone(),
            question: None,
            continuation: Some(identity),
        },
    )
    .unwrap();
    assert!(ordinary(&f.store, &f.first)
        .unwrap_err()
        .is::<SharedExhausted>());
    call(
        &f.store,
        &f.first,
        Request::Read {
            window: f.first.id.clone(),
            request,
            path: "scratch.txt".into(),
        },
    )
    .unwrap();
    let snapshot = f.store.authority_snapshot().unwrap();
    assert_eq!(snapshot.batches[&f.operation].tickets.len(), 2);
    assert!(snapshot.batches[&f.operation]
        .tickets
        .iter()
        .all(|t| t.request.is_some()));
}
#[test]
#[ignore = "subprocess entry used by shared_admissions_contend_across_processes"]
fn process_admission_worker() {
    use std::io::{BufRead, Write};
    let payload: Value =
        serde_json::from_str(&std::env::var("AOE_AFK_SHARED_CHILD").unwrap()).unwrap();
    let store = Store::authority(
        Path::new(payload["app"].as_str().unwrap()),
        "default",
        payload["session"].as_str().unwrap(),
        false,
    )
    .unwrap();
    let window = Book::load(&store).unwrap().windows[0].clone();
    println!("AFK_CHILD:{}", json!({"ready":true}));
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "GO");
    let result = call(
        &store,
        &window,
        serde_json::from_value(payload["command"].clone()).unwrap(),
    );
    println!(
        "AFK_CHILD:{}",
        json!({"ok":result.is_ok(),"shared_denied":result.as_ref().err().is_some_and(|e|e.is::<SharedExhausted>())})
    );
    std::io::stdout().flush().unwrap();
}
#[test]
fn shared_admissions_contend_across_processes() {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    struct Owned(std::process::Child);
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for kind in ["reserve", "question", "nudge", "unbudgeted"] {
        let shared = match kind {
            "unbudgeted" => None,
            "nudge" => Some(3),
            _ => Some(1),
        };
        let f = fixture(shared, kind == "question", kind == "nudge");
        let mut commands = Vec::new();
        for (store, window) in [(&f.store, &f.first), (&f.other, &f.second)] {
            commands.push(match kind {
                "question"=>json!({"kind":"question_admit","window":window.id,"identity":question()}),
                "nudge"=>{
                    let request=ordinary(store,window).unwrap();
                    let observed=call(store,window,Request::Read{window:window.id.clone(),request:request.clone(),path:"scratch.txt".into()}).unwrap();
                    let claims=serde_json::from_value(json!({"status":"unfinished","next_step":{"kind":"record","path":"scratch.txt"},"rationale":"next granted step","evidence":[observed["evidence"]],"depends_on":[]})).unwrap();
                    call(store,window,Request::Checkpoint{window:window.id.clone(),request:request.clone(),claims}).unwrap();
                    json!({"kind":"nudge","window":window.id,"request":request,"checkpoint":1})
                }
                _=>json!({"kind":"reserve","window":window.id,"request":uuid::Uuid::new_v4().to_string(),"question":null,"continuation":null}),
            });
        }
        let (sent, received) = std::sync::mpsc::channel();
        let mut children = Vec::new();
        let mut readers = Vec::new();
        for (index, (window, command)) in
            [&f.first, &f.second].into_iter().zip(commands).enumerate()
        {
            let mut child=Command::new(std::env::current_exe().unwrap()).args(["--ignored","--exact","session::afk::delegation::tests::batches::process_admission_worker","--nocapture","--test-threads=1"]).env("AOE_AFK_SHARED_CHILD",json!({"app":f._app.path(),"session":window.binding.instance_id,"command":command}).to_string()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
            let stdout = child.stdout.take().unwrap();
            let sent = sent.clone();
            readers.push(std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout).lines() {
                    let line = line.unwrap();
                    if let Some((_, value)) = line.split_once("AFK_CHILD:") {
                        if sent
                            .send((index, serde_json::from_str::<Value>(value).unwrap()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }));
            children.push(Owned(child));
        }
        let mut ready = std::collections::BTreeSet::new();
        while ready.len() < 2 {
            let (index, value) = received.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(value["ready"], true);
            ready.insert(index);
        }
        for child in &mut children {
            child.0.stdin.as_mut().unwrap().write_all(b"GO\n").unwrap();
            child.0.stdin.as_mut().unwrap().flush().unwrap();
        }
        let results: Vec<_> = (0..2)
            .map(|_| received.recv_timeout(Duration::from_secs(10)).unwrap().1)
            .collect();
        assert_eq!(
            results.iter().filter(|r| r["ok"] == true).count(),
            if shared.is_some() { 1 } else { 2 },
            "{kind}: {results:?}"
        );
        assert_eq!(
            results
                .iter()
                .filter(|r| r["shared_denied"] == true)
                .count(),
            usize::from(shared.is_some())
        );
        for child in &mut children {
            assert!(child.0.wait().unwrap().success());
        }
        for reader in readers {
            reader.join().unwrap();
        }
        let snapshot = f.store.authority_snapshot().unwrap();
        assert_eq!(
            snapshot.batches[&f.operation].tickets.len(),
            shared.map_or(0, usize::from),
            "{kind}"
        );
    }
}

#[test]
fn cross_record_corruption_and_unpaired_generic_save_are_refused() {
    let f = fixture(Some(2), false, false);
    ordinary(&f.store, &f.first).unwrap();
    let _lock = f.store.lock_existing(TIMEOUT).unwrap();
    let original = f.store.bytes("snapshot.json").unwrap();
    let mut book = Book::load(&f.store).unwrap();
    book.windows[0]
        .reservations
        .push(uuid::Uuid::new_v4().to_string());
    assert!(book.save(&f.store).is_err());
    assert_eq!(f.store.bytes("snapshot.json").unwrap(), original);
    let snapshot = f.store.authority_snapshot().unwrap();
    for case in [
        "refund",
        "foreign-window",
        "duplicate",
        "ordinary-as-held",
        "no-budget-with-tickets",
    ] {
        let mut changed = snapshot.clone();
        let op = changed.batches.get_mut(&f.operation).unwrap();
        match case {
            "refund" => op.tickets.clear(),
            "foreign-window" => op.tickets[0].window = uuid::Uuid::new_v4().to_string(),
            "duplicate" => op.tickets.push(op.tickets[0].clone()),
            "ordinary-as-held" => op.tickets[0].request = None,
            _ => op.shared_requests = None,
        }
        assert!(changed.validate("default").is_err(), "{case}");
    }
    let mut reassigned = snapshot.clone();
    let mut duplicate = reassigned.batches[&f.operation].clone();
    duplicate.id = uuid::Uuid::new_v4().to_string();
    reassigned.batches.insert(duplicate.id.clone(), duplicate);
    assert!(reassigned.validate("default").is_err());
    let mut changed = snapshot.clone();
    changed.batches = BTreeMap::new();
    assert!(snapshot.transition(&changed).is_err());
}
