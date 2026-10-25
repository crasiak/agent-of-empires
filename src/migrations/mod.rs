//! One-time transformations of persisted data, run in order on upgrade. See
//! `docs/development/adding-a-migration.md` to add one.

#[cfg(test)]
mod afk_tests;
mod config_file;
pub mod progress;
mod runner_lock;
mod sessions_file;
mod store_fs;
#[cfg(test)]
mod test_cases;
mod v001_xdg_linux;
mod v002_seed_sandbox_from_volumes;
mod v003_yolo_mode_config;
mod v004_unified_environment;
mod v005_cockpit_defaults;
mod v006_unlimited_cockpit_history;
mod v007_serve_log_to_legacy;
mod v008_lock_in_default_profile;
mod v009_update_check_mode;
mod v010_drop_legacy_live_send_exit_chord;
mod v011_relocate_sandbox_image;
mod v012_acp_rename;
mod v013_strip_profile_theme;
mod v014_rename_default_theme;
mod v015_rewrite_hook_strings;
mod v016_clear_archived_tmux_gone_error;
mod v017_rewrite_hook_strings_for_per_user_base;
mod v018_strip_codex_config_toml_hooks;
mod v019_move_acp_defaults_to_acp;
mod v020_move_tui_branch_suffix_to_row_tag;
mod v021_split_app_state_to_state_toml;
mod v022_prune_tuning_settings;
mod v023_clear_structured_container_error;
mod v024_backfill_detect_as;
mod v025_reenable_confirm_delete;
mod v026_repoint_acp_default_agent;
pub(crate) mod v027_isolate_sandbox_stores;
mod v028_clear_archived_live_status;
mod v029_fold_pending_initial_turn;
mod v030_global_only_profile_settings;
mod v031_conversation_provenance;
mod v032_bound_capture_exclusions;
pub(crate) mod v033_isolate_sandbox_content;
mod v034_trash_retention_minutes;
mod v035_custom_sort_order;
mod v036_session_creation_count;
mod v037_trash_lifecycle_id;
mod v038_prime_native_runtime;
mod v039_prime_control_path;
mod v040_worker_ownership;
mod v041_repo_appearances;
mod v042_afk_control;
mod v043_afk_runtime;
mod v044_afk_settlement;
mod v045_afk_questions;
mod v046_afk_authority;
mod v047_afk_batches;
mod v048_afk_usage_guards;
mod v049_afk_return_review;
mod v050_tailored_history;
mod v051_research_requests;
mod v052_document_conversations;

/// Fixtures shared by the migrations that rewrite agent hook files.
#[cfg(test)]
mod hook_fixtures {
    use crate::session::test_support::EnvGuard;
    use serde_json::Value;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// Clear every agent config-dir override, so a migration's path
    /// resolution sees only the fixtures under `home` and `app_dir`.
    pub(super) fn unset_agent_home_env() -> EnvGuard {
        EnvGuard::unset(&[
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "CURSOR_CONFIG_DIR",
            "GEMINI_CONFIG_DIR",
            "QWEN_CONFIG_DIR",
        ])
    }

    /// A tempdir holding an empty `home` and app dir.
    pub(super) fn setup_dirs() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let app_dir = tmp.path().join("app");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&app_dir).unwrap();
        (tmp, home, app_dir)
    }

    /// Write `value` as pretty JSON, creating the parent directory.
    pub(super) fn write_json(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }
}

use crate::session::afk::schema as afk_schema;
use anyhow::Result;
use std::fs;
use tracing::{debug, info};

const CURRENT_VERSION: u32 = 52;
const VERSION_FILE: &str = ".schema_version";

