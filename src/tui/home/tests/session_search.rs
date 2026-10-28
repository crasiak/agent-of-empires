//! Session search overlay wiring: Ctrl-F, the palette entry, and the outcome actions.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::*;
use crate::session::ResumeIntent;
use crate::session_search::{parse, ResumeSpec, Session, Target};
use crate::tui::dialogs::SessionSearchDialog;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn id_of(view: &HomeView, title: &str) -> String {
    view.instances()
        .find(|i| i.title == title)
        .map(|i| i.id.clone())
        .unwrap()
}

/// Open the overlay over the fixture hits with `resolver`, so no test runs the real CLI.
fn open_stub(view: &mut HomeView, resolver: impl Fn(&Session) -> Target + Send + 'static) {
    view.session_search = Some(SessionSearchDialog::new(
        None,
        Box::new(|req| {
            let hits = parse(include_bytes!(
                "../../../session_search/testdata/query_v1.json"
            ))
            .unwrap();
            (req.generation, Ok(hits))
        }),
        Box::new(resolver),
    ));
}

/// Type a query and tick until its results land; the best hit is the fixture's `L1`.
fn load(view: &mut HomeView) {
    for c in "abc".chars() {
        view.handle_key(key(KeyCode::Char(c)), None);
    }
    let t = Instant::now() + Duration::from_secs(1);
    view.tick_session_search(t);
    view.tick_session_search(t + Duration::from_secs(1));
    let deadline = Instant::now() + Duration::from_secs(10);
    while view.session_search.as_ref().unwrap().busy() {
        assert!(Instant::now() < deadline, "query never answered");
        view.tick_session_search(t + Duration::from_secs(1));
        std::thread::yield_now();
    }
}

fn resume_spec(tool: &str, cwd: &str) -> ResumeSpec {
    ResumeSpec {
        tool: tool.into(),
        cwd: cwd.into(),
        session_id: "c-111".into(),
        aoe_profile: "test".into(),
        store: PathBuf::from("/nonexistent/store"),
        title: "fix the deploy".into(),
        when: "10-25".into(),
        transcript_path: None,
        command: "ledger run claude -- --resume c-111".into(),
    }
}

#[test]
#[serial]
fn ctrl_f_opens_the_overlay_which_owns_typing_until_esc() {
    let mut env = create_test_env_with_sessions(3);
    env.view.cursor = 0;
    env.view.update_selected();

    env.view.handle_key(ctrl('f'), None);
    assert!(env.view.session_search.is_some());

    env.view.handle_key(key(KeyCode::Char('j')), None);
    assert_eq!(env.view.cursor, 0, "j types into the query");
    assert!(env.view.session_search.is_some());

    env.view.handle_key(key(KeyCode::Esc), None);
    assert!(env.view.session_search.is_none());
}

#[test]
#[serial]
fn the_palette_lists_and_opens_search_transcripts() {
    let mut env = create_test_env_with_sessions(1);
    env.view.handle_key(ctrl('k'), None);
    for c in "search transcripts".chars() {
        env.view.handle_key(key(KeyCode::Char(c)), None);
    }
    env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.command_palette.is_none());
    assert!(env.view.session_search.is_some());
}

#[test]
#[serial]
fn enter_on_a_jump_selects_that_session_and_attaches() {
    let mut env = create_test_env_with_sessions(2);
    let target = id_of(&env.view, "session1");
    env.view.select_session_by_id(&id_of(&env.view, "session0"));
    let jump = target.clone();
    open_stub(&mut env.view, move |_| Target::Jump(jump.clone()));
    load(&mut env.view);

    let action = env.view.handle_key(key(KeyCode::Enter), None);
    assert!(env.view.session_search.is_none());
    assert_eq!(env.view.selected_session.as_deref(), Some(target.as_str()));
    assert_eq!(action, Some(Action::AttachSession(target)));
}

#[test]
#[serial]
fn enter_on_a_resume_adds_one_resuming_instance_and_selects_it() {
    let mut env = create_test_env_with_sessions(1);
    let cwd = TempDir::new().unwrap();
    let cwd_path = cwd.path().to_string_lossy().into_owned();
    open_stub(&mut env.view, move |_| {
        Target::Resume(resume_spec("claude", &cwd_path))
    });
    load(&mut env.view);
    let before = env.view.instances().len();

    let action = env.view.handle_key(key(KeyCode::Enter), None);
    assert_eq!(env.view.instances().len(), before + 1);
    let added = env
        .view
        .instances()
        .find(|i| i.title == "fix the deploy")
        .unwrap();
    assert_eq!(added.resume_intent, ResumeIntent::Use("c-111".into()));
    assert_eq!(
        env.view.selected_session.as_deref(),
        Some(added.id.as_str())
    );
    assert_eq!(action, Some(Action::AttachSession(added.id.clone())));
}

