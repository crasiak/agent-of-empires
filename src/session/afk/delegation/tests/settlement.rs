use super::*;

fn run(store: &Store, w: &Window, mut packet: Value) -> Result<Value> {
    packet["window"] = json!(w.id);
    execute(
        store,
        &w.binding,
        &w.generation,
        serde_json::from_value(packet)?,
        2000,
    )
}
fn read(store: &Store, w: &Window, request: &str) -> Value {
    run(
        store,
        w,
        json!({"kind":"read","request":request,"path":"scratch.txt"}),
    )
    .unwrap()
}
fn checkpoint(store: &Store, w: &Window, request: &str, evidence: Value, rationale: &str) -> Value {
    run(store, w, json!({"kind":"checkpoint","request":request,"claims":{"status":"unfinished","next_step":{"kind":"record","path":"scratch.txt"},"evidence":[evidence],"rationale":rationale,"depends_on":[]}})).unwrap()
}
fn allow(store: &Store, nudges: u8) {
    let mut book = Book::load(store).unwrap();
    book.windows[0].grant.settlement_nudges = nudges;
    book.save(store).unwrap();
}
fn admit(store: &Store, w: &Window, request: &str, c: &Value) -> Result<Value> {
    run(
        store,
        w,
        json!({"kind":"nudge","request":request,"checkpoint":c["sequence"]}),
    )
}
fn observe(store: &Store, w: &Window, receipt: &Value) -> String {
    let intent = json!({"kind":"contribution_intent","identity":receipt["identity"]});
    assert_eq!(
        run(store, w, intent.clone()).unwrap(),
        run(store, w, intent).unwrap()
    );
    let request = uuid::Uuid::new_v4().to_string();
    run(
        store,
        w,
        json!({"kind":"reserve","request":request,"continuation":receipt["identity"]}),
    )
    .unwrap();
    request
}

#[test]
fn meaningful_evidence_not_prose_ids_timestamps_or_identical_reads_controls_nudges() {
    for change in ["identical", "prose", "read", "changed"] {
        let (_app, root, store, w) = setup(8);
        allow(&store, 2);
        let request = reserve(&store, &w);
        let fact = read(&store, &w, &request);
        let c = checkpoint(&store, &w, &request, fact["evidence"].clone(), "next step");
        assert_eq!(
            c,
            checkpoint(&store, &w, &request, fact["evidence"].clone(), "next step")
        );
        let n = admit(&store, &w, &request, &c).unwrap();
        assert_eq!(n, admit(&store, &w, &request, &c).unwrap());
        assert_eq!(Book::load(&store).unwrap().windows[0].nudges.len(), 1);
        let request = observe(&store, &w, &n);
        if change == "changed" {
            std::fs::write(root.path().join("scratch.txt"), "observed change").unwrap();
        }
        let fact = read(&store, &w, &request);
        let c = checkpoint(
            &store,
            &w,
            &request,
            fact["evidence"].clone(),
            if change == "prose" {
                "different prose"
            } else {
                "next step"
            },
        );
        // Changed file state is meaningful, but a create-only next step is now out of scope.
        let c = if change == "changed" {
            run(&store, &w, json!({"kind":"checkpoint","request":request,"claims":{"status":"unfinished","next_step":{"kind":"read","path":"scratch.txt"},"evidence":[fact["evidence"]],"rationale":"inspect granted file","depends_on":[]}})).unwrap()
        } else {
            c
        };
        let next = admit(&store, &w, &request, &c);
        if change == "identical" || change == "read" {
            assert_eq!(
                next.unwrap(),
                n,
                "identical checkpoint receipt is idempotent, not a second admission"
            );
        } else if change == "prose" {
            assert!(next.is_err());
        } else {
            assert!(next.is_ok());
        }
        let book = Book::load(&store).unwrap();
        assert_eq!(
            book.windows[0].nudges.len(),
            if change == "changed" { 2 } else { 1 }
        );
        assert_eq!(book.windows[0].reservations.len(), 2);
        assert!(book.windows[0].permit.is_none());
    }
}

