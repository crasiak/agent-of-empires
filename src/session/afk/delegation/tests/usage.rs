use super::*;
use crate::session::afk::usage::{
    self as meter, Boundary, Cursor, Entry, Gap, Kind, Model, Observation, Policy, Report,
};

struct Fixture {
    app: tempfile::TempDir,
    root: tempfile::TempDir,
    store: Store,
    transport: Store,
    window: Window,
    server: meter::transport::Server,
    reply: Value,
    rows: Vec<Entry>,
}
impl Fixture {
    fn new(policy: Option<Policy>) -> Self {
        Self::with_grant(policy, false, false)
    }
    fn with_grant(policy: Option<Policy>, question: bool, nudge: bool) -> Self {
        let (app, root, store, mut window) = setup(3);
        window.grant.question_deferrals = u8::from(question);
        window.grant.settlement_nudges = u8::from(nudge);
        window.issued_at_ms = now_ms();
        window.expires_at_ms = window.issued_at_ms + 60_000;
        {
            let _lock = store.lock(TIMEOUT).unwrap();
            Book {
                version: 4,
                windows: vec![window.clone()],
            }
            .save(&store)
            .unwrap();
        }
        let transport = Store::transport(app.path(), "default", "test", true).unwrap();
        let mut server = meter::transport::Server::new();
        let reply = server
            .observe(
                &transport,
                &store,
                &window.binding,
                &window.generation,
                None,
                || Ok(()),
            )
            .unwrap();
        let mut f = Self {
            app,
            root,
            store,
            transport,
            window,
            server,
            reply,
            rows: vec![],
        };
        f.send(
            vec![Entry {
                id: "ordinary".into(),
                parent_id: None,
                kind: Kind::User,
                model: None,
                counters: None,
                gap: None,
                sdk_cost_nano: None,
            }],
            Boundary::Poll,
        );
        assert_eq!(f.reply["fresh"], true);
        let mut tx = authority::SessionTransaction::open(&f.store).unwrap();
        let mut next = tx.snapshot().clone();
        next.usage
            .enroll(
                f.app.path(),
                &f.window.id,
                &f.window.binding,
                &f.window.generation,
                policy.clone(),
                policy.is_some(),
            )
            .unwrap();
        tx.publish(next).unwrap().warn();
        drop(tx);
        f
    }
    fn send(&mut self, new: Vec<Entry>, boundary: Boundary) {
        self.rows.extend(new);
        let after: Cursor =
            serde_json::from_value(self.reply["challenge"]["after"].clone()).unwrap();
        let head = self
            .rows
            .iter()
            .try_fold(Cursor::default(), |c, r| c.append(r))
            .unwrap();
        let report = Report {
            version: 1,
            challenge: self.reply["challenge"]["id"].as_str().unwrap().into(),
            identity: meter::Identity {
                native_id: self.window.binding.native_id.clone(),
                file: "a".repeat(64),
                parent: None,
            },
            after: after.clone(),
            entries: self.rows[after.count..].to_vec(),
            head,
        };
        self.reply = self
            .server
            .observe(
                &self.transport,
                &self.store,
                &self.window.binding,
                &self.window.generation,
                Some(Observation {
                    window: Some(self.window.id.clone()),
                    boundary,
                    report: Some(report),
                    gap: None,
                }),
                || Ok(()),
            )
            .unwrap();
    }
    fn reserve(&self) -> String {
        let request = uuid::Uuid::new_v4().to_string();
        self.call(Request::Reserve {
            window: self.window.id.clone(),
            request: request.clone(),
            question: None,
            continuation: None,
        })
        .unwrap();
        request
    }
    fn call(&self, request: Request) -> Result<Value> {
        execute(
            &self.store,
            &self.window.binding,
            &self.window.generation,
            request,
            now_ms(),
        )
    }
}
fn row(id: &str) -> Entry {
    Entry {
        id: id.into(),
        parent_id: Some("ordinary".into()),
        kind: Kind::Assistant,
        model: Some(Model {
            provider: "fixture".into(),
            model: "model".into(),
        }),
        counters: Some(meter::Counters {
            input: 5,
            output: 5,
            ..Default::default()
        }),
        gap: None,
        sdk_cost_nano: None,
    }
}
fn guard(usd: bool) -> Policy {
    serde_json::from_value(json!({"version":1,"basis":"observed-native-entries","tokens":if usd{Value::Null}else{json!(20)},"usd":if usd{json!({"currency":"USD","basis":"operator-fixed-rates","limit_nano":2,"quotes":[{"model":{"provider":"fixture","model":"model"},"input":1,"output":1,"cache_read":1,"cache_write":1,"cache_write_1h":1}]})}else{Value::Null}})).unwrap()
}

