//! The turns around a hit, read straight from its transcript, so the preview
//! can show the exchange a snippet came from. Reads Claude and Pi JSONL.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// How far either side of a hit to read; records cut by the window are dropped.
const WINDOW: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// A tool call, as `name  argument`.
    Tool,
    ToolResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub ts: Option<String>,
    pub text: String,
    /// From the record the hit's span falls in.
    pub hit: bool,
}

/// Up to `before` turns ahead of the record holding byte `start`, that
/// record's turns, and up to `after` turns behind it. Tool results and
/// harness-injected user text show only when they are the hit.
pub fn around(path: &Path, start: u64, before: usize, after: usize) -> std::io::Result<Vec<Turn>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = start.min(len);
    let from = start.saturating_sub(WINDOW);
    let to = start.saturating_add(WINDOW).min(len);
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity((to - from) as usize);
    file.take(to - from).read_to_end(&mut buf)?;
    Ok(turns_around(
        &buf,
        (start - from) as usize,
        from == 0,
        to == len,
        before,
        after,
    ))
}

fn turns_around(
    buf: &[u8],
    at: usize,
    whole_start: bool,
    whole_end: bool,
    before: usize,
    after: usize,
) -> Vec<Turn> {
    let at = at.min(buf.len());
    let line_start = buf[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let line_end = buf[at..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(buf.len(), |i| at + i);

    let mut earlier: Vec<&[u8]> = buf[..line_start.saturating_sub(1)]
        .split(|&b| b == b'\n')
        .collect();
    if !whole_start && !earlier.is_empty() {
        earlier.remove(0);
    }
    let mut later: Vec<&[u8]> = buf[(line_end + 1).min(buf.len())..]
        .split(|&b| b == b'\n')
        .collect();
    if !whole_end {
        later.pop();
    }

    let mut ahead: Vec<Turn> = earlier
        .iter()
        .rev()
        .flat_map(|line| record_turns(line, false).into_iter().rev())
        .take(before)
        .collect();
    ahead.reverse();
    let mut out = ahead;
    out.extend(record_turns(&buf[line_start..line_end], true));
    out.extend(
        later
            .iter()
            .flat_map(|line| record_turns(line, false))
            .take(after),
    );
    out
}

fn record_turns(line: &[u8], hit: bool) -> Vec<Turn> {
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    if !hit && v["isMeta"].as_bool() == Some(true) {
        return Vec::new();
    }
    let msg = &v["message"];
    let role = match (v["type"].as_str(), msg["role"].as_str()) {
        (Some("user"), _) | (Some("message"), Some("user")) => Role::User,
        (Some("assistant"), _) | (Some("message"), Some("assistant")) => Role::Assistant,
        (Some("message"), Some("toolResult")) => Role::ToolResult,
        _ => return Vec::new(),
    };
    let ts = v["timestamp"].as_str().map(str::to_string);
    let mut parts: Vec<(Role, String)> = Vec::new();
    match &msg["content"] {
        Value::String(s) => parts.push((role, s.clone())),
        Value::Array(items) => {
            for item in items {
                match item["type"].as_str() {
                    Some("text") => parts.push((role, str_of(&item["text"]))),
                    Some("tool_use") => parts.push((Role::Tool, tool_line(item, "input"))),
                    Some("toolCall") => parts.push((Role::Tool, tool_line(item, "arguments"))),
                    Some("tool_result") => {
                        parts.push((Role::ToolResult, result_text(&item["content"])))
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    parts
        .into_iter()
        .filter(|(role, text)| {
            !text.trim().is_empty()
                && (hit
                    || match role {
                        Role::ToolResult => false,
                        // System reminders, slash-command echoes and the like.
                        Role::User => !text.trim_start().starts_with('<'),
                        _ => true,
                    })
        })
        .map(|(role, text)| Turn {
            role,
            ts: ts.clone(),
            text,
            hit,
        })
        .collect()
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// `name  argument`, the argument being the field that says what the call did.
fn tool_line(item: &Value, args_key: &str) -> String {
    let name = item["name"].as_str().unwrap_or("tool");
    let args = &item[args_key];
    let arg = [
        "command",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "description",
        "prompt",
    ]
    .iter()
    .find_map(|k| args[*k].as_str())
    .map_or_else(|| args.to_string(), str::to_string);
    format!("{name}  {arg}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(records: &[&str]) -> (Vec<u8>, Vec<usize>) {
        let mut buf = Vec::new();
        let mut starts = Vec::new();
        for r in records {
            starts.push(buf.len());
            buf.extend_from_slice(r.as_bytes());
            buf.push(b'\n');
        }
        (buf, starts)
    }

    #[test]
    fn claude_exchange_around_a_hit() {
        let (buf, starts) = lines(&[
            r#"{"type":"user","timestamp":"t0","message":{"role":"user","content":"first ask"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"meta noise"}}"#,
            r#"{"type":"assistant","timestamp":"t1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"on it"},{"type":"tool_use","name":"Bash","input":{"command":"git status"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"clean"}]}}"#,
            r#"{"type":"user","timestamp":"t2","message":{"role":"user","content":"the margin please"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}"#,
            r#"{"type":"assistant","timestamp":"t3","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
        ]);
        let turns = turns_around(&buf, starts[4] + 10, true, true, 3, 3);
        let got: Vec<(Role, &str, bool)> = turns
            .iter()
            .map(|t| (t.role, t.text.as_str(), t.hit))
            .collect();
        assert_eq!(
            got,
            [
                (Role::User, "first ask", false),
                (Role::Assistant, "on it", false),
                (Role::Tool, "Bash  git status", false),
                (Role::User, "the margin please", true),
                (Role::Assistant, "done", false),
            ]
        );
        assert_eq!(turns[3].ts.as_deref(), Some("t2"));
    }

    #[test]
    fn pi_tool_result_shows_only_as_the_hit() {
        let (buf, starts) = lines(&[
            r#"{"type":"session","id":"s"}"#,
            r#"{"type":"message","timestamp":"t0","message":{"role":"user","content":[{"type":"text","text":"run it"}]}}"#,
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"bash","arguments":{"command":"make"}}]}}"#,
            r#"{"type":"message","message":{"role":"toolResult","content":[{"type":"text","text":"build ok"}]}}"#,
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"built"}]}}"#,
        ]);
        let turns = turns_around(&buf, starts[3] + 40, true, true, 1, 1);
        let got: Vec<(Role, &str)> = turns.iter().map(|t| (t.role, t.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (Role::Tool, "bash  make"),
                (Role::ToolResult, "build ok"),
                (Role::Assistant, "built"),
            ]
        );
        let none = turns_around(&buf, starts[4], true, true, 3, 0);
        assert!(none.iter().all(|t| t.role != Role::ToolResult), "{none:?}");
    }

    #[test]
    fn records_cut_by_the_window_are_dropped() {
        let (buf, starts) = lines(&[
            r#"{"type":"user","message":{"role":"user","content":"cut head"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"kept"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"hit"}}"#,
        ]);
        // The window began part-way through the first record.
        let window = &buf[5..];
        let turns = turns_around(window, starts[2] - 5, false, true, 5, 5);
        let texts: Vec<&str> = turns.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["kept", "hit"]);
    }

    #[test]
    fn around_reads_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let (buf, starts) = lines(&[
            r#"{"type":"user","message":{"role":"user","content":"ask"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":"reply"}}"#,
        ]);
        std::fs::write(&path, &buf).unwrap();
        let turns = around(&path, starts[1] as u64, 2, 2).unwrap();
        assert_eq!(turns.len(), 2);
        assert!(turns[1].hit && !turns[0].hit);
        assert!(around(&dir.path().join("missing"), 0, 1, 1).is_err());
    }
}
