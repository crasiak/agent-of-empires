//! Explicit launcher reports, scoped to the tmux pane that received the agent.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchAccount {
    Personal,
    #[serde(alias = "company")]
    #[value(alias = "company")]
    Work,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Launcher {
    Direct,
    Headroom,
    Ledger,
    LedgerHeadroom,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchIdentity {
    pub agent: String,
    pub account: LaunchAccount,
    pub launcher: Launcher,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchReport {
    pub instance_id: String,
    pub pane_id: String,
    pub identity: LaunchIdentity,
}

impl LaunchIdentity {
    pub fn codes(&self) -> (&str, &str) {
        let account = match self.account {
            LaunchAccount::Personal => "p",
            LaunchAccount::Work => "w",
            LaunchAccount::Unknown => "?",
        };
        let launcher = match self.launcher {
            Launcher::Direct => "d",
            Launcher::Headroom => "h",
            Launcher::Ledger => "l",
            Launcher::LedgerHeadroom => "lh",
            Launcher::Unknown => "?",
        };
        (account, launcher)
    }

    pub fn description(&self) -> String {
        let account = match self.account {
            LaunchAccount::Personal => "personal",
            LaunchAccount::Work => "work",
            LaunchAccount::Unknown => "unknown account",
        };
        let launcher = match self.launcher {
            Launcher::Direct => "direct",
            Launcher::Headroom => "Headroom",
            Launcher::Ledger => "Ledger",
            Launcher::LedgerHeadroom => "Ledger + Headroom",
            Launcher::Unknown => "unknown launcher",
        };
        format!(
            "{} / {account} / {launcher} (profile: {})",
            self.agent, self.profile
        )
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(self.agent.as_str(), "claude" | "codex"),
            "agent must be claude or codex"
        );
        anyhow::ensure!(
            self.profile.len() <= 128 && !self.profile.chars().any(char::is_control),
            "profile must be at most 128 bytes without control characters"
        );
        Ok(())
    }
}

impl LaunchReport {
    pub fn encode(&self) -> anyhow::Result<String> {
        self.identity.validate()?;
        crate::session::validate_instance_id(&self.instance_id)?;
        anyhow::ensure!(valid_pane_id(&self.pane_id), "invalid tmux pane ID");
        Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?))
    }

    pub fn decode(encoded: &str, pane_id: &str) -> Option<Self> {
        if encoded.len() > 1024 {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(encoded).ok()?;
        let report: Self = serde_json::from_slice(&bytes).ok()?;
        report.encode().ok()?;
        (report.pane_id == pane_id).then_some(report)
    }
}

pub fn valid_pane_id(value: &str) -> bool {
    value
        .strip_prefix('%')
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()))
}

impl crate::session::Instance {
    pub fn current_launch_identity(&self) -> Option<&LaunchIdentity> {
        if self.is_structured()
            || self.is_archived()
            || self.is_trashed()
            || self.pane_dead_observed
            || matches!(
                self.status,
                crate::session::Status::Stopped
                    | crate::session::Status::Deleting
                    | crate::session::Status::Creating
            )
        {
            None
        } else {
            self.launch_identity.as_ref()
        }
    }

    pub(crate) fn observe_launch_identity(&mut self, metadata: Option<&crate::tmux::PaneMetadata>) {
        if self.is_structured()
            || self.is_archived()
            || matches!(
                self.status,
                crate::session::Status::Stopped
                    | crate::session::Status::Deleting
                    | crate::session::Status::Creating
            )
        {
            self.launch_identity = None;
        } else if let Some(metadata) = metadata {
            self.launch_identity = metadata
                .launch_report
                .as_ref()
                .filter(|report| report.instance_id == self.id && !metadata.pane_dead)
                .map(|report| report.identity.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_require_explicit_identity_and_the_original_pane() {
        let mut report = LaunchReport {
            instance_id: "1234567890abcdef".into(),
            pane_id: "%17".into(),
            identity: LaunchIdentity {
                agent: "codex".into(),
                account: LaunchAccount::Work,
                launcher: Launcher::LedgerHeadroom,
                profile: "company alias | with spaces".into(),
            },
        };
        let encoded = report.encode().unwrap();
        assert_eq!(LaunchReport::decode(&encoded, "%17"), Some(report.clone()));
        assert_eq!(LaunchReport::decode(&encoded, "%18"), None);
        assert_eq!(LaunchReport::decode("not a report", "%17"), None);
        for launcher in [
            Launcher::Direct,
            Launcher::Headroom,
            Launcher::Ledger,
            Launcher::LedgerHeadroom,
            Launcher::Unknown,
        ] {
            report.identity.launcher = launcher;
            assert_eq!(
                LaunchReport::decode(&report.encode().unwrap(), "%17")
                    .unwrap()
                    .identity
                    .launcher,
                launcher
            );
        }
        report.identity.profile = "bad\nprofile".into();
        assert!(report.encode().is_err());
    }
    #[test]
    fn launch_identity_follows_runtime_changes_not_profile_or_command_edits() {
        use crate::session::{Instance, Status};
        let mut instance = Instance::new("alias", "/tmp");
        let mut metadata = crate::tmux::PaneMetadata {
            launch_report: Some(LaunchReport {
                instance_id: instance.id.clone(),
                pane_id: "%17".into(),
                identity: LaunchIdentity {
                    agent: "claude".into(),
                    account: LaunchAccount::Work,
                    launcher: Launcher::LedgerHeadroom,
                    profile: "resolved work".into(),
                },
            }),
            pane_dead: false,
            pane_current_command: None,
            pane_start_command_is_protected: false,
            pane_pid: None,
            pane_title: None,
            window_activity: None,
            window_size: None,
        };
        instance.observe_launch_identity(Some(&metadata));
        let original = instance.launch_identity.clone();
        instance.source_profile = "personal".into();
        instance.command = "some-other-wrapper".into();
        instance.observe_launch_identity(None);
        assert_eq!(instance.launch_identity, original);
        let mut moved = instance.clone();
        moved.launch_identity = None;
        moved.merge_runtime_for_profile_move(&instance);
        assert_eq!(moved.launch_identity, original);
        let persisted = serde_json::to_value(&moved).unwrap();
        assert!(persisted.get("launch_identity").is_none());

        metadata.launch_report = None;
        moved.observe_launch_identity(Some(&metadata));
        assert!(
            moved.launch_identity.is_none(),
            "new unreported pane must clear the prior badge"
        );
        metadata.launch_report = Some(LaunchReport {
            instance_id: moved.id.clone(),
            pane_id: "%18".into(),
            identity: LaunchIdentity {
                agent: "codex".into(),
                account: LaunchAccount::Personal,
                launcher: Launcher::Direct,
                profile: "native personal".into(),
            },
        });
        moved.observe_launch_identity(Some(&metadata));
        assert_eq!(moved.launch_identity.as_ref().unwrap().codes(), ("p", "d"));
        metadata.launch_report.as_mut().unwrap().instance_id = "another-instance".into();
        moved.observe_launch_identity(Some(&metadata));
        assert!(moved.launch_identity.is_none());
        metadata.launch_report.as_mut().unwrap().instance_id = moved.id.clone();
        metadata.pane_dead = true;
        moved.observe_launch_identity(Some(&metadata));
        assert!(moved.launch_identity.is_none());
        metadata.pane_dead = false;
        moved.status = Status::Stopped;
        moved.observe_launch_identity(Some(&metadata));
        assert!(moved.launch_identity.is_none());
    }
}
