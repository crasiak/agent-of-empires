//! Blank interactive Pi sessions using existing account profiles.

use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};

use super::{CaptureAccount, TailoredLaunch};
use crate::session::Instance;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickPreset {
    Work,
    Personal,
}

impl QuickPreset {
    pub fn title(self) -> &'static str {
        match self {
            Self::Work => "Work Quick",
            Self::Personal => "Personal Quick",
        }
    }

    pub fn account(self) -> CaptureAccount {
        match self {
            Self::Work => CaptureAccount::Work,
            Self::Personal => CaptureAccount::Personal,
        }
    }

    pub fn aoe_profile(self) -> &'static str {
        match self {
            Self::Work => "work-headroom-ledger",
            Self::Personal => "personal-sub-headroom-ledger",
        }
    }

    fn intent_marker(self) -> &'static str {
        match self {
            Self::Work => "AoE Quick Pi session v1: work\n",
            Self::Personal => "AoE Quick Pi session v1: personal\n",
        }
    }

    pub fn verify_profile(self) -> Result<()> {
        let profile = self.aoe_profile();
        crate::session::resolve_existing_profile(profile)
            .with_context(|| format!("Quick requires existing AoE profile {profile}"))?;
        crate::session::config::profile_config::load_profile_config(profile)
            .with_context(|| format!("Cannot read Quick AoE profile {profile}"))?;
        Ok(())
    }

    pub fn plan(self, home: &Path) -> Result<QuickPlan> {
        ensure!(
            home.is_absolute(),
            "Quick requires an absolute home directory"
        );
        let roots = [
            "code/attic",
            "code/agent-of-empires",
            "corelight/labs/infra-c",
            "corelight/labs/murmr",
        ]
        .map(|relative| home.join(relative));
        let roots = roots
            .iter()
            .map(|root| serde_json::to_string(root).context("Invalid Quick context path"))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let prompt = format!(
            "{}Additional working context is available at these paths: {roots}. Use absolute paths when needed. Use the existing add_directory tool if you need to load external directory instructions or skills. These paths are awareness, not a sandbox or an instruction to load them automatically.",
            self.intent_marker()
        );
        Ok(QuickPlan {
            preset: self,
            cwd: home.join("knowledge"),
            launch: TailoredLaunch {
                command: String::new(),
                extra_args: format!("--append-system-prompt {}", shell_words::quote(&prompt)),
            },
        })
    }
}

pub struct QuickPlan {
    pub preset: QuickPreset,
    pub cwd: PathBuf,
    pub launch: TailoredLaunch,
}

