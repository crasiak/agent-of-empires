//! Resolve a search hit to a jump, a native resume, or a command to copy.

use std::path::{Path, PathBuf};

use super::query::{Resume, Session};

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
    /// The folder's name and the conversation's last day; the caller may swap
    /// the folder for the title of a pane already there.
    pub title: String,
    /// The conversation's last day, `MM-DD` local, for that swap.
    pub when: String,
    pub transcript_path: Option<PathBuf>,
    /// The hit's resume command, copied when the native resume cannot be built.
    pub command: String,
}

pub enum Target {
    Jump(String),
    Resume(ResumeSpec),
    CopyOnly(String),
    Nothing,
}

/// `live` says whether an AoE instance id is still listed; `bound` names the
/// listed instance running a conversation id, which catches panes the hit
/// carries no AoE link for (Pi conversations, notably).
pub fn resolve(
    s: &Session,
    live: &dyn Fn(&str) -> bool,
    bound: &dyn Fn(&str) -> Option<String>,
    profiles: &[ProfileHomes],
    ledger_home: &Path,
) -> Target {
    if let Some(id) = s.aoe_instance_id.as_deref().filter(|id| live(id)) {
        return Target::Jump(id.to_string());
    }
    let resume_id = s.resume.as_ref().and_then(|r| r.session_id.as_deref());
    if let Some(id) = [s.harness_session_id.as_deref(), resume_id]
        .into_iter()
        .flatten()
        .find_map(bound)
    {
        return Target::Jump(id);
    }
    let Some(resume) = &s.resume else {
        return Target::Nothing;
    };
    match native_resume(s, resume, profiles, ledger_home) {
        Some(spec) => Target::Resume(spec),
        None => Target::CopyOnly(resume.command.clone()),
    }
}

