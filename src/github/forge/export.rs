use super::*;
use git2::{ObjectType, Oid, Repository};
use std::path::Path;

/// Durable private cache receipt. Dropping it never removes deployment evidence.
/// The host persists it before calling a helper and owns explicit safe cleanup.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionReceipt {
    directory: std::path::PathBuf,
    landed: LandedSource,
    destination: Destination,
    snapshot_digest: String,
    digest: String,
}
impl ProjectionReceipt {
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn repository(&self) -> std::path::PathBuf {
        self.directory.join("repository.git")
    }
    pub fn source_root(&self) -> std::path::PathBuf {
        self.directory.join("source")
    }
    pub fn landed(&self) -> &LandedSource {
        &self.landed
    }
    fn computed_digest(&self) -> Result<String> {
        let mut copy = self.clone();
        copy.digest.clear();
        digest(&copy)
    }
    /// Local-only reopen verification, including full private-cache file fingerprint.
    pub fn verify(&self) -> Result<()> {
        self.landed.validate_execution()?;
        ensure!(
            self.computed_digest()? == self.digest,
            "Projection receipt changed"
        );
        private_directory(&self.directory)?;
        private_directory(
            self.directory
                .parent()
                .context("Missing projection parent")?,
        )?;
        let snapshot = snapshot(&self.directory)?;
        ensure!(
            digest(&snapshot)? == self.snapshot_digest,
            "Projection snapshot changed"
        );
        let metadata = read_file(&self.directory.join("projection.json"), 64 * 1024)?;
        ensure!(
            metadata == serde_json::to_vec(self)?,
            "Projection metadata differs from receipt"
        );
        let repo = Repository::open_bare(self.repository())?;
        verify_projection(&repo, &self.landed, &self.source_root())
    }
}

