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
            digest: String::new(),
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
    fn digest_covers_inline_rules_and_both_files_and_their_absence() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let inline = vec!["id='a', answer='stop'".to_string()];
        let digest = |inline: &[String]| {
            Rules::load(Some(dir.path()), Some(&user), inline)
                .unwrap()
                .digest
        };
        let mut seen = vec![digest(&inline)];
        assert_eq!(seen[0].len(), 64);
        assert_eq!(digest(&inline), seen[0], "stable for unchanged sources");
        seen.push(digest(&[]));
        std::fs::write(&user, "").unwrap();
        seen.push(digest(&inline));
        std::fs::write(&user, "[[rule]]\nid='u'\nanswer='stop'").unwrap();
        seen.push(digest(&inline));
        std::fs::write(dir.path().join("afk-rules.toml"), "").unwrap();
        seen.push(digest(&inline));
        std::fs::write(
            dir.path().join("afk-rules.toml"),
            "[[rule]]\nid='r'\nanswer='approve'",
        )
        .unwrap();
        seen.push(digest(&inline));
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(unique.len(), seen.len(), "every change moves the digest");
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
            digest: String::new(),
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
    fn long_proposal_reason_is_truncated_not_refused() {
        let m = menu(Kind::Confirm, None, &["Yes", "No"]);
        let proposal = Proposal {
            key: Some("1".into()),
            text: None,
            // Three-byte chars, so 1024 is not a char boundary.
            reason: Some("\u{20ac}".repeat(100 * 1024 / 3)),
        };
        let d = decide(&m, &run(Policy::Yolo), &Rules::empty(), Some(&proposal));
        let Decision::Answer { choice, reason, .. } = &d else {
            panic!("expected an answer: {d:?}");
        };
        assert_eq!(choice.as_deref(), Some("1"));
        assert_eq!(reason.len(), 1023);
        assert!(reason.chars().all(|c| c == '\u{20ac}'));
        let dir = tempfile::tempdir().unwrap();
        let anchored = crate::session::anchored_fs::AnchoredDir::open(dir.path()).unwrap();
        let log =
            super::super::log::AnswerLog::new(&anchored, "11111111-1111-1111-1111-111111111111")
                .unwrap();
        log.decision(&serde_json::json!({ "menu": m, "decision": d, "reason": reason }))
            .unwrap();
    }

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

    #[test]
    fn adversarial_subjects_stop_under_rules() {
        let allow_all = rules("[[rule]]\nid='all'\nkind='permission'\nanswer='approve'");
        let cases = [
            (
                "Bash command: git push origin HEAD:main",
                "scope:protected_push",
            ),
            (
                "Bash command: git push origin HEAD:refs/heads/master",
                "scope:protected_push",
            ),
            (
                "Bash command: git push -fu origin x",
                "scope:protected_push",
            ),
            (
                "Bash command: git push -uf origin x",
                "scope:protected_push",
            ),
            (
                "Bash command: git -C dir push -f origin x",
                "scope:protected_push",
            ),
            (
                "Bash command: git -c k=v push --force origin x",
                "scope:protected_push",
            ),
            (
                "Bash command: git push --mirror origin",
                "scope:protected_push",
            ),
            (
                "Bash command: git push --prune origin",
                "scope:protected_push",
            ),
            ("Bash command: git branch -a -D x", "scope:protected_push"),
            (
                "Bash command: git branch -v --delete x",
                "scope:protected_push",
            ),
            (
                "Bash command: npm install -D left-pad",
                "scope:dependencies",
            ),
            ("Bash command: npm i --save x", "scope:dependencies"),
            ("Bash command: yarn add -D x", "scope:dependencies"),
            ("Bash command: pnpm add -D x", "scope:dependencies"),
            ("Bash command: npm install -g x", "scope:dependencies"),
            ("Bash command: poetry add requests", "scope:dependencies"),
            ("Bash command: pdm add requests", "scope:dependencies"),
            ("Bash command: bundle add rails", "scope:dependencies"),
            (
                "Bash command: composer require foo/bar",
                "scope:dependencies",
            ),
            ("Bash command: cargo update", "scope:dependencies"),
            ("Edit file: /repo/Gemfile", "scope:dependencies"),
            ("Edit file: /repo/Gemfile.lock", "scope:dependencies"),
            ("Edit file: /repo/poetry.lock", "scope:dependencies"),
            ("Edit file: /repo/composer.json", "scope:dependencies"),
            ("Edit file: /repo/bun.lockb", "scope:dependencies"),
            ("Bash command: cat a > /etc/x", "scope:outside_repo"),
            ("Bash command: ls && rm -rf /etc/foo", "scope:outside_repo"),
            ("Bash command: grep x f | tee /etc/y", "scope:outside_repo"),
            ("Bash command: cat a;rm /etc/x", "scope:outside_repo"),
            ("Bash command: rm -rf ~", "scope:outside_repo"),
            ("Bash command: rm -rf ~/x", "scope:outside_repo"),
            ("Bash command: rm -rf $HOME/x", "scope:outside_repo"),
            ("Bash command: rm -rf ${HOME}/x", "scope:outside_repo"),
            ("Bash command: rm -rf ~user/x", "scope:outside_repo"),
            ("Bash command: echo x;/etc/y", "scope:outside_repo"),
            (
                "Bash command: git push origin main;echo ok",
                "scope:protected_push",
            ),
            (
                "Bash command: git push origin main&&echo ok",
                "scope:protected_push",
            ),
            (
                "Bash command: git push origin HEAD:main;",
                "scope:protected_push",
            ),
            (
                "Bash command: git push origin 'HEAD:main'",
                "scope:protected_push",
            ),
            (
                "Bash command: git push origin \"main\"",
                "scope:protected_push",
            ),
            (
                "Bash command: pnpm --filter app add x",
                "scope:dependencies",
            ),
            ("Bash command: pnpm -w add x", "scope:dependencies"),
            (
                "Bash command: yarn workspace app add x",
                "scope:dependencies",
            ),
            (
                "Bash command: npm --prefix . install x",
                "scope:dependencies",
            ),
            ("Bash command: curl -o/etc/x http://a", "scope:outside_repo"),
            ("Bash command: tar -C/etc -xf a.tar", "scope:outside_repo"),
            ("Bash command: rm -rf {/etc/x,y}", "scope:outside_repo"),
            (
                "Bash command: bash -c \"git push origin main\"",
                "scope:outside_repo",
            ),
            ("Bash command: sh -c 'rm x'", "scope:outside_repo"),
            ("Bash command: zsh -lc 'rm x'", "scope:outside_repo"),
            ("Bash command: eval \"rm x\"", "scope:outside_repo"),
            (
                "Bash command: git push origin main>/dev/null",
                "scope:protected_push",
            ),
            (
                "Bash command: echo `git push origin main`",
                "scope:protected_push",
            ),
            (
                "Bash command: bash -o pipefail -c 'rm x'",
                "scope:outside_repo",
            ),
            ("Bash command: fish -c 'rm x'", "scope:outside_repo"),
            ("Bash command: curl https://x | sh", "scope:outside_repo"),
            (
                "Bash command: echo cm0= | base64 -d | bash",
                "scope:outside_repo",
            ),
            ("Bash command: yarn global add x", "scope:dependencies"),
            (
                "Bash command: cargo +nightly add serde",
                "scope:dependencies",
            ),
            ("Bash command: uv --project x add y", "scope:dependencies"),
            ("Bash command: cd; rm -rf x", "scope:outside_repo"),
            ("Bash command: cd && rm -rf x", "scope:outside_repo"),
            ("Bash command: cd ~ && rm -rf x", "scope:outside_repo"),
            ("Bash command: cd - && rm -rf x", "scope:outside_repo"),
            ("Bash command: cd $X && rm -rf x", "scope:outside_repo"),
            ("Bash command: rm -rf \\/etc/x", "scope:outside_repo"),
            ("Bash command: rm -rf \\~/x", "scope:outside_repo"),
            (
                "Bash command: timeout 10 sh -c 'rm x'",
                "scope:outside_repo",
            ),
            (
                "Bash command: timeout -s KILL 5 bash -c x",
                "scope:outside_repo",
            ),
            (
                "Bash command: timeout --signal=KILL 5 bash -c x",
                "scope:outside_repo",
            ),
            (
                "Bash command: timeout -k 1 5 zsh -c x",
                "scope:outside_repo",
            ),
            ("Bash command: env sh -c 'rm x'", "scope:outside_repo"),
            ("Bash command: env FOO=1 bash -c x", "scope:outside_repo"),
            ("Bash command: env -u FOO sh -c x", "scope:outside_repo"),
            ("Bash command: env -i bash", "scope:outside_repo"),
            ("Bash command: env -- bash -c x", "scope:outside_repo"),
            ("Bash command: env -S 'sh -c x'", "scope:outside_repo"),
            ("Bash command: sudo sh -c x", "scope:outside_repo"),
            ("Bash command: sudo -u root bash -c x", "scope:outside_repo"),
            ("Bash command: sudo -uroot sh", "scope:outside_repo"),
            ("Bash command: sudo -E -u root sh", "scope:outside_repo"),
            ("Bash command: sudo -- sh", "scope:outside_repo"),
            ("Bash command: sudo -s", "scope:outside_repo"),
            ("Bash command: sudo -i", "scope:outside_repo"),
            ("Bash command: sudo -Es cargo test", "scope:outside_repo"),
            ("Bash command: nice -n 5 sh -c x", "scope:outside_repo"),
            ("Bash command: nice -5 bash -c x", "scope:outside_repo"),
            ("Bash command: nohup bash -c x", "scope:outside_repo"),
            ("Bash command: nohup eval x", "scope:outside_repo"),
            ("Bash command: xargs sh -c x", "scope:outside_repo"),
            ("Bash command: xargs -I {} sh -c x", "scope:outside_repo"),
            ("Bash command: xargs -I{} sh -c x", "scope:outside_repo"),
            ("Bash command: xargs -I '{}' sh -c x", "scope:outside_repo"),
            ("Bash command: xargs -n1 bash -c x", "scope:outside_repo"),
            ("Bash command: xargs -0 -P 4 sh -c x", "scope:outside_repo"),
            ("Bash command: ls | xargs sh -c x", "scope:outside_repo"),
            ("Bash command: stdbuf -o L sh -c x", "scope:outside_repo"),
            ("Bash command: stdbuf -oL bash -c x", "scope:outside_repo"),
            (
                "Bash command: stdbuf --output=L sh -c x",
                "scope:outside_repo",
            ),
            ("Bash command: timeout 5 env sudo sh", "scope:outside_repo"),
            (
                "Bash command: nohup timeout 5 bash -c x",
                "scope:outside_repo",
            ),
            (
                "Bash command: find . -exec sh -c x {} +",
                "scope:outside_repo",
            ),
            (
                "Bash command: find . -name y -execdir bash -c x {} +",
                "scope:outside_repo",
            ),
            (
                "Bash command: find . -exec env sh -c x {} +",
                "scope:outside_repo",
            ),
            (
                "Bash command: find . -exec timeout 5 sh -c x {} +",
                "scope:outside_repo",
            ),
            ("Bash command: npm update", "scope:dependencies"),
            ("Bash command: npm update left-pad", "scope:dependencies"),
            ("Bash command: npm up", "scope:dependencies"),
            ("Bash command: pnpm update", "scope:dependencies"),
            ("Bash command: pnpm up", "scope:dependencies"),
            ("Bash command: yarn upgrade", "scope:dependencies"),
            ("Bash command: yarn up left-pad", "scope:dependencies"),
            ("Bash command: poetry update", "scope:dependencies"),
            ("Bash command: go mod tidy", "scope:dependencies"),
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
    }

    #[test]
    fn ordinary_subjects_still_answer_under_rules() {
        let allow_all = rules("[[rule]]\nid='all'\nkind='permission'\nanswer='approve'");
        let cases = [
            "Bash command: cargo test",
            "Bash command: cargo test 2>/dev/null",
            "Bash command: ls /repo/src",
            "Bash command: cat /etc/hosts",
            "Bash command: git push origin feature/x",
            "Bash command: git push",
            "Bash command: npm test",
            "Bash command: npm run build",
            "Bash command: pnpm test",
            "Bash command: git push origin main-fix",
            "Bash command: git push origin main/x",
            "Bash command: cargo test eval",
            "Bash command: pytest tests/eval/",
            "Bash command: npm install",
            "Bash command: pnpm -w test",
            "Bash command: cd src",
            "Bash command: cd src && cargo test",
            "Bash command: git branch feature/x",
            "Bash command: timeout 600 cargo test",
            "Bash command: timeout -k 5 60 cargo build",
            "Bash command: timeout --signal=KILL 60 cargo test",
            "Bash command: env RUST_LOG=debug cargo test",
            "Bash command: env -u FOO cargo test",
            "Bash command: nice -n 10 cargo build",
            "Bash command: nohup cargo build",
            "Bash command: stdbuf -oL cargo test",
            "Bash command: find src -name '*.rs' -exec grep -l foo {} +",
            "Bash command: find . -type f | xargs grep foo",
            "Bash command: xargs -I {} cp {} dst",
            "Bash command: go test ./...",
            "Bash command: go mod download",
            "Bash command: poetry run pytest",
            "Bash command: yarn test",
            "Bash command: npm run build",
        ];
        for subject in cases {
            assert_eq!(
                choice(&decide(
                    &perm(subject),
                    &run(Policy::Rules),
                    &allow_all,
                    None
                )),
                Some("1"),
                "{subject}"
            );
        }
    }

    #[test]
    fn pi_permission_gate_dialog_stops_under_a_broad_permission_rule() {
        // The menu the extension's dialogMenu builds for Pi's permission-gate example.
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("testdata/pi-permission-gate.json")).unwrap();
        let input: MenuInput = serde_json::from_value(fixture["menu"].clone()).unwrap();
        let broad = rules("[[rule]]\nid='all'\nkind='permission'\nanswer='approve'");
        let menu = Menu::from_input(input.clone(), &RoleWords::default()).unwrap();
        let d = decide(&menu, &run(Policy::Rules), &broad, None);
        assert_eq!(stop(&d), Some("scope:harness_warning"), "{d:?}");
        // Without the warning, the command in the title still stops it.
        let mut quiet = input;
        quiet.warnings.clear();
        let menu = Menu::from_input(quiet, &RoleWords::default()).unwrap();
        assert_eq!(
            stop(&decide(&menu, &run(Policy::Rules), &broad, None)),
            Some("scope:outside_repo")
        );
        // The guide's narrow rule answers a dialog whose message is the command, and not the gate.
        let narrow = rules(
            "[[rule]]\nid='tooling'\nsource='pi_dialog'\nkind='permission'\nsubject_matches=['^(cargo|npm|pnpm) (test|build|check)\\b']\nanswer='approve'",
        );
        let mut allow = menu.clone();
        allow.subject = Some("cargo test".into());
        assert_eq!(
            choice(&decide(&allow, &run(Policy::Rules), &narrow, None)),
            Some("1")
        );
        assert_eq!(
            stop(&decide(&menu, &run(Policy::Rules), &narrow, None)),
            Some("no_rule")
        );
    }

    #[test]
    fn tilde_paths_resolve_against_home_or_count_as_outside() {
        use super::super::scope::outside_repo;
        use std::path::Path;
        let roots = [PathBuf::from("/repo")];
        assert!(outside_repo("Bash command: rm ~/x", &roots, None));
        assert!(!outside_repo(
            "Bash command: rm ~/x",
            &roots,
            Some(Path::new("/repo"))
        ));
        assert!(outside_repo(
            "Bash command: rm ~/x",
            &roots,
            Some(Path::new("/home/u"))
        ));
    }
}