#[test]
fn completed_usage_view_explicitly_proves_absent_shared_denial() {
    let mut policy = guard(false);
    policy.tokens = Some(200);
    let mut f = Fixture::new(Some(policy));
    let first = f.reserve();
    f.send(vec![row("first")], Boundary::BeforeTool);
    let d = decision();
    f.call(Request::Record {
        window: f.window.id.clone(),
        request: first,
        decision: d.clone(),
    })
    .unwrap();
    let last = f.reserve();
    f.send(vec![row("last")], Boundary::BeforeTool);
    f.call(Request::Apply {
        window: f.window.id.clone(),
        request: last,
        decision: d["id"].as_str().unwrap().into(),
    })
    .unwrap();
    for boundary in [Boundary::Poll, Boundary::TurnEnd, Boundary::BeforeRequest] {
        f.send(vec![], boundary);
        assert!(
            snapshot(&f.store).unwrap()["usage"]["cutoff"].is_null(),
            "effect completion is not final SDK settlement"
        );
    }
    f.send(vec![], Boundary::Settled);
    let view = snapshot(&f.store).unwrap();
    assert_eq!(view["usage"]["source_cursor"], view["usage"]["cutoff"]);
    assert_eq!(view["usage"]["source_complete"], true);
    assert_eq!(view["state"], "ended");
    assert_eq!(view["terminal_reason"], "completed");
    assert!(view["usage"]["cutoff"].is_object());
    assert_eq!(view["usage"]["measurement"]["tokens_complete"], true);
    assert_eq!(
        view["usage"].get("shared_blocked"),
        Some(&Value::Null),
        "absence must be explicitly qualified, not inferred from an omitted field"
    );
}

