use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};

fn sessions(app: &Path, profile: &str, rows: &[Instance]) {
    let dir = app.join("profiles").join(profile);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("sessions.json"), serde_json::to_vec(rows).unwrap()).unwrap();
}

pub(in crate::session::afk) fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.insert(path.clone(), Vec::new());
            files.extend(tree(&path));
        } else {
            files.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    files
}

#[test]
fn all_profiles_and_ineligible_sessions_remain_visible_without_writes() {
    let app = tempfile::tempdir().unwrap();
    let mut stopped = Instance::new("stopped", "/workspace");
    stopped.status = crate::session::Status::Stopped;
    let mut archived = Instance::new("archived", "/workspace");
    archived.archive();
    sessions(app.path(), "alpha", &[stopped]);
    sessions(app.path(), "beta", &[archived]);
    let before = tree(app.path());
    let overview = snapshot_at(app.path(), 1000).unwrap();
    assert!(!overview.complete());
    assert_eq!(overview.profiles.len(), 2);
    assert_eq!(overview.profiles[0].sessions[0].activity, "stopped");
    assert!(overview.profiles[1].sessions[0].archived);
    assert!(overview
        .profiles
        .iter()
        .all(|p| p.sessions[0].support != "eligible"));
    assert!(overview.shared_budget.contains("unobserved"));
    assert!(overview.account_coverage.contains("unknown"));
    assert_eq!(before, tree(app.path()));
    let empty = app.path().join("absent");
    assert!(snapshot_at(&empty, 1000).unwrap().profiles.is_empty());
    assert!(!empty.exists());
}

#[test]
fn corrupt_rows_and_unreadable_profiles_are_errors_not_an_empty_fleet() {
    let app = tempfile::tempdir().unwrap();
    sessions(
        app.path(),
        "good",
        &[Instance::new("visible", "/workspace")],
    );
    sessions(app.path(), "broken", &[]);
    fs::write(app.path().join("profiles/broken/sessions.json"), b"[{}]").unwrap();
    sessions(app.path(), "unsafe", &[]);
    fs::remove_file(app.path().join("profiles/unsafe/sessions.json")).unwrap();
    symlink("/missing", app.path().join("profiles/unsafe/sessions.json")).unwrap();
    let overview = snapshot_at(app.path(), 1000).unwrap();
    assert!(!overview.complete());
    assert_eq!(overview.profiles.len(), 3);
    for name in ["broken", "unsafe"] {
        let profile = overview
            .profiles
            .iter()
            .find(|p| p.profile == name)
            .unwrap();
        assert!(!profile.errors.is_empty(), "{name}");
    }
    assert_eq!(
        overview
            .profiles
            .iter()
            .map(|p| p.sessions.len())
            .sum::<usize>(),
        1
    );
    assert!(!app
        .path()
        .join("profiles/broken/sessions.corrupt.jsonl")
        .exists());
}

