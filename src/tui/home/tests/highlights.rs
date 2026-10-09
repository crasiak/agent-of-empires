use super::*;
use crate::session::config::{GroupByMode, SortOrder};
use crate::session::repo_appearance::{RepoAppearance, RepoColor};

#[test]
#[serial]
fn highlight_filters_preserve_live_target_and_footer_on_external_color_change() {
    let active = Instance::new("active-live", "/repo/a");
    let peer = Instance::new("peer", "/repo/b");
    let mut env = seeded_env(test_home(), &[active.clone(), peer.clone()], true);
    let v = &mut env.view;
    crate::session::update_app_state(|state| {
        for id in ["/repo/a", "/repo/b"] {
            state.repo_appearances.insert(
                id.into(),
                RepoAppearance {
                    alias: None,
                    color: Some(RepoColor::Sky),
                },
            );
        }
    })
    .unwrap();
    v.try_refresh_from_config_watcher().unwrap();
    v.highlight_filters.projects = vec![Some(RepoColor::Sky)];
    v.rebuild_flat_items_keeping_cursor();
    v.select_session_by_id(&active.id);
    v.live_send = Some(live_send_state(&active.id, "active-live", "fake-live"));
    v.search_query = Input::new("peer".into());
    crate::session::update_app_state(|state| {
        state.repo_appearances.get_mut("/repo/a").unwrap().color = None
    })
    .unwrap();
    v.try_refresh_from_config_watcher().unwrap();
    assert!(!v
        .flat_items
        .iter()
        .any(|item| matches!(item, Item::Session { id, .. } if *id == active.id)));
    assert_eq!(v.selected_session.as_deref(), Some(active.id.as_str()));
    assert_eq!(v.live_send.as_ref().unwrap().session_id, active.id);
    for (index, item) in v.flat_items.iter().enumerate() {
        assert!(!v.is_sidebar_item_selected(item, index));
    }
    assert!(v
        .search_matches
        .iter()
        .any(|&index| matches!(&v.flat_items[index], Item::Session { id, .. } if *id == peer.id)));
    let screen = render_home_to_string(v, 120, 20);
    assert!(
        screen.contains("LIVE") && screen.contains("active-live"),
        "{screen}"
    );
    assert!(!screen.contains("Esc clear"));
    v.live_send.as_mut().unwrap().leader = super::super::live_send::parse_chord_list("C-b")
        .first()
        .copied();
    v.live_send_pending_leader = true;
    let screen = render_home_to_string(v, 120, 20);
    assert!(screen.contains("k palette"), "{screen}");
    assert!(!screen.contains("F4 edit"));
}

#[test]
#[serial]
fn highlight_rebuild_restores_shifted_group_and_attention_skips_filtered_waiting() {
    let mut a = Instance::new("alpha", "/repo/alpha");
    a.color = Some("red".into());
    let mut b = Instance::new("beta", "/repo/beta");
    b.color = Some("red".into());
    let mut env = seeded_env(test_home(), &[a.clone(), b.clone()], false);
    let v = &mut env.view;
    v.group_by = GroupByMode::Project;
    v.sort_order = SortOrder::AZ;
    v.highlight_filters.sessions = vec![Some("red".into())];
    v.rebuild_flat_items();
    v.cursor = v
        .flat_items
        .iter()
        .position(|item| matches!(item, Item::Group {path,..} if path == "beta"))
        .unwrap();
    v.update_selected();
    v.instances.get_mut(&a.id).unwrap().color = None;
    v.rebuild_flat_items_keeping_cursor();
    assert_eq!(v.selected_group.as_deref(), Some("beta"));
    assert_eq!(v.cursor, 0);
    let mut waiting = Instance::new("hidden-waiting", "/hidden");
    waiting.source_profile = "test".into();
    waiting.status = Status::Waiting;
    let mut idle = Instance::new("visible-idle", "/idle");
    idle.source_profile = "test".into();
    idle.status = Status::Idle;
    idle.color = Some("red".into());
    v.instances.insert(waiting.id.clone(), waiting.clone());
    v.instances.insert(idle.id.clone(), idle.clone());
    v.group_by = GroupByMode::Manual;
    v.rebuild_group_trees();
    v.rebuild_flat_items();
    v.select_session_by_id(&b.id);
    v.handle_key(key(KeyCode::Char('w')), None);
    assert_eq!(v.selected_session.as_deref(), Some(idle.id.as_str()));
    // A matching collapsed row remains eligible for the hidden fallback.
    v.instances.get_mut(&waiting.id).unwrap().color = Some("red".into());
    v.instances.get_mut(&waiting.id).unwrap().group_path = "hidden".into();
    v.rebuild_group_trees();
    v.group_trees
        .get_mut("test")
        .unwrap()
        .set_collapsed("hidden", true);
    v.rebuild_flat_items();
    v.handle_key(key(KeyCode::Char('w')), None);
    assert_eq!(v.selected_session.as_deref(), Some(waiting.id.as_str()));
}

