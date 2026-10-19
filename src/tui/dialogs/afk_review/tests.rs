use super::*;
use crate::session::afk::review::SelectedWindow;
use serde_json::{json, Value};

type Calls = Receiver<(Request, mpsc::Sender<Reply>)>;
pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}
fn selection(profile: &str, session: &str, number: u128) -> Selection {
    Selection {
        profile: profile.into(),
        session_id: session.into(),
        window_id: uuid::Uuid::from_u128(number).to_string(),
    }
}
pub(super) fn record(number: u128, disposition: &str, human: bool) -> Value {
    json!({"id":uuid::Uuid::from_u128(number).to_string(),"payload_hash":"b".repeat(64),"recorded_at_ms":2000,"body_status":"retained","disposition":disposition,
        "claims":{"question":"Choose a plan","recommendation":"small","human_required":human,
        "criteria":[{"name":"clarity","weight":4}],"alternatives":[{"name":"small","scores":[5],"pros":"simple","cons":"limited"},{"name":"large","scores":[2],"pros":"complete","cons":"costly"}],
        "assumptions":["Original operator scope"],"evidence":["stored claim"],"uncertainties":["unknown price"],"rationale":"Keep it reversible\nDo not infer approval\u{1b}[2J","risk":"low","rollback":"remove scratch","depends_on":[]}})
}
pub(super) fn report() -> Report {
    let selections = [
        selection("alpha\u{202e}\u{7f}", "first", 1),
        selection("beta", "second", 2),
    ];
    let first = json!({"state":"ended","expired":true,"terminal_reason":"completed","grant_hash":"a".repeat(64),"grant":{"task":"Scratch\u{2066}","scope":"One file","requests":4},
        "action":{"decision":uuid::Uuid::from_u128(11).to_string(),"outcomes":["unknown","observed_applied"]},"reservations_used":2,"nudges_used":0,"question_deferrals_used":0,
        "records":[record(11,"admitted",false),record(12,"deferred",false),record(13,"human_required",true)],
        "usage":{"measurement":{"reported_tokens":123,"tokens_complete":false,"operator_estimate_nano":null,"valuation_complete":false,"sdk_recorded_nano":42},"gap":"closing","blocked":null,"negative_evidence_pending":true}});
    let second = json!({"state":"ended","expired":true,"terminal_reason":"failed","grant_hash":"c".repeat(64),"grant":{"task":"Other task","scope":"One file","requests":2},
        "action":{"decision":uuid::Uuid::from_u128(21).to_string(),"outcomes":["unknown"]},"reservations_used":2,"records":[record(21,"admitted",false)],"usage":{"coverage":"not_collected"}});
    let mut rows: Vec<_> = selections
        .into_iter()
        .zip([first, second])
        .map(|(selection, window)| SelectedWindow {
            selection,
            window: Some(window),
            source_revision: Some(7),
            annotations: vec![],
            errors: vec![],
        })
        .collect();
    let target_selection = rows[0].selection.clone();
    rows[0].annotations.push(annotations::Annotation {
        id: uuid::Uuid::from_u128(99).to_string(),
        target: annotations::Target {
            selection: target_selection,
            decision: Some(uuid::Uuid::from_u128(11).to_string()),
            original_hash: "b".repeat(64),
        },
        kind: annotations::Kind::Reviewed,
        body: Some("Operator claim, not an answer\u{202e}".into()),
        prior: None,
        created_at_ms: 3000,
        request_hash: "f".repeat(64),
    });
    Report {
        version: 1,
        capture_started_at_ms: 4000,
        capture_finished_at_ms: 4100,
        complete: true,
        selections: rows,
        shared_usage: vec![],
        observation: "Stored only",
        privacy: "Private, heuristic filtering",
    }
}
pub(super) fn harness(report: Report) -> (AfkReviewDialog, Calls) {
    let selections = report
        .selections
        .iter()
        .map(|row| row.selection.clone())
        .collect::<Vec<_>>();
    let labels = ReviewLabels {
        names: selections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    (s.profile.clone(), s.session_id.clone()),
                    Some(format!("Workstream {}", i + 1)),
                )
            })
            .collect(),
        historical: BTreeSet::new(),
    };
    let (send, calls) = mpsc::channel();
    let mut dialog = AfkReviewDialog::with_dispatch(
        selections.clone(),
        labels,
        Box::new(move |request| {
            let (reply, receive) = mpsc::channel();
            assert!(send.send((request, reply)).is_ok());
            receive
        }),
    );
    let (Request::Read(received), reply) = calls.try_recv().unwrap() else {
        panic!("initial dispatch must only read")
    };
    assert_eq!(received, selections);
    assert!(reply.send(Reply::Read(Ok(report))).is_ok());
    assert!(dialog.tick());
    (dialog, calls)
}
pub(super) fn screen(dialog: &mut AfkReviewDialog) -> String {
    screen_size(dialog, 132, 44)
}
pub(super) fn screen_size(dialog: &mut AfkReviewDialog, width: u16, height: u16) -> String {
    let buffer = buffer_size(dialog, width, height);
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}
pub(super) fn buffer_size(
    dialog: &mut AfkReviewDialog,
    width: u16,
    height: u16,
) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area(), &Theme::default()))
        .unwrap();
    terminal.backend().buffer().clone()
}
fn type_text(dialog: &mut AfkReviewDialog, text: &str) {
    for c in text.chars() {
        dialog.handle_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn mixed_review_drilldown_keeps_claims_effects_and_human_review_separate() {
    let (mut dialog, calls) = harness(report());
    let text = screen(&mut dialog);
    assert!(text.contains("AFK return review"));
    assert!(text.contains("COMPLETE COLLECTION"));
    assert!(text.contains("Applied (observed after the attempt)"));
    assert!(text.contains("Workstream 1"));
    assert!(text.contains("host-required; model-required"));
    assert!(!text.contains(['\u{202e}', '\u{7f}', '\u{2066}', '\u{1b}']));
    for hidden in [
        uuid::Uuid::from_u128(11).to_string(),
        "b".repeat(64),
        "Criterion clarity".into(),
        "payload_hash".into(),
        "capture 4000".into(),
    ] {
        assert!(
            !text.contains(&hidden),
            "summary leaked diagnostic/detail content: {hidden}"
        );
    }
    dialog.handle_key(key(KeyCode::Char('d')));
    assert!(dialog.detail().contains("observed_applied"));
    assert!(dialog
        .detail()
        .contains("[\"unknown\",\"observed_applied\"]"));
    assert!(dialog
        .detail()
        .contains("SDK-recorded estimate nanoUSD: 42"));
    assert!(dialog.detail().contains("NOT an invoice"));
    assert!(screen(&mut dialog).contains("Captured source revision"));
    dialog.handle_key(key(KeyCode::Esc));
    dialog.handle_key(key(KeyCode::Enter));
    let detail = dialog.detail();
    for expected in [
        "Host disposition: admitted",
        "Model recommendation: small",
        "human-required claim: false",
        "Criterion clarity: weight 4",
        "Alternative small | scores clarity: 5",
        "Original operator scope",
        "not answers, approvals or host observations",
        "Operator claim, not an answer",
    ] {
        assert!(detail.contains(expected), "missing {expected}: {detail}");
    }
    assert!(
        detail.lines().count() > 15,
        "layout newlines must remain newlines"
    );
    assert!(!detail.contains(['\u{202e}', '\u{7f}', '\u{2066}', '\u{1b}']));
    assert!(
        detail.contains("\\nDo not infer approval"),
        "model newlines are not layout separators"
    );
    let weighted_screen = screen(&mut dialog);
    assert!(weighted_screen.contains("Criterion clarity: weight 4"));
    assert!(weighted_screen.contains("Alternative small"));
    dialog.handle_key(key(KeyCode::PageDown));
    assert!(dialog.scroll > 0);
    assert!(screen(&mut dialog).contains("Original operator scope"));
    dialog.handle_key(key(KeyCode::Down));
    assert!(dialog.detail().contains("Host disposition: deferred"));
    assert!(screen(&mut dialog).contains("No effect observation recorded for this decision"));
    dialog.handle_key(key(KeyCode::Down));
    assert!(dialog.detail().contains("human-required claim: true"));
    dialog.handle_key(key(KeyCode::Esc));
    dialog.handle_key(key(KeyCode::Down));
    let unknown = screen(&mut dialog);
    assert!(unknown.contains("Outcome: Failed"));
    assert!(unknown.contains("UNKNOWN; attempted effect not confirmed"));
    assert!(!unknown.contains("Applied (observed"));
    dialog.handle_key(key(KeyCode::Char('d')));
    assert!(dialog
        .detail()
        .contains("Window action observations: [\"unknown\"]"));
    assert!(!dialog.detail().contains("observed_applied"));
    dialog.handle_key(key(KeyCode::Esc));
    for code in [KeyCode::Char('o'), KeyCode::Enter] {
        dialog.handle_key(key(code));
    }
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    dialog.handle_key(key(KeyCode::Char('r')));
    let (Request::Read(selections), _) = calls.try_recv().unwrap() else {
        panic!("refresh can only read")
    };
    assert_eq!(selections.len(), 2);
    assert_eq!(selections[0].profile, "alpha\u{202e}\u{7f}");
}

#[test]
fn export_and_annotation_editors_dispatch_only_explicit_exact_submissions() {
    let (mut dialog, calls) = harness(report());
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('n'),
        KeyCode::Char('c'),
        KeyCode::Char('v'),
    ] {
        dialog.handle_key(key(code));
        assert!(dialog.editor.is_some());
        assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Continue);
        assert!(dialog.editor.is_none());
    }
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    let captured = serde_json::to_value(report()).unwrap();
    for (code, expected) in [
        (KeyCode::Char('j'), Format::Json),
        (KeyCode::Char('m'), Format::Markdown),
    ] {
        if code == KeyCode::Char('m') {
            dialog.handle_key(key(KeyCode::Char('d')));
        }
        dialog.handle_key(key(code));
        type_text(&mut dialog, "/private/review.out");
        assert!(screen(&mut dialog).contains("owned 0700 directory"));
        dialog.handle_key(key(KeyCode::Enter));
        let (Request::Export(report, format, path), reply) = calls.try_recv().unwrap() else {
            panic!("explicit export expected")
        };
        assert_eq!(
            std::mem::discriminant(&format),
            std::mem::discriminant(&expected)
        );
        assert_eq!(path, PathBuf::from("/private/review.out"));
        assert_eq!(report.selections.len(), 2);
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            captured,
            "export must preserve the full captured report at every disclosure level"
        );
        assert!(reply
            .send(Reply::Written(Err(anyhow::anyhow!("destination exists"))))
            .is_ok());
        assert!(dialog.tick());
        assert!(screen(&mut dialog).contains("Write not confirmed"));
        dialog.handle_key(key(KeyCode::Esc));
        if code == KeyCode::Char('m') {
            dialog.handle_key(key(KeyCode::Esc));
        }
    }
    dialog.handle_key(key(KeyCode::Enter));
    for (code, kind) in [
        ('n', annotations::Kind::Note),
        ('c', annotations::Kind::Correction),
        ('v', annotations::Kind::Reviewed),
    ] {
        dialog.handle_key(key(KeyCode::Char(code)));
        type_text(&mut dialog, "Operator comment");
        dialog.handle_key(key(KeyCode::Enter));
        let (Request::Annotate(request), reply) = calls.try_recv().unwrap() else {
            panic!("annotation expected")
        };
        assert_eq!(request.kind, kind);
        assert_eq!(request.target.selection.profile, "alpha\u{202e}\u{7f}");
        assert_eq!(
            request.target.decision,
            Some(uuid::Uuid::from_u128(11).to_string())
        );
        assert_eq!(request.target.original_hash, "b".repeat(64));
        assert!(uuid::Uuid::parse_str(&request.id).is_ok());
        assert_eq!(
            request.prior.is_some(),
            kind == annotations::Kind::Correction
        );
        assert!(reply
            .send(Reply::Written(Err(anyhow::anyhow!(
                "publication uncertain"
            ))))
            .is_ok());
        assert!(dialog.tick());
        type_text(&mut dialog, "must not change submitted content");
        dialog.handle_key(key(KeyCode::Enter));
        let (Request::Annotate(retry), reply) = calls.try_recv().unwrap() else {
            panic!("exact retry expected")
        };
        assert_eq!(retry, request);
        assert!(reply
            .send(Reply::Written(Err(anyhow::anyhow!("still uncertain"))))
            .is_ok());
        dialog.tick();
        dialog.handle_key(key(KeyCode::Esc));
    }
    dialog.handle_key(key(KeyCode::Home));
    dialog.handle_key(key(KeyCode::Char('v')));
    dialog.handle_key(key(KeyCode::Enter));
    let (Request::Annotate(request), reply) = calls.try_recv().unwrap() else {
        panic!("explicit reviewed claim expected")
    };
    assert!(request.target.decision.is_none());
    assert_eq!(request.target.original_hash, "a".repeat(64));
    assert!(reply
        .send(Reply::Written(Ok(
            "Operator note stored; not approval".into()
        )))
        .is_ok());
    dialog.tick();
    let (Request::Read(_), reply) = calls.try_recv().unwrap() else {
        panic!("successful write only refreshes stored review")
    };
    assert!(reply.send(Reply::Read(Ok(report()))).is_ok());
    dialog.tick();
    assert!(dialog.editor.is_none());
    assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Continue);
    assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Cancel);
}