#[test]
fn oversized_and_unsafe_host_snapshots_are_reported_without_payloads() {
    let app = tempfile::tempdir().unwrap();
    let instance = Instance::new("visible", "/workspace");
    sessions(app.path(), "alpha", std::slice::from_ref(&instance));
    authority::migration::initialize_test(app.path());
    let store = Store::authority(app.path(), "alpha", &instance.id, true).unwrap();
    store
        .corrupt_authority_for_test(
            "policy.json",
            &serde_json::json!({"secret":"PRIVATE-PAYLOAD"}),
        )
        .unwrap();
    let overview = snapshot_at(app.path(), 1000).unwrap();
    assert!(!overview.complete());
    let text = serde_json::to_string(&overview).unwrap();
    assert!(!text.contains("PRIVATE-PAYLOAD"));
    assert!(text.contains("snapshot.json"));
    fs::set_permissions(
        app.path()
            .join("afk-authority-v3/profiles/alpha/snapshot.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(!snapshot_at(app.path(), 1000).unwrap().complete());
    let file = fs::File::create(app.path().join("profiles/alpha/sessions.json")).unwrap();
    file.set_len((PROFILE_BYTES + 1) as u64).unwrap();
    let overview = snapshot_at(app.path(), 1000).unwrap();
    assert!(overview.profiles[0]
        .errors
        .iter()
        .any(|e| e.contains("oversized")));
}

#[test]
fn stored_acknowledgements_guards_and_reservations_are_redacted_and_never_fresh() {
    use serde_json::json;
    let app = tempfile::tempdir().unwrap();
    let mut instance = Instance::new("workstream", "/workspace");
    instance.tool = "pi".into();
    instance.source_profile = "alpha".into();
    instance.agent_session_id = Some("native".into());
    instance.active_execution = Some(serde_json::from_value(json!({
        "launch_id":"launch", "binding":{"agent":"pi", "stores":[], "cwd":"/workspace", "cwd_filesystem":"host", "filesystem":"host"}
    })).unwrap());
    let binding = Binding::for_instance(&instance).unwrap();
    let generation = uuid::Uuid::new_v4().to_string();
    let window = uuid::Uuid::new_v4().to_string();
    let policy = json!({"version":1, "binding":binding, "revision":1, "window":window, "generation":generation, "enabled":true, "issued_at_ms":1000, "duration_ms":60000, "expires_at_ms":61000, "mode":"control-only", "allowance":0});
    sessions(app.path(), "alpha", std::slice::from_ref(&instance));
    authority::migration::initialize_test(app.path());
    let runtime = Store::authority(app.path(), "alpha", &instance.id, true).unwrap();
    let control = Store::transport(app.path(), "alpha", &instance.id, true).unwrap();
    runtime.write("policy.json", &policy).unwrap();
    control
        .write(
            "request.json",
            &json!({"version":1, "binding":binding, "challenge":"challenge", "revision":1}),
        )
        .unwrap();
    control.write("ack.json", &json!({"version":1,"binding":binding,"challenge":"challenge","generation":generation,"revision":1,"window":window,"expires_at_ms":61000,"mode":"control-only","allowance":0,"state":"control-only"})).unwrap();
    control
        .write(
            "runtime-ack.json",
            &json!({"version":BRIDGE_VERSION,"binding":binding,"generation":generation,"window":window}),
        )
        .unwrap();

    let ledger = json!({"version":4,"windows":[{
        "version":4,"binding":binding,"generation":generation,"id":window,"state":"owned","confirmed":true,"expires_at_ms":61000,
        "reservations":[uuid::Uuid::new_v4().to_string()],"grant":{"version":4,"requests":2,"task":"PRIVATE-GRANT"},"nudges":[],
        "question":{"outcome":"unknown","observed_request":null,"recovered_at_ms":null,"identity":"PRIVATE-GUARD"},
        "records":[{"disposition":"human_required","claims":"PRIVATE-DECISION"}],"permit":{"outcomes":["unknown"],"content":"PRIVATE-PAYLOAD","preimage":"PRIVATE-PREIMAGE"}
    }]});
    runtime
        .corrupt_authority_for_test("ledger.json", &ledger)
        .unwrap();
    let before = tree(app.path());
    let overview = snapshot_at(app.path(), 2000).unwrap();
    assert!(overview.complete());
    let row = &overview.profiles[0].sessions[0];
    assert!(row.presence_acknowledgement.contains("freshness unknown"));
    let d = row.delegation.as_ref().unwrap();
    assert_eq!(
        (d.state.as_str(), d.reservations_used, d.requests),
        ("owned", 1, 2)
    );
    assert_eq!(
        (d.unresolved_question_guards, d.human_required_records),
        (1, 1)
    );
    assert_eq!(d.action_outcomes, ["unknown"]);
    assert_eq!(d.questions_requiring_review, 1);
    assert_eq!(d.latest_question_outcome.as_deref(), Some("unknown"));
    assert!(d.runtime_acknowledgement.contains("unverified"));
    assert!(!serde_json::to_string(&overview)
        .unwrap()
        .contains("PRIVATE-"));
    assert_eq!(before, tree(app.path()));
    assert_eq!(
        snapshot_at(app.path(), 61000).unwrap().profiles[0].sessions[0]
            .delegation
            .as_ref()
            .unwrap()
            .state,
        "expired"
    );
    for resolution in ["observed", "recovered", "ordinary"] {
        let mut resolved = ledger.clone();
        let request = resolved["windows"][0]["reservations"][0].clone();
        let question = &mut resolved["windows"][0]["question"];
        match resolution {
            "observed" => {
                question["outcome"] = json!("accepted");
                question["observed_request"] = request;
            }
            "recovered" => question["recovered_at_ms"] = json!(3000),
            _ => question["outcome"] = json!("ordinary"),
        }
        runtime
            .corrupt_authority_for_test("ledger.json", &resolved)
            .unwrap();
        let overview = snapshot_at(app.path(), 4000).unwrap();
        let d = overview.profiles[0].sessions[0]
            .delegation
            .as_ref()
            .unwrap();
        assert_eq!(d.unresolved_question_guards, 0, "{resolution}");
        assert_eq!(d.questions_requiring_review, 1, "{resolution}");
        assert!(d
            .question_answer_observation
            .contains("does not resolve human review"));
    }
    instance.agent_session_id = Some("replacement".into());
    sessions(app.path(), "alpha", std::slice::from_ref(&instance));
    let overview = snapshot_at(app.path(), 2000).unwrap();
    let row = &overview.profiles[0].sessions[0];
    assert!(row.presence_acknowledgement.contains("stale"));
    assert_eq!(row.delegation.as_ref().unwrap().state, "invalidated");
    sessions(app.path(), "beta", &[instance]);
    let overview = snapshot_at(app.path(), 2000).unwrap();
    let foreign = &overview.profiles[1].sessions[0];
    assert!(!foreign.errors.is_empty());
    assert!(foreign.delegation.is_none());
    assert!(!foreign.requested_presence.contains("on (control-only)"));
    let mut remaining = 1;
    assert!(runtime
        .read_budgeted::<SummaryProfile>("snapshot.json", &mut remaining)
        .is_err());
}

#[test]
fn capacity_limits_refuse_silent_truncation() {
    let app = tempfile::tempdir().unwrap();
    sessions(app.path(), "rows", &[]);
    fs::write(
        app.path().join("profiles/rows/sessions.json"),
        serde_json::to_vec(&vec![serde_json::json!({}); MAX_SESSIONS + 1]).unwrap(),
    )
    .unwrap();
    let overview = snapshot_at(app.path(), 0).unwrap();
    assert!(!overview.complete());
    assert!(overview.profiles[0]
        .errors
        .iter()
        .any(|e| e.contains("capacity")));
    for i in 0..MAX_PROFILES {
        fs::create_dir(app.path().join("profiles").join(format!("p-{i}"))).unwrap();
    }
    assert!(snapshot_at(app.path(), 0)
        .unwrap_err()
        .to_string()
        .contains("capacity"));

    let non_profiles = tempfile::tempdir().unwrap();
    let directory = non_profiles.path().join("profiles");
    fs::create_dir(&directory).unwrap();
    for i in 0..=MAX_PROFILES * 2 {
        fs::write(directory.join(format!("not-a-profile-{i}")), b"").unwrap();
    }
    assert!(snapshot_at(non_profiles.path(), 0)
        .unwrap_err()
        .to_string()
        .contains("directory entry capacity"));
}

#[test]
fn older_partial_and_missing_authority_keep_rows_and_leave_files_unchanged() {
    let app = tempfile::tempdir().unwrap();
    sessions(
        app.path(),
        "alpha",
        &[Instance::new("visible", "/workspace")],
    );
    for condition in ["older", "partial", "missing"] {
        authority::migration::initialize_test(app.path());
        let manifest = app.path().join("afk-authority-v2/migration/manifest.json");
        let original = fs::read(&manifest).unwrap();
        if condition == "older" {
            fs::write(app.path().join(".schema_version"), "39").unwrap();
        } else if condition == "partial" {
            let mut value: Value = serde_json::from_slice(&original).unwrap();
            value["phase"] = serde_json::json!("imports");
            fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let before = tree(app.path());
        let overview = snapshot_at(app.path(), 1000).unwrap();
        assert!(!overview.complete());
        assert_eq!(overview.profiles[0].sessions.len(), 1);
        assert_eq!(
            overview.profiles[0].sessions[0].requested_presence,
            "unknown; storage error"
        );
        assert!(!overview.profiles[0].errors.is_empty());
        assert_eq!(tree(app.path()), before, "{condition}");
        fs::write(&manifest, original).unwrap();
        fs::write(app.path().join(".schema_version"), "41").unwrap();
    }
}
