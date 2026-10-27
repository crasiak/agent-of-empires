use super::*;
use crate::tui::dialogs::ContextMenuAction;

#[test]
#[serial]
fn work_arc_consent_is_explicit_durable_and_bound_to_the_confirmed_conversation() {
    let mut pi = observed_fork_parent("pi");
    pi.source_profile = "test".into();
    let id = pi.id.clone();
    let mut env = seeded_env(test_home(), &[pi], true);
    env.view.selected_session = Some(id.clone());
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ToggleWorkArc);
    assert!(!env.view.get_instance(&id).unwrap().work_arc_enabled);
    assert_eq!(
        env.view.confirm_dialog.as_ref().unwrap().action(),
        "enable_work_arc"
    );
    env.view.dispatch_confirm_submit("enable_work_arc");
    assert!(env.view.get_instance(&id).unwrap().work_arc_enabled);
    assert!(Storage::new_unwatched("test").unwrap().load().unwrap()[0].work_arc_enabled);
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ToggleWorkArc);
    let row = Storage::new_unwatched("test")
        .unwrap()
        .load()
        .unwrap()
        .remove(0);
    assert!(!row.work_arc_enabled);
    assert!(row.work_arc.record.is_none());
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ToggleWorkArc);
    env.view
        .instances
        .get_mut(&id)
        .unwrap()
        .set_agent_conversation(Some("different".into()), None, None);
    env.view.dispatch_confirm_submit("enable_work_arc");
    assert!(!env.view.get_instance(&id).unwrap().work_arc_enabled);
}

use crate::session::work_arc::{self as arc, test_support::Fixture, ArcStatus};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

type FakeRequest = (tokio::io::BufReader<tokio::net::UnixStream>, Value);

struct FakeRoute {
    available: std::sync::Arc<std::sync::atomic::AtomicBool>,
    generations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for FakeRoute {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn fake_route(fixture: &Fixture) -> (FakeRoute, tokio::sync::mpsc::Receiver<FakeRequest>) {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    fixture.listener.set_nonblocking(true).unwrap();
    let listener =
        tokio::net::UnixListener::from_std(fixture.listener.try_clone().unwrap()).unwrap();
    let available = Arc::new(AtomicBool::new(true));
    let generations = Arc::new(AtomicUsize::new(0));
    let ready = available.clone();
    let attempts = generations.clone();
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            if request["op"] == "status" {
                assert!(request.get("input").is_none(), "status contains no excerpt");
                let reply = if ready.load(Ordering::SeqCst) {
                    json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/virtual"})
                } else {
                    json!({"ok":false,"nonce":request["nonce"],"error":"unavailable"})
                };
                stream
                    .get_mut()
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            } else {
                attempts.fetch_add(1, Ordering::SeqCst);
                if sender.send((stream, request)).await.is_err() {
                    break;
                }
            }
        }
    });
    (
        FakeRoute {
            available,
            generations,
            task,
        },
        receiver,
    )
}

fn allow_refresh(env: &mut TestEnv, id: &str) {
    let storage = Storage::new_unwatched("test").unwrap();
    storage
        .update(|rows, _| {
            let row = rows.iter_mut().find(|r| r.id == id).unwrap();
            let mut value = serde_json::to_value(&row.work_arc).unwrap();
            value["attempts"] = json!([chrono::Utc::now() - chrono::Duration::minutes(6)]);
            row.work_arc = serde_json::from_value(value).unwrap();
            Ok(())
        })
        .unwrap();
    env.view.work_arc_refresh.checked = None;
}

async fn complete_refresh(env: &mut TestEnv) {
    assert!(env.view.tick_work_arc());
    let task = env.view.work_arc_refresh.task.take().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    env.view.tick_work_arc();
}

