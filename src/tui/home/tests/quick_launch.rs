use super::*;
use crate::session::config::quick_launch::QuickLaunchPreset;
use crate::tui::dialogs::DialogResult;

fn assign(slot: &str, profile: &str, tool: Option<&str>, ledger_profile: Option<&str>) {
    crate::session::config::update_config(|config| {
        config.session.show_tips = false;
        config.session.quick_launch.insert(
            slot.into(),
            QuickLaunchPreset {
                profile: profile.into(),
                tool: tool.map(str::to_string),
                ledger_profile: ledger_profile.map(str::to_string),
            },
        );
    })
    .unwrap();
}

#[test]
#[serial]
fn quick_launch_preserves_destination_and_ledger_identity_with_editable_sidebar_defaults() {
    let (temp, _guard) = test_home();
    let directory = temp.path().join("repo");
    std::fs::create_dir(&directory).unwrap();
    let mut source = Instance::new("source", directory.to_str().unwrap());
    source.group_path = "team/project".into();
    source.tool = "claude".into();
    seed_profile("source", &[source.clone()]);
    seed_profile("work-headroom-ledger", &[]);
    assign("5", "work-headroom-ledger", Some("pi"), Some("work-claude"));
    crate::session::config::profile_config::save_profile_config(
        "work-headroom-ledger",
        &toml::from_str("[session]\nyolo_mode_default = true\ndefault_tool = 'claude'\n").unwrap(),
    )
    .unwrap();

    for strict in [false, true] {
        let mut view = test_view(Some("source"));
        view.available_tools = AvailableTools::with_tools(&["claude", "pi"]);
        view.strict_hotkeys = strict;
        view.selected_session = Some(source.id.clone());
        view.selected_group = None;
        let before = view.instances.len();
        assert!(view
            .handle_key(
                KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL),
                None
            )
            .is_none());
        assert_eq!(
            view.instances.len(),
            before,
            "shortcut must not create a session"
        );
        assert_eq!(view.active_profile_display(), Some("source"));
        let dialog = view.new_dialog.as_mut().expect("prefilled form");
        assert_eq!(dialog.path_value(), directory.to_str().unwrap());
        assert_eq!(dialog.group_value(), "team/project");
        assert_eq!(dialog.selected_tool(), "pi");
        assert!(
            dialog.yolo_value(),
            "target profile defaults, not source session modes"
        );
        dialog.set_group("changed/group".into());
        dialog.set_title("editable".into());
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(data) => {
                assert_eq!(data.profile, "work-headroom-ledger");
                assert_eq!(data.tool, "pi");
                assert_eq!(
                    data.command_override,
                    "ledger run pi --profile work-claude --"
                );
                assert_eq!(data.title, "editable");
                assert_eq!(data.group, "changed/group");
                assert!(!data.structured);
            }
            _ => panic!("expected an editable, submittable form"),
        }
    }
}

#[test]
#[serial]
fn quick_launch_uses_group_context_without_inheriting_a_scratch_directory() {
    let (temp, _guard) = test_home();
    let mut source = Instance::new("source", temp.path().to_str().unwrap());
    source.group_path = "team/project".into();
    seed_profile("test", &[source.clone()]);
    assign("1", "test", None, None);
    let mut view = test_view(Some("test"));
    view.group_by = crate::session::config::GroupByMode::Manual;
    view.selected_session = None;
    view.selected_group = Some("team".into());
    view.open_quick_launch('1');
    let dialog = view.new_dialog.as_ref().unwrap();
    assert_eq!(dialog.path_value(), temp.path().to_str().unwrap());
    assert_eq!(dialog.group_value(), "team");

    view.new_dialog = None;
    source.scratch = true;
    view.instances.insert(source.id.clone(), source.clone());
    view.selected_session = Some(source.id);
    view.selected_group = None;
    view.open_quick_launch('1');
    assert_ne!(
        view.new_dialog.as_ref().unwrap().path_value(),
        temp.path().to_str().unwrap()
    );
}

#[test]
#[serial]
fn quick_launch_refuses_bad_assignments_and_respects_overlay_and_search_routing() {
    let (_temp, _guard) = test_home();
    seed_profile("test", &[]);
    let mut view = test_view(Some("test"));
    for (slot, profile, tool) in [("1", "missing", None), ("2", "test", Some("unavailable"))] {
        assign(slot, profile, tool, None);
        view.open_quick_launch(slot.chars().next().unwrap());
        assert!(view.new_dialog.is_none());
        assert!(view.info_dialog.take().is_some());
    }
    view.open_quick_launch('9');
    assert!(view.info_dialog.take().is_some());
    assert!(view.new_dialog.is_none());

    assign("3", "test", None, None);
    view.creating_stub_id = Some("creating".into());
    view.open_quick_launch('3');
    assert!(view.info_dialog.is_some());
    assert!(view.new_dialog.is_none());
    view.creating_stub_id = None;
    view.handle_key(
        KeyEvent::new(KeyCode::Char('3'), KeyModifiers::CONTROL),
        None,
    );
    assert!(view.new_dialog.is_none(), "an overlay owns its keys");
    view.info_dialog = None;
    view.search_active = true;
    view.handle_key(
        KeyEvent::new(KeyCode::Char('3'), KeyModifiers::CONTROL),
        None,
    );
    assert!(view.new_dialog.is_none(), "search owns its keys");
}

#[test]
fn quick_launch_chords_require_control_digits_and_a_press() {
    use crate::tui::home::quick_launch::shortcut_slot;
    for digit in '1'..='9' {
        assert_eq!(
            shortcut_slot(&KeyEvent::new(KeyCode::Char(digit), KeyModifiers::CONTROL)),
            Some(digit)
        );
    }
    for event in [
        key(KeyCode::Char('5')),
        KeyEvent::new(KeyCode::Char('0'), KeyModifiers::CONTROL),
        KeyEvent::new(
            KeyCode::Char('5'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ),
        KeyEvent::new(
            KeyCode::Char('5'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
        KeyEvent::new_with_kind(
            KeyCode::Char('5'),
            KeyModifiers::CONTROL,
            crossterm::event::KeyEventKind::Repeat,
        ),
        KeyEvent::new_with_kind(
            KeyCode::Char('5'),
            KeyModifiers::CONTROL,
            crossterm::event::KeyEventKind::Release,
        ),
    ] {
        assert_eq!(shortcut_slot(&event), None, "{event:?}");
    }
}
