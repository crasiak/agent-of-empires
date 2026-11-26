//! Bulk actions on the Marked set, driven through the home view.

use super::*;
use crate::tui::dialogs::ContextMenuAction;
use crate::tui::home::bulk::BulkAction;
use crate::tui::session_feed::SessionFeedResult;

fn mark_all(env: &mut TestEnv) {
    let ids: Vec<String> = env
        .view
        .flat_items
        .iter()
        .filter_map(|item| match item {
            Item::Session { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    env.view.marked.extend(ids);
}

fn applied() -> Result<crate::daemon::RuntimeCursor, String> {
    Ok(crate::daemon::RuntimeCursor {
        epoch: "test".into(),
        revision: 2,
    })
}

/// Publish and apply a runtime snapshot of the view's own rows at the next revision,
/// so replies up to that revision settle, then run the restart poller.
fn settle(env: &mut TestEnv) {
    let sessions = env
        .view
        .instances()
        .map(|instance| {
            crate::server::api::sessions::session_response_from_instance(instance, false)
        })
        .collect();
    let revision = env.view.session_feed.next_revision_for_test();
    env.view
        .session_feed
        .publish_for_test(SessionFeedResult::Snapshot(Arc::new(fixture_snapshot(
            sessions, "test", "test", revision,
        ))));
    env.view.apply_session_feed();
    env.view.apply_restart_results();
}

/// A view with `count` Sessions, all marked, attached to a runtime that accepts commands.
fn marked_env(
    count: usize,
) -> (
    TestEnv,
    impl FnMut(
        Result<crate::daemon::RuntimeCursor, String>,
    ) -> Option<(String, crate::daemon::SessionMutation)>,
) {
    let mut env = create_test_env_with_sessions(count);
    settle(&mut env);
    let respond = env.view.session_feed.command_driver_for_test();
    mark_all(&mut env);
    (env, respond)
}

#[test]
#[serial]
fn marks_are_listed_in_sidebar_order() {
    let mut env = create_test_env_with_sessions(3);
    let first = session_id_at(&env.view, 0).unwrap();
    let third = session_id_at(&env.view, 2).unwrap();
    env.view.marked.insert(third.clone());
    env.view.marked.insert(first.clone());
    assert_eq!(env.view.marked_in_view_order(), vec![first, third]);
}

#[test]
#[serial]
fn restart_key_with_marks_opens_the_bulk_confirm() {
    let mut env = create_test_env_with_sessions(3);
    mark_all(&mut env);

    env.view.handle_key(key(KeyCode::Char('e')), None);

    assert_eq!(
        env.view
            .confirm_dialog
            .as_ref()
            .map(|dialog| dialog.action()),
        Some("bulk_restart"),
        "bulk confirm, not the restart options dialog"
    );
    assert!(env.view.pending_bulk_restart);
    assert_eq!(env.view.bulk_plan(BulkAction::Restart).eligible.len(), 3);
}

#[test]
#[serial]
fn bulk_confirm_lists_skipped_sessions() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    let archived = session_id_at(&env.view, 1).unwrap();
    let title = env.view.get_instance(&archived).unwrap().title.clone();
    env.view.mutate_instance(&archived, |inst| {
        inst.archived_at = Some(chrono::Utc::now())
    });

    env.view.handle_key(key(KeyCode::Char('e')), None);

    assert!(env.view.pending_bulk_restart, "confirm open");
    let plan = env.view.bulk_plan(BulkAction::Restart);
    assert_eq!(plan.eligible.len(), 1);
    assert_eq!(plan.skipped_lines(), format!("  {title}: archived"));
}

#[test]
#[serial]
fn nothing_eligible_explains_instead_of_confirming() {
    let mut env = create_test_env_with_sessions(1);
    mark_all(&mut env);
    let id = session_id_at(&env.view, 0).unwrap();
    env.view.restart_in_flight.insert(id);

    env.view.handle_key(key(KeyCode::Char('e')), None);

    assert!(env.view.confirm_dialog.is_none());
    assert!(!env.view.pending_bulk_restart);
    let dialog = env.view.info_dialog.as_ref().expect("explanation");
    assert_eq!(dialog.title(), "Nothing to Restart");
    assert!(dialog.message().contains("session0: already restarting"));
}

#[test]
#[serial]
fn cancelling_the_confirm_keeps_marks_and_drops_the_plan() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    env.view.handle_key(key(KeyCode::Char('e')), None);
    env.view.handle_key(key(KeyCode::Esc), None);

    assert!(!env.view.pending_bulk_restart);
    assert!(env.view.bulk_restart.is_none());
    assert_eq!(env.view.marked.len(), 2);
}

#[test]
#[serial]
fn confirmed_bulk_restart_runs_two_at_a_time_and_clears_marks() {
    let (mut env, mut respond) = marked_env(3);

    env.view.handle_key(key(KeyCode::Char('e')), None);
    env.view.handle_key(key(KeyCode::Char('y')), None);

    assert!(
        env.view.marked.is_empty(),
        "marks clear once the action is submitted"
    );
    assert_eq!(env.view.restart_in_flight.len(), 2, "at most two in flight");

    // Finish the first two; the third is submitted on the next tick.
    assert!(respond(applied()).is_some());
    assert!(respond(applied()).is_some());
    assert!(
        respond(applied()).is_none(),
        "the third has not been submitted yet"
    );
    settle(&mut env);
    assert!(env.view.tick_bulk_restart());
    assert!(respond(applied()).is_some_and(|(_, mutation)| matches!(
        mutation,
        crate::daemon::SessionMutation::Restart(body)
            if body.profile.is_none() && body.tool.is_none() && body.extra_args.is_none()
    )));

    settle(&mut env);
    assert!(env.view.tick_bulk_restart());
    assert!(
        env.view.bulk_restart.is_none(),
        "the run ends when the queue drains"
    );
    assert_eq!(env.view.status_flash_text(), Some("Restarted 3 sessions"));
    assert!(env.view.info_dialog.is_none());
}

#[test]
#[serial]
fn a_refused_restart_is_reported_in_the_summary() {
    let (mut env, mut respond) = marked_env(2);

    env.view.handle_key(key(KeyCode::Char('e')), None);
    env.view.handle_key(key(KeyCode::Char('y')), None);
    assert!(respond(applied()).is_some());
    let (refused, _) = respond(Err("lifecycle_busy".into())).expect("second restart submitted");
    let refused_title = env.view.get_instance(&refused).unwrap().title.clone();
    settle(&mut env);
    env.view.tick_bulk_restart();

    assert!(env.view.bulk_restart.is_none());
    let dialog = env.view.info_dialog.as_ref().expect("failures are listed");
    assert_eq!(dialog.title(), "Bulk Restart");
    assert!(
        dialog
            .message()
            .contains("Restarted 1 session. 1 did not restart"),
        "{}",
        dialog.message()
    );
    assert!(
        dialog
            .message()
            .contains(&format!("{refused_title}: lifecycle_busy")),
        "{}",
        dialog.message()
    );
}

#[test]
#[serial]
fn idle_only_leaves_busy_sessions_running() {
    let (mut env, mut respond) = marked_env(2);
    let busy = session_id_at(&env.view, 0).unwrap();
    let idle = session_id_at(&env.view, 1).unwrap();
    env.view
        .mutate_instance(&busy, |inst| inst.status = Status::Running);
    env.view
        .mutate_instance(&idle, |inst| inst.status = Status::Idle);

    env.view.handle_key(key(KeyCode::Char('e')), None);
    assert!(env.view.pending_bulk_restart, "bulk confirm");
    let plan = env.view.bulk_plan(BulkAction::Restart);
    assert_eq!(plan.eligible.iter().filter(|c| c.is_busy()).count(), 1);
    env.view.confirm_checked = vec![crate::tui::home::bulk_restart::BULK_RESTART_IDLE_ONLY];
    env.view.confirm_dialog = None;
    let _ = env.view.dispatch_confirm_submit("bulk_restart");

    assert_eq!(
        respond(applied()).map(|(id, _)| id),
        Some(idle.clone()),
        "only the idle Session restarts"
    );
    assert!(respond(applied()).is_none());
    assert!(!env.view.restart_in_flight.contains(&busy));
}

#[test]
#[serial]
fn a_session_restarted_moments_ago_waits_instead_of_failing() {
    let (mut env, mut respond) = marked_env(1);
    let id = session_id_at(&env.view, 0).unwrap();
    // A single restart just finished: it left the pending set but is inside the
    // key-repeat cooldown, so an immediate second restart would be skipped.
    env.view
        .restart_cooldown_at
        .insert(id.clone(), std::time::Instant::now());

    env.view.handle_key(key(KeyCode::Char('e')), None);
    env.view.handle_key(key(KeyCode::Char('y')), None);
    assert!(respond(applied()).is_none(), "held back by the cooldown");
    env.view.tick_bulk_restart();
    assert!(env.view.bulk_restart.is_some(), "still queued, not failed");
    assert!(env.view.info_dialog.is_none());

    env.view.restart_cooldown_at.clear();
    env.view.tick_bulk_restart();
    assert_eq!(respond(applied()).map(|(target, _)| target), Some(id));
}

fn make_sendable(env: &mut TestEnv) {
    let ids: Vec<String> = env.view.marked.iter().cloned().collect();
    for id in ids {
        env.view.mutate_instance(&id, |inst| {
            inst.status = Status::Idle;
            inst.agent_pane.state = crate::session::PanePresence::Alive;
        });
    }
}

#[test]
#[serial]
fn send_key_with_marks_opens_one_dialog_for_all_eligible_sessions() {
    let mut env = create_test_env_with_sessions(3);
    mark_all(&mut env);
    make_sendable(&mut env);
    let waiting = env.view.marked_in_view_order()[0].clone();
    env.view
        .mutate_instance(&waiting, |inst| inst.status = Status::Waiting);

    env.view.handle_key(key(KeyCode::Char('m')), None);

    assert!(env.view.send_message_dialog.is_some());
    let targets = env.view.pending_bulk_send.as_ref().expect("bulk targets");
    assert_eq!(targets.len(), 2, "the waiting session is skipped");
    assert!(targets.iter().all(|target| target.id != waiting));
    assert!(
        env.view.pending_send_session.is_none(),
        "the single-session send slot is not used"
    );
}

#[test]
#[serial]
fn cancelling_the_bulk_send_dialog_keeps_marks() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    make_sendable(&mut env);
    env.view.handle_key(key(KeyCode::Char('m')), None);
    env.view.handle_key(key(KeyCode::Esc), None);

    assert!(env.view.pending_bulk_send.is_none());
    assert_eq!(env.view.marked.len(), 2);
}

