use super::tests::{buffer_size, harness, key, record, report, screen, screen_size};
use super::*;
use serde_json::json;

fn complete_usage() -> serde_json::Value {
    json!({"measurement":{"reported_tokens":10,"tokens_complete":true,"valuation_complete":true,"gaps":[]},"gap":null,"blocked":null,"closing":true,"cutoff":{"count":2},"source_complete":true})
}

#[test]
fn abandoned_uncertain_write_survives_unrelated_success_and_refresh() {
    for disconnect in [false, true] {
        let (mut dialog, calls) = harness(report());
        dialog.handle_key(key(KeyCode::Char('n')));
        dialog.handle_paste("uncertain A");
        dialog.handle_key(key(KeyCode::Enter));
        let (Request::Annotate(original), reply) = calls.try_recv().unwrap() else {
            panic!("A expected")
        };
        if disconnect {
            drop(reply);
        } else {
            reply
                .send(Reply::Written(Err(anyhow::anyhow!(
                    "publication uncertain"
                ))))
                .unwrap();
        }
        assert!(dialog.tick());
        dialog.handle_key(key(KeyCode::Esc));
        dialog.handle_key(key(KeyCode::Char(if disconnect { 'j' } else { 'n' })));
        dialog.handle_paste(if disconnect {
            "/private/B.json"
        } else {
            "unrelated B"
        });
        dialog.handle_key(key(KeyCode::Enter));
        let (request, reply) = calls.try_recv().unwrap();
        match request {
            Request::Annotate(other) => assert_ne!(other.id, original.id),
            Request::Export(_, Format::Json, path) => {
                assert_eq!(path, PathBuf::from("/private/B.json"))
            }
            _ => panic!("only explicit B may be submitted"),
        }
        reply.send(Reply::Written(Ok("B saved".into()))).unwrap();
        assert!(dialog.tick());
        let (Request::Read(selections), reply) = calls.try_recv().unwrap() else {
            panic!("post-write reread expected")
        };
        assert_eq!(selections, dialog.selections);
        reply.send(Reply::Read(Ok(report()))).unwrap();
        assert!(dialog.tick());
        for (width, height) in [(80, 24), (132, 44)] {
            let text = screen_size(&mut dialog, width, height);
            assert!(
                text.contains("Earlier write not confirmed"),
                "unrelated success lost A's uncertainty: {text}"
            );
            assert!(
                !text.contains(&original.id),
                "summary should not expose request UUIDs"
            );
        }
        dialog.handle_key(key(KeyCode::Char('r')));
        let (Request::Read(_), reply) = calls.try_recv().unwrap() else {
            panic!("explicit refresh expected")
        };
        reply.send(Reply::Read(Ok(report()))).unwrap();
        assert!(dialog.tick());
        assert!(screen(&mut dialog).contains("Earlier write not confirmed"));
        assert!(
            matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "A must never be retried automatically"
        );
    }
}