#[test]
#[serial]
fn highlight_manual_groups_do_not_borrow_matches_from_other_profiles() {
    let (temp, guard) = test_home();
    let mut red = Instance::new("red", "/a");
    red.group_path = "work".into();
    red.color = Some("red".into());
    let mut green = Instance::new("green", "/b");
    green.group_path = "work".into();
    green.color = Some("green".into());
    seed_profile("one", &[red]);
    seed_profile("two", &[green]);
    let mut v = test_view(None);
    v.group_by = GroupByMode::Manual;
    v.sort_order = SortOrder::AZ;
    v.highlight_filters.sessions = vec![Some("red".into())];
    v.rebuild_flat_items();
    let groups: Vec<_> = v
        .flat_items
        .iter()
        .filter_map(|item| match item {
            Item::Group { path, profile, .. } => Some((path.as_str(), profile.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(groups, [("work", Some("one"))]);
    drop(v);
    drop(guard);
    drop(temp);
}

#[test]
#[serial]
fn highlight_filter_rebuild_selection_search_and_duplicate_basename_safety() {
    let mut a = Instance::new("alpha", "/one/repo");
    a.color = Some("red".into());
    let mut b = Instance::new("beta", "/two/repo");
    b.color = Some("green".into());
    let mut env = seeded_env(test_home(), &[a.clone(), b.clone()], false);
    let v = &mut env.view;
    v.group_by = GroupByMode::Project;
    v.repo_appearances.insert(
        "/one/repo".into(),
        RepoAppearance {
            alias: Some("First".into()),
            color: Some(RepoColor::Sky),
        },
    );
    v.rebuild_flat_items();
    v.select_session_by_id(&b.id);
    assert_eq!(
        v.project_appearance_id("repo"),
        None,
        "a display-label collision must never lend an arbitrary path"
    );
    v.search_query = Input::new("alpha".into());
    v.refresh_search_matches();
    v.highlight_filters.projects = vec![Some(RepoColor::Sky)];
    v.highlight_filters.sessions = vec![Some("red".into()), None];
    v.rebuild_flat_items_keeping_cursor();
    assert_eq!(
        v.flat_items
            .iter()
            .filter_map(|i| if let Item::Session { id, .. } = i {
                Some(id.as_str())
            } else {
                None
            })
            .collect::<Vec<_>>(),
        [a.id.as_str()]
    );
    assert_ne!(v.selected_session.as_deref(), Some(b.id.as_str()));
    assert!(v
        .search_matches
        .iter()
        .all(|&i| matches!(&v.flat_items[i], Item::Session { id,.. } if *id == a.id)));
    v.repo_appearances.clear();
    v.rebuild_flat_items_keeping_cursor();
    assert!(v.flat_items.is_empty());
    assert!(v.selected_session.is_none());
    assert!(v.search_matches.is_empty());
    v.highlight_filters = Default::default();
    v.rebuild_flat_items_keeping_cursor();
    assert!(
        !v.search_matches.is_empty(),
        "a committed query with zero matches must be recomputed"
    );
    v.highlight_filters.sessions = vec![Some("red".into())];
    v.group_by = GroupByMode::Manual;
    v.sort_order = SortOrder::Custom;
    v.rebuild_flat_items_keeping_cursor();
    v.move_row_at_cursor(1).unwrap();
    assert!(v
        .status_flash_text()
        .unwrap()
        .contains("Clear highlight filters"));
    v.select_session_by_id(&a.id);
    v.dispatch_context_menu_action(crate::tui::dialogs::ContextMenuAction::HighlightGreen);
    assert!(
        v.flat_items.is_empty(),
        "a local color edit must immediately remove its nonmatching row"
    );
    assert!(v.selected_session.is_none());
}

#[test]
#[serial]
fn highlight_empty_projects_archived_trash_and_synthetic_identity() {
    use crate::session::repo_appearance::{MULTI_REPO_ID, SCRATCH_REPO_ID};
    let mut multi = Instance::new("multi", "/workspace/a");
    multi.workspace_info = Some(serde_json::from_value(serde_json::json!({
        "branch":"main","workspace_dir":"/workspace","created_at":"2026-01-01T00:00:00Z",
        "repos":[
            {"name":"a","source_path":"/a","branch":"main","worktree_path":"/workspace/a","main_repo_path":"/a","managed_by_aoe":false},
            {"name":"b","source_path":"/b","branch":"main","worktree_path":"/workspace/b","main_repo_path":"/b","managed_by_aoe":false}
        ]
    })).unwrap());
    let mut scratch = Instance::new("scratch", "/scratch/temporary");
    scratch.scratch = true;
    let mut trash = Instance::new("trash", "/trash");
    trash.trash();
    let mut env = seeded_env(
        test_home(),
        &[multi.clone(), scratch.clone(), trash.clone()],
        false,
    );
    let v = &mut env.view;
    v.group_by = GroupByMode::Project;
    v.registered_projects = vec![crate::session::Project::new(
        "Pinned",
        "/empty",
        crate::session::ProjectScope::Global,
    )
    .with_pinned(true)];
    for id in [MULTI_REPO_ID, SCRATCH_REPO_ID, "/empty"] {
        v.repo_appearances.insert(
            id.into(),
            RepoAppearance {
                alias: None,
                color: Some(RepoColor::Sky),
            },
        );
    }
    v.highlight_filters.projects = vec![Some(RepoColor::Sky)];
    v.trashed_section_collapsed = false;
    v.rebuild_flat_items();
    assert!(v
        .flat_items
        .iter()
        .any(|item| matches!(item,Item::Group {path,..} if path == "empty")));
    assert!(v
        .flat_items
        .iter()
        .any(|item| matches!(item,Item::Session {id,..} if *id == trash.id)));
    assert_eq!(v.group_repo_path(MULTI_REPO_ID), None);
    let mut literal = Instance::new("literal", "/other/__multi_repo__");
    literal.source_profile = "test".into();
    v.instances.insert(literal.id.clone(), literal.clone());
    assert_eq!(
        v.project_appearance_id(MULTI_REPO_ID),
        None,
        "synthetic headers cannot borrow a real repository with a colliding label"
    );
    v.instances.shift_remove(&literal.id);
    v.cursor = v
        .flat_items
        .iter()
        .position(|item| matches!(item,Item::Group {path,..} if path == MULTI_REPO_ID))
        .unwrap();
    v.update_selected();
    v.handle_key(key(KeyCode::Char('N')), None);
    let dialog = v.new_dialog.as_ref().unwrap();
    assert_eq!(dialog.group_value(), "Multi-repo");
    assert_ne!(dialog.path_value(), "/workspace/a");
    v.new_dialog = None;
    v.instances.get_mut(&multi.id).unwrap().archive();
    v.archived_section_collapsed = false;
    v.rebuild_flat_items_keeping_cursor();
    assert!(v.flat_items.iter().any(|item| matches!(item,Item::Group {path,name,..} if crate::session::is_within_archived_section(path) && name == "Multi-repo")));
    v.highlight_filters.sessions = vec![Some("red".into())];
    v.rebuild_flat_items_keeping_cursor();
    assert!(!v.flat_items.iter().any(|item| matches!(item,Item::Group {path,..} if path == "empty" || crate::session::is_within_archived_section(path))));
    assert_eq!(
        v.flat_items
            .iter()
            .filter_map(|item| if let Item::Session { id, .. } = item {
                Some(id)
            } else {
                None
            })
            .collect::<Vec<_>>(),
        [&trash.id]
    );
}

#[test]
#[serial]
fn project_menu_preserves_alias_tints_with_precedence_and_refreshes_external_state() {
    let a = Instance::new("alpha", "/one/repo");
    let mut env = seeded_env(test_home(), &[a], false);
    let v = &mut env.view;
    v.group_by = GroupByMode::Project;
    v.rebuild_flat_items();
    v.cursor = 0;
    v.update_selected();
    crate::session::update_app_state(|state| {
        state.repo_appearances.insert(
            "/one/repo".into(),
            RepoAppearance {
                alias: Some("Alias".into()),
                color: None,
            },
        );
    })
    .unwrap();
    v.open_project_context_menu((1, 1));
    assert!(v.context_menu.is_some());
    v.set_project_highlight(Some(RepoColor::Sky));
    assert_eq!(
        v.repo_appearances["/one/repo"].alias.as_deref(),
        Some("Alias")
    );
    for palette in [false, true] {
        let theme = crate::tui::styles::load_theme_with_mode("empire", palette);
        let item = &v.flat_items[0];
        assert_eq!(
            v.sidebar_row_background(item, true, false, &theme),
            Some(theme.session_selection)
        );
        assert_eq!(
            v.sidebar_row_background(item, false, true, &theme),
            Some(theme.selection)
        );
        let tint = v
            .sidebar_row_background(item, false, false, &theme)
            .unwrap();
        assert_ne!(tint, theme.session_selection);
        if palette {
            assert!(matches!(tint, ratatui::style::Color::Indexed(_)));
        }
    }
    v.highlight_filters.projects = vec![Some(RepoColor::Sky)];
    crate::session::update_app_state(|state| {
        state.repo_appearances.get_mut("/one/repo").unwrap().color = None
    })
    .unwrap();
    v.try_refresh_from_config_watcher().unwrap();
    assert!(v.flat_items.is_empty());
    assert!(v.selected_session.is_none());
    assert_eq!(
        v.repo_appearances["/one/repo"].alias.as_deref(),
        Some("Alias")
    );
    v.highlight_filters = Default::default();
    for instance in v.instances.values_mut() {
        instance.archive();
    }
    v.archived_section_collapsed = false;
    v.rebuild_flat_items();
    v.cursor = v.flat_items.iter().position(|item| matches!(item,Item::Group {path,..} if path == &crate::session::archived_project_sub_path("repo"))).unwrap();
    v.update_selected();
    v.open_project_context_menu((1, 1));
    let items = v.context_menu.as_ref().unwrap().items_for_test();
    assert_eq!(items.len(), 7);
    assert!(
        items.iter().all(|(action, _)| matches!(
            action,
            crate::tui::dialogs::ContextMenuAction::ProjectColor(_)
        )),
        "archived headers must not offer nonfunctional lifecycle or pin actions"
    );
    v.set_project_highlight(Some(RepoColor::Rose));
    assert_eq!(v.repo_appearances["/one/repo"].color, Some(RepoColor::Rose));
    assert_eq!(
        v.repo_appearances["/one/repo"].alias.as_deref(),
        Some("Alias")
    );
}