fn native_resume(
    s: &Session,
    resume: &Resume,
    profiles: &[ProfileHomes],
    ledger_home: &Path,
) -> Option<ResumeSpec> {
    let tool = match s.harness.as_str() {
        "claude" => "claude",
        "pi" => "pi",
        _ => return None,
    };
    let (session_id, cwd, profile) = (
        resume.session_id.as_deref()?,
        s.cwd.as_deref()?,
        s.profile.as_deref()?,
    );
    if !crate::session::capture::is_valid_session_id(session_id) {
        return None;
    }
    // An empty or multi-part profile would own every Ledger home.
    let mut parts = Path::new(profile).components();
    if !matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    ) {
        return None;
    }
    // A ledger-parent hit's own transcript_path is the run's file, not the parent's.
    let transcript_path = match (&resume.transcript_path, resume.kind.as_str()) {
        (Some(path), _) => Some(path),
        (None, "ledger-parent") => return None,
        (None, _) => s.transcript_path.as_ref(),
    }
    .map(PathBuf::from);
    // Pi resumes only from its exact transcript.
    if tool == "pi" && transcript_path.is_none() {
        return None;
    }
    let owned = ledger_home.join("homes").join(profile);
    let (aoe_profile, store) = profiles.iter().find_map(|p| {
        let (_, dir) = p
            .tool_dirs
            .iter()
            .find(|(t, d)| t == tool && d.starts_with(&owned))?;
        Some((p.profile.clone(), dir.clone()))
    })?;
    // A first prompt makes a poor pane name; the folder is what AoE names panes by.
    let when = chrono::DateTime::parse_from_rfc3339(&s.last_ts)
        .map(|t| t.with_timezone(&chrono::Local).format("%m-%d").to_string())
        .unwrap_or_default();
    let folder = Path::new(cwd)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("resumed {}", session_id.chars().take(8).collect::<String>()));
    let title = format!("{folder} {when}").trim_end().to_string();
    Some(ResumeSpec {
        tool: tool.to_string(),
        cwd: cwd.to_string(),
        session_id: session_id.to_string(),
        aoe_profile,
        store,
        title,
        when,
        transcript_path,
        command: resume.command.clone(),
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
            matches!(resolve(&hits[0].session, &|id| id == "aoe1", &|_| None, &profiles, home), Target::Jump(id) if id == "aoe1")
        );
        match resolve(&hits[0].session, &|_| false, &|_| None, &profiles, home) {
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
                assert!(r.command.ends_with("--resume c-111"));
                // Named by its folder and day, not its first prompt.
                assert!(r.title.starts_with("deploy 10-2"), "{}", r.title);
                assert_eq!(r.when.len(), 5);
            }
            _ => panic!("expected resume"),
        }
        assert!(
            matches!(resolve(&hits[0].session, &|_| false, &|_| None, &[], home), Target::CopyOnly(c) if c.contains("--resume c-111"))
        );
        // A pane AoE runs the conversation in is jumped to, even with no AoE link
        // on the hit; a subagent hit jumps to its parent's pane.
        let bound = |id: &str| (id == "p-222").then(|| "pane9".to_string());
        for hit in &hits[1..] {
            assert!(
                matches!(resolve(&hit.session, &|_| false, &bound, &profiles, home), Target::Jump(id) if id == "pane9")
            );
        }
        // Pi resumes only from its exact transcript.
        match resolve(&hits[1].session, &|_| false, &|_| None, &profiles, home) {
            Target::Resume(r) => {
                assert_eq!((r.tool.as_str(), r.session_id.as_str()), ("pi", "p-222"));
                assert_eq!(
                    r.transcript_path.as_deref(),
                    hits[1].session.transcript_path.as_deref().map(Path::new)
                );
            }
            _ => panic!("expected resume"),
        }
        let mut pi = crate::session_search::query::parse(include_bytes!("testdata/query_v1.json"))
            .unwrap()
            .remove(1)
            .session;
        pi.transcript_path = None;
        assert!(matches!(
            resolve(&pi, &|_| false, &|_| None, &profiles, home),
            Target::CopyOnly(_)
        ));
        // A subagent hit resumes its parent; its own transcript_path is the run's file.
        let mut run = crate::session_search::query::parse(include_bytes!("testdata/query_v1.json"))
            .unwrap()
            .remove(2)
            .session;
        run.transcript_path = Some("/home/u/.ledger/homes/personal/pi/runs/r-333.jsonl".into());
        match resolve(&run, &|_| false, &|_| None, &profiles, home) {
            Target::Resume(r) => {
                assert_eq!(r.session_id, "p-222");
                assert_eq!(
                    r.transcript_path.as_deref(),
                    hits[1].session.transcript_path.as_deref().map(Path::new)
                );
            }
            _ => panic!("expected the parent's resume"),
        }
        run.resume.as_mut().unwrap().transcript_path = None;
        assert!(matches!(
            resolve(&run, &|_| false, &|_| None, &profiles, home),
            Target::CopyOnly(c) if c.contains("--session p-222")
        ));
        let mut bare =
            crate::session_search::query::parse(include_bytes!("testdata/query_v1.json"))
                .unwrap()
                .remove(0)
                .session;
        bare.resume = None;
        assert!(matches!(
            resolve(&bare, &|_| false, &|_| None, &profiles, home),
            Target::Nothing
        ));
    }

    #[test]
    fn native_resume_needs_a_plain_profile_and_a_valid_id() {
        let home = Path::new("/home/u/.ledger");
        let profiles = vec![ProfileHomes {
            profile: "oss".into(),
            tool_dirs: vec![
                (
                    "claude".into(),
                    home.join("homes/personal-oss/claude-code/runtime"),
                ),
                ("pi".into(), home.join("homes/personal-oss/pi/runtime")),
            ],
        }];
        let hit = || {
            crate::session_search::query::parse(include_bytes!("testdata/query_v1.json"))
                .unwrap()
                .remove(0)
                .session
        };
        for profile in ["personal", "", ".", "..", "personal-oss/claude-code"] {
            let mut s = hit();
            s.profile = Some(profile.into());
            assert!(
                matches!(
                    resolve(&s, &|_| false, &|_| None, &profiles, home),
                    Target::CopyOnly(_)
                ),
                "profile {profile:?}"
            );
        }
        for tool in ["claude", "pi"] {
            let mut s = hit();
            s.profile = Some("personal-oss".into());
            s.harness = tool.into();
            assert!(
                matches!(
                    resolve(&s, &|_| false, &|_| None, &profiles, home),
                    Target::Resume(_)
                ),
                "{tool}"
            );
            s.resume.as_mut().unwrap().session_id = Some("-rf".into());
            assert!(
                matches!(
                    resolve(&s, &|_| false, &|_| None, &profiles, home),
                    Target::CopyOnly(_)
                ),
                "{tool}"
            );
        }
    }
}