#[test]
fn settlement_vetoes_and_terminal_reasons_are_durable() {
    for case in [
        "zero",
        "request-one",
        "blocked",
        "completed",
        "dependency",
        "human",
        "missing",
        "unknown-evidence",
        "out-of-scope",
        "reads-exhausted",
        "stale-file",
        "off",
        "expired",
        "generation",
        "exhausted",
    ] {
        let (_app, root, store, w) = setup(if case == "request-one" { 1 } else { 8 });
        if case != "zero" {
            allow(&store, if case == "exhausted" { 1 } else { 2 });
        }
        let request = reserve(&store, &w);
        let fact = read(&store, &w, &request);
        if case == "human" {
            let mut d = decision();
            d["human_required"] = json!(true);
            record(&store, &w, &request, d).unwrap();
        }
        let mut claims = json!({"status":"unfinished","next_step":{"kind":"record","path":"scratch.txt"},"evidence":[fact["evidence"]],"rationale":"work remains","depends_on":[]});
        match case {
            "completed" | "blocked" => claims["status"] = json!(case),
            "dependency" => claims["depends_on"] = json!(["human permission"]),
            "unknown-evidence" => claims["evidence"] = json!(["invented"]),
            "out-of-scope" => claims["next_step"]["path"] = json!("other.txt"),
            "reads-exhausted" => {
                for _ in 1..16 {
                    read(&store, &w, &request);
                }
                claims["next_step"] = json!({"kind":"read","path":"scratch.txt"});
            }
            _ => {}
        }
        let mut c = if case == "missing" {
            json!({"sequence":1})
        } else {
            let packet = json!({"kind":"checkpoint","request":request,"claims":claims});
            let result = run(&store, &w, packet.clone());
            if matches!(case, "completed" | "blocked" | "dependency") {
                assert_eq!(result.as_ref().unwrap(), &run(&store, &w, packet).unwrap());
            }
            if case == "unknown-evidence" {
                assert!(result.is_err());
                continue;
            }
            result.unwrap()
        };
        let mut request = request;
        if case == "exhausted" {
            let n = admit(&store, &w, &request, &c).unwrap();
            request = observe(&store, &w, &n);
            let d = decision();
            let receipt = record(&store, &w, &request, d.clone()).unwrap();
            c = run(&store, &w, json!({"kind":"checkpoint","request":request,"claims":{"status":"unfinished","next_step":{"kind":"apply","decision":d["id"]},"evidence":receipt["evidence"],"rationale":"apply admitted change","depends_on":[]}})).unwrap();
        }
        match case {
            "stale-file" => std::fs::write(root.path().join("scratch.txt"), "changed").unwrap(),
            "off" => stop(&store).unwrap(),
            "expired" | "generation" => {
                super::super::settlement::reconcile(
                    &store,
                    &w.binding,
                    if case == "generation" {
                        "replacement"
                    } else {
                        &w.generation
                    },
                    if case == "expired" { 61000 } else { 2000 },
                )
                .unwrap();
            }
            _ => {}
        }
        assert!(admit(&store, &w, &request, &c).is_err(), "{case}");
        let book = Book::load(&store).unwrap();
        let latest = &book.windows[0];
        assert_eq!(latest.state, "ended", "{case}");
        assert_eq!(
            latest.nudges.len(),
            usize::from(case == "exhausted"),
            "{case}"
        );
        let reason = match case {
            "completed" => TerminalReason::Completed,
            "zero" | "request-one" | "exhausted" | "reads-exhausted" => TerminalReason::Exhausted,
            "off" | "expired" | "generation" => TerminalReason::Interrupted,
            _ => TerminalReason::Deferred,
        };
        assert_eq!(latest.terminal_reason, Some(reason), "{case}");
        assert!(latest
            .permit
            .as_ref()
            .is_none_or(|p| p.attempted_at_ms.is_none()));
    }
}

