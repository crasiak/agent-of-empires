use super::*;
use crate::{
    session::afk::overview::{HistoricalAuthority, ProfileOverview, SessionOverview},
    tui::dialogs::afk_review::{Reply as ReviewReply, Request as ReviewRequest},
};
use serde_json::json;

fn select(profile: &str, session: &str, window: u128) -> Selection {
    Selection {
        profile: profile.into(),
        session_id: session.into(),
        window_id: uuid::Uuid::from_u128(window).to_string(),
    }
}
fn session(windows: Vec<Selection>) -> SessionOverview {
    SessionOverview {
        review_windows: windows,
        id: "shared-id".into(),
        title: "Workstream".into(),
        tool: "pi".into(),
        activity: "stopped".into(),
        archived: false,
        trashed: false,
        support: "stored only".into(),
        requested_presence: "off".into(),
        presence_expires_at_ms: None,
        presence_acknowledgement: "unobserved".into(),
        delegation: None,
        legacy_evidence: vec![],
        bulk_preference: "unrecorded".into(),
        exception_epoch: Some(0),
        batch: None,
        batch_membership: "outside original batch windows",
        usage: json!({"coverage":"not_collected"}),
        errors: vec![],
    }
}
fn overview(profile: &str, windows: Vec<Selection>) -> Overview {
    Overview {
        captured_at_ms: 1000,
        profiles: vec![ProfileOverview {
            review_batches: Default::default(),
            profile: review::terminal_text(profile),
            sessions: vec![session(windows)],
            batches: vec![],
            errors: vec![],
        }],
        historical_authority: vec![],
        migration_evidence: vec![],
        shared_budget: "No shared budget",
        batch_membership: "original windows",
        bulk_exclusions: "unrecorded",
        account_coverage: "unknown",
        observation: "stored only",
    }
}
fn harness() -> (
    AfkOverviewDialog,
    Receiver<mpsc::Sender<Reply>>,
    Receiver<(Vec<Selection>, mpsc::Sender<ReviewReply>)>,
) {
    let (send, reads) = mpsc::channel();
    let mut dialog = AfkOverviewDialog::with_dispatch(Box::new(move || {
        let (reply, result) = mpsc::channel();
        assert!(send.send(reply).is_ok());
        result
    }));
    let (send_review, reviews) = mpsc::channel();
    dialog.review_factory = Box::new(move |selections| {
        let send_review = send_review.clone();
        AfkReviewDialog::with_dispatch(
            selections,
            Box::new(move |request| {
                let (reply, result) = mpsc::channel();
                let ReviewRequest::Read(selections) = request else {
                    panic!("overview navigation must never dispatch a write")
                };
                assert!(send_review.send((selections, reply)).is_ok());
                result
            }),
        )
    });
    (dialog, reads, reviews)
}
fn deliver(
    dialog: &mut AfkOverviewDialog,
    reads: &Receiver<mpsc::Sender<Reply>>,
    snapshot: Overview,
) {
    assert!(reads.try_recv().unwrap().send(Ok(snapshot)).is_ok());
    assert!(dialog.tick());
}
fn press(dialog: &mut AfkOverviewDialog, code: KeyCode) -> DialogResult<()> {
    dialog.handle_key(KeyEvent::from(code))
}

