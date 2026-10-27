use super::tests::{bundle, envelope, repository};
use super::*;
use std::path::Path;

fn add_commit(repo: &git2::Repository, path: &str, content: &str) {
    let root = repo.workdir().unwrap();
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, content).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(path)).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    let sig = git2::Signature::now("fixture", "fixture@example.invalid").unwrap();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "fixture change",
        &tree,
        &[&parent],
    )
    .unwrap();
}
fn reviewed(store: &Store, repo: &Path) -> Request {
    let id = uuid::Uuid::new_v4().to_string();
    store.create(id.clone(), envelope(repo)).unwrap();
    let draft = store
        .operate(Operation::Propose {
            id: id.clone(),
            revision: 0,
            bundle: bundle(),
        })
        .unwrap();
    store
        .operate(Operation::Review {
            id,
            revision: draft.revision,
        })
        .unwrap()
}
#[test]
fn review_captures_deleted_files_and_refuses_drift_without_staging_unrelated_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let repo = repository(&path);
    add_commit(&repo, ".agents/skills/example/obsolete.txt", "delete me\n");
    let store = Store::at(dir.path().join("workshop.json"));
    let request = reviewed(&store, &path);
    let review = request.review.as_ref().unwrap();
    assert!(review.diff.contains("deleted file mode"));
    assert_eq!(review.previous_files["obsolete.txt"], "delete me\n");
    add_commit(&repo, "another.txt", "HEAD changed\n");
    assert!(store
        .operate(Operation::Commit {
            id: request.id.clone(),
            revision: request.revision,
            review_digest: review.digest.clone()
        })
        .unwrap_err()
        .to_string()
        .contains("HEAD changed"));
    assert!(store.get(&request.id).unwrap().commit_intent.is_none());
    assert!(repo
        .find_reference(&format!("refs/heads/workshop/{}", request.id))
        .is_err());
    let next = store
        .operate(Operation::Review {
            id: request.id.clone(),
            revision: request.revision,
        })
        .unwrap();
    let mut index = repo.index().unwrap();
    std::fs::write(path.join("unrelated.txt"), "staged edit preserved").unwrap();
    index.add_path(Path::new("unrelated.txt")).unwrap();
    index.write().unwrap();
    let before = std::fs::read(repo.path().join("index")).unwrap();
    let result = store
        .operate(Operation::Commit {
            id: next.id,
            revision: next.revision,
            review_digest: next.review.unwrap().digest,
        })
        .unwrap();
    assert_eq!(result.state, State::Committed);
    assert_eq!(std::fs::read(repo.path().join("index")).unwrap(), before);
    assert!(!result
        .commit
        .unwrap()
        .worktree
        .join(".agents/skills/example/obsolete.txt")
        .exists());
}
#[test]
fn unknown_commit_result_reconciles_success_and_never_replays_conflicting_intent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let repo = repository(&path);
    let store = Store::at(dir.path().join("workshop.json"));
    let request = reviewed(&store, &path);
    let intent = source::intent(&request, dir.path()).unwrap();
    let pending = store
        .change(&request.id, |d| {
            d.commit_intent = Some(intent);
            d.state = State::Committing;
            d.revision += 1;
            Ok(d.clone())
        })
        .unwrap();
    let receipt = source::commit(&pending).unwrap();
    assert!(store.get(&pending.id).unwrap().commit.is_none());
    let reconciled = store
        .operate(Operation::Reconcile {
            id: pending.id.clone(),
            revision: pending.revision,
        })
        .unwrap();
    assert_eq!(
        reconciled.commit.as_ref().unwrap().commit_oid,
        receipt.commit_oid
    );
    let duplicate = store
        .operate(Operation::Commit {
            id: pending.id,
            revision: request.revision,
            review_digest: request.review.unwrap().digest,
        })
        .unwrap();
    assert_eq!(duplicate.revision, reconciled.revision);
    let conflict = reviewed(&store, &path);
    let reference = format!("refs/heads/workshop/{}", conflict.id);
    let base = repo.head().unwrap().target().unwrap();
    repo.reference(&reference, base, false, "fixture conflict")
        .unwrap();
    let uncertain = store
        .operate(Operation::Commit {
            id: conflict.id.clone(),
            revision: conflict.revision,
            review_digest: conflict.review.as_ref().unwrap().digest.clone(),
        })
        .unwrap();
    assert_eq!(uncertain.state, State::Uncertain);
    let again = store
        .operate(Operation::Reconcile {
            id: conflict.id,
            revision: uncertain.revision,
        })
        .unwrap();
    assert_eq!(again.state, State::Uncertain);
    assert_eq!(
        repo.find_reference(&reference).unwrap().target(),
        Some(base)
    );
    assert!(!again.commit_intent.unwrap().worktree.exists());
}
#[test]
fn scope_rejects_local_changes_and_symlink_escape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    repository(&path);
    std::fs::create_dir_all(path.join(".agents/skills/example")).unwrap();
    std::fs::write(path.join(".agents/skills/example/untracked"), "preserve me").unwrap();
    let store = Store::at(dir.path().join("workshop.json"));
    let request = reviewed(&store, &path);
    assert_eq!(request.state, State::Failed);
    assert!(request.review.is_none());
    assert_eq!(
        std::fs::read_to_string(path.join(".agents/skills/example/untracked")).unwrap(),
        "preserve me"
    );
    #[cfg(unix)]
    {
        std::fs::remove_dir_all(path.join(".agents")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, path.join(".agents")).unwrap();
        let request = reviewed(&store, &path);
        assert_eq!(request.state, State::Failed);
        assert!(request.error.unwrap().contains("link"));
        assert!(std::fs::read_dir(outside).unwrap().next().is_none());
    }
}

