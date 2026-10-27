use super::*;
use git2::{ObjectType, Repository};
use std::path::Path;

fn repository(path: &Path) -> Result<Repository> {
    ensure!(
        path.is_absolute() && path.canonicalize()? == path,
        "Source repository must be canonical absolute path"
    );
    let repo = Repository::open(path)?;
    ensure!(
        !repo.is_bare() && repo.workdir().context("Missing worktree")?.canonicalize()? == path,
        "Source must be worktree root"
    );
    Ok(repo)
}
pub(super) fn validate_skill_path(path: &str) -> Result<()> {
    let slug = path
        .strip_prefix(".agents/skills/")
        .context("Skill path must be .agents/skills/<slug>")?;
    ensure!(
        !slug.is_empty()
            && slug.len() <= 64
            && !slug.starts_with('-')
            && !slug.ends_with('-')
            && slug
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
        "Unsafe skill slug/path"
    );
    Ok(())
}
fn files(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    prefix: &str,
    depth: usize,
    out: &mut Vec<(String, i32, String)>,
    bytes: &mut usize,
) -> Result<()> {
    ensure!(depth <= 8, "Skill tree too deep");
    for entry in tree {
        let name = entry.name().context("Non-UTF8 skill path")?;
        component(name)?;
        ensure!(
            !name.starts_with('.') && !name.eq_ignore_ascii_case(".git"),
            "Hidden/unsafe skill file"
        );
        let path = format!("{prefix}{name}");
        ensure!(path.len() <= 240, "Skill path too long");
        if entry.kind() == Some(ObjectType::Tree) && entry.filemode() == 0o040000 {
            files(
                repo,
                &repo.find_tree(entry.id())?,
                &format!("{path}/"),
                depth + 1,
                out,
                bytes,
            )?;
        } else {
            ensure!(
                entry.kind() == Some(ObjectType::Blob)
                    && matches!(entry.filemode(), 0o100644 | 0o100755)
                    && out.len() < 32,
                "Skill links/submodules/count unsupported"
            );
            let blob = repo.find_blob(entry.id())?;
            ensure!(blob.size() <= 65_536, "Skill blob too large");
            std::str::from_utf8(blob.content())?;
            *bytes += blob.size();
            ensure!(*bytes <= 256 * 1024, "Skill byte limit");
            out.push((path, entry.filemode(), entry.id().to_string()));
        }
    }
    Ok(())
}
fn entries(repo: &Repository, tree: &str) -> Result<Vec<(String, i32, String)>> {
    let mut out = Vec::new();
    files(repo, &repo.find_tree(oid(tree)?)?, "", 0, &mut out, &mut 0)?;
    out.sort();
    ensure!(
        !out.is_empty() && out.iter().any(|e| e.0 == "SKILL.md"),
        "Skill needs SKILL.md"
    );
    let mut paths = std::collections::BTreeMap::new();
    for (path, _, _) in &out {
        let mut prefix = String::new();
        for part in path.split('/') {
            prefix.push_str(part);
            if let Some(old) = paths.insert(prefix.to_ascii_lowercase(), prefix.clone()) {
                ensure!(old == prefix, "Case alias in skill path");
            }
            prefix.push('/');
        }
    }
    Ok(out)
}
pub(super) fn content_digest(path: &Path, tree: &str) -> Result<String> {
    digest(&entries(&repository(path)?, tree)?)
}
pub(super) fn verify(source: &HostSource) -> Result<()> {
    validate_skill_path(&source.skill_path)?;
    ensure!(
        uuid::Uuid::parse_str(&source.request_id)?.to_string() == source.request_id,
        "Noncanonical Workshop UUID"
    );
    ensure!(
        source.revision > 0
            && source.review_digest.len() == 64
            && source.review_digest.bytes().all(|c| c.is_ascii_hexdigit())
            && !source.source_scope.trim().is_empty(),
        "Incomplete source/review binding"
    );
    bounded_text(&source.source_scope, 4096)?;
    let repo = repository(&source.repository)?;
    ensure!(
        source.common_dir.is_absolute()
            && source.common_dir.canonicalize()? == source.common_dir
            && repo.commondir().canonicalize()? == source.common_dir,
        "Source common-dir identity changed"
    );
    let commit = repo.find_commit(oid(&source.commit_oid)?)?;
    ensure!(
        commit.parent_count() == 1
            && commit.parent_id(0)? == oid(&source.base_commit)?
            && commit.tree_id() == oid(&source.source_tree)?,
        "Reviewed commit/parent/tree changed"
    );
    ensure!(
        repo.find_reference(&source.owned_ref())?.target() == Some(commit.id()),
        "Owned local ref changed"
    );
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(&source.skill_path))?;
    ensure!(
        entry.kind() == Some(ObjectType::Tree) && entry.id() == oid(&source.skill_tree)?,
        "Reviewed subtree changed"
    );
    ensure!(
        digest(&entries(&repo, &source.skill_tree)?)? == source.content_digest,
        "Reviewed skill content digest differs"
    );
    let parent = commit.parent(0)?;
    let diff = repo.diff_tree_to_tree(Some(&parent.tree()?), Some(&tree), None)?;
    ensure!(diff.deltas().len() > 0, "Empty reviewed change");
    for delta in diff.deltas() {
        for file in [delta.old_file(), delta.new_file()] {
            if let Some(path) = file.path() {
                ensure!(
                    path.starts_with(&source.skill_path),
                    "Commit contains unrelated changes"
                );
            }
        }
    }
    // These objects are read directly; no checkout, filters, hooks or user index writes.
    Ok(())
}
pub(super) fn upload<T: Transport>(
    forge: &Forge<T>,
    intent: &PublishIntent,
) -> Result<RemoteCommit> {
    use base64::Engine;
    let source = &intent.preview.source;
    let destination = &intent.preview.destination;
    let repo = repository(&source.repository)?;
    let items = entries(&repo, &source.skill_tree)?;
    for (_, _, sha) in &items {
        let blob = repo.find_blob(oid(sha)?)?;
        let response:Value=forge.write(destination,Method::Post,"/git/blobs",json!({"content":base64::engine::general_purpose::STANDARD.encode(blob.content()),"encoding":"base64"}))?;
        ensure!(
            response["sha"].as_str() == Some(sha),
            "Server blob SHA differs"
        );
    }
    let tree:Vec<_>=items.iter().map(|(path,mode,sha)|json!({"path":path,"mode":format!("{mode:o}"),"type":"blob","sha":sha})).collect();
    let response: Value = forge.write(
        destination,
        Method::Post,
        "/git/trees",
        json!({"tree":tree}),
    )?;
    ensure!(
        response["sha"].as_str() == Some(&source.skill_tree),
        "Uploaded skill tree differs"
    );
    let commit = repo.find_commit(oid(&source.commit_oid)?)?;
    let response:Value=forge.write(destination,Method::Post,"/git/trees",json!({"base_tree":commit.parent(0)?.tree_id().to_string(),"tree":[{"path":source.skill_path,"mode":"040000","type":"tree","sha":source.skill_tree}]}))?;
    ensure!(
        response["sha"].as_str() == Some(&source.source_tree),
        "Uploaded full source tree differs"
    );
    fn signature(sig: git2::Signature<'_>) -> Result<Value> {
        let when = sig.when();
        let offset = chrono::FixedOffset::east_opt(when.offset_minutes() * 60)
            .context("Invalid commit timezone")?;
        let date = chrono::DateTime::from_timestamp(when.seconds(), 0)
            .context("Invalid commit time")?
            .with_timezone(&offset)
            .to_rfc3339();
        Ok(
            json!({"name":sig.name().context("Missing author name")?,"email":sig.email().context("Missing author email")?,"date":date}),
        )
    }
    let message = commit.message().context("Commit message is not UTF-8")?;
    bounded_text(message, 65_536)?;
    let response:Value=forge.write(destination,Method::Post,"/git/commits",json!({"message":message,"tree":source.source_tree,"parents":[source.base_commit],"author":signature(commit.author())?,"committer":signature(commit.committer())?}))?;
    let receipt = RemoteCommit {
        commit_oid: response["sha"]
            .as_str()
            .context("Missing remote commit SHA")?
            .into(),
        source_tree: source.source_tree.clone(),
        skill_tree: source.skill_tree.clone(),
    };
    forge.verify_commit(destination, &receipt, source)?;
    Ok(receipt)
}