#[test]
#[serial]
fn nothing_sendable_explains_why() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    let ids: Vec<String> = env.view.marked.iter().cloned().collect();
    for id in ids {
        env.view.mutate_instance(&id, |inst| {
            inst.agent_pane.state = crate::session::PanePresence::Dead;
        });
    }

    env.view.handle_key(key(KeyCode::Char('m')), None);

    assert!(env.view.send_message_dialog.is_none());
    assert!(env.view.info_dialog.is_some());
}

#[test]
#[serial]
fn a_finished_bulk_send_reports_the_count() {
    let mut env = create_test_env_with_sessions(1);
    env.view.bulk_send_worker = Some(
        crate::tui::home::bulk_send::BulkSendWorker::seeded_for_test(
            crate::tui::home::bulk_send::BulkSendResult {
                sent: 3,
                failed: Vec::new(),
            },
        ),
    );

    assert!(env.view.tick_bulk_send());
    assert!(env.view.bulk_send_worker.is_none());
    assert!(env.view.info_dialog.is_none());
}

#[test]
#[serial]
fn a_bulk_send_with_failures_lists_them() {
    let mut env = create_test_env_with_sessions(1);
    env.view.bulk_send_worker = Some(
        crate::tui::home::bulk_send::BulkSendWorker::seeded_for_test(
            crate::tui::home::bulk_send::BulkSendResult {
                sent: 1,
                failed: vec![("Vikings".to_string(), "not running".to_string())],
            },
        ),
    );

    assert!(env.view.tick_bulk_send());
    assert!(env.view.info_dialog.is_some());
}

fn shift_k() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)
}

#[test]
#[serial]
fn shift_k_opens_the_bulk_menu_only_with_marks() {
    let mut env = create_test_env_with_sessions(2);
    env.view.handle_key(shift_k(), None);
    assert!(env.view.context_menu.is_none());
    assert!(
        env.view.status_flash.is_some(),
        "explains that nothing is marked"
    );

    mark_all(&mut env);
    env.view.handle_key(shift_k(), None);
    assert!(env.view.context_menu.is_some());
}

#[test]
#[serial]
fn shift_k_works_in_strict_mode() {
    let mut env = create_test_env_with_sessions(2);
    env.view.strict_hotkeys = true;
    mark_all(&mut env);
    env.view.handle_key(shift_k(), None);
    assert!(env.view.context_menu.is_some());
}

#[test]
#[serial]
fn the_bulk_menu_restart_entry_opens_the_confirm() {
    // Menu without Breakout entries: Send, Restart, Show skipped, Clear marks.
    let _forced = ForcedSupport::set(false);
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    env.view.handle_key(shift_k(), None);
    env.view.handle_key(key(KeyCode::Down), None);
    env.view.handle_key(key(KeyCode::Enter), None);

    assert!(env.view.context_menu.is_none());
    assert!(env.view.pending_bulk_restart);
}