#[tokio::test]
#[serial]
async fn work_arc_controller_tracks_synthetic_progress_branches_and_completion_with_durable_budget()
{
    let home = test_home();
    let fixture = Fixture::new(home.0.path());
    let (_route, mut requests) = fake_route(&fixture);
    let id = fixture.session.id.clone();
    let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
    env.view.selected_session = Some(id.clone());
    let server = tokio::spawn(async move {
        for now in [
            "Proposed plan",
            "Reported implementation",
            "Blocked on review",
            "Returned to main branch",
            "Agent reports complete",
        ] {
            let (mut stream, request) = requests.recv().await.unwrap();
            assert_eq!(request["instance_id"], id);
            assert!(request["input"]
                .as_str()
                .unwrap()
                .contains("Synthetic goal"));
            let reply = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/virtual","provider":"fake","model":"physical",
                "usage":{"input":10,"output":4},"text":json!({"goal":"Implement widget","now":now,"remaining":"Unknown","next":"Review reported evidence"}).to_string()});
            stream
                .get_mut()
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let id = fixture.session.id.clone();
    for (leaf, parent, evidence) in [
        ("initial", "goal", ""),
        ("progress", "initial", "Reported implementation"),
        ("detour", "initial", "Blocked on review"),
        ("return", "progress", "Returned to main branch"),
        ("complete", "return", "Agent reports complete"),
    ] {
        if leaf != "initial" {
            fixture.append(leaf, parent, evidence);
            allow_refresh(&mut env, &id);
        }
        complete_refresh(&mut env).await;
        let row = env.view.get_instance(&id).unwrap();
        assert_eq!(arc::projection(row), ArcStatus::Current);
        assert_eq!(row.work_arc.record.as_ref().unwrap().identity.leaf, leaf);
        assert_eq!(row.work_arc.cost, arc::ArcCost::Measured);
        let stored = Storage::new_unwatched("test")
            .unwrap()
            .load()
            .unwrap()
            .remove(0);
        assert_eq!(
            arc::projection(&stored),
            ArcStatus::Stale,
            "restart needs source validation"
        );
        assert!(!serde_json::to_string(&stored.work_arc)
            .unwrap()
            .contains("Synthetic goal"));
        env.view.work_arc_refresh.checked = None;
        complete_refresh(&mut env).await;
        assert_eq!(
            arc::projection(env.view.get_instance(&id).unwrap()),
            ArcStatus::Current,
            "same leaf doesn't request model again"
        );
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    fixture.append("later", "complete", "More work");
    env.view.work_arc_refresh.checked = None;
    complete_refresh(&mut env).await;
    assert_eq!(
        arc::projection(env.view.get_instance(&id).unwrap()),
        ArcStatus::Stale,
        "five-minute budget prevents another generation"
    );
}

#[tokio::test]
#[serial]
async fn work_arc_disable_aborts_live_socket_and_cannot_restore_a_late_cue() {
    let home = test_home();
    let fixture = Fixture::new(home.0.path());
    let (_route, mut requests) = fake_route(&fixture);
    let id = fixture.session.id.clone();
    let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
    env.view.selected_session = Some(id.clone());
    let (started, ready) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _request) = requests.recv().await.unwrap();
        started.send(()).unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(
            stream.read(&mut byte).await.unwrap(),
            0,
            "disable drops the model IPC"
        );
    });
    env.view.tick_work_arc();
    tokio::time::timeout(std::time::Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ToggleWorkArc);
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    env.view.tick_work_arc();
    let stored = Storage::new_unwatched("test")
        .unwrap()
        .load()
        .unwrap()
        .remove(0);
    assert!(!stored.work_arc_enabled);
    assert!(stored.work_arc.record.is_none());
    assert_eq!(
        arc::projection(env.view.get_instance(&id).unwrap()),
        ArcStatus::Cleared
    );
    assert_eq!(
        stored.work_arc.cost,
        arc::ArcCost::Unknown,
        "in-flight spend not zero"
    );
}

#[tokio::test]
#[serial]
async fn work_arc_controller_rejects_late_results_after_leaf_clear_owner_or_selection_changes() {
    for change in ["leaf", "clear", "owner", "selection", "purge"] {
        let home = test_home();
        let fixture = Fixture::new(home.0.path());
        let (_route, mut requests) = fake_route(&fixture);
        let id = fixture.session.id.clone();
        let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
        env.view.selected_session = Some(id.clone());
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, reply) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, request) = requests.recv().await.unwrap();
            started.send(()).unwrap();
            reply.await.unwrap();
            let result = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/route","provider":"fake","model":"physical",
                "text":r#"{"goal":"G","now":"Old leaf","remaining":"Unknown","next":"Review"}"#});
            let _ = stream
                .get_mut()
                .write_all(format!("{result}\n").as_bytes())
                .await;
        });
        env.view.tick_work_arc();
        tokio::time::timeout(std::time::Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        let storage = Storage::new_unwatched("test").unwrap();
        match change {
            "leaf" => fixture.append("new-leaf", "initial", "New evidence"),
            "clear" => {
                storage
                    .update(|rows, _| {
                        rows[0].set_agent_conversation(None, None, None);
                        Ok(())
                    })
                    .unwrap();
                env.view
                    .instances
                    .insert(id.clone(), storage.load().unwrap().remove(0));
            }
            "owner" => {
                let mut duplicate = fixture.session.clone();
                duplicate.id = Instance::new("other", "/tmp/other").id;
                seed_profile("other", &[duplicate]);
            }
            "selection" => env.view.selected_session = None,
            "purge" => {
                storage
                    .update(|rows, _| {
                        rows.clear();
                        Ok(())
                    })
                    .unwrap();
                env.view.instances.shift_remove(&id);
            }
            _ => unreachable!(),
        }
        env.view.tick_work_arc();
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        if let Some(task) = env.view.work_arc_refresh.task.take() {
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
        }
        env.view.tick_work_arc();
        assert!(
            storage
                .load()
                .unwrap()
                .iter()
                .all(|row| row.work_arc.record.is_none()),
            "{change}"
        );
        assert!(
            env.view
                .get_instance(&id)
                .is_none_or(|row| arc::projection(row) != ArcStatus::Current),
            "{change}"
        );
    }
}

