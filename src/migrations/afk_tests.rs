use super::*;
use crate::session::afk;
use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const ROOTS: [&str; 8] = [
    "afk",
    "afk-runtime-v2",
    "afk-runtime-v3",
    "afk-runtime-v4",
    "afk-authority-v1",
    "afk-authority-v2",
    "afk-authority-v3",
    "afk-review-v1",
];

struct CleanupRoots(PathBuf);
impl Drop for CleanupRoots {
    fn drop(&mut self) {
        for name in ROOTS {
            for suffix in ["", "/profiles"] {
                let _ = fs::set_permissions(
                    self.0.join(format!("{name}{suffix}")),
                    fs::Permissions::from_mode(0o700),
                );
            }
        }
    }
}

fn tree(path: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u32, u64)> {
    let mut entries = BTreeMap::new();
    for item in fs::read_dir(path).unwrap() {
        let path = item.unwrap().path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let bytes = if metadata.is_dir() {
            Vec::new()
        } else {
            fs::read(&path).unwrap()
        };
        entries.insert(path.clone(), (bytes, metadata.mode(), metadata.ino()));
        if metadata.is_dir() {
            entries.extend(tree(&path));
        }
    }
    entries
}

fn reporter() -> (progress::Reporter, Arc<Mutex<Vec<u32>>>) {
    let versions = Arc::new(Mutex::new(Vec::new()));
    let output = versions.clone();
    (
        Arc::new(move |event| {
            if let progress::Event::Started { version, .. } = event {
                output.lock().unwrap().push(version);
            }
        }),
        versions,
    )
}

fn legacy_roots(app: &Path) {
    afk::initialize().unwrap();
    afk::delegation::initialize().unwrap();
    afk::delegation::initialize_settlement().unwrap();
    afk::delegation::initialize_questions().unwrap();
    fs::write(app.join(VERSION_FILE), afk_schema::QUESTIONS.to_string()).unwrap();
}

#[test]
#[serial_test::serial]
fn frozen_development_layouts_are_refused_before_any_migration_body_or_cursor_write() {
    for version in 36..=43 {
        let _home = crate::session::test_support::isolate_app_dir();
        let app = crate::session::get_app_dir().unwrap();
        let _cleanup = CleanupRoots(app.clone());
        drop(runner_lock::acquire(std::time::Duration::ZERO).unwrap());
        fs::write(app.join(VERSION_FILE), version.to_string()).unwrap();
        for (index, namespace) in ROOTS.iter().enumerate().take((version - 35) as usize) {
            let root = app.join(namespace);
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(
                root.join(".original"),
                format!("held intent and counters {index}"),
            )
            .unwrap();
            if index < 4 && version >= 40 {
                fs::set_permissions(root, fs::Permissions::from_mode(0o500)).unwrap();
            }
        }
        let before = tree(&app);
        let (report, started) = reporter();
        let error = run_migrations_with(Some(report)).unwrap_err();
        assert!(
            error.to_string().contains("unsupported AFK development"),
            "{version}: {error:#}"
        );
        assert!(started.lock().unwrap().is_empty());
        assert_eq!(
            tree(&app),
            before,
            "{version}: no AFK conversion or cursor publication"
        );
    }
}

