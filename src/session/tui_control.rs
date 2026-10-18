//! Registry of running TUIs, so an external tool can ask exactly one of them
//! to show a session. Each TUI owns `<app_dir>/tui-control/<pid>.json` (and,
//! in the TUI, a request socket beside it). Records are owner-only; entries
//! whose process is gone or was replaced are ignored and swept.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 1;
/// `profile` of a TUI showing every profile.
pub const ALL_PROFILES: &str = "*";
const CONTROL_DIR: &str = "tui-control";
const ACTIVITY_DIR: &str = "tui-activity";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuiRecord {
    pub protocol: u32,
    pub pid: u32,
    /// Process start identity; a reused PID never matches a stale record.
    pub start_id: String,
    pub tty: String,
    pub profile: String,
    pub attached_session_id: Option<String>,
}

/// A live record plus the time of the TUI's last real user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTui {
    pub record: TuiRecord,
    pub last_input: SystemTime,
}

/// `<app_dir>/tui-control`, created 0700. Refuses a symlink or a directory
/// owned by another user or writable by group/other.
pub fn control_dir() -> Result<PathBuf> {
    ensure_private_dir(&super::get_app_dir()?.join(CONTROL_DIR))
}

pub fn socket_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.sock"))
}

fn record_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.json"))
}

fn ensure_private_dir(dir: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Err(e) => return Err(e.into()),
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != nix::unistd::getuid().as_raw() || meta.mode() & 0o022 != 0 {
        bail!("unsafe TUI control directory: {}", dir.display());
    }
    Ok(dir.to_path_buf())
}

/// Atomically publish this TUI's record (0600, never through a symlink).
pub fn write_record(dir: &Path, record: &TuiRecord) -> Result<()> {
    let bytes = serde_json::to_vec(record)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    tmp.write_all(&bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(record_path(dir, record.pid))
        .context("publish TUI control record")?;
    Ok(())
}

/// Remove this process's record and socket.
pub fn remove_own(dir: &Path, pid: u32) {
    let _ = fs::remove_file(record_path(dir, pid));
    let _ = fs::remove_file(socket_path(dir, pid));
}

fn read_record(path: &Path) -> Option<TuiRecord> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.uid() != nix::unistd::getuid().as_raw() || meta.mode() & 0o077 != 0 {
        return None;
    }
    let record: TuiRecord = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let named = path.file_stem()?.to_str()? == record.pid.to_string();
    (named && record.protocol == PROTOCOL).then_some(record)
}

/// Live records in `dir`. `alive` decides whether a record's process still
/// exists with the same start identity; dead entries are swept.
pub fn live_records_in(
    dir: &Path,
    activity_dir: &Path,
    alive: impl Fn(&TuiRecord) -> bool,
) -> Vec<LiveTui> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut live = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(record) = read_record(&path) else {
            continue;
        };
        if !alive(&record) {
            remove_own(dir, record.pid);
            continue;
        }
        let last_input = fs::metadata(activity_dir.join(record.pid.to_string()))
            .or_else(|_| fs::metadata(&path))
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        live.push(LiveTui { record, last_input });
    }
    live
}

/// Live TUIs in the real app directory.
pub fn live_records() -> Vec<LiveTui> {
    let (Ok(dir), Ok(app)) = (control_dir(), super::get_app_dir()) else {
        return Vec::new();
    };
    live_records_in(
        &dir,
        &app.join(ACTIVITY_DIR),
        |r| matches!(crate::process::process_birth_identity(r.pid), Ok(Some(b)) if b.start_id == r.start_id),
    )
}

/// Pick exactly one TUI for `target_id`: one already showing it, else the most
/// recently used TUI on the target's profile (or showing all profiles), else
/// the most recently used TUI.
pub fn select<'a>(
    tuis: &'a [LiveTui],
    target_id: &str,
    target_profile: &str,
) -> Option<&'a LiveTui> {
    let newest = |it: Vec<&'a LiveTui>| {
        it.into_iter()
            .max_by_key(|t| (t.last_input, std::cmp::Reverse(t.record.pid)))
    };
    newest(
        tuis.iter()
            .filter(|t| t.record.attached_session_id.as_deref() == Some(target_id))
            .collect(),
    )
    .or_else(|| {
        newest(
            tuis.iter()
                .filter(|t| t.record.profile == target_profile || t.record.profile == ALL_PROFILES)
                .collect(),
        )
    })
    .or_else(|| newest(tuis.iter().collect()))
}

/// This process's start identity, or an error when it cannot be proven.
pub fn own_start_id() -> Result<String> {
    match crate::process::process_birth_identity(std::process::id())? {
        Some(birth) => Ok(birth.start_id),
        None => bail!("own process start identity unavailable"),
    }
}