#[test]
#[serial]
fn the_bulk_menu_can_clear_marks() {
    // Fixed menu: Send, Restart, Show skipped, Clear marks (no Breakout entries).
    let _forced = ForcedSupport::set(false);
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    env.view.handle_key(shift_k(), None);
    for _ in 0..3 {
        env.view.handle_key(key(KeyCode::Down), None);
    }
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.marked.is_empty());
}

#[test]
#[serial]
fn the_bulk_menu_lists_what_would_be_skipped() {
    // Fixed menu: Send, Restart, Show skipped, Clear marks (no Breakout entries).
    let _forced = ForcedSupport::set(false);
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    env.view.handle_key(shift_k(), None);
    for _ in 0..2 {
        env.view.handle_key(key(KeyCode::Down), None);
    }
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.context_menu.is_none());
    assert!(env.view.info_dialog.is_some());
    assert_eq!(env.view.marked.len(), 2, "listing skips keeps the Marks");
}

fn sidebar_area(env: &mut TestEnv) {
    env.view.list_inner_area = ratatui::layout::Rect::new(1, 1, 28, 10);
    env.view.list_area = ratatui::layout::Rect::new(0, 0, 30, 12);
}

#[test]
#[serial]
fn right_click_on_a_marked_row_opens_the_bulk_menu() {
    let mut env = create_test_env_with_sessions(3);
    sidebar_area(&mut env);
    let id = session_id_at(&env.view, 2).expect("a Session row");
    env.view.marked.insert(id);

    assert!(env.view.handle_right_click(5, 3));
    let menu = env.view.context_menu.as_ref().expect("a menu opens");
    assert_eq!(menu.selected_action(), ContextMenuAction::BulkSend);
}

#[test]
#[serial]
fn right_click_on_an_unmarked_row_keeps_the_session_menu() {
    let mut env = create_test_env_with_sessions(3);
    sidebar_area(&mut env);
    let id = session_id_at(&env.view, 0).expect("a Session row");
    env.view.marked.insert(id);

    assert!(env.view.handle_right_click(5, 3));
    let menu = env.view.context_menu.as_ref().expect("a menu opens");
    assert_eq!(menu.selected_action(), ContextMenuAction::NewFromSelection);
}

#[test]
#[serial]
fn right_click_on_a_group_header_keeps_its_menu_with_marks() {
    let mut env = create_test_env_with_groups();
    sidebar_area(&mut env);
    mark_all(&mut env);
    assert!(!env.view.marked.is_empty());
    let group_idx = env
        .view
        .flat_items
        .iter()
        .position(|item| matches!(item, Item::Group { .. }))
        .expect("a Group row");

    assert!(env
        .view
        .handle_right_click(5, env.view.list_inner_area.y + group_idx as u16));
    let menu = env.view.context_menu.as_ref().expect("a menu opens");
    assert_eq!(menu.selected_action(), ContextMenuAction::NewFromSelection);
}

/// Three Sessions: the cursor sits on a trashed first row, the other two are marked
/// and can take a message.
fn trashed_cursor_env() -> TestEnv {
    let mut env = create_test_env_with_sessions(3);
    let trashed = session_id_at(&env.view, 0).expect("a Session row");
    env.view.marked.extend([
        session_id_at(&env.view, 1).expect("a Session row"),
        session_id_at(&env.view, 2).expect("a Session row"),
    ]);
    make_sendable(&mut env);
    env.view
        .mutate_instance(&trashed, |inst| inst.trashed_at = Some(chrono::Utc::now()));
    env.view.select_session_by_id(&trashed);
    assert!(env.view.selected_is_trashed());
    env
}

#[test]
#[serial]
fn the_bulk_menu_survives_a_trashed_cursor_row() {
    // Fixed menu: Send, Restart, Show skipped, Clear marks (no Breakout entries).
    let _forced = ForcedSupport::set(false);
    let mut env = trashed_cursor_env();
    env.view.handle_key(shift_k(), None);
    assert!(env.view.context_menu.is_some());

    env.view.handle_key(key(KeyCode::Down), None);
    assert!(
        env.view.context_menu.is_some(),
        "the next key keeps the menu"
    );
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.pending_bulk_restart);
}

#[test]
#[serial]
fn the_bulk_send_dialog_survives_a_trashed_cursor_row() {
    // Fixed menu: Send, Restart, Show skipped, Clear marks (no Breakout entries).
    let _forced = ForcedSupport::set(false);
    let mut env = trashed_cursor_env();
    env.view.handle_key(shift_k(), None);
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.send_message_dialog.is_some());

    env.view.handle_key(key(KeyCode::Char('h')), None);
    assert!(
        env.view.send_message_dialog.is_some(),
        "typing keeps the dialog"
    );
    let targets = env.view.pending_bulk_send.as_ref().expect("bulk targets");
    assert_eq!(targets.len(), 2);
}

#[test]
#[serial]
fn the_send_key_with_marks_survives_a_trashed_cursor_row() {
    let mut env = trashed_cursor_env();
    env.view.handle_key(key(KeyCode::Char('m')), None);
    env.view.handle_key(key(KeyCode::Char('h')), None);
    assert!(env.view.send_message_dialog.is_some());
    assert_eq!(env.view.pending_bulk_send.as_ref().map(Vec::len), Some(2));
}

#[test]
#[serial]
fn the_bulk_restart_confirm_survives_a_trashed_cursor_row() {
    let mut env = trashed_cursor_env();
    env.view.handle_key(key(KeyCode::Char('e')), None);
    assert!(env.view.pending_bulk_restart);

    env.view.handle_key(key(KeyCode::Tab), None);
    assert!(env.view.confirm_dialog.is_some());
    assert!(env.view.pending_bulk_restart);
}

#[test]
#[serial]
fn cursor_bound_surfaces_on_a_trashed_row_still_close_with_marks() {
    let mut env = trashed_cursor_env();
    env.view.context_menu = Some(crate::tui::dialogs::ContextMenuDialog::for_session(
        (1, 1),
        false,
        None,
        None,
        false,
        None,
    ));
    env.view.enforce_trash_read_only();
    assert!(env.view.context_menu.is_none());

    // A single-Session send dialog never keeps, or inherits, bulk targets.
    let trashed = env.view.selected_session.clone().expect("a selection");
    env.view.pending_send_session = Some(trashed);
    env.view.send_message_dialog = Some(crate::tui::dialogs::SendMessageDialog::new("one"));
    env.view.enforce_trash_read_only();
    assert!(env.view.send_message_dialog.is_none());
    assert!(env.view.pending_send_session.is_none());
    assert!(env.view.pending_bulk_send.is_none());
}

