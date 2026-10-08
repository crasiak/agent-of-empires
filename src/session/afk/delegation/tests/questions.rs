use super::*;

fn identity() -> QuestionIdentity {
    serde_json::from_value(json!({"operation":uuid::Uuid::new_v4().to_string(),"tool_call_id":"ordinary-question",
        "interaction_id":uuid::Uuid::new_v4().to_string(),"kind":"input","arguments_hash":hash("private observed arguments"),"context_ref":"native-entry"})).unwrap()
}
fn save_question_grant(store: &Store, w: &mut Window) {
    w.grant.question_deferrals = 1;
    Book {
        version: PROTOCOL,
        windows: vec![w.clone()],
    }
    .save(store)
    .unwrap();
}
#[test]
fn question_admission_intent_outcome_and_return_are_separate_locked_facts() {
    let (_app, _root, store, mut w) = setup(2);
    let q = identity();
    let admit = || Request::QuestionAdmit {
        window: w.id.clone(),
        identity: q.clone(),
    };
    assert!(execute(&store, &w.binding, &w.generation, admit(), 2000).is_err());
    w.grant.question_deferrals = 2;
    assert!(w.grant.validate().is_err());
    save_question_grant(&store, &mut w);
    let admit = || Request::QuestionAdmit {
        window: w.id.clone(),
        identity: q.clone(),
    };
    let receipt = execute(&store, &w.binding, &w.generation, admit(), 2000).unwrap();
    assert_eq!(
        execute(&store, &w.binding, &w.generation, admit(), 3000).unwrap(),
        receipt
    );
    assert!(execute(
        &store,
        &w.binding,
        &w.generation,
        Request::QuestionAdmit {
            window: w.id.clone(),
            identity: identity()
        },
        2000
    )
    .is_err());
    assert!(execute(
        &store,
        &w.binding,
        &uuid::Uuid::new_v4().to_string(),
        admit(),
        2000
    )
    .is_err());
    assert!(execute(&store, &w.binding, &w.generation, admit(), 61000).is_err());
    assert_eq!(
        super::super::questions::guards(&store, &w.binding).unwrap()[0]["emission_intent"],
        false
    );
    let intent = || Request::QuestionIntent {
        window: w.id.clone(),
        identity: q.clone(),
    };
    execute(&store, &w.binding, &w.generation, intent(), 2000).unwrap();
    assert!(execute(&store, &w.binding, &w.generation, intent(), 2000).is_err());
    assert_eq!(
        super::super::questions::guards(&store, &w.binding)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let request = uuid::Uuid::new_v4().to_string();
    let reserve = || Request::Reserve {
        window: w.id.clone(),
        request: request.clone(),
        continuation: None,
        question: Some(q.clone()),
    };
    assert!(execute(&store, &w.binding, &w.generation, reserve(), 2000).is_err());
    execute(
        &store,
        &w.binding,
        &w.generation,
        Request::QuestionOutcome {
            window: w.id.clone(),
            identity: q.clone(),
            outcome: QuestionOutcome::Accepted,
        },
        2000,
    )
    .unwrap();
    assert!(execute(
        &store,
        &w.binding,
        &w.generation,
        Request::QuestionOutcome {
            window: w.id.clone(),
            identity: q.clone(),
            outcome: QuestionOutcome::Ordinary
        },
        2000
    )
    .is_err());
    let view = snapshot(&store).unwrap();
    assert_eq!(view["reservations_used"], 0);
    assert_eq!(view["nudges_used"], 0);
    assert_eq!(view["question_deferrals_used"], 1);
    let result = execute(&store, &w.binding, &w.generation, reserve(), 2000).unwrap();
    assert_eq!(result["human_required"], true);
    assert!(result["unanswered_gate"].is_object());
    assert!(super::super::questions::guards(&store, &w.binding)
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    let mut d = decision();
    d["depends_on"] = json!([receipt["identity"]["operation"]]);
    assert_eq!(
        record(&store, &w, &request, d).unwrap()["disposition"],
        "human_required"
    );
    assert!(Book::load(&store).unwrap().windows[0].permit.is_none());
}
#[test]
fn question_unknown_survives_off_restart_pruning_and_requires_exact_recovery() {
    for outcome in [
        None,
        Some(QuestionOutcome::Accepted),
        Some(QuestionOutcome::Unknown),
    ] {
        let (_app, _root, store, mut w) = setup(1);
        save_question_grant(&store, &mut w);
        let q = identity();
        execute(
            &store,
            &w.binding,
            &w.generation,
            Request::QuestionAdmit {
                window: w.id.clone(),
                identity: q.clone(),
            },
            2000,
        )
        .unwrap();
        execute(
            &store,
            &w.binding,
            &w.generation,
            Request::QuestionIntent {
                window: w.id.clone(),
                identity: q.clone(),
            },
            2000,
        )
        .unwrap();
        stop(&store).unwrap();
        if let Some(outcome) = outcome {
            execute(
                &store,
                &w.binding,
                &w.generation,
                Request::QuestionOutcome {
                    window: w.id.clone(),
                    identity: q.clone(),
                    outcome,
                },
                62000,
            )
            .unwrap();
        }
        let mut book = Book::load(&store).unwrap();
        book.prune(RETENTION_MS * 2);
        book.save(&store).unwrap();
        let mut new_binding = w.binding.clone();
        new_binding.launch_id = uuid::Uuid::new_v4().to_string();
        let guards = super::super::questions::guards(&store, &new_binding).unwrap();
        assert_eq!(guards.as_array().unwrap().len(), 1);
        assert!(!serde_json::to_string(&book)
            .unwrap()
            .contains("private observed arguments"));
        assert!(
            super::super::questions::recover(&store, &new_binding, &w.id, &identity(), || Ok(()))
                .is_err()
        );
        assert!(execute(
            &store,
            &w.binding,
            &w.generation,
            Request::Reserve {
                window: w.id.clone(),
                request: uuid::Uuid::new_v4().to_string(),
                continuation: None,
                question: Some(q.clone())
            },
            2000
        )
        .is_err());
        assert!(
            super::super::questions::recover(&store, &new_binding, &w.id, &q, || {
                anyhow::bail!("current binding changed under lock")
            })
            .is_err()
        );
        assert_eq!(
            super::super::questions::guards(&store, &new_binding)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        super::super::questions::recover(&store, &new_binding, &w.id, &q, || Ok(())).unwrap();
        assert!(super::super::questions::guards(&store, &new_binding)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(snapshot(&store).unwrap()["state"], "ended");
        assert_eq!(snapshot(&store).unwrap()["question_deferrals_used"], 1);
    }
}

#[test]
fn actual_native_question_sdk_and_rust_ledger() {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    let (Ok(source), Ok(_package)) = (
        std::env::var("AOE_PI_SOURCE_ROOT"),
        std::env::var("AOE_ASK_USER_SOURCE_ROOT"),
    ) else {
        eprintln!(
            "Set AOE_PI_SOURCE_ROOT and AOE_ASK_USER_SOURCE_ROOT for actual question integration"
        );
        return;
    };
    for scenario in [
        "input",
        "options",
        "fullscreen",
        "mixed",
        "lost-reply",
        "ledger-failed",
        "admission-failed",
        "restore-readable",
        "restore-unavailable",
        "restore-corrupt",
    ] {
        let (_app, _root, store, mut w) = setup(1);
        let control_root = tempfile::tempdir().unwrap();
        let control = Store::open(control_root.path(), "test", true).unwrap();
        let mut child = Command::new("node")
            .args([
                "--import",
                &format!("{source}/node_modules/tsx/dist/loader.mjs"),
                "assets/session/aoe-afk-questions.test.mjs",
                "--host",
                scenario,
            ])
            .env("TSX_TSCONFIG_PATH", format!("{source}/tsconfig.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let output = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut initialized = false;
        let mut observed = false;
        for line in output.lines() {
            let value: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if value.get("result").is_some() {
                observed = true;
                break;
            }
            if !initialized {
                w.binding = serde_json::from_value(value["binding"].clone()).unwrap();
                w.generation = value["generation"].as_str().unwrap().into();
                w.grant = serde_json::from_value(value["grant"].clone()).unwrap();
                w.issued_at_ms = now_ms();
                w.expires_at_ms = w.issued_at_ms + 60000;
                Book {
                    version: PROTOCOL,
                    windows: vec![w.clone()],
                }
                .save(&store)
                .unwrap();
                initialized = true;
            }
            if value["packet"]["fixture"] == "corrupt" {
                store
                    .write("ledger.json", &json!({"version":4,"windows":"corrupt"}))
                    .unwrap();
                input.write_all(b"{\"ok\":true,\"value\":null}\n").unwrap();
                input.flush().unwrap();
                continue;
            }
            let generation = value["generation"].as_str().unwrap();
            let mut reply = Vec::new();
            super::super::super::bridge::serve(
                &control,
                Some(&store),
                &w.binding,
                generation,
                Cursor::new(serde_json::to_string(&value["packet"]).unwrap() + "\n"),
                &mut reply,
                || Ok(()),
            )
            .unwrap();
            input.write_all(&reply).unwrap();
            input.flush().unwrap();
        }
        drop(input);
        assert!(child.wait().unwrap().success(), "{scenario}");
        assert!(observed);
        if scenario == "restore-corrupt" {
            assert!(Book::load(&store).is_err());
            continue;
        }
        let view = snapshot(&store).unwrap();
        assert_eq!(view["question_deferrals_used"], 1, "{scenario}");
        assert_eq!(
            view["reservations_used"],
            if ["input", "options", "fullscreen"].contains(&scenario) {
                1
            } else {
                0
            },
            "{scenario}"
        );
        assert_eq!(view["nudges_used"], 0);
        assert!(view["action"].is_null());
    }
}
