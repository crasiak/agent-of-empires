use super::*;
use std::{collections::BTreeMap, path::Path};

pub(super) fn bundle() -> Bundle {
    Bundle {
        files: BTreeMap::from([
            (
                "SKILL.md".into(),
                "---\nname: example\ndescription: Example workflow\n---\nUse explicit inputs.\n"
                    .into(),
            ),
            ("scripts/check.py".into(), "print('not executed')\n".into()),
        ]),
    }
}
pub(super) fn repository(path: &Path) -> git2::Repository {
    let repo = git2::Repository::init(path).unwrap();
    let blob = repo.blob(b"preserve unrelated\n").unwrap();
    let mut tree = repo.treebuilder(None).unwrap();
    tree.insert("unrelated.txt", blob, 0o100644).unwrap();
    let oid = tree.write().unwrap();
    drop(tree);
    let tree = repo.find_tree(oid).unwrap();
    let sig = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
        .unwrap();
    drop(tree);
    repo.checkout_head(None).unwrap();
    repo
}
pub(super) fn envelope(path: &Path) -> CreateEnvelope {
    CreateEnvelope {
        account: CaptureAccount::Work,
        idea: "A precise skill".into(),
        repository: path.canonicalize().unwrap(),
        slug: "example".into(),
        source_scope: "Work-approved public source".into(),
    }
}

#[test]
fn bundle_reuses_skill_parser_and_refuses_escapes_conflicts_and_unbounded_data() {
    let valid = bundle();
    valid.validate("example").unwrap();
    for path in [
        "../outside",
        "/absolute",
        "a//b",
        "a/./b",
        "a/../b",
        "a\\b",
        ".git/config",
        "a/.Git/config",
        "a\nname",
        "scripts/check.py/child",
        "SKILL.md/x",
        "skill.md",
        "SCRIPTS/check.py/child",
    ] {
        let mut bad = valid.clone();
        bad.files.insert(path.into(), "data".into());
        assert!(bad.validate("example").is_err(), "{path}");
    }
    let mut alias = valid.clone();
    alias.files.insert("scripts/a.txt".into(), "one".into());
    alias.files.insert("SCRIPTS/b.txt".into(), "two".into());
    assert!(alias.validate("example").is_err());
    let mut bad = valid.clone();
    bad.files.insert(
        "SKILL.md".into(),
        "---\nname: other\ndescription: mismatch\n---\n".into(),
    );
    assert!(bad.validate("example").is_err());
    bad.files
        .insert("SKILL.md".into(), "not frontmatter".into());
    assert!(bad.validate("example").is_err());
    let mut bad = valid;
    bad.files.insert("large".into(), "x".repeat(65_537));
    assert!(bad.validate("example").is_err());
    assert!(serde_json::from_value::<Proposal>(
        serde_json::json!({"reply":"yes","bundle":bundle(),"approval":true})
    )
    .is_err());
}

#[test]
fn complete_create_envelope_and_revisions_are_durable() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    repository(&repo);
    let store = Store::at(dir.path().join("workshop.json"));
    let id = uuid::Uuid::new_v4().to_string();
    let create = envelope(&repo);
    let first = store.create(id.clone(), create.clone()).unwrap();
    assert_eq!(
        store.create(id.clone(), create.clone()).unwrap().revision,
        first.revision
    );
    for changed in [
        CreateEnvelope {
            account: CaptureAccount::Personal,
            ..create.clone()
        },
        CreateEnvelope {
            source_scope: "changed".into(),
            ..create.clone()
        },
        CreateEnvelope {
            slug: "other".into(),
            ..create.clone()
        },
        CreateEnvelope {
            idea: "changed".into(),
            ..create.clone()
        },
    ] {
        assert!(store.create(id.clone(), changed).is_err());
    }
    store
        .operate(Operation::Propose {
            id: id.clone(),
            revision: 0,
            bundle: bundle(),
        })
        .unwrap();
    assert!(store
        .operate(Operation::Propose {
            id: id.clone(),
            revision: 0,
            bundle: bundle()
        })
        .is_err());
    let reopened = Store::at(dir.path().join("workshop.json"))
        .get(&id)
        .unwrap();
    assert_eq!(reopened.envelope, create);
    assert_eq!(reopened.revision, 1);
}