/// Version, log name, and the one-time transformation to run.
type Migration = (u32, &'static str, fn() -> Result<()>);

const MIGRATIONS: &[Migration] = &[
    (1, "xdg_linux", v001_xdg_linux::run),
    (
        2,
        "seed_sandbox_from_volumes",
        v002_seed_sandbox_from_volumes::run,
    ),
    (3, "yolo_mode_config", v003_yolo_mode_config::run),
    (4, "unified_environment", v004_unified_environment::run),
    (5, "acp_defaults", v005_cockpit_defaults::run),
    (
        6,
        "unlimited_cockpit_history",
        v006_unlimited_cockpit_history::run,
    ),
    (7, "serve_log_to_legacy", v007_serve_log_to_legacy::run),
    (
        8,
        "lock_in_default_profile",
        v008_lock_in_default_profile::run,
    ),
    (9, "update_check_mode", v009_update_check_mode::run),
    (
        10,
        "drop_legacy_live_send_exit_chord",
        v010_drop_legacy_live_send_exit_chord::run,
    ),
    (
        11,
        "relocate_sandbox_image",
        v011_relocate_sandbox_image::run,
    ),
    (12, "acp_rename", v012_acp_rename::run),
    (13, "strip_profile_theme", v013_strip_profile_theme::run),
    (14, "rename_default_theme", v014_rename_default_theme::run),
    (15, "rewrite_hook_strings", v015_rewrite_hook_strings::run),
    (
        16,
        "clear_archived_tmux_gone_error",
        v016_clear_archived_tmux_gone_error::run,
    ),
    (
        17,
        "rewrite_hook_strings_for_per_user_base",
        v017_rewrite_hook_strings_for_per_user_base::run,
    ),
    (
        18,
        "strip_codex_config_toml_hooks",
        v018_strip_codex_config_toml_hooks::run,
    ),
    (
        19,
        "move_acp_defaults_to_acp",
        v019_move_acp_defaults_to_acp::run,
    ),
    (
        20,
        "move_tui_branch_suffix_to_row_tag",
        v020_move_tui_branch_suffix_to_row_tag::run,
    ),
    (
        21,
        "split_app_state_to_state_toml",
        v021_split_app_state_to_state_toml::run,
    ),
    (22, "prune_tuning_settings", v022_prune_tuning_settings::run),
    (
        23,
        "clear_structured_container_error",
        v023_clear_structured_container_error::run,
    ),
    (24, "backfill_detect_as", v024_backfill_detect_as::run),
    (
        25,
        "reenable_confirm_delete",
        v025_reenable_confirm_delete::run,
    ),
    (
        26,
        "repoint_acp_default_agent",
        v026_repoint_acp_default_agent::run,
    ),
    (
        27,
        "isolate_sandbox_stores",
        v027_isolate_sandbox_stores::run,
    ),
    (
        28,
        "clear_archived_live_status",
        v028_clear_archived_live_status::run,
    ),
    (
        29,
        "fold_pending_initial_turn",
        v029_fold_pending_initial_turn::run,
    ),
    (
        30,
        "global_only_profile_settings",
        v030_global_only_profile_settings::run,
    ),
    (
        31,
        "conversation_provenance",
        v031_conversation_provenance::run,
    ),
    (
        32,
        "bound_capture_exclusions",
        v032_bound_capture_exclusions::run,
    ),
    (
        33,
        "isolate_sandbox_content",
        v033_isolate_sandbox_content::run,
    ),
    (
        34,
        "trash_retention_minutes",
        v034_trash_retention_minutes::run,
    ),
    (35, "custom_sort_order", v035_custom_sort_order::run),
    (
        36,
        "session_creation_count",
        v036_session_creation_count::run,
    ),
    (37, "trash_lifecycle_id", v037_trash_lifecycle_id::run),
    (38, "prime_native_runtime", v038_prime_native_runtime::run),
    (39, "prime_control_path", v039_prime_control_path::run),
    (40, "worker_ownership", v040_worker_ownership::run),
    (
        41,
        "Shared repository appearances",
        v041_repo_appearances::run,
    ),
    (afk_schema::CONTROL, "afk_control", v042_afk_control::run),
    (afk_schema::RUNTIME, "afk_runtime", v043_afk_runtime::run),
    (
        afk_schema::SETTLEMENT,
        "afk_settlement",
        v044_afk_settlement::run,
    ),
    (
        afk_schema::QUESTIONS,
        "afk_questions",
        v045_afk_questions::run,
    ),
    (
        afk_schema::AUTHORITY,
        "afk_authority",
        v046_afk_authority::run,
    ),
    (afk_schema::BATCHES, "afk_batches", v047_afk_batches::run),
    (
        afk_schema::USAGE,
        "afk_usage_guards",
        v048_afk_usage_guards::run,
    ),
    (
        afk_schema::REVIEW,
        "afk_return_review",
        v049_afk_return_review::run,
    ),
    (50, "tailored_history", v050_tailored_history::run),
    (51, "research_requests", v051_research_requests::run),
    (
        52,
        "document_conversations",
        v052_document_conversations::run,
    ),
];

/// The data-schema version this build targets, i.e. the version every install
/// converges to after a successful startup (migration failures abort boot, so a
/// running install is always at this version). Surfaced in telemetry as a coarse
/// version-health signal; see `crate::telemetry`.
pub fn current_schema_version() -> u32 {
    CURRENT_VERSION
}

/// Check whether there are any pending migrations to run.
pub fn has_pending_migrations() -> bool {
    get_current_version() < CURRENT_VERSION
}

/// Move this session's sandbox store into the private layout, if it is still
/// on the shared one. Called from the container path so the copy is paid by
/// the session that needs it rather than by every pending row on any `aoe`
/// start.
///
/// `reporter` is how a caller with a screen narrates the copy: the TUI
/// forwards it to its status line from a worker thread. Callers without one
/// pass [`progress::tracing_reporter`], which leaves a trail in the log.
///
/// Unproven native content is never a launch fallback: errors leave the store
/// pending, and admission refuses it until a stopped-store transition succeeds.
pub fn migrate_sandbox_store_for_with(
    id: &str,
    reporter: Option<progress::Reporter>,
) -> Result<()> {
    if get_current_version() < 27 {
        return Ok(());
    }
    let _installed = progress::install(reporter);
    v027_isolate_sandbox_stores::migrate_instance(id)?;
    v033_isolate_sandbox_content::migrate_instance(id)
}

/// [`migrate_sandbox_store_for_with`] with the container probes injected, for
/// a test that drives the launch-time move with no container runtime.
#[cfg(test)]
pub(crate) fn migrate_sandbox_store_for_test(
    id: &str,
    reporter: Option<progress::Reporter>,
    is_running: &dyn Fn(&str) -> Result<bool>,
    reap: &dyn Fn(&str) -> Result<bool>,
) -> Result<()> {
    let _installed = progress::install(reporter);
    v027_isolate_sandbox_stores::migrate_instance_with(id, is_running, reap)
}

pub fn run_migrations() -> Result<()> {
    run_migrations_with(None)
}

/// Run all pending migrations, sending [`progress::Event`]s to `reporter` so a
/// long one (store copies, container probes) reads as work, not a hang.
///
/// A still-pending sandbox store move is *not* retried here: v027's rows move
/// when their session next needs a container, or all at once under
/// [`run_migrations_announced`] for `aoe migrate`. This path only advances the
/// schema version and reports the migrations it actually runs.
pub fn run_migrations_with(reporter: Option<progress::Reporter>) -> Result<()> {
    run_migrations_inner(reporter, false)
}

/// [`run_migrations_with`] for an explicit `aoe migrate`: a pending sandbox
/// store move also narrates what is still pending and why.
pub fn run_migrations_announced(reporter: Option<progress::Reporter>) -> Result<()> {
    run_migrations_inner(reporter, true)
}

fn run_migrations_inner(reporter: Option<progress::Reporter>, announce: bool) -> Result<()> {
    let _installed = progress::install(reporter);
    let _announced = progress::install_announced(announce);
    let _runner_lock = runner_lock::acquire(std::time::Duration::from_secs(30))?;
    let app = crate::session::get_app_dir()?;
    let current = afk_schema::read_cursor(&app)?;
    debug!("Current schema version: {}", current);

    ensure_supported_version(current, CURRENT_VERSION)?;
    afk_schema::ensure_mainline(&app, current)?;
    if current == CURRENT_VERSION {
        v027_isolate_sandbox_stores::reconcile_pending(announce)?;
        return v033_isolate_sandbox_content::reconcile_pending(announce);
    }

    let pending: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|(version, ..)| *version > current)
        .collect();
    for (index, (version, name, run)) in pending.iter().enumerate() {
        let start = std::time::Instant::now();
        info!(target: "migrations", version, name, "running migration");
        progress::report(progress::Event::Started {
            version: *version,
            name,
            position: index + 1,
            total: pending.len(),
        });
        run()?;
        set_version(*version)?;
        progress::report(progress::Event::Finished {
            version: *version,
            elapsed: start.elapsed(),
        });
        info!(
            target: "migrations",
            version,
            name,
            duration_ms = start.elapsed().as_millis() as u64,
            "migration completed"
        );
    }

    Ok(())
}

