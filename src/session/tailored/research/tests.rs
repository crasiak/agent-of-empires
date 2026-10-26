use super::*;

fn inputs() -> Inputs {
    Inputs {
        account: CaptureAccount::Work,
        context: "Explicit context only".into(),
        sources: vec![SourceScope {
            category: SourceCategory::Slack,
            scope: "workspace A, channel B; supplied excerpts only".into(),
        }],
        developed_prompt: String::new(),
        backend: None,
        workspace: String::new(),
        permitted_scope: String::new(),
        evaluation: String::new(),
        unknowns_resolution: String::new(),
        public_disclosure: "No private context in public queries".into(),
        consequences_reviewed: false,
    }
}

#[test]
fn review_is_revision_bound_and_original_and_boundaries_survive_edits() {
    let mut draft = Draft::new(
        uuid::Uuid::new_v4().to_string(),
        "Original question".into(),
        inputs(),
    )
    .unwrap();
    assert!(draft.authorize(0).is_err());
    draft
        .apply_development(Development {
            developed_prompt: "Developed question".into(),
            recommendation: Backend::Orx,
            reason: "Literature review".into(),
            unknowns: vec!["Which target?".into()],
        })
        .unwrap();
    assert!(draft.authorize(draft.revision).is_err());
    let mut edit = draft.inputs.clone();
    edit.backend = Some(Backend::Evo);
    edit.workspace = "/tmp".into();
    edit.permitted_scope = "Only baseline worktree".into();
    edit.evaluation = "Baseline score".into();
    edit.unknowns_resolution = "Use explicitly selected workspace".into();
    edit.consequences_reviewed = true;
    let old = draft.revision;
    draft.edit(old, edit).unwrap();
    assert!(draft.authorize(old).is_err());
    let prompt = draft.authorize(draft.revision).unwrap();
    assert!(prompt.contains("Original question"));
    assert!(prompt.contains("workspace A, channel B"));
    assert!(prompt.contains("No private context in public queries"));
    assert!(prompt.contains("discovery and baseline ONLY"));
    assert_eq!(draft.original_prompt, "Original question");
    assert_eq!(draft.state, State::Running);
    assert!(draft.authorize(draft.revision).is_err());
    assert!(draft.edit(draft.revision, draft.inputs.clone()).is_err());
}

#[test]
fn account_scopes_and_typed_model_output_fail_closed() {
    let mut request = inputs();
    request.sources.push(SourceScope {
        category: SourceCategory::Grafana,
        scope: "private".into(),
    });
    assert!(request.validate().is_err());
    for text in [
        "not json",
        r#"{"developed_prompt":"x","recommendation":"shell","reason":"x","unknowns":[]}"#,
        r#"{"developed_prompt":"x","recommendation":"orx","reason":"x","unknowns":[],"command":"sh"}"#,
    ] {
        assert!(developer::parse(text.as_bytes()).is_err());
    }
}

#[test]
fn store_freezes_intent_before_side_effect_and_does_not_retry() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("research.json"));
    let id = uuid::Uuid::new_v4().to_string();
    let draft = Draft::new(id.clone(), "Question".into(), inputs()).unwrap();
    store.create(draft.clone()).unwrap();
    assert_eq!(store.create(draft).unwrap().id, id);
    store
        .change(&id, |draft| {
            draft.apply_development(Development {
                developed_prompt: "Review papers".into(),
                recommendation: Backend::Orx,
                reason: "Research".into(),
                unknowns: vec![],
            })?;
            let mut edit = draft.inputs.clone();
            edit.backend = Some(Backend::Orx);
            edit.workspace = dir.path().to_string_lossy().into();
            edit.permitted_scope = "Read supplied files only".into();
            edit.consequences_reviewed = true;
            draft.edit(draft.revision, edit)?;
            draft.authorize(draft.revision)?;
            Ok(())
        })
        .unwrap();
    let reopened = store.get(&id).unwrap();
    assert_eq!(reopened.state, State::Running);
    assert!(store
        .change(&id, |draft| draft.authorize(draft.revision))
        .is_err());
    assert!(reopened.frozen_prompt.is_some());
}

#[test]
fn concurrent_review_claims_have_exactly_one_winner_and_visibility_survives() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("research.json"));
    let id = uuid::Uuid::new_v4().to_string();
    let mut draft = Draft::new(id.clone(), "Original".into(), inputs()).unwrap();
    draft
        .apply_development(Development {
            developed_prompt: "Reviewed".into(),
            recommendation: Backend::Orx,
            reason: "Papers".into(),
            unknowns: vec![],
        })
        .unwrap();
    draft.inputs.backend = Some(Backend::Orx);
    draft.inputs.workspace = dir.path().to_string_lossy().into();
    draft.inputs.permitted_scope = "Supplied files only".into();
    draft.inputs.consequences_reviewed = true;
    let revision = draft.revision;
    store.create(draft).unwrap();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let gate = gate.clone();
            let store = store.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                gate.wait();
                store.change(&id, |d| d.authorize(revision)).is_ok()
            })
        })
        .collect();
    gate.wait();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| h.join().unwrap() as usize)
            .sum::<usize>(),
        1
    );
    store
        .operate(Operation {
            operation: "visibility".into(),
            id: id.clone(),
            revision: None,
            original_prompt: None,
            inputs: None,
            hidden: Some(true),
        })
        .unwrap();
    let rows = store.history().unwrap();
    assert!(rows[0].hidden && rows[0].research);
    assert!(store.get(&id).unwrap().frozen_prompt.is_some());
    let before = std::fs::read(&store.path).unwrap();
    store
        .operate(Operation {
            operation: "run".into(),
            id,
            revision: Some(revision),
            original_prompt: None,
            inputs: None,
            hidden: None,
        })
        .unwrap();
    assert_eq!(std::fs::read(&store.path).unwrap(), before);
}