#[test]
fn exact_marks_survive_refresh_and_nested_escape_without_resolving_display_identities() {
    let profile = "legacy\u{202e}\u{7f}";
    let original = select(profile, "shared-id", 1);
    let replacement = select(profile, "shared-id", 2);
    let (mut dialog, reads, reviews) = harness();
    deliver(
        &mut dialog,
        &reads,
        overview(profile, vec![original.clone()]),
    );
    press(&mut dialog, KeyCode::Char(' '));
    assert_eq!(dialog.marked, BTreeSet::from([original.clone()]));
    press(&mut dialog, KeyCode::Char('r'));
    deliver(
        &mut dialog,
        &reads,
        overview(profile, vec![replacement.clone()]),
    );
    assert_eq!(dialog.marked, BTreeSet::from([original.clone()]));
    press(&mut dialog, KeyCode::Char('v'));
    let (selected, reply) = reviews.try_recv().unwrap();
    assert_eq!(selected, vec![original]);
    assert!(reply
        .send(ReviewReply::Read(Ok(review::Report {
            version: 1,
            capture_started_at_ms: 1,
            capture_finished_at_ms: 2,
            complete: false,
            selections: vec![review::SelectedWindow {
                selection: selected[0].clone(),
                source_revision: Some(2),
                window: None,
                annotations: vec![],
                errors: vec!["Original window unavailable".into()]
            }],
            shared_usage: vec![],
            observation: "stored",
            privacy: "private"
        })))
        .is_ok());
    assert!(dialog.tick());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(130, 44)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area(), &Theme::default()))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("AFK return review"));
    assert!(text.contains("INCOMPLETE COLLECTION"));
    assert!(text.contains("Original window unavailable"));
    assert!(!text.contains(['\u{202e}', '\u{7f}']));
    assert_eq!(press(&mut dialog, KeyCode::Esc), DialogResult::Continue);
    assert!(dialog.review.is_none());
    press(&mut dialog, KeyCode::Char('x'));
    assert!(dialog.marked.is_empty());
    press(&mut dialog, KeyCode::Char('v'));
    assert_eq!(reviews.try_recv().unwrap().0, vec![replacement]);
    assert_eq!(press(&mut dialog, KeyCode::Esc), DialogResult::Continue);
    assert_eq!(press(&mut dialog, KeyCode::Esc), DialogResult::Cancel);
    assert!(matches!(reads.try_recv(), Err(mpsc::TryRecvError::Empty)));
    assert!(matches!(reviews.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn original_batch_and_orphan_windows_are_exact_and_overflow_never_partially_marks() {
    let a = select("alpha", "shared-id", 1);
    let old = select("beta", "orphan", 1);
    let newer = select("beta", "orphan", 2);
    let batch: Vec<_> = (10..26)
        .map(|id| select("alpha", "shared-id", id))
        .collect();
    let mut snapshot = overview("alpha", vec![a.clone()]);
    snapshot.historical_authority.push(HistoricalAuthority {
        review_windows: vec![old.clone(), newer.clone()],
        profile: "beta".into(),
        instance_id: "orphan".into(),
        observation: "Historical only",
        evidence: vec![],
        windows: vec![
            json!({"window":old.window_id}),
            json!({"window":newer.window_id}),
        ],
    });
    snapshot.profiles[0]
        .batches
        .push(json!({"operation_id":"operation","members":[],"shared_requests":16}));
    snapshot.profiles[0]
        .review_batches
        .insert("operation".into(), batch.clone());
    let (mut dialog, reads, reviews) = harness();
    deliver(&mut dialog, &reads, snapshot);
    assert_eq!(dialog.current_windows(), vec![newer.clone()]);
    press(&mut dialog, KeyCode::Down);
    assert_eq!(dialog.current_windows(), vec![old.clone()]);
    press(&mut dialog, KeyCode::Char(' '));
    press(&mut dialog, KeyCode::Down);
    assert_eq!(dialog.current_windows(), batch);
    let before = dialog.marked.clone();
    press(&mut dialog, KeyCode::Char(' '));
    assert_eq!(
        dialog.marked, before,
        "17-window union must be refused atomically"
    );
    assert!(dialog.notice.as_ref().unwrap().contains("no marks changed"));
    press(&mut dialog, KeyCode::Char('x'));
    press(&mut dialog, KeyCode::Char(' '));
    assert_eq!(dialog.marked.len(), 16);
    press(&mut dialog, KeyCode::Char('v'));
    let (selections, _) = reviews.try_recv().unwrap();
    assert_eq!(
        selections.into_iter().collect::<BTreeSet<_>>(),
        batch.into_iter().collect::<BTreeSet<_>>()
    );
    press(&mut dialog, KeyCode::Esc);
    press(&mut dialog, KeyCode::Char(' '));
    assert!(
        dialog.marked.is_empty(),
        "batch mark toggles the whole original set"
    );
    dialog.selection.select(Some(1));
    press(&mut dialog, KeyCode::Char(' '));
    dialog.selection.select(Some(3));
    press(&mut dialog, KeyCode::Char(' '));
    press(&mut dialog, KeyCode::Char('v'));
    let (selections, _) = reviews.try_recv().unwrap();
    assert_eq!(
        selections.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([a, old])
    );
}
