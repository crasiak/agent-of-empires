//! Bulk actions on the Marked set, driven through the home view.

use super::*;
use crate::tui::dialogs::ContextMenuAction;
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
    assert_eq!(
        env.view
            .pending_bulk_restart
            .as_ref()
            .map(|plan| plan.eligible.len()),
        Some(3)
    );
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

    let plan = env
        .view
        .pending_bulk_restart
        .as_ref()
        .expect("confirm open");
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
    assert!(env.view.pending_bulk_restart.is_none());
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

    assert!(env.view.pending_bulk_restart.is_none());
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
    let plan = env
        .view
        .pending_bulk_restart
        .as_ref()
        .expect("bulk confirm");
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
    let mut env = create_test_env_with_sessions(2);
    mark_all(&mut env);
    env.view.handle_key(shift_k(), None);
    // Entries: Send, Restart, Show skipped, Clear marks.
    env.view.handle_key(key(KeyCode::Down), None);
    env.view.handle_key(key(KeyCode::Enter), None);

    assert!(env.view.context_menu.is_none());
    assert!(env.view.pending_bulk_restart.is_some());
}

#[test]
#[serial]
fn the_bulk_menu_can_clear_marks() {
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
    let mut env = trashed_cursor_env();
    env.view.handle_key(shift_k(), None);
    assert!(env.view.context_menu.is_some());

    env.view.handle_key(key(KeyCode::Down), None);
    assert!(
        env.view.context_menu.is_some(),
        "the next key keeps the menu"
    );
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.pending_bulk_restart.is_some());
}

#[test]
#[serial]
fn the_bulk_send_dialog_survives_a_trashed_cursor_row() {
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
    assert!(env.view.pending_bulk_restart.is_some());

    env.view.handle_key(key(KeyCode::Tab), None);
    assert!(env.view.confirm_dialog.is_some());
    assert!(env.view.pending_bulk_restart.is_some());
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
