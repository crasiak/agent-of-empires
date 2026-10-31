use super::*;
use crate::session::test_support::isolate_app_dir_at;
use serial_test::serial;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
#[serial]
fn ledger_profile_is_visible_editable_and_tracks_the_selected_agent() {
    let temp = tempfile::TempDir::new().unwrap();
    let _guard = isolate_app_dir_at(temp.path());
    let mut dialog = NewSessionDialog::new_with_tools(
        vec!["pi", "claude", "gemini"],
        temp.path().to_str().unwrap().into(),
    );
    dialog.set_structured_capable(true);
    dialog.structured_default = true;
    dialog.set_ledger_profile("personal".into());
    assert!(!dialog.structured_enabled);
    assert_eq!(dialog.field_indices().structured, FieldIndices::ABSENT);

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
    let theme = crate::tui::styles::Theme::default();
    terminal
        .draw(|frame| dialog.render(frame, frame.area(), &theme))
        .unwrap();
    let field = dialog.field_indices().ledger_profile;
    let rect = dialog
        .focusable_rects
        .iter()
        .find(|(index, _)| *index == field)
        .unwrap()
        .1;
    dialog.handle_click(rect.x + 1, rect.y);
    assert_eq!(dialog.focused_field, field);
    for _ in 0.."personal".len() {
        dialog.handle_key(key(KeyCode::Backspace));
    }
    dialog.handle_paste("work-claude");
    dialog.set_tool("claude");
    match dialog.handle_key(key(KeyCode::Enter)) {
        DialogResult::Submit(data) => {
            assert_eq!(
                data.command_override,
                "ledger run claude --profile work-claude --"
            );
            assert!(!data.structured);
        }
        _ => panic!("edited Ledger profile must submit"),
    }

    dialog.set_tool("gemini");
    assert!(matches!(
        dialog.handle_key(key(KeyCode::Enter)),
        DialogResult::Continue
    ));
    assert!(dialog
        .error_message
        .as_deref()
        .unwrap()
        .contains("Ledger quick launch requires"));

    dialog.set_tool("pi");
    dialog.ledger_profile = Some(Input::new("work; touch /tmp/bad".into()));
    assert!(matches!(
        dialog.handle_key(key(KeyCode::Enter)),
        DialogResult::Continue
    ));
    assert!(dialog
        .error_message
        .as_deref()
        .unwrap()
        .contains("Ledger profile"));
    dialog.ledger_profile = Some(Input::default());
    dialog.command_override = Input::new("pi-normal-wrapper".into());
    match dialog.handle_key(key(KeyCode::Enter)) {
        DialogResult::Submit(data) => assert_eq!(data.command_override, "pi-normal-wrapper"),
        _ => panic!("clearing the Ledger profile restores the normal command"),
    }
}
