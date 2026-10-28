//! The session search overlay: opening it and acting on a pick.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use super::HomeView;
use crate::session::Status;
use crate::session_search::{ResumeSpec, Session, Target};
use crate::tui::app::Action;
use crate::tui::dialogs::{SearchOutcome, SessionSearchDialog};

impl HomeView {
    pub(super) fn open_session_search(&mut self) {
        let here_repo = self
            .selected_session
            .as_deref()
            .and_then(|id| self.get_instance(id))
            .map(|inst| {
                crate::git::GitWorktree::find_main_repo(Path::new(&inst.project_path))
                    .ok()
                    .and_then(|main| main.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| crate::session::projects::repo_label(&inst.project_path))
            });
        // An instance AoE still lists can be jumped to: Enter on a stopped one restarts it as usual.
        let listed: Vec<_> = self
            .instances()
            .filter(|i| !i.is_trashed() && !i.is_archived() && i.status != Status::Deleting)
            .collect();
        let live: HashSet<String> = listed.iter().map(|i| i.id.clone()).collect();
        // The conversation each pane runs, which hits for Pi do not carry.
        let bound: HashMap<String, String> = listed
            .iter()
            .filter_map(|i| Some((i.agent_session_id.clone()?, i.id.clone())))
            .collect();
        let profiles = crate::session_search::profile_homes();
        let ledger_home = dirs::home_dir().unwrap_or_default().join(".ledger");
        let resolver = Box::new(move |s: &Session| {
            crate::session_search::resolve(
                s,
                &|id| live.contains(id),
                &|sid| bound.get(sid).cloned(),
                &profiles,
                &ledger_home,
            )
        });
        let client = crate::session_search::Client::new();
        self.session_search = Some(SessionSearchDialog::new(
            here_repo,
            Box::new(move |req| (req.generation, client.run(&req))),
            resolver,
        ));
    }

    /// Fire a due query and drain results; true when the overlay needs a repaint.
    pub fn tick_session_search(&mut self, now: Instant) -> bool {
        self.session_search
            .as_mut()
            .is_some_and(|dialog| dialog.tick(now))
    }

    pub(super) fn apply_search_outcome(&mut self, outcome: SearchOutcome) -> Option<Action> {
        // Every outcome moves focus off the relay, as a palette command does.
        if let Some(state) = self.live_send.clone() {
            self.exit_live_send_and_restore_sizing(&state);
        }
        match outcome {
            SearchOutcome::Open(Target::Jump(id)) => self.activate_session_by_id(&id),
            SearchOutcome::Open(Target::Resume(spec)) => self.resume_search_hit(spec),
            SearchOutcome::Open(Target::CopyOnly(text)) | SearchOutcome::Copy(text) => {
                Some(Action::CopyToClipboard {
                    text,
                    status: "resume command copied".into(),
                })
            }
            SearchOutcome::Open(Target::Nothing) => Some(Action::SetTransientStatus(
                "no way back for this session".into(),
            )),
            SearchOutcome::OpenTranscript(path) => Some(Action::EditFile(path)),
        }
    }

    /// Add an instance resuming the hit natively; on failure copy its command instead.
    fn resume_search_hit(&mut self, spec: ResumeSpec) -> Option<Action> {
        if !Path::new(&spec.cwd).is_dir() {
            return Some(Action::CopyToClipboard {
                text: spec.command,
                status: "folder is gone; command copied".into(),
            });
        }
        // Repo config included, as `aoe session import` resolves it.
        let config = crate::session::config::repo_config::resolve_config_with_repo_or_warn(
            &spec.aoe_profile,
            Path::new(&spec.cwd),
        );
        // Named and grouped after the pane already in that folder, if there is one.
        let sibling = self
            .instances()
            .filter(|i| !i.is_trashed() && !i.is_archived() && same_dir(&i.project_path, &spec.cwd))
            .max_by_key(|i| i.tool == spec.tool)
            .map(|i| (i.title.clone(), i.group_path.clone()));
        let added = crate::session_search::instance(&spec, &config.session)
            .map(|mut inst| {
                if let Some((title, group)) = sibling {
                    inst.title = format!("{title} {}", spec.when).trim_end().to_string();
                    inst.group_path = group;
                }
                inst
            })
            .and_then(|inst| self.insert_new_session(inst));
        match added {
            Ok(id) => self.activate_session_by_id(&id),
            Err(e) => Some(Action::CopyToClipboard {
                text: spec.command,
                status: format!("resume failed: {e}; command copied"),
            }),
        }
    }

    /// Activating a row the list hides would act on whatever the cursor fell back to.
    fn activate_session_by_id(&mut self, id: &str) -> Option<Action> {
        self.select_and_reveal_session(id);
        if self.selected_session.as_deref() != Some(id) {
            self.select_session_by_id(id);
            return Some(Action::SetTransientStatus(
                "session is hidden in the list".into(),
            ));
        }
        self.activate_selected_session()
    }
}

fn same_dir(a: &str, b: &str) -> bool {
    crate::session::capture::canonicalize_or_raw(a)
        == crate::session::capture::canonicalize_or_raw(b)
}
