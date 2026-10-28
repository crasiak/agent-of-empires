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

/// Where to take a hit, and how.
pub struct Panes<'a> {
    /// Whether an AoE instance id is still listed.
    pub live: &'a dyn Fn(&str) -> bool,
    /// The listed pane running a conversation id right now.
    pub current: &'a dyn Fn(&str) -> Option<String>,
    /// The listed pane whose thread ever ran a conversation id; it catches
    /// earlier iterations, before a `/clear` or compaction.
    pub hosted: &'a dyn Fn(&str) -> Option<String>,
    /// The live pane for work in a folder, given the folder and harness.
    pub folder: &'a dyn Fn(&str, &str) -> Option<String>,
}

/// A hit goes to the live session for its work: the pane whose thread ran
/// it, else a pane open in its folder. Only with no live pane does it resume.
/// With `exact`, it reopens the conversation itself instead, unless a pane is
/// running it already: one conversation never runs twice.
pub fn resolve(
    s: &Session,
    panes: &Panes,
    exact: bool,
    profiles: &[ProfileHomes],
    ledger_home: &Path,
) -> Target {
    let resume_id = s.resume.as_ref().and_then(|r| r.session_id.as_deref());
    let ids = [s.harness_session_id.as_deref(), resume_id];
    if let Some(id) = ids.into_iter().flatten().find_map(panes.current) {
        return Target::Jump(id);
    }
    if !exact {
        if let Some(id) = s.aoe_instance_id.as_deref().filter(|id| (panes.live)(id)) {
            return Target::Jump(id.to_string());
        }
        if let Some(id) = ids.into_iter().flatten().find_map(panes.hosted) {
            return Target::Jump(id);
        }
        if let Some(id) = s
            .cwd
            .as_deref()
            .and_then(|cwd| (panes.folder)(cwd, &s.harness))
        {
            return Target::Jump(id);
        }
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

    fn none(_: &str) -> Option<String> {
        None
    }

    fn no_folder(_: &str, _: &str) -> Option<String> {
        None
    }

    const NO_PANES: Panes<'static> = Panes {
        live: &|_| false,
        current: &none,
        hosted: &none,
        folder: &no_folder,
    };

    fn panes<'a>(
        live: &'a dyn Fn(&str) -> bool,
        current: &'a dyn Fn(&str) -> Option<String>,
        hosted: &'a dyn Fn(&str) -> Option<String>,
    ) -> Panes<'a> {
        Panes {
            live,
            current,
            hosted,
            folder: &no_folder,
        }
    }

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
            matches!(resolve(&hits[0].session, &panes(&|id| id == "aoe1", &none, &none), false, &profiles, home), Target::Jump(id) if id == "aoe1")
        );
        match resolve(&hits[0].session, &NO_PANES, false, &profiles, home) {
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
            matches!(resolve(&hits[0].session, &NO_PANES, false, &[], home), Target::CopyOnly(c) if c.contains("--resume c-111"))
        );
        // A pane AoE runs the conversation in is jumped to, even with no AoE link
        // on the hit; a subagent hit jumps to its parent's pane.
        let pane9 = |id: &str| (id == "p-222").then(|| "pane9".to_string());
        for hit in &hits[1..] {
            for p in [
                panes(&|_| false, &pane9, &none),
                panes(&|_| false, &none, &pane9),
            ] {
                assert!(
                    matches!(resolve(&hit.session, &p, false, &profiles, home), Target::Jump(id) if id == "pane9")
                );
            }
        }
        // No recorded owner: the live pane in the hit's folder, unless exact.
        let in_folder = Panes {
            folder: &|cwd, harness| {
                (cwd.ends_with("/deploy") && harness == "claude").then(|| "here".to_string())
            },
            ..NO_PANES
        };
        assert!(
            matches!(resolve(&hits[0].session, &in_folder, false, &profiles, home), Target::Jump(id) if id == "here")
        );
        assert!(matches!(
            resolve(&hits[0].session, &in_folder, true, &profiles, home),
            Target::Resume(_)
        ));
        // Exact reopens an earlier iteration, but never a conversation a pane runs now.
        assert!(matches!(
            resolve(
                &hits[1].session,
                &panes(&|_| false, &none, &pane9),
                true,
                &profiles,
                home
            ),
            Target::Resume(_)
        ));
        assert!(
            matches!(resolve(&hits[1].session, &panes(&|_| false, &pane9, &none), true, &profiles, home), Target::Jump(id) if id == "pane9")
        );
        assert!(matches!(
            resolve(
                &hits[0].session,
                &panes(&|_| true, &none, &none),
                true,
                &profiles,
                home
            ),
            Target::Resume(_)
        ));
        // Pi resumes only from its exact transcript.
        match resolve(&hits[1].session, &NO_PANES, false, &profiles, home) {
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
            resolve(&pi, &NO_PANES, false, &profiles, home),
            Target::CopyOnly(_)
        ));
        // A subagent hit resumes its parent; its own transcript_path is the run's file.
        let mut run = crate::session_search::query::parse(include_bytes!("testdata/query_v1.json"))
            .unwrap()
            .remove(2)
            .session;
        run.transcript_path = Some("/home/u/.ledger/homes/personal/pi/runs/r-333.jsonl".into());
        match resolve(&run, &NO_PANES, false, &profiles, home) {
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
            resolve(&run, &NO_PANES, false, &profiles, home),
            Target::CopyOnly(c) if c.contains("--session p-222")
        ));
        let mut bare =
            crate::session_search::query::parse(include_bytes!("testdata/query_v1.json"))
                .unwrap()
                .remove(0)
                .session;
        bare.resume = None;
        assert!(matches!(
            resolve(&bare, &NO_PANES, false, &profiles, home),
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
                    resolve(&s, &NO_PANES, false, &profiles, home),
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
                    resolve(&s, &NO_PANES, false, &profiles, home),
                    Target::Resume(_)
                ),
                "{tool}"
            );
            s.resume.as_mut().unwrap().session_id = Some("-rf".into());
            assert!(
                matches!(
                    resolve(&s, &NO_PANES, false, &profiles, home),
                    Target::CopyOnly(_)
                ),
                "{tool}"
            );
        }
    }
}
