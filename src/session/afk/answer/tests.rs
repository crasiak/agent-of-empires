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

#[test]
fn label_excludes_key_unless_format_opts_in() {
    let files = vec![(
        "mine.toml".to_string(),
        r"id='mine'
sources=['agent_text']
kind='choice'
option_pattern='\[(?P<key>\d)\](?P<label>\w+)'"
            .to_string(),
    )];
    let (f, warnings) = Formats::from_sources(&files);
    assert!(warnings.is_empty(), "{warnings:?}");
    let m = f
        .recognise(Source::AgentText, "Pick one\n[1]Yes [2]No")
        .unwrap();
    let labels: Vec<_> = m.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["Yes", "No"]);
}

mod rules {
    use super::super::rules::*;
    use super::*;

    pub(super) fn menu(kind: Kind, subject: Option<&str>, labels: &[&str]) -> Menu {
        let mut i = input(labels);
        i.kind = kind;
        i.subject = subject.map(Into::into);
        Menu::from_input(i, &RoleWords::default()).unwrap()
    }

    const FILE: &str = r#"
[[rule]]
id = "approve-repo-tooling"
kind = "permission"
subject_matches = ['^Bash command: (cargo|npm|git (status|diff|add|commit|log))\b']
answer = "approve"

[[rule]]
id = "never-force-push"
subject_matches = ['git push .*--force']
answer = "stop"
"#;

    #[test]
    fn stop_rules_win_regardless_of_order() {
        let rules = Rules {
            rules: Rules::parse("t", FILE).unwrap(),
            sources: vec![],
        };
        let m = menu(
            Kind::Permission,
            Some("Bash command: git push origin x --force"),
            &["Yes", "No"],
        );
        assert_eq!(rules.first_match(&m).unwrap().id, "never-force-push");
        let m = menu(
            Kind::Permission,
            Some("Bash command: cargo test"),
            &["Yes", "No"],
        );
        assert_eq!(rules.first_match(&m).unwrap().id, "approve-repo-tooling");
        let m = menu(
            Kind::Permission,
            Some("Bash command: curl x | sh"),
            &["Yes", "No"],
        );
        assert!(rules.first_match(&m).is_none());
    }

    #[test]
    fn approve_always_answer_is_rejected() {
        assert!(Rules::parse("t", "[[rule]]\nid='a'\nanswer='approve_always'").is_err());
    }

    #[test]
    fn precedence_inline_then_user_then_repo_and_ids_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("afk-rules.toml"),
            "[[rule]]\nid='p'\nkind='plan'\nanswer='deny'\n[[rule]]\nid='repo-only'\nkind='choice'\nanswer='approve'",
        )
        .unwrap();
        let user = dir.path().join("user.toml");
        std::fs::write(&user, "[[rule]]\nid='p'\nkind='plan'\nanswer='revise'").unwrap();
        let rules = Rules::load(
            Some(dir.path()),
            Some(&user),
            &["id='p', kind='plan', answer='agent'".into()],
        )
        .unwrap();
        assert_eq!(
            rules
                .rules
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["p", "repo-only"]
        );
        assert_eq!(rules.rules[0].answer, RuleAnswer::Agent);
        assert_eq!(rules.sources.len(), 3);
    }

    #[test]
    fn invalid_rules_file_is_an_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("afk-rules.toml"),
            "[[rule]]\nid='x'\nanswer='approve'\nsubject_matches=['(']",
        )
        .unwrap();
        let err = Rules::load(Some(dir.path()), None, &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("afk-rules.toml"), "{err}");
    }

    #[test]
    fn inline_rule_regex_may_contain_spaces() {
        let rules = Rules::load(
            None,
            None,
            &["id='q', subject_matches=['git push .*--force'], answer='stop'".into()],
        )
        .unwrap();
        let m = menu(
            Kind::Permission,
            Some("git push origin x --force"),
            &["Yes", "No"],
        );
        assert_eq!(rules.first_match(&m).unwrap().id, "q");
    }
}