fn bulk_targets(env: &TestEnv) -> Vec<crate::tui::home::bulk_send::BulkSendTarget> {
    env.view
        .pending_bulk_send
        .as_ref()
        .expect("bulk targets")
        .iter()
        .map(|target| crate::tui::home::bulk_send::BulkSendTarget {
            id: target.id.clone(),
            title: target.title.clone(),
            structured: target.structured,
        })
        .collect()
}

#[test]
#[serial]
fn submit_with_every_target_still_eligible_builds_a_request_for_all() {
    let mut env = create_test_env_with_sessions(3);
    mark_all(&mut env);
    make_sendable(&mut env);
    env.view.handle_key(key(KeyCode::Char('m')), None);

    let request = env
        .view
        .prepare_bulk_send(bulk_targets(&env), "hello".to_string());

    assert_eq!(request.targets.len(), 3);
    assert!(request.skipped.is_empty());
    assert_eq!(request.message, "hello");
}

#[test]
#[serial]
fn a_target_that_starts_waiting_after_the_dialog_opened_is_dropped_with_its_reason() {
    let mut env = create_test_env_with_sessions(3);
    mark_all(&mut env);
    make_sendable(&mut env);
    env.view.handle_key(key(KeyCode::Char('m')), None);
    let targets = bulk_targets(&env);
    let waiting = targets[1].id.clone();
    let title = targets[1].title.clone();
    env.view
        .mutate_instance(&waiting, |inst| inst.status = Status::Waiting);

    let request = env.view.prepare_bulk_send(targets, "hello".to_string());

    assert_eq!(request.targets.len(), 2);
    assert!(request.targets.iter().all(|target| target.id != waiting));
    assert_eq!(
        request.skipped,
        vec![(title, "waiting on a prompt".to_string())]
    );
}

#[test]
#[serial]
fn a_target_unmarked_after_the_dialog_opened_is_dropped_as_no_longer_marked() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    make_sendable(&mut env);
    env.view.handle_key(key(KeyCode::Char('m')), None);
    let targets = bulk_targets(&env);
    env.view.marked.remove(&targets[0].id);
    let title = targets[0].title.clone();

    let request = env.view.prepare_bulk_send(targets, "hello".to_string());

    assert_eq!(request.targets.len(), 1);
    assert_eq!(
        request.skipped,
        vec![(title, "no longer marked".to_string())]
    );
}

#[test]
#[serial]
fn submit_with_no_eligible_target_left_sends_nothing_and_says_so() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    make_sendable(&mut env);
    env.view.handle_key(key(KeyCode::Char('m')), None);
    let ids: Vec<String> = env.view.marked.iter().cloned().collect();
    for id in ids {
        env.view
            .mutate_instance(&id, |inst| inst.status = Status::Waiting);
    }
    // A stale single-Session slot must not win over the bulk slot.
    env.view.pending_send_session = Some(session_id_at(&env.view, 0).unwrap());

    env.view.handle_key(key(KeyCode::Char('h')), None);
    let action = env.view.handle_key(key(KeyCode::Enter), None);

    assert!(action.is_none(), "the single slot was not used");
    assert!(env.view.bulk_send_worker.is_none(), "nothing was sent");
    assert!(env.view.pending_bulk_send.is_none());
    assert_eq!(env.view.marked.len(), 2, "Marks stay when nothing was sent");
    let info = env.view.info_dialog.as_ref().expect("explains why");
    assert_eq!(info.title(), "Nothing Sent");
    assert!(info.message().contains("waiting on a prompt"));
}

#[test]
#[serial]
fn show_skipped_uses_the_singular_for_one_marked_session() {
    let mut env = create_test_env_with_sessions(1);
    mark_all(&mut env);
    make_sendable(&mut env);

    env.view.show_bulk_skips();

    let info = env.view.info_dialog.as_ref().expect("summary");
    assert!(info.message().contains("Send: all 1 marked session"));
    assert!(!info.message().contains("1 marked sessions"));
}

#[test]
#[serial]
fn an_unknown_session_counts_as_busy_in_the_restart_confirm() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    let ids = env.view.marked_in_view_order();
    env.view
        .mutate_instance(&ids[0], |inst| inst.status = Status::Unknown);
    env.view
        .mutate_instance(&ids[1], |inst| inst.status = Status::Idle);

    env.view.handle_key(key(KeyCode::Char('e')), None);

    let dialog = env.view.confirm_dialog.as_ref().expect("confirm");
    assert_eq!(
        dialog.checkbox_labels_for_test(),
        ["Idle only (leave 1 busy running)"],
        "the Unknown session is the busy one"
    );
}

fn breakout_outcome(opened: usize, note: Option<&str>) -> crate::breakout::BreakoutOutcome {
    crate::breakout::BreakoutOutcome {
        opened,
        tiled: note.is_none(),
        titles: (0..opened).map(|i| format!("t{i}")).collect(),
        note: note.map(str::to_string),
    }
}

#[test]
#[serial]
fn a_finished_breakout_records_its_windows() {
    let mut env = create_test_env_with_sessions(2);
    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_job = Some(rx);
    tx.send(Ok(breakout_outcome(2, None))).unwrap();

    assert!(env.view.tick_breakout());
    assert_eq!(env.view.breakout_titles, ["t0", "t1"]);
    assert!(env.view.breakout_job.is_none());
    assert!(env.view.info_dialog.is_none());
}

#[test]
#[serial]
fn an_untiled_breakout_shows_the_note() {
    let mut env = create_test_env_with_sessions(1);
    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_job = Some(rx);
    tx.send(Ok(breakout_outcome(1, Some("not tiled: grant access"))))
        .unwrap();

    assert!(env.view.tick_breakout());
    assert!(env.view.info_dialog.is_some());
    assert_eq!(
        env.view.breakout_titles.len(),
        1,
        "the windows are still tracked"
    );
}

#[test]
#[serial]
fn a_failed_breakout_is_reported() {
    let mut env = create_test_env_with_sessions(1);
    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_job = Some(rx);
    tx.send(Err("Ghostty is not installed".to_string()))
        .unwrap();

    assert!(env.view.tick_breakout());
    assert!(env.view.info_dialog.is_some());
    assert!(env.view.breakout_titles.is_empty());
}

#[test]
#[serial]
fn more_than_nine_sessions_is_refused_and_seven_asks_first() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(10);
    mark_all(&mut env);
    make_sendable(&mut env);
    env.view.start_breakout();
    assert!(env.view.info_dialog.is_some(), "ten is refused");
    assert!(!env.view.pending_breakout);

    env.view.info_dialog = None;
    let extra: Vec<String> = env
        .view
        .marked_in_view_order()
        .into_iter()
        .skip(7)
        .collect();
    for id in extra {
        env.view.marked.remove(&id);
    }
    env.view.start_breakout();
    assert!(
        env.view.confirm_dialog.is_some(),
        "seven asks for confirmation"
    );
    assert!(env.view.pending_breakout);
    assert!(
        env.view.breakout_job.is_none(),
        "nothing opens before confirm"
    );
}

