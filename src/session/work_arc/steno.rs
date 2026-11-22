//! The stenographer's derived arc: `<live>/<profile>/<harness>/<session>/derive/arc.json`,
//! written beside the session's record without a model call (attic CRA-1821).

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 64 * 1024;
const ARC_VERSION: u32 = 2;
const MAX_LINES_BYTES: usize = 2048;
const MAX_LINES_WORDS: usize = 120;

/// The four lines in display order; each is cited statements or "Unknown".
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArcLines {
    pub goal: String,
    pub recent: String,
    pub pending: String,
    pub notes: String,
}

impl ArcLines {
    fn validate(raw: serde_json::Value) -> Option<Self> {
        if raw.to_string().len() > MAX_LINES_BYTES {
            return None;
        }
        let lines: Self = serde_json::from_value(raw).ok()?;
        let fields = lines.labelled();
        let words: usize = fields
            .iter()
            .map(|(_, v)| v.split_whitespace().count())
            .sum();
        (words <= MAX_LINES_WORDS
            && fields
                .iter()
                .all(|(_, v)| !v.trim().is_empty() && !v.chars().any(|c| c.is_control())))
        .then_some(lines)
    }

    pub(crate) fn labelled(&self) -> [(&'static str, &str); 4] {
        [
            ("Goal", self.goal.as_str()),
            ("Recent", self.recent.as_str()),
            ("Pending", self.pending.as_str()),
            ("Notes", self.notes.as_str()),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StenoArc {
    pub lines: ArcLines,
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

    /// The record directory `derive/arc.json` sits in.
    fn record(&self) -> Option<&Path> {
        self.path.parent()?.parent()
    }

    /// The popup text for the cited windows, in order; an unreadable window says so.
    pub(crate) fn windows_text(&self, windows: &[u32]) -> String {
        let mut out = Vec::new();
        for &window in windows {
            match self.record().and_then(|record| read_window(record, window)) {
                Some(view) => out.push(view.text()),
                None => out.push(format!("w{window}: not readable in the record")),
            }
        }
        out.join("\n\n────────\n\n")
    }
}

const MAX_WINDOW_BYTES: u64 = 512 * 1024;
/// A cite range wider than this is not expanded.
const MAX_CITED_WINDOWS: u32 = 50;

static CITE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    let one = r"w\d+(?:\s*[-–]\s*w?\d+)?";
    regex::Regex::new(&format!(r"\[\s*({one}(?:\s*,\s*{one})*)\s*\]")).unwrap()
});

/// The cites in `text`: each one's byte span and the windows it names, ranges expanded.
pub(crate) fn cite_spans(text: &str) -> Vec<(std::ops::Range<usize>, Vec<u32>)> {
    CITE.captures_iter(text)
        .map(|caps| {
            let mut windows = Vec::new();
            for part in caps[1].split(',') {
                let digits = |s: &str| s.trim().trim_start_matches('w').parse::<u32>().ok();
                let (lo, hi) = match part.split_once(['-', '–']) {
                    Some((a, b)) => (digits(a), digits(b)),
                    None => (digits(part), digits(part)),
                };
                if let (Some(lo), Some(hi)) = (lo, hi) {
                    let (lo, hi) = (lo.min(hi), lo.max(hi));
                    windows.extend(lo..=hi.min(lo + MAX_CITED_WINDOWS - 1));
                }
            }
            windows.dedup();
            (caps.get(0).unwrap().range(), windows)
        })
        .collect()
}

/// One landed window of the record, as the stenographer wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowView {
    pub window: u32,
    pub turns: Option<(u64, u64)>,
    pub partial: bool,
    pub narrative: String,
    /// Checked statements under each heading, in the window's order.
    pub statements: Vec<(&'static str, Vec<String>)>,
    /// Statements left out because their check did not pass.
    pub unchecked: usize,
}

impl WindowView {
    fn text(&self) -> String {
        let turns = self
            .turns
            .map(|(a, b)| format!(" · turns {a}-{b}"))
            .unwrap_or_default();
        let partial = if self.partial { " (partial)" } else { "" };
        let mut out = format!("w{}{turns}{partial}\n\n{}\n", self.window, self.narrative);
        for (heading, items) in &self.statements {
            out.push_str(&format!("\n{heading}:\n"));
            for item in items {
                out.push_str(&format!("- {item}\n"));
            }
        }
        if self.unchecked > 0 {
            out.push_str(&format!(
                "\n{} statement(s) left out: their check did not pass.\n",
                self.unchecked
            ));
        }
        out
    }
}

pub(crate) fn read_window(record: &Path, window: u32) -> Option<WindowView> {
    let file = std::fs::File::open(record.join(format!("windows/{window:02}.json"))).ok()?;
    if file.metadata().ok()?.len() > MAX_WINDOW_BYTES {
        return None;
    }
    let doc: serde_json::Value = serde_json::from_reader(std::io::BufReader::new(file)).ok()?;
    if doc.get("window")?.as_u64()? != u64::from(window) {
        return None;
    }
    let turns = doc.get("turns").and_then(|t| {
        let t = t.as_array()?;
        Some((t.first()?.as_u64()?, t.last()?.as_u64()?))
    });
    let mut unchecked = 0;
    let mut statements = Vec::new();
    for (key, heading) in [
        ("intent", "Intent"),
        ("progress", "Progress"),
        ("decisions", "Decisions"),
        ("unresolved", "Unresolved"),
        ("next", "Next"),
    ] {
        let mut items = Vec::new();
        for st in doc["fields"][key].as_array().into_iter().flatten() {
            let text = st["text"].as_str().unwrap_or("").trim();
            let ok = st["verdicts"]
                .as_array()
                .is_some_and(|v| v.len() == 1 && v[0] == "ok");
            // "None." is how the model fills an empty field.
            if text.is_empty() || matches!(text.trim_end_matches('.'), "None" | "none" | "N/A") {
                continue;
            }
            if ok {
                items.push(text.to_string());
            } else {
                unchecked += 1;
            }
        }
        if !items.is_empty() {
            statements.push((heading, items));
        }
    }
    Some(WindowView {
        window,
        turns,
        partial: doc["partial"].as_bool().unwrap_or(false),
        narrative: doc["narrative"].as_str().unwrap_or("").trim().to_string(),
        statements,
        unchecked,
    })
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
    if doc.arc_version != ARC_VERSION
        || doc
            .covered
            .session_id
            .as_deref()
            .is_some_and(|id| id != session_id)
    {
        return None;
    }
    let lines = ArcLines::validate(doc.arc)?;
    let transcript_changed = std::fs::read(record.join("state.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<RecordState>(&raw).ok())
        .and_then(|state| state.transcript)
        .and_then(|transcript| std::fs::metadata(transcript).ok()?.modified().ok())
        .map(DateTime::<Utc>::from);
    Some(StenoArc {
        lines,
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

    const ARC: &str = r#"{"goal":"Ship the reducer [w5]","recent":"Ran the suite [w6]","pending":"Deploy [w6]","notes":"Unknown"}"#;

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
            r#"{{"arc_version":2,"arc":{arc},"covered":{{"session_id":"{sid}","text":"covered through w6 (turns 51-53, partial); reduced w1-w6","window":6}},"words":11}}"#
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
        assert_eq!(stale.lines.goal, "Land the reducer [w5]");
        assert_eq!(stale.path, new.join("derive/arc.json"));
        assert!(stale.covered.starts_with("covered through w6"));
        assert!(!stale.current, "written before the last turn");

        write_arc(&new, &doc("s1", ARC), touched + Duration::from_secs(5));
        assert!(read(root, "pi", "s1").unwrap().current);
    }

    #[test]
    fn cites_expand_ranges_and_keep_their_spans() {
        let text = "Ship it [w1-w3, w6]; then [w9]. Not [x1] or [w].";
        let spans = cite_spans(text);
        assert_eq!(spans.len(), 2);
        assert_eq!(&text[spans[0].0.clone()], "[w1-w3, w6]");
        assert_eq!(spans[0].1, [1, 2, 3, 6]);
        assert_eq!(spans[1].1, [9]);
        assert_eq!(
            cite_spans("[w5-w1000]")[0].1.len(),
            MAX_CITED_WINDOWS as usize
        );
        for (text, expected) in [
            ("[w4294967295]", vec![u32::MAX]),
            ("[w4294967294-w4294967295]", vec![u32::MAX - 1, u32::MAX]),
            ("[w4294967295-w4294967294]", vec![u32::MAX - 1, u32::MAX]),
        ] {
            let spans = cite_spans(text);
            assert_eq!(spans.len(), 1, "{text}");
            assert_eq!(spans[0].1, expected, "{text}");
        }
    }

    #[test]
    fn a_window_popup_shows_the_narrative_and_only_checked_statements() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("personal/claude/s1");
        std::fs::create_dir_all(record.join("windows")).unwrap();
        std::fs::create_dir_all(record.join("derive")).unwrap();
        let ok = |t: &str| serde_json::json!({"text": t, "cites": [3], "verdicts": ["ok"]});
        std::fs::write(
            record.join("windows/07.json"),
            serde_json::json!({
                "window": 7, "turns": [61, 70], "partial": false,
                "narrative": "The assistant fixed the parser.",
                "fields": {
                    "intent": [ok("Fix the parser")],
                    "progress": [ok("Ran the suite"),
                                 {"text": "Merged it", "cites": [], "verdicts": ["uncited"]}],
                    "unresolved": [ok("None.")],
                    "next": [ok("Open the PR")]
                }
            })
            .to_string(),
        )
        .unwrap();
        let arc = StenoArc {
            lines: ArcLines {
                goal: "g [w7]".into(),
                recent: "r [w7]".into(),
                pending: "p [w7]".into(),
                notes: "Unknown".into(),
            },
            covered: String::new(),
            at: Utc::now(),
            current: true,
            path: record.join("derive/arc.json"),
        };
        let text = arc.windows_text(&[7, 8]);
        assert!(
            text.starts_with("w7 · turns 61-70\n\nThe assistant fixed the parser."),
            "{text}"
        );
        assert!(text.contains("Progress:\n- Ran the suite\n"), "{text}");
        assert!(text.contains("Next:\n- Open the PR"), "{text}");
        assert!(
            !text.contains("Merged it") && !text.contains("Unresolved"),
            "{text}"
        );
        assert!(text.contains("1 statement(s) left out"), "{text}");
        assert!(text.ends_with("w8: not readable in the record"), "{text}");
    }

    #[test]
    fn rejects_what_aoe_would_not_render() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let transcript = root.join("t.jsonl");
        let record = record(root, "personal", "claude", "s1", &transcript);
        let now = SystemTime::now();
        let words = vec!["word"; 121].join(" ");
        for (case, body) in [
            (
                "version",
                doc("s1", ARC).replace(r#""arc_version":2"#, r#""arc_version":1"#),
            ),
            ("foreign session", doc("s2", ARC)),
            (
                "extra field",
                doc("s1", &ARC.replace('}', r#","model":"x"}"#)),
            ),
            ("over 120 words", doc("s1", &ARC.replace("Unknown", &words))),
            ("blank line", doc("s1", &ARC.replace("Unknown", " "))),
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
