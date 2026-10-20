use super::*;

#[test]
#[serial]
fn afk_palette_opens_visible_modal_and_off_does_not_send_terminal_input() {
    let mut env = create_test_env_with_sessions(1);
    crate::session::afk::authority::migration::initialize_test(
        &crate::session::get_app_dir().unwrap(),
    );
    env.view.handle_key(
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        None,
    );
    for ch in "AFK".chars() {
        env.view.handle_key(key(KeyCode::Char(ch)), None);
    }
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.afk_dialog.is_some());
    assert!(env.view.has_dialog());
    assert!(env.view.has_non_live_send_overlay());
    let wait = |view: &mut HomeView| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !view.afk_dialog.as_mut().unwrap().tick() {
            assert!(
                std::time::Instant::now() < deadline,
                "AFK worker did not finish"
            );
            std::thread::yield_now();
        }
    };
    wait(&mut env.view);
    let text = render_home_to_string(&mut env.view, 100, 30);
    for expected in [
        "AFK control-only",
        "Unsupported",
        "Expiry in minutes:",
        "o: off",
        "r: inspect",
        "Questions remain open",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(env.view.handle_dialog_click(0, 0));
    assert!(env.view.handle_key(key(KeyCode::Char('o')), None).is_none());
    wait(&mut env.view);
    let text = render_home_to_string(&mut env.view, 100, 30);
    assert!(text.contains("Requested: off"));
    assert!(env.view.live_send.is_none());
    assert!(env.view.send_message_dialog.is_none());
    env.view.handle_key(key(KeyCode::Esc), None);
    assert!(env.view.afk_dialog.is_none());
}

#[test]
#[serial]
fn batch_review_opens_without_selection_and_captures_input() {
    let mut env = create_test_env_with_sessions(0);
    env.view.handle_key(
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        None,
    );
    for ch in "AFK batch".chars() {
        env.view.handle_key(key(KeyCode::Char(ch)), None);
    }
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.afk_batch_dialog.is_some());
    assert!(env.view.has_non_live_send_overlay());
    assert!(env.view.handle_dialog_click(0, 0));
    for ch in "/private/preview.json".chars() {
        assert!(env.view.handle_key(key(KeyCode::Char(ch)), None).is_none());
    }
    let text = render_home_to_string(&mut env.view, 120, 38);
    assert!(text.contains("/private/preview.json"));
    assert!(text.contains("load without activation"));
    assert!(env.view.live_send.is_none());
    assert!(env.view.send_message_dialog.is_none());
    env.view.handle_key(key(KeyCode::Esc), None);
    assert!(env.view.afk_batch_dialog.is_none());
}

#[test]
#[serial]
fn global_afk_overview_opens_without_a_selected_session_and_captures_input() {
    let mut env = create_test_env_with_sessions(0);
    env.view.handle_key(
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        None,
    );
    for ch in "AFK global overview".chars() {
        env.view.handle_key(key(KeyCode::Char(ch)), None);
    }
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.afk_overview_dialog.is_some());
    assert!(env.view.has_dialog());
    assert!(env.view.has_non_live_send_overlay());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !env.view.afk_overview_dialog.as_mut().unwrap().tick() {
        assert!(
            std::time::Instant::now() < deadline,
            "overview worker did not finish"
        );
        std::thread::yield_now();
    }
    let text = render_home_to_string(&mut env.view, 120, 34);
    assert!(text.contains("Global AFK overview"));
    assert!(text.contains("Shared budgets unobserved"));
    assert!(env.view.handle_dialog_click(0, 0));
    for code in [KeyCode::Char('o'), KeyCode::Enter] {
        assert!(env.view.handle_key(key(code), None).is_none());
    }
    assert!(env.view.live_send.is_none());
    assert!(env.view.send_message_dialog.is_none());
    env.view.handle_key(key(KeyCode::Esc), None);
    assert!(env.view.afk_overview_dialog.is_none());
}