#[test]
#[serial]
fn the_bulk_menu_offers_close_wherever_breakout_works() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    {
        let _forced = ForcedSupport::set(false);
        env.view.handle_key(shift_k(), None);
        let menu = env.view.context_menu.as_ref().unwrap();
        assert!(menu.is_marks_menu());
        assert!(!menu.items_contain(ContextMenuAction::CloseBreakout));
        assert!(
            !menu.items_contain(ContextMenuAction::BulkBreakout),
            "Break Out is hidden where Breakout is unsupported and no agent applies"
        );
        assert!(menu.items_contain(ContextMenuAction::BulkSend));
    }

    env.view.context_menu = None;
    let _forced = ForcedSupport::on();
    assert!(env.view.breakout_titles.is_empty());
    env.view.handle_key(shift_k(), None);
    let menu = env.view.context_menu.as_ref().unwrap();
    assert!(
        menu.is_marks_menu(),
        "still a bulk menu with the close entry"
    );
    assert!(
        menu.items_contain(ContextMenuAction::CloseBreakout),
        "offered with no tracked windows, since the sweep finds untracked ones"
    );
}

#[test]
#[serial]
fn a_scoped_tui_runs_no_orphan_sweep_before_a_local_breakout() {
    let _forced = ForcedSupport::on();
    crate::tui::home::breakout::test_support::OPENED_FOR.with(|o| *o.borrow_mut() = None);
    let mut env = create_test_env_with_sessions(2);
    env.view.active_profile = Some("work".to_string());
    break_out_all(&mut env);
    let opened = opened_for().expect("opened");
    assert_eq!(
        opened.orphan_keep, None,
        "a scoped TUI cannot see other profiles"
    );
}

#[test]
#[serial]
fn an_unscoped_tui_sweeps_orphans_keeping_every_loaded_session() {
    let _forced = ForcedSupport::on();
    crate::tui::home::breakout::test_support::OPENED_FOR.with(|o| *o.borrow_mut() = None);
    let mut env = create_test_env_with_sessions(2);
    env.view.active_profile = None;
    let mut expected: Vec<String> = env
        .view
        .instances
        .keys()
        .map(|id| crate::breakout::id_prefix(id))
        .collect();
    expected.sort();
    break_out_all(&mut env);
    let mut keep = opened_for().expect("opened").orphan_keep.expect("sweeps");
    keep.sort();
    assert_eq!(keep, expected);
    assert_eq!(keep.len(), 2);
}

/// Forces `supported_here` for one test and restores it even when an assertion fails.
struct ForcedSupport;

impl ForcedSupport {
    fn on() -> Self {
        Self::set(true)
    }

    fn set(supported: bool) -> Self {
        crate::breakout::test_support::FORCED.with(|f| f.set(Some(supported)));
        Self
    }
}

impl Drop for ForcedSupport {
    fn drop(&mut self) {
        crate::breakout::test_support::FORCED.with(|f| f.set(None));
    }
}

#[test]
#[serial]
fn a_failed_breakout_keeps_the_marks_and_a_finished_one_clears_them() {
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_job = Some(rx);
    tx.send(Err("boom".to_string())).unwrap();
    assert!(env.view.tick_breakout());
    assert_eq!(env.view.marked.len(), 2, "a failure keeps the Marked set");

    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_job = Some(rx);
    tx.send(Ok(breakout_outcome(2, None))).unwrap();
    assert!(env.view.tick_breakout());
    assert!(env.view.marked.is_empty());
}

#[test]
#[serial]
fn a_disconnected_breakout_worker_keeps_the_marks() {
    let mut env = create_test_env_with_sessions(1);
    mark_all(&mut env);
    let (tx, rx) = std::sync::mpsc::channel::<Result<crate::breakout::BreakoutOutcome, String>>();
    drop(tx);
    env.view.breakout_job = Some(rx);
    assert!(env.view.tick_breakout());
    assert!(env.view.info_dialog.is_some());
    assert_eq!(env.view.marked.len(), 1);
}

#[test]
#[serial]
fn closing_runs_as_a_job_and_clears_titles_on_success() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(1);
    env.view.breakout_titles = vec!["t0".into()];
    env.view.close_breakout();
    assert!(env.view.breakout_close_job.is_some());
    assert_eq!(
        env.view.breakout_titles,
        ["t0"],
        "kept until the close lands"
    );

    env.view.close_breakout();
    assert!(
        env.view.breakout_close_job.is_some(),
        "a second close is refused"
    );

    assert!(env.view.tick_breakout());
    assert!(env.view.breakout_close_job.is_none());
    assert!(env.view.breakout_titles.is_empty());
}

#[test]
#[serial]
fn a_failed_close_keeps_the_titles_for_a_retry() {
    let mut env = create_test_env_with_sessions(1);
    env.view.breakout_titles = vec!["t0".into()];
    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_close_job = Some(rx);
    tx.send(Err("Ghostty hung".to_string())).unwrap();
    assert!(env.view.tick_breakout());
    assert!(env.view.info_dialog.is_some());
    assert_eq!(env.view.breakout_titles, ["t0"]);
    assert!(env.view.breakout_close_job.is_none());
}

#[test]
#[serial]
fn a_running_close_refuses_a_new_breakout() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    make_sendable(&mut env);
    let (_tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_close_job = Some(rx);
    env.view.start_breakout();
    assert!(env.view.breakout_job.is_none());
    assert_eq!(env.view.marked.len(), 2);
}

#[test]
#[serial]
fn run_breakout_builds_one_job_and_waits_for_success_to_clear_marks() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    make_sendable(&mut env);
    env.view.start_breakout();
    assert!(env.view.breakout_job.is_some());
    assert_eq!(env.view.marked.len(), 2, "Marks stay while the job runs");
    assert!(env.view.tick_breakout());
    assert_eq!(env.view.breakout_titles.len(), 2);
    assert!(env.view.marked.is_empty());
}

fn open_breakout_confirm(env: &mut TestEnv) {
    mark_all(env);
    make_sendable(env);
    let extra: Vec<String> = env
        .view
        .marked_in_view_order()
        .into_iter()
        .skip(7)
        .collect();
    for id in extra {
        env.view.marked.remove(&id);
    }
    env.view.start_breakout();
    assert!(env.view.pending_breakout);
}

#[test]
#[serial]
fn cancelling_the_breakout_confirm_resets_the_flag() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(8);
    open_breakout_confirm(&mut env);
    env.view.handle_key(key(KeyCode::Esc), None);
    assert!(!env.view.pending_breakout);
    assert!(env.view.breakout_job.is_none());
    assert_eq!(env.view.marked.len(), 7);
}