fn ensure_supported_version(current: u32, supported: u32) -> Result<()> {
    if current > supported {
        anyhow::bail!(
            "data schema version {current} is newer than this build supports ({supported}); refusing to downgrade"
        );
    }
    Ok(())
}

/// Get the schema version from the selected app directory.
fn get_current_version() -> u32 {
    crate::session::get_app_dir()
        .ok()
        .and_then(|dir| fs::read_to_string(dir.join(VERSION_FILE)).ok())
        .and_then(|content| content.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

/// Write the version to the current app directory.
fn set_version(version: u32) -> Result<()> {
    let dir = crate::session::get_app_dir()?;
    let version_file = dir.join(VERSION_FILE);
    crate::session::atomic_write(&version_file, version.to_string().as_bytes())?;
    debug!("Updated schema version to {}", version);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // CRA-1131: an app dir that is the user's own symlink (for example
    // ~/.config/agent-of-empires -> ~/.agent-of-empires) must still run
    // migrations; anchoring refused the symlinked leaf, so every command failed.
    #[test]
    #[serial_test::serial]
    fn migrations_run_when_the_app_dir_is_a_symlink() {
        let home = crate::session::test_support::isolate_app_dir();
        let app = crate::session::get_app_dir().unwrap();
        let real = home.path().join("real-app-dir");
        fs::remove_dir_all(&app).unwrap();
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &app).unwrap();
        // Start just below the AFK schema: the runner lock and AFK migrations
        // still anchor the symlinked app dir, but no historical host-hook
        // migration (agent homes, /tmp/aoe-hooks) runs from this test.
        fs::write(
            real.join(VERSION_FILE),
            (crate::session::afk::schema::CONTROL - 1).to_string(),
        )
        .unwrap();
        run_migrations().expect("migrations through a symlinked app dir");
        assert_eq!(get_current_version(), CURRENT_VERSION);
        assert!(
            real.join(".migration-runner.lock").exists(),
            "lock lives in the resolved dir"
        );
        assert!(
            fs::symlink_metadata(&app).unwrap().file_type().is_symlink(),
            "the user's symlink is kept"
        );
    }

    #[test]
    #[serial_test::serial]
    fn concurrent_new_runners_reread_schema_after_the_stable_lock() {
        use std::time::Duration;
        let _home = crate::session::test_support::isolate_app_dir();
        let app = crate::session::get_app_dir().unwrap();
        fs::write(app.join(VERSION_FILE), "35").unwrap();
        let held = runner_lock::acquire(Duration::ZERO).unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let peers: Vec<_> = (0..2)
            .map(|_| {
                let sent = sent.clone();
                std::thread::spawn(move || {
                    assert_eq!(get_current_version(), 35);
                    assert!(runner_lock::acquire(Duration::ZERO).is_err());
                    sent.send(()).unwrap();
                    run_migrations().unwrap();
                })
            })
            .collect();
        received.recv_timeout(Duration::from_secs(3)).unwrap();
        received.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(held);
        for peer in peers {
            peer.join().unwrap();
        }
        assert_eq!(get_current_version(), CURRENT_VERSION);
        assert!(crate::session::afk::authority::migration::run_at(&app).is_err());
        assert!(crate::session::afk::authority::migration_v2::run_at(&app).is_err());
        crate::session::afk::authority::migration_v3::run_at(&app).unwrap();
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(app.join("afk-authority-v3/migration/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["phase"], "complete");
        fs::remove_file(app.join(".migration-runner.lock")).unwrap();
        std::os::unix::fs::symlink("/missing", app.join(".migration-runner.lock")).unwrap();
        assert!(run_migrations().is_err());
    }

    #[test]
    #[serial_test::serial]
    fn wake_preparation_sync_failure_retries_before_schema_publication() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _home = crate::session::test_support::isolate_app_dir();
        let app = crate::session::get_app_dir().unwrap();
        crate::session::afk::authority::migration_v2::initialize_test(&app);
        crate::session::afk::authority::migration_v3::run_at(&app).unwrap();
        let root = app.join("afk-authority-v3/automatic-wake");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("writer.lock"))
            .unwrap()
            .sync_all()
            .unwrap();
        let manifest = app.join("afk-authority-v3/migration/manifest.json");
        let original = fs::read(&manifest).unwrap();
        crate::session::anchored_fs::FAIL_SYNC_ONCE.set(Some(root.clone()));
        assert!(run_migrations().is_err());
        crate::session::anchored_fs::FAIL_SYNC_ONCE.set(None);
        assert!(
            root.join("version.json").exists(),
            "version record rename preceded failed directory sync"
        );
        assert_eq!(get_current_version(), afk_schema::BATCHES);
        run_migrations().unwrap();
        assert_eq!(get_current_version(), CURRENT_VERSION);
        assert_eq!(fs::read(&manifest).unwrap(), original);
        let mut instance = crate::session::Instance::new("later authority", app.to_str().unwrap());
        instance.source_profile = "alpha".into();
        crate::session::afk::control(&instance, crate::session::afk::Operation::Off).unwrap();
        let snapshot = app.join("afk-authority-v3/profiles/alpha/snapshot.json");
        let advanced = fs::read(&snapshot).unwrap();
        crate::session::anchored_fs::FAIL_SYNC_ONCE.set(Some(root));
        assert!(v048_afk_usage_guards::run().is_err());
        crate::session::anchored_fs::FAIL_SYNC_ONCE.set(None);
        v048_afk_usage_guards::run().unwrap();
        assert_eq!(fs::read(snapshot).unwrap(), advanced);
        assert_eq!(get_current_version(), CURRENT_VERSION);
    }

    #[test]
    fn test_migrations_are_sequential() {
        let mut prev = 0;
        for (version, ..) in MIGRATIONS {
            assert!(*version > prev, "migration {version} should be > {prev}");
            prev = *version;
        }
    }

    #[test]
    #[serial_test::serial]
    fn selected_app_dir_refuses_a_newer_schema() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(temp.path());
        let app = crate::session::get_app_dir().unwrap();
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join(VERSION_FILE), (CURRENT_VERSION + 1).to_string()).unwrap();

        let error = run_migrations().unwrap_err().to_string();

        assert!(error.contains("refusing to downgrade"));
    }

    #[test]
    #[serial_test::serial]
    fn document_schema_51_upgrade_preserves_rows_and_fences_old_readers() {
        let _home = crate::session::test_support::isolate_app_dir();
        let app = crate::session::get_app_dir().unwrap();
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join(VERSION_FILE), "51").unwrap();
        let mut instance = crate::session::Instance::new("Document", "/managed/scratch/document");
        instance.tool = "pi".into();
        instance.scratch = true;
        instance.command = "ledger run pi --profile aoe-document-work --".into();
        instance.extra_args = crate::session::tailored::document::restrictive_args();
        let sid = "original-native-conversation";
        let transcript = "/protected/ledger/runtime/sessions/document.jsonl";
        instance.set_agent_conversation(
            Some(sid.into()),
            Some(crate::session::ConversationBinding {
                session_id: sid.into(),
                execution: Some(crate::session::ExecutionBinding {
                    agent: "pi".into(),
                    stores: vec!["/protected/ledger/runtime/sessions".into()],
                    configuration: vec![crate::session::ExecutionLocation {
                        filesystem: "host".into(),
                        path: "/protected/ledger/runtime".into(),
                    }],
                    cwd: instance.project_path.clone().into(),
                    cwd_filesystem: "host".into(),
                    filesystem: "host".into(),
                    exported_default_store: None,
                }),
                provenance: crate::session::ConversationProvenance::Observed,
                transcript_path: Some(transcript.into()),
            }),
            Some(transcript.into()),
        );
        let storage = crate::session::Storage::new_unwatched("document-test").unwrap();
        storage
            .update(|rows, _| {
                rows.push(instance.clone());
                Ok(())
            })
            .unwrap();
        let before = fs::read(storage.sessions_path()).unwrap();
        let row: serde_json::Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(row[0]["tool"], "pi");
        assert_eq!(row[0]["scratch"], true);
        assert_eq!(row[0]["command"], instance.command);
        assert_eq!(row[0]["extra_args"], instance.extra_args);
        assert_eq!(row[0]["agent_session_id"], "original-native-conversation");
        assert!(
            row[0].get("resume_intent").is_none(),
            "default continuation uses existing fields"
        );
        assert_eq!(row[0]["agent_session_binding"]["session_id"], sid);
        assert_eq!(
            row[0]["agent_session_binding"]["execution"]["stores"][0],
            "/protected/ledger/runtime/sessions"
        );
        assert_eq!(row[0]["pi_session_path"], transcript);
        run_migrations().unwrap();
        assert_eq!(get_current_version(), 52);
        assert_eq!(fs::read(storage.sessions_path()).unwrap(), before);
        run_migrations().unwrap();
        assert_eq!(fs::read(storage.sessions_path()).unwrap(), before);
        // Mirror the schema-51 startup fence against the newly published cursor.
        let cursor = afk_schema::read_cursor(&app).unwrap();
        assert!(ensure_supported_version(cursor, 51)
            .unwrap_err()
            .to_string()
            .contains("refusing to downgrade"));
    }

    /// Upgrading preserves the sort order, and a second run leaves state untouched.
    #[test]
    #[serial_test::serial]
    fn schema_34_advances_and_keeps_its_sort_order() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(temp.path());
        let app = crate::session::get_app_dir().unwrap();
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join(VERSION_FILE), "34").unwrap();
        let state = "sort_order = \"oldest\"\n";
        fs::write(app.join("state.toml"), state).unwrap();
        assert_eq!(
            crate::session::config::AppStateConfig::load()
                .unwrap()
                .sort_order,
            Some(crate::session::config::SortOrder::Oldest),
            "the fixture holds a non-default order the previous schema can read"
        );

        run_migrations().unwrap();
        let advanced = get_current_version();
        assert!(advanced > 34, "the version advances past 34, to {advanced}");
        assert_eq!(advanced, CURRENT_VERSION);
        assert_eq!(
            crate::session::config::AppStateConfig::load()
                .unwrap()
                .sort_order,
            Some(crate::session::config::SortOrder::Oldest)
        );
        let migrated_state = fs::read_to_string(app.join("state.toml")).unwrap();

        run_migrations().unwrap();
        assert_eq!(get_current_version(), advanced, "a second run stays put");
        assert_eq!(
            fs::read_to_string(app.join("state.toml")).unwrap(),
            migrated_state,
            "and leaves the state alone"
        );
    }

    #[test]
    #[serial_test::serial]
    fn schema_31_content_isolation_still_receives_upstream_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(temp.path());
        let app = crate::session::get_app_dir().unwrap();
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join(VERSION_FILE), "31").unwrap();
        fs::write(
            app.join("sessions.json"),
            r#"[{"agent_session_id":"old","resume_intent":{"kind":"Use","value":"target"},"retroactive_capture_excludes":["old"]}]"#,
        )
        .unwrap();

        run_migrations().unwrap();

        let rows: serde_json::Value =
            serde_json::from_slice(&fs::read(app.join("sessions.json")).unwrap()).unwrap();
        assert_eq!(rows[0]["agent_session_id"], "old");
        assert_eq!(rows[0]["agent_session_binding"]["provenance"], "unknown");
        assert!(rows[0]["agent_session_binding"]["execution"].is_null());
        assert_eq!(rows[0]["resume_binding"]["session_id"], "target");
        assert_eq!(
            rows[0]["retroactive_capture_excludes"][0]["session_id"],
            "old"
        );
        assert_eq!(get_current_version(), CURRENT_VERSION);
    }

    #[test]
    #[serial_test::serial]
    fn oldest_migration_backup_of_an_upgrade_stays_readable_by_the_previous_release() {
        // v1.16.1 typed both of these as plain strings.
        #[derive(serde::Deserialize)]
        struct Pre116 {
            #[serde(default)]
            retroactive_capture_excludes: std::collections::HashSet<String>,
            pending_initial_turn: Option<String>,
        }

        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(temp.path());
        let app = crate::session::get_app_dir().unwrap();
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join(VERSION_FILE), "28").unwrap();
        fs::write(
            app.join("sessions.json"),
            r#"[{"retroactive_capture_excludes":["legacy-sid"],"pending_initial_turn":"go"}]"#,
        )
        .unwrap();

        run_migrations().unwrap();

        let backups = crate::session::migration_backups(&app.join("sessions.json")).unwrap();
        assert!(
            !backups.is_empty(),
            "an upgrade that retypes a field must leave a migration backup"
        );

        let before: Vec<Pre116> =
            serde_json::from_slice(&fs::read(&backups[0].1).unwrap()).unwrap();
        assert!(before[0]
            .retroactive_capture_excludes
            .contains("legacy-sid"));
        assert_eq!(before[0].pending_initial_turn.as_deref(), Some("go"));
    }
}