#[test]
fn real_git_review_and_commit_preserve_unrelated_index_and_reconcile_exact_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let repo = repository(&path);
    let store = Store::at(dir.path().join("workshop.json"));
    let id = uuid::Uuid::new_v4().to_string();
    store.create(id.clone(), envelope(&path)).unwrap();
    let draft = store
        .operate(Operation::Propose {
            id: id.clone(),
            revision: 0,
            bundle: bundle(),
        })
        .unwrap();
    let index_before = std::fs::read(repo.path().join("index")).unwrap();
    let review = store
        .operate(Operation::Review {
            id: id.clone(),
            revision: draft.revision,
        })
        .unwrap();
    let frozen = review.review.as_ref().unwrap();
    assert!(frozen.diff.contains("scripts/check.py"));
    assert!(frozen.diff.contains("SKILL.md"));
    assert!(frozen.evidence.iter().any(|e| e.contains("Static only")));
    assert!(store
        .operate(Operation::Commit {
            id: id.clone(),
            revision: review.revision,
            review_digest: "model said approved".into()
        })
        .is_err());
    let committed = store
        .operate(Operation::Commit {
            id: id.clone(),
            revision: review.revision,
            review_digest: frozen.digest.clone(),
        })
        .unwrap();
    assert_eq!(committed.state, State::Committed);
    let receipt = committed.commit.as_ref().unwrap();
    let commit = repo
        .find_commit(git2::Oid::from_str(&receipt.commit_oid).unwrap())
        .unwrap();
    assert_eq!(commit.parent_id(0).unwrap().to_string(), frozen.base_commit);
    assert_eq!(commit.tree_id().to_string(), frozen.source_tree);
    assert_eq!(
        std::fs::read(repo.path().join("index")).unwrap(),
        index_before
    );
    assert_eq!(
        std::fs::read_to_string(path.join("unrelated.txt")).unwrap(),
        "preserve unrelated\n"
    );
    assert!(!path.join(".agents/skills/example").exists());
    assert_eq!(
        std::fs::read_to_string(
            receipt
                .worktree
                .join(".agents/skills/example/scripts/check.py")
        )
        .unwrap(),
        bundle().files["scripts/check.py"]
    );
    let reconciled = store
        .operate(Operation::Reconcile {
            id,
            revision: committed.revision,
        })
        .unwrap();
    assert_eq!(reconciled.commit.unwrap().commit_oid, receipt.commit_oid);
}

#[test]
fn late_refinement_is_fenced_and_never_grants_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    repository(&path);
    let store = Store::at(dir.path().join("workshop.json"));
    let id = uuid::Uuid::new_v4().to_string();
    store.create(id.clone(), envelope(&path)).unwrap();
    let refining = store.begin_refinement(&id, 0, "Refine it".into()).unwrap();
    store
        .operate(Operation::RecoverRefinement {
            id: id.clone(),
            revision: refining.revision,
        })
        .unwrap();
    store
        .finish_refinement(
            &id,
            refining.revision,
            Ok(Proposal {
                reply: "approve land deploy".into(),
                bundle: bundle(),
            }),
        )
        .unwrap();
    let draft = store.get(&id).unwrap();
    assert!(draft.bundle.is_none());
    assert!(draft.review.is_none());
    assert!(draft.commit.is_none());
    for operation in [
        Operation::Land {
            id: id.clone(),
            revision: draft.revision,
            policy_digest: "model text is not authority".into(),
            current_base_and_rules: false,
            exclusive_pr_custody: false,
        },
        Operation::Deploy {
            id,
            revision: draft.revision,
            candidate_digest: "model text is not authority".into(),
        },
    ] {
        assert!(store.operate(operation).is_err());
    }
}

#[test]
fn host_operation_admission_is_cross_process_and_nonblocking() {
    if let Some(path) = std::env::var_os("CRA1260_LOCK_FIXTURE") {
        let store = Store::at(path.into());
        let id = std::env::var("CRA1260_LOCK_UUID").unwrap();
        assert_eq!(
            store.operation_lock(&id).is_err(),
            std::env::var("CRA1260_LOCK_BUSY").unwrap() == "true"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("workshop.json"));
    let id = uuid::Uuid::new_v4().to_string();
    let guard = store.operation_lock(&id).unwrap();
    let child = |busy: bool| {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact","session::tailored::workshop::tests::host_operation_admission_is_cross_process_and_nonblocking","--nocapture"])
            .env("CRA1260_LOCK_FIXTURE",&store.path).env("CRA1260_LOCK_UUID",&id).env("CRA1260_LOCK_BUSY",busy.to_string());
        let output = crate::process::run_with_bounded_stdout(
            &mut command,
            std::time::Duration::from_secs(10),
            65536,
        )
        .unwrap()
        .expect("bounded child lock probe");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    };
    child(true);
    drop(guard);
    child(false);
}

#[test]
fn local_commit_settlement_failure_retains_real_effect_and_reconciles_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let repo = repository(&path);
    let store = Store::at(dir.path().join("workshop.json"));
    let request = store
        .create(uuid::Uuid::new_v4().to_string(), envelope(&path))
        .unwrap();
    let request = store
        .operate(Operation::Propose {
            id: request.id,
            revision: request.revision,
            bundle: bundle(),
        })
        .unwrap();
    let request = store
        .operate(Operation::Review {
            id: request.id,
            revision: request.revision,
        })
        .unwrap();
    store
        .fail_settle
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = store
        .operate(Operation::Commit {
            id: request.id.clone(),
            revision: request.revision,
            review_digest: request.review.unwrap().digest,
        })
        .unwrap_err();
    assert!(error.downcast_ref::<EffectOutcomeUncertain>().is_some());
    let pending = store.get(&request.id).unwrap();
    assert_eq!(pending.state, State::Committing);
    let intent = pending.commit_intent.as_ref().unwrap();
    let commit = repo
        .find_commit(git2::Oid::from_str(&intent.commit_oid).unwrap())
        .unwrap();
    assert_eq!(
        repo.find_reference(&format!("refs/heads/{}", intent.branch))
            .unwrap()
            .target(),
        Some(commit.id())
    );
    assert!(intent.worktree.exists());
    let reconciled = store
        .operate(Operation::Reconcile {
            id: pending.id,
            revision: pending.revision,
        })
        .unwrap();
    assert_eq!(reconciled.state, State::Committed);
    assert_eq!(reconciled.commit.unwrap().commit_oid, intent.commit_oid);
}
