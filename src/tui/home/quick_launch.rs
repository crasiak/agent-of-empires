use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::HomeView;
use crate::session::config::quick_launch::QuickLaunchPreset;
use crate::tui::dialogs::{InfoDialog, NewSessionDialog};

pub(super) fn shortcut_slot(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(slot @ '1'..='9')
            if key.modifiers == KeyModifiers::CONTROL && key.kind == KeyEventKind::Press =>
        {
            Some(slot)
        }
        _ => None,
    }
}

impl HomeView {
    pub(super) fn open_quick_launch(&mut self, slot: char) {
        let preset = crate::session::config::load_config().map(|config| {
            config
                .unwrap_or_default()
                .session
                .quick_launch
                .get(&slot.to_string())
                .cloned()
        });
        match preset {
            Ok(Some(preset)) => self.open_quick_launch_preset(preset),
            Ok(None) => {
                self.info_dialog = Some(InfoDialog::new("Quick Launch", &format!(
                    "Ctrl+{slot} is not assigned. Configure session.quick_launch in config.toml or Quick Launch Presets in Settings > Interaction."
                )));
            }
            Err(error) => {
                self.info_dialog = Some(InfoDialog::new("Quick Launch", &error.to_string()));
            }
        }
    }

    fn open_quick_launch_preset(&mut self, preset: QuickLaunchPreset) {
        if self.creating_stub_id.is_some() {
            self.info_dialog = Some(InfoDialog::new("Please Wait", "A session is already being created. Wait for it to finish or press Ctrl+C to cancel."));
            return;
        }
        if let Err(error) = preset.validate() {
            self.info_dialog = Some(InfoDialog::new("Quick Launch", &error));
            return;
        }
        let profiles = match crate::session::list_profiles_for_display() {
            Ok(profiles) => profiles,
            Err(error) => {
                self.info_dialog = Some(InfoDialog::new("Quick Launch", &error.to_string()));
                return;
            }
        };
        if !profiles.contains(&preset.profile) {
            self.info_dialog = Some(InfoDialog::new(
                "Quick Launch",
                &format!(
                    "AoE profile '{}' does not exist. Update this quick-launch preset.",
                    preset.profile
                ),
            ));
            return;
        }
        if !self.available_tools.any_available() {
            self.show_no_agents();
            return;
        }
        if let Some(tool) = &preset.tool {
            if !self.available_tools.available_list().contains(tool) {
                self.info_dialog = Some(InfoDialog::new("Quick Launch", &format!("Agent '{tool}' is not available. Install it or update this quick-launch preset.")));
                return;
            }
        }
        let source = self
            .selected_session
            .as_ref()
            .and_then(|id| self.get_instance(id));
        let path = source
            .filter(|instance| !instance.scratch)
            .map(|instance| instance.repo_path().to_string())
            .or_else(|| {
                self.selected_group
                    .as_deref()
                    .and_then(|group| self.group_repo_path(group))
            });
        let group = source
            .map(|instance| instance.group_path.clone())
            .filter(|group| !group.is_empty())
            .or_else(|| {
                self.selected_group
                    .as_deref()
                    .filter(|group| {
                        !crate::session::is_synthetic_project_header(group)
                            && !crate::session::is_archived_section_path(group)
                            && !crate::session::is_trash_section_path(group)
                    })
                    .map(|group| crate::session::project_group_display_name(group).to_string())
            });
        let mut dialog = NewSessionDialog::new(
            self.available_tools.clone(),
            self.all_group_paths(),
            &preset.profile,
            profiles,
        );
        let has_path = path.is_some();
        if let Some(path) = path {
            dialog.set_path(path);
        }
        if let Some(group) = group {
            dialog.set_group(group);
        }
        if let Some(tool) = preset.tool {
            dialog.set_tool(&tool);
        }
        if let Some(profile) = preset.ledger_profile {
            dialog.set_ledger_profile(profile);
        }
        if has_path {
            dialog.focus_title();
        }
        self.new_dialog = Some(dialog);
    }
}