#[tokio::test]
#[serial]
async fn work_arc_failed_attempt_preserves_last_good_as_stale_and_unknown_cost() {
    for unavailable in [false, true] {
        let home = test_home();
        let fixture = Fixture::new(home.0.path());
        let (_route, mut requests) = fake_route(&fixture);
        let id = fixture.session.id.clone();
        let mut session = fixture.session.clone();
        let identity = arc::read_arc_source(&session).unwrap().identity();
        let nonce = session
            .work_arc
            .reserve(&identity, chrono::Utc::now() - chrono::Duration::minutes(6))
            .unwrap();
        session.work_arc.finish(
            &nonce,
            Some(arc::StoredArc {
                identity,
                at: chrono::Utc::now(),
                arc: arc::WorkArc::from_json(
                    r#"{"goal":"G","now":"Last good","remaining":"Unknown","next":"Review"}"#,
                )
                .unwrap(),
                selected_route: "fake/route".into(),
                provider: "fake".into(),
                model: "model".into(),
                usage: Some(json!({"input":12})),
            }),
            None,
        );
        fixture.append("next", "initial", "Reported new work");
        let mut env = seeded_env(home, &[session], true);
        env.view.selected_session = Some(id.clone());
        let server = tokio::spawn(async move {
            let (mut stream, request) = requests.recv().await.unwrap();
            stream
            .get_mut()
            .write_all(
                format!(
                    "{}\n",
                    json!({"ok":false,"nonce":request["nonce"],"error":if unavailable { "unavailable" } else { "failed" },"usage":{"input":0,"output":0}})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        });
        complete_refresh(&mut env).await;
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        let row = env.view.get_instance(&id).unwrap();
        let expected = if unavailable {
            ArcStatus::Unavailable
        } else {
            ArcStatus::Failed
        };
        assert_eq!(arc::projection(row), expected);
        assert_eq!(
            row.work_arc.record.as_ref().unwrap().identity.leaf,
            "initial"
        );
        assert_eq!(
            row.work_arc.cost,
            if unavailable {
                arc::ArcCost::NoRequest
            } else {
                arc::ArcCost::Unknown
            }
        );
        env.view.work_arc_refresh.checked = None;
        complete_refresh(&mut env).await;
        assert_eq!(
            arc::projection(env.view.get_instance(&id).unwrap()),
            expected,
            "failure respects retry spacing"
        );
    }
}

#[tokio::test]
#[serial]
async fn work_arc_rejected_cue_retains_measured_attempt_usage() {
    for text in [
        "not JSON".to_owned(),
        json!({"goal":"word ".repeat(81),"now":"N","remaining":"R","next":"X"}).to_string(),
    ] {
        let home = test_home();
        let fixture = Fixture::new(home.0.path());
        let (_route, mut requests) = fake_route(&fixture);
        let id = fixture.session.id.clone();
        let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
        env.view.selected_session = Some(id.clone());
        let usage = json!({"input":25,"output":8,"cost":{"total":0.02}});
        let receipt = usage.clone();
        let server = tokio::spawn(async move {
            let (mut stream, request) = requests.recv().await.unwrap();
            let reply = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/virtual",
                "provider":"fake","model":"physical","usage":receipt,"text":text});
            stream
                .get_mut()
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });
        complete_refresh(&mut env).await;
        server.await.unwrap();
        let row = env.view.get_instance(&id).unwrap();
        assert_eq!(arc::projection(row), ArcStatus::Failed);
        assert!(row.work_arc.record.is_none());
        assert_eq!(row.work_arc.cost, arc::ArcCost::Measured);
        let state = serde_json::to_value(&row.work_arc).unwrap();
        assert_eq!(state["last_attempt_usage"], usage);
        let stored = Storage::new_unwatched("test").unwrap().load().unwrap();
        assert_eq!(serde_json::to_value(&stored[0].work_arc).unwrap(), state);
    }
}