pub fn preset(instance: &Instance) -> Option<QuickPreset> {
    if instance.tool != "pi" {
        return None;
    }
    let words = shell_words::split(&instance.command).ok()?;
    if let Some((preset, _)) = [
        (QuickPreset::Work, "aoe-quick-work"),
        (QuickPreset::Personal, "aoe-quick-personal"),
    ]
    .into_iter()
    .find(|(_, profile)| words == ["ledger", "run", "pi", "--profile", profile, "--"])
    {
        return Some(preset);
    }
    let args = shell_words::split(&instance.extra_args).ok()?;
    if args.len() != 2 || args[0] != "--append-system-prompt" {
        return None;
    }
    [QuickPreset::Work, QuickPreset::Personal]
        .into_iter()
        .find(|preset| {
            args[1].starts_with(preset.intent_marker())
                && instance.effective_profile() == preset.aoe_profile()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn quick_preflight_uses_existing_aoe_profiles_without_inspecting_or_creating_ledger_profiles() {
        let root = tempfile::tempdir().unwrap();
        let _home = crate::session::test_support::isolate_app_dir_at(root.path());
        let probe = root.path().join("ledger-inspected");
        let _ledger = crate::session::test_support::install_login_shell_path_command(
            root.path(),
            "ledger",
            &format!(
                "#!/bin/sh\nprintf inspected > {}\nexit 99\n",
                shell_words::quote(probe.to_str().unwrap())
            ),
        );
        for (preset, profile, launcher) in [
            (
                QuickPreset::Work,
                "work-headroom-ledger",
                "configured-work-pi",
            ),
            (
                QuickPreset::Personal,
                "personal-sub-headroom-ledger",
                "configured-personal-pi",
            ),
        ] {
            let path = crate::session::get_profile_dir_path(profile).unwrap();
            assert!(preset.verify_profile().is_err());
            assert!(!path.exists(), "missing profile was created");
            std::fs::create_dir_all(&path).unwrap();
            let config = format!("[session.agent_command_override]\npi = \"{launcher}\"\n");
            std::fs::write(path.join("config.toml"), &config).unwrap();
            preset
                .verify_profile()
                .expect("existing AoE profile must be accepted");
            assert_eq!(
                std::fs::read_to_string(path.join("config.toml")).unwrap(),
                config
            );
            std::fs::write(path.join("config.toml"), "session = [\n").unwrap();
            assert!(
                preset.verify_profile().is_err(),
                "invalid profile silently fell back"
            );
            assert_eq!(
                std::fs::read_to_string(path.join("config.toml")).unwrap(),
                "session = [\n"
            );
            std::fs::write(path.join("config.toml"), config).unwrap();
        }
        assert!(!probe.exists(), "Quick inspected a separate Ledger profile");
        for invented in ["aoe-quick-work", "aoe-quick-personal"] {
            assert!(!crate::session::get_profile_dir_path(invented)
                .unwrap()
                .exists());
        }
    }

    #[test]
    fn quick_history_requires_account_intent_and_the_existing_aoe_owner() {
        for (selected, owner, other, account) in [
            (
                QuickPreset::Work,
                "work-headroom-ledger",
                "personal-sub-headroom-ledger",
                "work",
            ),
            (
                QuickPreset::Personal,
                "personal-sub-headroom-ledger",
                "work-headroom-ledger",
                "personal",
            ),
        ] {
            let plan = selected.plan(Path::new("/home/test")).unwrap();
            let args = shell_words::split(&plan.launch.extra_args).unwrap();
            let marker = format!("AoE Quick Pi session v1: {account}\n");
            assert!(
                args[1].starts_with(&marker),
                "missing explicit Quick intent"
            );
            let mut instance = Instance::new(selected.title(), "/home/test/knowledge");
            instance.tool = "pi".into();
            instance.source_profile = owner.into();
            instance.command = "configured-pi-launcher".into();
            instance.extra_args = plan.launch.extra_args.clone();
            assert_eq!(preset(&instance), Some(selected));
            let row = super::super::history::entry(&instance, &Default::default()).unwrap();
            assert_eq!(row.account, selected.account());
            assert_eq!(row.profile, owner);
            assert!(row.input_preview.is_empty());
            instance.extra_args.clear();
            assert_eq!(
                preset(&instance),
                None,
                "ordinary Pi was classified as Quick"
            );
            instance.extra_args = plan.launch.extra_args.clone();
            instance.source_profile = other.into();
            assert_eq!(preset(&instance), None, "Quick crossed its account owner");
            instance.source_profile = owner.into();
            instance.tool = "claude".into();
            assert_eq!(preset(&instance), None);
            instance.tool = "pi".into();
            instance.extra_args = plan
                .launch
                .extra_args
                .replacen("session v1:", "session v2:", 1);
            assert_eq!(preset(&instance), None, "near-match intent was accepted");
        }
    }

    #[test]
    fn quick_plans_fix_accounts_and_quote_awareness_without_forcing_model_or_prompt() {
        let home = Path::new("/home/a 'quoted'; $(not-a-command)");
        let mut awareness = Vec::new();
        for preset in [QuickPreset::Work, QuickPreset::Personal] {
            let plan = preset.plan(home).unwrap();
            assert_eq!(plan.cwd, home.join("knowledge"));
            assert!(plan.launch.command.is_empty());
            let args = shell_words::split(&plan.launch.extra_args).unwrap();
            assert_eq!(args.len(), 2);
            assert_eq!(args[0], "--append-system-prompt");
            for relative in [
                "code/attic",
                "code/agent-of-empires",
                "corelight/labs/infra-c",
                "corelight/labs/murmr",
            ] {
                assert!(args[1].contains(&serde_json::to_string(&home.join(relative)).unwrap()));
            }
            assert!(args[1].contains("add_directory"));
            for forbidden in [
                "--model",
                "--thinking",
                "--add-dir",
                "capture-idea",
                "Immediately",
                "gpt-6.1-sol",
            ] {
                assert!(!plan.launch.extra_args.contains(forbidden));
            }
            awareness.push(args[1].split_once('\n').unwrap().1.to_owned());
        }
        assert_eq!(awareness[0], awareness[1]);
        assert!(QuickPreset::Work.plan(Path::new("relative")).is_err());
    }
}