#[test]
fn observed_threshold_vetoes_later_effect_without_refunding_reserved_requests() {
    for usd in [false, true] {
        let mut f = Fixture::new(Some(guard(usd)));
        let first = f.reserve();
        f.send(vec![row("first")], Boundary::BeforeTool);
        let d = decision();
        f.call(Request::Record {
            window: f.window.id.clone(),
            request: first,
            decision: d.clone(),
        })
        .unwrap();
        let last = f.reserve();
        f.send(vec![row("last")], Boundary::BeforeTool);
        let error = f
            .call(Request::Apply {
                window: f.window.id.clone(),
                request: last,
                decision: d["id"].as_str().unwrap().into(),
            })
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<meter::Denied>().unwrap().0,
            meter::Denial::Exhausted
        );
        assert!(!f.root.path().join("scratch.txt").exists());
        let snapshot = f.store.authority_snapshot().unwrap();
        assert_eq!(
            snapshot.sessions["test"].book.windows[0].reservations.len(),
            2
        );
        assert_eq!(
            snapshot.usage.windows[&f.window.id].blocked,
            Some(meter::Denial::Exhausted)
        );
        meter::recover(&f.store, &f.window.binding, &f.window.id, || Ok(())).unwrap();
        let recovered = f.store.authority_snapshot().unwrap();
        assert!(recovered.usage.windows[&f.window.id].ordinary_recovered);
        assert_eq!(recovered.sessions["test"].book.windows[0].state, "ended");
        assert_eq!(
            recovered.sessions["test"].book.windows[0]
                .reservations
                .len(),
            2
        );
    }
}
#[test]
fn usage_denial_before_controlled_intent_keeps_the_original_held_ticket() {
    for question in [true, false] {
        let mut f = Fixture::with_grant(Some(guard(false)), question, !question);
        let operation = uuid::Uuid::new_v4().to_string();
        {
            let mut tx = authority::SessionTransaction::open(&f.store).unwrap();
            let mut next = tx.snapshot().clone();
            let session = &next.sessions["test"];
            next.batches.insert(
                operation.clone(),
                batch::state::Operation {
                    id: operation.clone(),
                    preview_digest: "a".repeat(64),
                    issued_at_ms: f.window.issued_at_ms,
                    expires_at_ms: f.window.expires_at_ms,
                    shared_requests: Some(if question { 1 } else { 2 }),
                    stopped: false,
                    members: std::collections::BTreeMap::from([(
                        "test".into(),
                        batch::state::Member {
                            binding: Some(f.window.binding.clone()),
                            generation: Some(f.window.generation.clone()),
                            window: Some(f.window.id.clone()),
                            grant_hash: hash(serde_json::to_vec(&f.window.grant).unwrap()),
                            control_epoch: session.epoch,
                            exception_epoch: session.exception_epoch,
                            outcome: batch::state::Outcome::Confirmed,
                            reason: None,
                        },
                    )]),
                    tickets: vec![],
                },
            );
            tx.publish(next).unwrap().warn();
        }
        let intent = if question {
            let identity:crate::session::afk::delegation::questions::QuestionIdentity=serde_json::from_value(json!({"operation":uuid::Uuid::new_v4().to_string(),"interaction_id":uuid::Uuid::new_v4().to_string(),"tool_call_id":"call","context_ref":"entry","kind":"input","arguments_hash":"a".repeat(64)})).unwrap();
            f.call(Request::QuestionAdmit {
                window: f.window.id.clone(),
                identity: identity.clone(),
            })
            .unwrap();
            Request::QuestionIntent {
                window: f.window.id.clone(),
                identity,
            }
        } else {
            let request = f.reserve();
            let read = f
                .call(Request::Read {
                    window: f.window.id.clone(),
                    request: request.clone(),
                    path: "scratch.txt".into(),
                })
                .unwrap();
            let claims=serde_json::from_value(json!({"status":"unfinished","next_step":{"kind":"record","path":"scratch.txt"},"rationale":"next granted step","evidence":[read["evidence"]],"depends_on":[]})).unwrap();
            f.call(Request::Checkpoint {
                window: f.window.id.clone(),
                request: request.clone(),
                claims,
            })
            .unwrap();
            let nudge = f
                .call(Request::Nudge {
                    window: f.window.id.clone(),
                    request,
                    checkpoint: 1,
                })
                .unwrap();
            Request::ContributionIntent {
                window: f.window.id.clone(),
                identity: serde_json::from_value(nudge["identity"].clone()).unwrap(),
            }
        };
        let tickets = f.store.authority_snapshot().unwrap().batches[&operation]
            .tickets
            .clone();
        let mut spent = row("spent");
        spent.counters.as_mut().unwrap().input = 15;
        f.send(vec![spent], Boundary::Poll);
        let error = f.call(intent).unwrap_err();
        assert_eq!(
            error.downcast_ref::<meter::Denied>().unwrap().0,
            meter::Denial::Exhausted
        );
        let after = f.store.authority_snapshot().unwrap();
        assert_eq!(after.batches[&operation].tickets, tickets);
        assert!(tickets.last().unwrap().request.is_none());
        if question {
            let book = serde_json::to_value(&after.sessions["test"].book).unwrap();
            assert!(book["windows"][0]["question"]["emission_intent_at_ms"].is_null());
        }
    }
}

