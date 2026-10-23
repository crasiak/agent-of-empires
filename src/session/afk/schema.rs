use crate::session::AnchoredDir;
use anyhow::{ensure, Context, Result};
use std::io::Read;
use std::path::Path;

// Global runner positions, not frozen AFK manifest or fence versions.
pub(crate) const CONTROL: u32 = 42;
pub(crate) const RUNTIME: u32 = 43;
pub(crate) const SETTLEMENT: u32 = 44;
pub(crate) const QUESTIONS: u32 = 45;
pub(crate) const AUTHORITY: u32 = 46;
pub(crate) const BATCHES: u32 = 47;
pub(crate) const USAGE: u32 = 48;
pub(crate) const REVIEW: u32 = 49;

const ROOTS: [(&str, u32); 8] = [
    ("afk", CONTROL),
    ("afk-runtime-v2", RUNTIME),
    ("afk-runtime-v3", SETTLEMENT),
    ("afk-runtime-v4", QUESTIONS),
    ("afk-authority-v1", AUTHORITY),
    ("afk-authority-v2", BATCHES),
    ("afk-authority-v3", USAGE),
    ("afk-review-v1", REVIEW),
];

pub(crate) fn includes_migration(current: u32, introduced: u32) -> bool {
    (introduced..=crate::migrations::current_schema_version()).contains(&current)
}

pub(crate) fn read_cursor(app: &Path) -> Result<u32> {
    let dir = AnchoredDir::open(app)?;
    let path = Path::new(".schema_version");
    let Some(stat) = dir.entry_stat(path)? else {
        return Ok(0);
    };
    ensure!(
        stat.st_mode & libc::S_IFMT == libc::S_IFREG
            && stat.st_uid == unsafe { libc::geteuid() }
            && stat.st_size >= 0
            && stat.st_size <= 16,
        "schema cursor must be a bounded, owned regular file"
    );
    let file = dir
        .open_regular(path, 16)?
        .context("schema cursor changed or is unsafe")?;
    let mut bytes = Vec::with_capacity(16);
    file.take(17).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 16, "schema cursor exceeds its read limit");
    std::str::from_utf8(&bytes)?
        .trim()
        .parse()
        .context("invalid schema cursor")
}

pub(crate) fn ensure_mainline(app: &Path, current: u32) -> Result<()> {
    let app = AnchoredDir::open(app)?;
    for (namespace, introduced) in ROOTS {
        let path = Path::new(namespace);
        let Some(stat) = app.entry_stat(path)? else {
            continue;
        };
        ensure!(
            current >= introduced - 1
                && stat.st_mode & libc::S_IFMT == libc::S_IFDIR
                && stat.st_uid == unsafe { libc::geteuid() },
            "unsupported AFK development state at {namespace}; preserve it for its frozen development build, do not relabel the schema cursor"
        );
        if introduced <= QUESTIONS && current == introduced - 1 {
            let root = app.child(path)?;
            super::store::private_dir(&root).with_context(|| {
                format!("unsupported AFK development state at {namespace}; uncommitted initializer must be private")
            })?;
            ensure!(
                root.is_empty()?,
                "unsupported AFK development contents at {namespace}; no automatic development-store conversion"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn root(app: &Path, namespace: &str) -> std::path::PathBuf {
        let root = app.join(namespace);
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    #[test]
    fn namespace_boundaries_refuse_old_layouts_but_allow_canonical_partial_work() {
        for (namespace, introduced) in ROOTS {
            let app = tempfile::tempdir().unwrap();
            assert!(ensure_mainline(app.path(), introduced - 1).is_ok());
            let path = root(app.path(), namespace);
            assert!(ensure_mainline(app.path(), introduced - 2).is_err());
            assert!(ensure_mainline(app.path(), introduced - 1).is_ok());
            fs::write(path.join(".original"), b"preserve").unwrap();
            assert_eq!(
                ensure_mainline(app.path(), introduced - 1).is_ok(),
                introduced > QUESTIONS
            );
            assert!(ensure_mainline(app.path(), introduced).is_ok());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).unwrap();
            let accepted = ensure_mainline(app.path(), REVIEW).is_ok();
            let original = fs::read(path.join(".original")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(
                accepted,
                "sealed predecessor roots are valid after authority transition"
            );
            assert_eq!(original, b"preserve");
        }
    }

    #[test]
    fn pending_initializer_requires_a_private_empty_directory_without_following_links() {
        for (namespace, introduced) in ROOTS.into_iter().take(4) {
            for kind in [
                "file",
                "symlink",
                "dangling",
                "mode",
                "hidden",
                "lock",
                "directory",
            ] {
                let app = tempfile::tempdir().unwrap();
                let path = app.path().join(namespace);
                let other = tempfile::tempdir().unwrap();
                match kind {
                    "file" => fs::write(&path, b"untouched").unwrap(),
                    "symlink" => symlink(other.path(), &path).unwrap(),
                    "dangling" => symlink(other.path().join("absent"), &path).unwrap(),
                    _ => {
                        root(app.path(), namespace);
                        match kind {
                            "mode" => fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                                .unwrap(),
                            "hidden" => fs::write(path.join(".original"), b"preserve").unwrap(),
                            "lock" => fs::write(path.join("writer.lock"), b"preserve").unwrap(),
                            "directory" => fs::create_dir(path.join("child")).unwrap(),
                            _ => unreachable!(),
                        }
                    }
                }
                assert!(
                    ensure_mainline(app.path(), introduced - 1).is_err(),
                    "{namespace}/{kind}"
                );
                assert!(fs::read_dir(other.path()).unwrap().next().is_none());
            }
        }
    }

    #[test]
    fn only_an_absent_cursor_means_zero_and_unsafe_or_malformed_entries_are_refused() {
        for kind in [
            "missing",
            "valid",
            "empty",
            "text",
            "overflow",
            "utf8",
            "oversize",
            "directory",
            "symlink",
            "dangling",
            "fifo",
        ] {
            let app = tempfile::tempdir().unwrap();
            let path = app.path().join(".schema_version");
            let other = tempfile::tempdir().unwrap();
            let target = other.path().join("original");
            fs::write(&target, b"49").unwrap();
            match kind {
                "missing" => {}
                "valid" => fs::write(&path, b"49\n").unwrap(),
                "empty" => fs::write(&path, b"").unwrap(),
                "text" => fs::write(&path, b"unknown").unwrap(),
                "overflow" => fs::write(&path, b"4294967296").unwrap(),
                "utf8" => fs::write(&path, [255]).unwrap(),
                "oversize" => fs::write(&path, [b'4'; 17]).unwrap(),
                "directory" => fs::create_dir(&path).unwrap(),
                "symlink" => symlink(&target, &path).unwrap(),
                "dangling" => symlink(other.path().join("absent"), &path).unwrap(),
                "fifo" => nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR).unwrap(),
                _ => unreachable!(),
            }
            let result = read_cursor(app.path());
            match kind {
                "missing" => assert_eq!(result.unwrap(), 0),
                "valid" => assert_eq!(result.unwrap(), 49),
                _ => assert!(result.is_err(), "{kind}"),
            }
            assert_eq!(fs::read(&target).unwrap(), b"49");
        }
    }
}
