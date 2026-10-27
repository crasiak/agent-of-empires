use super::{tests::*, *};
use crate::github::forge::{Forge, HttpRequest, HttpResponse, Method, PublishStage, Transport};
use git2::{Oid, Repository};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

fn tree_set(repo: &Repository, root: Oid, path: &str, id: Oid, mode: i32) -> Result<Oid> {
    let (name, rest) = path
        .split_once('/')
        .map_or((path, None), |(a, b)| (a, Some(b)));
    let tree = repo.find_tree(root)?;
    let mut builder = repo.treebuilder(Some(&tree))?;
    if let Some(rest) = rest {
        let child = builder
            .get(name)?
            .map(|e| e.id())
            .unwrap_or(repo.treebuilder(None)?.write()?);
        builder.insert(name, tree_set(repo, child, rest, id, mode)?, 0o040000)?;
    } else {
        builder.insert(name, id, mode)?;
    }
    Ok(builder.write()?)
}
fn remote_commit(repo: &Repository, tree: Oid, parent: Oid, message: &str) -> Result<Oid> {
    let sig = git2::Signature::new(
        "Server",
        "server@fixture.invalid",
        &git2::Time::new(1_700_000_000, 0),
    )?;
    Ok(repo.commit(
        None,
        &sig,
        &sig,
        message,
        &repo.find_tree(tree)?,
        &[&repo.find_commit(parent)?],
    )?)
}
struct Remote {
    repo: Repository,
    base: Oid,
    original_base: Oid,
    pr: Option<Value>,
    writes: Vec<String>,
    lost_ref: bool,
    lost_merge: bool,
    bad_merge: Option<&'static str>,
    bad_poll: bool,
    fail_ref_settle: bool,
    fail_poll_settle: bool,
    fail_poll_dirsync: bool,
    requests: usize,
    request: String,
    store: Store,
}
#[derive(Clone)]
struct Raw(Arc<Mutex<Remote>>);
impl Transport for Raw {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse> {
        use base64::Engine;
        let mut r = self.0.lock().unwrap();
        r.requests += 1;
        let body: Value = req
            .body()
            .map(serde_json::from_slice)
            .transpose()?
            .unwrap_or(Value::Null);
        let request = r.store.get(&r.request)?;
        let source = request
            .publication
            .as_ref()
            .and_then(|p| p.preview.as_ref())
            .map(|p| &p.source);
        if req.method() != Method::Get {
            let p = request
                .publication
                .as_ref()
                .context("Intent must precede remote write")?;
            if req.method() == Method::Put {
                assert_eq!(request.state, State::Landing);
                assert!(p.land_intent.is_some());
                assert!(p.land_observation_pending);
                let execution = p
                    .land_execution
                    .as_ref()
                    .expect("begin_land execution must be durable BEFORE PUT");
                assert!(matches!(
                    execution.state(),
                    crate::github::forge::LandExecutionState::Admitted
                ));
                assert_eq!(
                    execution.intent().attempt(),
                    p.land_intent.as_ref().unwrap().attempt()
                );
            } else {
                assert_eq!(request.state, State::Publishing);
                let stage = p
                    .intent
                    .as_ref()
                    .context("Persist exact per-effect stage")?
                    .stage();
                if req.path().ends_with("/git/refs") {
                    assert!(matches!(stage, PublishStage::CreateRef { .. }));
                } else if req.path().ends_with("/pulls") {
                    assert!(matches!(stage, PublishStage::CreatePr { .. }));
                } else {
                    assert!(matches!(stage, PublishStage::Upload));
                }
            }
            r.writes.push(req.path().into());
        }
        let path = req
            .path()
            .strip_prefix("/repos/fixture/skills")
            .unwrap_or(req.path());
        let mut status = 200;
        let value = match (req.method(), path) {
            (Method::Get, "/user") => json!({"id":7,"login":"host-owner","type":"User"}),
            (Method::Get, "") => {
                json!({"id":42,"node_id":"R_fixture","full_name":"fixture/skills","clone_url":"https://github.com/fixture/skills.git","visibility":"public","private":false,"default_branch":"main","archived":false,"disabled":false})
            }
            (Method::Get, "/git/ref/heads/main") => {
                json!({"ref":"refs/heads/main","object":{"type":"commit","sha":r.base.to_string()}})
            }
            (Method::Get, p) if p.starts_with("/compare/") => {
                json!({"status":"ahead","merge_base_commit":{"sha":r.original_base.to_string()}})
            }
            (Method::Post, "/git/blobs") => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(body["content"].as_str().unwrap())?;
                json!({"sha":r.repo.blob(&bytes)?.to_string()})
            }
            (Method::Post, "/git/trees") => {
                let mut tree = body["base_tree"]
                    .as_str()
                    .map(Oid::from_str)
                    .transpose()?
                    .unwrap_or(r.repo.treebuilder(None)?.write()?);
                for entry in body["tree"].as_array().unwrap() {
                    tree = tree_set(
                        &r.repo,
                        tree,
                        entry["path"].as_str().unwrap(),
                        Oid::from_str(entry["sha"].as_str().unwrap())?,
                        i32::from_str_radix(entry["mode"].as_str().unwrap(), 8)?,
                    )?;
                }
                json!({"sha":tree.to_string()})
            }
            (Method::Post, "/git/commits") => {
                json!({"sha":remote_commit(&r.repo, Oid::from_str(body["tree"].as_str().unwrap())?, Oid::from_str(body["parents"][0].as_str().unwrap())?, body["message"].as_str().unwrap())?.to_string()})
            }
            (Method::Get, p) if p.starts_with("/git/commits/") => {
                let commit = r
                    .repo
                    .find_commit(Oid::from_str(p.trim_start_matches("/git/commits/"))?)?;
                let signature = |sig: git2::Signature<'_>| json!({"name":sig.name().unwrap(),"email":sig.email().unwrap(),"date":chrono::DateTime::from_timestamp(sig.when().seconds(),0).unwrap().to_rfc3339()});
                json!({"sha":commit.id().to_string(),"tree":{"sha":commit.tree_id().to_string()},"parents":commit.parent_ids().map(|id|json!({"sha":id.to_string()})).collect::<Vec<_>>(),"author":signature(commit.author()),"committer":signature(commit.committer()),"message":commit.message().unwrap(),"verification":{"verified":false,"reason":"unsigned","signature":null,"payload":null}})
            }
            (Method::Get, p) if p.starts_with("/git/trees/") => {
                let tree = r
                    .repo
                    .find_tree(Oid::from_str(p.trim_start_matches("/git/trees/"))?)?;
                json!({"sha":tree.id().to_string(),"truncated":false,"tree":tree.iter().map(|e|json!({"path":e.name().unwrap(),"mode":format!("{:06o}",e.filemode()),"type":if e.kind()==Some(git2::ObjectType::Tree){"tree"}else{"blob"},"sha":e.id().to_string(),"size":if e.kind()==Some(git2::ObjectType::Blob){Some(r.repo.find_blob(e.id()).unwrap().size())}else{None}})).collect::<Vec<_>>()})
            }
            (Method::Get, p) if p.starts_with("/git/blobs/") => {
                let blob = r
                    .repo
                    .find_blob(Oid::from_str(p.trim_start_matches("/git/blobs/"))?)?;
                json!({"sha":blob.id().to_string(),"size":blob.size(),"encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(blob.content())})
            }
            (Method::Get, p) if p.starts_with("/git/ref/heads/workshop%2F") => {
                let owned = source.context("source")?.owned_ref();
                match r.repo.find_reference(&owned) {
                    Ok(reference) => {
                        json!({"ref":owned,"object":{"type":"commit","sha":reference.target().unwrap().to_string()}})
                    }
                    Err(_) => {
                        status = 404;
                        json!({"message":"Not Found"})
                    }
                }
            }
            (Method::Post, "/git/refs") => {
                r.repo.reference(
                    body["ref"].as_str().unwrap(),
                    Oid::from_str(body["sha"].as_str().unwrap())?,
                    false,
                    "fixture",
                )?;
                if r.fail_ref_settle {
                    r.fail_ref_settle = false;
                    r.store
                        .fail_settle
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                if r.lost_ref {
                    r.lost_ref = false;
                    bail!("lost ref acknowledgement");
                }
                status = 201;
                json!({"ref":body["ref"],"object":{"type":"commit","sha":body["sha"]}})
            }
            (Method::Get, p) if p.starts_with("/pulls?state=all&head=") => {
                json!(r.pr.iter().collect::<Vec<_>>())
            }
            (Method::Post, "/pulls") => {
                let head = r
                    .repo
                    .find_reference(&source.context("source")?.owned_ref())?
                    .target()
                    .unwrap()
                    .to_string();
                let pr = json!({"id":123,"node_id":"PR_fixture","number":3,"html_url":"https://github.com/fixture/skills/pull/3","title":body["title"],"body":body["body"],"user":{"id":7},"maintainer_can_modify":false,"head":{"sha":head,"ref":body["head"],"repo":{"id":42}},"base":{"sha":r.base.to_string(),"ref":"main","repo":{"id":42}},"state":"open","draft":false,"merged":false,"merged_at":null,"merge_commit_sha":null,"mergeable":true,"auto_merge":null,"stack":null});
                r.pr = Some(pr.clone());
                status = 201;
                pr
            }
            (Method::Get, "/pulls/3") => r.pr.clone().unwrap(),
            (Method::Get, p) if p.starts_with("/rules/branches/main?") => json!([]),
            (Method::Get, "/branches/main/protection") => {
                json!({"required_status_checks":{"strict":true,"contexts":["ci"],"checks":[{"context":"ci","app_id":42}]},"required_pull_request_reviews":{"dismiss_stale_reviews":true,"require_code_owner_reviews":false,"require_last_push_approval":false,"required_approving_review_count":1},"enforce_admins":{"enabled":false},"restrictions":null,"required_linear_history":{"enabled":true},"required_conversation_resolution":{"enabled":true},"lock_branch":{"enabled":false}})
            }
            (Method::Put, "/pulls/3/merge-async") => {
                assert_eq!(req.api_version(), "2026-03-10");
                assert_eq!(
                    body,
                    json!({"sha":r.pr.as_ref().unwrap()["head"]["sha"],"merge_method":"squash","merge_action":"direct_merge","bypass_rules":false})
                );
                let source = source.unwrap();
                let head = r.repo.find_commit(Oid::from_str(
                    r.pr.as_ref().unwrap()["head"]["sha"].as_str().unwrap(),
                )?)?;
                let tree = tree_set(
                    &r.repo,
                    r.repo.find_commit(r.base)?.tree_id(),
                    &source.skill_path,
                    head.tree()?.get_path(Path::new(&source.skill_path))?.id(),
                    0o040000,
                )?;
                let merged = remote_commit(&r.repo, tree, r.base, "Squashed actual fixture")?;
                drop(head);
                let pr = r.pr.as_mut().unwrap();
                pr["merged"] = json!(true);
                pr["state"] = json!("closed");
                pr["merged_at"] = json!("2026-01-01T00:00:00Z");
                pr["merge_commit_sha"] = json!(merged.to_string());
                pr["merged_by"] = json!({"id":7});
                if r.lost_merge {
                    r.lost_merge = false;
                    bail!("lost merge acknowledgement");
                }
                status = 202;
                let mut response = json!({"status":"pending","details":{"message":"pending is not landed","uuid":"4fdf0961-8919-43c8-9c39-6cfec9a094d7","merge_method":"squash","merge_action":"direct_merge","expected_head_sha":body["sha"],"bypass_rules":false}});
                match r.bad_merge {
                    Some("bypass") => response["details"]["bypass_rules"] = json!(true),
                    Some("queue") => {
                        response =
                            json!({"status":"enqueued","details":{"message":"queue is prohibited"}})
                    }
                    Some("wrong-action") => {
                        response["details"]["merge_action"] = json!("auto_merge")
                    }
                    _ => {}
                }
                response
            }
            (Method::Get, p) if p.starts_with("/pulls/3/merge-async/") => {
                if r.bad_poll {
                    r.bad_poll = false;
                    let store = r.store.clone();
                    let id = request.id.clone();
                    let revision = request.revision;
                    let host = Forge::new(self.clone());
                    let (tx, rx) = std::sync::mpsc::channel();
                    let contender = std::thread::spawn(move || {
                        let visibility = store.operate(Operation::Visibility {
                            id: id.clone(),
                            revision,
                            hidden: true,
                        });
                        let reconcile = store.publication_operation(
                            Operation::ReconcilePublication { id, revision },
                            &host,
                        );
                        tx.send((visibility.is_err(), reconcile.is_err())).unwrap();
                    });
                    assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),(true,true),"Concurrent UI revision or second authority observer must be refused during admitted read/settle");
                    contender.join().unwrap();
                    assert_eq!(r.store.get(&request.id)?.revision, request.revision);
                    if r.fail_poll_dirsync {
                        r.fail_poll_dirsync = false;
                        r.store
                            .fail_directory_sync
                            .store(2, std::sync::atomic::Ordering::SeqCst);
                    }
                    if r.fail_poll_settle {
                        r.fail_poll_settle = false;
                        r.store
                            .fail_settle
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    return Ok(HttpResponse {
                        status: 200,
                        body: serde_json::to_vec(
                            &json!({"status":"pending","details":{"message":"observed prohibited execution","uuid":"4fdf0961-8919-43c8-9c39-6cfec9a094d7","merge_method":"squash","merge_action":"direct_merge","expected_head_sha":r.pr.as_ref().unwrap()["head"]["sha"],"bypass_rules":true}}),
                        )?,
                        next_page: false,
                    });
                }
                json!({"status":"merged","details":{"message":"actual merge","sha":r.pr.as_ref().unwrap()["merge_commit_sha"]}})
            }
            _ => bail!(
                "Unexpected fixture request {:?} {}",
                req.method(),
                req.path()
            ),
        };
        Ok(HttpResponse {
            status,
            body: serde_json::to_vec(&value)?,
            next_page: false,
        })
    }
}

fn committed(store: &Store, path: &Path) -> Request {
    let request = store
        .create(uuid::Uuid::new_v4().to_string(), envelope(path))
        .unwrap();
    let request = store
        .operate(Operation::Propose {
            id: request.id,
            revision: 0,
            bundle: bundle(),
        })
        .unwrap();
    let request = store
        .operate(Operation::Review {
            id: request.id,
            revision: request.revision,
        })
        .unwrap();
    store
        .operate(Operation::Commit {
            id: request.id,
            revision: request.revision,
            review_digest: request.review.unwrap().digest,
        })
        .unwrap()
}
#[test]
#[serial_test::serial]
fn store_persists_each_real_forge_stage_and_reopens_pending_without_duplicate_writes() {
    run_scenario("pending");
}
#[test]
#[serial_test::serial]
fn known_bad_land_evidence_survives_reopen_and_cannot_mint_deploy_authority() {
    for mode in [
        "bypass",
        "queue",
        "wrong-action",
        "lost-ack",
        "old-execution",
        "bad-poll",
        "settle-failure",
        "bad-poll-settle",
        "bad-poll-dirsync",
        "admission-open",
        "admission-sync",
    ] {
        run_scenario(mode);
    }
}
fn run_scenario(mode: &str) {
    let dir = tempfile::tempdir().unwrap();
    let _app_guard = crate::session::test_support::isolate_app_dir_at(&dir.path().join("app"));
    let path = dir.path().join("source");
    repository(&path);
    let store = Store::at(dir.path().join("workshop.json"));
    let request = committed(&store, &path);
    let remote_dir = dir.path().join("remote");
    let remote = git2::build::RepoBuilder::new()
        .bare(true)
        .clone(path.to_str().unwrap(), &remote_dir)
        .unwrap();
    if let Ok(mut reference) = remote.find_reference(&format!("refs/heads/workshop/{}", request.id))
    {
        reference.delete().unwrap();
    }
    let base = remote.head().unwrap().peel_to_commit().unwrap().id();
    let new_blob = remote.blob(b"legitimate base progress").unwrap();
    let tree = tree_set(
        &remote,
        remote.find_commit(base).unwrap().tree_id(),
        "unrelated.txt",
        new_blob,
        0o100644,
    )
    .unwrap();
    let advanced = remote_commit(&remote, tree, base, "base advanced").unwrap();
    let raw = Raw(Arc::new(Mutex::new(Remote {
        repo: remote,
        base: advanced,
        original_base: base,
        pr: None,
        writes: vec![],
        lost_ref: mode != "settle-failure",
        bad_poll: ["bad-poll", "bad-poll-settle", "bad-poll-dirsync"].contains(&mode),
        fail_poll_dirsync: mode == "bad-poll-dirsync",
        fail_poll_settle: mode == "bad-poll-settle",
        requests: 0,
        fail_ref_settle: mode == "settle-failure",
        lost_merge: mode == "lost-ack",
        bad_merge: match mode {
            "bypass" => Some("bypass"),
            "queue" => Some("queue"),
            "wrong-action" => Some("wrong-action"),
            _ => None,
        },
        request: request.id.clone(),
        store: store.clone(),
    })));
    let host = Forge::new(raw.clone());
    let request = store
        .publication_operation(
            Operation::PreviewPublish {
                id: request.id,
                revision: request.revision,
                remote: "https://github.com/fixture/skills.git".into(),
                repository_id: 42,
                base_branch: "main".into(),
                title: "Exact reviewed skill".into(),
                body: "Explicit source disclosure".into(),
            },
            &host,
        )
        .unwrap();
    let digest = request
        .publication
        .as_ref()
        .unwrap()
        .preview
        .as_ref()
        .unwrap()
        .digest
        .clone();
    assert_ne!(digest, request.review.as_ref().unwrap().digest);
    assert!(store
        .publication_operation(
            Operation::Publish {
                id: request.id.clone(),
                revision: request.revision,
                preview_digest: digest.clone(),
                disclosure_confirmed: false
            },
            &host
        )
        .is_err());
    assert!(raw.0.lock().unwrap().writes.is_empty());
    let published = store.publication_operation(
        Operation::Publish {
            id: request.id.clone(),
            revision: request.revision,
            preview_digest: digest,
            disclosure_confirmed: true,
        },
        &host,
    );
    if mode == "settle-failure" {
        let error = published.unwrap_err();
        assert!(error.downcast_ref::<EffectOutcomeUncertain>().is_some());
        let pending = store.get(&request.id).unwrap();
        assert_eq!(pending.state, State::Publishing);
        assert!(matches!(
            pending
                .publication
                .as_ref()
                .unwrap()
                .intent
                .as_ref()
                .unwrap()
                .stage(),
            PublishStage::CreateRef { .. }
        ));
        let writes = raw.0.lock().unwrap().writes.len();
        let reconciled = store
            .publication_operation(
                Operation::ReconcilePublication {
                    id: pending.id,
                    revision: pending.revision,
                },
                &host,
            )
            .unwrap();
        assert_eq!(reconciled.state, State::PublishReady);
        assert_eq!(raw.0.lock().unwrap().writes.len(), writes);
        return;
    }
    let request = published.unwrap();
    assert_eq!(request.state, State::PublishUnknown);
    let writes = raw.0.lock().unwrap().writes.len();
    let request = store
        .publication_operation(
            Operation::ReconcilePublication {
                id: request.id,
                revision: request.revision,
            },
            &host,
        )
        .unwrap();
    assert_eq!(request.state, State::PublishReady);
    assert_eq!(raw.0.lock().unwrap().writes.len(), writes);
    let request = store
        .publication_operation(
            Operation::ContinuePublish {
                id: request.id,
                revision: request.revision,
            },
            &host,
        )
        .unwrap();
    assert_eq!(request.state, State::Published);
    let published = request
        .publication
        .as_ref()
        .unwrap()
        .published
        .as_ref()
        .unwrap();
    assert_ne!(published.head, request.commit.as_ref().unwrap().commit_oid);
    let request = store
        .publication_operation(
            Operation::PreviewLand {
                id: request.id,
                revision: request.revision,
            },
            &host,
        )
        .unwrap();
    let policy_digest = request
        .publication
        .as_ref()
        .unwrap()
        .policy
        .as_ref()
        .unwrap()
        .digest
        .clone();
    assert!(store
        .publication_operation(
            Operation::Land {
                id: request.id.clone(),
                revision: request.revision,
                policy_digest: policy_digest.clone(),
                current_base_and_rules: true,
                exclusive_pr_custody: false
            },
            &host
        )
        .is_err());
    if ["admission-open", "admission-sync"].contains(&mode) {
        let calls = raw.0.lock().unwrap().requests;
        store.fail_directory_sync.store(
            if mode == "admission-open" { 1 } else { 2 },
            std::sync::atomic::Ordering::SeqCst,
        );
        let result = store.publication_operation(
            Operation::Land {
                id: request.id.clone(),
                revision: request.revision,
                policy_digest,
                current_base_and_rules: true,
                exclusive_pr_custody: true,
            },
            &host,
        );
        assert!(result.is_err());
        assert_eq!(
            raw.0.lock().unwrap().requests,
            calls,
            "Post-rename directory failure must refuse admission BEFORE any Land host read/write"
        );
        let pending = store.get(&request.id).unwrap();
        assert!(
            pending
                .publication
                .as_ref()
                .unwrap()
                .land_observation_pending
        );
        assert!(store
            .publication_operation(
                Operation::ReconcilePublication {
                    id: pending.id,
                    revision: pending.revision
                },
                &host
            )
            .is_err());
        assert_eq!(raw.0.lock().unwrap().requests, calls);
        return;
    }
    let request = store
        .publication_operation(
            Operation::Land {
                id: request.id,
                revision: request.revision,
                policy_digest,
                current_base_and_rules: true,
                exclusive_pr_custody: true,
            },
            &host,
        )
        .unwrap();
    assert_eq!(
        request.state,
        if [
            "pending",
            "old-execution",
            "bad-poll",
            "bad-poll-settle",
            "bad-poll-dirsync"
        ]
        .contains(&mode)
        {
            State::LandPending
        } else {
            State::LandUnknown
        }
    );
    assert!(request.publication.as_ref().unwrap().landed.is_none());
    if mode == "old-execution" {
        let writes = raw.0.lock().unwrap().writes.len();
        store
            .change(&request.id, |r| {
                r.publication.as_mut().unwrap().land_execution = None;
                Ok(())
            })
            .unwrap();
        let reopened = store.get(&request.id).unwrap();
        assert!(store
            .publication_operation(
                Operation::ReconcilePublication {
                    id: reopened.id,
                    revision: reopened.revision
                },
                &host
            )
            .is_err());
        assert_eq!(raw.0.lock().unwrap().writes.len(), writes);
        assert!(store
            .get(&request.id)
            .unwrap()
            .publication
            .unwrap()
            .landed
            .is_none());
        return;
    }
    if ["bypass", "queue", "wrong-action"].contains(&mode) {
        let execution = request
            .publication
            .as_ref()
            .unwrap()
            .land_execution
            .as_ref()
            .unwrap();
        assert!(matches!(
            execution.state(),
            crate::github::forge::LandExecutionState::Quarantined { .. }
        ));
        let writes = raw.0.lock().unwrap().writes.len();
        let mut reopened = store.get(&request.id).unwrap();
        for _ in 0..2 {
            reopened = store
                .publication_operation(
                    Operation::ReconcilePublication {
                        id: reopened.id,
                        revision: reopened.revision,
                    },
                    &host,
                )
                .unwrap();
            assert_eq!(reopened.state, State::LandUnknown);
            assert!(reopened.publication.as_ref().unwrap().landed.is_none());
            assert_eq!(raw.0.lock().unwrap().writes.len(), writes);
            assert!(store
                .deployment_with(
                    Operation::PreviewImport {
                        id: reopened.id.clone(),
                        revision: reopened.revision,
                        source_name: "fixture".into(),
                        targets: vec![]
                    },
                    None::<&super::deployment::CliCompanion>,
                    Some(&host)
                )
                .is_err());
        }
        return;
    }
    let writes = raw.0.lock().unwrap().writes.len();
    let reopened = Store::at(dir.path().join("workshop.json"))
        .get(&request.id)
        .unwrap();
    let observed = store.publication_operation(
        Operation::ReconcilePublication {
            id: reopened.id,
            revision: reopened.revision,
        },
        &host,
    );
    if mode == "bad-poll-settle" || mode == "bad-poll-dirsync" {
        assert!(observed
            .unwrap_err()
            .downcast_ref::<EffectOutcomeUncertain>()
            .is_some());
        let retained = store.get(&request.id).unwrap();
        let publication = retained.publication.as_ref().unwrap();
        assert!(
            publication.land_observation_pending
                || matches!(
                    publication.land_execution.as_ref().unwrap().state(),
                    crate::github::forge::LandExecutionState::Quarantined { .. }
                )
        );
        assert!(retained.publication.as_ref().unwrap().landed.is_none());
        let requests = raw.0.lock().unwrap().requests;
        let result = store.publication_operation(
            Operation::ReconcilePublication {
                id: retained.id.clone(),
                revision: retained.revision,
            },
            &host,
        );
        if publication.land_observation_pending {
            assert!(result.is_err());
        } else {
            let row = result.unwrap();
            assert_eq!(row.state, State::LandUnknown);
            assert!(row.publication.unwrap().landed.is_none());
        }
        assert_eq!(
            raw.0.lock().unwrap().requests,
            requests,
            "Unsettled known-bad observation must not query older execution/PR fallback"
        );
        assert!(store
            .deployment_with(
                Operation::PreviewImport {
                    id: retained.id,
                    revision: retained.revision,
                    source_name: "fixture".into(),
                    targets: vec![]
                },
                None::<&super::deployment::CliCompanion>,
                Some(&host)
            )
            .is_err());
        return;
    }
    let mut request = observed.unwrap();
    if mode == "bad-poll" {
        for _ in 0..2 {
            assert_eq!(request.state, State::LandUnknown);
            assert!(matches!(
                request
                    .publication
                    .as_ref()
                    .unwrap()
                    .land_execution
                    .as_ref()
                    .unwrap()
                    .state(),
                crate::github::forge::LandExecutionState::Quarantined { .. }
            ));
            assert!(request.publication.as_ref().unwrap().landed.is_none());
            request = store
                .publication_operation(
                    Operation::ReconcilePublication {
                        id: request.id,
                        revision: request.revision,
                    },
                    &host,
                )
                .unwrap();
        }
        assert_eq!(raw.0.lock().unwrap().writes.len(), writes);
        return;
    }
    for _ in 0..2 {
        if request.state == State::Landed {
            break;
        }
        request = store
            .publication_operation(
                Operation::ReconcilePublication {
                    id: request.id,
                    revision: request.revision,
                },
                &host,
            )
            .unwrap();
    }
    assert_eq!(request.state, State::Landed);
    assert_eq!(raw.0.lock().unwrap().writes.len(), writes);
    let landed = request
        .publication
        .as_ref()
        .unwrap()
        .landed
        .as_ref()
        .unwrap();
    assert_ne!(
        landed.merged_commit,
        request.commit.as_ref().unwrap().commit_oid
    );
    assert_eq!(
        landed.skill_tree,
        request.review.as_ref().unwrap().skill_tree
    );
    assert_ne!(
        landed.source_tree,
        request.review.as_ref().unwrap().source_tree
    );
    assert!(Repository::open(&path)
        .unwrap()
        .find_commit(Oid::from_str(&landed.merged_commit).unwrap())
        .is_err());
    if mode == "pending" {
        super::composition_tests::optional_real_cli(&store, &request, &host);
    }
}
