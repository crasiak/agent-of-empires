//! Shared Tailored history projection and view-only visibility state.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::CaptureAccount;
use crate::session::{config::AppStateConfig, Instance};

pub type HiddenSessions = BTreeMap<String, BTreeSet<String>>;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: String,
    pub profile: String,
    pub title: String,
    pub account: CaptureAccount,
    pub input_preview: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub hidden: bool,
    pub research: bool,
    #[serde(skip_serializing)]
    pub workshop: bool,
}

pub fn capture_account(instance: &Instance) -> Option<CaptureAccount> {
    if instance.tool != "pi" {
        return None;
    }
    let words = shell_words::split(&instance.command).ok()?;
    [CaptureAccount::Personal, CaptureAccount::Work]
        .into_iter()
        .find(|account| {
            words
                == [
                    "ledger",
                    "run",
                    "pi",
                    "--profile",
                    account.ledger_profile(),
                    "--",
                ]
        })
}

pub fn tailored_account(instance: &Instance) -> Option<CaptureAccount> {
    capture_account(instance)
        .or_else(|| super::document::account(instance))
        .or_else(|| super::quick::preset(instance).map(|preset| preset.account()))
}

fn input_preview(instance: &Instance) -> String {
    if super::quick::preset(instance).is_some() {
        return String::new();
    }
    if super::document::account(instance).is_some() {
        return super::document::preview(instance)
            .chars()
            .filter(|c| !c.is_control())
            .take(160)
            .collect();
    }
    let raw = shell_words::split(&instance.extra_args)
        .ok()
        .and_then(|args| {
            let prompt = args.first()?;
            let (_, idea) = prompt.split_once("\n<idea>\n")?;
            idea.strip_suffix("\n</idea>").map(str::to_owned)
        })
        .unwrap_or_default();
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|ch| {
            !ch.is_control() && !matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(160)
        .collect()
}

pub fn entry(instance: &Instance, hidden: &HiddenSessions) -> Option<HistoryEntry> {
    if instance.is_trashed() || instance.is_archived() {
        return None;
    }
    let account = tailored_account(instance)?;
    let profile = instance.effective_profile();
    Some(HistoryEntry {
        id: instance.id.clone(),
        research: false,
        workshop: false,
        hidden: hidden
            .get(&profile)
            .is_some_and(|ids| ids.contains(&instance.id)),
        profile,
        title: if super::document::account(instance).is_some() {
            format!("Document chat · {}", instance.title)
        } else {
            instance.title.clone()
        },
        account,
        input_preview: input_preview(instance),
        status: instance.status.wire_str().into(),
        created_at: instance.created_at.into(),
    })
}

pub fn compare(a: &HistoryEntry, b: &HistoryEntry) -> std::cmp::Ordering {
    b.created_at
        .cmp(&a.created_at)
        .then_with(|| a.profile.cmp(&b.profile))
        .then_with(|| a.id.cmp(&b.id))
}

pub fn entries<'a>(
    instances: impl IntoIterator<Item = &'a Instance>,
    hidden: &HiddenSessions,
) -> Vec<HistoryEntry> {
    let mut entries: Vec<_> = instances
        .into_iter()
        .filter_map(|instance| entry(instance, hidden))
        .collect();
    entries.sort_by(compare);
    entries
}

pub fn load_hidden() -> anyhow::Result<HiddenSessions> {
    Ok(AppStateConfig::load()?.tailored_hidden_sessions)
}

pub fn set_hidden(profile: &str, id: &str, hidden: bool) -> anyhow::Result<()> {
    crate::session::update_app_state(|state| {
        set_visibility(&mut state.tailored_hidden_sessions, profile, id, hidden);
    })
}