mod log {
    use super::super::decide::Policy;
    use super::super::log::*;
    use crate::session::anchored_fs::AnchoredDir;

    const PERIOD: &str = "11111111-1111-1111-1111-111111111111";

    #[test]
    fn answer_log_appends_decision_then_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let anchored = AnchoredDir::open(dir.path()).unwrap();
        let log = AnswerLog::new(&anchored, PERIOD).unwrap();
        let id = log
            .decision(&serde_json::json!({"decision":"answer","choice":"1"}))
            .unwrap();
        log.delivered(&id, Delivered::Ok).unwrap();
        let text = std::fs::read_to_string(dir.path().join(format!("afk-answers/{PERIOD}.jsonl")))
            .unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["schema"], "aoe.afk-answer.v1");
        assert_eq!(lines[0]["delivered"], "pending");
        assert_eq!(lines[1]["answer_id"], lines[0]["answer_id"]);
        assert_eq!(lines[1]["delivered"], "ok");
    }

    #[test]
    fn answer_log_fits_a_maximal_worst_case_escaped_menu() {
        use super::super::menu::{InputOption, Kind, Menu, MenuInput, RoleWords, Source};
        // Control bytes escape to `\u00XX`, six bytes each: the worst case for every text field.
        let worst = |n: usize| {
            let mut s = String::from("\"\\");
            s.push_str(&"\u{1}".repeat(n - 2));
            s
        };
        let menu = Menu::from_input(
            MenuInput {
                source: Source::PiDialog,
                format: worst(128),
                kind: Kind::Choice,
                question: worst(2048),
                subject: Some(worst(2048)),
                options: (0..32)
                    .map(|i| InputOption {
                        key: format!("{i:02}{}", "\u{1}".repeat(62)),
                        label: worst(256),
                        role: None,
                    })
                    .collect(),
                free_text: true,
                warnings: vec![worst(512); 8],
                deadline_ms: Some(u64::MAX),
                raw: worst(4096),
            },
            &RoleWords::default(),
        )
        .unwrap();
        assert_eq!(menu.raw.len(), super::super::menu::RAW_MAX);
        let record = serde_json::json!({
            "run": PERIOD, "item": PERIOD, "period": PERIOD, "menu": menu,
            "proposal": menu.options[0].key, "decision": "answer", "choice": menu.options[0].key,
            "text": worst(1024), "policy": "rules", "rule": worst(128),
            "checks": [{"id": "outside_repo", "result": "pass"}], "warnings": menu.warnings,
            "reason": worst(512), "delivered": "pending",
        });
        let dir = tempfile::tempdir().unwrap();
        let log = AnswerLog::new(&AnchoredDir::open(dir.path()).unwrap(), PERIOD).unwrap();
        log.decision(&record).unwrap();
        let text = std::fs::read_to_string(dir.path().join(format!("afk-answers/{PERIOD}.jsonl")))
            .unwrap();
        assert!(
            text.len() > 16 * 1024 * 6,
            "record is not worst case: {}",
            text.len()
        );
        assert!(text.len() <= LINE_MAX, "{} > {LINE_MAX}", text.len());
        let line: serde_json::Value = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(line["menu"]["raw"], serde_json::json!(menu.raw));
    }

    #[test]
    fn unrecognised_samples_are_numbered() {
        let dir = tempfile::tempdir().unwrap();
        let log = AnswerLog::new(&AnchoredDir::open(dir.path()).unwrap(), PERIOD).unwrap();
        assert_eq!(
            log.unrecognised("a").unwrap(),
            format!("{PERIOD}-unrecognised-1.txt")
        );
        assert_eq!(
            log.unrecognised("b").unwrap(),
            format!("{PERIOD}-unrecognised-2.txt")
        );
    }

    #[test]
    fn answer_log_refuses_symlinked_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("afk-answers")).unwrap();
        std::fs::set_permissions(
            dir.path().join("afk-answers"),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "/tmp/elsewhere",
            dir.path().join(format!("afk-answers/{PERIOD}.jsonl")),
        )
        .unwrap();
        let log = AnswerLog::new(&AnchoredDir::open(dir.path()).unwrap(), PERIOD).unwrap();
        assert!(log.decision(&serde_json::json!({})).is_err());
    }

    #[test]
    fn run_context_falls_back_to_cwd_outside_git() {
        let dir = tempfile::tempdir().unwrap();
        let input = super::super::run::RunInput {
            period: "p".into(),
            item: None,
            goal: "g".into(),
            policy: Policy::Rules,
            rules: vec![],
            rules_digest: None,
            cwd: dir.path().to_path_buf(),
            requests_used: 0,
            request_limit: 4,
        };
        let ctx = super::super::run::context(&input);
        assert_eq!(ctx.repo_roots, vec![dir.path().canonicalize().unwrap()]);
        assert_eq!(ctx.diff_lines, None);
        assert_eq!(ctx.max_diff_lines, 1500);
    }

    #[test]
    fn run_context_counts_diff_inside_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("a.txt"), "two\nthree\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        let input = super::super::run::RunInput {
            period: "p".into(),
            item: None,
            goal: "g".into(),
            policy: Policy::Rules,
            rules: vec![],
            rules_digest: None,
            cwd: root.join("sub"),
            requests_used: 0,
            request_limit: 4,
        };
        let ctx = super::super::run::context(&input);
        assert_eq!(ctx.repo_roots, vec![root]);
        // One line deleted, two added; no origin/HEAD, so the base is HEAD.
        assert_eq!(ctx.diff_lines, Some(3));
    }
}
