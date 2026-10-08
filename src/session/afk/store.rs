use super::*;
use crate::session::AnchoredDir;
use fs2::FileExt;
use std::fs::{File, Permissions};
use std::io::Cursor;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

pub(crate) fn initialize() -> Result<()> {
    let app = AnchoredDir::open(&super::super::get_app_dir()?)?;
    let dir = app.create_child(Path::new("afk"))?;
    private_dir(&dir)?;
    Ok(())
}

fn private_dir(dir: &AnchoredDir) -> Result<()> {
    let stat = dir.metadata()?;
    ensure!(
        (stat.st_mode & libc::S_IFMT) == libc::S_IFDIR
            && stat.st_uid == unsafe { libc::geteuid() }
            && stat.st_mode & 0o777 == 0o700,
        "AFK directory must be private, owned, and not a symlink"
    );
    Ok(())
}

pub(super) struct Store {
    dir: AnchoredDir,
    limit: usize,
}
impl Store {
    pub(super) fn open(app_dir: &Path, id: &str, create: bool) -> Result<Self> {
        Self::open_namespace(app_dir, id, create, "afk", MAX_BYTES)
    }
    pub(super) fn runtime(app_dir: &Path, id: &str, create: bool) -> Result<Self> {
        Self::open_namespace(
            app_dir,
            id,
            create,
            "afk-runtime-v3",
            super::delegation::SESSION_BYTES,
        )
    }
    fn open_namespace(
        app_dir: &Path,
        id: &str,
        create: bool,
        namespace: &str,
        limit: usize,
    ) -> Result<Self> {
        super::super::validate_instance_id(id)?;
        let app = AnchoredDir::open(app_dir)?;
        let root = if create {
            app.create_child(Path::new(namespace))?
        } else {
            app.child(Path::new(namespace))?
        };
        private_dir(&root)?;
        let dir = if create {
            root.create_child(Path::new(id))?
        } else {
            root.child(Path::new(id))?
        };
        private_dir(&dir)?;
        Ok(Self { dir, limit })
    }
    fn check_leaf(&self, leaf: &str) -> Result<bool> {
        let Some(stat) = self.dir.entry_stat(Path::new(leaf))? else {
            return Ok(false);
        };
        ensure!(
            (stat.st_mode & libc::S_IFMT) == libc::S_IFREG
                && stat.st_uid == unsafe { libc::geteuid() }
                && stat.st_mode & 0o777 == 0o600
                && stat.st_nlink <= 1
                && stat.st_size >= 0
                && stat.st_size as u64 <= self.limit as u64,
            "unsafe or oversized AFK file: {leaf}"
        );
        Ok(true)
    }
    pub(super) fn read<T: serde::de::DeserializeOwned>(&self, leaf: &str) -> Result<Option<T>> {
        if !self.check_leaf(leaf)? {
            return Ok(None);
        }
        let file = self
            .dir
            .open_regular(Path::new(leaf), self.limit)?
            .context("AFK file changed while opening")?;
        read_snapshot_bounded(file, leaf, self.limit).map(Some)
    }
    pub(super) fn write(&self, leaf: &str, value: &impl Serialize) -> Result<()> {
        self.check_leaf(leaf)?;
        let bytes = serde_json::to_vec(value)?;
        ensure!(bytes.len() <= self.limit, "oversized AFK publication");
        self.dir.publish_file(
            Path::new(leaf),
            &mut Cursor::new(bytes),
            Permissions::from_mode(0o600),
            true,
            None,
        )?;
        Ok(())
    }
    pub(super) fn lock(&self, timeout: Duration) -> Result<File> {
        let file = match self.dir.create_new_regular(Path::new("writer.lock"))? {
            Some(file) => {
                file.sync_all()?;
                self.dir.sync()?;
                file
            }
            None => {
                self.check_leaf("writer.lock")?;
                self.dir
                    .open_regular(Path::new("writer.lock"), MAX_BYTES)?
                    .context("AFK lock changed")?
            }
        };
        let meta = file.metadata()?;
        ensure!(
            meta.uid() == unsafe { libc::geteuid() }
                && meta.mode() & 0o777 == 0o600
                && meta.nlink() == 1,
            "unsafe AFK lock"
        );
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) => return Err(e).context("AFK writer is busy"),
            }
        }
        let stat = self
            .dir
            .entry_stat(Path::new("writer.lock"))?
            .context("AFK lock disappeared")?;
        ensure!(
            i128::from(stat.st_dev) == i128::from(meta.dev()) && stat.st_ino == meta.ino(),
            "AFK lock replaced"
        );
        Ok(file)
    }
}

#[cfg(test)]
fn read_snapshot<T: serde::de::DeserializeOwned>(file: File, leaf: &str) -> Result<T> {
    read_snapshot_bounded(file, leaf, MAX_BYTES)
}
fn read_snapshot_bounded<T: serde::de::DeserializeOwned>(
    file: File,
    leaf: &str,
    limit: usize,
) -> Result<T> {
    // Atomic replacement can unlink an already-open, still-valid snapshot.
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o777 == 0o600
            && meta.nlink() <= 1,
        "unsafe AFK file ownership"
    );
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "oversized AFK file");
    serde_json::from_slice(&bytes).with_context(|| format!("invalid {leaf}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn open_snapshot_survives_replacement_but_external_hardlinks_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "snapshot", true).unwrap();
        store.write("ack.json", &json!({"revision": 1})).unwrap();
        let snapshot = store
            .dir
            .open_regular(Path::new("ack.json"), MAX_BYTES)
            .unwrap()
            .unwrap();
        store.write("ack.json", &json!({"revision": 2})).unwrap();
        assert_eq!(snapshot.metadata().unwrap().nlink(), 0);
        assert_eq!(
            read_snapshot::<Value>(snapshot, "ack.json").unwrap(),
            json!({"revision": 1})
        );
        assert_eq!(
            store.read::<Value>("ack.json").unwrap(),
            Some(json!({"revision": 2}))
        );

        std::fs::hard_link(
            root.path().join("afk/snapshot/ack.json"),
            root.path().join("outside-link"),
        )
        .unwrap();
        assert!(store.read::<Value>("ack.json").is_err());
        assert!(store.write("ack.json", &json!({"revision": 3})).is_err());
    }
}
