use super::*;

use std::fs::{File, Metadata};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

struct OpenedSource {
    file: File,
    hierarchy: Vec<(libc::dev_t, libc::ino_t)>,
}

fn open_source(path: &Path) -> Result<OpenedSource, SourceError> {
    use nix::fcntl::{open, openat, OFlag};
    use nix::sys::stat::{fstat, Mode};
    if !path.is_absolute() {
        return Err(SourceError::Unavailable);
    }
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut directory =
        open(Path::new("/"), flags, Mode::empty()).map_err(|_| SourceError::Unavailable)?;
    let mut hierarchy = Vec::new();
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            if component == Component::RootDir {
                continue;
            }
            return Err(SourceError::Unavailable);
        };
        let stat = fstat(&directory).map_err(|_| SourceError::Unavailable)?;
        hierarchy.push((stat.st_dev, stat.st_ino));
        if components.peek().is_some() {
            directory = openat(&directory, name, flags, Mode::empty())
                .map_err(|_| SourceError::Unavailable)?;
        } else {
            let fd = openat(
                &directory,
                name,
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
                Mode::empty(),
            )
            .map_err(|_| SourceError::Unavailable)?;
            let file = File::from(fd);
            let metadata = file.metadata().map_err(|_| SourceError::Unavailable)?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.len() > MAX_SOURCE_BYTES as u64
            {
                return Err(SourceError::Unavailable);
            }
            return Ok(OpenedSource { file, hierarchy });
        }
    }
    Err(SourceError::Unavailable)
}

fn stamp(metadata: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.uid(),
    )
}

fn read_admitted(
    reader: &mut impl Read,
    expected_sid: &str,
    started: Instant,
) -> Result<Vec<u8>, SourceError> {
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= MAX_RECORD_BYTES || started.elapsed() > Duration::from_secs(1) {
            return Err(SourceError::Unavailable);
        }
        let mut byte = [0_u8];
        reader
            .read_exact(&mut byte)
            .map_err(|_| SourceError::Unavailable)?;
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    let header: Value = serde_json::from_slice(&bytes).map_err(|_| SourceError::Unavailable)?;
    if header.get("type").and_then(Value::as_str) != Some("session")
        || header.get("id").and_then(Value::as_str) != Some(expected_sid)
    {
        return Err(SourceError::Unavailable);
    }
    reader
        .take((MAX_SOURCE_BYTES + 1 - bytes.len()) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| SourceError::Unavailable)?;
    if bytes.len() > MAX_SOURCE_BYTES || started.elapsed() > Duration::from_secs(1) {
        return Err(SourceError::Unavailable);
    }
    Ok(bytes)
}

fn read_with_open_hook(
    path: &Path,
    expected_sid: &str,
    after_open: impl FnOnce(),
) -> Result<Vec<u8>, SourceError> {
    let started = Instant::now();
    let mut opened = open_source(path)?;
    let original = opened
        .file
        .metadata()
        .map_err(|_| SourceError::Unavailable)?;
    after_open();
    let bytes = read_admitted(&mut opened.file, expected_sid, started)?;
    let descriptor = opened
        .file
        .metadata()
        .map_err(|_| SourceError::Unavailable)?;
    let rebound = open_source(path)?;
    let published = rebound
        .file
        .metadata()
        .map_err(|_| SourceError::Unavailable)?;
    if opened.hierarchy != rebound.hierarchy
        || stamp(&original) != stamp(&descriptor)
        || stamp(&original) != stamp(&published)
        || started.elapsed() > Duration::from_secs(1)
    {
        return Err(SourceError::Stale);
    }
    Ok(bytes)
}

pub(super) fn read_owned_transcript(
    path: &Path,
    expected_sid: &str,
) -> Result<Vec<u8>, SourceError> {
    read_with_open_hook(path, expected_sid, || {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn foreign_header_stops_before_any_conversation_body_read() {
        struct Observed {
            source: Cursor<Vec<u8>>,
            header_end: usize,
            body_bytes: usize,
        }
        impl Read for Observed {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let before = self.source.position() as usize;
                let n = self.source.read(buffer)?;
                self.body_bytes += (before + n).saturating_sub(self.header_end)
                    - before.saturating_sub(self.header_end);
                Ok(n)
            }
        }
        for header in [
            r#"{"type":"session","id":"foreign"}"#,
            r#"{"type":"message","id":"expected"}"#,
        ] {
            let header = format!("{header}\n");
            let mut reader = Observed {
                source: Cursor::new(format!("{header}PRIVATE_CONVERSATION\n").into_bytes()),
                header_end: header.len(),
                body_bytes: 0,
            };
            assert!(read_admitted(&mut reader, "expected", std::time::Instant::now()).is_err());
            assert_eq!(reader.body_bytes, 0, "foreign admission cannot read ahead");
        }
    }

    #[test]
    fn directory_rebind_rejects_parent_and_ancestor_substitution() {
        for (ancestor, symlink) in [(false, false), (false, true), (true, false), (true, true)] {
            let temp = tempfile::tempdir().unwrap();
            let base = temp.path().canonicalize().unwrap();
            let parent = base.join("ancestor/parent");
            std::fs::create_dir_all(&parent).unwrap();
            let path = parent.join("transcript.jsonl");
            let bytes = b"{\"type\":\"session\",\"id\":\"expected\"}\n{\"body\":\"SYNTHETIC\"}\n";
            std::fs::write(&path, bytes).unwrap();
            let original = if ancestor {
                base.join("ancestor")
            } else {
                parent.clone()
            };
            let saved = base.join("detached");
            let (opened, ready) = std::sync::mpsc::channel();
            let (changed, done) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                let target = path.clone();
                scope.spawn(move || {
                    ready
                        .recv_timeout(std::time::Duration::from_secs(3))
                        .unwrap();
                    std::fs::rename(&original, &saved).unwrap();
                    if symlink {
                        std::os::unix::fs::symlink(&saved, &original).unwrap();
                    } else {
                        std::fs::create_dir_all(&parent).unwrap();
                        std::fs::write(&target, bytes).unwrap();
                    }
                    changed.send(()).unwrap();
                });
                let result = read_with_open_hook(&path, "expected", || {
                    opened.send(()).unwrap();
                    done.recv_timeout(std::time::Duration::from_secs(3))
                        .unwrap();
                });
                assert!(
                    result.is_err(),
                    "detached descriptor cannot qualify published hierarchy"
                );
            });
        }
    }
}
