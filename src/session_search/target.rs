//! Resolve a search hit to a jump, a native resume, or a command to copy.

use std::path::{Path, PathBuf};

use super::query::Session;

/// An AoE profile and the agent config dir it uses for each tool.
pub struct ProfileHomes {
    pub profile: String,
    pub tool_dirs: Vec<(String, PathBuf)>,
}

pub struct ResumeSpec {
    pub tool: String,
    pub cwd: String,
    pub session_id: String,
    pub aoe_profile: String,
    pub store: PathBuf,
    pub title: String,
}

pub enum Target {
    Jump(String),
    Resume(ResumeSpec),
    CopyOnly(String),
    Nothing,
}

pub fn resolve(
    s: &Session,
    live: &dyn Fn(&str) -> bool,
    profiles: &[ProfileHomes],
    ledger_home: &Path,
) -> Target {
    if let Some(id) = s.aoe_instance_id.as_deref().filter(|id| live(id)) {
        return Target::Jump(id.to_string());
    }
    let Some(resume) = &s.resume else {
        return Target::Nothing;
    };
    match native_resume(s, resume.session_id.as_deref(), profiles, ledger_home) {
        Some(spec) => Target::Resume(spec),
        None => Target::CopyOnly(resume.command.clone()),
    }
}

fn native_resume(
    s: &Session,
    session_id: Option<&str>,
    profiles: &[ProfileHomes],
    ledger_home: &Path,
) -> Option<ResumeSpec> {
    let tool = match s.harness.as_str() {
        "claude" => "claude",
        "pi" => "pi",
        _ => return None,
    };
    let (session_id, cwd, profile) = (session_id?, s.cwd.as_deref()?, s.profile.as_deref()?);
    let owned = ledger_home.join("homes").join(profile);
    let (aoe_profile, store) = profiles.iter().find_map(|p| {
        let (_, dir) = p
            .tool_dirs
            .iter()
            .find(|(t, d)| t == tool && d.starts_with(&owned))?;
        Some((p.profile.clone(), dir.clone()))
    })?;
    let title = s
        .title
        .clone()
        .unwrap_or_else(|| format!("Resumed {}", session_id.chars().take(8).collect::<String>()));
    Some(ResumeSpec {
        tool: tool.to_string(),
        cwd: cwd.to_string(),
        session_id: session_id.to_string(),
        aoe_profile,
        store,
        title,
    })
}

/// AoE profiles with their claude and pi config dirs, read from user state.
pub fn profile_homes() -> Vec<ProfileHomes> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    crate::session::list_profiles()
        .unwrap_or_default()
        .into_iter()
        .map(|profile| {
            let config = crate::session::config::profile_config::resolve_config_or_warn(&profile);
            let tool_dirs = ["claude", "pi"]
                .into_iter()
                .filter_map(|tool| {
                    let dir = config.session.agent_config_dir_for(tool, &home)?;
                    Some((tool.to_string(), dir))
                })
                .collect();
            ProfileHomes { profile, tool_dirs }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_jump_then_native_resume_then_copy() {
        let hits =
            crate::session_search::query::parse(include_bytes!("testdata/query_v1.json")).unwrap();
        let home = std::path::Path::new("/home/u/.ledger");
        let profiles = vec![ProfileHomes {
            profile: "personal-sub".into(),
            tool_dirs: vec![
                (
                    "claude".into(),
                    home.join("homes/personal/claude-code/runtime"),
                ),
                ("pi".into(), home.join("homes/personal/pi/runtime")),
            ],
        }];
        assert!(
            matches!(resolve(&hits[0].session, &|id| id == "aoe1", &profiles, home), Target::Jump(id) if id == "aoe1")
        );
        match resolve(&hits[0].session, &|_| false, &profiles, home) {
            Target::Resume(r) => {
                assert_eq!(
                    (
                        r.tool.as_str(),
                        r.session_id.as_str(),
                        r.aoe_profile.as_str()
                    ),
                    ("claude", "c-111", "personal-sub")
                );
                assert_eq!(r.store, home.join("homes/personal/claude-code/runtime"));
            }
            _ => panic!("expected resume"),
        }
        assert!(
            matches!(resolve(&hits[2].session, &|_| false, &profiles, home), Target::Resume(r) if r.session_id == "p-222" && r.tool == "pi")
        );
        assert!(
            matches!(resolve(&hits[0].session, &|_| false, &[], home), Target::CopyOnly(c) if c.contains("--resume c-111"))
        );
        let mut bare =
            crate::session_search::query::parse(include_bytes!("testdata/query_v1.json"))
                .unwrap()
                .remove(0)
                .session;
        bare.resume = None;
        assert!(matches!(
            resolve(&bare, &|_| false, &profiles, home),
            Target::Nothing
        ));
    }
}
