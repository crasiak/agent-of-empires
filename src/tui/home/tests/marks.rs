//! Marks: toggling, Group headers, range extension, clearing and pruning.

use super::*;

fn space() -> KeyEvent {
    key(KeyCode::Char(' '))
}

fn put_cursor(env: &mut TestEnv, row: usize) {
    env.view.cursor = row;
    env.view.update_selected();
}

#[test]
#[serial]
fn space_toggles_the_mark_on_the_cursor_session() {
    let mut env = create_test_env_with_sessions(3);
    put_cursor(&mut env, 1);
    let id = session_id_at(&env.view, 1).unwrap();

    env.view.handle_key(space(), None);
    assert!(env.view.marked.contains(&id));
    assert_eq!(env.view.cursor, 1, "marking does not move the cursor");

    env.view.handle_key(space(), None);
    assert!(env.view.marked.is_empty());
}

#[test]
#[serial]
fn space_marks_in_strict_mode_too() {
    let mut env = create_test_env_with_sessions(2);
    env.view.strict_hotkeys = true;
    put_cursor(&mut env, 0);
    env.view.handle_key(space(), None);
    assert_eq!(env.view.marked.len(), 1);
}

#[test]
#[serial]
fn a_trashed_session_loses_its_mark_on_prune() {
    let mut env = create_test_env_with_sessions(2);
    put_cursor(&mut env, 0);
    env.view.handle_key(space(), None);
    let id = session_id_at(&env.view, 0).unwrap();

    env.view
        .mutate_instance(&id, |inst| inst.trashed_at = Some(chrono::Utc::now()));
    env.view.prune_marks();
    assert!(env.view.marked.is_empty());
}

#[test]
#[serial]
fn a_mark_on_a_vanished_session_is_pruned() {
    let mut env = create_test_env_with_sessions(1);
    env.view.marked.insert("no-such-session".to_string());
    env.view.prune_marks();
    assert!(env.view.marked.is_empty());
}

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn group_row(env: &TestEnv, group: &str) -> usize {
    env.view
        .flat_items
        .iter()
        .position(|item| matches!(item, Item::Group { path, .. } if path == group))
        .expect("group header row")
}

#[test]
#[serial]
fn space_on_a_group_header_marks_then_unmarks_its_sessions() {
    // ungrouped sessions, 2 in "work", 1 in "work/projects"
    let mut env = create_test_env_with_group_sessions();
    let row = group_row(&env, "work");
    put_cursor(&mut env, row);

    env.view.handle_key(space(), None);
    assert_eq!(
        env.view.marked.len(),
        3,
        "nested group sessions are included"
    );

    env.view.handle_key(space(), None);
    assert!(
        env.view.marked.is_empty(),
        "all marked, so the header unmarks"
    );
}

#[test]
#[serial]
fn a_partly_marked_group_becomes_fully_marked() {
    let mut env = create_test_env_with_group_sessions();
    let row = group_row(&env, "work");
    put_cursor(&mut env, row + 1);
    env.view.handle_key(space(), None);
    assert_eq!(env.view.marked.len(), 1);

    put_cursor(&mut env, row);
    env.view.handle_key(space(), None);
    assert_eq!(env.view.marked.len(), 3);
}

#[test]
#[serial]
fn shift_arrows_extend_marks_one_row_at_a_time() {
    let mut env = create_test_env_with_sessions(5);
    put_cursor(&mut env, 1);

    env.view.handle_key(shift(KeyCode::Down), None);
    env.view.handle_key(shift(KeyCode::Down), None);
    assert_eq!(env.view.cursor, 3, "moves one row per press, not ten");
    let expected: std::collections::HashSet<String> = (1..=3)
        .map(|row| session_id_at(&env.view, row).unwrap())
        .collect();
    assert_eq!(env.view.marked, expected);

    env.view.handle_key(shift(KeyCode::Up), None);
    assert_eq!(env.view.cursor, 2);
    assert_eq!(
        env.view.marked, expected,
        "extending back keeps existing marks"
    );
}

#[test]
#[serial]
fn esc_clears_marks_only_after_the_search_is_cleared() {
    let mut env = create_test_env_with_sessions(3);
    put_cursor(&mut env, 0);
    env.view.handle_key(space(), None);
    env.view.search_query = Input::new("zzz".to_string());

    env.view.handle_key(key(KeyCode::Esc), None);
    assert!(env.view.search_query.value().is_empty());
    assert_eq!(
        env.view.marked.len(),
        1,
        "first Esc clears the committed search"
    );

    env.view.handle_key(key(KeyCode::Esc), None);
    assert!(env.view.marked.is_empty(), "second Esc clears the marks");
}
