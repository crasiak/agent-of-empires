//! Shared config resolutions with byte-exact input and repository-source validation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{discover_repo_config_source, resolve_config_with_repo_source_or_warn};
use crate::session::config::{read_file, Config, FileRead};

#[derive(PartialEq, Eq)]
struct SourceFingerprint {
    git: FileRead,
    gitdir: Option<PathBuf>,
    config: FileRead,
    main_repo: Option<PathBuf>,
}

struct Entry {
    inputs: Vec<(PathBuf, FileRead)>,
    config: Arc<Config>,
    last_used: Instant,
}

struct SourceEntry {
    fingerprint: SourceFingerprint,
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
    pub miss: bool,
}

impl ResolvedConfigCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve_or_warn(&self, profile: &str, project_path: &Path) -> Resolved {
        let source = self.source_path(project_path);
        self.resolve_source_or_warn(profile, &source)
    }

    pub fn source_path(&self, project: &Path) -> PathBuf {
        if project.as_os_str().is_empty() {
            return PathBuf::new();
        }
        let git = read_file(&project.join(".git"));
        let gitdir = git
            .content
            .as_ref()
            .ok()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|content| crate::git::GitWorktree::gitfile_target(project, content));
        // Canonicalizing the gitdir on every lookup also detects retargeted ancestor links.
        let main_repo = gitdir
            .as_deref()
            .and_then(crate::git::GitWorktree::find_main_repo_from_worktree_gitdir);
        if main_repo.is_none()
            && git.content.as_ref().is_ok_and(|bytes| std::str::from_utf8(bytes).is_ok())
            && gitdir.is_none()
        {
            return project.to_path_buf();
        }
        let config = read_file(
            &gitdir.clone().unwrap_or_else(|| project.join(".git")).join("config"),
        );
        let fingerprint = SourceFingerprint { git, gitdir, config, main_repo };
        {
            let mut sources = self.sources.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = sources.get_mut(project) {
                if entry.fingerprint == fingerprint {
                    entry.last_used = Instant::now();
                    return entry.source.clone();
                }
            }
        }
        #[cfg(test)]
        self.source_discoveries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(source) = fingerprint.main_repo.clone().or_else(|| discover_repo_config_source(project)) else {
            // Failed discovery is retried next time, not cached as a successful source.
            return project.to_path_buf();
        };
        self.sources.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(
            project.to_path_buf(),
            SourceEntry { fingerprint, source: source.clone(), last_used: Instant::now() },
        );
        source
    }

    /// Resolve from the same bytes that validate the entry, without holding a lock over I/O.
    pub fn resolve_source_or_warn(&self, profile: &str, source: &Path) -> Resolved {
        let profile = crate::session::config::effective_profile(profile);
        let key = (profile.clone(), source.to_path_buf());
        let previous = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key).map(|entry| (entry.inputs.clone(), entry.config.clone()));
        let mut seen = HashMap::new();
        if let Some((inputs, config)) = previous {
            let current = inputs.iter().all(|(path, expected)| {
                seen.entry(path.clone()).or_insert_with(|| read_file(path)) == expected
            });
            if current {
                if let Some(entry) = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_mut(&key) {
                    entry.last_used = Instant::now();
                }
                return Resolved { config, miss: false };
            }
        }
        let seen = std::cell::RefCell::new(seen);
        let inputs = std::cell::RefCell::new(Vec::new());
        let read = |path: &Path| {
            let bytes = seen.borrow_mut().entry(path.to_path_buf()).or_insert_with(|| read_file(path)).clone();
            inputs.borrow_mut().push((path.to_path_buf(), bytes.clone()));
            bytes
        };
        let config = Arc::new(resolve_config_with_repo_source_or_warn(&profile, source, &read));
        self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(
            key, Entry { inputs: inputs.into_inner(), config: config.clone(), last_used: Instant::now() },
        );
        Resolved { config, miss: true }
    }

    pub fn evict_idle(&self, idle: Duration) {
        self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, entry| entry.last_used.elapsed() < idle);
        self.sources.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, entry| entry.last_used.elapsed() < idle);
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::REPO_CONFIG_PATH;

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
        for repo in [&stale, &fresh] {
            git2::Repository::init(repo.path()).unwrap();
        }

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
