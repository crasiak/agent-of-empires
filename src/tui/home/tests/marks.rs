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