#[test]
#[serial]
fn submitting_the_breakout_confirm_resets_the_flag_and_opens() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(8);
    open_breakout_confirm(&mut env);
    env.view.handle_key(key(KeyCode::Char('y')), None);
    assert!(!env.view.pending_breakout);
    assert!(env.view.breakout_job.is_some());
}

/// Open a finished Breakout of every Session through the real request path.
fn break_out_all(env: &mut TestEnv) -> Vec<String> {
    mark_all(env);
    make_sendable(env);
    let ids = env.view.marked_in_view_order();
    env.view.start_breakout();
    assert!(env.view.tick_breakout());
    ids
}

#[test]
#[serial]
fn a_finished_breakout_marks_its_sessions_until_closed() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(3);
    let ids = break_out_all(&mut env);
    assert_eq!(env.view.breakout_titles.len(), 3);
    assert!(env
        .view
        .breakout_titles
        .iter()
        .all(|t| t.starts_with(crate::breakout::TITLE_PREFIX)));
    for id in &ids {
        assert!(env.view.is_broken_out(id));
    }
    assert!(!env.view.is_broken_out("not-a-session"));

    env.view.close_breakout();
    assert!(env.view.tick_breakout());
    for id in &ids {
        assert!(!env.view.is_broken_out(id));
    }
}

#[test]
#[serial]
fn restarting_home_rediscovers_existing_local_breakout_windows() {
    use crate::tui::home::breakout::test_support::DISCOVERY;
    use crate::tui::home::icons::ICON_BREAKOUT;

    let _forced = ForcedSupport::on();
    let env = create_test_env_with_sessions(4);
    let ids: Vec<_> = (0..4).map(|i| env.view.instance_at(i).id.clone()).collect();
    let separate = crate::breakout::window_title(None, "existing, 'window'", &ids[0]);
    let splits = crate::breakout::splits_title(None, ids[1..3].iter().map(String::as_str));
    let remote = crate::breakout::window_title(Some("other-host"), "remote", &ids[3]);
    let (tx, rx) = std::sync::mpsc::channel();
    DISCOVERY.with(|job| *job.borrow_mut() = Some(rx));
    let mut restarted = HomeView::new_for_test(
        Some("test".into()),
        AvailableTools::with_tools(&["claude"]),
        crate::file_watch::FileWatchService::noop(),
    )
    .unwrap();
    restarted.group_by = crate::session::config::GroupByMode::Manual;
    restarted.flat_items = restarted.build_flat_items();
    assert!(restarted.breakout_discovery_job.is_some());
    assert!(!restarted.is_broken_out(&ids[0]));
    let cursor = restarted.cursor;
    let profile = restarted.active_profile.clone();
    restarted.marked.insert(ids[3].clone());
    // Native inventory may arrive before the runtime's first session snapshot.
    let instances = std::mem::take(&mut restarted.instances);
    tx.send(Ok(vec![
        separate.clone(),
        splits.clone(),
        remote,
        separate.clone(),
        "ordinary Ghostty window".into(),
    ]))
    .unwrap();
    restarted.tick_breakout();
    restarted.instances = instances;

    assert!(
        restarted.is_broken_out(&ids[0]),
        "existing window must regain its marker"
    );
    assert!(restarted.is_broken_out(&ids[1]));
    assert!(restarted.is_broken_out(&ids[2]));
    assert!(
        !restarted.is_broken_out(&ids[3]),
        "another host is not local"
    );
    assert_eq!(restarted.breakout_titles, [separate, splits]);
    let row = restarted
        .flat_items
        .iter()
        .find(|item| {
            matches!(
                item, Item::Session { id, .. } if id == &ids[0]
            )
        })
        .unwrap();
    assert!(rendered_row_text(&restarted, row).starts_with(ICON_BREAKOUT));
    assert_eq!(restarted.cursor, cursor);
    assert_eq!(restarted.active_profile, profile);
    assert!(restarted.marked.contains(&ids[3]));
    assert!(restarted.info_dialog.is_none());
    assert!(restarted.breakout_job.is_none());
    assert!(restarted.breakout_close_job.is_none());
    assert!(restarted.breakout_session_close_job.is_none());
    assert!(restarted.breakout_close_queue.is_empty());
    assert!(restarted.breakout_discovery_job.is_none());
}

#[test]
#[serial]
fn startup_discovery_serializes_window_actions_and_recovers_after_failure() {
    let _forced = ForcedSupport::on();
    for reply in [Some(Ok(Vec::new())), Some(Err("denied".into())), None] {
        let mut env = create_test_env_with_sessions(1);
        mark_all(&mut env);
        make_sendable(&mut env);
        let marks = env.view.marked.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        env.view.breakout_discovery_job = Some(rx);
        env.view.info_dialog = Some(InfoDialog::new("Keep this", "unrelated dialog"));
        env.view.start_breakout();
        env.view.close_breakout();
        assert!(env.view.breakout_job.is_none());
        assert!(env.view.breakout_close_job.is_none());
        assert!(
            !env.view.tick_breakout(),
            "an empty channel is not completion"
        );
        if let Some(reply) = reply {
            tx.send(reply).unwrap();
        }
        drop(tx);
        assert!(env.view.tick_breakout());
        assert!(env.view.breakout_discovery_job.is_none());
        assert!(env.view.breakout_titles.is_empty());
        assert_eq!(env.view.info_dialog.as_ref().unwrap().title(), "Keep this");
        assert_eq!(env.view.marked, marks);
        env.view.info_dialog = None;
        env.view.start_breakout();
        assert!(
            env.view.breakout_job.is_some(),
            "discovery cannot leave Breakout busy"
        );
    }
}

#[test]
#[serial]
fn stopping_during_discovery_closes_only_the_sessions_separate_window() {
    use crate::tui::home::breakout::test_support::CLOSED;
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(3);
    let ids: Vec<_> = (0..3).map(|i| env.view.instance_at(i).id.clone()).collect();
    let separate = crate::breakout::window_title(None, "existing", &ids[0]);
    let splits = crate::breakout::splits_title(None, ids[1..3].iter().map(String::as_str));
    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_discovery_job = Some(rx);
    CLOSED.with(|closed| closed.borrow_mut().clear());
    let _respond = env.view.session_feed.command_driver_for_test();
    assert!(env.view.submit_daemon_stop(&ids[0]).unwrap());
    assert!(env.view.submit_daemon_stop(&ids[1]).unwrap());
    assert!(env.view.breakout_session_close_job.is_none());
    tx.send(Ok(vec![separate.clone(), splits.clone()])).unwrap();
    env.view.tick_breakout();
    assert!(!env.view.is_broken_out(&ids[0]));
    assert!(
        env.view.is_broken_out(&ids[1]),
        "shared splits retain their marker"
    );
    assert!(env.view.is_broken_out(&ids[2]));
    assert_eq!(env.view.breakout_titles, [splits]);
    CLOSED.with(|closed| assert_eq!(*closed.borrow(), [separate]));
    assert!(env.view.breakout_discovery_closes.is_empty());
}