#[test]
fn unchanged_heartbeats_leave_canonical_bytes_and_journal_untouched() {
    let mut f = Fixture::new(Some(guard(false)));
    f.send(vec![row("a")], Boundary::BeforeTool);
    let before = f.store.bytes("snapshot.json").unwrap();
    for _ in 0..3 {
        f.send(vec![], Boundary::Poll);
        assert_eq!(f.reply["fresh"], true);
        assert_eq!(f.store.bytes("snapshot.json").unwrap(), before);
        assert!(f.store.bytes("publication.pending.json").unwrap().is_none());
    }
    let mut restarted = meter::transport::Server::new();
    restarted
        .observe(
            &f.transport,
            &f.store,
            &f.window.binding,
            &f.window.generation,
            None,
            || Ok(()),
        )
        .unwrap();
    assert_eq!(
        meter::check(
            &f.store.authority_snapshot().unwrap(),
            f.app.path(),
            &f.window.id
        )
        .map(|d| d.reason),
        Some(meter::Denial::Stale)
    );
    assert_eq!(f.store.bytes("snapshot.json").unwrap(), before);
}
#[test]
fn missing_measurement_blocks_required_guard_but_not_unguarded_work() {
    for required in [false, true] {
        let mut f = Fixture::new(required.then(|| guard(false)));
        let request = f.reserve();
        let mut unknown = row("unknown");
        unknown.counters = None;
        unknown.gap = Some(Gap::MissingUsage);
        f.send(vec![unknown], Boundary::BeforeTool);
        let result = f.call(Request::Record {
            window: f.window.id.clone(),
            request,
            decision: decision(),
        });
        assert_eq!(result.is_ok(), !required);
        if required {
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<meter::Denied>()
                    .unwrap()
                    .0,
                meter::Denial::Unavailable
            );
        }
    }
}
#[test]
fn usage_publication_capacity_is_not_an_unguarded_spending_veto() {
    for required in [false, true] {
        let mut f = Fixture::new(required.then(|| guard(false)));
        let before = f.store.bytes("snapshot.json").unwrap();
        f.server.limit_publication_for_test(1);
        f.send(vec![row("over-capacity")], Boundary::BeforeTool);
        assert_eq!(f.reply["fresh"], false);
        assert!(f.reply["diagnostic"].as_str().unwrap().contains("capacity"));
        assert_eq!(f.store.bytes("snapshot.json").unwrap(), before);
        let view = snapshot(&f.store).unwrap();
        assert_eq!(view["usage"]["negative_evidence_pending"], true);
        assert_eq!(view["usage"]["measurement"]["tokens_complete"], false);
        assert!(f.store.bytes("publication.pending.json").unwrap().is_none());
        let request = Request::Reserve {
            window: f.window.id.clone(),
            request: uuid::Uuid::new_v4().to_string(),
            question: None,
            continuation: None,
        };
        assert_eq!(f.call(request).is_ok(), !required);
        f.server
            .limit_publication_for_test(authority::PROFILE_BYTES);
        f.send(vec![], Boundary::BeforeTool);
        assert_eq!(
            f.reply["fresh"], true,
            "complete matching source evidence can reconcile rejected publication"
        );
        assert!(f
            .transport
            .bytes(meter::transport::UNRESOLVED)
            .unwrap()
            .is_none());
        if required {
            assert!(
                f.store.authority_snapshot().unwrap().usage.windows[&f.window.id]
                    .blocked
                    .is_some(),
                "reconciliation must not clear the denied window"
            );
        }
    }
}

#[test]
fn usage_lease_is_rechecked_at_the_final_effect_commit_boundary() {
    let mut p = guard(false);
    p.tokens = Some(100);
    let mut f = Fixture::new(Some(p));
    let first = f.reserve();
    f.send(vec![row("first")], Boundary::BeforeTool);
    let d = decision();
    f.call(Request::Record {
        window: f.window.id.clone(),
        request: first,
        decision: d.clone(),
    })
    .unwrap();
    let last = f.reserve();
    f.send(vec![row("last")], Boundary::BeforeTool);
    let checks = std::cell::Cell::new(0);
    let result = execute_checked(
        &f.store,
        &f.window.binding,
        &f.window.generation,
        Request::Apply {
            window: f.window.id.clone(),
            request: last,
            decision: d["id"].as_str().unwrap().into(),
        },
        now_ms(),
        || {
            checks.set(checks.get() + 1);
            if checks.get() == 2 {
                let mut lease: Value = f.transport.read_raw("usage-lease.json")?.unwrap();
                lease["clock"]["boot"] = json!("expired-between-checks");
                f.transport.write_raw("usage-lease.json", &lease)?;
            }
            Ok(())
        },
    );
    assert!(checks.get() >= 2);
    assert!(
        result.is_err(),
        "effect committed against a lease invalidated during eligibility I/O"
    );
    assert_eq!(
        result
            .unwrap_err()
            .downcast_ref::<meter::Denied>()
            .unwrap()
            .0,
        meter::Denial::Stale
    );
    assert!(!f.root.path().join("scratch.txt").exists());
    let book = f.store.session().unwrap().book;
    assert!(book.windows[0]
        .permit
        .as_ref()
        .unwrap()
        .attempted_at_ms
        .is_none());
    assert_eq!(book.windows[0].reservations.len(), 2);
}