#[test]
fn destination_ancestor_case_alias_is_refused_before_source_effects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let repo = repository(&path);
    add_commit(
        &repo,
        ".AGENTS/skills/other/data.txt",
        "preserve other skill",
    );
    let store = Store::at(dir.path().join("workshop.json"));
    let request = reviewed(&store, &path);
    assert_eq!(request.state, State::Failed);
    assert!(request.error.unwrap().contains("case alias"));
    assert!(request.commit_intent.is_none());
    assert!(repo
        .find_reference(&format!("refs/heads/workshop/{}", request.id))
        .is_err());
}

#[test]
fn worktree_checkout_preserves_raw_reviewed_bytes_despite_attributes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let repo = repository(&path);
    add_commit(&repo, ".gitattributes", "*.md ident\n*.py text eol=crlf\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = repo.path().join("hooks/post-checkout");
        let sentinel = dir.path().join("unexpected-hook");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nprintf hook > {}\n",
                shell_words::quote(sentinel.to_str().unwrap())
            ),
        )
        .unwrap();
        std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::at(dir.path().join("workshop.json"));
    let id = uuid::Uuid::new_v4().to_string();
    store.create(id.clone(), envelope(&path)).unwrap();
    let mut proposed = bundle();
    proposed
        .files
        .get_mut("SKILL.md")
        .unwrap()
        .push_str("Literal $Id$ marker.\n");
    let draft = store
        .operate(Operation::Propose {
            id: id.clone(),
            revision: 0,
            bundle: proposed.clone(),
        })
        .unwrap();
    let request = store
        .operate(Operation::Review {
            id: id.clone(),
            revision: draft.revision,
        })
        .unwrap();
    let committed = store
        .operate(Operation::Commit {
            id: id.clone(),
            revision: request.revision,
            review_digest: request.review.unwrap().digest,
        })
        .unwrap();
    assert_eq!(committed.state, State::Committed, "{:?}", committed.error);
    assert!(!dir.path().join("unexpected-hook").exists());
    let receipt = committed.commit.as_ref().unwrap();
    let skill = receipt.worktree.join(".agents/skills/example");
    for (file, content) in &proposed.files {
        assert_eq!(std::fs::read(skill.join(file)).unwrap(), content.as_bytes());
    }
    let reconciled = store
        .operate(Operation::Reconcile {
            id: id.clone(),
            revision: committed.revision,
        })
        .unwrap();
    assert_eq!(reconciled.state, State::Committed);
    std::fs::write(
        skill.join("SKILL.md"),
        proposed.files["SKILL.md"].replace("$Id$", "$Id: falsely-expanded $"),
    )
    .unwrap();
    let changed = store
        .operate(Operation::Reconcile {
            id: id.clone(),
            revision: reconciled.revision,
        })
        .unwrap();
    assert_eq!(changed.state, State::Uncertain);
    assert!(changed.error.unwrap().contains("raw"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(skill.join("SKILL.md"), &proposed.files["SKILL.md"]).unwrap();
        std::fs::set_permissions(
            skill.join("SKILL.md"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let changed_mode = store
            .operate(Operation::Reconcile {
                id,
                revision: changed.revision,
            })
            .unwrap();
        assert_eq!(changed_mode.state, State::Uncertain);
        assert!(changed_mode.error.unwrap().contains("Git mode"));
    }
}