#[test]
fn incomplete_and_failed_reads_remain_visible_without_permitting_uncertain_annotations() {
    let mut value = report();
    value.complete = false;
    value.selections[0]
        .errors
        .push("Annotation target mismatch\u{202e}\u{1b}".into());
    let (mut dialog, calls) = harness(value);
    assert!(screen(&mut dialog).contains("INCOMPLETE COLLECTION"));
    dialog.handle_key(key(KeyCode::Char('d')));
    assert!(dialog
        .detail()
        .contains("INCOMPLETE: Annotation target mismatch"));
    dialog.handle_key(key(KeyCode::Esc));
    dialog.handle_key(key(KeyCode::Enter));
    assert!(
        dialog.detail().contains("Operator claim, not an answer"),
        "mismatched claims must remain visible"
    );
    dialog.handle_key(key(KeyCode::Char('n')));
    assert!(dialog.editor.is_none());
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    dialog.handle_key(key(KeyCode::Char('r')));
    let (Request::Read(_), reply) = calls.try_recv().unwrap() else {
        panic!("read expected")
    };
    assert!(reply
        .send(Reply::Read(Err(anyhow::anyhow!("journal veto\u{202e}"))))
        .is_ok());
    dialog.tick();
    let text = screen(&mut dialog);
    assert!(text.contains("Read failed"));
    assert!(text.contains("snapshot is older"));
    assert!(!text.contains(['\u{202e}', '\u{1b}', '\u{7f}']));
    assert!(dialog.report.is_some());
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn review_usage_preserves_production_uncertainty_and_operator_rate_basis() {
    for (usage, expected) in [
        (
            json!({"measurement":{"gaps":["closing"],"reported_tokens":10,"tokens_complete":false},"gap":null,"closing":true,"baseline":{"count":0},"observed_through":{"count":2},"cutoff":null,"guard":{"version":1,"basis":"observed-native-entries","tokens":null,"usd":{"currency":"USD","basis":"operator-fixed-rates","limit_nano":1000000000,"quotes":[{"model":{"provider":"fixture","model":"chosen"},"input":123,"output":456,"cache_read":0,"cache_write":0,"cache_write_1h":null}]}}}),
            vec![
                "Measurement gaps: [\"closing\"]",
                "Closing: true",
                "Baseline:",
                "Observed through:",
                "Cutoff:",
                "observed-native-entries",
                "operator-fixed-rates",
                "chosen",
                "123",
            ],
        ),
        (
            json!({"measurement":{"gaps":["unpriced"],"valuation_complete":false},"gap":null}),
            vec!["Measurement gaps: [\"unpriced\"]"],
        ),
        (
            json!({"measurement":null,"unavailable":"source unavailable"}),
            vec!["Unavailable: source unavailable"],
        ),
        (
            json!({"coverage":"not_collected","guard":null}),
            vec!["Coverage: not_collected"],
        ),
    ] {
        let mut value = report();
        value.selections[0].window.as_mut().unwrap()["usage"] = usage;
        let (mut dialog, _) = harness(value);
        assert!(screen(&mut dialog).contains("Accounting gaps: local"));
        dialog.handle_key(key(KeyCode::Char('d')));
        let detail = dialog.detail();
        for expected in expected {
            assert!(detail.contains(expected), "missing {expected}: {detail}");
        }
        assert!(detail.contains("In-flight overshoot: unknown"));
        assert!(detail.contains("NOT an invoice"));
    }
}

#[test]
fn review_annotations_keep_exact_targets_corrections_and_tombstones_without_window() {
    let mut value = report();
    let decision_note = value.selections[0].annotations[0].clone();
    let mut window_note = decision_note.clone();
    window_note.id = uuid::Uuid::from_u128(100).to_string();
    window_note.target.decision = None;
    window_note.target.original_hash = "a".repeat(64);
    window_note.kind = annotations::Kind::Note;
    window_note.body = Some("whole window claim".into());
    let mut correction = decision_note.clone();
    correction.id = uuid::Uuid::from_u128(101).to_string();
    correction.kind = annotations::Kind::Correction;
    correction.prior = Some(decision_note.id.clone());
    correction.body = None;
    value.selections[0]
        .annotations
        .extend([window_note.clone(), correction.clone()]);
    for missing_window in [false, true] {
        let mut value = value.clone();
        if missing_window {
            value.complete = false;
            value.selections[0].window = None;
            value.selections[0]
                .errors
                .push("captured source unavailable".into());
        }
        let (mut dialog, _) = harness(value);
        dialog.handle_key(key(KeyCode::Char('d')));
        let detail = dialog.detail();
        for note in [&decision_note, &window_note, &correction] {
            let block = detail
                .split("Annotation ")
                .find(|part| part.starts_with(&note.id))
                .expect("each retained annotation has an attributed block");
            assert!(block.contains(&review::terminal_text(&note.target.selection.profile)));
            assert!(block.contains(&note.target.selection.session_id));
            assert!(block.contains(&note.target.selection.window_id));
            assert!(block.contains(note.target.decision.as_deref().unwrap_or("window grant")));
            assert!(block.contains(&note.target.original_hash));
        }
        assert!(detail.contains("whole window claim"));
        assert!(detail.contains("Operator claim, not an answer"));
        assert!(detail.contains(&format!("Prior annotation: {}", decision_note.id)));
        assert!(detail.contains("expired body; identity retained"));
        if missing_window {
            assert!(detail.contains("INCOMPLETE: captured source unavailable"));
        }
    }
}

#[test]
fn review_host_continuation_guards_are_not_model_claims_or_operator_approvals() {
    for (question, nudge, outcome) in [
        (true, true, "accepted"),
        (false, false, "ordinary"),
        (false, false, "accepted"),
        (true, false, "unknown"),
    ] {
        let mut value = report();
        let window = value.selections[0].window.as_mut().unwrap();
        window["unresolved_question_guard"] = json!(question);
        window["unresolved_nudge"] = json!(nudge);
        window["ambiguous"] = json!(question || nudge);
        window["question"] = json!({"outcome":outcome,"observed_request":null,"recovered_at_ms":if outcome == "accepted" && !question {Some(3000)} else {None}});
        window["nudges"] =
            json!([{"observed_request":if nudge {None} else {Some("observed-followup")}}]);
        let (mut dialog, calls) = harness(value);
        let summary = screen(&mut dialog);
        assert_eq!(summary.contains("question unresolved"), question);
        assert_eq!(summary.contains("nudge unresolved"), nudge);
        dialog.handle_key(key(KeyCode::Enter));
        assert!(dialog.detail().contains("human-required claim: false"));
        dialog.handle_key(key(KeyCode::Char('d')));
        let detail = dialog.detail();
        for expected in [
            format!("Host question guard unresolved: {question}"),
            format!("Host nudge unresolved: {nudge}"),
            format!("Stored ambiguity: {}", question || nudge),
        ] {
            assert!(detail.contains(&expected), "missing {expected}: {detail}");
        }
        assert!(detail.contains(outcome));
        if !nudge {
            assert!(detail.contains("observed-followup"));
        }
        assert!(detail.contains("human-required claim: false"));
        assert!(detail.contains("Reviewed annotations do not resolve host guards"));
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

#[test]
fn review_write_pending_disconnect_and_escape_never_imply_cancellation_or_retry() {
    for level in [View::Summary, View::Decisions, View::Diagnostics] {
        let (mut dialog, calls) = harness(report());
        if level != View::Summary {
            dialog.handle_key(key(KeyCode::Enter));
        }
        if level == View::Diagnostics {
            dialog.handle_key(key(KeyCode::Char('d')));
        }
        let target = dialog.target().unwrap();
        dialog.handle_key(key(KeyCode::Char('n')));
        type_text(&mut dialog, "deliberate claim");
        dialog.handle_key(key(KeyCode::Enter));
        let (Request::Annotate(request), reply) = calls.try_recv().unwrap() else {
            panic!("explicit annotation expected")
        };
        assert_eq!(request.target, target);
        assert!(!dialog.tick(), "pending channel has no reply yet");
        assert!(screen_size(&mut dialog, 80, 24)
            .contains("Submitted write still running; closing does not cancel"));
        dialog.handle_key(key(KeyCode::Enter));
        type_text(&mut dialog, "ignored while pending");
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(reply);
        assert!(dialog.tick(), "disconnect is causally established");
        assert!(screen(&mut dialog).contains("worker unavailable"));
        dialog.handle_key(key(KeyCode::Enter));
        let (Request::Annotate(retry), reply) = calls.try_recv().unwrap() else {
            panic!("explicit retry expected")
        };
        assert_eq!(retry, request);
        assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Continue);
        assert!(dialog.editor.is_none());
        assert!(dialog.pending.is_some());
        assert!(dialog.message.as_ref().unwrap().contains("does not cancel"));
        while dialog.view != View::Summary {
            assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Continue);
            assert!(
                dialog.pending.is_some(),
                "internal back must retain the receiver"
            );
            assert_eq!(dialog.selected_window.as_ref(), Some(&target.selection));
        }
        assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Cancel);
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(reply);
    }
}

#[test]
fn review_long_editor_content_is_visible_and_scrollable_on_small_terminal() {
    let (mut dialog, _) = harness(report());
    dialog.handle_key(key(KeyCode::Char('n')));
    type_text(&mut dialog, &format!("{}TAIL-MARKER", "word ".repeat(350)));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area(), &Theme::default()))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        text.contains("TAIL-MARKER"),
        "the active input tail must remain visible"
    );
    for _ in 0..4 {
        dialog.handle_key(key(KeyCode::PageUp));
    }
    terminal
        .draw(|frame| dialog.render(frame, frame.area(), &Theme::default()))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("note") || text.contains("Note"));
    assert!(
        !text.contains("TAIL-MARKER"),
        "page navigation must expose earlier editor context"
    );
}