#[tokio::test]
#[serial]
async fn unchanged_work_arc_becomes_unavailable_when_route_disappears_without_new_attempt() {
    let home = test_home();
    let fixture = Fixture::new(home.0.path());
    let (route, mut requests) = fake_route(&fixture);
    let id = fixture.session.id.clone();
    let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
    env.view.selected_session = Some(id.clone());
    let server = tokio::spawn(async move {
        let (mut stream, request) = requests.recv().await.unwrap();
        let reply = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/virtual","provider":"fake","model":"physical",
            "text":r#"{"goal":"G","now":"Last good","remaining":"Unknown","next":"Review"}"#});
        stream
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    });
    complete_refresh(&mut env).await;
    server.await.unwrap();
    assert_eq!(
        arc::projection(env.view.get_instance(&id).unwrap()),
        ArcStatus::Current
    );
    let before = Storage::new_unwatched("test")
        .unwrap()
        .load()
        .unwrap()
        .remove(0)
        .work_arc;
    route
        .available
        .store(false, std::sync::atomic::Ordering::SeqCst);
    env.view.work_arc_refresh.checked = None;
    complete_refresh(&mut env).await;
    let row = env.view.get_instance(&id).unwrap();
    assert_eq!(arc::projection(row), ArcStatus::Unavailable);
    assert_eq!(row.work_arc.record, before.record);
    assert_eq!(
        Storage::new_unwatched("test")
            .unwrap()
            .load()
            .unwrap()
            .remove(0)
            .work_arc,
        before,
        "status consumes no budget"
    );
    assert_eq!(
        route.generations.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
#[serial]
async fn work_arc_loading_survives_production_reload_only_with_unchanged_provenance_and_state() {
    let home = test_home();
    let fixture = Fixture::new(home.0.path());
    let (_route, mut requests) = fake_route(&fixture);
    let id = fixture.session.id.clone();
    let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
    env.view.selected_session = Some(id.clone());
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, request) = requests.recv().await.unwrap();
        started.send(()).unwrap();
        wait.await.unwrap();
        let reply = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/virtual","provider":"fake","model":"physical",
            "text":r#"{"goal":"G","now":"N","remaining":"Unknown","next":"Review"}"#});
        stream
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    });
    env.view.tick_work_arc();
    tokio::time::timeout(std::time::Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    env.view.tick_work_arc();
    assert_eq!(
        arc::projection(env.view.get_instance(&id).unwrap()),
        ArcStatus::Loading
    );
    env.view.reload_storage_only().unwrap();
    assert_eq!(
        arc::projection(env.view.get_instance(&id).unwrap()),
        ArcStatus::Loading
    );
    release.send(()).unwrap();
    let task = env.view.work_arc_refresh.task.take().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    env.view.tick_work_arc();
    assert_eq!(
        arc::projection(env.view.get_instance(&id).unwrap()),
        ArcStatus::Current
    );
    let original = env.view.get_instance(&id).unwrap().clone();
    for change in [
        "consent",
        "epoch",
        "conversation",
        "execution",
        "generation",
        "record",
    ] {
        env.view.instances.insert(id.clone(), original.clone());
        Storage::new_unwatched("test")
            .unwrap()
            .update(|rows, _| {
                rows[0] = original.clone();
                match change {
                    "consent" => rows[0].work_arc_enabled = false,
                    "epoch" => rows[0].work_arc.clear(),
                    "conversation" => {
                        rows[0].set_agent_conversation(Some("other".into()), None, None)
                    }
                    "execution" => {
                        rows[0].active_execution.as_mut().unwrap().launch_id =
                            uuid::Uuid::new_v4().to_string()
                    }
                    "generation" => rows[0].lifecycle_generation += 1,
                    "record" => rows[0].work_arc.record = None,
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        env.view.reload_storage_only().unwrap();
        assert_eq!(
            env.view.get_instance(&id).unwrap().work_arc_status,
            None,
            "{change}"
        );
    }
}
