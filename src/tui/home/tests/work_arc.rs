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

#[test]
#[serial]
fn work_arc_pause_and_resume_retain_document_history_and_budget() {
    let home = test_home();
    let mut fixture = Fixture::new(home.0.path());
    let bound = arc::read_arc_source(&fixture.session).unwrap();
    let identity = bound.identity();
    let at = chrono::Utc::now();
    let nonce = fixture.session.work_arc.reserve(&identity, at).unwrap();
    fixture.session.work_arc.finish(&nonce, Some(arc::StoredArc {
        identity, at,
        arc: arc::WorkArc::from_json(r#"{"goal":"Stable goal","now":"Reported progress","remaining":"Checks","next":"Review"}"#).unwrap(),
        selected_route: "fake/route".into(), provider: "fake".into(), model: "synthetic".into(), usage: None,
    }), None);
    let retained = fixture.session.work_arc.record.clone();
    let id = fixture.session.id.clone();
    let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
    env.view.selected_session = Some(id.clone());
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ToggleWorkArc);
    let storage = Storage::new_unwatched("test").unwrap();
    let paused = storage.load().unwrap().remove(0);
    assert!(!paused.work_arc_enabled);
    assert_eq!(paused.work_arc.record, retained, "pause is not erasure");
    assert_eq!(arc::projection(&paused).label(), "paused");
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ToggleWorkArc);
    env.view.dispatch_confirm_submit("enable_work_arc");
    let mut resumed = storage.load().unwrap().remove(0);
    assert!(resumed.work_arc_enabled);
    assert_eq!(resumed.work_arc.record, retained);
    let mut later = retained.as_ref().unwrap().identity.clone();
    later.leaf = "later".into();
    assert!(
        resumed
            .work_arc
            .reserve(&later, at + chrono::Duration::minutes(1))
            .is_none(),
        "resume cannot reset cooldown"
    );
}

use crate::session::work_arc::{self as arc, test_support::Fixture, ArcStatus};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

type FakeRequest = (tokio::io::BufReader<tokio::net::UnixStream>, Value);