#[test]
#[serial]
fn a_resume_takes_the_name_and_group_of_the_pane_in_its_folder() {
    let mut env = create_test_env_with_sessions(1);
    let cwd = TempDir::new().unwrap();
    let cwd_path = cwd.path().to_string_lossy().into_owned();
    let pane = id_of(&env.view, "session0");
    let path = cwd_path.clone();
    env.view.mutate_instance(&pane, |i| {
        i.title = "sshca-last-mile".into();
        i.project_path = path;
        i.group_path = "sshca".into();
    });
    open_stub(&mut env.view, move |_| {
        Target::Resume(resume_spec("claude", &cwd_path))
    });
    load(&mut env.view);

    env.view.handle_key(key(KeyCode::Enter), None);
    let added = env
        .view
        .instances()
        .find(|i| i.title == "sshca-last-mile 10-25")
        .unwrap();
    assert_eq!(added.group_path, "sshca");
}

#[test]
#[serial]
fn a_resume_that_cannot_build_copies_its_command_instead() {
    let cwd = TempDir::new().unwrap();
    let cwd_path = cwd.path().to_string_lossy().into_owned();
    // Pi without its transcript cannot resume natively; a gone folder is not tried.
    for (tool, cwd, want) in [
        ("pi", cwd_path.as_str(), "resume failed: "),
        (
            "claude",
            "/nonexistent/session-search/repo",
            "folder is gone",
        ),
    ] {
        let mut env = create_test_env_with_sessions(1);
        let spec_cwd = cwd.to_string();
        open_stub(&mut env.view, move |_| {
            Target::Resume(resume_spec(tool, &spec_cwd))
        });
        load(&mut env.view);
        let before = env.view.instances().len();

        let action = env.view.handle_key(key(KeyCode::Enter), None);
        assert_eq!(env.view.instances().len(), before, "{tool}");
        match action {
            Some(Action::CopyToClipboard { text, status }) => {
                assert_eq!(text, "ledger run claude -- --resume c-111");
                assert!(status.starts_with(want), "{status}");
                assert!(status.ends_with("; command copied"), "{status}");
            }
            other => panic!("expected a copy, got {other:?}"),
        }
    }
}

#[test]
#[serial]
fn ctrl_y_copies_the_resume_command() {
    let mut env = create_test_env_with_sessions(1);
    open_stub(&mut env.view, |_| Target::Nothing);
    load(&mut env.view);

    let action = env.view.handle_key(ctrl('y'), None);
    assert!(env.view.session_search.is_none());
    assert_eq!(
        action,
        Some(Action::CopyToClipboard {
            text: "cd /home/u/code/attic-worktrees/deploy && ledger run claude --profile personal -- --resume c-111".into(),
            status: "resume command copied".into(),
        })
    );
}

#[test]
#[serial]
fn enter_with_no_way_back_says_so() {
    let mut env = create_test_env_with_sessions(1);
    open_stub(&mut env.view, |_| Target::Nothing);
    load(&mut env.view);

    let action = env.view.handle_key(key(KeyCode::Enter), None);
    assert_eq!(
        action,
        Some(Action::SetTransientStatus(
            "no way back for this session".into()
        ))
    );
}

#[test]
#[serial]
fn ctrl_o_opens_the_transcript() {
    let mut env = create_test_env_with_sessions(1);
    open_stub(&mut env.view, |_| Target::Nothing);
    load(&mut env.view);

    let action = env.view.handle_key(ctrl('o'), None);
    assert_eq!(
        action,
        Some(Action::EditFile(PathBuf::from(
            "/home/u/.ledger/homes/personal/claude-code/state/protected/projects/x/c-111.jsonl"
        )))
    );
}

#[test]
#[serial]
fn a_paste_lands_in_the_query_not_a_compose_dialog() {
    let mut env = create_test_env_with_sessions(1);
    env.view.handle_key(ctrl('f'), None);
    env.view.handle_paste("deploy");
    assert!(env.view.send_message_dialog.is_none());
    assert!(env.view.pending_paste.is_none());
    assert!(env.view.session_search.is_some());
}