#[test]
fn unpublishable_announced_usage_cannot_be_forgotten_by_the_next_heartbeat() {
    let mut f = Fixture::new(None);
    f.server.limit_publication_for_test(1);
    f.send(vec![row("not-recorded")], Boundary::BeforeTool);
    assert_eq!(f.reply["fresh"], false);
    f.server = meter::transport::Server::new();
    f.reply = f
        .server
        .observe(
            &f.transport,
            &f.store,
            &f.window.binding,
            &f.window.generation,
            None,
            || Ok(()),
        )
        .unwrap();
    f.rows.pop();
    f.send(vec![], Boundary::Poll);
    assert_eq!(
        f.reply["fresh"], false,
        "rejected newer source evidence was forgotten"
    );
    assert!(
        f.call(Request::Reserve {
            window: f.window.id.clone(),
            request: uuid::Uuid::new_v4().to_string(),
            question: None,
            continuation: None
        })
        .is_ok(),
        "negative measurement veto must not spend-stop unguarded work"
    );
}

#[test]
fn terminal_book_publication_also_marks_unreconciled_accounting_closing() {
    let mut p = guard(false);
    p.tokens = Some(100);
    let mut f = Fixture::new(Some(p));
    f.send(vec![row("active")], Boundary::BeforeTool);
    f.call(Request::Stop {
        window: f.window.id.clone(),
        reason: TerminalReason::Interrupted,
    })
    .unwrap();
    let stopped = f.store.authority_snapshot().unwrap();
    assert!(
        stopped.usage.windows[&f.window.id].closing,
        "terminal book left accounting mislabeled open"
    );
    assert!(stopped.usage.windows[&f.window.id].cutoff.is_none());
    assert_eq!(
        meter::check(&stopped, f.app.path(), &f.window.id).map(|d| d.reason),
        Some(meter::Denial::Unavailable)
    );
}

#[test]
fn qualified_new_user_cutoff_retires_original_authority_in_the_same_publication() {
    let mut p = guard(false);
    p.tokens = Some(100);
    let mut f = Fixture::new(Some(p));
    let reserved = f.reserve();
    f.send(vec![row("prior-response")], Boundary::BeforeTool);
    let user = Entry {
        id: "next-user".into(),
        parent_id: Some("prior-response".into()),
        kind: Kind::User,
        model: None,
        counters: None,
        gap: None,
        sdk_cost_nano: None,
    };
    f.send(vec![user], Boundary::BeforeRequest);
    let snapshot = f.store.authority_snapshot().unwrap();
    let interval = &snapshot.usage.windows[&f.window.id];
    assert!(interval.cutoff.is_some());
    let original = &snapshot.sessions["test"].book.windows[0];
    assert_eq!(
        original.state, "ended",
        "closed accounting left original authority live"
    );
    assert_eq!(original.reservations, vec![reserved]);
    assert!(f
        .call(Request::Reserve {
            window: f.window.id.clone(),
            request: uuid::Uuid::new_v4().to_string(),
            question: None,
            continuation: None
        })
        .is_err());
}