/// The controlling terminal of stdin, e.g. `/dev/ttys001`.
pub fn own_tty() -> Option<String> {
    // SAFETY: ttyname returns a pointer to static storage or null.
    let ptr = unsafe { libc::ttyname(libc::STDIN_FILENO) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null ttyname result is a NUL-terminated C string.
    let name = unsafe { std::ffi::CStr::from_ptr(ptr) };
    name.to_str().ok().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn record(pid: u32, profile: &str, attached: Option<&str>) -> TuiRecord {
        TuiRecord {
            protocol: PROTOCOL,
            pid,
            start_id: format!("start-{pid}"),
            tty: format!("/dev/ttys{pid:03}"),
            profile: profile.into(),
            attached_session_id: attached.map(str::to_owned),
        }
    }

    fn tui(pid: u32, profile: &str, attached: Option<&str>, secs: u64) -> LiveTui {
        LiveTui {
            record: record(pid, profile, attached),
            last_input: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    #[test]
    fn records_are_private_atomic_and_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = ensure_private_dir(&tmp.path().join("tui-control")).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        let r = record(41, "work", Some("abc"));
        write_record(&dir, &r).unwrap();
        let path = record_path(&dir, 41);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(read_record(&path), Some(r));
    }

    #[test]
    fn unsafe_directories_and_records_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let open = tmp.path().join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(ensure_private_dir(&open).is_err());
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&open, &link).unwrap();
        assert!(ensure_private_dir(&link).is_err());

        let dir = ensure_private_dir(&tmp.path().join("ok")).unwrap();
        let r = record(7, "p", None);
        write_record(&dir, &r).unwrap();
        let path = record_path(&dir, 7);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read_record(&path), None, "group/other readable");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let renamed = record_path(&dir, 8);
        fs::rename(&path, &renamed).unwrap();
        assert_eq!(read_record(&renamed), None, "pid must match file name");
        let target = record_path(&dir, 9);
        write_record(&dir, &record(9, "p", None)).unwrap();
        let sym = record_path(&dir, 10);
        std::os::unix::fs::symlink(&target, &sym).unwrap();
        assert_eq!(read_record(&sym), None, "symlinked record");
        fs::write(record_path(&dir, 11), br#"{"protocol":1,"pid":11,"start_id":"s","tty":"t","profile":"p","attached_session_id":null,"extra":1}"#).unwrap();
        fs::set_permissions(record_path(&dir, 11), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_record(&record_path(&dir, 11)), None, "unknown field");
    }

    #[test]
    fn dead_records_are_swept_and_activity_orders_live_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = ensure_private_dir(&tmp.path().join("tui-control")).unwrap();
        let activity = tmp.path().join("tui-activity");
        fs::create_dir(&activity).unwrap();
        for pid in [1, 2] {
            write_record(&dir, &record(pid, "p", None)).unwrap();
            fs::write(socket_path(&dir, pid), b"").unwrap();
        }
        fs::write(activity.join("1"), b"").unwrap();
        let live = live_records_in(&dir, &activity, |r| r.pid == 1);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].record.pid, 1);
        assert!(!record_path(&dir, 2).exists() && !socket_path(&dir, 2).exists());
        assert_eq!(
            live[0].last_input,
            fs::metadata(activity.join("1"))
                .unwrap()
                .modified()
                .unwrap()
        );
    }

    #[test]
    fn selection_prefers_showing_then_profile_then_recency() {
        let tuis = vec![
            tui(1, "work", None, 30),
            tui(2, "personal", Some("target"), 10),
            tui(3, "personal", None, 20),
            tui(4, "work", None, 25),
        ];
        let pick = |id: &str, profile: &str, tuis: &[LiveTui]| {
            select(tuis, id, profile).map(|t| t.record.pid)
        };
        assert_eq!(
            pick("target", "work", &tuis),
            Some(2),
            "already showing wins"
        );
        assert_eq!(
            pick("other", "personal", &tuis),
            Some(3),
            "profile, newest first"
        );
        assert_eq!(pick("other", "work", &tuis), Some(1));
        assert_eq!(pick("other", "nobody", &tuis), Some(1), "global newest");
        assert_eq!(pick("x", "work", &[]), None);
        let all = vec![tui(1, "work", None, 30), tui(6, ALL_PROFILES, None, 10)];
        assert_eq!(
            pick("other", "personal", &all),
            Some(6),
            "an all-profiles TUI covers any profile"
        );
        let tie = vec![tui(9, "p", None, 5), tui(5, "p", None, 5)];
        assert_eq!(pick("x", "p", &tie), Some(5), "ties pick the lower pid");
    }
}
