use crate::{
    session::tailored::{
        workshop::{CreateEnvelope, Operation, Request, State, Store},
        CaptureAccount,
    },
    tui::styles::Theme,
};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::{TextArea, WrapMode};
use std::sync::mpsc;

const LABELS: [&str; 13] = [
    "Original skill idea",
    "Canonical absolute source repository",
    "Skill slug",
    "Explicit account/source suitability",
    "Refinement message",
    "Proposed bundle JSON (Ctrl+Alt+B saves)",
    "Canonical GitHub HTTPS remote",
    "GitHub repository ID",
    "Destination base branch",
    "PR title",
    "PR body",
    "Explicit Ledger source name",
    "Explicit targets JSON: profile/harness/account/sessions",
];
struct TaskError {
    message: String,
    uncertain: bool,
}
pub(super) struct WorkshopWindow {
    pub(super) id: String,
    pub(super) new_draft: bool,
    request: Option<Request>,
    account: CaptureAccount,
    fields: Vec<TextArea<'static>>,
    focus: usize,
    pending: Option<mpsc::Receiver<Result<Request, TaskError>>>,
    create: Option<Operation>,
    uncertain: bool,
    error: Option<String>,
    show_review: bool,
    scroll: u16,
    workflow: bool,
    disclosure: bool,
    current_rules: bool,
    custody: bool,
}
impl WorkshopWindow {
    pub fn new(account: CaptureAccount, request: Option<Request>) -> Self {
        let mut window = Self {
            new_draft: request.is_none(),
            id: request
                .as_ref()
                .map_or_else(|| uuid::Uuid::new_v4().to_string(), |d| d.id.clone()),
            request: None,
            account,
            fields: (0..13)
                .map(|_| {
                    let mut field = TextArea::default();
                    field.set_wrap_mode(WrapMode::WordOrGlyph);
                    field
                })
                .collect(),
            focus: 0,
            pending: None,
            create: None,
            uncertain: false,
            error: None,
            show_review: false,
            scroll: 0,
            workflow: false,
            disclosure: false,
            current_rules: false,
            custody: false,
        };
        if let Some(request) = request {
            window.accept(request);
        } else if let Ok(config) = crate::session::config::Config::load() {
            if let Some(repo) = config.skills.workshop_repository {
                window.fields[1].insert_str(repo);
            }
        }
        window
    }
    pub fn load(account: CaptureAccount, id: String) -> Self {
        let mut window = Self::new(account, None);
        window.id = id.clone();
        window.new_draft = false;
        window.start(move || Store::open()?.get(&id));
        window
    }
    fn accept(&mut self, request: Request) {
        self.new_draft = false;
        self.account = request.envelope.account;
        for (index, value) in [
            &request.envelope.idea,
            &request.envelope.repository.display().to_string(),
            &request.envelope.slug,
            &request.envelope.source_scope,
        ]
        .into_iter()
        .enumerate()
        {
            self.fields[index] = TextArea::from(value.split('\n'));
            self.fields[index].set_wrap_mode(WrapMode::WordOrGlyph);
        }
        self.fields[5] = TextArea::from(
            serde_json::to_string_pretty(&request.bundle.clone().unwrap_or(
                crate::session::tailored::workshop::Bundle {
                    files: Default::default(),
                },
            ))
            .expect("bundle")
            .lines(),
        );
        self.fields[5].set_wrap_mode(WrapMode::WordOrGlyph);
        self.disclosure = false;
        self.current_rules = false;
        self.custody = false;
        if matches!(
            request.state,
            State::PublishPreview
                | State::LandPreview
                | State::ImportPreview
                | State::CandidatePreview
                | State::RolloutPreview
        ) {
            self.show_review = true;
        }
        self.error = request.error.clone();
        self.request = Some(request);
        self.uncertain = false;
        self.create = None;
    }
    fn value(&self, index: usize) -> String {
        self.fields[index].lines().join("\n")
    }
    fn editable(&self) -> bool {
        !self.uncertain
            && self.pending.is_none()
            && self
                .request
                .as_ref()
                .is_none_or(|r| matches!(r.state, State::Draft | State::Review | State::Failed))
    }
    fn dirty(&self) -> bool {
        self.request.as_ref().is_some_and(|r| {
            self.value(5)
                != serde_json::to_string_pretty(&r.bundle.clone().unwrap_or(
                    crate::session::tailored::workshop::Bundle {
                        files: Default::default(),
                    },
                ))
                .expect("bundle")
        })
    }
    fn start(&mut self, task: impl FnOnce() -> Result<Request> + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let _ = tx.send(task().map_err(|e| {
                TaskError {
                    uncertain: e
                        .downcast_ref::<crate::session::tailored::workshop::EffectOutcomeUncertain>(
                        )
                        .is_some(),
                    message: format!("{e:#}"),
                }
            }));
        });
    }
    fn operate(&mut self, operation: Operation) {
        self.start(move || Store::open()?.operate(operation));
    }
    pub(super) fn account(&self) -> CaptureAccount {
        self.account
    }
    pub(super) fn can_switch_account(&self) -> bool {
        self.request.is_none() && self.create.is_none() && self.pending.is_none()
    }
    pub(super) fn repeat(&mut self, key: KeyEvent) {
        if !self.show_review
            && self.pending.is_none()
            && !self.uncertain
            && (self.workflow || self.editable())
            && (self.request.is_none() && self.create.is_none() || self.focus >= 4)
            && !(key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.fields[self.focus].input(key);
        }
    }
    pub fn key(&mut self, key: KeyEvent) {
        if self.pending.is_some() {
            return;
        }
        let revision = self.request.as_ref().map_or(0, |r| r.revision);
        match key.code {
            KeyCode::F(8) => {
                self.workflow = !self.workflow;
                self.show_review = false;
                self.focus = if self.workflow { 6 } else { 0 };
                return;
            }
            KeyCode::F(4)
                if self
                    .request
                    .as_ref()
                    .is_some_and(|r| r.state == State::PublishPreview) =>
            {
                self.disclosure = !self.disclosure;
                return;
            }
            KeyCode::F(5)
                if self
                    .request
                    .as_ref()
                    .is_some_and(|r| r.state == State::LandPreview) =>
            {
                self.current_rules = !self.current_rules;
                return;
            }
            KeyCode::F(6)
                if self
                    .request
                    .as_ref()
                    .is_some_and(|r| r.state == State::LandPreview) =>
            {
                self.custody = !self.custody;
                return;
            }
            KeyCode::F(10) => {
                let id = self.id.clone();
                self.start(move || Store::open()?.get(&id));
                return;
            }
            KeyCode::F(11) => {
                self.show_review = !self.show_review;
                return;
            }
            KeyCode::F(9) => {
                let state = self.request.as_ref().map(|r| r.state);
                let operation = match state {
                    Some(
                        State::Publishing
                        | State::PublishUnknown
                        | State::Landing
                        | State::LandPending
                        | State::LandUnknown,
                    ) => Operation::ReconcilePublication {
                        id: self.id.clone(),
                        revision,
                    },
                    Some(
                        State::Importing
                        | State::Applying
                        | State::Materializing
                        | State::RollingOut
                        | State::DeployUnknown,
                    ) => Operation::ReconcileDeploy {
                        id: self.id.clone(),
                        revision,
                    },
                    _ => Operation::Reconcile {
                        id: self.id.clone(),
                        revision,
                    },
                };
                self.operate(operation);
                return;
            }
            KeyCode::F(12)
                if self
                    .request
                    .as_ref()
                    .is_some_and(|r| r.state == State::Refining) =>
            {
                self.operate(Operation::RecoverRefinement {
                    id: self.id.clone(),
                    revision,
                });
                return;
            }
            _ => {}
        }
        if self.workflow && key.modifiers == (KeyModifiers::CONTROL | KeyModifiers::ALT) {
            if !self.uncertain {
                if let Some(operation) = self.workflow_operation(key.code) {
                    self.operate(operation);
                }
            }
            return;
        }
        if self.show_review && key.modifiers != (KeyModifiers::CONTROL | KeyModifiers::ALT) {
            match key.code {
                KeyCode::Up | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
                KeyCode::Down | KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
                _ => {}
            }
            return;
        }
        if key.modifiers == (KeyModifiers::CONTROL | KeyModifiers::ALT) {
            if self.uncertain
                && !(key.code == KeyCode::Char('s')
                    && self.request.is_none()
                    && self.create.is_some())
            {
                return;
            }
            if self.workflow {
                return;
            }
            let op = match key.code {
                KeyCode::Char('s') if self.request.is_none() => {
                    let op = self.create.clone().unwrap_or_else(|| Operation::Create {
                        id: self.id.clone(),
                        envelope: CreateEnvelope {
                            account: self.account,
                            idea: self.value(0),
                            repository: self.value(1).into(),
                            slug: self.value(2),
                            source_scope: self.value(3),
                        },
                    });
                    self.create = Some(op.clone());
                    Some(op)
                }
                KeyCode::Char('d')
                    if self.editable() && self.request.is_some() && !self.dirty() =>
                {
                    Some(Operation::Refine {
                        id: self.id.clone(),
                        revision,
                        message: self.value(4),
                    })
                }
                KeyCode::Char('b') if self.editable() && self.request.is_some() => {
                    match serde_json::from_str(&self.value(5)) {
                        Ok(bundle) => Some(Operation::Propose {
                            id: self.id.clone(),
                            revision,
                            bundle,
                        }),
                        Err(error) => {
                            self.error = Some(error.to_string());
                            None
                        }
                    }
                }
                KeyCode::Char('r')
                    if self.editable() && self.request.is_some() && !self.dirty() =>
                {
                    self.show_review = true;
                    Some(Operation::Review {
                        id: self.id.clone(),
                        revision,
                    })
                }
                KeyCode::Char('k') if self.editable() && !self.dirty() => self
                    .request
                    .as_ref()
                    .filter(|r| r.state == State::Review)
                    .and_then(|r| r.review.as_ref())
                    .map(|r| Operation::Commit {
                        id: self.id.clone(),
                        revision,
                        review_digest: r.digest.clone(),
                    }),
                _ => None,
            };
            if let Some(op) = op {
                self.operate(op);
            }
            return;
        }
        match key.code {
            KeyCode::Tab => {
                self.focus = if self.workflow {
                    6 + (self.focus - 6 + 1) % 7
                } else {
                    (self.focus + 1) % 6
                }
            }
            KeyCode::BackTab => {
                self.focus = if self.workflow {
                    6 + (self.focus - 6 + 6) % 7
                } else {
                    (self.focus + 5) % 6
                }
            }
            KeyCode::F(6) if self.request.is_none() && self.create.is_none() => {
                self.account = if self.account == CaptureAccount::Personal {
                    CaptureAccount::Work
                } else {
                    CaptureAccount::Personal
                }
            }
            _ if (self.workflow && !self.uncertain || self.editable())
                && (self.request.is_none() && self.create.is_none() || self.focus >= 4) =>
            {
                self.fields[self.focus].input(key);
            }
            _ => {}
        }
    }
    pub fn paste(&mut self, text: &str) {
        if (self.workflow && !self.uncertain && self.pending.is_none() || self.editable())
            && (self.request.is_none() && self.create.is_none() || self.focus >= 4)
        {
            self.fields[self.focus].insert_str(text);
        }
    }
    fn workflow_operation(&mut self, code: KeyCode) -> Option<Operation> {
        let r = self.request.as_ref()?;
        let id = self.id.clone();
        let revision = r.revision;
        let p = r.publication.as_ref();
        let d = r.deployment.as_ref();
        match code {
            KeyCode::Char('p') if matches!(r.state, State::Committed | State::PublishPreview) => {
                match self.value(7).parse() {
                    Ok(repository_id) => Some(Operation::PreviewPublish {
                        id,
                        revision,
                        remote: self.value(6),
                        repository_id,
                        base_branch: self.value(8),
                        title: self.value(9),
                        body: self.value(10),
                    }),
                    Err(_) => {
                        self.error = Some("Enter the actual numeric GitHub repository ID".into());
                        None
                    }
                }
            }
            KeyCode::Char('u') if r.state == State::PublishPreview && self.disclosure => {
                Some(Operation::Publish {
                    id,
                    revision,
                    preview_digest: p?.preview.as_ref()?.digest.clone(),
                    disclosure_confirmed: true,
                })
            }
            KeyCode::Char('u') if r.state == State::PublishReady => {
                Some(Operation::ContinuePublish { id, revision })
            }
            KeyCode::Char('l') if matches!(r.state, State::Published | State::LandPreview) => {
                Some(Operation::PreviewLand { id, revision })
            }
            KeyCode::Char('g')
                if r.state == State::LandPreview && self.current_rules && self.custody =>
            {
                Some(Operation::Land {
                    id,
                    revision,
                    policy_digest: p?.policy.as_ref()?.digest.clone(),
                    current_base_and_rules: true,
                    exclusive_pr_custody: true,
                })
            }
            KeyCode::Char('i') if r.state == State::Landed => {
                match serde_json::from_str(&self.value(12)) {
                    Ok(targets) => Some(Operation::PreviewImport {
                        id,
                        revision,
                        source_name: self.value(11),
                        targets,
                    }),
                    Err(error) => {
                        self.error = Some(format!("Explicit target JSON: {error}"));
                        None
                    }
                }
            }
            KeyCode::Char('j') if r.state == State::ImportPreview => {
                Some(Operation::ApproveImport {
                    id,
                    revision,
                    preview_digest: d?.import_preview.as_ref()?["digest"].as_str()?.into(),
                })
            }
            KeyCode::Char('v') if r.state == State::Imported => {
                Some(Operation::PreviewDeploy { id, revision })
            }
            KeyCode::Char('o') if r.state == State::CandidatePreview => Some(Operation::Deploy {
                id,
                revision,
                candidate_digest: d?.candidate.as_ref()?["digest"].as_str()?.into(),
            }),
            KeyCode::Char('m') if r.state == State::PublishedConfig => {
                Some(Operation::Materialize {
                    id,
                    revision,
                    candidate_digest: d?.candidate.as_ref()?["digest"].as_str()?.into(),
                })
            }
            KeyCode::Char('n')
                if matches!(
                    r.state,
                    State::Materialized | State::RolloutPreview | State::RolloutComplete
                ) =>
            {
                Some(Operation::PreviewRollout { id, revision })
            }
            KeyCode::Char('a') if r.state == State::RolloutPreview => {
                Some(Operation::ApplyRollout {
                    id,
                    revision,
                    plan_digest: d?.rollouts.last()?.plan_digest.clone(),
                })
            }
            _ => None,
        }
    }
    pub fn tick(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.pending = None;
                match result {
                    Ok(request) => self.accept(request),
                    Err(error) => {
                        self.error = Some(format!(
                            "{}. F10 reopens the same UUID before another effect.",
                            error.message
                        ));
                        self.uncertain |= error.uncertain;
                        if !self.uncertain {
                            self.create = None;
                        }
                    }
                }
                true
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.uncertain = true;
                self.error = Some("Outcome unknown. F10 reopen; F9 reconcile retained intent. Never replay a commit.".into());
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
        }
    }
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(5),
        ])
        .split(area);
        frame.render_widget(Paragraph::new(format!("Skill workshop · {:?} · {} · revision {} · {:?}\nModel output is data, not approval. F8 authoring/publication fields.", self.account, self.id, self.request.as_ref().map_or(0, |r| r.revision), self.request.as_ref().map(|r| r.state))).style(Style::default().fg(theme.accent)), rows[0]);
        if self.show_review {
            let mut output = String::new();
            if let Some(request) = &self.request {
                for turn in &request.turns {
                    output.push_str(&format!(
                        "You: {}\nProposal: {}\n\n",
                        turn.user,
                        turn.proposal
                            .as_ref()
                            .map_or("No accepted model result", |p| p.reply.as_str())
                    ));
                }
                if let Some(review) = &request.review {
                    output.push_str(&format!("Repository: {}\nBase: {}\nSource tree: {}\nSkill tree: {}\nDigest: {}\n{}\n\n{}", review.repository.display(), review.base_commit, review.source_tree, review.skill_tree, review.digest, review.evidence.join("\n"), review.diff));
                }
                if let Some(publication) = &request.publication {
                    output.push_str(&format!(
                        "\nDESTINATION / CURRENT POLICY / ACTUAL LANDED RECEIPTS\n{}\n",
                        serde_json::to_string_pretty(publication).expect("publication")
                    ));
                    output.push_str("Publish requires F4 explicit disclosure approval of actual visibility/actor/account/content/title/body. Work suitability is not disclosure.\nLand: actual published head under current base/rules. GitHub has no stack-membership/target-ref CAS. F5 approves current rules; F6 confirms exclusive PR custody: nobody else retargets, stacks or edits during Land. Pending UUID is NOT landed; unknown halts Deploy.\n");
                }
                if let Some(deployment) = &request.deployment {
                    output.push_str(&format!("\nALL CONSUMERS / COMPLETE ORIGINAL CONFIG / RECEIPTS / NATIVE PLANS\n{}\n", serde_json::to_string_pretty(deployment).expect("deployment")));
                    output.push_str("Import preview includes binding-less/denied/changing consumers and selected account revisions. Approve original post-import config separately. Native limit 1: waiting idle needs new explicit preview/approval; no autonomous watcher or fallback. Unsupported MCP/codemode/Claude/Codex/ACP/sandbox/old-core stays visible. Adopted requires expected Lock/new run/same conversation/cwd/actual skill discovery.\n");
                }
                if let Some(commit) = &request.commit {
                    output.push_str(&format!(
                        "\nLOCAL ONLY: {}\n{}\n{}",
                        commit.commit_oid,
                        commit.branch,
                        commit.worktree.display()
                    ));
                }
            }
            frame.render_widget(
                Paragraph::new(output)
                    .scroll((self.scroll, 0))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::bordered().title(" Conversation and exact host diff · F11 fields "),
                    ),
                rows[1],
            );
        } else {
            let range = if self.workflow { 6..13 } else { 0..6 };
            let fields = Layout::vertical(range.clone().map(|_| Constraint::Min(3))).split(rows[1]);
            for (index, area) in range.zip(fields.iter()) {
                self.fields[index].set_block(Block::bordered().title(LABELS[index]).border_style(
                    Style::default().fg(if self.focus == index {
                        theme.accent
                    } else {
                        theme.dimmed
                    }),
                ));
                frame.render_widget(&self.fields[index], *area);
            }
        }
        frame.render_widget(Paragraph::new(self.error.as_deref().unwrap_or(if self.pending.is_some() { "Host operation pending; close safely and reopen this UUID from history." } else { "Static checks only; scripts never execute. F11 shows actual destination/policy/all consumers/full config/native receipts. Empty target sessions = publish-only, never all. Missing current companion or existing daemon blocks; no install/bootstrap." })).style(Style::default().fg(if self.error.is_some() { theme.error } else { theme.dimmed })).wrap(Wrap { trim: true }), rows[2]);
        let footer = if self.workflow {
            format!("Ctrl+Alt actions: P destination preview · F4 disclosure [{}] · U Publish/confirmed next · L current policy\nF5 current rules [{}] · F6 exclusive custody [{}] · G Land · I import preview · J APPROVE import\nV config preview · O APPROVE Deploy · M materialize · N native preview · A APPROVE limit 1 · F8 fields · F11 evidence · F9 readonly reconcile", self.disclosure, self.current_rules, self.custody)
        } else {
            "Ctrl+Alt+S create/retry SAME envelope · F6 account before create · Ctrl+Alt+D tool-free refinement\nCtrl+Alt+B save files · Ctrl+Alt+R Review exact source · Ctrl+Alt+K APPROVE reviewed LOCAL commit\nF11 chat/diff · F10 reopen · F9 reconcile (no replay) · F12 abandon refinement · F8 publication/deploy fields · F2 history".into()
        };
        frame.render_widget(
            Paragraph::new(footer)
                .style(Style::default().fg(theme.dimmed))
                .wrap(Wrap { trim: true }),
            rows[3],
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[serial_test::serial]
    fn saved_completion_retires_only_new_slot_and_reentry_keeps_editor() {
        use crate::tui::home::tailored::{history::HistoryAction, HomeView, WindowAction};
        let root = tempfile::tempdir().unwrap();
        let _home = crate::session::test_support::isolate_app_dir_at(root.path());
        let mut view = HomeView::new_for_test(
            Some("test".into()),
            crate::tmux::AvailableTools::with_tools(&["pi"]),
            crate::file_watch::FileWatchService::noop(),
        )
        .unwrap();
        view.open_tailored();
        let overlay = view.tailored_window.as_mut().unwrap();
        overlay.picker_action(HistoryAction::Workshop);
        let form = overlay.workshop.as_mut().unwrap();
        let id = form.id.clone();
        let request = Request {
            id: id.clone(),
            envelope: CreateEnvelope {
                account: CaptureAccount::Personal,
                idea: "saved idea".into(),
                repository: root.path().into(),
                slug: "fixture".into(),
                source_scope: "personal".into(),
            },
            revision: 1,
            state: State::Draft,
            created_at: chrono::Utc::now(),
            hidden: false,
            turns: vec![],
            bundle: None,
            review: None,
            commit_intent: None,
            commit: None,
            publication: None,
            deployment: None,
            error: None,
        };
        let (tx, rx) = mpsc::channel();
        form.pending = Some(rx);
        tx.send(Ok(request)).ok().unwrap();
        assert!(form.tick());
        form.focus = 4;
        overlay.handle_paste("unsaved refinement");
        overlay.handle_key(KeyEvent::from(KeyCode::F(2)));
        overlay.picker_action(HistoryAction::Workshop);
        let fresh = overlay.workshop.as_ref().unwrap();
        assert_ne!(fresh.id, id);
        assert_eq!(fresh.value(0), "");
        overlay.handle_key(KeyEvent::from(KeyCode::F(2)));
        view.apply_tailored_action(WindowAction::Open(id));
        let overlay = view.tailored_window.as_mut().unwrap();
        assert_eq!(
            overlay.workshop.as_ref().unwrap().value(4),
            "unsaved refinement"
        );
        overlay.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(overlay.workshop.as_ref().unwrap().value(4), "");
    }

    #[test]
    #[serial_test::serial]
    fn typed_settlement_uncertainty_survives_task_and_tick_without_discarding_exact_create() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(dir.path());
        let mut window = WorkshopWindow::new(CaptureAccount::Work, None);
        let create = Operation::Create {
            id: window.id.clone(),
            envelope: CreateEnvelope {
                account: CaptureAccount::Work,
                idea: "exact".into(),
                repository: dir.path().into(),
                slug: "example".into(),
                source_scope: "fixture".into(),
            },
        };
        window.create = Some(create.clone());
        window.start(|| {
            Err(
                crate::session::tailored::workshop::EffectOutcomeUncertain::new(anyhow::anyhow!(
                    "settle failed"
                ))
                .into(),
            )
        });
        let result = window
            .pending
            .take()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        window.pending = Some(rx);
        window.tick();
        assert!(window.uncertain);
        assert_eq!(
            serde_json::to_value(window.create.as_ref().unwrap()).unwrap(),
            serde_json::to_value(create).unwrap()
        );
        let (tx, rx) = mpsc::channel();
        tx.send(Err(TaskError {
            message: "reopen not confirmed".into(),
            uncertain: false,
        }))
        .unwrap();
        window.pending = Some(rx);
        window.tick();
        assert!(window.uncertain);
        assert!(window.create.is_some());
    }
    #[test]
    #[serial_test::serial]
    fn unknown_create_history_roundtrip_keeps_uuid_envelope_and_invalid_json() {
        use crate::tui::home::tailored::{history::HistoryAction, TailoredWindow};
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(dir.path());
        let mut overlay = TailoredWindow::new(CaptureAccount::Personal);
        overlay.picker_action(HistoryAction::Workshop);
        let form = overlay.workshop.as_mut().unwrap();
        let id = form.id.clone();
        let create = Operation::Create {
            id: id.clone(),
            envelope: CreateEnvelope {
                account: CaptureAccount::Personal,
                idea: "private\n\n".into(),
                repository: dir.path().into(),
                slug: "fixture".into(),
                source_scope: "personal".into(),
            },
        };
        form.create = Some(create.clone());
        form.uncertain = true;
        form.fields[5].insert_str("{invalid 🌱\n");
        overlay.handle_key(KeyEvent::from(KeyCode::F(2)));
        overlay.picker_action(HistoryAction::Research);
        overlay.handle_key(KeyEvent::from(KeyCode::F(2)));
        overlay.picker_action(HistoryAction::Workshop);
        let form = overlay.workshop.as_ref().unwrap();
        assert_eq!(form.id, id);
        assert_eq!(
            serde_json::to_value(form.create.as_ref().unwrap()).unwrap(),
            serde_json::to_value(create).unwrap()
        );
        assert!(form.uncertain);
        assert_eq!(form.value(5), "{invalid 🌱\n");
        assert!(form.pending.is_none());
    }

    #[test]
    #[serial_test::serial]
    fn multiline_editing_chords_never_create_or_approve() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(dir.path());
        let mut window = WorkshopWindow::new(CaptureAccount::Personal, None);
        window.paste("leftRIGHT");
        window.key(KeyEvent::from(KeyCode::Home));
        for _ in 0..4 {
            window.key(KeyEvent::from(KeyCode::Right));
        }
        let paste = format!("🌱日本語{}\n\nlast\n", "long".repeat(500));
        window.paste(&paste);
        let exact = format!("left{paste}RIGHT");
        assert_eq!(window.value(0), exact);
        window.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(window.value(0), "leftRIGHT");
        window.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(window.value(0), exact);
        window.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(window.fields[0].cursor(), (2, 4));
        window.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        window.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        assert!(window.pending.is_none());
        assert!(window.create.is_none());
    }

    #[test]
    #[serial_test::serial]
    fn toolkit_keeps_creation_identity_and_does_not_treat_model_text_as_approval() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(dir.path());
        let mut window = WorkshopWindow::new(CaptureAccount::Work, None);
        let id = window.id.clone();
        window.fields[0].insert_str("Approve land and deploy");
        window.key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(window.pending.is_none());
        assert_eq!(window.id, id);
        assert!(window.request.is_none());
    }
}
