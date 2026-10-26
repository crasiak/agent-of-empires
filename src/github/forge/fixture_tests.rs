use super::*;
use crate::github::forge::transport::decode_http;
use git2::{Oid, Repository};
use std::sync::Arc;

const UUID: &str = "92f9571f-b989-46ef-b82c-ddd4b3fa8874";
const ASYNC_UUID: &str = "4fdf0961-8919-43c8-9c39-6cfec9a094d7";
fn set(repo: &Repository, root: Oid, path: &str, id: Oid, mode: i32) -> Oid {
    let (name, rest) = path
        .split_once('/')
        .map_or((path, None), |(a, b)| (a, Some(b)));
    let existing = repo.find_tree(root).ok();
    let mut builder = repo.treebuilder(existing.as_ref()).unwrap();
    if let Some(rest) = rest {
        let child = builder
            .get(name)
            .unwrap()
            .map(|e| e.id())
            .unwrap_or_else(|| repo.treebuilder(None).unwrap().write().unwrap());
        let next = set(repo, child, rest, id, mode);
        builder.insert(name, next, 0o040000).unwrap();
    } else {
        builder.insert(name, id, mode).unwrap();
    }
    builder.write().unwrap()
}
fn commit(repo: &Repository, tree: Oid, parent: Option<Oid>, message: &str) -> Oid {
    let signature = git2::Signature::new(
        "Fixture",
        "fixture@example.invalid",
        &git2::Time::new(1_700_000_000, 0),
    )
    .unwrap();
    let parents: Vec<_> = parent
        .map(|p| repo.find_commit(p).unwrap())
        .into_iter()
        .collect();
    repo.commit(
        None,
        &signature,
        &signature,
        message,
        &repo.find_tree(tree).unwrap(),
        &parents.iter().collect::<Vec<_>>(),
    )
    .unwrap()
}
fn classic() -> Value {
    json!({"required_status_checks":{"strict":true,"contexts":["ci"],"checks":[{"context":"ci","app_id":42}]},"required_pull_request_reviews":{"dismiss_stale_reviews":true,"require_code_owner_reviews":false,"require_last_push_approval":false,"required_approving_review_count":1},"enforce_admins":{"enabled":false},"restrictions":null,"required_linear_history":{"enabled":true},"required_conversation_resolution":{"enabled":true},"lock_branch":{"enabled":false}})
}
struct Remote {
    repo: Repository,
    source: HostSource,
    base: Oid,
    pr: Option<Value>,
    fault: String,
    writes: Vec<(String, Value)>,
    pending_reads: usize,
}
struct Fake(Arc<Mutex<Remote>>);
impl Transport for Fake {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse> {
        assert_eq!(request.api_version(), API_VERSION);
        let mut state = self.0.lock().unwrap();
        let body: Value = request
            .body()
            .map(serde_json::from_slice)
            .transpose()?
            .unwrap_or(Value::Null);
        if request.method() != Method::Get {
            state.writes.push((request.path().into(), body.clone()));
        }
        let suffix = request
            .path()
            .strip_prefix("/repos/acme/skills")
            .unwrap_or(request.path());
        let mut status = 200;
        let value = match (request.method(), suffix) {
            (Method::Get, "/user") => {
                json!({"id":if state.fault=="actor" {8}else{7},"login":"host-user","type":"User"})
            }
            (Method::Get, "") => {
                json!({"id":1,"node_id":"R_fixture","full_name":"acme/skills","clone_url":"https://github.com/acme/skills.git","visibility":if state.fault=="visibility" {"private"}else{"public"},"private":state.fault=="visibility","default_branch":"main","archived":false,"disabled":false})
            }
            (Method::Get, "/git/ref/heads/main") => {
                json!({"ref":"refs/heads/main","object":{"type":"commit","sha":state.base.to_string()}})
            }
            (Method::Get, p) if p.starts_with("/compare/") => {
                json!({"status":"ahead","merge_base_commit":{"sha":state.source.base_commit}})
            }
            (Method::Get, p) if p.starts_with("/git/ref/heads/workshop%2F") => {
                match state.repo.find_reference(&state.source.owned_ref()) {
                    Ok(reference) => {
                        json!({"ref":state.source.owned_ref(),"object":{"type":"commit","sha":reference.target().unwrap().to_string()}})
                    }
                    Err(_) => {
                        status = 404;
                        json!({"message":"Not Found"})
                    }
                }
            }
            (Method::Post, "/git/blobs") => {
                use base64::Engine;
                assert_eq!(body["encoding"], "base64");
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(body["content"].as_str().unwrap())?;
                json!({"sha":state.repo.blob(&bytes)?.to_string()})
            }
            (Method::Post, "/git/trees") => {
                let mut root = body["base_tree"]
                    .as_str()
                    .map(Oid::from_str)
                    .transpose()?
                    .unwrap_or(state.repo.treebuilder(None)?.write()?);
                for entry in body["tree"].as_array().unwrap() {
                    root = set(
                        &state.repo,
                        root,
                        entry["path"].as_str().unwrap(),
                        Oid::from_str(entry["sha"].as_str().unwrap())?,
                        i32::from_str_radix(entry["mode"].as_str().unwrap(), 8)?,
                    );
                }
                json!({"sha":root.to_string()})
            }
            (Method::Post, "/git/commits") => {
                let sha = commit(
                    &state.repo,
                    Oid::from_str(body["tree"].as_str().unwrap())?,
                    Some(Oid::from_str(body["parents"][0].as_str().unwrap())?),
                    body["message"].as_str().unwrap(),
                );
                json!({"sha":sha.to_string()})
            }
            (Method::Get, p) if p.starts_with("/git/commits/") => {
                let sha = Oid::from_str(p.trim_start_matches("/git/commits/"))?;
                let commit = state.repo.find_commit(sha)?;
                let signature = |sig: git2::Signature<'_>| {
                    let date = chrono::DateTime::from_timestamp(sig.when().seconds(), 0).unwrap();
                    json!({"name":sig.name().unwrap(),"email":sig.email().unwrap(),"date":date.to_rfc3339()})
                };
                let verification = match state.repo.extract_signature(&sha, None) {
                    Ok((signature, payload)) => {
                        json!({"verified":true,"reason":"valid","signature":std::str::from_utf8(&signature)?,"payload":std::str::from_utf8(&payload)?})
                    }
                    Err(_) => {
                        json!({"verified":false,"reason":"unsigned","signature":null,"payload":null})
                    }
                };
                let mut value = json!({"sha":sha.to_string(),"tree":{"sha":commit.tree_id().to_string()},"parents":commit.parent_ids().map(|sha|json!({"sha":sha.to_string()})).collect::<Vec<_>>(),"author":signature(commit.author()),"committer":signature(commit.committer()),"message":commit.message().unwrap_or("lossy invalid UTF8"),"verification":verification});
                if state.fault == "extra_commit_field" {
                    value["future_header"] = json!("unknown");
                }
                value
            }
            (Method::Get, p) if p.starts_with("/git/trees/") => {
                let sha = Oid::from_str(p.trim_start_matches("/git/trees/"))?;
                let tree = state.repo.find_tree(sha)?;
                json!({"sha":sha.to_string(),"truncated":state.fault=="truncated_tree","tree":tree.iter().map(|e|json!({"path":e.name().unwrap(),"mode":format!("{:06o}",e.filemode()),"type":if e.kind()==Some(git2::ObjectType::Tree){"tree"}else{"blob"},"sha":e.id().to_string(),"size":if e.kind()==Some(git2::ObjectType::Blob) {Some(state.repo.find_blob(e.id()).unwrap().size())}else{None}})).collect::<Vec<_>>()})
            }
            (Method::Get, p) if p.starts_with("/git/blobs/") => {
                use base64::Engine;
                let sha = Oid::from_str(p.trim_start_matches("/git/blobs/"))?;
                let blob = state.repo.find_blob(sha)?;
                json!({"sha":sha.to_string(),"size":if state.fault=="blob_size" {65_537}else{blob.size()},"encoding":"base64","content":if state.fault=="blob_truncated" {String::new()}else{base64::engine::general_purpose::STANDARD.encode(blob.content())}})
            }
            (Method::Post, "/git/refs") => {
                assert_eq!(body["ref"], state.source.owned_ref());
                state.repo.reference(
                    body["ref"].as_str().unwrap(),
                    Oid::from_str(body["sha"].as_str().unwrap())?,
                    false,
                    "fixture create only",
                )?;
                if state.fault == "ref_ack" {
                    bail!("fixture timeout after server ref create");
                }
                status = 201;
                json!({"ref":body["ref"],"object":{"type":"commit","sha":body["sha"]}})
            }
            (Method::Get, p) if p.starts_with("/pulls?state=all&head=") => {
                let mut values = state.pr.clone().into_iter().collect::<Vec<_>>();
                if state.fault == "duplicate" {
                    values.push(values[0].clone());
                }
                json!(values)
            }
            (Method::Post, "/pulls") => {
                assert_eq!(body["head"], state.source.branch());
                assert_eq!(body["base"], "main");
                assert_eq!(body["maintainer_can_modify"], false);
                let head = state
                    .repo
                    .find_reference(&state.source.owned_ref())?
                    .target()
                    .unwrap()
                    .to_string();
                let pr = json!({"id":123,"node_id":"PR_fixture","number":3,"html_url":"https://github.com/acme/skills/pull/3","title":body["title"],"body":body["body"],"user":{"id":7},"maintainer_can_modify":false,"head":{"sha":head,"ref":state.source.branch(),"repo":{"id":1}},"base":{"sha":state.base.to_string(),"ref":"main","repo":{"id":1}},"state":"open","draft":false,"merged":false,"merged_at":null,"merge_commit_sha":null,"mergeable":true,"auto_merge":null,"stack":null});
                state.pr = Some(pr.clone());
                if state.fault == "pr_ack" {
                    bail!("fixture timeout after server PR create");
                }
                status = 201;
                pr
            }
            (Method::Get, "/pulls/3") => {
                let mut pr = state.pr.clone().unwrap();
                pr["base"]["sha"] = json!(state.base.to_string());
                match state.fault.as_str() {
                    "head" => pr["head"]["sha"] = json!("1111111111111111111111111111111111111111"),
                    "stack" => {
                        pr["stack"] =
                            json!({"base":{"ref":"main","sha":state.base.to_string()},"size":2})
                    }
                    "missing_stack" => {
                        pr.as_object_mut().unwrap().remove("stack");
                    }
                    "null_repo" => pr["head"]["repo"] = Value::Null,
                    "drift_after_merge" if pr["merged"] == true => {
                        pr["base"]["ref"] = json!("other")
                    }
                    _ => {}
                }
                pr
            }
            (Method::Get, p) if p.starts_with("/rules/branches/main?") => {
                if state.fault == "truncated_pages" {
                    json!((0..100)
                        .map(|_| json!({"type":"deletion"}))
                        .collect::<Vec<_>>())
                } else if state.fault == "unknown_rule" {
                    json!([{"type":"future_policy","ruleset_id":1,"ruleset_source":"acme","ruleset_source_type":"Organization"}])
                } else {
                    json!([])
                }
            }
            (Method::Get, "/branches/main/protection") => match state.fault.as_str() {
                "private403" => {
                    status = 403;
                    json!({"message":"Forbidden"})
                }
                "private404" => {
                    status = 404;
                    json!({"message":"Not Found"})
                }
                "partial_policy" => json!({"required_status_checks":null}),
                "null_policy" => Value::Null,
                "codeowners" => {
                    let mut p = classic();
                    p["required_pull_request_reviews"]["require_code_owner_reviews"] = json!(true);
                    p
                }
                _ => classic(),
            },
            (Method::Put, "/pulls/3/merge-async") => {
                assert_eq!(
                    body,
                    json!({"sha":state.pr.as_ref().unwrap()["head"]["sha"],"merge_method":"squash","merge_action":"direct_merge","bypass_rules":false})
                );
                if state.fault == "policy_rejected" {
                    return raw_response(
                        200,
                        json!({"status":"failed","details":{"message":"Required review/check absent"}}),
                        false,
                    );
                }
                let old_tree = state.repo.find_commit(state.base)?.tree_id();
                let skill = if state.fault == "wrong_tree" {
                    state.repo.treebuilder(None)?.write()?
                } else {
                    Oid::from_str(&state.source.skill_tree)?
                };
                let tree = set(
                    &state.repo,
                    old_tree,
                    &state.source.skill_path,
                    skill,
                    0o040000,
                );
                let merged = commit(&state.repo, tree, Some(state.base), "Squashed fixture");
                let merged = if state.fault == "signed" {
                    let raw = state.repo.odb()?.read(merged)?.data().to_vec();
                    state.repo.commit_signed(std::str::from_utf8(&raw)?,"-----BEGIN PGP SIGNATURE-----\nFixture only, not cryptographic proof\n-----END PGP SIGNATURE-----\n",None)?
                } else if state.fault == "extra_header" || state.fault == "non_utf8_commit" {
                    let mut raw = state.repo.odb()?.read(merged)?.data().to_vec();
                    if state.fault == "extra_header" {
                        let position = raw.windows(2).position(|b| b == b"\n\n").unwrap() + 1;
                        raw.splice(position..position, b"encoding ISO-8859-1\n".iter().copied());
                    } else {
                        raw.push(0xff);
                    }
                    state.repo.odb()?.write(git2::ObjectType::Commit, &raw)?
                } else {
                    merged
                };
                let pr = state.pr.as_mut().unwrap();
                pr["merged"] = json!(true);
                pr["state"] = json!("closed");
                pr["merged_at"] = json!("2026-01-01T00:00:00Z");
                pr["merge_commit_sha"] = json!(merged.to_string());
                pr["merged_by"] = json!({"id":7});
                if state.fault == "merge_ack" {
                    bail!("fixture lost merge ACK");
                }
                status = 202;
                let mut response = json!({"status":"pending","details":{"message":"accepted, not protection approval","uuid":ASYNC_UUID,"merge_method":"squash","merge_action":"direct_merge","expected_head_sha":body["sha"],"bypass_rules":false}});
                match state.fault.as_str() {
                    "bad_bypass" => response["details"]["bypass_rules"] = json!(true),
                    "bad_action" => response["details"]["merge_action"] = json!("merge_queue"),
                    "bad_method" => response["details"]["merge_method"] = json!("merge"),
                    "bad_head" => {
                        response["details"]["expected_head_sha"] =
                            json!("1111111111111111111111111111111111111111")
                    }
                    "bad_unknown" => response["details"]["unknown_policy"] = json!(true),
                    "bad_enqueued" => {
                        response = json!({"status":"enqueued","details":{"message":"queued"}})
                    }
                    _ => {}
                }
                response
            }
            (Method::Get, p) if p == format!("/pulls/3/merge-async/{ASYNC_UUID}") => {
                state.pending_reads += 1;
                if state.fault == "expired" {
                    status = 404;
                    json!({"message":"Not Found","status":"404"})
                } else if state.fault.starts_with("bad_") || state.fault == "404_bypass" {
                    let mut value = json!({"status":"pending","details":{"message":"observed contradictory execution","uuid":ASYNC_UUID,"merge_method":"squash","merge_action":"direct_merge","expected_head_sha":state.source.commit_oid,"bypass_rules":false}});
                    match state.fault.as_str() {
                        "bad_bypass" | "404_bypass" => {
                            value["details"]["bypass_rules"] = json!(true);
                            if state.fault == "404_bypass" {
                                status = 404;
                            }
                        }
                        "bad_action" => value["details"]["merge_action"] = json!("merge_queue"),
                        "bad_method" => value["details"]["merge_method"] = json!("merge"),
                        "bad_head" => {
                            value["details"]["expected_head_sha"] =
                                json!("1111111111111111111111111111111111111111")
                        }
                        "bad_unknown" => value["details"]["future"] = json!(true),
                        "bad_enqueued" => {
                            value = json!({"status":"enqueued","details":{"message":"queued"}})
                        }
                        _ => {}
                    }
                    value
                } else {
                    json!({"status":"merged","details":{"message":"merged","sha":state.pr.as_ref().unwrap()["merge_commit_sha"]}})
                }
            }
            _ => panic!(
                "Unexpected fixture request {:?} {}",
                request.method(),
                request.path()
            ),
        };
        raw_response(status, value, state.fault == "truncated_pages")
    }
}
fn raw_response(status: u16, body: Value, next: bool) -> Result<HttpResponse> {
    let body = serde_json::to_vec(&body)?;
    let mut raw = format!(
        "HTTP/2.0 {status} Fixture\r\nContent-Length: {}\r\n{}\r\n",
        body.len(),
        if next {
            "Link: <https://api.github.com/fixture?page=2>; rel=\"next\"\r\n"
        } else {
            ""
        }
    )
    .into_bytes();
    raw.extend(body);
    decode_http(&raw)
}
struct Setup {
    _local: tempfile::TempDir,
    _remote: tempfile::TempDir,
    source: HostSource,
    remote: Arc<Mutex<Remote>>,
}
impl Setup {
    fn new() -> Self {
        Self::new_with_mode(0o100644)
    }
    fn new_with_mode(mode: i32) -> Self {
        let local = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        let repo = Repository::init(local.path()).unwrap();
        let empty = repo.treebuilder(None).unwrap().write().unwrap();
        let blob = repo.blob(b"old").unwrap();
        let root = set(&repo, empty, "README.md", blob, 0o100644);
        let base = commit(&repo, root, None, "base");
        repo.reference("refs/heads/main", base, false, "fixture")
            .unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let blob = repo
            .blob(b"---\nname: fixture\ndescription: fake skill\n---\nReviewed fixture\n")
            .unwrap();
        let source_tree = set(&repo, root, ".agents/skills/fixture/SKILL.md", blob, mode);
        let head = commit(&repo, source_tree, Some(base), "Reviewed fixture");
        repo.reference(
            &format!("refs/heads/workshop/{UUID}"),
            head,
            false,
            "fixture",
        )
        .unwrap();
        let skill_tree = repo
            .find_tree(source_tree)
            .unwrap()
            .get_path(std::path::Path::new(".agents/skills/fixture"))
            .unwrap()
            .id();
        let source = HostSource {
            repository: local.path().canonicalize().unwrap(),
            common_dir: repo.commondir().canonicalize().unwrap(),
            request_id: UUID.into(),
            revision: 1,
            review_digest: "a".repeat(64),
            base_commit: base.to_string(),
            commit_oid: head.to_string(),
            source_tree: source_tree.to_string(),
            skill_tree: skill_tree.to_string(),
            skill_path: ".agents/skills/fixture".into(),
            content_digest: HostSource::reviewed_content_digest(
                &local.path().canonicalize().unwrap(),
                &skill_tree.to_string(),
            )
            .unwrap(),
            account: Account::Work,
            source_scope: "Work private source context, not a disclosure grant".into(),
        };
        let mut options = git2::build::RepoBuilder::new();
        options.bare(true);
        let server = options
            .clone(local.path().to_str().unwrap(), remote.path())
            .unwrap();
        if let Ok(mut reference) = server.find_reference(&source.owned_ref()) {
            reference.delete().unwrap();
        }
        let changed = server.blob(b"legitimate unrelated base advance").unwrap();
        let new_tree = set(&server, root, "README.md", changed, 0o100644);
        let advanced = commit(&server, new_tree, Some(base), "base advanced");
        Self {
            _local: local,
            _remote: remote,
            source: source.clone(),
            remote: Arc::new(Mutex::new(Remote {
                repo: server,
                source,
                base: advanced,
                pr: None,
                fault: String::new(),
                writes: vec![],
                pending_reads: 0,
            })),
        }
    }
    fn forge(&self) -> Forge<Fake> {
        Forge::new(Fake(self.remote.clone()))
    }
    fn intent(&self) -> PublishIntent {
        let forge = self.forge();
        let preview = forge
            .preview_publish(
                self.source.clone(),
                Destination::new("https://github.com/acme/skills.git", 1, "main").unwrap(),
                "Review fixture".into(),
                "Explicit fake source publication".into(),
            )
            .unwrap();
        let approval = PublishApproval {
            preview_digest: preview.digest.clone(),
            disclosure_confirmed: true,
        };
        forge.prepare_publish(preview, approval).unwrap()
    }
    fn published(&self) -> PublishedPr {
        let mut intent = self.intent();
        loop {
            match self.forge().publish(&intent) {
                Outcome::Confirmed(PublishProgress::Next(next)) => {
                    intent = serde_json::from_slice(&serde_json::to_vec(&next).unwrap()).unwrap()
                }
                Outcome::Confirmed(PublishProgress::Published(pr)) => return *pr,
                other => panic!("{other:?}"),
            }
        }
    }
    fn land_intent(&self, published: &PublishedPr) -> LandIntent {
        let observation = self.forge().observe_policy(published).unwrap();
        let approval = LandApproval {
            policy_digest: observation.digest.clone(),
            method: MergeMethod::Squash,
            current_base_and_rules: true,
            exclusive_pr_custody: true,
        };
        self.forge().prepare_land(observation, approval).unwrap()
    }
}
#[test]
fn host_publish_and_nonbypass_land_verify_actual_subtree_under_advanced_base() {
    let setup = Setup::new();
    let published = setup.published();
    assert_eq!(published.head, setup.source.commit_oid);
    let intent = setup.land_intent(&published);
    let pending = match setup
        .forge()
        .land(&setup.forge().begin_land(&intent).unwrap())
    {
        LandOutcome::Confirmed(LandProgress::Pending(next)) => next,
        other => panic!("{other:?}"),
    };
    let landed = match setup.forge().poll_land(&pending) {
        LandOutcome::Confirmed(LandProgress::Landed(receipt)) => receipt,
        other => panic!("{other:?}"),
    };
    assert_eq!(landed.approved_head, published.head);
    assert_eq!(landed.skill_tree, setup.source.skill_tree);
    assert_ne!(landed.source_tree, setup.source.source_tree);
    let remote = setup.remote.lock().unwrap();
    let actual = remote
        .repo
        .find_commit(Oid::from_str(&landed.merged_commit).unwrap())
        .unwrap();
    assert_eq!(actual.parent_id(0).unwrap(), remote.base);
    let readme = actual.tree().unwrap().get_name("README.md").unwrap().id();
    assert_eq!(
        remote.repo.find_blob(readme).unwrap().content(),
        b"legitimate unrelated base advance"
    );
    assert_eq!(
        remote
            .writes
            .iter()
            .filter(|(p, _)| p.ends_with("merge-async"))
            .count(),
        1
    );
}
#[test]
fn publication_requires_destination_disclosure_and_invalidates_identity_changes() {
    let setup = Setup::new();
    let intent = setup.intent();
    let forge = setup.forge();
    let approval = PublishApproval {
        preview_digest: intent.preview.digest.clone(),
        disclosure_confirmed: false,
    };
    assert!(forge
        .prepare_publish(intent.preview.clone(), approval)
        .is_err());
    setup.remote.lock().unwrap().fault = "visibility".into();
    assert!(matches!(forge.publish(&intent), Outcome::Refused(_)));
    assert!(setup.remote.lock().unwrap().writes.is_empty());
    let mut source = setup.source.clone();
    source.skill_path = ".agents/skills/../escape".into();
    assert!(source::verify(&source).is_err());
}
#[test]
fn lost_publication_ack_reconciles_without_replaying_create() {
    for fault in ["ref_ack", "pr_ack"] {
        let setup = Setup::new();
        let mut intent = setup.intent();
        while !matches!(
            (&intent.stage, fault),
            (PublishStage::CreateRef { .. }, "ref_ack") | (PublishStage::CreatePr { .. }, "pr_ack")
        ) {
            intent = match setup.forge().publish(&intent) {
                Outcome::Confirmed(PublishProgress::Next(next)) => *next,
                other => panic!("{other:?}"),
            };
        }
        setup.remote.lock().unwrap().fault = fault.into();
        assert!(matches!(
            setup.forge().publish(&intent),
            Outcome::Unknown(_)
        ));
        let writes = setup.remote.lock().unwrap().writes.len();
        assert!(matches!(
            setup.forge().reconcile_publish(&intent),
            Outcome::Confirmed(_)
        ));
        assert_eq!(setup.remote.lock().unwrap().writes.len(), writes);
    }
}
#[test]
fn unknown_policy_identity_and_pagination_never_allow_merge() {
    for fault in [
        "private403",
        "private404",
        "partial_policy",
        "null_policy",
        "codeowners",
        "unknown_rule",
        "truncated_pages",
        "head",
        "stack",
        "missing_stack",
        "null_repo",
    ] {
        let setup = Setup::new();
        let published = setup.published();
        setup.remote.lock().unwrap().fault = fault.into();
        assert!(setup.forge().observe_policy(&published).is_err(), "{fault}");
        assert!(!setup
            .remote
            .lock()
            .unwrap()
            .writes
            .iter()
            .any(|(p, _)| p.ends_with("merge-async")));
    }
}
#[test]
fn exclusive_custody_is_explicit_and_detected_concurrent_drift_blocks_landed_proof() {
    let setup = Setup::new();
    let published = setup.published();
    let intent = setup.land_intent(&published);
    let mut approval = intent.approval.clone();
    approval.exclusive_pr_custody = false;
    assert!(setup
        .forge()
        .prepare_land(intent.observation.clone(), approval)
        .is_err());
    setup.remote.lock().unwrap().fault = "drift_after_merge".into();
    let pending = match setup
        .forge()
        .land(&setup.forge().begin_land(&intent).unwrap())
    {
        LandOutcome::Confirmed(LandProgress::Pending(next)) => next,
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        setup.forge().reconcile_land(&pending),
        LandOutcome::Uncertain { .. }
    ));
}
#[test]
fn lost_merge_ack_only_inspects_and_wrong_landed_tree_never_produces_receipt() {
    for fault in ["merge_ack", "wrong_tree", "policy_rejected"] {
        let setup = Setup::new();
        let published = setup.published();
        let intent = setup.land_intent(&published);
        setup.remote.lock().unwrap().fault = fault.into();
        let result = setup
            .forge()
            .land(&setup.forge().begin_land(&intent).unwrap());
        let writes = setup.remote.lock().unwrap().writes.len();
        match (fault, result) {
            ("merge_ack", LandOutcome::Uncertain { execution, .. }) => {
                assert!(matches!(
                    execution.state(),
                    LandExecutionState::UnobservedAck
                ));
                let restored: LandExecution =
                    serde_json::from_slice(&serde_json::to_vec(&execution).unwrap()).unwrap();
                assert!(matches!(
                    setup.forge().reconcile_land(&restored),
                    LandOutcome::Confirmed(LandProgress::Landed(_))
                ));
            }
            ("wrong_tree", LandOutcome::Confirmed(LandProgress::Pending(next))) => {
                assert!(matches!(
                    setup.forge().reconcile_land(&next),
                    LandOutcome::Uncertain { .. }
                ))
            }
            ("policy_rejected", LandOutcome::Uncertain { execution, .. }) => assert!(matches!(
                setup.forge().reconcile_land(&execution),
                LandOutcome::Uncertain { .. }
            )),
            (_, other) => panic!("{other:?}"),
        }
        assert_eq!(setup.remote.lock().unwrap().writes.len(), writes);
    }
}