#[test]
#[serial_test::serial]
fn canonical_main_and_every_afk_cursor_boundary_resume_to_review_without_renumbering_formats() {
    let _agent_homes = super::hook_fixtures::unset_agent_home_env();
    for version in [0, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48] {
        let _home = crate::session::test_support::isolate_app_dir();
        let app = crate::session::get_app_dir().unwrap();
        let _cleanup = CleanupRoots(app.clone());
        if version >= afk_schema::QUESTIONS {
            legacy_roots(&app);
            afk::authority::migration::run_at(&app).unwrap();
            if version >= afk_schema::AUTHORITY {
                fs::write(app.join(VERSION_FILE), afk_schema::AUTHORITY.to_string()).unwrap();
                afk::authority::migration_v2::run_at(&app).unwrap();
            }
            if version >= afk_schema::BATCHES {
                fs::write(app.join(VERSION_FILE), afk_schema::BATCHES.to_string()).unwrap();
                afk::authority::migration_v3::run_at(&app).unwrap();
            }
            if version == afk_schema::USAGE {
                afk::wake::initialize(&app).unwrap();
                fs::write(app.join(VERSION_FILE), afk_schema::USAGE.to_string()).unwrap();
                afk::review::annotations::initialize(&app).unwrap();
            }
        } else if version >= 41 {
            for namespace in ROOTS.iter().take((version - 40) as usize) {
                let root = app.join(namespace);
                fs::create_dir(&root).unwrap();
                fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        if version != 0 {
            fs::write(app.join(VERSION_FILE), version.to_string()).unwrap();
        }
        let (report, started) = reporter();
        run_migrations_with(Some(report)).unwrap();
        assert_eq!(
            *started.lock().unwrap(),
            ((version + 1)..=CURRENT_VERSION).collect::<Vec<_>>()
        );
        assert_eq!(get_current_version(), CURRENT_VERSION);
        for (namespace, format) in [
            ("afk-authority-v1", 40),
            ("afk-authority-v2", 41),
            ("afk-authority-v3", 42),
        ] {
            let manifest: serde_json::Value = serde_json::from_slice(
                &fs::read(app.join(namespace).join("migration/manifest.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(manifest["version"], format);
            assert_eq!(manifest["phase"], "complete");
        }
        afk::authority::migration_v3::run_at(&app).unwrap();
        assert!(app.join("afk-review-v1/annotations/index.json").is_file());
        let before = tree(&app);
        run_migrations().unwrap();
        assert_eq!(
            tree(&app),
            before,
            "current-schema replay is validation, not reinitialization"
        );
    }
}

#[test]
#[serial_test::serial]
fn work_arc_upgrade_from_review_preserves_authority_and_registry() {
    let _agent_homes = super::hook_fixtures::unset_agent_home_env();
    let _home = crate::session::test_support::isolate_app_dir();
    let app = crate::session::get_app_dir().unwrap();
    let _cleanup = CleanupRoots(app.clone());
    legacy_roots(&app);
    afk::authority::migration::run_at(&app).unwrap();
    set_version(afk_schema::AUTHORITY).unwrap();
    afk::authority::migration_v2::run_at(&app).unwrap();
    set_version(afk_schema::BATCHES).unwrap();
    afk::authority::migration_v3::run_at(&app).unwrap();
    afk::wake::initialize(&app).unwrap();
    set_version(afk_schema::USAGE).unwrap();
    afk::review::annotations::initialize(&app).unwrap();
    set_version(afk_schema::REVIEW).unwrap();
    drop(runner_lock::acquire(std::time::Duration::ZERO).unwrap());
    let mut row =
        serde_json::to_value(crate::session::Instance::new("legacy", "/tmp/legacy")).unwrap();
    row.as_object_mut().unwrap().remove("work_arc_enabled");
    row.as_object_mut().unwrap().remove("work_arc");
    let profile = app.join("profiles/alpha");
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        profile.join("sessions.json"),
        serde_json::to_vec(&vec![row]).unwrap(),
    )
    .unwrap();
    let mut before = tree(&app);
    before.remove(&app.join(VERSION_FILE));
    let (report, started) = reporter();

    run_migrations_with(Some(report)).unwrap();

    assert_eq!(*started.lock().unwrap(), vec![50]);
    assert_eq!(get_current_version(), 50);
    afk::authority::migration_v3::run_at(&app).unwrap();
    afk::review::annotations::initialize(&app).unwrap();
    let mut after = tree(&app);
    after.remove(&app.join(VERSION_FILE));
    assert_eq!(
        after, before,
        "consent upgrade leaves AFK and legacy rows intact"
    );
    let current = tree(&app);
    run_migrations().unwrap();
    assert_eq!(
        tree(&app),
        current,
        "replay does not rewrite migrated state"
    );
}

#[test]
#[serial_test::serial]
fn unsafe_current_cursor_and_current_version_shortcut_do_not_bypass_refusal() {
    use std::os::unix::fs::symlink;
    let _home = crate::session::test_support::isolate_app_dir();
    let app = crate::session::get_app_dir().unwrap();
    fs::write(app.join(VERSION_FILE), CURRENT_VERSION.to_string()).unwrap();
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("original"), b"preserve").unwrap();
    symlink(other.path(), app.join("afk")).unwrap();
    let (report, started) = reporter();
    assert!(run_migrations_with(Some(report)).is_err());
    assert!(started.lock().unwrap().is_empty());
    assert_eq!(
        fs::read(other.path().join("original")).unwrap(),
        b"preserve"
    );
    fs::remove_file(app.join("afk")).unwrap();
    fs::write(app.join(VERSION_FILE), b"invalid").unwrap();
    assert!(run_migrations().is_err());
    assert_eq!(fs::read(app.join(VERSION_FILE)).unwrap(), b"invalid");
    assert!(!app.join("afk-authority-v3").exists());
}