struct FakeRoute {
    available: std::sync::Arc<std::sync::atomic::AtomicBool>,
    generations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    reads: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
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
    let reads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_reads = reads.clone();
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            let mut line = String::new();
            let count = stream.read_line(&mut line).await.unwrap();
            observed_reads
                .lock()
                .unwrap()
                .push(line.as_bytes().to_vec());
            if count == 0 {
                continue;
            }
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
            reads,
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
            let record = row.work_arc.record.clone();
            let document = row.work_arc.document.clone();
            row.work_arc = serde_json::from_value(value).unwrap();
            row.work_arc.record = record;
            row.work_arc.document = document;
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
async fn slice1_session_arc_preview_two_sessions_handoff_pause_restart_and_history() {
    let home = test_home();
    let mut first = Fixture::new(home.0.path());
    first.session.title = "Arc Alpha".into();
    let second_root = home.0.path().join("second");
    std::fs::create_dir(&second_root).unwrap();
    let mut second = Fixture::new_with_ids(
        &second_root,
        "33333333-3333-4333-8333-333333333333",
        "44444444-4444-4444-8444-444444444444",
    );
    second.session.title = "Arc Beta".into();
    let (first_route, mut first_requests) = fake_route(&first);
    let (second_route, mut second_requests) = fake_route(&second);
    let first_server = tokio::spawn(async move {
        for progress in ["Alpha initial progress", "Alpha successor progress"] {
            let (mut stream, request) = first_requests.recv().await.unwrap();
            let reply = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/local","provider":"synthetic","model":"no-inference",
                "usage":{"input":12,"output":5},"text":json!({"goal":"Alpha stable goal","now":progress,"remaining":"Alpha checks","next":"Alpha review"}).to_string()});
            stream
                .get_mut()
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let second_server = tokio::spawn(async move {
        let (mut stream, request) = second_requests.recv().await.unwrap();
        let reply = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/local","provider":"synthetic","model":"no-inference",
            "usage":{"input":9,"output":3},"text":r#"{"goal":"Beta independent goal","now":"Beta progress","remaining":"Beta checks","next":"Beta review"}"#});
        stream
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    });
    let mut env = seeded_env(home, &[first.session.clone(), second.session.clone()], true);
    let first_id = first.session.id.clone();
    let second_id = second.session.id.clone();
    let select = |view: &mut HomeView, id: &str| {
        view.cursor = view
            .flat_items
            .iter()
            .position(|item| matches!(item, Item::Session { id: row_id, .. } if row_id == id))
            .unwrap();
        view.update_selected();
    };
    select(&mut env.view, &first_id);
    complete_refresh(&mut env).await;
    let initial = env
        .view
        .get_instance(&first_id)
        .unwrap()
        .work_arc
        .document
        .clone()
        .unwrap();
    let screen = render_home_to_string(&mut env.view, 160, 46);
    assert!(
        screen.contains("Alpha stable goal") && !screen.contains("Beta independent goal"),
        "{screen}"
    );
    println!("SLICE1 PREVIEW 1: Alpha current\n{screen}");
    select(&mut env.view, &second_id);
    complete_refresh(&mut env).await;
    let beta = env
        .view
        .get_instance(&second_id)
        .unwrap()
        .work_arc
        .document
        .clone()
        .unwrap();
    let screen = render_home_to_string(&mut env.view, 160, 46);
    assert!(
        screen.contains("Beta independent goal") && !screen.contains("Alpha stable goal"),
        "{screen}"
    );
    println!("SLICE1 PREVIEW 2: Beta independent\n{screen}");
    first.handoff("55555555-5555-4555-8555-555555555555");
    let storage = Storage::new_unwatched("test").unwrap();
    storage
        .update(|rows, _| {
            rows.iter_mut()
                .find(|row| row.id == first_id)
                .unwrap()
                .set_agent_conversation(
                    first.session.agent_session_id.clone(),
                    first.session.agent_session_binding.clone(),
                    None,
                );
            Ok(())
        })
        .unwrap();
    env.view.reload_storage_only().unwrap();
    select(&mut env.view, &first_id);
    assert_eq!(
        env.view
            .get_instance(&first_id)
            .unwrap()
            .work_arc
            .document
            .as_ref(),
        Some(&initial)
    );
    let screen = render_home_to_string(&mut env.view, 160, 46);
    assert!(
        screen.contains("carried forward") && screen.contains("Alpha stable goal"),
        "{screen}"
    );
    println!("SLICE1 PREVIEW 3: handoff retains field times and provenance\n{screen}");
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ToggleWorkArc);
    assert!(!env.view.get_instance(&first_id).unwrap().work_arc_enabled);
    assert!(!env.view.tick_work_arc());
    env.view = test_view(Some("test"));
    select(&mut env.view, &first_id);
    let screen = render_home_to_string(&mut env.view, 160, 46);
    assert!(
        screen.contains("paused") && screen.contains("Alpha stable goal"),
        "{screen}"
    );
    assert_eq!(
        env.view
            .get_instance(&first_id)
            .unwrap()
            .work_arc
            .document
            .as_ref(),
        Some(&initial)
    );
    println!("SLICE1 PREVIEW 4: paused document survives fresh HomeView\n{screen}");
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ExpandWorkArc);
    let expanded = render_home_to_string(&mut env.view, 160, 46);
    assert!(
        expanded.contains("Observed:")
            && expanded.contains("Changed:")
            && expanded.contains("carried forward"),
        "{expanded}"
    );
    println!("SLICE1 PREVIEW 5: per-field local datetimes\n{expanded}");
    env.view.handle_key(key(KeyCode::Esc), None);
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ToggleWorkArc);
    env.view.handle_key(key(KeyCode::Left), None);
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.confirm_dialog.is_none());
    assert!(env.view.get_instance(&first_id).unwrap().work_arc_enabled);
    complete_refresh(&mut env).await;
    assert_eq!(
        first_route
            .generations
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "resume cannot reset durable limits"
    );
    assert_eq!(
        env.view
            .get_instance(&first_id)
            .unwrap()
            .work_arc
            .document
            .as_ref(),
        Some(&initial)
    );
    allow_refresh(&mut env, &first_id);
    complete_refresh(&mut env).await;
    let row = env.view.get_instance(&first_id).unwrap();
    let latest = row.work_arc.document.as_ref().unwrap();
    assert_eq!(
        latest.fields[0], initial.fields[0],
        "copied Goal is not a fresh observation"
    );
    assert_eq!(
        latest.fields[1].source.pi_id,
        first.session.agent_session_id.as_deref().unwrap()
    );
    assert_eq!(storage.work_arc_history(row, 8).unwrap().len(), 2);
    assert_eq!(
        env.view
            .get_instance(&second_id)
            .unwrap()
            .work_arc
            .document
            .as_ref(),
        Some(&beta)
    );
    let latest_revision = latest.revision.clone();
    let screen = render_home_to_string(&mut env.view, 160, 46);
    assert!(screen.contains("Alpha successor progress"), "{screen}");
    println!("SLICE1 PREVIEW 6: successor updates Alpha, Beta unchanged\n{screen}");
    env.view
        .dispatch_context_menu_action(ContextMenuAction::ExpandWorkArc);
    env.view.handle_key(key(KeyCode::End), None);
    let screen = render_home_to_string(&mut env.view, 160, 46);
    assert!(
        screen.contains("Recent history")
            && screen.contains("Alpha initial progress")
            && screen.contains("Alpha successor progress"),
        "{screen}"
    );
    println!("SLICE1 PREVIEW 7: retained audit history\n{screen}");
    println!("SLICE1 AUDIT: initial={} latest={} history=2; Beta revision={} history=1; fake generations Alpha=2 Beta={}; no provider inference", initial.revision, latest_revision, beta.revision, second_route.generations.load(std::sync::atomic::Ordering::SeqCst));
    first_server.await.unwrap();
    second_server.await.unwrap();
}