impl<T: Transport> Forge<T> {
    /// Read-only remote export of actual landed objects into a new private bare cache.
    /// Unsupported lossless commit data fails closed, never substituting local Review.
    pub fn export_landed(
        &self,
        landed: &LandedSource,
        destination: &Destination,
        private_parent: &Path,
    ) -> Result<ProjectionReceipt> {
        landed.validate_execution()?;
        destination.validate()?;
        ensure!(
            landed.identity.repository_id == destination.repository_id
                && landed.identity.url
                    == format!(
                        "https://github.com/{}/{}/pull/{}",
                        destination.owner, destination.name, landed.identity.number
                    )
                && landed.api_version == API_VERSION,
            "Landed destination/API identity differs"
        );
        oid(&landed.merged_commit)?;
        oid(&landed.source_tree)?;
        oid(&landed.skill_tree)?;
        source::validate_skill_path(&landed.skill_path)?;
        private_directory(private_parent)?;
        self.repository(destination)?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("landed-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let directory = builder.tempdir_in(private_parent)?;
        private_directory(directory.path())?;
        let repo = Repository::init_opts(
            directory.path().join("repository.git"),
            git2::RepositoryInitOptions::new()
                .bare(true)
                .no_reinit(true)
                .external_template(false),
        )?;
        let mut export = Export {
            forge: self,
            destination,
            repo,
            deadline: Instant::now() + Duration::from_secs(120),
            bytes: 0,
            objects: 0,
            files: Vec::new(),
            skill_bytes: 0,
        };
        let value: Value = export.get(&format!("/git/commits/{}", landed.merged_commit))?;
        let raw = commit_bytes(value, landed)?;
        export.write(ObjectType::Commit, &landed.merged_commit, &raw)?;
        let mut current = landed.source_tree.clone();
        for part in landed.skill_path.split('/') {
            let tree = export.tree(&current)?;
            let matches: Vec<_> = tree
                .tree
                .iter()
                .filter(|e| e.path.eq_ignore_ascii_case(part))
                .collect();
            ensure!(
                matches.len() == 1
                    && matches[0].path == part
                    && matches[0].kind == "tree"
                    && matches[0].mode == "040000",
                "Landed ancestry missing/aliased/non-tree"
            );
            current = matches[0].sha.clone();
        }
        ensure!(
            current == landed.skill_tree,
            "Exported skill subtree differs from Land"
        );
        export.skill(&current, "", 0)?;
        export.files.sort();
        ensure!(
            export.files.iter().any(|e| e.0 == "SKILL.md")
                && digest(&export.files)? == landed.content_digest,
            "Exported content fingerprint differs from Land"
        );
        ensure!(
            export
                .repo
                .find_commit(oid(&landed.merged_commit)?)?
                .tree_id()
                == oid(&landed.source_tree)?
                && export
                    .repo
                    .find_tree(oid(&landed.source_tree)?)?
                    .get_path(Path::new(&landed.skill_path))?
                    .id()
                    == oid(&landed.skill_tree)?,
            "Projection object binding differs"
        );
        let source_root = directory.path().join("source");
        std::fs::create_dir(&source_root)?;
        for (path, mode, sha) in &export.files {
            let target = source_root.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let blob = export.repo.find_blob(oid(sha)?)?;
            write_file(&target, blob.content(), *mode == 0o100755)?;
        }
        verify_projection(&export.repo, landed, &source_root)?;
        drop(export);
        private_tree(directory.path(), 0)?;
        let mut receipt = ProjectionReceipt {
            directory: directory.path().to_owned(),
            landed: landed.clone(),
            destination: destination.clone(),
            snapshot_digest: digest(&snapshot(directory.path())?)?,
            digest: String::new(),
        };
        receipt.digest = receipt.computed_digest()?;
        write_file(
            &directory.path().join("projection.json"),
            &serde_json::to_vec(&receipt)?,
            false,
        )?;
        sync_tree(directory.path())?;
        receipt.verify()?;
        std::fs::File::open(private_parent)?.sync_all()?;
        let _retained = directory.keep();
        Ok(receipt)
    }
}

fn private_directory(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.canonicalize()? == path,
        "Projection parent must be canonical absolute path without links"
    );
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Projection parent must be a directory, not a link"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() }
                && metadata.permissions().mode() & 0o077 == 0,
            "Projection parent must be owned by this host user and private (0700)"
        );
    }
    #[cfg(not(unix))]
    bail!("Private projection permissions unsupported on this platform");
    Ok(())
}
fn private_tree(path: &Path, depth: usize) -> Result<()> {
    ensure!(depth <= 16, "Unexpected projection layout");
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "Unexpected projection link"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                entry.path(),
                std::fs::Permissions::from_mode(
                    if metadata.is_dir() || metadata.permissions().mode() & 0o100 != 0 {
                        0o700
                    } else {
                        0o600
                    },
                ),
            )?;
        }
        if metadata.is_dir() {
            private_tree(&entry.path(), depth + 1)?;
        }
    }
    Ok(())
}
struct Export<'a, T: Transport> {
    forge: &'a Forge<T>,
    destination: &'a Destination,
    repo: Repository,
    deadline: Instant,
    bytes: usize,
    objects: usize,
    files: Vec<(String, i32, String)>,
    skill_bytes: usize,
}
impl<T: Transport> Export<'_, T> {
    fn get(&self, suffix: &str) -> Result<Value> {
        ensure!(
            Instant::now() < self.deadline,
            "Projection export deadline exceeded"
        );
        let value = self.forge.get(self.destination, suffix)?;
        ensure!(
            Instant::now() < self.deadline,
            "Projection export deadline exceeded"
        );
        Ok(value)
    }
    fn write(&mut self, kind: ObjectType, sha: &str, bytes: &[u8]) -> Result<()> {
        ensure!(
            Oid::hash_object(kind, bytes)? == oid(sha)?,
            "Lossless Git object unavailable or OID mismatch"
        );
        self.bytes += bytes.len();
        self.objects += 1;
        ensure!(
            self.bytes <= 8 * 1024 * 1024 && self.objects <= 300,
            "Projection object/byte bound exceeded"
        );
        ensure!(
            self.repo.odb()?.write(kind, bytes)? == oid(sha)?,
            "Projection ODB differs"
        );
        Ok(())
    }
    fn tree(&mut self, sha: &str) -> Result<GitTree> {
        let tree: GitTree = serde_json::from_value(self.get(&format!("/git/trees/{sha}"))?)?;
        ensure!(
            tree.sha == sha && !tree.truncated && tree.tree.len() <= 4096,
            "Incomplete/oversized Git tree"
        );
        let mut entries: Vec<_> = tree.tree.iter().collect();
        entries.sort_by_key(|e| {
            format!("{}{}", e.path, if e.kind == "tree" { "/" } else { "\0" }).into_bytes()
        });
        let mut previous = None;
        let mut raw = Vec::new();
        for entry in entries {
            ensure!(
                !entry.path.is_empty()
                    && entry.path.len() <= 1024
                    && !entry.path.contains(['\0', '/'])
                    && entry.path != "."
                    && entry.path != "..",
                "Unsafe/lossy Git tree name"
            );
            ensure!(previous != Some(&entry.path), "Duplicate Git tree name");
            previous = Some(&entry.path);
            ensure!(
                matches!(
                    (entry.mode.as_str(), entry.kind.as_str()),
                    ("040000", "tree")
                        | ("100644" | "100755" | "120000", "blob")
                        | ("160000", "commit")
                ),
                "Unknown Git tree mode/type"
            );
            raw.extend_from_slice(entry.mode.trim_start_matches('0').as_bytes());
            raw.push(b' ');
            raw.extend_from_slice(entry.path.as_bytes());
            raw.push(0);
            raw.extend_from_slice(oid(&entry.sha)?.as_bytes());
        }
        self.write(ObjectType::Tree, sha, &raw)?;
        Ok(tree)
    }
    fn skill(&mut self, sha: &str, prefix: &str, depth: usize) -> Result<()> {
        ensure!(depth <= 8, "Exported skill too deep");
        let tree = self.tree(sha)?;
        ensure!(tree.tree.len() <= 32, "Exported skill tree too wide");
        let mut folded = std::collections::BTreeSet::new();
        for entry in tree.tree {
            component(&entry.path)?;
            ensure!(
                !entry.path.starts_with('.') && folded.insert(entry.path.to_ascii_lowercase()),
                "Unsafe/aliased skill path"
            );
            let path = format!("{prefix}{}", entry.path);
            ensure!(path.len() <= 240, "Skill path exceeds bound");
            if entry.kind == "tree" {
                self.skill(&entry.sha, &format!("{path}/"), depth + 1)?;
            } else {
                ensure!(
                    matches!(entry.mode.as_str(), "100644" | "100755")
                        && entry.kind == "blob"
                        && self.files.len() < 32,
                    "Skill links/submodules/count unsupported"
                );
                ensure!(
                    entry.size.is_some_and(|size| size <= 65_536),
                    "Missing/oversized skill blob size"
                );
                let blob: GitBlob =
                    serde_json::from_value(self.get(&format!("/git/blobs/{}", entry.sha))?)?;
                ensure!(
                    blob.sha == entry.sha
                        && blob.encoding == "base64"
                        && blob.size <= 65_536
                        && Some(blob.size) == entry.size
                        && blob.content.len() <= 90_000,
                    "Incomplete/oversized Git blob"
                );
                use base64::Engine;
                let encoded = blob.content.replace(['\r', '\n'], "");
                let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
                ensure!(bytes.len() == blob.size, "Truncated Git blob");
                std::str::from_utf8(&bytes)?;
                self.skill_bytes += bytes.len();
                ensure!(self.skill_bytes <= 256 * 1024, "Skill byte bound exceeded");
                self.write(ObjectType::Blob, &entry.sha, &bytes)?;
                self.files
                    .push((path, i32::from_str_radix(&entry.mode, 8)?, entry.sha));
            }
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitTree {
    sha: String,
    #[serde(rename = "url")]
    _url: Option<String>,
    tree: Vec<TreeEntry>,
    truncated: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
    size: Option<usize>,
    #[serde(rename = "url")]
    _url: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitBlob {
    sha: String,
    #[serde(rename = "node_id")]
    _node_id: Option<String>,
    size: usize,
    #[serde(rename = "url")]
    _url: Option<String>,
    content: String,
    encoding: String,
}

fn commit_bytes(value: Value, landed: &LandedSource) -> Result<Vec<u8>> {
    let commit: GitCommit = serde_json::from_value(value)?;
    ensure!(
        commit.sha == landed.merged_commit
            && commit.tree.sha == landed.source_tree
            && commit.parents.len() == 1,
        "Actual squash commit identity differs"
    );
    oid(&commit.parents[0].sha)?;
    bounded_text(&commit.message, 65_536)?;
    let author = signature(&commit.author)?;
    let committer = signature(&commit.committer)?;
    let unsigned = format!(
        "tree {}\nparent {}\nauthor {author}\ncommitter {committer}\n\n{}",
        commit.tree.sha, commit.parents[0].sha, commit.message
    );
    let raw = match (commit.verification.signature, commit.verification.payload) {
        (None, None) => {
            ensure!(
                !commit.verification.verified && commit.verification.reason == "unsigned",
                "Lossless signature payload unavailable"
            );
            unsigned.into_bytes()
        }
        (Some(signature), Some(payload)) => {
            bounded_text(&signature, 32 * 1024)?;
            bounded_text(&payload, 128 * 1024)?;
            ensure!(
                !signature.is_empty() && !signature.contains('\r'),
                "Unsupported signature encoding"
            );
            let (headers, message) = payload
                .split_once("\n\n")
                .context("Incomplete signed commit payload")?;
            let lines: Vec<_> = headers.split('\n').collect();
            ensure!(
                lines.len() == 4
                    && lines[0] == format!("tree {}", commit.tree.sha)
                    && lines[1] == format!("parent {}", commit.parents[0].sha)
                    && lines[2].starts_with("author ")
                    && lines[3].starts_with("committer ")
                    && message == commit.message,
                "Unexpected signed commit header/payload"
            );
            verify_signature_line(lines[2].trim_start_matches("author "), &commit.author)?;
            verify_signature_line(lines[3].trim_start_matches("committer "), &commit.committer)?;
            format!(
                "{headers}\ngpgsig {}\n\n{message}",
                signature.replace('\n', "\n ")
            )
            .into_bytes()
        }
        _ => bail!("Lossless GitHub signed commit payload unavailable"),
    };
    ensure!(Oid::hash_object(ObjectType::Commit,&raw)?==oid(&landed.merged_commit)?,"Lossless Git commit unavailable (JSON normalization/extra headers); no substitute permitted");
    Ok(raw)
}
fn signature(value: &GitSignature) -> Result<String> {
    for text in [&value.name, &value.email] {
        bounded_text(text, 1024)?;
        ensure!(
            !text.contains(['\n', '\r', '<', '>']),
            "Unsupported Git identity encoding"
        );
    }
    let date = chrono::DateTime::parse_from_rfc3339(&value.date)?;
    ensure!(
        date.timestamp_subsec_nanos() == 0 && date.offset().local_minus_utc() % 60 == 0,
        "Unsupported Git timestamp precision"
    );
    Ok(format!(
        "{} <{}> {} {}",
        value.name,
        value.email,
        date.timestamp(),
        date.format("%z")
    ))
}
fn verify_signature_line(line: &str, value: &GitSignature) -> Result<()> {
    let prefix = format!("{} <{}> ", value.name, value.email);
    signature(value)?;
    let rest = line
        .strip_prefix(&prefix)
        .context("Signed Git identity differs")?;
    let (seconds, offset) = rest.split_once(' ').context("Incomplete signed Git date")?;
    ensure!(
        seconds.parse::<i64>()? == chrono::DateTime::parse_from_rfc3339(&value.date)?.timestamp()
            && offset.len() == 5
            && matches!(offset.as_bytes()[0], b'+' | b'-')
            && offset.as_bytes()[1..].iter().all(u8::is_ascii_digit),
        "Signed Git timestamp differs"
    );
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitCommit {
    sha: String,
    #[serde(rename = "node_id")]
    _node_id: Option<String>,
    #[serde(rename = "url")]
    _url: Option<String>,
    #[serde(rename = "html_url")]
    _html_url: Option<String>,
    author: GitSignature,
    committer: GitSignature,
    tree: GitObject,
    message: String,
    parents: Vec<GitObject>,
    verification: Verification,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitSignature {
    name: String,
    email: String,
    date: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitObject {
    sha: String,
    #[serde(rename = "url")]
    _url: Option<String>,
    #[serde(rename = "html_url")]
    _html_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Verification {
    verified: bool,
    reason: String,
    signature: Option<String>,
    payload: Option<String>,
    #[serde(rename = "verified_at")]
    _verified_at: Option<String>,
}

fn write_file(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(if executable { 0o700 } else { 0o600 })
            .custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn read_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= limit as u64,
        "Projection file unsupported/oversized"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "Projection file exceeded bound");
    Ok(bytes)
}
fn snapshot(root: &Path) -> Result<Vec<(String, u32, String)>> {
    fn walk(
        root: &Path,
        path: &Path,
        depth: usize,
        total: &mut usize,
        out: &mut Vec<(String, u32, String)>,
    ) -> Result<()> {
        ensure!(depth <= 16, "Projection directory depth exceeded");
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "Projection contains link"
            );
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o7777
            };
            #[cfg(not(unix))]
            let mode = 0;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    metadata.uid() == unsafe { libc::geteuid() } && mode & 0o7077 == 0,
                    "Projection file ownership/permissions changed"
                );
                if metadata.is_file() {
                    ensure!(
                        metadata.nlink() == 1,
                        "Projection file has external hardlinks"
                    );
                }
            }
            if metadata.is_dir() {
                walk(root, &entry.path(), depth + 1, total, out)?;
            } else {
                ensure!(
                    metadata.is_file() && out.len() < 1024,
                    "Unexpected projection file/count"
                );
                let relative = entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .context("Projection path is not UTF-8")?
                    .to_owned();
                if relative == "projection.json" {
                    continue;
                }
                let bytes = read_file(&entry.path(), 8 * 1024 * 1024)?;
                *total += bytes.len();
                ensure!(
                    *total <= 10 * 1024 * 1024,
                    "Projection snapshot byte bound exceeded"
                );
                out.push((
                    relative,
                    mode,
                    Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                ));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, 0, &mut 0, &mut files)?;
    files.sort();
    Ok(files)
}
fn verify_projection(repo: &Repository, landed: &LandedSource, source_root: &Path) -> Result<()> {
    ensure!(repo.is_bare(), "Projection repository must be bare");
    let odb = repo.odb()?;
    let commit = odb.read(oid(&landed.merged_commit)?)?;
    ensure!(
        commit.kind() == ObjectType::Commit
            && Oid::hash_object(commit.kind(), commit.data())? == commit.id(),
        "Projection commit OID differs"
    );
    ensure!(
        repo.find_commit(commit.id())?.tree_id() == oid(&landed.source_tree)?,
        "Projection source tree differs"
    );
    let mut current = oid(&landed.source_tree)?;
    for part in landed.skill_path.split('/') {
        let object = odb.read(current)?;
        ensure!(
            object.kind() == ObjectType::Tree
                && Oid::hash_object(object.kind(), object.data())? == current,
            "Projection ancestor OID differs"
        );
        let tree = repo.find_tree(current)?;
        let entry = tree.get_name(part).context("Projection ancestry missing")?;
        ensure!(
            entry.kind() == Some(ObjectType::Tree),
            "Projection ancestry is not a tree"
        );
        current = entry.id();
    }
    ensure!(
        current == oid(&landed.skill_tree)?,
        "Projection skill tree differs"
    );
    let mut files = Vec::new();
    let mut total = 0;
    fn walk(
        repo: &Repository,
        tree: Oid,
        prefix: &str,
        depth: usize,
        out: &mut Vec<(String, i32, String)>,
        total: &mut usize,
    ) -> Result<()> {
        ensure!(depth <= 8, "Projection skill too deep");
        let odb = repo.odb()?;
        let raw = odb.read(tree)?;
        ensure!(
            raw.kind() == ObjectType::Tree && Oid::hash_object(raw.kind(), raw.data())? == tree,
            "Projection subtree OID differs"
        );
        for entry in &repo.find_tree(tree)? {
            let name = entry.name().context("Projection skill path is not UTF8")?;
            component(name)?;
            let path = format!("{prefix}{name}");
            if entry.kind() == Some(ObjectType::Tree) {
                walk(repo, entry.id(), &format!("{path}/"), depth + 1, out, total)?;
            } else {
                ensure!(
                    matches!(entry.filemode(), 0o100644 | 0o100755)
                        && entry.kind() == Some(ObjectType::Blob)
                        && out.len() < 32,
                    "Unsupported projection skill entry"
                );
                let raw = odb.read(entry.id())?;
                ensure!(
                    raw.kind() == ObjectType::Blob
                        && raw.len() <= 65_536
                        && Oid::hash_object(raw.kind(), raw.data())? == entry.id(),
                    "Projection blob OID differs"
                );
                *total += raw.len();
                ensure!(*total <= 256 * 1024, "Projection skill byte bound exceeded");
                out.push((path, entry.filemode(), entry.id().to_string()));
            }
        }
        Ok(())
    }
    walk(repo, current, "", 0, &mut files, &mut total)?;
    files.sort();
    ensure!(
        digest(&files)? == landed.content_digest,
        "Projection skill fingerprint differs"
    );
    let mut actual = Vec::new();
    fn disk(root: &Path, path: &Path, depth: usize, out: &mut Vec<String>) -> Result<()> {
        ensure!(depth <= 8, "Projection source snapshot too deep");
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            ensure!(!metadata.file_type().is_symlink(), "Projection source link");
            if metadata.is_dir() {
                disk(root, &entry.path(), depth + 1, out)?;
            } else {
                ensure!(
                    metadata.is_file() && out.len() < 32,
                    "Projection source snapshot has extra files"
                );
                out.push(
                    entry
                        .path()
                        .strip_prefix(root)?
                        .to_str()
                        .context("Projection source path is not UTF8")?
                        .to_owned(),
                );
            }
        }
        Ok(())
    }
    disk(source_root, source_root, 0, &mut actual)?;
    actual.sort();
    ensure!(
        actual == files.iter().map(|f| f.0.clone()).collect::<Vec<_>>(),
        "Projection source snapshot paths differ"
    );
    for (path, mode, sha) in files {
        let bytes = read_file(&source_root.join(&path), 65_536)?;
        ensure!(
            Oid::hash_object(ObjectType::Blob, &bytes)? == oid(&sha)?,
            "Projection source snapshot bytes differ"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                (std::fs::metadata(source_root.join(path))?
                    .permissions()
                    .mode()
                    & 0o100
                    != 0)
                    == (mode == 0o100755),
                "Projection source executable mode differs"
            );
        }
    }
    Ok(())
}
fn sync_tree(path: &Path) -> Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        } else {
            std::fs::File::open(entry.path())?.sync_all()?;
        }
    }
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}
