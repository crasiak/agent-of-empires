//! Cross-request cache for [`resolve_config_with_repo_or_warn`].
//!
//! The daemon re-projects every session on each publication (every 500 ms
//! while a TUI subscribes). Resolving one `(profile, project_path)` pair from
//! scratch parses the global and profile TOML, type-checks the overrides
//! against a full `Config`, and opens the repository with libgit2 to find the
//! main worktree. At ~100 unique pairs that is seconds per publication, so the
//! publish lane never idles and every mutation queues behind it until the TUI
//! quarantines the row as an unknown outcome.
//!
//! An entry is revalidated on every hit by `stat`-ing the files it was derived
//! from, so a config edit still lands on the next publication while an
//! unchanged pair costs a handful of `metadata` calls instead of a resolve.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::{
    repo_config_source_path, resolve_config_with_repo_source_or_warn, LEGACY_REPO_CONFIG_PATH,
    REPO_CONFIG_PATH,
};
use crate::session::config::Config;

/// Modification time and length of one input; `None` when absent or unreadable.
type Stamp = Option<(Option<SystemTime>, u64)>;

fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.modified().ok(), meta.len()))
}

/// Every file a resolved pair was read from, or probed and found absent.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    global: Stamp,
    profile: Stamp,
    /// `<project>/.git`: the repo layout decides where the repo config is read
    /// from, so a layout change re-resolves the source path.
    git_marker: Stamp,
    repo: Stamp,
    legacy_repo: Stamp,
}

impl Fingerprint {
    /// Taken before the resolve reads anything, so a write that races the
    /// resolve shows up as a change on the next hit rather than being lost.
    fn take(profile: &str, project_path: &Path, source: &Path) -> Self {
        let global = crate::session::get_app_dir_path()
            .ok()
            .map(|dir| dir.join("config.toml"))
            .as_deref()
            .and_then(stamp);
        let profile = crate::session::get_profile_dir_path(profile)
            .ok()
            .map(|dir| dir.join("config.toml"))
            .as_deref()
            .and_then(stamp);
        let git_marker = (!project_path.as_os_str().is_empty())
            .then(|| stamp(&project_path.join(".git")))
            .flatten();
        let (repo, legacy_repo) = if source.as_os_str().is_empty() {
            (None, None)
        } else {
            (
                stamp(&source.join(REPO_CONFIG_PATH)),
                stamp(&source.join(LEGACY_REPO_CONFIG_PATH)),
            )
        };
        Self {
            global,
            profile,
            git_marker,
            repo,
            legacy_repo,
        }
    }
}

struct Entry {
    fingerprint: Fingerprint,
    config: Arc<Config>,
    last_used: Instant,
}

struct SourceEntry {
    git_marker: Stamp,
    source: PathBuf,
    last_used: Instant,
}

/// Resolved configs and repository sources shared across requests.
#[derive(Default)]
pub struct ResolvedConfigCache {
    entries: Mutex<HashMap<(String, PathBuf), Entry>>,
    sources: Mutex<HashMap<PathBuf, SourceEntry>>,
    #[cfg(test)]
    pub(crate) source_discoveries: std::sync::atomic::AtomicUsize,
}

/// One lookup's outcome: the config and whether it came from disk.
pub struct Resolved {
    pub config: Arc<Config>,
    /// `true` when this call read and merged the config files.
    pub miss: bool,
}

