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

    env.view.mutate_instance(&id, |inst| {
        inst.trashed_at = Some(crate::time::Timestamp::now())
    });
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

#[test]
#[serial]
fn a_marked_row_shows_the_marker_and_others_stay_aligned() {
    let mut env = create_test_env_with_sessions(2);
    put_cursor(&mut env, 0);
    let unmarked_before = rendered_row_text(&env.view, &env.view.flat_items[1].clone());

    env.view.handle_key(space(), None);
    let marked = rendered_row_text(&env.view, &env.view.flat_items[0].clone());
    let unmarked = rendered_row_text(&env.view, &env.view.flat_items[1].clone());

    assert!(marked.starts_with(crate::tui::home::icons::ICON_MARK));
    // Rows are padded to the width, so compare the content before the padding.
    assert_eq!(
        unmarked.trim_end(),
        format!("  {}", unmarked_before.trim_end()),
        "unmarked rows are padded by the marker column"
    );
}

#[test]
#[serial]
fn the_sidebar_title_counts_marks_and_hidden_marks() {
    let mut env = create_test_env_with_group_sessions();
    let row = group_row(&env, "work");
    put_cursor(&mut env, row);
    env.view.handle_key(space(), None);
    assert_eq!(env.view.marks_title().as_deref(), Some("3 marked"));
    assert!(render_home_to_string(&mut env.view, 120, 40).contains("3 marked"));

    // Collapsing "work" goes through the daemon in this harness, so drop its rows
    // directly: the sessions leave the list but stay marked.
    env.view
        .flat_items
        .retain(|item| !matches!(item, Item::Session { .. }));
    assert_eq!(
        env.view.marks_title().as_deref(),
        Some("3 marked (3 hidden)")
    );
}

#[test]
#[serial]
fn no_marks_means_no_title_text() {
    let env = create_test_env_with_sessions(1);
    assert_eq!(env.view.marks_title(), None);
}

/// Two Sessions in a collapsed "work" Group and one ungrouped Session.
fn collapsed_group_env() -> TestEnv {
    use crate::session::config::GroupByMode;
    let (temp, guard) = test_home();
    let instances = [
        instance_in("w1", "/tmp/w1", "work"),
        instance_in("w2", "/tmp/w2", "work"),
        instance_in("solo", "/tmp/solo", ""),
    ];
    Storage::new_unwatched("test")
        .unwrap()
        .update(|i, g| {
            *i = instances.to_vec();
            let mut groups = GroupTree::new_with_groups(&instances, &[]).get_all_groups();
            for group in &mut groups {
                group.collapsed = group.path == "work";
            }
            *g = groups;
            Ok(())
        })
        .unwrap();
    let mut view = test_view(Some("test"));
    view.group_by = GroupByMode::Manual;
    view.flat_items = view.build_flat_items();
    view.update_selected();
    TestEnv {
        view,
        _guard: guard,
        _temp: temp,
    }
}

#[test]
#[serial]
fn space_on_a_collapsed_group_header_marks_its_hidden_sessions() {
    let mut env = collapsed_group_env();
    let row = group_row(&env, "work");
    assert_eq!(
        env.view
            .flat_items
            .iter()
            .filter(|item| matches!(item, Item::Session { .. }))
            .count(),
        1,
        "only the ungrouped Session is listed"
    );
    put_cursor(&mut env, row);

    env.view.handle_key(space(), None);

    assert_eq!(
        env.view.marked.len(),
        2,
        "the collapsed Sessions are marked"
    );
    assert_eq!(
        env.view.marks_title().as_deref(),
        Some("2 marked (2 hidden)")
    );

    env.view.handle_key(space(), None);
    assert!(env.view.marked.is_empty());
}

#[test]
#[serial]
fn space_on_a_project_header_marks_that_projects_sessions() {
    use crate::session::config::{GroupByMode, SortOrder};
    let mut env = create_test_env_two_projects_mixed_attention();
    env.view.group_by = GroupByMode::Project;
    env.view.sort_order = SortOrder::Newest;
    env.view.flat_items = env.view.build_flat_items();
    let rows: Vec<usize> = env
        .view
        .flat_items
        .iter()
        .enumerate()
        .filter_map(|(row, item)| matches!(item, Item::Group { .. }).then_some(row))
        .collect();
    assert_eq!(rows.len(), 2, "one header per project");
    put_cursor(&mut env, rows[0]);

    env.view.handle_key(space(), None);

    assert_eq!(env.view.marked.len(), 2, "only that project's Sessions");
    let titles: Vec<String> = env
        .view
        .marked
        .iter()
        .map(|id| env.view.get_instance(id).unwrap().title.clone())
        .collect();
    let prefix = titles[0].split('-').next().unwrap().to_string();
    assert!(
        titles.iter().all(|title| title.starts_with(&prefix)),
        "{titles:?}"
    );

    env.view.handle_key(space(), None);
    assert!(env.view.marked.is_empty());
}

#[test]
#[serial]
fn switching_profile_drops_marks_on_sessions_it_no_longer_shows() {
    let (_temp, _guard) = test_home();
    seed_profile("alpha", &[instance_in("a1", "/tmp/a", "")]);
    seed_profile("beta", &[instance_in("b1", "/tmp/b", "")]);
    let mut view = test_view(None);
    view.rebuild_flat_items();
    let ids: Vec<String> = view
        .flat_items
        .iter()
        .filter_map(|item| match item {
            Item::Session { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    view.marked.extend(ids);

    view.switch_profile(Some("beta".to_string())).unwrap();

    assert_eq!(
        view.marked.len(),
        1,
        "only the Session still shown stays marked"
    );
    assert_eq!(view.marks_title().as_deref(), Some("1 marked"));
}

#[test]
#[serial]
fn the_footer_hides_esc_clear_marks_while_the_retained_reader_is_focused() {
    let mut env = create_test_env_with_sessions(2);
    let id = env.view.instance_at(0).id.clone();
    env.view.mutate_instance(&id, |inst| {
        inst.trashed_at = Some(crate::time::Timestamp::now())
    });
    env.view.trashed_section_collapsed = false;
    env.view.rebuild_flat_items();
    env.view.select_session_by_id(&id);
    let other = env.view.instance_at(1).id.clone();
    env.view.marked.insert(other);

    let unfocused = render_home_to_string(&mut env.view, 140, 40);
    assert!(!env.view.retained_reader_focused());
    assert!(unfocused.contains("clear marks"), "{unfocused}");

    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.retained_reader_focused());
    let focused = render_home_to_string(&mut env.view, 140, 40);
    assert!(focused.contains("Space"), "the Marks hint stays");
    assert!(!focused.contains("clear marks"), "{focused}");
}
