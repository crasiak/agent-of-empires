//! Which pane hosted each conversation, from AoE's usage log. A pane's thread
//! runs through many conversations, one per `/clear`, compaction or resume;
//! search hits name a conversation, and this maps it back to the pane.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::usage::{UsageEvent, UsageKind};

/// One conversation in a pane's thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iteration {
    pub session_id: String,
    pub started: DateTime<Utc>,
    pub last: DateTime<Utc>,
    pub prompts: u32,
    /// What began it, as the usage log's `context_start` detail: `startup`,
    /// `clear`, `compact`, `resume`; empty when the log saw no start.
    pub start: String,
}

/// A listed pane, for finding the live session in a folder.
#[derive(Debug, Clone)]
pub struct Pane {
    pub id: String,
    pub cwd: String,
    pub tool: String,
}

#[derive(Debug, Default)]
pub struct Threads {
    hosted: HashMap<String, String>,
    iterations: HashMap<String, Vec<Iteration>>,
    /// Listed panes by canonical folder.
    folders: HashMap<PathBuf, Vec<Pane>>,
}

impl Threads {
    /// Threads of the panes `listed` keeps, from events oldest first. A
    /// conversation resumed into another pane belongs to the later one.
    pub fn from_events(events: &[UsageEvent], listed: &dyn Fn(&str) -> bool) -> Self {
        let mut threads = Self::default();
        for event in events.iter().filter(|e| listed(&e.instance_id)) {
            let Some(sid) = event.agent_session_id.as_deref() else {
                continue;
            };
            threads
                .hosted
                .insert(sid.to_string(), event.instance_id.clone());
            let thread = threads
                .iterations
                .entry(event.instance_id.clone())
                .or_default();
            let iteration = match thread.iter_mut().find(|i| i.session_id == sid) {
                Some(i) => i,
                None => {
                    thread.push(Iteration {
                        session_id: sid.to_string(),
                        started: event.occurred_at,
                        last: event.occurred_at,
                        prompts: 0,
                        start: String::new(),
                    });
                    thread.last_mut().expect("just pushed")
                }
            };
            iteration.last = event.occurred_at;
            match event.kind {
                UsageKind::Prompt => iteration.prompts += 1,
                UsageKind::ContextStart if iteration.start.is_empty() => {
                    iteration.start = event.detail.clone().unwrap_or_default();
                }
                _ => {}
            }
        }
        for thread in threads.iterations.values_mut() {
            thread.sort_by_key(|i| i.started);
        }
        threads
    }

    /// Reads the whole usage log; an unreadable log means no threads.
    pub fn load(listed: &dyn Fn(&str) -> bool) -> Self {
        let events = crate::usage::UsageStore::open_default()
            .and_then(|store| store.events_after(0, usize::MAX >> 1))
            .unwrap_or_default();
        Self::from_events(&events, listed)
    }

    /// Adds the listed panes, so a hit can find the live session in its folder.
    pub fn with_panes(mut self, panes: impl IntoIterator<Item = Pane>) -> Self {
        for pane in panes {
            let dir = crate::session::capture::canonicalize_or_raw(&pane.cwd);
            self.folders.entry(dir).or_default().push(pane);
        }
        self
    }

    /// The live session for work in `cwd`: a listed pane there, preferring
    /// `tool`, then the one whose thread was active last.
    pub fn pane_in_folder(&self, cwd: &str, tool: &str) -> Option<&str> {
        let dir = crate::session::capture::canonicalize_or_raw(cwd);
        let active = |p: &Pane| self.iterations(&p.id).last().map(|i| i.last);
        self.folders
            .get(&dir)?
            .iter()
            .max_by_key(|p| (p.tool == tool, active(p)))
            .map(|p| p.id.as_str())
    }

    /// The pane whose thread includes conversation `session_id`.
    pub fn pane_of(&self, session_id: &str) -> Option<&str> {
        self.hosted.get(session_id).map(String::as_str)
    }

    /// A pane's conversations, oldest first.
    pub fn iterations(&self, pane: &str) -> &[Iteration] {
        self.iterations.get(pane).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: i64, pane: &str, kind: UsageKind, detail: &str, sid: &str) -> UsageEvent {
        UsageEvent {
            id,
            occurred_at: DateTime::from_timestamp(1_700_000_000 + id * 60, 0).unwrap(),
            instance_id: pane.into(),
            profile: None,
            agent: Some("pi".into()),
            kind,
            detail: (!detail.is_empty()).then(|| detail.into()),
            agent_session_id: (!sid.is_empty()).then(|| sid.into()),
        }
    }

    #[test]
    fn a_pane_thread_lists_its_conversations_and_who_hosted_each() {
        use UsageKind::*;
        let events = [
            event(1, "p1", ContextStart, "startup", "a"),
            event(2, "p1", Prompt, "", "a"),
            event(3, "p1", Prompt, "", "a"),
            event(4, "p1", ContextStart, "clear", "b"),
            event(5, "p1", Prompt, "", "b"),
            event(6, "p1", InstanceRestarted, "", ""),
            event(7, "gone", ContextStart, "startup", "z"),
            // Resumed into another pane: that pane hosts it now.
            event(8, "p2", ContextStart, "resume", "a"),
        ];
        let threads = Threads::from_events(&events, &|pane| pane != "gone");
        let p1 = threads.iterations("p1");
        assert_eq!(
            p1.iter()
                .map(|i| (i.session_id.as_str(), i.prompts, i.start.as_str()))
                .collect::<Vec<_>>(),
            [("a", 2, "startup"), ("b", 1, "clear")]
        );
        assert_eq!(threads.pane_of("b"), Some("p1"));
        assert_eq!(threads.pane_of("a"), Some("p2"));
        assert_eq!(threads.pane_of("z"), None);
        assert!(threads.iterations("gone").is_empty());
    }

    #[test]
    fn the_live_session_in_a_folder_prefers_the_tool_then_recent_work() {
        use UsageKind::*;
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let pane = |id: &str, tool: &str| Pane {
            id: id.into(),
            cwd: cwd.clone(),
            tool: tool.into(),
        };
        let events = [
            event(1, "old", ContextStart, "startup", "a"),
            event(2, "new", ContextStart, "startup", "b"),
        ];
        let threads = Threads::from_events(&events, &|_| true).with_panes([
            pane("old", "pi"),
            pane("new", "pi"),
            pane("shell", "claude"),
        ]);
        assert_eq!(threads.pane_in_folder(&cwd, "pi"), Some("new"));
        assert_eq!(threads.pane_in_folder(&cwd, "claude"), Some("shell"));
        // The same folder spelled through a trailing slash still matches.
        assert_eq!(
            threads.pane_in_folder(&format!("{cwd}/"), "pi"),
            Some("new")
        );
        assert_eq!(threads.pane_in_folder("/nowhere", "pi"), None);
    }
}
