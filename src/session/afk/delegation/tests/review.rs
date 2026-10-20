use super::*;
use crate::session::afk::review::{self as service, annotations, Format, Selection};
use std::collections::BTreeMap;
use std::fs;

fn initialize(app: &Path) {
    annotations::initialize(app).unwrap();
    fs::write(
        app.join(".schema_version"),
        crate::session::afk::schema::REVIEW.to_string(),
    )
    .unwrap();
}
fn selected(w: &Window) -> Selection {
    Selection {
        profile: w.binding.profile.clone(),
        session_id: w.binding.instance_id.clone(),
        window_id: w.id.clone(),
    }
}
fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.insert(path.clone(), vec![]);
            result.extend(tree(&path));
        } else {
            result.insert(path.clone(), fs::read(&path).unwrap());
        }
    }
    result
}
fn claims(w: &mut Window, human: bool) -> String {
    let mut value = decision();
    value["human_required"] = json!(human);
    value["assumptions"] = json!(["Operator supplied scope, not a new rationale"]);
    value["question"] = json!("<script> [link](https://example.invalid) `code`");
    let claims: Decision = serde_json::from_value(value).unwrap();
    let id = claims.id.clone();
    w.state = "owned".into();
    if w.reservations.is_empty() {
        w.reservations.push(uuid::Uuid::new_v4().to_string());
    }
    w.records.push(Record {
        id: id.clone(),
        payload_hash: hash(serde_json::to_vec(&claims).unwrap()),
        claims: Some(claims),
        request: 1,
        recorded_at_ms: 2000,
        disposition: if human { "human_required" } else { "deferred" }.into(),
    });
    id
}

#[test]
fn work_arc_schema_keeps_afk_authority_and_review_usable_without_reimport() {
    let (app, _root, store, w) = setup(2);
    initialize(app.path());
    Book {
        version: 4,
        windows: vec![w.clone()],
    }
    .save(&store)
    .unwrap();
    let original = tree(&app.path().join(authority::NAMESPACE));
    fs::write(
        app.path().join(".schema_version"),
        crate::migrations::current_schema_version().to_string(),
    )
    .unwrap();

    authority::ready(app.path()).unwrap();
    authority::migration_v3::run_at(app.path()).unwrap();
    annotations::initialize(app.path()).unwrap();
    let selection = selected(&w);
    let report = service::collect_at(app.path(), std::slice::from_ref(&selection), 2000).unwrap();
    assert!(report.complete, "{:?}", report.selections);
    let request = window_note(&w);
    let id = request.id.clone();
    annotations::append_at(app.path(), request, 2000).unwrap();
    let report = service::collect_at(app.path(), &[selection], 2000).unwrap();
    assert!(report.complete, "{:?}", report.selections);
    assert_eq!(report.selections[0].annotations.len(), 1);
    assert_eq!(report.selections[0].annotations[0].id, id);
    assert_eq!(tree(&app.path().join(authority::NAMESPACE)), original);

    let index = app.path().join("afk-review-v1/annotations/index.json");
    let saved = fs::read(&index).unwrap();
    fs::remove_file(&index).unwrap();
    let before = tree(app.path());
    assert!(annotations::initialize(app.path()).is_err());
    assert_eq!(
        tree(app.path()),
        before,
        "missing review data is not recreated"
    );
    fs::write(index, saved).unwrap();
    fs::write(
        app.path().join(".schema_version"),
        (crate::migrations::current_schema_version() + 1).to_string(),
    )
    .unwrap();
    let before = tree(app.path());
    assert!(authority::ready(app.path()).is_err());
    assert!(authority::migration_v3::run_at(app.path()).is_err());
    assert!(annotations::initialize(app.path()).is_err());
    assert!(annotations::append_at(app.path(), window_note(&w), 2000).is_err());
    assert!(
        !service::collect_at(app.path(), &[selected(&w)], 2000)
            .unwrap()
            .complete
    );
    assert_eq!(tree(app.path()), before);
}

