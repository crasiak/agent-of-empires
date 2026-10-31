use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type QuickLaunchPresets = BTreeMap<String, QuickLaunchPreset>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickLaunchPreset {
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_profile: Option<String>,
}

impl QuickLaunchPreset {
    pub fn validate(&self) -> Result<(), String> {
        crate::session::validate_profile_name(&self.profile).map_err(|e| e.to_string())?;
        if let Some(profile) = &self.ledger_profile {
            ledger_command(self.tool.as_deref().unwrap_or(""), profile)?;
        }
        Ok(())
    }
}

pub fn validate_presets(value: serde_json::Value) -> Result<QuickLaunchPresets, String> {
    let presets: QuickLaunchPresets = serde_json::from_value(value).map_err(|e| e.to_string())?;
    for (slot, preset) in &presets {
        if slot.len() != 1 || !matches!(slot.as_bytes()[0], b'1'..=b'9') {
            return Err("Quick-launch slots must be 1 through 9".into());
        }
        preset.validate()?;
    }
    Ok(presets)
}

pub fn ledger_command(tool: &str, profile: &str) -> Result<String, String> {
    if !matches!(tool, "pi" | "claude" | "codex") {
        return Err("Ledger quick launch requires Pi, Claude Code or Codex".into());
    }
    if profile.is_empty()
        || profile.starts_with('-')
        || !profile
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err(
            "Ledger profile must contain only letters, digits, dots, underscores or hyphens".into(),
        );
    }
    Ok(format!("ledger run {tool} --profile {profile} --"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn presets_validate_slots_profiles_and_ledger_arguments() {
        let valid = json!({"5": {"profile": "work-headroom-ledger", "tool": "pi", "ledger_profile": "work-claude"}});
        let presets = validate_presets(valid).unwrap();
        assert_eq!(
            ledger_command("pi", presets["5"].ledger_profile.as_deref().unwrap()).unwrap(),
            "ledger run pi --profile work-claude --"
        );
        for invalid in [
            json!({"0": {"profile": "work"}}),
            json!({"10": {"profile": "work"}}),
            json!({"1": {"profile": "../work"}}),
            json!({"1": {"profile": "work", "ledger_profile": "work"}}),
            json!({"1": {"profile": "work", "tool": "pi", "ledger_profile": "work; echo bad"}}),
            json!({"1": {"profile": "work", "tool": "pi", "ledger_profile": "--help"}}),
            json!({"1": {"profile": "work", "unknown": true}}),
        ] {
            assert!(validate_presets(invalid.clone()).is_err(), "{invalid}");
        }
        assert!(validate_presets(json!({"9": {"profile": "work"}})).is_ok());
    }
}