fn afk_paste_stays_in_review(session_count: usize) {
    use crate::{
        session::afk::review::{Report, SelectedWindow, Selection},
        tui::dialogs::afk_overview::AfkOverviewDialog,
    };
    use std::sync::{Arc, Mutex};
    let mut env = create_test_env_with_sessions(session_count);
    env.view.view_mode = ViewMode::Terminal;
    assert_eq!(env.view.selected_session.is_some(), session_count > 0);
    env.view.pending_send_session = Some("existing compose target".into());
    env.view.pending_paste = Some("existing paste".into());
    env.view
        .pending_paste_for_structured_view
        .insert("other".into(), "existing structured text".into());
    let original_target = env.view.pending_send_target.clone();
    let original_structured = env.view.pending_paste_for_structured_view.clone();
    let report = Report {
        version: 1,
        capture_started_at_ms: 1,
        capture_finished_at_ms: 2,
        complete: true,
        selections: vec![SelectedWindow {
            selection: Selection {
                profile: "test".into(),
                session_id: "exact".into(),
                window_id: uuid::Uuid::new_v4().to_string(),
            },
            source_revision: Some(1),
            window: Some(serde_json::json!({"grant_hash":"a".repeat(64),"records":[]})),
            annotations: vec![],
            errors: vec![],
        }],
        shared_usage: vec![],
        observation: "stored only",
        privacy: "private",
    };
    let submissions = Arc::new(Mutex::new(Vec::new()));
    env.view.afk_overview_dialog = Some(AfkOverviewDialog::with_review_for_test(
        report,
        submissions.clone(),
    ));
    let untouched = |view: &HomeView| {
        assert!(
            view.send_message_dialog.is_none(),
            "review paste cannot open runtime composition"
        );
        assert_eq!(
            view.pending_send_session.as_deref(),
            Some("existing compose target")
        );
        assert_eq!(view.pending_send_target, original_target);
        assert_eq!(view.pending_paste.as_deref(), Some("existing paste"));
        assert_eq!(view.pending_paste_for_structured_view, original_structured);
        assert!(view.live_send.is_none());
        assert!(view.live_send_worker.is_none());
    };
    env.view
        .handle_paste("summary without editor: private review text");
    untouched(&env.view);
    for (code, level) in [
        (KeyCode::Enter, "Decisions"),
        (KeyCode::Char('d'), "Diagnostics"),
        (KeyCode::Char('d'), "Decisions"),
        (KeyCode::Esc, "Summary"),
        (KeyCode::Char('d'), "Diagnostics"),
        (KeyCode::Esc, "Summary"),
        (KeyCode::Char('?'), "Summary"),
        (KeyCode::Esc, "Summary"),
    ] {
        assert!(env.view.handle_key(key(code), None).is_none());
        assert!(env.view.afk_overview_dialog.is_some());
        assert!(render_home_to_string(&mut env.view, 132, 44).contains(level));
        env.view
            .handle_paste("decision/diagnostics navigation\nnever runtime input");
        untouched(&env.view);
        assert!(env
            .view
            .afk_overview_dialog
            .as_ref()
            .unwrap()
            .review_editor_text_for_test()
            .is_none());
        assert!(
            submissions.lock().unwrap().is_empty(),
            "view navigation and paste must not submit"
        );
    }
    for mode in ['j', 'n'] {
        assert!(env
            .view
            .handle_key(key(KeyCode::Char(mode)), None)
            .is_none());
        env.view.handle_paste("private\ntext");
        untouched(&env.view);
        assert_eq!(
            env.view
                .afk_overview_dialog
                .as_ref()
                .unwrap()
                .review_editor_text_for_test(),
            Some("private\ntext")
        );
        assert!(
            submissions.lock().unwrap().is_empty(),
            "pasted newline must not submit"
        );
        env.view.handle_paste(&"é".repeat(1024));
        assert_eq!(
            env.view
                .afk_overview_dialog
                .as_ref()
                .unwrap()
                .review_editor_text_for_test(),
            Some("private\ntext"),
            "oversized paste must be rejected atomically"
        );
        assert!(env
            .view
            .handle_key(
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
                None
            )
            .is_none());
        let full = "é".repeat(1024);
        env.view.handle_paste(&full);
        env.view.handle_paste("é");
        assert_eq!(
            env.view
                .afk_overview_dialog
                .as_ref()
                .unwrap()
                .review_editor_text_for_test(),
            Some(full.as_str())
        );
        assert!(env.view.handle_key(key(KeyCode::Esc), None).is_none());
        untouched(&env.view);
    }
    env.view.handle_key(key(KeyCode::Char('n')), None);
    env.view.handle_paste("unchanged\nclaim");
    assert!(submissions.lock().unwrap().is_empty());
    assert!(env.view.handle_key(key(KeyCode::Enter), None).is_none());
    assert_eq!(submissions.lock().unwrap().len(), 1);
    assert_eq!(submissions.lock().unwrap()[0].body, "unchanged\nclaim");
    env.view
        .handle_paste("pending: must not enter runtime or mutate request");
    untouched(&env.view);
    assert!(
        env.view.afk_overview_dialog.as_mut().unwrap().tick(),
        "observe the disconnected reply channel"
    );
    env.view
        .handle_paste("submitted: immutable after disconnect");
    assert_eq!(
        env.view
            .afk_overview_dialog
            .as_ref()
            .unwrap()
            .review_editor_text_for_test(),
        Some("unchanged\nclaim")
    );
    assert_eq!(submissions.lock().unwrap().len(), 1);
    untouched(&env.view);
    env.view.handle_key(key(KeyCode::Esc), None);
    env.view.handle_paste("review without editor");
    untouched(&env.view);
    env.view.handle_key(key(KeyCode::Esc), None);
    assert!(env.view.afk_overview_dialog.is_some());
    env.view.handle_paste("overview without child");
    untouched(&env.view);
}

#[test]
#[serial]
fn afk_review_paste_never_reaches_selected_terminal_composition() {
    afk_paste_stays_in_review(1);
}

#[test]
#[serial]
fn afk_review_paste_never_enters_pending_paste_without_selection() {
    afk_paste_stays_in_review(0);
}