#[test]
fn unresolved_admission_restart_lost_ack_and_conflicts_never_refund_or_replay() {
    for intent in [false, true] {
        let (_app, _root, store, w) = setup(8);
        allow(&store, 2);
        let request = reserve(&store, &w);
        let fact = read(&store, &w, &request);
        let c = checkpoint(&store, &w, &request, fact["evidence"].clone(), "next");
        let n = admit(&store, &w, &request, &c).unwrap();
        if intent {
            run(
                &store,
                &w,
                json!({"kind":"contribution_intent","identity":n["identity"]}),
            )
            .unwrap();
        }
        let mut conflict = n["identity"].clone();
        conflict["admission"] = json!(uuid::Uuid::new_v4().to_string());
        assert!(run(
            &store,
            &w,
            json!({"kind":"contribution_intent","identity":conflict})
        )
        .is_err());
        assert!(run(
            &store,
            &w,
            json!({"kind":"reserve","request":uuid::Uuid::new_v4().to_string(),"continuation":null})
        )
        .is_err());
        let view = super::super::settlement::reconcile(
            &store,
            &w.binding,
            &uuid::Uuid::new_v4().to_string(),
            2000,
        )
        .unwrap();
        assert_eq!(view["terminal_reason"], "delivery_unknown");
        assert_eq!(view["nudges_used"], 1);
        assert_eq!(view["reservations_used"], 1);
        assert!(admit(&store, &w, &request, &c).is_err());
        run(&store, &w, json!({"kind":"stop","reason":"completed"})).unwrap();
        assert_eq!(
            snapshot(&store).unwrap()["terminal_reason"],
            "delivery_unknown"
        );
        let mut book = Book::load(&store).unwrap();
        book.prune(RETENTION_MS + 61001);
        book.save(&store).unwrap();
        assert_eq!(book.windows[0].nudges.len(), 1);
    }
    let (_app, _root, store, w) = setup(8);
    assert_eq!(w.grant.settlement_nudges, 0);
    let mut bad = w.grant.clone();
    bad.settlement_nudges = 3;
    assert!(bad.validate().is_err());
    let mut old = serde_json::to_value(Book::load(&store).unwrap()).unwrap();
    old["version"] = json!(2);
    store.write("ledger.json", &old).unwrap();
    assert!(Book::load(&store).is_err());
}

#[test]
fn checkpoint_bodies_expire_without_erasing_identity_or_ambiguous_evidence() {
    for stage in ["none", "admitted", "intent", "observed"] {
        let (_app, _root, store, w) = setup(8);
        allow(&store, 2);
        let request = reserve(&store, &w);
        let fact = read(&store, &w, &request);
        let c = checkpoint(
            &store,
            &w,
            &request,
            fact["evidence"].clone(),
            "private rationale",
        );
        if stage != "none" {
            let n = admit(&store, &w, &request, &c).unwrap();
            if stage == "intent" {
                run(
                    &store,
                    &w,
                    json!({"kind":"contribution_intent","identity":n["identity"]}),
                )
                .unwrap();
            } else if stage == "observed" {
                observe(&store, &w, &n);
            }
        }
        run(&store, &w, json!({"kind":"stop","reason":"interrupted"})).unwrap();
        let before = snapshot(&store).unwrap();
        let mut book = Book::load(&store).unwrap();
        book.prune(RETENTION_MS + 61001);
        book.save(&store).unwrap();
        let after = snapshot(&store).unwrap();
        let ambiguous = matches!(stage, "admitted" | "intent");
        assert_eq!(
            after["checkpoints"][0]["claims"].is_null(),
            !ambiguous,
            "{stage}"
        );
        assert_eq!(
            serde_json::to_string(&book)
                .unwrap()
                .contains("private rationale"),
            ambiguous,
            "{stage}"
        );
        for key in ["sequence", "request", "recorded_at_ms", "facts"] {
            assert_eq!(
                after["checkpoints"][0][key], before["checkpoints"][0][key],
                "{stage}: {key}"
            );
        }
        for key in [
            "nudges",
            "nudges_used",
            "reservations_used",
            "terminal_reason",
        ] {
            assert_eq!(after[key], before[key], "{stage}: {key}");
        }
        assert!(admit(&store, &w, &request, &c).is_err());
    }
}