#[test]
fn pending_read_and_write_escape_preserve_disclosure_path_and_receiver() {
    for write in [false, true] {
        for origin in [View::Summary, View::Decisions] {
            let (mut dialog, calls) = harness(report());
            if origin == View::Decisions {
                dialog.handle_key(key(KeyCode::Enter));
                dialog.handle_key(key(KeyCode::Down));
            }
            let target = dialog.target().unwrap();
            dialog.handle_key(key(KeyCode::Char('d')));
            if write {
                dialog.handle_key(key(KeyCode::Char('n')));
                dialog.handle_paste("pending exact claim");
                dialog.handle_key(key(KeyCode::Enter));
            } else {
                dialog.handle_key(key(KeyCode::Char('r')));
            }
            let (request, reply) = calls.try_recv().unwrap();
            match request {
                Request::Annotate(request) => assert_eq!(request.target, target),
                Request::Read(selections) => assert_eq!(selections, dialog.selections),
                _ => panic!("unexpected operation"),
            }
            if write {
                assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Continue);
                assert!(dialog.editor.is_none());
                assert_eq!(dialog.view, View::Diagnostics);
                let text = screen_size(&mut dialog, 80, 24);
                assert!(text.contains("closing does not cancel"));
            }
            assert_eq!(
                dialog.handle_key(key(KeyCode::Esc)),
                DialogResult::Continue,
                "pending diagnostics should go back, not close review"
            );
            assert_eq!(dialog.view, origin);
            assert_eq!(dialog.target().unwrap(), target);
            assert!(dialog.pending.is_some());
            if origin == View::Decisions {
                assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Continue);
                assert_eq!(dialog.view, View::Summary);
                assert_eq!(dialog.selected_window.as_ref(), Some(&target.selection));
                assert_eq!(
                    dialog.decisions.get(&target.selection).unwrap(),
                    &target.decision
                );
            }
            assert!(dialog.pending.is_some());
            assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
            if write {
                reply.send(Reply::Written(Ok("saved".into()))).unwrap();
                assert!(dialog.tick());
                let (Request::Read(_), reply) = calls.try_recv().unwrap() else {
                    panic!("post-write read expected")
                };
                reply.send(Reply::Read(Ok(report()))).unwrap();
            } else {
                reply.send(Reply::Read(Ok(report()))).unwrap();
            }
            assert!(dialog.tick());
            if origin == View::Decisions {
                dialog.handle_key(key(KeyCode::Enter));
            }
            assert_eq!(dialog.target().unwrap(), target);
            assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
        }
    }
}

