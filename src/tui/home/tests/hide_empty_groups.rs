use super::*;
use crate::session::config::{update_config, GroupByMode, SortOrder};

#[test]
#[serial]
fn empty_groups_follow_settings_without_removing_stored_groups() {
    let live = instance_in("live", "/repos/live", "parent/child");
    let mut archived = instance_in("archived", "/repos/archived", "archived-only");
    archived.archive();
    let mut trashed = instance_in("trashed", "/repos/trashed", "trash-only");
    trashed.trash();
    let mut env = seeded_env(test_home(), &[live.clone(), archived, trashed], true);
    Storage::open_unwatched("test")
        .unwrap()
        .update(|_, groups| {
            groups.push(Group::new("empty", "empty"));
            groups.push(Group::new("empty-child", "parent/empty-child"));
            Ok(())
        })
        .unwrap();
    seed_profile("other", &[]);
    Storage::open_unwatched("other")
        .unwrap()
        .update(|_, groups| {
            groups.push(Group::new("parent", "parent"));
            Ok(())
        })
        .unwrap();

    for active in [Some("test"), None] {
        let mut view = test_view(active);
        view.group_by = GroupByMode::Manual;
        view.sort_order = SortOrder::AZ;
        view.archived_section_collapsed = false;
        view.trashed_section_collapsed = false;
        for hide in [true, false, true] {
            update_config(|config| config.session.hide_empty_groups = hide).unwrap();
            view.try_refresh_from_config_watcher().unwrap();
            view.rebuild_flat_items();
            let groups: Vec<_> = view
                .flat_items
                .iter()
                .filter_map(|item| match item {
                    Item::Group {
                        path,
                        session_count,
                        ..
                    } => Some((path.as_str(), *session_count)),
                    _ => None,
                })
                .collect();
            assert_eq!(groups.contains(&("empty", 0)), !hide);
            assert_eq!(groups.contains(&("parent/empty-child", 0)), !hide);
            assert_eq!(groups.contains(&("archived-only", 0)), !hide);
            assert_eq!(groups.contains(&("trash-only", 0)), !hide);
            assert!(groups.contains(&("parent", 1)));
            assert!(groups.contains(&("parent/child", 1)));
            assert_eq!(
                groups.iter().filter(|(path, _)| *path == "parent").count(),
                1
            );
            assert!(!groups.contains(&("parent", 0)));
            assert!(view
                .flat_items
                .iter()
                .any(|item| matches!(item, Item::Session { id, .. } if *id == live.id)));
            assert!(view.flat_items.iter().any(|item| matches!(item, Item::Group { path, .. } if crate::session::is_within_archived_section(path))));
            assert!(view.flat_items.iter().any(|item| matches!(item, Item::Group { path, .. } if crate::session::is_within_trash_section(path))));
        }
    }
    env.view.reload_storage_only().unwrap();
    assert!(env.view.group_trees["test"].group_exists("empty"));
}

#[test]
#[serial]
fn hidden_stopped_groups_return_when_their_sessions_are_shown() {
    let mut stopped = instance_in("stopped", "/repos/stopped", "stopped");
    stopped.status = Status::Stopped;
    let mut env = seeded_env(test_home(), &[stopped], true);
    for grouping in [GroupByMode::Manual, GroupByMode::Project, GroupByMode::Org] {
        env.view.group_by = grouping;
        env.view.hide_stopped_in_groups = true;
        env.view.rebuild_flat_items();
        assert!(env.view.flat_items.is_empty(), "{grouping:?}");
        env.view.hide_stopped_in_groups = false;
        env.view.rebuild_flat_items();
        assert!(
            env.view.flat_items.iter().any(|item| matches!(
                item,
                Item::Group {
                    session_count: 1,
                    ..
                }
            )),
            "{grouping:?}"
        );
    }
}
