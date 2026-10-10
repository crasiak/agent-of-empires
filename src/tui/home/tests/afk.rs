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