#[test]
fn request_identity_must_match_managed_launchers_canonical_uuid_contract() {
    let uuid = uuid::Uuid::new_v4();
    for id in [
        uuid.simple().to_string(),
        uuid.to_string().to_uppercase(),
        uuid.urn().to_string(),
        format!("{{{uuid}}}"),
    ] {
        assert!(Draft::new(id, "Question".into(), inputs()).is_err());
    }
    assert!(Draft::new(uuid.to_string(), "Question".into(), inputs()).is_ok());
}

#[test]
fn exact_frozen_utf8_envelope_is_bounded_before_intent_and_rejection_is_editable() {
    let dir = tempfile::tempdir().unwrap();
    let mut draft = Draft::new(
        uuid::Uuid::new_v4().to_string(),
        "Original".into(),
        inputs(),
    )
    .unwrap();
    draft
        .apply_development(Development {
            developed_prompt: "Developed".into(),
            recommendation: Backend::Evo,
            reason: "Benchmark".into(),
            unknowns: vec![],
        })
        .unwrap();
    draft.inputs.backend = Some(Backend::Evo);
    draft.inputs.workspace = dir.path().to_string_lossy().into();
    draft.inputs.permitted_scope = "Baseline only".into();
    draft.inputs.evaluation = "Score".into();
    draft.inputs.consequences_reviewed = true;
    draft.inputs.developed_prompt = "d".repeat(20_000);
    for context in ["🌱".repeat(8192), "\"\\\n".repeat(10000)] {
        let mut oversized = draft.clone();
        oversized.inputs.context = context;
        assert!(oversized.authorize(oversized.revision).is_err());
        assert_eq!(oversized.state, State::Review);
        assert!(oversized.frozen_prompt.is_none());
        assert!(oversized
            .edit(oversized.revision, draft.inputs.clone())
            .is_ok());
    }
    draft.inputs.developed_prompt = "d".repeat(20_000);
    draft.inputs.context.clear();
    let baseline = draft.clone().authorize(draft.revision).unwrap().len();
    draft.inputs.context = "a".repeat(65_536 - baseline);
    assert_eq!(
        draft.clone().authorize(draft.revision).unwrap().len(),
        65_536
    );
    draft.inputs.context.push('a');
    assert!(draft.authorize(draft.revision).is_err());
    assert_eq!(draft.state, State::Review);
}

#[test]
fn explicit_development_recovery_fences_late_results_but_cannot_recover_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("research.json"));
    let mut draft = Draft::new(
        uuid::Uuid::new_v4().to_string(),
        "Retained original".into(),
        inputs(),
    )
    .unwrap();
    draft.state = State::Developing;
    draft.revision = 3;
    store.create(draft.clone()).unwrap();
    let recovered = store
        .operate(Operation {
            operation: "recover-development".into(),
            id: draft.id.clone(),
            revision: Some(3),
            original_prompt: None,
            inputs: None,
            hidden: None,
        })
        .unwrap();
    assert_eq!(recovered.state, State::Draft);
    assert_eq!(recovered.original_prompt, "Retained original");
    store
        .finish_development(
            &draft.id,
            3,
            Ok(Development {
                developed_prompt: "STALE".into(),
                recommendation: Backend::Orx,
                reason: "late".into(),
                unknowns: vec![],
            }),
        )
        .unwrap();
    assert_eq!(store.get(&draft.id).unwrap().revision, 4);
    assert!(!store
        .get(&draft.id)
        .unwrap()
        .inputs
        .developed_prompt
        .contains("STALE"));
    store
        .change(&draft.id, |d| {
            d.state = State::Developing;
            d.revision = 5;
            Ok(())
        })
        .unwrap();
    store
        .finish_development(&draft.id, 3, Err(anyhow::anyhow!("late failure")))
        .unwrap();
    assert_eq!(store.get(&draft.id).unwrap().state, State::Developing);
    assert_eq!(store.get(&draft.id).unwrap().revision, 5);
    store
        .change(&draft.id, |d| {
            d.state = State::Running;
            Ok(())
        })
        .unwrap();
    assert!(store
        .operate(Operation {
            operation: "recover-development".into(),
            id: draft.id,
            revision: Some(4),
            original_prompt: None,
            inputs: None,
            hidden: None
        })
        .is_err());
}