fn set_visibility(state: &mut HiddenSessions, profile: &str, id: &str, hidden: bool) {
    if hidden {
        state.entry(profile.into()).or_default().insert(id.into());
    } else if let Some(ids) = state.get_mut(profile) {
        ids.remove(id);
        if ids.is_empty() {
            state.remove(profile);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tailored::TailoredRequest;

    fn capture(title: &str, account: CaptureAccount) -> Instance {
        let mut instance = Instance::new(title, "/tmp/capture");
        let launch = TailoredRequest {
            preset: "capture-idea".into(),
            account,
            idea: "A Unicode 🌱 idea\nwith <idea> markers and \u{202e}control".into(),
            immediately: false,
            document: None,
        }
        .launch()
        .unwrap();
        instance.tool = "pi".into();
        instance.scratch = true;
        instance.command = launch.command;
        instance.extra_args = launch.extra_args;
        instance.source_profile = "alpha".into();
        instance
    }

    #[test]
    fn history_recognizes_saved_launches_but_not_similar_commands_or_shelved_rows() {
        let mut instance = capture("Renamed capture", CaptureAccount::Work);
        let entry = entry(&instance, &HiddenSessions::new()).unwrap();
        assert_eq!(entry.account, CaptureAccount::Work);
        assert_eq!(entry.title, "Renamed capture");
        assert!(entry
            .input_preview
            .contains("Unicode 🌱 idea with <idea> markers"));
        assert!(!entry.input_preview.contains('\u{202e}'));
        assert_eq!(capture_account(&instance), Some(CaptureAccount::Work));
        for command in [
            "echo ledger run pi --profile aoe-capture-work --",
            "ledger run pi --profile work --",
            "ledger run pi --profile aoe-capture-work --; echo x",
        ] {
            let mut other = instance.clone();
            other.command = command.into();
            assert!(capture_account(&other).is_none(), "{command}");
        }
        instance.scratch = false;
        assert!(super::entry(&instance, &HiddenSessions::new()).is_some());
        instance.archived_at = Some(crate::time::Timestamp::now());
        assert!(super::entry(&instance, &HiddenSessions::new()).is_none());
    }

    #[test]
    fn legacy_quick_history_preserves_conversation_and_storage_identity_without_awareness_preview()
    {
        for (preset, profile) in [
            (super::super::quick::QuickPreset::Work, "aoe-quick-work"),
            (
                super::super::quick::QuickPreset::Personal,
                "aoe-quick-personal",
            ),
        ] {
            let plan = preset.plan(std::path::Path::new("/home/test")).unwrap();
            let mut instance = Instance::new(preset.title(), "/home/test/knowledge");
            instance.tool = "pi".into();
            instance.command = format!("ledger run pi --profile {profile} --");
            instance.extra_args = plan.launch.extra_args;
            instance.source_profile = "storage-owner".into();
            instance.agent_session_id = Some("existing-conversation".into());
            let before = serde_json::to_value(&instance).unwrap();
            let projected = entry(&instance, &HiddenSessions::new()).unwrap();
            assert_eq!(projected.id, instance.id);
            assert_eq!(projected.profile, "storage-owner");
            assert_eq!(projected.account, preset.account());
            assert_eq!(projected.title, preset.title());
            assert!(projected.input_preview.is_empty());
            assert_eq!(serde_json::to_value(&instance).unwrap(), before);
            for command in [
                format!("{} --model other", instance.command),
                format!("echo {}", instance.command),
                "ledger run pi --profile work --".into(),
            ] {
                let mut other = instance.clone();
                other.command = command;
                assert!(tailored_account(&other).is_none());
            }
        }
    }

    #[test]
    fn hiding_is_profile_scoped_and_never_changes_the_session() {
        let instance = capture("Idea", CaptureAccount::Personal);
        let before = serde_json::to_value(&instance).unwrap();
        let mut hidden = HiddenSessions::new();
        set_visibility(&mut hidden, "beta", &instance.id, true);
        assert!(!entry(&instance, &hidden).unwrap().hidden);
        set_visibility(&mut hidden, "alpha", &instance.id, true);
        assert!(entry(&instance, &hidden).unwrap().hidden);
        set_visibility(&mut hidden, "alpha", &instance.id, false);
        assert!(!entry(&instance, &hidden).unwrap().hidden);
        assert!(hidden.contains_key("beta"));
        assert_eq!(serde_json::to_value(&instance).unwrap(), before);
    }
}