fn measured_synthetic_arc(
    row: &mut Instance,
    identity: arc::ArcIdentity,
    at: chrono::DateTime<chrono::Utc>,
    usage: Value,
) {
    let nonce = row.work_arc.reserve(&identity, at).unwrap();
    assert!(row.work_arc.finish(
        &nonce,
        Some(arc::StoredArc {
            identity,
            at,
            arc: arc::WorkArc::from_json(
                r#"{"goal":"G","now":"Synthetic cue","remaining":"Unknown","next":"Review"}"#
            )
            .unwrap(),
            selected_route: "fake/route".into(),
            provider: "fake".into(),
            model: "physical".into(),
            usage: Some(usage),
        }),
        None
    ));
}

#[tokio::test]
#[serial]
async fn queued_projection_cannot_rollback_canonical_peer_state() {
    for update_kind in ["current", "loading-final", "status-only"] {
        for pending in [false, true] {
            let home = test_home();
            let fixture = Fixture::new(home.0.path());
            let (route, mut requests) = fake_route(&fixture);
            let id = fixture.session.id.clone();
            let mut session = fixture.session.clone();
            measured_synthetic_arc(
                &mut session,
                arc::read_arc_source(&fixture.session).unwrap().identity(),
                chrono::Utc::now() - chrono::Duration::minutes(6),
                json!({"input":11}),
            );
            let mut env = seeded_env(home, &[session], true);
            env.view.selected_session = Some(id.clone());
            if update_kind == "loading-final" {
                fixture.append("requested", "initial", "Synthetic requested work");
            } else if update_kind == "status-only" {
                route
                    .available
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
            env.view.tick_work_arc();
            if update_kind == "loading-final" {
                let (mut stream, request) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
                        .await
                        .unwrap()
                        .unwrap();
                let reply = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/route","provider":"fake","model":"physical",
                    "usage":{"input":21},"text":r#"{"goal":"G","now":"Delayed reply","remaining":"Unknown","next":"Review"}"#});
                stream
                    .get_mut()
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            let task = env.view.work_arc_refresh.task.take().unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
            fixture.append("peer-B", "initial", "Synthetic peer progress");
            let peer_identity = arc::read_arc_source(&fixture.session).unwrap().identity();
            let storage = Storage::new_unwatched("test").unwrap();
            storage
                .update(|rows, _| {
                    let row = rows.iter_mut().find(|row| row.id == id).unwrap();
                    let at = chrono::Utc::now() + chrono::Duration::minutes(6);
                    measured_synthetic_arc(
                        row,
                        peer_identity.clone(),
                        at,
                        json!({"input":77,"output":9}),
                    );
                    if pending {
                        let mut later = peer_identity.clone();
                        later.leaf = "peer-C-pending".into();
                        row.work_arc
                            .reserve(&later, at + chrono::Duration::minutes(6))
                            .unwrap();
                    }
                    Ok(())
                })
                .unwrap();
            env.view.reload_storage_only().unwrap();
            env.view.selected_session = Some(id.clone());
            let canonical = env.view.get_instance(&id).unwrap().clone();
            assert_eq!(
                canonical.work_arc.record.as_ref().unwrap().identity.leaf,
                "peer-B"
            );
            assert_eq!(canonical.work_arc_status, None);
            env.view.tick_work_arc();
            let row = env.view.get_instance(&id).unwrap();
            assert_eq!(
                row.work_arc, canonical.work_arc,
                "{update_kind}, pending={pending}: no rollback of record, budget, pending or usage"
            );
            assert_eq!(
                row.work_arc_status, None,
                "stale queued status cannot replace canonical state"
            );
            assert_eq!(arc::projection(row), ArcStatus::Stale);
            assert!(env.view.work_arc_refresh.task.is_none());
        }
    }
}

#[tokio::test]
#[serial]
async fn identical_canonical_reload_still_accepts_useful_updates() {
    for transition in ["current", "loading", "final"] {
        let home = test_home();
        let fixture = Fixture::new(home.0.path());
        let (_route, mut requests) = fake_route(&fixture);
        let id = fixture.session.id.clone();
        let mut session = fixture.session.clone();
        measured_synthetic_arc(
            &mut session,
            arc::read_arc_source(&fixture.session).unwrap().identity(),
            chrono::Utc::now() - chrono::Duration::minutes(6),
            json!({"input":11}),
        );
        let mut env = seeded_env(home, &[session], true);
        env.view.selected_session = Some(id.clone());
        if transition != "current" {
            fixture.append("requested", "initial", "Synthetic follow-up");
        }
        env.view.tick_work_arc();
        if transition != "current" {
            let (mut stream, request) =
                tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
                    .await
                    .unwrap()
                    .unwrap();
            if transition == "loading" {
                env.view.reload_storage_only().unwrap();
                env.view.selected_session = Some(id.clone());
                assert!(env.view.tick_work_arc());
                assert_eq!(
                    arc::projection(env.view.get_instance(&id).unwrap()),
                    ArcStatus::Loading
                );
            }
            let reply = json!({"ok":true,"nonce":request["nonce"],"selected_route":"fake/route","provider":"fake","model":"physical",
                "usage":{"input":21},"text":r#"{"goal":"G","now":"Completed","remaining":"Unknown","next":"Review"}"#});
            stream
                .get_mut()
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
        let task = env.view.work_arc_refresh.task.take().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        env.view.reload_storage_only().unwrap();
        env.view.selected_session = Some(id.clone());
        let canonical = env.view.get_instance(&id).unwrap().work_arc.clone();
        assert!(env.view.tick_work_arc(), "{transition}");
        let row = env.view.get_instance(&id).unwrap();
        assert_eq!(row.work_arc, canonical, "{transition}");
        assert_eq!(arc::projection(row), ArcStatus::Current, "{transition}");
    }
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn ui_invalidation_cancels_write_without_polling_the_refresh_future() {
    let home = test_home();
    let fixture = Fixture::new(home.0.path());
    let (mut route, _requests) = fake_route(&fixture);
    let id = fixture.session.id.clone();
    let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
    env.view.selected_session = Some(id.clone());
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let (finished, done) = std::sync::mpsc::channel();
    *arc::test_support::WRITE_GATE.lock().unwrap() = Some((id.clone(), entered, gate, finished));
    env.view.tick_work_arc();
    tokio::time::timeout(std::time::Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    let reads_before = route.reads.lock().unwrap().len();
    assert_eq!(
        route.generations.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    env.view.selected_session = None;
    env.view.invalidate_work_arc_selection();
    release.send(()).unwrap();
    // This blocks the sole runtime thread: the outer refresh cannot poll cancellation first.
    done.recv_timeout(std::time::Duration::from_secs(2))
        .expect("send worker returned while outer future stayed unpolled");
    let task = env.view.work_arc_refresh.task.take().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while route.reads.lock().unwrap().len() == reads_before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("server observed the generation connection reach data or EOF");
    let reads = route.reads.lock().unwrap()[reads_before..].to_vec();
    let generations = route.generations.load(std::sync::atomic::Ordering::SeqCst);
    route.task.abort();
    assert!((&mut route.task).await.unwrap_err().is_cancelled());
    let row = Storage::new_unwatched("test")
        .unwrap()
        .load()
        .unwrap()
        .remove(0);
    let state = serde_json::to_value(&row.work_arc).unwrap();
    assert!(row.work_arc_enabled);
    assert_eq!(
        state["attempts"].as_array().unwrap().len(),
        1,
        "cancellation cannot refund a reserved attempt"
    );
    assert_eq!(row.work_arc.cost, arc::ArcCost::Unknown);
    assert!(row.work_arc.record.is_none());
    assert!(
        reads.iter().all(Vec::is_empty),
        "canceled send transmitted {} bytes before the outer future polled",
        reads.iter().map(Vec::len).sum::<usize>()
    );
    assert_eq!(generations, 0);
}

#[tokio::test]
#[serial]
async fn canonical_revocation_or_source_change_before_send_transmits_no_excerpt() {
    use std::sync::atomic::Ordering;
    for change in ["pause", "conversation", "binding", "epoch", "owner", "leaf"] {
        let home = test_home();
        let fixture = Fixture::new(home.0.path());
        let (route, _requests) = fake_route(&fixture);
        let id = fixture.session.id.clone();
        let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
        env.view.selected_session = Some(id.clone());
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        *crate::tui::home::work_arc::SEND_HOOK.lock().unwrap() = Some((id.clone(), entered, wait));
        env.view.tick_work_arc();
        tokio::time::timeout(std::time::Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(route.generations.load(Ordering::SeqCst), 0);
        if change == "leaf" {
            fixture.append("new-leaf", "initial", "New synthetic progress");
        } else {
            Storage::new_unwatched("test")
                .unwrap()
                .update(|rows, _| {
                    match change {
                        "pause" => {
                            rows[0].work_arc_enabled = false;
                            rows[0].work_arc.invalidate();
                        }
                        "conversation" => {
                            rows[0].set_agent_conversation(Some("successor".into()), None, None)
                        }
                        "binding" => {
                            rows[0].agent_session_binding.as_mut().unwrap().session_id =
                                "foreign".into()
                        }
                        "epoch" => rows[0].work_arc.invalidate(),
                        "owner" => {
                            let mut peer = rows[0].clone();
                            peer.id = Instance::new("other", "/tmp/other").id;
                            rows.push(peer);
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                })
                .unwrap();
        }
        release.send(()).unwrap();
        let mut task = env.view.work_arc_refresh.task.take().unwrap();
        let settled = tokio::time::timeout(std::time::Duration::from_secs(5), &mut task).await;
        if settled.is_err() {
            task.abort();
            let _ = task.await;
        }
        assert!(
            matches!(settled, Ok(Ok(()))),
            "{change}: guarded attempt must settle without a model reply"
        );
        assert_eq!(
            route.generations.load(Ordering::SeqCst),
            0,
            "{change}: no excerpt crosses the canonical guard"
        );
    }
}

#[tokio::test]
#[serial]
async fn corrupt_possible_owner_blocks_transcript_before_route_or_generation() {
    use std::sync::atomic::Ordering;
    let home = test_home();
    let fixture = Fixture::new(home.0.path());
    let (route, _requests) = fake_route(&fixture);
    let id = fixture.session.id.clone();
    let mut env = seeded_env(home, std::slice::from_ref(&fixture.session), true);
    env.view.selected_session = Some(id);
    let storage = Storage::new_unwatched("test").unwrap();
    let path = storage.sessions_path();
    let mut rows: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    rows.push(json!({"id":fixture.session.id,"agent_session_id":fixture.session.agent_session_id,"work_arc":"damaged"}));
    let raw = serde_json::to_vec(&rows).unwrap();
    std::fs::write(path, &raw).unwrap();
    *arc::test_support::SOURCE_READS.lock().unwrap() = Some((fixture.transcript.clone(), 0));
    complete_refresh(&mut env).await;
    let (_, reads) = arc::test_support::SOURCE_READS
        .lock()
        .unwrap()
        .take()
        .unwrap();
    assert_eq!(
        reads, 0,
        "unknown ownership blocks before opening the transcript"
    );
    assert_eq!(route.generations.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(path).unwrap(),
        raw,
        "guard does not repair or prune the store"
    );
    assert!(path.with_file_name("sessions.corrupt.jsonl").exists());
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
        let storage = Storage::new_unwatched("test").unwrap();
        let metadata = std::fs::metadata(storage.sessions_path()).unwrap();
        let before = std::fs::read(storage.sessions_path()).unwrap();
        complete_refresh(&mut env).await;
        assert_eq!(
            arc::projection(env.view.get_instance(&id).unwrap()),
            ArcStatus::Current,
            "same leaf doesn't request model again"
        );
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(storage.sessions_path()).unwrap().ino(),
            metadata.ino(),
            "unchanged validation does not replace the store"
        );
        assert_eq!(std::fs::read(storage.sessions_path()).unwrap(), before);
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
        ArcStatus::Off
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
                    "record" => {
                        rows[0].work_arc.record = None;
                        rows[0].work_arc.document = None;
                        rows[0].work_arc.revision = None;
                    }
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