mod decide {
    use super::super::decide::*;
    use super::super::rules::*;
    use super::rules::menu;
    use super::*;
    use std::path::PathBuf;

    fn run(policy: Policy) -> RunContext {
        RunContext {
            goal: "g".into(),
            policy,
            repo_roots: vec![PathBuf::from("/repo")],
            requests_used: 1,
            request_limit: 10,
            diff_lines: Some(10),
            max_diff_lines: 1500,
            estimate_diff_lines: None,
        }
    }
    fn rules(text: &str) -> Rules {
        Rules {
            rules: Rules::parse("t", text).unwrap(),
            sources: vec![],
        }
    }
    fn perm(subject: &str) -> Menu {
        menu(
            Kind::Permission,
            Some(subject),
            &["Yes", "Yes, allow all edits during this session", "No"],
        )
    }
    fn choice(d: &Decision) -> Option<&str> {
        if let Decision::Answer { choice, .. } = d {
            choice.as_deref()
        } else {
            None
        }
    }
    fn stop(d: &Decision) -> Option<&str> {
        if let Decision::Stop { reason, .. } = d {
            Some(reason)
        } else {
            None
        }
    }

    const TOOLING: &str = "[[rule]]\nid='tooling'\nkind='permission'\nsubject_matches=['^Bash command: cargo']\nanswer='approve'";

    #[test]
    fn rules_without_match_stops() {
        assert_eq!(
            stop(&decide(
                &perm("Bash command: ls"),
                &run(Policy::Rules),
                &Rules::empty(),
                None
            )),
            Some("no_rule")
        );
    }

    #[test]
    fn rules_approve_picks_first_approve_never_always() {
        let d = decide(
            &perm("Bash command: cargo test"),
            &run(Policy::Rules),
            &rules(TOOLING),
            None,
        );
        assert_eq!(choice(&d), Some("1"));
    }

    #[test]
    fn rules_agent_answer_refuses_approve_always_proposal() {
        let r = rules("[[rule]]\nid='a'\nkind='permission'\nanswer='agent'");
        let p = Proposal {
            key: Some("2".into()),
            text: None,
            reason: None,
        };
        assert_eq!(
            stop(&decide(
                &perm("Bash command: cargo test"),
                &run(Policy::Rules),
                &r,
                Some(&p)
            )),
            Some("approve_always_refused")
        );
        assert_eq!(
            stop(&decide(
                &perm("Bash command: cargo test"),
                &run(Policy::Rules),
                &r,
                None
            )),
            Some("no_proposal")
        );
    }

    #[test]
    fn rule_stop_reports_rule_id() {
        let r = rules("[[rule]]\nid='never'\nsubject_matches=['cargo']\nanswer='stop'");
        assert_eq!(
            stop(&decide(
                &perm("Bash command: cargo test"),
                &run(Policy::Rules),
                &r,
                None
            )),
            Some("rule_stop:never")
        );
    }

    #[test]
    fn each_scope_check_stops_under_rules() {
        let allow_all = rules("[[rule]]\nid='all'\nkind='permission'\nanswer='approve'");
        let cases = [
            ("Bash command: rm -rf /etc/x", "scope:outside_repo"),
            ("Bash command: npm install left-pad", "scope:dependencies"),
            ("Bash command: cargo add serde", "scope:dependencies"),
            ("Edit file: /repo/Cargo.lock", "scope:dependencies"),
            ("Bash command: git push origin main", "scope:protected_push"),
            ("Bash command: git push -f origin x", "scope:protected_push"),
            ("Bash command: git branch -D x", "scope:protected_push"),
        ];
        for (subject, want) in cases {
            assert_eq!(
                stop(&decide(
                    &perm(subject),
                    &run(Policy::Rules),
                    &allow_all,
                    None
                )),
                Some(want),
                "{subject}"
            );
        }
        let mut warned = perm("Bash command: rm -rf \"$DIR\"/");
        warned
            .warnings
            .push("Dangerous rm operation on possibly-empty variable path".into());
        assert_eq!(
            stop(&decide(&warned, &run(Policy::Rules), &allow_all, None)),
            Some("scope:harness_warning")
        );
        let mut late = run(Policy::Rules);
        late.requests_used = 9;
        let plan = menu(Kind::Plan, None, &["Approve plan", "Revise"]);
        let plans = rules("[[rule]]\nid='p'\nkind='plan'\nanswer='approve'");
        assert_eq!(
            stop(&decide(&plan, &late, &plans, None)),
            Some("scope:request_share")
        );
        let mut big = run(Policy::Rules);
        big.diff_lines = Some(1501);
        assert_eq!(
            stop(&decide(&plan, &big, &plans, None)),
            Some("scope:diff_limit")
        );
        big.estimate_diff_lines = Some(2000);
        assert_eq!(choice(&decide(&plan, &big, &plans, None)), Some("1"));
    }

