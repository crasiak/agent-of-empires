use super::menu::*;

fn input(labels: &[&str]) -> MenuInput {
    MenuInput {
        source: Source::PiDialog,
        format: "pi.select".into(),
        kind: Kind::Choice,
        question: "Pick".into(),
        subject: None,
        options: labels
            .iter()
            .enumerate()
            .map(|(i, l)| InputOption {
                key: (i + 1).to_string(),
                label: (*l).into(),
                role: None,
            })
            .collect(),
        free_text: false,
        warnings: vec![],
        deadline_ms: None,
        raw: String::new(),
    }
}

#[test]
fn roles_come_from_labels() {
    let w = RoleWords::default();
    assert_eq!(role_for("Yes", &w), Role::Approve);
    assert_eq!(
        role_for("Yes, allow all edits during this session", &w),
        Role::ApproveAlways
    );
    assert_eq!(role_for("Always allow", &w), Role::ApproveAlways);
    assert_eq!(role_for("No", &w), Role::Deny);
    assert_eq!(role_for("Revise", &w), Role::Revise);
    assert_eq!(role_for("Keep planning", &w), Role::Revise);
    assert_eq!(role_for("Banana", &w), Role::Other);
}

#[test]
fn from_input_assigns_roles_and_bounds_raw() {
    let mut i = input(&["Approve plan", "Revise", "Something else"]);
    i.raw = "x".repeat(RAW_MAX + 10);
    let m = Menu::from_input(i, &RoleWords::default()).unwrap();
    assert_eq!(m.schema, MENU_SCHEMA);
    assert_eq!(
        m.options.iter().map(|o| o.role).collect::<Vec<_>>(),
        vec![Role::Approve, Role::Revise, Role::Other]
    );
    assert_eq!(m.raw.len(), RAW_MAX);
}

#[test]
fn from_input_rejects_bad_menus() {
    let w = RoleWords::default();
    assert!(
        Menu::from_input(input(&[]), &w).is_err(),
        "choice without options"
    );
    let mut dup = input(&["Yes", "No"]);
    dup.options[1].key = "1".into();
    assert!(Menu::from_input(dup, &w).is_err(), "duplicate keys");
    let mut free = input(&[]);
    free.kind = Kind::Input;
    free.free_text = true;
    assert!(
        Menu::from_input(free, &w).is_ok(),
        "input kind may have no options"
    );
}

#[test]
fn explicit_role_is_kept() {
    let mut i = input(&["Proceed"]);
    i.options[0].role = Some(Role::Deny);
    let m = Menu::from_input(i, &RoleWords::default()).unwrap();
    assert_eq!(m.options[0].role, Role::Deny);
}

use super::formats::*;
use std::path::Path;

fn corpus() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/afk/menu-formats/fixtures")
}

#[test]
fn every_builtin_format_loads() {
    let (f, warnings) = Formats::load(None);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        f.ids(),
        vec![
            "text.bracket-shortkey",
            "text.numbered-list",
            "text.yes-no",
            "text.select-prompt"
        ]
    );
}

#[test]
fn format_corpus_matches_expected_menus() {
    let (f, _) = Formats::load(None);
    let mut checked = 0;
    for dir in std::fs::read_dir(corpus()).unwrap() {
        let dir = dir.unwrap().path();
        let id = dir.file_name().unwrap().to_string_lossy().to_string();
        for case in std::fs::read_dir(&dir).unwrap() {
            let case = case.unwrap().path();
            if case.extension().is_none_or(|e| e != "txt") {
                continue;
            }
            let text = std::fs::read_to_string(&case).unwrap();
            if id == "_negative" {
                for s in [Source::AgentText, Source::BashPrompt, Source::Pane] {
                    assert!(f.recognise(s, &text).is_none(), "{case:?} matched");
                }
            } else {
                let mut want: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(case.with_extension("json")).unwrap(),
                )
                .unwrap();
                let source: Source = serde_json::from_value(want["source"].clone()).unwrap();
                let got = f
                    .recognise(source, &text)
                    .unwrap_or_else(|| panic!("{case:?} not recognised"));
                let mut got = serde_json::to_value(got).unwrap();
                got.as_object_mut().unwrap().remove("raw");
                want.as_object_mut().unwrap().remove("raw");
                assert_eq!(got, want, "{case:?}");
            }
            checked += 1;
        }
    }
    assert!(checked >= 9, "corpus too small: {checked}");
}

#[test]
fn invalid_format_is_skipped_and_others_load() {
    let files = vec![
        (
            "bad.toml".to_string(),
            "id = 'x'\nkind = 'nope'".to_string(),
        ),
        (
            "bad-regex.toml".to_string(),
            "id='r'\nsources=['agent_text']\nkind='choice'\noption_pattern='('".to_string(),
        ),
        ("ok.toml".to_string(), BUILTIN[2].1.to_string()),
    ];
    let (f, warnings) = Formats::from_sources(&files);
    assert_eq!(warnings.len(), 2);
    assert_eq!(f.ids(), vec!["text.yes-no"]);
}

#[test]
fn user_format_overrides_builtin_with_same_id() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("mine.toml"),
        "id='text.yes-no'\nsources=['agent_text']\nkind='confirm'\ntrigger='OK\\?$'\n[[option]]\nkey='y'\nlabel='Yes'\n[[option]]\nkey='n'\nlabel='No'",
    )
    .unwrap();
    let (f, warnings) = Formats::load(Some(dir.path()));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(f.recognise(Source::AgentText, "Continue? (y/N)").is_none());
    assert!(f.recognise(Source::AgentText, "All good. OK?").is_some());
}

#[test]
fn format_only_matches_its_sources() {
    let (f, _) = Formats::load(None);
    assert!(f
        .recognise(Source::AgentText, "? Pick a template ›\n❯ react\n  vue")
        .is_none());
    assert!(f
        .recognise(Source::BashPrompt, "? Pick a template ›\n❯ react\n  vue")
        .is_some());
}