#[test]
fn review_physical_profile_and_cumulative_read_limits_preserve_all_input() {
    let (app, _root, store, w) = setup(2);
    initialize(app.path());
    Book {
        version: 4,
        windows: vec![w.clone()],
    }
    .save(&store)
    .unwrap();
    let mut other = w.clone();
    other.binding.profile = "zz-review-budget".into();
    other.id = uuid::Uuid::new_v4().to_string();
    let second = Store::authority(app.path(), &other.binding.profile, "test", true).unwrap();
    Book {
        version: 4,
        windows: vec![other.clone()],
    }
    .save(&second)
    .unwrap();
    let path = |profile: &str| {
        app.path()
            .join(authority::NAMESPACE)
            .join("profiles")
            .join(profile)
            .join("snapshot.json")
    };
    let first_path = path(&w.binding.profile);
    let second_path = path(&other.binding.profile);
    let mut padded = fs::read(&first_path).unwrap();
    padded.resize(32 * 1024 * 1024, b' ');
    fs::write(&first_path, &padded).unwrap();
    let first = service::collect_at(app.path(), &[selected(&w)], 2000).unwrap();
    assert!(first.complete, "{:?}", first.selections);
    assert_eq!(fs::read(&first_path).unwrap(), padded);
    padded.push(b' ');
    fs::write(&first_path, &padded).unwrap();
    let oversized = service::collect_at(app.path(), &[selected(&w)], 2000).unwrap();
    assert!(!oversized.complete);
    assert!(oversized.selections[0].window.is_none());
    assert!(oversized.selections[0]
        .errors
        .iter()
        .any(|e| e.contains("capacity")));
    assert_eq!(fs::read(&first_path).unwrap(), padded);
    padded.pop();
    fs::write(&first_path, &padded).unwrap();
    let mut second_bytes = fs::read(&second_path).unwrap();
    second_bytes.resize(32 * 1024 * 1024, b' ');
    fs::write(&second_path, &second_bytes).unwrap();
    assert!(
        service::collect_at(app.path(), &[selected(&other)], 2000)
            .unwrap()
            .complete
    );
    let before = tree(app.path());
    let combined =
        service::collect_at(app.path(), &[selected(&w), selected(&other)], 2000).unwrap();
    assert!(!combined.complete);
    assert!(
        combined.selections[0].window.is_some(),
        "{:?}",
        combined.selections
    );
    assert!(combined.selections[1].window.is_none());
    assert!(combined.selections[1]
        .errors
        .iter()
        .any(|e| e.contains("capacity")));
    assert_eq!(tree(app.path()), before);
}

