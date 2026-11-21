//! The stenographer's derived arc: `<live>/<profile>/<harness>/<session>/derive/arc.json`,
//! written beside the session's record without a model call (attic CRA-1821).

use super::WorkArc;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StenoArc {
    pub arc: WorkArc,
    /// The record's own stamp, for example "covered through w31 (turns 301-310); reduced w1-w31".
    pub covered: String,
    /// When the file was written.
    pub at: DateTime<Utc>,
    /// Written after the transcript last changed, so no turn is missing from it.
    pub current: bool,
    pub path: PathBuf,
}

impl StenoArc {
    pub(crate) fn status(&self) -> super::ArcStatus {
        if self.current {
            super::ArcStatus::Current
        } else {
            super::ArcStatus::Stale
        }
    }
}

#[derive(Deserialize)]
struct ArcFile {
    arc_version: u32,
    arc: serde_json::Value,
    covered: Covered,
}

#[derive(Deserialize)]
struct Covered {
    text: String,
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct RecordState {
    transcript: Option<PathBuf>,
}

/// The stenographer's harness directory for an AoE tool, wrapped tools (`claude-hr`) included.
fn harness(tool: &str) -> Option<&'static str> {
    ["claude", "pi", "codex"].into_iter().find(|name| {
        tool.strip_prefix(name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    })
}

/// One path segment the stenographer accepts as a session id; never `.` or `..`.
fn valid_session_id(id: &str) -> bool {
    id.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Whether the session can have a record: a recorded harness and a native conversation id.
pub(crate) fn readable(session: &crate::session::Instance) -> bool {
    harness(&session.tool).is_some()
        && session
            .agent_session_id
            .as_deref()
            .is_some_and(valid_session_id)
}

pub(crate) fn live_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".local/share/steno-replay/live"))
}

/// The newest valid `derive/arc.json` for the session under any profile in `root`.
pub(crate) fn read(root: &Path, tool: &str, session_id: &str) -> Option<StenoArc> {
    let harness = harness(tool)?;
    if !valid_session_id(session_id) {
        return None;
    }
    let (at, record) = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|profile| {
            let record = profile.path().join(harness).join(session_id);
            let modified = std::fs::metadata(record.join("derive/arc.json"))
                .ok()?
                .modified()
                .ok()?;
            Some((DateTime::<Utc>::from(modified), record))
        })
        .max()?;
    load(&record, session_id, at)
}

fn load(record: &Path, session_id: &str, at: DateTime<Utc>) -> Option<StenoArc> {
    let path = record.join("derive/arc.json");
    let file = std::fs::File::open(&path).ok()?;
    if file.metadata().ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let doc: ArcFile = serde_json::from_reader(std::io::BufReader::new(file)).ok()?;
    if doc.arc_version != 1
        || doc
            .covered
            .session_id
            .as_deref()
            .is_some_and(|id| id != session_id)
    {
        return None;
    }
    let arc = WorkArc::from_json(&doc.arc.to_string()).ok()?;
    let transcript_changed = std::fs::read(record.join("state.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<RecordState>(&raw).ok())
        .and_then(|state| state.transcript)
        .and_then(|transcript| std::fs::metadata(transcript).ok()?.modified().ok())
        .map(DateTime::<Utc>::from);
    Some(StenoArc {
        arc,
        covered: doc.covered.text,
        at,
        current: transcript_changed.is_some_and(|changed| at >= changed),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    const ARC: &str = r#"{"goal":"Ship the reducer [w5]","now":"Ran the suite [w6]","remaining":"Unknown","next":"Deploy [w6]"}"#;

    fn record(root: &Path, profile: &str, harness: &str, sid: &str, transcript: &Path) -> PathBuf {
        let record = root.join(profile).join(harness).join(sid);
        std::fs::create_dir_all(record.join("derive")).unwrap();
        std::fs::write(
            record.join("state.json"),
            serde_json::json!({ "transcript": transcript }).to_string(),
        )
        .unwrap();
        record
    }

    fn write_arc(record: &Path, body: &str, modified: SystemTime) {
        let path = record.join("derive/arc.json");
        std::fs::write(&path, body).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn doc(sid: &str, arc: &str) -> String {
        format!(
            r#"{{"arc_version":1,"arc":{arc},"covered":{{"session_id":"{sid}","text":"covered through w6 (turns 51-53, partial); reduced w1-w6","window":6}},"words":11}}"#
        )
    }

    #[test]
    fn reads_the_newest_record_and_compares_it_with_the_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let transcript = root.join("t.jsonl");
        std::fs::write(&transcript, "").unwrap();
        let touched = std::fs::metadata(&transcript).unwrap().modified().unwrap();
        let old = record(root, "work", "pi", "s1", &transcript);
        let new = record(root, "personal", "pi", "s1", &transcript);
        write_arc(&old, &doc("s1", ARC), touched - Duration::from_secs(60));
        write_arc(
            &new,
            &doc("s1", &ARC.replace("Ship", "Land")),
            touched - Duration::from_secs(5),
        );

        let stale = read(root, "pi-hr", "s1").unwrap();
        assert_eq!(stale.arc.goal, "Land the reducer [w5]");
        assert_eq!(stale.path, new.join("derive/arc.json"));
        assert!(stale.covered.starts_with("covered through w6"));
        assert!(!stale.current, "written before the last turn");

        write_arc(&new, &doc("s1", ARC), touched + Duration::from_secs(5));
        assert!(read(root, "pi", "s1").unwrap().current);
    }

    #[test]
    fn rejects_what_aoe_would_not_render() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let transcript = root.join("t.jsonl");
        let record = record(root, "personal", "claude", "s1", &transcript);
        let now = SystemTime::now();
        let words = vec!["word"; 81].join(" ");
        for (case, body) in [
            (
                "version",
                doc("s1", ARC).replace(r#""arc_version":1"#, r#""arc_version":2"#),
            ),
            ("foreign session", doc("s2", ARC)),
            (
                "extra field",
                doc("s1", &ARC.replace('}', r#","model":"x"}"#)),
            ),
            ("over 80 words", doc("s1", &ARC.replace("Unknown", &words))),
            ("not json", "{".into()),
        ] {
            write_arc(&record, &body, now);
            assert!(read(root, "claude", "s1").is_none(), "{case}");
        }
        write_arc(&record, &doc("s1", ARC), now);
        assert!(read(root, "claude", "s1").is_some());
        for (tool, sid) in [
            ("gemini", "s1"),
            ("claude", ".."),
            ("claude", "a/b"),
            ("claudex", "s1"),
        ] {
            assert!(read(root, tool, sid).is_none(), "{tool} {sid}");
        }
    }
}