#[test]
fn unproven_late_assistant_usage_keeps_closing_incomplete() {
    for aborted in [false, true] {
        let mut f = Fixture::new(Some(guard(false)));
        f.send(vec![row("old")], Boundary::BeforeTool);
        f.call(Request::Stop {
            window: f.window.id.clone(),
            reason: TerminalReason::Interrupted,
        })
        .unwrap();
        f.send(vec![], Boundary::Settled);
        let closed = f.store.authority_snapshot().unwrap().usage.windows[&f.window.id].clone();
        let mut late = row("late-assistant");
        if aborted {
            late.counters = Some(Default::default());
            late.gap = Some(Gap::ErrorOrAbort);
        }
        f.send(vec![late], Boundary::Poll);
        let after = f.store.authority_snapshot().unwrap();
        let w = &after.usage.windows[&f.window.id];
        assert_eq!(w.cutoff, closed.cutoff);
        assert_eq!(w.observed_through, closed.observed_through);
        assert_eq!(
            w.gap,
            Some(Gap::LateWork),
            "unproven late assistant vanished from coverage"
        );
    }
}

#[test]
fn partial_report_truncation_never_produces_a_fresh_lease() {
    let mut f = Fixture::new(None);
    let after: Cursor = serde_json::from_value(f.reply["challenge"]["after"].clone()).unwrap();
    let next = row("page");
    let tail = row("announced-tail");
    let partial = after.append(&next).unwrap();
    let head = partial.append(&tail).unwrap();
    let identity = meter::Identity {
        native_id: f.window.binding.native_id.clone(),
        file: "a".repeat(64),
        parent: None,
    };
    let report = Report {
        version: 1,
        challenge: f.reply["challenge"]["id"].as_str().unwrap().into(),
        identity: identity.clone(),
        after,
        entries: vec![next],
        head,
    };
    f.reply = f
        .server
        .observe(
            &f.transport,
            &f.store,
            &f.window.binding,
            &f.window.generation,
            Some(Observation {
                window: Some(f.window.id.clone()),
                boundary: Boundary::Poll,
                report: Some(report),
                gap: None,
            }),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(f.reply["fresh"], false);
    let report = Report {
        version: 1,
        challenge: f.reply["challenge"]["id"].as_str().unwrap().into(),
        identity,
        after: partial.clone(),
        entries: vec![],
        head: partial,
    };
    let reply = f
        .server
        .observe(
            &f.transport,
            &f.store,
            &f.window.binding,
            &f.window.generation,
            Some(Observation {
                window: Some(f.window.id.clone()),
                boundary: Boundary::Poll,
                report: Some(report),
                gap: None,
            }),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(
        reply["fresh"], false,
        "announced newer incomplete evidence became fresh after truncation"
    );
}

#[test]
fn closing_watermark_stays_fixed_before_later_ordinary_usage() {
    let mut f = Fixture::new(Some(guard(false)));
    f.send(vec![row("old")], Boundary::BeforeTool);
    f.call(Request::Stop {
        window: f.window.id.clone(),
        reason: TerminalReason::Interrupted,
    })
    .unwrap();
    f.send(vec![], Boundary::Settled);
    let closed = f.store.authority_snapshot().unwrap().usage.windows[&f.window.id].clone();
    assert!(closed.closing);
    assert!(closed.cutoff.is_some());
    let mut human = row("human");
    human.kind = Kind::User;
    human.model = None;
    human.counters = None;
    f.send(vec![human, row("later")], Boundary::BeforeRequest);
    let after = f.store.authority_snapshot().unwrap();
    assert_eq!(after.usage.windows[&f.window.id].cutoff, closed.cutoff);
    assert_eq!(
        after.usage.windows[&f.window.id].observed_through,
        closed.observed_through
    );
    assert_eq!(
        after.usage.window_view(&f.window.id)["measurement"]["reported_tokens"],
        10
    );
    let mut late = row("aux");
    late.kind = Kind::Usage;
    f.send(vec![late], Boundary::Poll);
    assert_eq!(
        f.store.authority_snapshot().unwrap().usage.windows[&f.window.id].gap,
        Some(Gap::LateWork)
    );
}