impl ResolvedConfigCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the pair, reading from disk only when no entry exists or one
    /// of its input files changed since it was resolved. The lock is never
    /// held across the disk read, so a miss does not stall concurrent hits.
    pub fn resolve_or_warn(&self, profile: &str, project_path: &Path) -> Resolved {
        let source = self.source_path(project_path);
        self.resolve_source_or_warn(profile, &source)
    }

    pub fn source_path(&self, project_path: &Path) -> PathBuf {
        let git_marker = (!project_path.as_os_str().is_empty())
            .then(|| stamp(&project_path.join(".git")))
            .flatten();
        {
            let mut sources = self
                .sources
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = sources.get_mut(project_path) {
                if entry.git_marker == git_marker {
                    entry.last_used = Instant::now();
                    return entry.source.clone();
                }
            }
        }
        #[cfg(test)]
        self.source_discoveries
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let source = repo_config_source_path(project_path);
        self.sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                project_path.to_path_buf(),
                SourceEntry {
                    git_marker,
                    source: source.clone(),
                    last_used: Instant::now(),
                },
            );
        source
    }

    /// Resolve an already discovered source, shared by linked worktrees.
    pub fn resolve_source_or_warn(&self, profile: &str, source: &Path) -> Resolved {
        let key = (profile.to_string(), source.to_path_buf());
        {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = entries.get_mut(&key) {
                if entry.fingerprint == Fingerprint::take(profile, source, source) {
                    entry.last_used = Instant::now();
                    return Resolved {
                        config: entry.config.clone(),
                        miss: false,
                    };
                }
            }
        }

        let fingerprint = Fingerprint::take(profile, source, source);
        let config = Arc::new(resolve_config_with_repo_source_or_warn(profile, source));
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.insert(
            key,
            Entry {
                fingerprint,
                config: config.clone(),
                last_used: Instant::now(),
            },
        );
        Resolved { config, miss: true }
    }

    /// Drop entries no request has used for `idle`, so pairs of deleted
    /// sessions do not accumulate over a long daemon uptime.
    pub fn evict_idle(&self, idle: Duration) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.last_used.elapsed() < idle);
        self.sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, entry| entry.last_used.elapsed() < idle);
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn profile_config_path(profile: &str) -> PathBuf {
        crate::session::get_profile_dir_path(profile)
            .unwrap()
            .join("config.toml")
    }

    #[test]
    #[serial_test::serial]
    fn second_lookup_is_a_hit_on_the_same_config() {
        let home = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(home.path());
        let project = tempfile::tempdir().unwrap();
        let cache = ResolvedConfigCache::new();

        let first = cache.resolve_or_warn("default", project.path());
        let second = cache.resolve_or_warn("default", project.path());

        assert!(first.miss, "an empty cache reads from disk");
        assert!(!second.miss, "an unchanged pair is served from the cache");
        assert!(
            Arc::ptr_eq(&first.config, &second.config),
            "a hit returns the entry that was resolved, not a copy"
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    #[serial_test::serial]
    fn profile_config_edit_is_seen_on_the_next_lookup() {
        let home = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(home.path());
        let project = tempfile::tempdir().unwrap();
        let cache = ResolvedConfigCache::new();
        write(
            &profile_config_path("default"),
            "[session]\ndefault_tool = \"a\"\n",
        );

        let before = cache.resolve_or_warn("default", project.path());
        // A different length, so the stamp changes even within one mtime tick.
        write(
            &profile_config_path("default"),
            "[session]\ndefault_tool = \"longer\"\n",
        );
        let after = cache.resolve_or_warn("default", project.path());

        assert_eq!(before.config.session.default_tool.as_deref(), Some("a"));
        assert!(after.miss, "a changed profile file must be re-read");
        assert_eq!(after.config.session.default_tool.as_deref(), Some("longer"));
    }

    #[test]
    #[serial_test::serial]
    fn repo_config_appearing_is_seen_on_the_next_lookup() {
        let home = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(home.path());
        let project = tempfile::tempdir().unwrap();
        let cache = ResolvedConfigCache::new();

        let before = cache.resolve_or_warn("default", project.path());
        write(
            &project.path().join(REPO_CONFIG_PATH),
            "[worktree]\nauto_cleanup = false\n",
        );
        let after = cache.resolve_or_warn("default", project.path());

        assert!(before.config.worktree.auto_cleanup, "the default is on");
        assert!(after.miss, "a repo config that appeared must be merged");
        assert!(!after.config.worktree.auto_cleanup);
    }

    #[test]
    #[serial_test::serial]
    fn evict_idle_keeps_recently_used_entries_only() {
        let home = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(home.path());
        let stale = tempfile::tempdir().unwrap();
        let fresh = tempfile::tempdir().unwrap();
        let cache = ResolvedConfigCache::new();

        cache.resolve_or_warn("default", stale.path());
        cache.resolve_or_warn("default", fresh.path());
        let expired = Instant::now() - Duration::from_secs(60);
        cache
            .entries
            .lock()
            .unwrap()
            .get_mut(&("default".into(), stale.path().to_path_buf()))
            .unwrap()
            .last_used = expired;
        cache
            .sources
            .lock()
            .unwrap()
            .get_mut(stale.path())
            .unwrap()
            .last_used = expired;
        cache.evict_idle(Duration::from_secs(30));

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.sources.lock().unwrap().len(), 1);
        assert!(!cache.resolve_or_warn("default", fresh.path()).miss);
        assert!(cache.resolve_or_warn("default", stale.path()).miss);
    }
}