#[test]
fn stored_review_preserves_weighted_facts_scopes_retention_and_unknown_effects() {
    let (app, _root, store, mut w) = setup(4);
    initialize(app.path());
    claims(&mut w, false);
    claims(&mut w, true);
    let mut unknown = w.clone();
    unknown.id = uuid::Uuid::new_v4().to_string();
    unknown.revision = 2;
    unknown.records.truncate(1);
    unknown.records[0].disposition = "admitted".into();
    let decision = unknown.records[0].claims.as_mut().unwrap();
    let action = decision.action.as_mut().unwrap();
    action.content = "PAYLOAD_ONLY".into();
    action.expected_hash = Some(hash("ORIGINAL_ONLY"));
    unknown.permit = Some(Permit {
        decision: decision.id.clone(),
        path: action.path.clone(),
        expected_hash: action.expected_hash.clone(),
        payload_hash: hash(&action.content),
        content: Some(action.content.clone()),
        preimage: Some("ORIGINAL_ONLY".into()),
        record_request: 1,
        attempted_at_ms: Some(3000),
        outcomes: vec!["unknown".into()],
    });
    let payload_hash = hash(serde_json::to_vec(&decision).unwrap());
    unknown.records[0].payload_hash = payload_hash;
    unknown.end(TerminalReason::Failed);
    Book {
        version: 4,
        windows: vec![w.clone(), unknown.clone()],
    }
    .save(&store)
    .unwrap();
    let beta = Store::authority(app.path(), "beta", "test", true).unwrap();
    let mut other = w.clone();
    other.binding.profile = "beta".into();
    other.id = uuid::Uuid::new_v4().to_string();
    Book {
        version: 4,
        windows: vec![other.clone()],
    }
    .save(&beta)
    .unwrap();
    let selections = vec![selected(&w), selected(&unknown), selected(&other)];
    annotations::append_at(
        app.path(),
        annotations::Request {
            id: uuid::Uuid::new_v4().to_string(),
            target: annotations::Target {
                selection: selected(&unknown),
                decision: None,
                original_hash: hash(serde_json::to_vec(&unknown.grant).unwrap()),
            },
            kind: annotations::Kind::Reviewed,
            body: "Unknown effect still needs attention".into(),
            prior: None,
        },
        2000,
    )
    .unwrap();
    let before = tree(app.path());
    let boundary = w.expires_at_ms + RETENTION_MS;
    let report = service::collect_at(app.path(), &selections, boundary).unwrap();
    assert!(report.complete, "{:?}", report.selections);
    assert_eq!(report.selections.len(), 3);
    let first = report.selections[0].window.as_ref().unwrap();
    assert_eq!(first["state"], "owned");
    assert_eq!(
        first["records"][0]["claims"]["criteria"],
        json!([{"name":"clarity","weight":4}])
    );
    assert_eq!(
        first["records"][0]["claims"]["alternatives"][0]["scores"],
        json!([5])
    );
    assert_eq!(first["records"][0]["claims"]["recommendation"], "hello");
    assert_eq!(
        first["records"][0]["claims"]["assumptions"],
        json!(["Operator supplied scope, not a new rationale"])
    );
    assert_eq!(first["records"][1]["disposition"], "human_required");
    let unknown_view = report.selections[1].window.as_ref().unwrap();
    assert_eq!(unknown_view["terminal_reason"], "failed");
    assert_eq!(unknown_view["action"]["outcomes"], json!(["unknown"]));
    let expired = service::collect_at(app.path(), &selections, boundary + 1).unwrap();
    assert!(expired.selections[0].window.as_ref().unwrap()["records"][0]["claims"].is_null());
    assert_eq!(
        expired.selections[0].window.as_ref().unwrap()["state"],
        "owned"
    );
    assert!(expired.selections[0].window.as_ref().unwrap()["terminal_reason"].is_null());
    assert!(expired.selections[1].window.as_ref().unwrap()["records"][0]["claims"].is_object());
    assert!(expired.selections[1].annotations[0].body.is_some());
    for format in [Format::Json, Format::Markdown] {
        let text = String::from_utf8(service::render(&report, format).unwrap()).unwrap();
        for forbidden in [
            "PAYLOAD_ONLY",
            "ORIGINAL_ONLY",
            "scratch.txt",
            w.root.to_str().unwrap(),
            "native_id",
            "launch_id",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        if matches!(format, Format::Markdown) {
            assert!(text.contains("&lt;script&gt;"));
            assert!(!text.contains("[link](https://example.invalid)"));
        }
    }
    assert_eq!(before, tree(app.path()));
}

#[test]
fn annotations_append_retry_correct_expire_and_never_touch_authority() {
    let (app, _root, store, mut w) = setup(2);
    let id = claims(&mut w, false);
    Book {
        version: 4,
        windows: vec![w.clone()],
    }
    .save(&store)
    .unwrap();
    let original = tree(&app.path().join(authority::NAMESPACE));
    initialize(app.path());
    annotations::initialize(app.path()).unwrap();
    assert_eq!(original, tree(&app.path().join(authority::NAMESPACE)));
    let request = annotations::Request {
        id: uuid::Uuid::new_v4().to_string(),
        target: annotations::Target {
            selection: selected(&w),
            decision: Some(id),
            original_hash: w.records[0].payload_hash.clone(),
        },
        kind: annotations::Kind::Note,
        body: "Operator review, not approval".into(),
        prior: None,
    };
    let created = w.expires_at_ms + 100;
    let first = annotations::append_at(app.path(), request.clone(), created).unwrap();
    let after_first = tree(app.path());
    let retry = annotations::append_at(app.path(), request.clone(), created + 1).unwrap();
    assert_eq!(retry.created_at_ms, first.created_at_ms);
    assert_eq!(tree(app.path()), after_first);
    for changed in ["body", "hash", "unknown", "credential", "control", "size"] {
        let mut invalid = request.clone();
        match changed {
            "body" => invalid.body = "Different claim".into(),
            "hash" => invalid.target.original_hash = "a".repeat(64),
            "unknown" => invalid.target.decision = Some(uuid::Uuid::new_v4().to_string()),
            "credential" => invalid.body = "password=hidden".into(),
            "control" => invalid.body = "\u{1b}[2J".into(),
            _ => invalid.body = "x".repeat(2049),
        }
        assert!(
            annotations::append_at(app.path(), invalid, created).is_err(),
            "{changed}"
        );
    }
    let mut correction = request.clone();
    correction.id = uuid::Uuid::new_v4().to_string();
    correction.kind = annotations::Kind::Correction;
    correction.prior = Some(request.id.clone());
    correction.body = "A separate correction".into();
    annotations::append_at(app.path(), correction, created).unwrap();
    let report = service::collect_at(app.path(), &[selected(&w)], created + RETENTION_MS).unwrap();
    assert_eq!(report.selections[0].annotations.len(), 2);
    assert!(report.selections[0].annotations[0].body.is_some());
    let report =
        service::collect_at(app.path(), &[selected(&w)], created + RETENTION_MS + 1).unwrap();
    assert!(report.selections[0]
        .annotations
        .iter()
        .all(|a| a.body.is_none()));
    let retry =
        annotations::append_at(app.path(), request.clone(), created + RETENTION_MS + 1).unwrap();
    assert!(retry.body.is_none());
    assert_eq!(retry.created_at_ms, created);
    for _ in 2..16 {
        let mut extra = request.clone();
        extra.id = uuid::Uuid::new_v4().to_string();
        annotations::append_at(app.path(), extra, created + RETENTION_MS + 2).unwrap();
    }
    let mut extra = request.clone();
    extra.id = uuid::Uuid::new_v4().to_string();
    assert!(annotations::append_at(app.path(), extra, created + RETENTION_MS + 2).is_err());
    assert_eq!(original, tree(&app.path().join(authority::NAMESPACE)));
    fs::remove_file(
        app.path()
            .join("afk-review-v1/annotations/profile-default.json"),
    )
    .unwrap();
    assert!(annotations::append_at(app.path(), request, created).is_err());
    assert!(
        !service::collect_at(app.path(), &[selected(&w)], created)
            .unwrap()
            .complete
    );
}

#[test]
fn review_errors_and_exports_are_bounded_private_and_non_mutating() {
    let (app, _root, store, w) = setup(2);
    let selection = selected(&w);
    let before = tree(app.path());
    let unready = service::collect_at(app.path(), std::slice::from_ref(&selection), 2000).unwrap();
    assert!(!unready.complete);
    assert_eq!(before, tree(app.path()));
    initialize(app.path());
    let mut missing = selection.clone();
    missing.window_id = uuid::Uuid::new_v4().to_string();
    let report = service::collect_at(app.path(), &[selection.clone(), missing], 2000).unwrap();
    assert!(!report.complete);
    assert!(report.selections[0].errors.is_empty());
    assert!(!report.selections[1].errors.is_empty());
    assert!(service::collect_at(app.path(), &vec![selection.clone(); 17], 2000).is_err());
    let metadata =
        authority::migration_v3::overview_metadata(app.path(), &mut (64 * 1024 * 1024)).unwrap();
    assert!(service::read_authority(app.path(), "default", &metadata, &mut 0).is_err());
    let output = tempfile::tempdir().unwrap();
    fs::set_permissions(output.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = output.path().join("review.md");
    service::export_at(app.path(), &report, Format::Markdown, &path).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let bytes = fs::read(&path).unwrap();
    assert!(service::export_at(app.path(), &report, Format::Json, &path).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::set_permissions(output.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(service::export_at(
        app.path(),
        &report,
        Format::Json,
        &output.path().join("unsafe.json")
    )
    .is_err());
    let mut large = report.clone();
    large.selections[0].window = Some(json!({"text":"x".repeat(service::EXPORT_BYTES)}));
    for format in [Format::Json, Format::Markdown] {
        assert!(service::render(&large, format).is_err());
    }
    store
        .write_raw("publication.pending.json", &json!({"pending":true}))
        .unwrap();
    let before = tree(app.path());
    let vetoed = service::collect_at(app.path(), std::slice::from_ref(&selection), 2000).unwrap();
    assert!(!vetoed.complete);
    assert!(vetoed.selections[0].window.is_none());
    assert_eq!(before, tree(app.path()));
    fs::write(
        app.path().join(".schema_version"),
        (crate::migrations::current_schema_version() + 1).to_string(),
    )
    .unwrap();
    let before = tree(app.path());
    assert!(
        !service::collect_at(app.path(), &[selection], 2000)
            .unwrap()
            .complete
    );
    assert!(authority::ready(app.path()).is_err());
    assert!(authority::migration_v3::run_at(app.path()).is_err());
    assert_eq!(before, tree(app.path()));
}

#[test]
fn shared_review_reports_stored_denial_once_and_keeps_pending_evidence_unknown() {
    use crate::session::afk::usage as accounting;
    let (app, _root, store, w) = setup(2);
    initialize(app.path());
    let mut other = w.clone();
    other.binding.instance_id = "second".into();
    other.id = uuid::Uuid::new_v4().to_string();
    let second = Store::authority(app.path(), "default", "second", true).unwrap();
    Book {
        version: 4,
        windows: vec![other.clone()],
    }
    .save(&second)
    .unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let mut snapshot = store.authority_snapshot().unwrap();
    let mut members = BTreeMap::new();
    for window in [&w, &other] {
        members.insert(
            window.binding.instance_id.clone(),
            batch::state::Member {
                binding: Some(window.binding.clone()),
                generation: Some(window.generation.clone()),
                window: Some(window.id.clone()),
                grant_hash: hash(serde_json::to_vec(&window.grant).unwrap()),
                control_epoch: 0,
                exception_epoch: 0,
                outcome: batch::state::Outcome::Confirmed,
                reason: None,
            },
        );
    }
    snapshot.batches.insert(
        operation.clone(),
        batch::state::Operation {
            id: operation.clone(),
            preview_digest: "a".repeat(64),
            issued_at_ms: w.issued_at_ms,
            expires_at_ms: w.expires_at_ms,
            shared_requests: Some(4),
            stopped: false,
            members,
            tickets: vec![],
        },
    );
    for window in [&w, &other] {
        snapshot.usage.windows.insert(
            window.id.clone(),
            accounting::Window {
                binding: window.binding.clone(),
                generation: window.generation.clone(),
                source: None,
                baseline: None,
                cutoff: None,
                observed_through: None,
                closing: false,
                gap: None,
                policy: None,
                blocked: None,
                ordinary_recovered: false,
            },
        );
    }
    snapshot.usage.batches.insert(
        operation,
        accounting::SharedGuard {
            policy: accounting::Policy {
                version: 1,
                basis: "observed-native-entries".into(),
                tokens: Some(10),
                usd: None,
            },
            blocked: Some(accounting::Denial::Exhausted),
        },
    );
    snapshot.revision += 1;
    snapshot.validate("default").unwrap();
    store.write_raw("snapshot.json", &snapshot).unwrap();
    let before = tree(app.path());
    for selections in [vec![selected(&w)], vec![selected(&w), selected(&other)]] {
        let report = service::collect_at(app.path(), &selections, 2000).unwrap();
        assert!(report.complete, "{:?}", report.selections);
        assert_eq!(report.shared_usage.len(), 1);
        assert_eq!(report.shared_usage[0]["usage"]["blocked"], "exhausted");
        assert!(report
            .selections
            .iter()
            .all(|row| row.window.as_ref().unwrap()["usage"]["blocked"].is_null()));
        for format in [Format::Json, Format::Markdown] {
            let text = String::from_utf8(service::render(&report, format).unwrap()).unwrap();
            assert_eq!(text.matches("exhausted").count(), 1);
        }
    }
    assert_eq!(before, tree(app.path()));
    let transport = Store::transport(app.path(), "default", "second", true).unwrap();
    transport
        .write_raw(
            crate::session::afk::usage::transport::UNRESOLVED,
            &json!({"pending":true}),
        )
        .unwrap();
    let before = tree(app.path());
    let report = service::collect_at(app.path(), &[selected(&w), selected(&other)], 2000).unwrap();
    assert!(report.complete, "{:?}", report.selections);
    assert_eq!(report.shared_usage.len(), 1);
    assert_eq!(report.shared_usage[0]["shared_charges"], 0);
    assert_eq!(report.shared_usage[0]["usage"]["blocked"], "exhausted");
    assert_eq!(
        report.shared_usage[0]["usage"]["coverage"],
        "partial_or_unavailable"
    );
    assert_eq!(
        report.shared_usage[0]["usage"]["negative_evidence_pending"],
        true
    );
    assert_eq!(
        report.selections[1].window.as_ref().unwrap()["usage"]["negative_evidence_pending"],
        true
    );
    assert_eq!(before, tree(app.path()));
}

fn window_note(w: &Window) -> annotations::Request {
    annotations::Request {
        id: uuid::Uuid::new_v4().to_string(),
        target: annotations::Target {
            selection: selected(w),
            decision: None,
            original_hash: hash(serde_json::to_vec(&w.grant).unwrap()),
        },
        kind: annotations::Kind::Note,
        body: "Retain this operator claim".into(),
        prior: None,
    }
}

#[test]
fn review_marks_annotations_newer_than_captured_authority_incomplete_without_reread() {
    let (app, _root, store, w) = setup(2);
    initialize(app.path());
    annotations::append_at(app.path(), window_note(&w), 2000).unwrap();
    let revision = store.authority_snapshot().unwrap().revision;
    let written = std::rc::Rc::new(std::cell::RefCell::new(None));
    let after_write = written.clone();
    let app_path = app.path().to_owned();
    let mut updated = w.clone();
    service::AFTER_AUTHORITY_READ.set(Some(Box::new(move || {
        let decision = claims(&mut updated, false);
        Book {
            version: 4,
            windows: vec![updated.clone()],
        }
        .save(&store)
        .unwrap();
        let mut request = window_note(&updated);
        request.target.decision = Some(decision);
        request.target.original_hash = updated.records[0].payload_hash.clone();
        annotations::append_at(&app_path, request, 2000).unwrap();
        *after_write.borrow_mut() = Some(tree(&app_path));
    })));
    let report = service::collect_at(app.path(), &[selected(&w)], 2000).unwrap();
    assert!(
        written.borrow().is_some(),
        "the publication hook must have run"
    );
    assert_eq!(written.borrow().as_ref().unwrap(), &tree(app.path()));
    let row = &report.selections[0];
    assert_eq!(row.source_revision, Some(revision));
    assert!(row.window.as_ref().unwrap()["records"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(row.annotations.len(), 2);
    assert!(row.annotations.iter().all(|entry| entry.body.is_some()));
    assert!(
        !report.complete,
        "a newer annotation is not a fact in the captured revision"
    );
    assert!(row
        .errors
        .iter()
        .any(|error| error.contains(&row.annotations[1].id)));
}

#[test]
fn review_reports_missing_annotation_targets_and_hash_conflicts_without_pruning() {
    let (app, _root, _store, w) = setup(2);
    initialize(app.path());
    let request = window_note(&w);
    annotations::append_at(app.path(), request.clone(), 2000).unwrap();
    let notes = Store::open_namespace(
        app.path(),
        "annotations",
        false,
        "afk-review-v1",
        1024 * 1024,
    )
    .unwrap();
    for missing_decision in [true, false] {
        let mut changed = request.clone();
        if missing_decision {
            changed.target.decision = Some(uuid::Uuid::new_v4().to_string());
        } else {
            changed.target.original_hash = "a".repeat(64);
        }
        let entry = annotations::Annotation {
            id: changed.id.clone(),
            target: changed.target.clone(),
            kind: changed.kind,
            body: Some(changed.body.clone()),
            prior: None,
            created_at_ms: 2000,
            request_hash: hash(serde_json::to_vec(&changed).unwrap()),
        };
        notes
            .write_raw(
                "profile-default.json",
                &json!({"version":1,"profile":"default","entries":[entry]}),
            )
            .unwrap();
        let before = tree(app.path());
        let report = service::collect_at(
            app.path(),
            &[selected(&w)],
            w.expires_at_ms + RETENTION_MS + 1,
        )
        .unwrap();
        let row = &report.selections[0];
        assert_eq!(row.annotations.len(), 1);
        assert_eq!(
            row.annotations[0].body.as_deref(),
            Some(changed.body.as_str())
        );
        assert_eq!(before, tree(app.path()));
        assert!(
            !report.complete,
            "unresolvable annotation target must be explicit"
        );
        assert!(row.errors.iter().any(|error| error.contains(&changed.id)));
        assert!(row.window.is_some(), "captured host facts remain available");
    }
}

#[test]
fn review_json_escapes_unicode_identities_without_changing_exact_targets() {
    let (app, _root, _store, w) = setup(2);
    initialize(app.path());
    for profile in [
        "historical profile 名",
        "historical\u{202e}profile\u{2066}",
        "historical\u{7f}profile",
    ] {
        let mut historical = w.clone();
        historical.binding.profile = profile.into();
        let store = Store::authority(app.path(), profile, "test", true).unwrap();
        Book {
            version: 4,
            windows: vec![historical.clone()],
        }
        .save(&store)
        .unwrap();
        annotations::append_at(app.path(), window_note(&historical), 2000).unwrap();
        let mut missing = selected(&historical);
        missing.profile.push_str(" missing");
        let before = tree(app.path());
        let report =
            service::collect_at(app.path(), &[selected(&historical), missing.clone()], 2000)
                .unwrap();
        assert!(!report.complete);
        assert!(
            report.selections[0].errors.is_empty(),
            "{:?}",
            report.selections[0].errors
        );
        assert_eq!(report.selections[0].annotations.len(), 1);
        let bytes = service::render(&report, Format::Json).unwrap();
        assert!(
            bytes.windows(3).any(|window| window == b"\n  "),
            "JSON remains pretty formatted"
        );
        let decoded: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded["selections"][0]["selection"]["profile"], profile);
        assert_eq!(
            decoded["selections"][0]["annotations"][0]["target"]["selection"]["profile"],
            profile
        );
        assert_eq!(
            decoded["selections"][1]["selection"]["profile"],
            missing.profile
        );
        let receipt = service::render_annotation(&report.selections[0].annotations[0]).unwrap();
        let decoded: annotations::Annotation = serde_json::from_slice(&receipt).unwrap();
        assert_eq!(decoded.target.selection.profile, profile);
        assert!(!String::from_utf8(receipt)
            .unwrap()
            .contains(['\u{202e}', '\u{2066}', '\u{7f}']));
        for format in [Format::Json, Format::Markdown] {
            let text = String::from_utf8(service::render(&report, format).unwrap()).unwrap();
            assert!(
                !text.contains(['\u{202e}', '\u{2066}', '\u{7f}']),
                "raw Unicode controls in {format:?}"
            );
        }
        assert_eq!(before, tree(app.path()));
    }
}

#[test]
fn review_profile_admission_counts_utf8_bytes_without_truncation() {
    let app = tempfile::tempdir().unwrap();
    let at_limit = Selection {
        profile: "é".repeat(120),
        session_id: "test".into(),
        window_id: uuid::Uuid::new_v4().to_string(),
    };
    let report = service::collect_at(app.path(), std::slice::from_ref(&at_limit), 2000).unwrap();
    assert_eq!(report.selections[0].selection.profile.len(), 240);
    assert_eq!(report.selections[0].selection, at_limit);
    let mut too_long = at_limit;
    too_long.profile.push('x');
    let before = tree(app.path());
    assert!(
        service::collect_at(app.path(), &[too_long], 2000).is_err(),
        "241 UTF-8 bytes exceed review admission"
    );
    assert_eq!(before, tree(app.path()));
}

#[test]
fn review_annotation_first_profile_sync_failures_never_repair_or_change_authority() {
    use crate::session::anchored_fs::FAIL_SYNC_ONCE;
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAIL_SYNC_ONCE.set(None);
            annotations::AFTER_PROFILE_PUBLICATION.set(None);
        }
    }
    let _reset = Reset;
    for index_publication in [false, true] {
        let (app, _root, _store, w) = setup(2);
        initialize(app.path());
        let request = window_note(&w);
        let authority_before = tree(&app.path().join(authority::NAMESPACE));
        let notes = app.path().join("afk-review-v1/annotations");
        if index_publication {
            annotations::AFTER_PROFILE_PUBLICATION.set(Some(Box::new(move || {
                FAIL_SYNC_ONCE.set(Some(notes));
            })));
        } else {
            FAIL_SYNC_ONCE.set(Some(notes));
        }
        assert!(annotations::append_at(app.path(), request.clone(), 2000).is_err());
        assert!(app
            .path()
            .join("afk-review-v1/annotations/profile-default.json")
            .exists());
        let after_failure = tree(app.path());
        let retry = annotations::append_at(app.path(), request.clone(), 3000);
        if index_publication {
            let retry = retry.unwrap();
            assert_eq!(retry.id, request.id);
            assert_eq!(retry.created_at_ms, 2000);
        } else {
            assert!(retry.unwrap_err().to_string().contains("unregistered"));
        }
        let report = service::collect_at(app.path(), &[selected(&w)], 3000).unwrap();
        assert_eq!(report.complete, index_publication);
        assert_eq!(after_failure, tree(app.path()));
        assert_eq!(
            authority_before,
            tree(&app.path().join(authority::NAMESPACE))
        );
    }
}

#[test]
fn review_annotation_profile_capacity_refuses_append_without_eviction() {
    let (app, _root, store, w) = setup(2);
    initialize(app.path());
    let mut windows = vec![w.clone()];
    for revision in 2..=33 {
        let mut next = w.clone();
        next.id = uuid::Uuid::new_v4().to_string();
        next.revision = revision;
        windows.push(next);
    }
    Book {
        version: 4,
        windows: windows.clone(),
    }
    .save(&store)
    .unwrap();
    annotations::append_at(app.path(), window_note(&w), 2000).unwrap();
    let mut entries = Vec::new();
    let mut serialized = Vec::new();
    for index in 0..512 {
        let mut request = window_note(&windows[index / 16]);
        request.body = "x".repeat(2048);
        entries.push(annotations::Annotation {
            id: request.id.clone(),
            target: request.target.clone(),
            kind: request.kind,
            body: Some(request.body.clone()),
            prior: None,
            created_at_ms: 2000,
            request_hash: hash(serde_json::to_vec(&request).unwrap()),
        });
        let candidate =
            serde_json::to_vec(&json!({"version":1,"profile":"default","entries":entries}))
                .unwrap();
        if candidate.len() > 1024 * 1024 {
            break;
        }
        serialized = candidate;
    }
    assert!(!serialized.is_empty());
    let notes = Store::open_namespace(
        app.path(),
        "annotations",
        false,
        "afk-review-v1",
        1024 * 1024,
    )
    .unwrap();
    notes
        .write_bytes("profile-default.json", &serialized)
        .unwrap();
    let mut request = window_note(windows.last().unwrap());
    request.body = "x".repeat(2048);
    let before = tree(app.path());
    let error = annotations::append_at(app.path(), request, 2000).unwrap_err();
    assert!(error.to_string().contains("1 MiB"), "{error}");
    assert_eq!(before, tree(app.path()));
    assert!(
        service::collect_at(app.path(), &[selected(&w)], 2000)
            .unwrap()
            .complete
    );
}

#[test]
fn review_annotation_index_capacity_refuses_new_profile_before_any_publication() {
    let (app, _root, _store, w) = setup(2);
    initialize(app.path());
    annotations::append_at(app.path(), window_note(&w), 2000).unwrap();
    let notes = Store::open_namespace(
        app.path(),
        "annotations",
        false,
        "afk-review-v1",
        1024 * 1024,
    )
    .unwrap();
    let mut profiles = std::collections::BTreeSet::from(["default".to_owned()]);
    let mut rejected_profile = None;
    for index in 0..127 {
        let name = format!("{index:03}{}", "\u{1}".repeat(237));
        assert_eq!(name.len(), service::PROFILE_NAME_BYTES);
        let mut proposed = profiles.clone();
        proposed.insert(name.clone());
        let bytes = serde_json::to_vec(&json!({"version":1,"profiles":proposed})).unwrap();
        if bytes.len() > 64 * 1024 {
            rejected_profile = Some(name);
            break;
        }
        profiles = proposed;
        notes
            .write_raw(
                &format!("profile-{name}.json"),
                &json!({"version":1,"profile":name,"entries":[]}),
            )
            .unwrap();
    }
    let name = rejected_profile.expect("encoded names cross the index ceiling before128 profiles");
    assert!(profiles.len() < 128);
    let index = serde_json::to_vec(&json!({"version":1,"profiles":profiles})).unwrap();
    assert!(index.len() <= 64 * 1024);
    notes.write_bytes("index.json", &index).unwrap();
    let mut historical = w.clone();
    historical.binding.profile = name.clone();
    let store = Store::authority(app.path(), &name, "test", true).unwrap();
    Book {
        version: 4,
        windows: vec![historical.clone()],
    }
    .save(&store)
    .unwrap();
    let before = tree(app.path());
    assert!(
        service::collect_at(app.path(), &[selected(&w)], 2000)
            .unwrap()
            .complete
    );
    let error = annotations::append_at(app.path(), window_note(&historical), 2000).unwrap_err();
    assert!(error.to_string().contains("64 KiB"), "{error}");
    assert_eq!(before, tree(app.path()));
    assert!(notes
        .dir
        .entry_stat(Path::new(&format!("profile-{name}.json")))
        .unwrap()
        .is_none());
    assert!(
        service::collect_at(app.path(), &[selected(&w)], 2000)
            .unwrap()
            .complete
    );
    assert_eq!(before, tree(app.path()));
}

#[test]
fn review_projects_host_question_and_nudge_guards_without_mutating_or_resolving_them() {
    for (outcome, observed_question, recovered, observed_nudge, expected_question) in [
        ("accepted", false, false, false, true),
        ("ordinary", false, false, true, false),
        ("accepted", false, true, true, false),
        ("accepted", true, false, true, false),
        ("unknown", false, false, true, true),
    ] {
        let (app, _root, store, mut w) = setup(3);
        initialize(app.path());
        claims(&mut w, false);
        w.grant.question_deferrals = 1;
        w.grant.settlement_nudges = 1;
        let request = w.reservations[0].clone();
        w.question = Some(serde_json::from_value(json!({
            "identity":{"operation":uuid::Uuid::new_v4().to_string(),"tool_call_id":"q","interaction_id":uuid::Uuid::new_v4().to_string(),"kind":"input","arguments_hash":hash("args"),"context_ref":"stored"},
            "admitted_at_ms":1500,"emission_intent_at_ms":1600,"outcome":outcome,"outcome_at_ms":1700,
            "observed_request":if observed_question {Some(&request)} else {None},"recovered_at_ms":if recovered {Some(1900)} else {None}
        })).unwrap());
        let facts = hash("stored progress");
        w.checkpoints = vec![serde_json::from_value(json!({"sequence":1,"request":1,"recorded_at_ms":1800,"facts":facts,
            "claims":{"status":"unfinished","next_step":{"kind":"record","path":"scratch.txt"},"rationale":"stored checkpoint","evidence":[],"depends_on":[]}})).unwrap()];
        w.nudges = vec![serde_json::from_value(json!({"identity":{"window":w.id,"generation":w.generation,"revision":w.revision,"checkpoint":1,"admission":uuid::Uuid::new_v4().to_string()},
            "facts":facts,"admitted_at_ms":1800,"contribution_intent_at_ms":1900,"observed_request":if observed_nudge {Some(&request)} else {None}})).unwrap()];
        let book = Book {
            version: 4,
            windows: vec![w.clone()],
        };
        book.validate().unwrap();
        book.save(&store).unwrap();
        let original = serde_json::to_vec(&book).unwrap();
        let view = book.review_window(&w.id, 2000).unwrap();
        assert_eq!(view["unresolved_question_guard"], expected_question);
        assert_eq!(view["unresolved_nudge"], !observed_nudge);
        assert_eq!(view["ambiguous"], expected_question || !observed_nudge);
        assert_eq!(view["question"]["outcome"], outcome);
        assert_eq!(view["records"][0]["claims"]["human_required"], false);
        assert_eq!(serde_json::to_vec(&book).unwrap(), original);
        let authority_before = tree(&app.path().join(authority::NAMESPACE));
        let mut note = window_note(&w);
        note.kind = annotations::Kind::Reviewed;
        annotations::append_at(app.path(), note, 2000).unwrap();
        let review = service::collect_at(app.path(), &[selected(&w)], 2000).unwrap();
        let after = review.selections[0].window.as_ref().unwrap();
        assert_eq!(after["unresolved_question_guard"], expected_question);
        assert_eq!(after["unresolved_nudge"], !observed_nudge);
        assert_eq!(
            authority_before,
            tree(&app.path().join(authority::NAMESPACE))
        );
    }
}