fn actual_landed(setup: &Setup, fault: &str) -> (PublishedPr, Box<LandedSource>) {
    let published = setup.published();
    let intent = setup.land_intent(&published);
    setup.remote.lock().unwrap().fault = fault.into();
    let pending = match setup
        .forge()
        .land(&setup.forge().begin_land(&intent).unwrap())
    {
        LandOutcome::Confirmed(LandProgress::Pending(next)) => next,
        other => panic!("{other:?}"),
    };
    let landed = match setup.forge().reconcile_land(&pending) {
        LandOutcome::Confirmed(LandProgress::Landed(receipt)) => receipt,
        other => panic!("{other:?}"),
    };
    (published, landed)
}
#[test]
fn actual_signed_and_unsigned_remote_objects_export_losslessly_for_git_consumer() {
    for fault in ["", "signed"] {
        let setup = Setup::new_with_mode(0o100755);
        let (published, landed) = actual_landed(&setup, fault);
        let parent = private_parent();
        let writes = setup.remote.lock().unwrap().writes.len();
        let projection = setup
            .forge()
            .export_landed(
                &landed,
                &published.destination,
                &parent.path().canonicalize().unwrap(),
            )
            .unwrap();
        let odb = Repository::open_bare(projection.repository()).unwrap();
        let projected = odb
            .odb()
            .unwrap()
            .read(Oid::from_str(&landed.merged_commit).unwrap())
            .unwrap()
            .data()
            .to_vec();
        let remote = setup.remote.lock().unwrap();
        assert_eq!(
            projected,
            remote
                .repo
                .odb()
                .unwrap()
                .read(Oid::from_str(&landed.merged_commit).unwrap())
                .unwrap()
                .data()
        );
        assert!(
            odb.find_commit(remote.base).is_err(),
            "No full-repository/parent fetch"
        );
        assert_eq!(
            remote.writes.len(),
            writes,
            "Export must be read-only remotely"
        );
        drop(remote);
        let git = which::which("git").unwrap();
        let read = |args: &[&str]| {
            let mut command = std::process::Command::new(&git);
            command
                .env_clear()
                .env("HOME", "/dev/null")
                .env("PATH", "")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_NO_LAZY_FETCH", "1")
                .env("GIT_NO_REPLACE_OBJECTS", "1")
                .arg("--no-replace-objects")
                .arg("-C")
                .arg(projection.repository())
                .args(args);
            let output = crate::process::run_with_bounded_stdout(
                &mut command,
                Duration::from_secs(10),
                1024 * 1024,
            )
            .unwrap()
            .unwrap();
            assert!(output.status.success());
            output.stdout
        };
        assert_eq!(
            read(&["cat-file", "-t", &landed.merged_commit]),
            b"commit\n"
        );
        assert_eq!(
            String::from_utf8(read(&[
                "rev-parse",
                "--verify",
                &format!("{}^{{tree}}", landed.merged_commit)
            ]))
            .unwrap()
            .trim(),
            landed.source_tree
        );
        assert_eq!(
            String::from_utf8(read(&[
                "rev-parse",
                "--verify",
                &format!("{}:{}", landed.merged_commit, landed.skill_path)
            ]))
            .unwrap()
            .trim(),
            landed.skill_tree
        );
        let entries = read(&["ls-tree", "-r", "-z", &landed.skill_tree]);
        assert!(entries.ends_with(b"\tSKILL.md\0"));
        let blob = odb
            .find_tree(Oid::from_str(&landed.skill_tree).unwrap())
            .unwrap()
            .get_name("SKILL.md")
            .unwrap()
            .id()
            .to_string();
        assert_eq!(
            read(&["cat-file", "blob", &blob]),
            b"---\nname: fixture\ndescription: fake skill\n---\nReviewed fixture\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(projection.repository())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
    }
}
#[test]
fn export_refuses_lossy_commits_partial_objects_and_unsafe_projection_parent() {
    for fault in [
        "extra_header",
        "non_utf8_commit",
        "extra_commit_field",
        "truncated_tree",
        "blob_size",
        "blob_truncated",
    ] {
        let setup = Setup::new();
        let (published, landed) = actual_landed(
            &setup,
            if matches!(fault, "extra_header" | "non_utf8_commit") {
                fault
            } else {
                ""
            },
        );
        setup.remote.lock().unwrap().fault = fault.into();
        let parent = private_parent();
        assert!(
            setup
                .forge()
                .export_landed(
                    &landed,
                    &published.destination,
                    &parent.path().canonicalize().unwrap()
                )
                .is_err(),
            "{fault}"
        );
        assert_eq!(
            std::fs::read_dir(parent.path()).unwrap().count(),
            0,
            "Failed export must clean private projection"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let setup = Setup::new();
        let (published, landed) = actual_landed(&setup, "");
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().canonicalize().unwrap();
        symlink(&parent, parent.join("alias")).unwrap();
        assert!(setup
            .forge()
            .export_landed(&landed, &published.destination, &parent.join("alias"))
            .is_err());
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(setup
            .forge()
            .export_landed(&landed, &published.destination, &parent)
            .is_err());
    }
}

fn private_parent() -> tempfile::TempDir {
    let parent = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    parent
}
#[test]
fn projection_receipt_survives_handle_drop_and_reopens_but_refuses_snapshot_mutation() {
    let setup = Setup::new();
    let (published, landed) = actual_landed(&setup, "");
    let parent = private_parent();
    let receipt = setup
        .forge()
        .export_landed(
            &landed,
            &published.destination,
            &parent.path().canonicalize().unwrap(),
        )
        .unwrap();
    let json = serde_json::to_vec(&receipt).unwrap();
    let directory = receipt.directory().to_owned();
    drop(receipt);
    assert!(
        directory.exists(),
        "Handle drop must retain pending/uncertain deployment evidence"
    );
    let restored: ProjectionReceipt = serde_json::from_slice(&json).unwrap();
    restored.verify().unwrap();
    assert_eq!(
        std::fs::read(restored.source_root().join("SKILL.md")).unwrap(),
        b"---\nname: fixture\ndescription: fake skill\n---\nReviewed fixture\n"
    );
    std::fs::write(restored.source_root().join("SKILL.md"), "mutated").unwrap();
    assert!(restored.verify().is_err());
}

#[test]
fn publication_reconciliation_rejects_duplicate_or_absent_pr_without_write_replay() {
    let setup = Setup::new();
    let mut intent = setup.intent();
    for _ in 0..2 {
        intent = match setup.forge().publish(&intent) {
            Outcome::Confirmed(PublishProgress::Next(next)) => *next,
            other => panic!("{other:?}"),
        };
    }
    assert!(matches!(
        setup.forge().publish(&intent),
        Outcome::Confirmed(PublishProgress::Published(_))
    ));
    setup.remote.lock().unwrap().fault = "duplicate".into();
    let writes = setup.remote.lock().unwrap().writes.len();
    assert!(matches!(
        setup.forge().reconcile_publish(&intent),
        Outcome::Unknown(_)
    ));
    setup.remote.lock().unwrap().fault = String::new();
    setup.remote.lock().unwrap().pr = None;
    assert!(matches!(
        setup.forge().reconcile_publish(&intent),
        Outcome::Unknown(_)
    ));
    assert_eq!(setup.remote.lock().unwrap().writes.len(), writes);
}
#[test]
fn source_and_account_changes_refuse_before_writes_and_user_index_is_preserved() {
    let setup = Setup::new();
    let intent = setup.intent();
    setup.remote.lock().unwrap().fault = "actor".into();
    assert!(matches!(
        setup.forge().publish(&intent),
        Outcome::Refused(_)
    ));
    assert!(setup.remote.lock().unwrap().writes.is_empty());
    setup.remote.lock().unwrap().fault = String::new();
    let repo = Repository::open(&setup.source.repository).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add(&git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: 9,
            id: repo.blob(b"unrelated").unwrap(),
            flags: 0,
            flags_extended: 0,
            path: b"unrelated.txt".to_vec(),
        })
        .unwrap();
    index.write().unwrap();
    let before = std::fs::read(repo.path().join("index")).unwrap();
    assert!(matches!(
        setup.forge().publish(&intent),
        Outcome::Confirmed(PublishProgress::Next(_))
    ));
    assert_eq!(std::fs::read(repo.path().join("index")).unwrap(), before);
    repo.reference(
        &setup.source.owned_ref(),
        Oid::from_str(&setup.source.base_commit).unwrap(),
        true,
        "fixture drift",
    )
    .unwrap();
    let writes = setup.remote.lock().unwrap().writes.len();
    assert!(matches!(
        setup.forge().publish(&intent),
        Outcome::Refused(_)
    ));
    assert_eq!(setup.remote.lock().unwrap().writes.len(), writes);
}

#[test]
fn known_prohibited_execution_never_upgrades_through_reconciliation() {
    for fault in [
        "bad_bypass",
        "bad_enqueued",
        "bad_action",
        "bad_method",
        "bad_head",
        "bad_unknown",
    ] {
        let setup = Setup::new();
        let published = setup.published();
        let intent = setup.land_intent(&published);
        let admitted = setup.forge().begin_land(&intent).unwrap();
        setup.remote.lock().unwrap().fault = fault.into();
        let execution = match setup.forge().land(&admitted) {
            LandOutcome::Uncertain { execution, .. } => execution,
            other => panic!("{fault}: {other:?}"),
        };
        assert!(
            matches!(execution.state(), LandExecutionState::Quarantined { .. }),
            "{fault}"
        );
        assert_eq!(execution.attempt(), intent.attempt());
        assert_eq!(
            execution.intent().observation().published.head,
            published.head
        );
        if fault != "bad_enqueued" {
            assert_eq!(execution.async_uuid(), Some(ASYNC_UUID));
        }
        assert_eq!(
            setup.remote.lock().unwrap().pr.as_ref().unwrap()["merged"],
            true
        );
        let encoded = serde_json::to_vec(&execution).unwrap();
        drop(execution);
        drop(admitted);
        drop(intent);
        let restored: LandExecution = serde_json::from_slice(&encoded).unwrap();
        setup.remote.lock().unwrap().fault = String::new();
        let writes = setup.remote.lock().unwrap().writes.len();
        let next = match setup.forge().reconcile_land(&restored) {
            LandOutcome::Uncertain { execution, .. } => execution,
            other => panic!("{fault}: quarantined operation upgraded to {other:?}"),
        };
        assert!(matches!(
            next.state(),
            LandExecutionState::Quarantined { .. }
        ));
        assert_eq!(next.digest(), restored.digest());
        assert!(matches!(
            setup.forge().poll_land(&next),
            LandOutcome::Uncertain { .. }
        ));
        assert!(matches!(
            setup.forge().land(&next),
            LandOutcome::Uncertain { .. }
        ));
        assert_eq!(
            setup.remote.lock().unwrap().writes.len(),
            writes,
            "Quarantine is never replayed"
        );
    }
}

#[test]
fn polling_preserves_prohibited_response_and_never_reverts_to_prior_accepted_evidence() {
    for fault in [
        "bad_bypass",
        "bad_enqueued",
        "bad_action",
        "bad_method",
        "bad_head",
        "bad_unknown",
        "404_bypass",
    ] {
        let setup = Setup::new();
        let published = setup.published();
        let intent = setup.land_intent(&published);
        let admitted = setup.forge().begin_land(&intent).unwrap();
        let pending = match setup.forge().land(&admitted) {
            LandOutcome::Confirmed(LandProgress::Pending(next)) => next,
            other => panic!("{other:?}"),
        };
        let persisted: LandExecution =
            serde_json::from_slice(&serde_json::to_vec(&pending).unwrap()).unwrap();
        drop(pending);
        setup.remote.lock().unwrap().fault = fault.into();
        let quarantine = match setup.forge().poll_land(&persisted) {
            LandOutcome::Uncertain { execution, .. } => execution,
            other => panic!("{fault}: {other:?}"),
        };
        assert!(matches!(
            quarantine.state(),
            LandExecutionState::Quarantined { .. }
        ));
        assert_eq!(quarantine.async_uuid(), Some(ASYNC_UUID));
        let persisted: LandExecution =
            serde_json::from_slice(&serde_json::to_vec(&quarantine).unwrap()).unwrap();
        drop(quarantine);
        setup.remote.lock().unwrap().fault = String::new();
        let reads = setup.remote.lock().unwrap().pending_reads;
        assert!(matches!(
            setup.forge().reconcile_land(&persisted),
            LandOutcome::Uncertain { .. }
        ));
        assert_eq!(
            setup.remote.lock().unwrap().pending_reads,
            reads,
            "Quarantine must not be upgraded by later merged response"
        );
        assert_eq!(
            setup.remote.lock().unwrap().pr.as_ref().unwrap()["merged"],
            true
        );
    }
}
#[test]
fn execution_versions_fence_old_intents_landed_sources_and_projection_receipts() {
    let setup = Setup::new();
    let published = setup.published();
    let intent = setup.land_intent(&published);
    let mut old_intent = serde_json::to_value(&intent).unwrap();
    old_intent.as_object_mut().unwrap().remove("version");
    assert!(serde_json::from_value::<LandIntent>(old_intent).is_err());
    assert!(
        serde_json::from_value::<LandExecution>(serde_json::to_value(&intent).unwrap()).is_err(),
        "Original intent is not execution/reconciliation evidence"
    );
    let admitted = setup.forge().begin_land(&intent).unwrap();
    let mut missing_evidence = serde_json::to_value(&admitted).unwrap();
    missing_evidence.as_object_mut().unwrap().remove("state");
    assert!(serde_json::from_value::<LandExecution>(missing_evidence).is_err());
    let pending = match setup.forge().land(&admitted) {
        LandOutcome::Confirmed(LandProgress::Pending(next)) => next,
        other => panic!("{other:?}"),
    };
    setup.remote.lock().unwrap().fault = "expired".into();
    let landed = match setup.forge().reconcile_land(&pending) {
        LandOutcome::Confirmed(LandProgress::Landed(receipt)) => receipt,
        other => panic!("{other:?}"),
    };
    for field in ["execution_version", "execution_digest"] {
        let mut old_landed = serde_json::to_value(&landed).unwrap();
        old_landed.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<LandedSource>(old_landed).is_err(),
            "{field}"
        );
    }
    let mut invalid = (*landed).clone();
    invalid.execution_digest.clear();
    let parent = private_parent();
    assert!(setup
        .forge()
        .export_landed(
            &invalid,
            &published.destination,
            &parent.path().canonicalize().unwrap()
        )
        .is_err());
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    let receipt = setup
        .forge()
        .export_landed(
            &landed,
            &published.destination,
            &parent.path().canonicalize().unwrap(),
        )
        .unwrap();
    let mut old_projection = serde_json::to_value(&receipt).unwrap();
    old_projection["landed"]
        .as_object_mut()
        .unwrap()
        .remove("execution_digest");
    assert!(serde_json::from_value::<ProjectionReceipt>(old_projection.clone()).is_err());
    std::fs::write(
        receipt.directory().join("projection.json"),
        serde_json::to_vec(&old_projection).unwrap(),
    )
    .unwrap();
    assert!(receipt.verify().is_err());
    let directory = receipt.directory().to_owned();
    drop(receipt);
    assert!(
        directory.exists(),
        "Receipt fence must not delete retained evidence"
    );
}