#[test]
#[serial]
fn close_breakout_windows_sweeps_even_with_nothing_tracked() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(1);
    assert!(env.view.breakout_titles.is_empty());
    env.view.close_breakout();
    assert!(
        env.view.breakout_close_job.is_some(),
        "the sweep finds windows this TUI does not track"
    );
    assert!(env.view.tick_breakout());
    assert!(env.view.breakout_close_job.is_none());
}

#[test]
#[serial]
fn close_breakout_windows_is_refused_where_breakout_does_not_work() {
    let _forced = ForcedSupport::set(false);
    let mut env = create_test_env_with_sessions(1);
    env.view.close_breakout();
    assert!(env.view.breakout_close_job.is_none());
}

#[test]
#[serial]
fn trashing_a_broken_out_session_queues_a_close_for_exactly_its_title() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(2);
    let ids = break_out_all(&mut env);
    let gone = env
        .view
        .breakout_titles
        .iter()
        .find(|t| crate::breakout::title_is_for(t, &ids[0]))
        .cloned()
        .unwrap();

    // A sweep is running, so the close waits in the queue rather than being dropped.
    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_close_job = Some(rx);
    let _respond = env.view.session_feed.request_driver_for_test();
    env.view.trash_session_by_id(&ids[0]);

    assert_eq!(env.view.breakout_close_queue, [gone]);
    assert!(!env.view.is_broken_out(&ids[0]));
    assert!(env.view.is_broken_out(&ids[1]), "other windows stay");
    assert!(env.view.breakout_session_close_job.is_none());

    tx.send(Ok(0)).unwrap();
    assert!(env.view.tick_breakout());
    assert!(
        env.view.breakout_close_queue.is_empty(),
        "the queued close starts once the sweep is done"
    );
    assert!(env.view.breakout_session_close_job.is_some());
    assert!(env.view.tick_breakout());
    assert!(env.view.breakout_session_close_job.is_none());
}

#[test]
#[serial]
fn stopping_a_broken_out_session_closes_its_window() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(2);
    let ids = break_out_all(&mut env);
    let _respond = env.view.session_feed.command_driver_for_test();
    assert!(env.view.submit_daemon_stop(&ids[1]).unwrap());
    assert!(!env.view.is_broken_out(&ids[1]));
    assert!(env.view.is_broken_out(&ids[0]));
    assert!(
        env.view.breakout_session_close_job.is_some(),
        "nothing else runs, so the close starts at once"
    );
    assert!(env.view.breakout_close_queue.is_empty());
}

#[test]
#[serial]
fn stopping_a_session_that_is_not_broken_out_closes_nothing() {
    let mut env = create_test_env_with_sessions(1);
    let id = env.view.instance_at(0).id.clone();
    let _respond = env.view.session_feed.command_driver_for_test();
    assert!(env.view.submit_daemon_stop(&id).unwrap());
    assert!(env.view.breakout_session_close_job.is_none());
    assert!(env.view.breakout_close_queue.is_empty());
}

#[test]
#[serial]
fn a_broken_out_row_shows_the_marker_and_others_stay_aligned() {
    use crate::tui::home::icons::ICON_BREAKOUT;
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(2);
    let plain_before = rendered_row_text(&env.view, &env.view.flat_items[1].clone());
    mark_all(&mut env);
    let only_first: Vec<String> = env
        .view
        .marked_in_view_order()
        .into_iter()
        .skip(1)
        .collect();
    for id in only_first {
        env.view.marked.remove(&id);
    }
    make_sendable(&mut env);
    env.view.start_breakout();
    assert!(env.view.tick_breakout());

    let broken_out = rendered_row_text(&env.view, &env.view.flat_items[0].clone());
    let plain = rendered_row_text(&env.view, &env.view.flat_items[1].clone());
    assert!(broken_out.starts_with(ICON_BREAKOUT), "{broken_out:?}");
    assert_eq!(
        plain.trim_end(),
        format!("  {}", plain_before.trim_end()),
        "other rows are padded by the marker column"
    );
}

#[test]
#[serial]
fn the_preview_header_says_a_session_is_in_a_breakout_window() {
    use crate::tui::home::render::BREAKOUT_PREVIEW_NOTE;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(1);
    let screen = |view: &mut HomeView| {
        let theme = crate::tui::styles::load_theme("empire");
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                view.render(f, area, &theme, None, None, None);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    assert!(!screen(&mut env.view).contains(BREAKOUT_PREVIEW_NOTE));

    break_out_all(&mut env);
    env.view.update_selected();
    assert!(
        screen(&mut env.view).contains(BREAKOUT_PREVIEW_NOTE),
        "the selected Session is broken out"
    );
}

#[test]
#[serial]
fn a_session_stopped_while_its_breakout_opens_gets_no_marker_and_its_window_closes() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    make_sendable(&mut env);
    let ids = env.view.marked_in_view_order();
    env.view.start_breakout();
    assert!(env.view.breakout_job.is_some(), "the open job is running");

    let _respond = env.view.session_feed.command_driver_for_test();
    assert!(env.view.submit_daemon_stop(&ids[0]).unwrap());
    assert_eq!(
        env.view.breakout_close_queue.len(),
        1,
        "queued behind the open"
    );
    assert!(crate::breakout::title_is_for(
        &env.view.breakout_close_queue[0],
        &ids[0]
    ));

    assert!(env.view.tick_breakout());
    assert!(!env.view.is_broken_out(&ids[0]), "no stale marker");
    assert!(env.view.is_broken_out(&ids[1]));
    assert!(env.view.breakout_close_queue.is_empty());
    assert!(
        env.view.breakout_session_close_job.is_some(),
        "the queued close runs after the open lands"
    );
}

#[test]
#[serial]
fn a_finished_open_skips_titles_already_queued_to_close() {
    let mut env = create_test_env_with_sessions(1);
    let (tx, rx) = std::sync::mpsc::channel();
    env.view.breakout_job = Some(rx);
    env.view.breakout_close_queue.push("t0".into());
    tx.send(Ok(breakout_outcome(2, None))).unwrap();
    assert!(env.view.tick_breakout());
    assert_eq!(env.view.breakout_titles, ["t1"]);
}

