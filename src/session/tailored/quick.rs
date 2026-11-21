//! Blank interactive Pi launches. Dedicated Ledger settings own model defaults.

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

    pub fn profile(self) -> &'static str {
        match self {
            Self::Work => "aoe-quick-work",
            Self::Personal => "aoe-quick-personal",
        }
    }

    pub fn verify_profile(self) -> Result<()> {
        let output = crate::process::run_with_bounded_stdout(
            std::process::Command::new("ledger").args([
                "lock",
                "show",
                "--profile",
                self.profile(),
                "--harness",
                "pi",
            ]),
            std::time::Duration::from_secs(10),
            128 * 1024,
        )?
        .context("Quick profile inspection timed out or exceeded its output limit")?;
        ensure!(
            output.status.success(),
            "Quick profile {} is not configured",
            self.profile()
        );
        validate_lock(&output.stdout, self.profile())
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
            "Additional working context is available at these paths: {roots}. Use absolute paths when needed. Use the existing add_directory tool if you need to load external directory instructions or skills. These paths are awareness, not a sandbox or an instruction to load them automatically."
        );
        Ok(QuickPlan {
            preset: self,
            cwd: home.join("knowledge"),
            launch: TailoredLaunch {
                command: format!("ledger run pi --profile {} --", self.profile()),
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

fn validate_lock(bytes: &[u8], expected: &str) -> Result<()> {
    let lock: serde_json::Value = serde_json::from_slice(bytes)?;
    let binding = &lock["binding"];
    ensure!(
        matches!(
            lock["schema"].as_str(),
            Some("ledger-profile-lock-v4" | "ledger-profile-lock-v5")
        ) && lock["profile"]["name"] == expected
            && binding["harness"] == "pi"
            && binding["account_class"] == "pi-home"
            && matches!(
                binding["launcher"].as_str(),
                Some("direct" | "headroom-isolated")
            )
            && binding["launch"]["share"] == serde_json::json!(["conversation"]),
        "Quick requires its dedicated immutable Pi profile with conversation-only sharing"
    );
    for field in ["core", "runtime"] {
        let path = binding[field]["path"]
            .as_str()
            .context("Quick runtime path missing")?;
        ensure!(
            binding[field]["mode"] == "immutable"
                && Path::new(path).is_absolute()
                && Path::new(path).parent() == Some(Path::new("/nix/store")),
            "Quick requires an immutable {field} store object"
        );
    }
    Ok(())
}

pub fn preset(instance: &Instance) -> Option<QuickPreset> {
    if instance.tool != "pi" {
        return None;
    }
    let words = shell_words::split(&instance.command).ok()?;
    [QuickPreset::Work, QuickPreset::Personal]
        .into_iter()
        .find(|preset| words == ["ledger", "run", "pi", "--profile", preset.profile(), "--"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_preflight_rejects_wrong_identity_or_mutable_shared_runtime() {
        let valid = serde_json::json!({
            "schema": "ledger-profile-lock-v4",
            "profile": {"name": "aoe-quick-work"},
            "binding": {
                "harness": "pi", "account_class": "pi-home", "launcher": "headroom-isolated",
                "core": {"mode": "immutable", "path": "/nix/store/pi-core"},
                "runtime": {"mode": "immutable", "path": "/nix/store/headroom"},
                "launch": {"share": ["conversation"]}
            },
            "resources": [{"key": "skill/general-purpose"}]
        });
        assert!(validate_lock(&serde_json::to_vec(&valid).unwrap(), "aoe-quick-work").is_ok());
        for (pointer, replacement) in [
            ("/profile/name", serde_json::json!("aoe-quick-personal")),
            ("/binding/harness", serde_json::json!("codex")),
            ("/binding/account_class", serde_json::json!("codex-home")),
            ("/binding/launcher", serde_json::json!("shared")),
            (
                "/binding/launch/share",
                serde_json::json!(["conversation", "mcp-oauth"]),
            ),
            ("/binding/core/mode", serde_json::json!("mutable")),
            ("/binding/runtime/mode", serde_json::json!("mutable")),
            ("/binding/core/path", serde_json::json!("/tmp/pi-core")),
            ("/binding/runtime/path", serde_json::json!("relative")),
        ] {
            let mut wrong = valid.clone();
            *wrong.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                validate_lock(&serde_json::to_vec(&wrong).unwrap(), "aoe-quick-work").is_err(),
                "{pointer}"
            );
        }
    }

    #[test]
    fn quick_plans_fix_accounts_and_quote_awareness_without_forcing_model_or_prompt() {
        let home = Path::new("/home/a 'quoted'; $(not-a-command)");
        let mut awareness = Vec::new();
        for (preset, profile, account) in [
            (QuickPreset::Work, "aoe-quick-work", CaptureAccount::Work),
            (
                QuickPreset::Personal,
                "aoe-quick-personal",
                CaptureAccount::Personal,
            ),
        ] {
            let plan = preset.plan(home).unwrap();
            assert_eq!(plan.preset.account(), account);
            assert_eq!(plan.cwd, home.join("knowledge"));
            assert_eq!(
                shell_words::split(&plan.launch.command).unwrap(),
                ["ledger", "run", "pi", "--profile", profile, "--"]
            );
            assert_eq!(
                crate::session::ledger_pi::profile("pi", &plan.launch.command).as_deref(),
                Some(profile)
            );
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
            awareness.push(args[1].clone());
        }
        assert_eq!(awareness[0], awareness[1]);
        assert!(QuickPreset::Work.plan(Path::new("relative")).is_err());
    }
}
