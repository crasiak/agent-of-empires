use super::*;
use git2::{IndexEntry, IndexTime, Oid, Repository};
use std::path::Path;

pub(super) fn repository(path: &Path) -> Result<Repository> {
    ensure!(
        path.is_absolute() && path.canonicalize()? == path,
        "Choose a canonical absolute repository path"
    );
    let repo = Repository::open(path)?;
    ensure!(
        !repo.is_bare()
            && repo
                .workdir()
                .context("Repository needs a worktree")?
                .canonicalize()?
                == path,
        "Choose the repository worktree root, not a subdirectory"
    );
    repo.head()?.peel_to_commit()?;
    Ok(repo)
}
fn prefix(draft: &Request) -> String {
    format!(".agents/skills/{}", draft.envelope.slug)
}
fn clean_scope(repo: &Repository, draft: &Request) -> Result<()> {
    let root = prefix(draft);
    let mut path = draft.envelope.repository.clone();
    for part in root.split('/') {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "Skill source path contains a link or non-directory"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(true)
        .pathspec(&root);
    ensure!(repo.statuses(Some(&mut options))?.is_empty(), "Selected skill has local/staged/untracked changes; preserve and reconcile them before Workshop Review");
    Ok(())
}
fn scoped_files(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    root: &str,
) -> Result<BTreeMap<String, String>> {
    let mut current = tree.clone();
    for part in root.split('/') {
        for entry in &current {
            if entry
                .name()
                .is_ok_and(|name| name.eq_ignore_ascii_case(part))
            {
                ensure!(
                    entry.name_bytes() == part.as_bytes(),
                    "Existing destination ancestry has a case alias"
                );
            }
        }
        let Some(entry) = current.get_name(part) else {
            return Ok(BTreeMap::new());
        };
        ensure!(
            entry.filemode() == 0o040000,
            "Skill source ancestors must be Git trees, not links or files"
        );
        let next = repo.find_tree(entry.id())?;
        drop(entry);
        current = next;
    }
    let mut files = BTreeMap::new();
    fn walk(
        repo: &Repository,
        tree: &git2::Tree<'_>,
        prefix: &str,
        depth: usize,
        files: &mut BTreeMap<String, String>,
        total: &mut usize,
    ) -> Result<()> {
        ensure!(depth <= 8, "Existing skill tree is too deep");
        for entry in tree {
            let name = entry.name().context("Existing skill paths must be UTF-8")?;
            let path = format!("{prefix}{name}");
            if entry.filemode() == 0o040000 {
                walk(
                    repo,
                    &repo.find_tree(entry.id())?,
                    &format!("{path}/"),
                    depth + 1,
                    files,
                    total,
                )?;
            } else {
                ensure!(
                    matches!(entry.filemode(), 0o100644 | 0o100755),
                    "Existing skill links/submodules are unsupported"
                );
                ensure!(files.len() < 32, "Existing skill exceeds file limit");
                let blob = repo.find_blob(entry.id())?;
                ensure!(blob.size() <= 65_536, "Existing skill file exceeds 64 KiB");
                *total += blob.size();
                ensure!(*total <= 256 * 1024, "Existing skill exceeds 256 KiB");
                files.insert(path, std::str::from_utf8(blob.content())?.to_owned());
            }
        }
        Ok(())
    }
    walk(repo, &current, "", 0, &mut files, &mut 0)?;
    Ok(files)
}
pub(super) fn review(draft: &Request) -> Result<Review> {
    let bundle = draft.bundle.as_ref().context("Missing bundle")?;
    bundle.validate(&draft.envelope.slug)?;
    let repo = repository(&draft.envelope.repository)?;
    clean_scope(&repo, draft)?;
    let base = repo.head()?.peel_to_commit()?;
    let base_tree = base.tree()?;
    let root = prefix(draft);
    let previous_files = scoped_files(&repo, &base_tree, &root)?;
    let mut index = git2::Index::new()?;
    index.read_tree(&base_tree)?;
    let scoped = format!("{root}/");
    let remove: Vec<_> = index
        .iter()
        .filter(|e| e.path.starts_with(scoped.as_bytes()))
        .map(|e| e.path)
        .collect();
    for path in remove {
        index.remove_path(Path::new(std::str::from_utf8(&path)?))?;
    }
    for (path, content) in &bundle.files {
        let path = format!("{root}/{path}");
        index.add(&IndexEntry {
            ctime: IndexTime::new(0, 0),
            mtime: IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: content.len() as u32,
            id: repo.blob(content.as_bytes())?,
            flags: 0,
            flags_extended: 0,
            path: path.into_bytes(),
        })?;
    }
    let tree = repo.find_tree(index.write_tree_to(&repo)?)?;
    let skill_tree = tree.get_path(Path::new(&root))?.id();
    let diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&tree), None)?;
    let mut bytes = vec![];
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        if bytes.len() + line.content().len() + 1 > 1024 * 1024 {
            return false;
        }
        if matches!(line.origin(), ' ' | '+' | '-') {
            bytes.push(line.origin() as u8);
        }
        bytes.extend_from_slice(line.content());
        true
    })?;
    ensure!(
        !bytes.is_empty(),
        "Bundle does not change the selected skill source"
    );
    let mut review = Review { revision: draft.revision, repository: draft.envelope.repository.clone(), base_commit: base.id().to_string(), source_tree: tree.id().to_string(), skill_tree: skill_tree.to_string(), files: bundle.clone(), previous_files, diff: String::from_utf8(bytes)?, evidence: vec!["Static only: bounded UTF-8 bundle, safe relative paths, SKILL.md frontmatter/name and exact Git tree/diff validated. Generated scripts were not executed; no functional tests claimed.".into()], created_at: chrono::Utc::now(), digest: String::new() };
    review.digest = review.computed_digest()?;
    Ok(review)
}
pub(super) fn validate_review(draft: &Request) -> Result<()> {
    let review = draft.review.as_ref().context("Missing frozen review")?;
    ensure!(
        review.computed_digest()? == review.digest
            && draft.bundle.as_ref() == Some(&review.files)
            && review.repository == draft.envelope.repository,
        "Review content changed; freeze and approve a new review"
    );
    let repo = repository(&review.repository)?;
    ensure!(
        repo.head()?.peel_to_commit()?.id().to_string() == review.base_commit,
        "Repository HEAD changed; review the new base explicitly"
    );
    clean_scope(&repo, draft)?;
    let tree = repo.find_tree(Oid::from_str(&review.source_tree)?)?;
    ensure!(
        tree.get_path(Path::new(&prefix(draft)))?.id().to_string() == review.skill_tree,
        "Reviewed skill tree differs"
    );
    ensure!(
        scoped_files(&repo, &tree, &prefix(draft))? == review.files.files,
        "Reviewed blobs differ"
    );
    let base = repo.find_commit(Oid::from_str(&review.base_commit)?)?;
    let diff = repo.diff_tree_to_tree(Some(&base.tree()?), Some(&tree), None)?;
    for delta in diff.deltas() {
        for file in [delta.old_file(), delta.new_file()] {
            if let Some(path) = file.path() {
                ensure!(
                    path.starts_with(prefix(draft)),
                    "Review includes files outside the skill scope"
                );
            }
        }
    }
    Ok(())
}
fn signature(review: &Review) -> Result<git2::Signature<'static>> {
    Ok(git2::Signature::new(
        "AoE Skill Workshop",
        "workshop@localhost",
        &git2::Time::new(review.created_at.timestamp(), 0),
    )?)
}
fn commit_bytes(draft: &Request, repo: &Repository) -> Result<git2::Buf> {
    let review = draft.review.as_ref().context("Missing review")?;
    let base = repo.find_commit(Oid::from_str(&review.base_commit)?)?;
    let tree = repo.find_tree(Oid::from_str(&review.source_tree)?)?;
    let sig = signature(review)?;
    Ok(repo.commit_create_buffer(
        &sig,
        &sig,
        &format!(
            "Workshop {}: {}\n\nReview: {}\n",
            draft.id, draft.envelope.slug, review.digest
        ),
        &tree,
        &[&base],
    )?)
}
pub(super) fn intent(draft: &Request, root: &Path) -> Result<CommitIntent> {
    let repo = repository(&draft.envelope.repository)?;
    let bytes = commit_bytes(draft, &repo)?;
    Ok(CommitIntent {
        review_digest: draft
            .review
            .as_ref()
            .context("Missing review")?
            .digest
            .clone(),
        commit_oid: Oid::hash_object(git2::ObjectType::Commit, &bytes)?.to_string(),
        branch: format!("workshop/{}", draft.id),
        worktree: root
            .canonicalize()?
            .join("workshop-worktrees")
            .join(&draft.id),
    })
}
pub(super) fn commit(draft: &Request) -> Result<CommitReceipt> {
    validate_review(draft)?;
    let intent = draft
        .commit_intent
        .as_ref()
        .context("Missing commit intent")?;
    let repo = repository(&draft.envelope.repository)?;
    ensure!(
        !intent.worktree.exists(),
        "Owned worktree already exists; reconcile instead of overwriting"
    );
    let parent = intent
        .worktree
        .parent()
        .context("Missing worktree parent")?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    ensure!(
        parent.canonicalize()? == parent,
        "Worktree parent must not contain links"
    );
    let bytes = commit_bytes(draft, &repo)?;
    let oid = repo.odb()?.write(git2::ObjectType::Commit, &bytes)?;
    ensure!(
        oid.to_string() == intent.commit_oid,
        "Commit differs from durable intent"
    );
    repo.reference(
        &format!("refs/heads/{}", intent.branch),
        oid,
        false,
        "Workshop exact reviewed source",
    )?;
    register_worktree(draft)?;
    let worktree = repository(&intent.worktree)?;
    worktree.checkout_head(Some(
        git2::build::CheckoutBuilder::new()
            .disable_filters(true)
            .dir_perm(0o700),
    ))?;
    reconcile(draft)
}
fn register_worktree(draft: &Request) -> Result<()> {
    let intent = draft
        .commit_intent
        .as_ref()
        .context("Missing commit intent")?;
    ensure!(
        intent.branch == format!("workshop/{}", draft.id),
        "Unexpected Workshop branch"
    );
    ensure!(
        intent
            .worktree
            .file_name()
            .is_some_and(|name| name == draft.id.as_str()),
        "Unexpected owned worktree identity"
    );
    ensure!(
        intent.worktree.as_os_str().len() <= 4096,
        "Owned worktree path exceeds the command bound"
    );
    let home = tempfile::tempdir()?;
    let git = which::which("git").context("Git is required to register a no-checkout worktree")?;
    let output = crate::process::run_with_bounded_stdout(
        std::process::Command::new(git)
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(&draft.envelope.repository)
            .args(["worktree", "add", "--no-checkout", "--"])
            .arg(&intent.worktree)
            .arg(&intent.branch),
        std::time::Duration::from_secs(30),
        64 * 1024,
    )?
    .context(
        "Worktree registration timed out or exceeded its output bound; reconcile retained intent",
    )?;
    ensure!(
        output.status.success(),
        "No-checkout worktree registration failed; reconcile retained intent"
    );
    Ok(())
}
fn verify_raw_skill(draft: &Request) -> Result<()> {
    use std::io::Read;
    let intent = draft
        .commit_intent
        .as_ref()
        .context("Missing commit intent")?;
    let review = draft.review.as_ref().context("Missing review")?;
    let mut root = intent.worktree.clone();
    for component in prefix(draft).split('/') {
        root.push(component);
        let metadata = std::fs::symlink_metadata(&root)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Owned raw skill ancestry contains a link or non-directory"
        );
    }
    fn collect(
        root: &Path,
        relative: &Path,
        files: &mut BTreeMap<String, String>,
        depth: usize,
    ) -> Result<()> {
        ensure!(
            depth <= 8,
            "Owned raw skill directory exceeds the review depth"
        );
        for entry in std::fs::read_dir(root.join(relative))? {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            let metadata = std::fs::symlink_metadata(entry.path())?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "Owned raw skill contains a link"
            );
            if metadata.is_dir() {
                collect(root, &path, files, depth + 1)?;
            } else {
                ensure!(
                    files.len() < 32 && metadata.is_file() && metadata.len() <= 65_536,
                    "Owned raw skill has an unexpected file"
                );
                let mut options = std::fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                }
                let file = options.open(entry.path())?;
                let metadata = file.metadata()?;
                ensure!(metadata.is_file(), "Owned raw skill is not a regular file");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    ensure!(
                        metadata.permissions().mode() & 0o7111 == 0,
                        "Owned raw skill Git mode differs from reviewed non-executable files"
                    );
                }
                let mut content = String::new();
                file.take(65_537).read_to_string(&mut content)?;
                ensure!(
                    content.len() <= 65_536,
                    "Owned raw skill file exceeds the review bound"
                );
                files.insert(
                    path.to_str()
                        .context("Owned raw skill path is not UTF-8")?
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                    content,
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    collect(&root, Path::new(""), &mut files, 0)?;
    ensure!(
        files == review.files.files,
        "Owned raw skill bytes or paths differ from the frozen review"
    );
    Ok(())
}
pub(super) fn reconcile(draft: &Request) -> Result<CommitReceipt> {
    let intent = draft
        .commit_intent
        .as_ref()
        .context("Missing commit intent")?;
    let review = draft.review.as_ref().context("Missing review")?;
    ensure!(
        intent.review_digest == review.digest && review.computed_digest()? == review.digest,
        "Intent/review digest mismatch"
    );
    let repo = repository(&draft.envelope.repository)?;
    let commit = repo.find_commit(Oid::from_str(&intent.commit_oid)?)?;
    ensure!(
        commit.tree_id().to_string() == review.source_tree
            && commit.parent_count() == 1
            && commit.parent_id(0)?.to_string() == review.base_commit
            && repo
                .find_reference(&format!("refs/heads/{}", intent.branch))?
                .target()
                == Some(commit.id()),
        "Commit, parent or branch differs from durable intent"
    );
    let worktree = repository(&intent.worktree)?;
    ensure!(
        worktree.commondir().canonicalize()? == repo.commondir().canonicalize()?
            && worktree.head()?.peel_to_commit()?.id() == commit.id(),
        "Worktree repository/head differs"
    );
    verify_raw_skill(draft)?;
    ensure!(
        worktree
            .statuses(Some(
                git2::StatusOptions::new()
                    .include_untracked(true)
                    .recurse_untracked_dirs(true)
                    .include_ignored(true)
            ))?
            .is_empty(),
        "Owned worktree changed; committed source retained, no writes replayed"
    );
    Ok(CommitReceipt {
        commit_oid: commit.id().to_string(),
        source_tree: review.source_tree.clone(),
        skill_tree: review.skill_tree.clone(),
        branch: intent.branch.clone(),
        worktree: intent.worktree.clone(),
    })
}