#[test]
#[serial]
fn a_splits_window_marks_every_session_and_outlives_one_of_them() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(3);
    let ids: Vec<String> = (0..3).map(|i| env.view.instance_at(i).id.clone()).collect();
    let title = crate::breakout::splits_title(None, ids.iter().map(String::as_str));
    env.view.breakout_titles = vec![title.clone()];
    for id in &ids {
        assert!(env.view.is_broken_out(id));
    }

    // Stopping one Session ends its pane by itself; the window holds the others.
    let _respond = env.view.session_feed.command_driver_for_test();
    assert!(env.view.submit_daemon_stop(&ids[1]).unwrap());
    assert!(env.view.breakout_session_close_job.is_none());
    assert!(env.view.breakout_close_queue.is_empty());
    assert_eq!(env.view.breakout_titles, [title]);
    assert!(env.view.is_broken_out(&ids[0]));

    env.view.close_breakout();
    assert!(env.view.tick_breakout());
    assert!(env.view.breakout_titles.is_empty());
}

#[test]
#[serial]
fn a_session_stopped_while_its_splits_window_opens_closes_nothing() {
    let _forced = ForcedSupport::on();
    let mut env = create_test_env_with_sessions(2);
    let ids: Vec<String> = (0..2).map(|i| env.view.instance_at(i).id.clone()).collect();
    env.view.breakout_opening_titles = vec![crate::breakout::splits_title(
        None,
        ids.iter().map(String::as_str),
    )];
    let _respond = env.view.session_feed.command_driver_for_test();
    assert!(env.view.submit_daemon_stop(&ids[0]).unwrap());
    assert!(env.view.breakout_close_queue.is_empty());
    assert!(env.view.breakout_session_close_job.is_none());
}

/// Runs one test as if aoe were shown over ssh, and restores it even when an assertion fails.
struct OverSsh;

impl OverSsh {
    fn on() -> Self {
        crate::tui::home::breakout::test_support::UNDER_SSH.with(|u| u.set(true));
        crate::tui::home::breakout::test_support::OPENED_FOR.with(|o| *o.borrow_mut() = None);
        Self
    }
}

impl Drop for OverSsh {
    fn drop(&mut self) {
        crate::tui::home::breakout::test_support::UNDER_SSH.with(|u| u.set(false));
        crate::tui::home::breakout::test_support::OPENED_FOR.with(|o| *o.borrow_mut() = None);
    }
}

/// Stand-in for the ssh-forwarded agent socket: the TUI only checks that it exists,
/// and tests never open a real socket.
fn forward_agent_socket() {
    let path = crate::breakout::agent::socket_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
}

fn opened_for() -> Option<crate::tui::home::breakout::test_support::Opened> {
    crate::tui::home::breakout::test_support::OPENED_FOR.with(|o| o.borrow().clone())
}

#[test]
#[serial]
fn over_ssh_without_a_forwarded_socket_breakout_is_unavailable() {
    let _forced = ForcedSupport::on();
    let _ssh = OverSsh::on();
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    make_sendable(&mut env);
    env.view.start_breakout();
    assert!(
        env.view.info_dialog.is_some(),
        "never falls back to opening windows on this machine"
    );
    assert!(env.view.breakout_job.is_none());
    assert!(opened_for().is_none());
}

#[test]
#[serial]
fn over_ssh_with_a_forwarded_socket_the_agent_opens_and_the_sessions_are_marked() {
    let _forced = ForcedSupport::set(false);
    let _ssh = OverSsh::on();
    let mut env = create_test_env_with_sessions(2);
    forward_agent_socket();
    let ids = break_out_all(&mut env);
    let opened = opened_for().expect("opened");
    let (socket, host) = opened.agent.expect("the agent transport was used");
    assert_eq!(socket, crate::breakout::agent::socket_path().unwrap());
    assert_eq!(
        opened.request_host,
        Some(host),
        "the request names this host"
    );
    for id in &ids {
        assert!(env.view.is_broken_out(id));
    }
    assert!(env.view.marked.is_empty());
}

#[test]
#[serial]
fn a_local_breakout_names_no_host() {
    let _forced = ForcedSupport::on();
    crate::tui::home::breakout::test_support::OPENED_FOR.with(|o| *o.borrow_mut() = None);
    let mut env = create_test_env_with_sessions(1);
    break_out_all(&mut env);
    let opened = opened_for().expect("opened");
    assert_eq!((opened.request_host, opened.agent), (None, None));
}

#[test]
#[serial]
fn over_ssh_closing_clears_the_markers_without_a_close_job() {
    let _forced = ForcedSupport::set(false);
    let _ssh = OverSsh::on();
    let mut env = create_test_env_with_sessions(2);
    forward_agent_socket();
    let ids = break_out_all(&mut env);

    mark_all(&mut env);
    env.view.handle_key(shift_k(), None);
    let menu = env.view.context_menu.as_ref().unwrap();
    assert!(
        menu.items_contain(ContextMenuAction::CloseBreakout),
        "offered over ssh, where it clears the markers"
    );
    env.view.context_menu = None;

    env.view.close_breakout();
    assert!(env.view.breakout_close_job.is_none(), "no osascript here");
    assert!(env.view.breakout_titles.is_empty());
    for id in &ids {
        assert!(!env.view.is_broken_out(id));
    }
    assert!(env
        .view
        .status_flash_text()
        .is_some_and(|text| text.contains("machine with the screen")));
}

#[test]
#[serial]
fn over_ssh_stopping_a_broken_out_session_clears_its_marker_only() {
    let _forced = ForcedSupport::set(false);
    let _ssh = OverSsh::on();
    let mut env = create_test_env_with_sessions(2);
    forward_agent_socket();
    let ids = break_out_all(&mut env);
    let _respond = env.view.session_feed.command_driver_for_test();
    assert!(env.view.submit_daemon_stop(&ids[1]).unwrap());
    assert!(!env.view.is_broken_out(&ids[1]));
    assert!(env.view.is_broken_out(&ids[0]));
    assert!(
        env.view.breakout_session_close_job.is_none(),
        "no osascript here"
    );
    assert!(env.view.breakout_close_queue.is_empty());
    assert!(env
        .view
        .status_flash_text()
        .is_some_and(|text| text.contains("machine with the screen")));
}

#[test]
#[serial]
fn over_ssh_a_session_stopped_mid_open_gets_no_marker() {
    let _forced = ForcedSupport::set(false);
    let _ssh = OverSsh::on();
    let mut env = create_test_env_with_sessions(2);
    forward_agent_socket();
    mark_all(&mut env);
    make_sendable(&mut env);
    let ids = env.view.marked_in_view_order();
    env.view.start_breakout();
    assert!(env.view.breakout_job.is_some());
    let _respond = env.view.session_feed.command_driver_for_test();
    assert!(env.view.submit_daemon_stop(&ids[0]).unwrap());
    assert!(env.view.tick_breakout());
    assert!(!env.view.is_broken_out(&ids[0]));
    assert!(env.view.is_broken_out(&ids[1]));
    assert!(env.view.breakout_session_close_job.is_none());
    assert!(env.view.breakout_close_queue.is_empty());
}