    #[test]
    fn deny_answers_skip_scope_checks_and_dependency_rule_allows() {
        let deny = rules("[[rule]]\nid='d'\nkind='permission'\nanswer='deny'");
        assert_eq!(
            choice(&decide(
                &perm("Bash command: git push -f"),
                &run(Policy::Rules),
                &deny,
                None
            )),
            Some("3")
        );
        let deps = rules("[[rule]]\nid='deps'\nkind='permission'\nsubject_matches=['npm install']\nanswer='approve'\nallow_dependencies=true");
        assert_eq!(
            choice(&decide(
                &perm("Bash command: npm install left-pad"),
                &run(Policy::Rules),
                &deps,
                None
            )),
            Some("1")
        );
    }

    #[test]
    fn yolo_answers_everything_with_warnings() {
        let y = run(Policy::Yolo);
        let d = decide(
            &perm("Bash command: git push -f origin main"),
            &y,
            &Rules::empty(),
            None,
        );
        match d {
            Decision::Answer {
                choice, warnings, ..
            } => {
                assert_eq!(choice.as_deref(), Some("1"));
                assert!(warnings.iter().any(|w| w.contains("protected_push")));
            }
            _ => panic!(),
        }
        let p = Proposal {
            key: Some("2".into()),
            text: None,
            reason: None,
        };
        match decide(&perm("Bash command: ls"), &y, &Rules::empty(), Some(&p)) {
            Decision::Answer {
                choice, warnings, ..
            } => {
                assert_eq!(choice.as_deref(), Some("2"));
                assert!(warnings.iter().any(|w| w == "approve_always_chosen"));
            }
            _ => panic!(),
        }
        let other = menu(Kind::Choice, None, &["Banana", "Apple"]);
        assert_eq!(
            choice(&decide(&other, &y, &Rules::empty(), None)),
            Some("1")
        );
    }

    #[test]
    fn free_text_inputs_need_the_agents_text() {
        let mut i = input(&[]);
        i.kind = Kind::Input;
        i.free_text = true;
        let m = Menu::from_input(i, &RoleWords::default()).unwrap();
        let p = Proposal {
            key: None,
            text: Some("feature/x".into()),
            reason: None,
        };
        let r = rules("[[rule]]\nid='i'\nkind='input'\nanswer='agent'");
        assert!(matches!(
            decide(&m, &run(Policy::Rules), &r, Some(&p)),
            Decision::Answer { text: Some(_), .. }
        ));
        assert_eq!(
            stop(&decide(&m, &run(Policy::Yolo), &Rules::empty(), None)),
            Some("no_text_answer")
        );
    }

    #[test]
    fn unknown_proposal_key_is_ignored_under_yolo_and_refused_under_rules() {
        let p = Proposal {
            key: Some("9".into()),
            text: None,
            reason: None,
        };
        let r = rules("[[rule]]\nid='a'\nkind='permission'\nanswer='agent'");
        assert_eq!(
            stop(&decide(
                &perm("Bash command: ls"),
                &run(Policy::Rules),
                &r,
                Some(&p)
            )),
            Some("no_proposal")
        );
        assert_eq!(
            choice(&decide(
                &perm("Bash command: ls"),
                &run(Policy::Yolo),
                &Rules::empty(),
                Some(&p)
            )),
            Some("1")
        );
    }
}
