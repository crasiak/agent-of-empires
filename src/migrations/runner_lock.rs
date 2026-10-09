use crate::session::AnchoredDir;
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) fn acquire(timeout: Duration) -> Result<File> {
    let dir = AnchoredDir::open(&crate::session::get_app_dir()?)?;
    let name = Path::new(".migration-runner.lock");
    let file = match dir.create_new_regular(name)? {
        Some(file) => {
            file.sync_all()?;
            dir.sync()?;
            file
        }
        None => dir
            .open_regular(name, 0)?
            .context("unsafe migration runner lock")?,
    };
    let meta = file.metadata()?;
    ensure!(
        meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o777 == 0o600
            && meta.nlink() == 1
            && meta.len() == 0,
        "unsafe migration runner lock"
    );
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => break,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(error).context(
                "migration runner busy; retry normal migration after the current attempt finishes",
            ),
        }
    }
    let current = dir
        .entry_stat(name)?
        .context("migration runner lock disappeared")?;
    ensure!(
        current.st_dev as u64 == meta.dev() && current.st_ino == meta.ino(),
        "migration runner lock replaced"
    );
    Ok(file)
}