#[test]
fn initial_read_error_or_disconnect_is_not_loading_or_an_older_snapshot() {
    for disconnect in [false, true] {
        let value = report();
        let selections = value
            .selections
            .iter()
            .map(|row| row.selection.clone())
            .collect::<Vec<_>>();
        let (send, calls) = mpsc::channel();
        let mut dialog = AfkReviewDialog::with_dispatch(
            selections.clone(),
            ReviewLabels::default(),
            Box::new(move |request| {
                let (reply, receiver) = mpsc::channel();
                send.send((request, reply)).unwrap();
                receiver
            }),
        );
        let (Request::Read(received), reply) = calls.try_recv().unwrap() else {
            panic!("initial read expected")
        };
        assert_eq!(received, selections);
        assert!(screen_size(&mut dialog, 80, 24).contains("Reading stored facts"));
        if disconnect {
            drop(reply);
        } else {
            reply
                .send(Reply::Read(Err(anyhow::anyhow!("source unavailable"))))
                .unwrap();
        }
        assert!(dialog.tick());
        for (width, height) in [(80, 24), (132, 44)] {
            let text = screen_size(&mut dialog, width, height);
            assert!(
                text.contains("Read failed; no snapshot available"),
                "first failure must not invent a snapshot: {text}"
            );
            assert!(!text.contains("Reading stored facts"));
            assert!(!text.contains("snapshot is older"));
            assert!(text.contains("r refresh"));
        }
        assert!(dialog.pending.is_none());
        dialog.handle_key(key(KeyCode::Char('r')));
        let (Request::Read(received), reply) = calls.try_recv().unwrap() else {
            panic!("explicit recovery read expected")
        };
        assert_eq!(received, selections);
        reply.send(Reply::Read(Ok(value))).unwrap();
        assert!(dialog.tick());
        let text = screen(&mut dialog);
        assert!(text.contains("Applied (observed after the attempt)"));
        assert!(!text.contains("Read failed"));
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

#[test]
fn success_receipts_are_visible_and_pending_or_fixed_editor_hints_are_truthful() {
    for (mode, complete, receipt) in [
        ('n', true, "Note saved"),
        ('j', true, "Export saved"),
        ('m', false, "Export saved (INCOMPLETE)"),
    ] {
        let mut value = report();
        value.complete = complete;
        let (mut dialog, calls) = harness(value.clone());
        dialog.handle_key(key(KeyCode::Char(mode)));
        dialog.handle_paste(if mode == 'n' {
            "claim"
        } else {
            "/private/result"
        });
        dialog.handle_key(key(KeyCode::Enter));
        let (request, reply) = calls.try_recv().unwrap();
        let original_note = match request {
            Request::Annotate(request) => Some(request),
            Request::Export(_, _, _) => None,
            _ => panic!("explicit write expected"),
        };
        for (width, height) in [(80, 24), (132, 44)] {
            let text = screen_size(&mut dialog, width, height);
            for inactive in [
                "Enter submit",
                "Ctrl+u clear",
                "PgUp/PgDn scroll input",
                "? actions",
            ] {
                assert!(
                    !text.contains(inactive),
                    "pending footer advertises inactive {inactive}"
                );
            }
            assert!(text.contains("Esc back"));
        }
        if let Some(original) = original_note {
            reply
                .send(Reply::Written(Err(anyhow::anyhow!("uncertain"))))
                .unwrap();
            assert!(dialog.tick());
            let text = screen(&mut dialog);
            assert!(
                !text.contains("Ctrl+u clear"),
                "submitted annotation is immutable"
            );
            dialog.handle_key(key(KeyCode::Enter));
            let (Request::Annotate(retry), retry_reply) = calls.try_recv().unwrap() else {
                panic!("exact retry expected")
            };
            assert_eq!(retry, original);
            retry_reply
                .send(Reply::Written(Ok(
                    "service receipt with UUID hidden by default".into(),
                )))
                .unwrap();
        } else {
            reply
                .send(Reply::Written(Ok(
                    "service receipt with path hidden by default".into(),
                )))
                .unwrap();
        }
        assert!(dialog.tick());
        let (Request::Read(_), reply) = calls.try_recv().unwrap() else {
            panic!("post-write read expected")
        };
        for (width, height) in [(80, 24), (132, 44)] {
            assert!(
                screen_size(&mut dialog, width, height).contains(receipt),
                "success must be visible during reread"
            );
        }
        reply.send(Reply::Read(Ok(value))).unwrap();
        assert!(dialog.tick());
        for (width, height) in [(80, 24), (132, 44)] {
            let text = screen_size(&mut dialog, width, height);
            assert!(
                text.contains(receipt),
                "success must stay visible after reread: {text}"
            );
            assert!(
                !text.contains("Write not confirmed"),
                "confirmed exact retry clears its own uncertainty"
            );
        }
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

#[test]
fn long_profile_does_not_hide_the_human_name_or_change_the_exact_target() {
    let mut value = report();
    let profile = format!("profile-{}", "p".repeat(220));
    value.selections[0].selection.profile = profile.clone();
    let (mut dialog, calls) = harness(value);
    for (width, height) in [(80, 24), (132, 44)] {
        let text = screen_size(&mut dialog, width, height);
        assert!(
            text.contains("Workstream 1"),
            "profile consumed the human name: {text}"
        );
        assert!(text.contains("#1"));
        assert!(text.contains("profile-"), "retain bounded profile context");
    }
    dialog.handle_key(key(KeyCode::Char('v')));
    dialog.handle_key(key(KeyCode::Enter));
    let (Request::Annotate(request), reply) = calls.try_recv().unwrap() else {
        panic!("explicit reviewed claim expected")
    };
    assert_eq!(request.target.selection.profile, profile);
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    drop(reply);
}

#[test]
fn changed_export_does_not_confirm_old_request_and_closed_exact_retry_can_confirm() {
    for changed in [false, true] {
        let (mut dialog, calls) = harness(report());
        dialog.handle_key(key(KeyCode::Char('j')));
        dialog.handle_paste("/private/A.json");
        dialog.handle_key(key(KeyCode::Enter));
        let (Request::Export(original, _, path), reply) = calls.try_recv().unwrap() else {
            panic!("export A expected")
        };
        reply
            .send(Reply::Written(Err(anyhow::anyhow!(
                "publication uncertain"
            ))))
            .unwrap();
        assert!(dialog.tick());
        if changed {
            dialog.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
            dialog.handle_paste("/private/B.json");
        }
        dialog.handle_key(key(KeyCode::Enter));
        let (Request::Export(retry, _, retry_path), reply) = calls.try_recv().unwrap() else {
            panic!("explicit export expected")
        };
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&retry).unwrap()
        );
        assert_eq!(path == retry_path, !changed);
        dialog.handle_key(key(KeyCode::Esc));
        assert!(dialog.pending.is_some());
        reply
            .send(Reply::Written(Ok("export stored".into())))
            .unwrap();
        assert!(dialog.tick());
        let (Request::Read(_), reply) = calls.try_recv().unwrap() else {
            panic!("post-write read expected")
        };
        reply.send(Reply::Read(Ok(report()))).unwrap();
        assert!(dialog.tick());
        let text = screen(&mut dialog);
        assert!(text.contains("Export saved"));
        assert_eq!(text.contains("Earlier write not confirmed"), changed);
        assert!(!text.contains("Write not confirmed;"));
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

#[test]
fn summary_has_plain_outcome_hierarchy_and_passive_actions_help() {
    let mut value = report();
    let window = value.selections[0].window.as_mut().unwrap();
    window["usage"] = complete_usage();
    window["unresolved_question_guard"] = json!(false);
    window["unresolved_nudge"] = json!(false);
    window["ambiguous"] = json!(false);
    window["records"].as_array_mut().unwrap().truncate(1);
    let (mut dialog, calls) = harness(value);
    let target = dialog.target().unwrap();
    for (width, height) in [(80, 24), (132, 44)] {
        let text = screen_size(&mut dialog, width, height);
        for expected in [
            "Outcome: Completed",
            "Effect: Applied",
            "Needs attention: none recorded",
            "? actions",
        ] {
            assert!(text.contains(expected));
        }
        for absent in [
            "Attention: human",
            "reason: completed",
            "expiry: passed",
            "question clear",
            "nudge clear",
            "Usage denied: none",
            "j/m export",
            "c correct",
        ] {
            assert!(
                !text.contains(absent),
                "default chrome should not include {absent}"
            );
        }
        dialog.handle_key(key(KeyCode::Char('?')));
        let help = screen_size(&mut dialog, width, height);
        assert!(help.contains("j/m export full report"));
        assert!(help.contains("n note  c correction  v reviewed claim"));
        assert_eq!(dialog.target().unwrap(), target);
        dialog.handle_paste("help paste is not composition");
        assert_eq!(dialog.handle_key(key(KeyCode::Esc)), DialogResult::Continue);
        assert_eq!(dialog.view, View::Summary);
        assert!(!dialog.actions_help);
    }
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    dialog.handle_key(key(KeyCode::Char('?')));
    dialog.handle_key(key(KeyCode::Char('n')));
    assert!(
        dialog.editor.is_some(),
        "help retains the same direct shortcuts"
    );
    assert!(!dialog.actions_help);
    assert_eq!(dialog.target().unwrap(), target);
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    dialog.handle_key(key(KeyCode::Esc));
    let report = dialog.report.as_mut().unwrap();
    report.complete = false;
    report.selections[0]
        .errors
        .push("retained source conflict".into());
    for (width, height) in [(80, 24), (132, 44)] {
        let text = screen_size(&mut dialog, width, height);
        assert!(text.contains("Source incomplete"));
        assert!(!text.contains("Needs attention: none recorded"));
    }
    assert!(dialog.target().is_none());
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn plain_outcome_keeps_active_expiry_and_exact_lifecycle_diagnostics() {
    for (state, reason, expired, expected) in [
        ("ended", Some("completed"), true, "Outcome: Completed"),
        ("ended", Some("failed"), true, "Outcome: Failed"),
        ("ended", Some("deferred"), false, "Outcome: Deferred"),
        (
            "ended",
            Some("exhausted"),
            true,
            "Outcome: Stopped: allowance exhausted",
        ),
        ("ended", Some("interrupted"), true, "Outcome: Interrupted"),
        (
            "ended",
            Some("delivery_unknown"),
            false,
            "Outcome: Delivery unknown",
        ),
        ("ended", None, false, "Outcome: Ended; reason unavailable"),
        (
            "pending",
            None,
            true,
            "Outcome: Pending (stored); expiry passed",
        ),
        (
            "owned",
            None,
            true,
            "Outcome: In progress (stored); expiry passed",
        ),
    ] {
        let mut value = report();
        let window = value.selections[0].window.as_mut().unwrap();
        window["state"] = json!(state);
        window["terminal_reason"] = json!(reason);
        window["expired"] = json!(expired);
        let (mut dialog, calls) = harness(value);
        let summary = screen_size(&mut dialog, 80, 24);
        assert!(summary.contains(expected));
        assert_eq!(
            summary.contains("expiry passed"),
            state != "ended" && expired
        );
        assert!(
            summary.contains("Applied (observed after the attempt)"),
            "lifecycle cannot reinterpret the effect observation"
        );
        dialog.handle_key(key(KeyCode::Char('d')));
        let exact = dialog.detail();
        assert!(exact.contains(&format!("\"state\": \"{state}\"")));
        assert!(exact.contains(&format!("\"expired\": {expired}")));
        assert!(exact.contains(&format!("\"terminal_reason\": {}", json!(reason))));
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

#[test]
fn critical_categories_survive_small_layout_long_labels_scroll_and_stale_snapshot() {
    let mut value = report();
    value.complete = false;
    let row = &mut value.selections[0];
    row.errors.push(format!(
        "{}\u{202e}\nerror tail",
        "source unavailable ".repeat(40)
    ));
    let window = row.window.as_mut().unwrap();
    window["action"]["outcomes"] = json!(["unknown"]);
    window["unresolved_question_guard"] = json!(true);
    window["unresolved_nudge"] = json!(true);
    window["ambiguous"] = json!(true);
    window["usage"]["blocked"] = json!("exhausted");
    window["records"][0]["claims"]["rationale"] = json!("unshown prose ".repeat(400));
    let (mut dialog, calls) = harness(value);
    let selection = dialog.selections[0].clone();
    dialog.labels.names.insert(
        (selection.profile, selection.session_id),
        Some(format!("Long workstream \u{202e}\n{}", "界".repeat(200))),
    );
    dialog.handle_key(key(KeyCode::Char('r')));
    let (Request::Read(_), reply) = calls.try_recv().unwrap() else {
        panic!("read expected")
    };
    reply
        .send(Reply::Read(Err(anyhow::anyhow!(
            "{}",
            "unavailable\n\u{1b}".repeat(100)
        ))))
        .unwrap();
    assert!(dialog.tick());
    for (width, height) in [(80, 24), (132, 44), (80, 24)] {
        for _ in 0..2 {
            let text = screen_size(&mut dialog, width, height);
            for expected in [
                "INCOMPLETE COLLECTION",
                "Read failed; shown snapshot is older",
                "Outcome: Completed",
                "UNKNOWN; attempted effect not confirmed",
                "question unresolved; nudge unresolved",
                "host-required; model-required",
                "Usage denied: local",
                "Accounting gaps: local",
                "Usage evidence pending",
                "Enter decisions",
                "? actions",
            ] {
                assert!(
                    text.contains(expected),
                    "{width}x{height} missing {expected}: {text}"
                );
            }
            for absent in [
                "payload_hash",
                "unshown prose",
                "Criterion clarity",
                "00000000-0000-0000-0000-000000000001",
            ] {
                assert!(!text.contains(absent));
            }
            assert!(!text.contains(['\u{202e}', '\u{1b}']));
            dialog.handle_key(key(KeyCode::PageDown));
        }
        let buffer = buffer_size(&mut dialog, width, height);
        if width == 80 {
            assert!((2..78).all(|x| buffer[(x, 5)].symbol() == "─"));
            assert_eq!(buffer[(2, 4)].symbol(), " ");
            assert_eq!(buffer[(2, 6)].symbol(), " ");
            assert_eq!(buffer[(3, 6)].symbol(), "#");
        } else {
            assert!((4..41).all(|y| buffer[(38, y)].symbol() == "│"));
            for x in [37, 39] {
                assert!((4..41).all(|y| buffer[(x, y)].symbol() == " "));
            }
            assert_eq!(buffer[(40, 4)].symbol(), "#");
        }
    }
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn shared_only_denial_and_missing_projection_are_joined_by_exact_original_reference() {
    let mut value = report();
    let window = value.selections[0].window.as_mut().unwrap();
    window["usage"] = complete_usage();
    window["shared_usage_reference"] =
        json!({"profile":"exact profile", "operation_id":"original operation"});
    value.shared_usage = vec![
        json!({"profile":"different profile", "operation_id":"original operation", "usage":{"coverage":"stored_reported_interval", "blocked":null}}),
        json!({"profile":"exact profile", "operation_id":"replacement operation", "usage":{"coverage":"stored_reported_interval", "blocked":null}}),
        json!({"profile":"exact profile", "operation_id":"original operation", "usage":{"blocked":"exhausted", "coverage":"partial_or_unavailable", "gaps":["member usage unavailable"], "negative_evidence_pending":true, "known_reported_tokens":42}}),
    ];
    let (mut dialog, calls) = harness(value.clone());
    for (width, height) in [(80, 24), (132, 44)] {
        let text = screen_size(&mut dialog, width, height);
        assert!(text.contains("Usage denied: shared | Accounting gaps: shared"));
        assert!(text.contains("Usage evidence pending"));
        assert!(!text.contains("known_reported_tokens"));
    }
    dialog.handle_key(key(KeyCode::Char('d')));
    let diagnostics = dialog.detail();
    assert!(diagnostics.contains("original operation"));
    assert!(diagnostics.contains("member usage unavailable"));
    assert!(diagnostics.contains("known_reported_tokens"));
    assert!(!diagnostics.contains("replacement operation"));
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    value.shared_usage.pop();
    let (mut missing, _) = harness(value);
    let text = screen_size(&mut missing, 80, 24);
    assert!(text.contains("Original shared usage unavailable"));
    assert!(text.contains("Accounting gaps: shared"));
    assert!(
        !presentation::usage_gap(&complete_usage(), false),
        "closing with a fixed cutoff and complete projection is not unfinished accounting"
    );
}

#[test]
fn latest_effect_observation_is_not_intent_uncertainty_or_inferred_absence() {
    for (action, expected, uncertain) in [
        (
            json!({"decision":"matching", "outcomes":["unknown","observed_applied"]}),
            "Applied (observed after the attempt)",
            false,
        ),
        (
            json!({"decision":"matching", "outcomes":["unknown"]}),
            "UNKNOWN; attempted effect not confirmed",
            true,
        ),
        (
            json!({"decision":"matching", "outcomes":[]}),
            "No effect observation recorded; not proof of no effect",
            true,
        ),
        (
            serde_json::Value::Null,
            "No effect observation recorded; not proof of no effect",
            true,
        ),
    ] {
        let mut value = report();
        let window = value.selections[0].window.as_mut().unwrap();
        window["action"] = action;
        assert_eq!(presentation::effect(window, None), expected);
        assert!(presentation::effect(window, Some("other decision")).contains("for this decision"));
        let (mut dialog, calls) = harness(value);
        for (width, height) in [(80, 24), (132, 44)] {
            assert!(screen_size(&mut dialog, width, height).contains(expected));
        }
        let attention =
            presentation::attention(dialog.report.as_ref().unwrap(), dialog.selected_row());
        assert_eq!(attention.effect, uncertain);
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

#[test]
fn refresh_reorder_disappearance_and_post_write_reread_never_retarget_selection() {
    let initial = report();
    let selections: Vec<_> = initial
        .selections
        .iter()
        .map(|r| r.selection.clone())
        .collect();
    let (mut dialog, calls) = harness(initial.clone());
    dialog.handle_key(key(KeyCode::Enter));
    dialog.handle_key(key(KeyCode::Down));
    let original = dialog.target().unwrap();
    assert_eq!(
        original.decision,
        Some(uuid::Uuid::from_u128(12).to_string())
    );
    dialog.handle_key(key(KeyCode::Char('d')));
    let mut reordered = initial.clone();
    reordered.selections.reverse();
    let window = reordered.selections[1].window.as_mut().unwrap();
    window["records"].as_array_mut().unwrap().reverse();
    window["records"][1]["claims"] = serde_json::Value::Null;
    window["records"][1]["body_status"] = json!("expired");
    let reread = |dialog: &mut AfkReviewDialog, value: Report| {
        dialog.handle_key(key(KeyCode::Char('r')));
        let (Request::Read(received), reply) = calls.try_recv().unwrap() else {
            panic!("read expected")
        };
        assert_eq!(received, selections);
        assert!(!dialog.tick());
        reply.send(Reply::Read(Ok(value))).unwrap();
        assert!(dialog.tick());
    };
    reread(&mut dialog, reordered.clone());
    assert_eq!(dialog.view, View::Diagnostics);
    assert_eq!(dialog.target().unwrap(), original);
    assert!(dialog.detail().contains("Original claim body unavailable"));
    for code in [KeyCode::Down, KeyCode::Home, KeyCode::End] {
        dialog.handle_key(key(code));
    }
    assert_eq!(dialog.target().unwrap(), original);
    dialog.handle_key(key(KeyCode::Esc));
    assert_eq!(dialog.view, View::Decisions);
    assert!(screen(&mut dialog).contains("Original claim body unavailable"));
    dialog.handle_key(key(KeyCode::Esc));
    dialog.handle_key(key(KeyCode::Down));
    dialog.handle_key(key(KeyCode::Enter));
    assert_eq!(dialog.target().unwrap().selection, selections[1]);
    dialog.handle_key(key(KeyCode::Esc));
    dialog.handle_key(key(KeyCode::Up));
    dialog.handle_key(key(KeyCode::Enter));
    assert_eq!(dialog.target().unwrap(), original);
    dialog.handle_key(key(KeyCode::Char('n')));
    dialog.handle_paste("retained exact target");
    dialog.handle_key(key(KeyCode::Enter));
    let (Request::Annotate(request), reply) = calls.try_recv().unwrap() else {
        panic!("explicit annotation expected")
    };
    assert_eq!(request.target, original);
    reply
        .send(Reply::Written(Ok("stored operator claim".into())))
        .unwrap();
    assert!(dialog.tick());
    let (Request::Read(received), reply) = calls.try_recv().unwrap() else {
        panic!("post-write read expected")
    };
    assert_eq!(received, selections);
    reply.send(Reply::Read(Ok(reordered.clone()))).unwrap();
    assert!(dialog.tick());
    assert_eq!(dialog.target().unwrap(), original);
    let records = reordered.selections[1].window.as_mut().unwrap()["records"]
        .as_array_mut()
        .unwrap();
    records.retain(|r| r["id"].as_str() != original.decision.as_deref());
    records.push(record(99, "admitted", false));
    reread(&mut dialog, reordered.clone());
    assert_eq!(dialog.decision(), original.decision.as_deref());
    assert!(dialog.target().is_none());
    assert!(screen(&mut dialog).contains("Selected decision unavailable"));
    dialog.handle_key(key(KeyCode::Char('v')));
    assert!(dialog.editor.is_none());
    dialog.handle_key(key(KeyCode::Char('r')));
    let (Request::Read(_), reply) = calls.try_recv().unwrap() else {
        panic!("read expected")
    };
    reply
        .send(Reply::Read(Err(anyhow::anyhow!("journal veto"))))
        .unwrap();
    assert!(dialog.tick());
    assert!(screen(&mut dialog).contains("Read failed; shown snapshot is older"));
    assert_eq!(dialog.decision(), original.decision.as_deref());
    reordered
        .selections
        .retain(|r| r.selection != original.selection);
    reread(&mut dialog, reordered);
    assert_eq!(dialog.selected_window.as_ref(), Some(&original.selection));
    assert!(dialog.target().is_none());
    assert!(screen(&mut dialog).contains("Selected window unavailable"));
    dialog.handle_key(key(KeyCode::Char('d')));
    assert!(dialog.detail().contains(&original.selection.window_id));
    dialog.handle_key(key(KeyCode::Esc));
    dialog.handle_key(key(KeyCode::Esc));
    assert_eq!(dialog.view, View::Summary);
    assert_eq!(dialog.selected.selected(), Some(0));
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn disclosure_navigation_exposes_weighted_claims_and_exact_diagnostics_at_both_sizes() {
    for (width, height) in [(80, 24), (132, 44)] {
        let (mut dialog, calls) = harness(report());
        let summary = screen_size(&mut dialog, width, height);
        for hidden in [
            "payload_hash",
            "Criterion clarity",
            "reported_tokens",
            "00000000-0000-0000-0000-000000000001",
        ] {
            assert!(!summary.contains(hidden));
        }
        assert!(summary.contains("Workstream 1"));
        assert!(summary.contains("Applied (observed after the attempt)"));
        dialog.handle_key(key(KeyCode::Enter));
        let target = dialog.target().unwrap();
        let mut observed = String::new();
        for _ in 0..15 {
            observed.push_str(&screen_size(&mut dialog, width, height));
            dialog.handle_key(key(KeyCode::PageDown));
        }
        for expected in [
            "Host disposition: admitted",
            "human-required claim: false",
            "Criterion clarity: weight 4",
            "Alternative small",
            "Original operator scope",
            "Operator claim, not an answer",
        ] {
            assert!(
                observed.contains(expected),
                "{width}x{height} missing decision detail {expected}"
            );
        }
        assert_eq!(dialog.target().unwrap(), target);
        dialog.handle_key(key(KeyCode::Char('d')));
        let diagnostics = screen_size(&mut dialog, width, height);
        assert!(diagnostics.contains(&target.selection.window_id));
        assert!(diagnostics.contains(target.decision.as_deref().unwrap()));
        let mut observed = diagnostics;
        for _ in 0..40 {
            dialog.handle_key(key(KeyCode::PageDown));
            observed.push_str(&screen_size(&mut dialog, width, height));
        }
        for expected in [
            "Original target hash:",
            "Operator estimate nanoUSD",
            "SDK-recorded estimate nanoUSD: 42",
            "NOT an invoice",
            "Window action observations",
        ] {
            assert!(
                observed.contains(expected),
                "{width}x{height} missing diagnostics {expected}"
            );
        }
        assert!(!observed.contains(['\u{202e}', '\u{7f}', '\u{2066}', '\u{1b}']));
        dialog.handle_key(key(KeyCode::Down));
        assert_eq!(dialog.target().unwrap(), target);
        dialog.handle_key(key(KeyCode::Char('d')));
        assert_eq!(dialog.view, View::Decisions);
        assert_eq!(dialog.target().unwrap(), target);
        dialog.handle_key(key(KeyCode::Esc));
        assert_eq!(dialog.view, View::Summary);
        assert_eq!(dialog.target().unwrap().decision, None);
        assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}

#[test]
fn duplicate_names_and_historical_fallbacks_never_become_operation_keys() {
    let value = report();
    let (mut dialog, calls) = harness(value.clone());
    for row in &value.selections {
        dialog.labels.names.insert(
            (
                row.selection.profile.clone(),
                row.selection.session_id.clone(),
            ),
            Some("Same current name".into()),
        );
    }
    let first = dialog.target().unwrap();
    assert!(screen(&mut dialog).contains("#1 Same current name [alpha"));
    dialog.handle_key(key(KeyCode::Down));
    let second = dialog.target().unwrap();
    assert_ne!(first.selection, second.selection);
    assert!(screen(&mut dialog).contains("#2 Same current name [beta]"));
    dialog.labels.names.clear();
    dialog.labels.historical.insert(second.selection.clone());
    assert!(screen(&mut dialog).contains("Historical session (name unavailable)"));
    dialog.handle_key(key(KeyCode::Char('v')));
    dialog.handle_key(key(KeyCode::Enter));
    let (Request::Annotate(request), reply) = calls.try_recv().unwrap() else {
        panic!("explicit claim expected")
    };
    assert_eq!(request.target, second);
    reply
        .send(Reply::Written(Err(anyhow::anyhow!("uncertain"))))
        .unwrap();
    assert!(dialog.tick());
    dialog.handle_key(key(KeyCode::Esc));
    dialog.handle_key(key(KeyCode::Up));
    assert!(screen(&mut dialog).contains("Session name unavailable"));
    assert_eq!(dialog.target().unwrap(), first);
    assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
}
