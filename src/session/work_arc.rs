//! One opted-in Pi Agent conversation's bounded work-arc source and derived cue.

mod source;
mod state;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) use state::*;

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 24_000;
const MAX_INPUT_BYTES: usize = 24_000;
const MAX_FIRST_BYTES: usize = 2_048;
const MAX_ARC_BYTES: usize = 1_024;
const MAX_WIRE_BYTES: usize = MAX_INPUT_BYTES * 6 + 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArcInput {
    pub covered_leaf: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkArc {
    pub goal: String,
    pub now: String,
    pub remaining: String,
    pub next: String,
}

impl WorkArc {
    pub(crate) fn from_json(raw: &str) -> Result<Self, &'static str> {
        if raw.len() > MAX_ARC_BYTES {
            return Err("arc exceeds byte limit");
        }
        let mut arc: Self = serde_json::from_str(raw).map_err(|_| "invalid arc fields")?;
        let mut words = 0;
        for field in [
            &mut arc.goal,
            &mut arc.now,
            &mut arc.remaining,
            &mut arc.next,
        ] {
            if field.chars().any(|c| c.is_control() && !c.is_whitespace()) {
                return Err("arc contains control characters");
            }
            *field = field.split_whitespace().collect::<Vec<_>>().join(" ");
            if field.is_empty() {
                return Err("arc field is empty");
            }
            words += field.split_whitespace().count();
        }
        if words > 80 {
            return Err("arc exceeds word limit");
        }
        Ok(arc)
    }
}

struct Entry {
    parent: Option<String>,
    message: Option<String>,
}

fn text_from_message(message: &Value) -> Option<String> {
    let role = message.get("role")?.as_str()?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let content = message.get("content")?;
    let text = match content {
        Value::String(text) => text.to_owned(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| format!("[{role}] {}", truncate_utf8(text, MAX_FIRST_BYTES)))
}

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Extract only visible user and assistant text on `leaf`'s exact path. This does not
/// authorize a file read: its caller must validate opt-in, host binding, cursor and path.
pub(crate) fn extract_arc_input(
    source: &[u8],
    expected_sid: &str,
    leaf: &str,
) -> Result<ArcInput, &'static str> {
    let started = std::time::Instant::now();
    if source.len() > MAX_SOURCE_BYTES || !source.ends_with(b"\n") || leaf.is_empty() {
        return Err("source incomplete or too large");
    }
    let mut rows = source
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty());
    let first = rows.next().ok_or("missing header")?;
    if first.len() > MAX_RECORD_BYTES {
        return Err("header exceeds byte limit");
    }
    let header: Value = serde_json::from_slice(first).map_err(|_| "invalid header")?;
    if header.get("type").and_then(Value::as_str) != Some("session")
        || header.get("id").and_then(Value::as_str) != Some(expected_sid)
    {
        return Err("foreign Pi session header");
    }
    let mut entries = HashMap::new();
    for (count, row) in rows.enumerate() {
        if started.elapsed() > std::time::Duration::from_millis(500)
            || count >= MAX_RECORDS
            || row.len() > MAX_RECORD_BYTES
        {
            return Err("source record limit exceeded");
        }
        let value: Value = serde_json::from_slice(row).map_err(|_| "invalid source record")?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or("missing entry id")?;
        if id.len() > 64 || id.is_empty() {
            return Err("invalid entry id");
        }
        let parent = match value.get("parentId") {
            Some(Value::Null) => None,
            Some(Value::String(parent)) => Some(parent.as_str()),
            _ => return Err("invalid parent id"),
        };
        if parent.is_some_and(|parent| parent.len() > 64) {
            return Err("invalid parent id");
        }
        let message = (value.get("type").and_then(Value::as_str) == Some("message"))
            .then(|| value.get("message").and_then(text_from_message))
            .flatten();
        if entries
            .insert(
                id.to_owned(),
                Entry {
                    parent: parent.map(str::to_owned),
                    message,
                },
            )
            .is_some()
        {
            return Err("duplicate entry id");
        }
    }
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(leaf);
    while let Some(id) = current {
        if started.elapsed() > std::time::Duration::from_millis(500) {
            return Err("source time budget exceeded");
        }
        if !seen.insert(id) {
            return Err("cyclic branch");
        }
        let entry = entries.get(id).ok_or("missing branch ancestor")?;
        if let Some(message) = &entry.message {
            path.push(message.as_str());
        }
        current = entry.parent.as_deref();
    }
    path.reverse();
    let first = path
        .iter()
        .position(|text| text.starts_with("[user]"))
        .ok_or("no useful initial exchange")?;
    if !path
        .iter()
        .skip(first + 1)
        .any(|text| text.starts_with("[assistant]"))
    {
        return Err("no useful initial exchange");
    }
    let mut text = String::from("[bounded excerpt: messages may be truncated or omitted; tool results and thinking excluded; claims are not independently verified]\n");
    text.push_str(path[first]);
    let mut recent = Vec::new();
    let mut remaining = MAX_INPUT_BYTES.saturating_sub(text.len());
    for (index, message) in path.iter().enumerate().rev() {
        let needed = message.len() + usize::from(!text.is_empty() || !recent.is_empty());
        if index == first || needed > remaining {
            continue;
        }
        recent.push(*message);
        remaining -= needed;
    }
    recent.reverse();
    for message in recent {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(message);
    }
    if text.is_empty() {
        return Err("no conversational text on branch");
    }
    Ok(ArcInput {
        covered_leaf: leaf.to_owned(),
        text,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceError {
    Disabled,
    Missing,
    Unavailable,
    Stale,
    Failed,
}

pub(crate) struct BoundArcInput {
    pub input: ArcInput,
    pub session_id: String,
    pub pi_id: String,
    pub launch_id: String,
    pub generation: u64,
    pub socket_path: std::path::PathBuf,
}

impl BoundArcInput {
    pub(crate) fn identity(&self) -> ArcIdentity {
        ArcIdentity {
            session_id: self.session_id.clone(),
            pi_id: self.pi_id.clone(),
            launch_id: self.launch_id.clone(),
            generation: self.generation,
            leaf: self.input.covered_leaf.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ArcFailure {
    pub error: SourceError,
    pub usage: Option<Value>,
}

impl From<SourceError> for ArcFailure {
    fn from(error: SourceError) -> Self {
        Self { error, usage: None }
    }
}

pub(crate) struct GeneratedArc {
    pub arc: WorkArc,
    pub selected_route: String,
    pub provider: String,
    pub model: String,
    pub usage: Option<Value>,
    pub bound: BoundArcInput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    session_id: String,
    source: String,
    leaf_id: Option<String>,
    settled: bool,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// The only filesystem path into the producer. No caller may discover a Pi file by
/// cwd, title or filename: admission starts with the observed pane publication.
pub(crate) fn read_arc_source(
    session: &crate::session::Instance,
) -> Result<BoundArcInput, SourceError> {
    use std::time::Duration;

    if !session.work_arc_enabled {
        return Err(SourceError::Disabled);
    }
    if session.is_structured() || session.is_sandboxed() {
        return Err(SourceError::Unavailable);
    }
    let active = session
        .active_execution
        .as_ref()
        .filter(|active| active.binding.agent == "pi" && active.container.is_none())
        .ok_or(SourceError::Unavailable)?;
    let sid = session
        .agent_session_id
        .as_deref()
        .ok_or(SourceError::Unavailable)?;
    let binding = session
        .agent_session_binding
        .as_ref()
        .filter(|binding| binding.is_known() && binding.session_id == sid)
        .ok_or(SourceError::Unavailable)?;
    if binding.execution.as_ref() != Some(&active.binding) {
        return Err(SourceError::Unavailable);
    }
    let source = session
        .pi_sidecar_source()
        .ok_or(SourceError::Unavailable)?;
    let Some(crate::session::instance::CaptureContext::Pi {
        source: expected,
        root,
    }) = &active.capture
    else {
        return Err(SourceError::Unavailable);
    };
    if &source != expected
        || session.is_capture_excluded(sid, Some(&active.binding))
        || !matches!(
            &session.resume_intent,
            crate::session::ResumeIntent::Default
        ) && !matches!(&session.resume_intent, crate::session::ResumeIntent::Use(pinned) if pinned == sid)
    {
        return Err(SourceError::Unavailable);
    }
    let read_publication = || {
        let published_sid = source
            .read_file(
                &session.id,
                &format!("session_id.{}", active.launch_id),
                128,
                None,
            )
            .ok_or(SourceError::Unavailable)?;
        if std::str::from_utf8(&published_sid).ok().map(str::trim) != Some(sid) {
            return Err(SourceError::Unavailable);
        }
        let path = source
            .read_file(
                &session.id,
                &format!("session_path.{}", active.launch_id),
                4096,
                None,
            )
            .ok_or(SourceError::Unavailable)?;
        let path = std::str::from_utf8(&path)
            .map_err(|_| SourceError::Unavailable)?
            .trim();
        let transcript = std::path::PathBuf::from(path);
        if !transcript.is_absolute()
            || !transcript.starts_with(root)
            || transcript == *root
            || crate::git::template::lexical_normalize(&transcript) != transcript
            || !transcript
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| name.ends_with(&format!("_{sid}.jsonl")))
        {
            return Err(SourceError::Unavailable);
        }
        Ok(transcript)
    };
    let transcript = read_publication()?;
    let leaf = format!("work_arc_cursor.{}", active.launch_id);
    let read_cursor = || {
        source
            .read_file(&session.id, &leaf, 512, Some(Duration::from_secs(120)))
            .ok_or(SourceError::Unavailable)
    };
    let cursor_bytes = read_cursor()?;
    let cursor: Cursor =
        serde_json::from_slice(&cursor_bytes).map_err(|_| SourceError::Unavailable)?;
    if cursor.version != 1 || cursor.session_id != sid || cursor.source != active.launch_id {
        return Err(SourceError::Unavailable);
    }
    let age = chrono::Utc::now().signed_duration_since(cursor.updated_at);
    if age < chrono::Duration::zero() || age > chrono::Duration::minutes(2) {
        return Err(SourceError::Stale);
    }
    if !cursor.settled {
        return Err(SourceError::Stale);
    }
    let leaf_id = cursor.leaf_id.as_deref().ok_or(SourceError::Missing)?;
    let bytes = source::read_owned_transcript(&transcript, sid)?;
    let input = extract_arc_input(&bytes, sid, leaf_id).map_err(|reason| match reason {
        "no useful initial exchange" => SourceError::Missing,
        "missing branch ancestor" => SourceError::Stale,
        _ => SourceError::Unavailable,
    })?;
    let socket_leaf = format!("work_arc_socket.{}", active.launch_id);
    let socket_bytes = source
        .read_file(&session.id, &socket_leaf, 512, None)
        .ok_or(SourceError::Unavailable)?;
    let socket_name = std::str::from_utf8(&socket_bytes).map_err(|_| SourceError::Unavailable)?;
    let socket_path = std::path::PathBuf::from(socket_name.trim());
    if !socket_path.is_absolute() || socket_path.as_os_str().len() > 99 {
        return Err(SourceError::Unavailable);
    }
    let socket_parent = socket_path.parent().ok_or(SourceError::Unavailable)?;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let dir = std::fs::symlink_metadata(socket_parent).map_err(|_| SourceError::Unavailable)?;
    let socket = std::fs::symlink_metadata(&socket_path).map_err(|_| SourceError::Unavailable)?;
    if !dir.file_type().is_dir()
        || dir.mode() & 0o077 != 0
        || dir.uid() != unsafe { libc::geteuid() }
        || !socket.file_type().is_socket()
        || socket.uid() != unsafe { libc::geteuid() }
    {
        return Err(SourceError::Unavailable);
    }
    if read_cursor()? != cursor_bytes || read_publication()? != transcript {
        return Err(SourceError::Stale);
    }
    Ok(BoundArcInput {
        input,
        session_id: session.id.clone(),
        pi_id: sid.to_owned(),
        launch_id: active.launch_id.clone(),
        generation: session.lifecycle_generation,
        socket_path,
    })
}

async fn exchange(
    bound: &BoundArcInput,
    operation: &str,
    deadline: std::time::Duration,
) -> Result<Value, SourceError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let nonce = uuid::Uuid::new_v4().to_string();
    if bound.input.text.len() > MAX_INPUT_BYTES {
        return Err(SourceError::Failed);
    }
    let mut envelope = serde_json::json!({
        "version": 1,
        "op": operation,
        "instance_id": bound.session_id,
        "session_id": bound.pi_id,
        "source": bound.launch_id,
        "leaf_id": bound.input.covered_leaf,
        "nonce": nonce,
    });
    if operation == "generate" {
        envelope["input"] = Value::String(bound.input.text.clone());
    }
    let mut request = serde_json::to_vec(&envelope).map_err(|_| SourceError::Failed)?;
    request.push(b'\n');
    if request.len() > MAX_WIRE_BYTES {
        return Err(SourceError::Failed);
    }
    let exchange = async {
        let mut stream = tokio::net::UnixStream::connect(&bound.socket_path)
            .await
            .map_err(|_| SourceError::Unavailable)?;
        stream
            .write_all(&request)
            .await
            .map_err(|_| SourceError::Failed)?;
        let mut response = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|_| SourceError::Failed)?;
            if n == 0 || response.len() + n > 4096 {
                return Err(SourceError::Failed);
            }
            response.extend_from_slice(&chunk[..n]);
            if let Some(end) = response.iter().position(|byte| *byte == b'\n') {
                response.truncate(end);
                break;
            }
        }
        let value: Value = serde_json::from_slice(&response).map_err(|_| SourceError::Failed)?;
        if value.get("nonce").and_then(Value::as_str) != Some(nonce.as_str()) {
            return Err(SourceError::Stale);
        }
        Ok(value)
    };
    tokio::time::timeout(deadline, exchange)
        .await
        .map_err(|_| SourceError::Failed)?
}

fn response_field(value: &Value, name: &str, cap: usize) -> Result<String, SourceError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty() && s.len() <= cap && (name == "text" || !s.chars().any(char::is_control))
        })
        .map(str::to_owned)
        .ok_or(SourceError::Failed)
}

fn response_status(value: &Value) -> Result<(), SourceError> {
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else if value.get("error").and_then(Value::as_str) == Some("unavailable") {
        Err(SourceError::Unavailable)
    } else {
        Err(SourceError::Failed)
    }
}

pub(crate) async fn check_arc_route(bound: &BoundArcInput) -> Result<(), SourceError> {
    let value = exchange(bound, "status", std::time::Duration::from_secs(2)).await?;
    response_status(&value)?;
    response_field(&value, "selected_route", 128)?;
    Ok(())
}

pub(crate) async fn request_arc(bound: BoundArcInput) -> Result<GeneratedArc, ArcFailure> {
    let value = exchange(&bound, "generate", std::time::Duration::from_secs(155)).await?;
    let usage = value.get("usage").and_then(measured_usage);
    let result = (|| {
        response_status(&value)?;
        let arc = WorkArc::from_json(&response_field(&value, "text", MAX_ARC_BYTES)?)
            .map_err(|_| SourceError::Failed)?;
        Ok(GeneratedArc {
            arc,
            selected_route: response_field(&value, "selected_route", 128)?,
            provider: response_field(&value, "provider", 128)?,
            model: response_field(&value, "model", 128)?,
            usage: usage.clone(),
            bound,
        })
    })();
    result.map_err(|error| ArcFailure { error, usage })
}

fn measured_usage(value: &Value) -> Option<Value> {
    let mut usage = serde_json::Map::new();
    let mut measured = false;
    for key in ["input", "output", "cacheRead", "cacheWrite", "totalTokens"] {
        if let Some(number) = value.get(key).and_then(Value::as_u64) {
            measured |= number > 0;
            usage.insert(key.into(), number.into());
        }
    }
    if let Some(cost) = value
        .get("cost")
        .and_then(|v| v.get("total"))
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
    {
        measured |= cost > 0.0;
        usage.insert("cost".into(), serde_json::json!({"total": cost}));
    }
    measured.then_some(Value::Object(usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines(rows: &[serde_json::Value]) -> Vec<u8> {
        let mut out = rows
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
        out.push(b'\n');
        out
    }

    #[test]
    fn follows_the_selected_pi_branch_without_reading_tool_output_or_thinking() {
        let sid = "11111111-1111-4111-8111-111111111111";
        let source = lines(&[
            json!({"type":"session","version":3,"id":sid,"cwd":"/project"}),
            json!({"type":"message","id":"root","parentId":null,"message":{"role":"user","content":"Build the dashboard"}}),
            json!({"type":"message","id":"done","parentId":"root","message":{"role":"assistant","content":[{"type":"thinking","thinking":"PRIVATE_REASONING"},{"type":"text","text":"Drafted the layout"}],"stopReason":"stop"}}),
            json!({"type":"message","id":"tool","parentId":"done","message":{"role":"toolResult","content":[{"type":"text","text":"PRIVATE_TOOL_OUTPUT"}]}}),
            json!({"type":"message","id":"other","parentId":"root","message":{"role":"assistant","content":[{"type":"text","text":"ABANDONED_BRANCH"}]}}),
        ]);
        let input = extract_arc_input(&source, sid, "tool").expect("selected branch");
        assert_eq!(input.covered_leaf, "tool");
        assert!(input.text.contains("Build the dashboard"));
        assert!(input.text.contains("Drafted the layout"));
        assert!(!input.text.contains("PRIVATE_REASONING"));
        assert!(!input.text.contains("PRIVATE_TOOL_OUTPUT"));
        assert!(!input.text.contains("ABANDONED_BRANCH"));
    }

    #[test]
    fn fails_closed_on_foreign_header_missing_parent_or_partial_record() {
        let sid = "11111111-1111-4111-8111-111111111111";
        let foreign = lines(&[
            json!({"type":"session","version":3,"id":"other"}),
            json!({"type":"message","id":"leaf","parentId":null,"message":{"role":"user","content":"hello"}}),
        ]);
        assert!(extract_arc_input(&foreign, sid, "leaf").is_err());
        let orphan = lines(&[
            json!({"type":"session","version":3,"id":sid}),
            json!({"type":"message","id":"leaf","parentId":"unseen","message":{"role":"user","content":"hello"}}),
        ]);
        assert!(extract_arc_input(&orphan, sid, "leaf").is_err());
        let mut partial = orphan;
        partial.extend_from_slice(b"{\"type\":\"message\"");
        assert!(extract_arc_input(&partial, sid, "leaf").is_err());
        let missing_parent = lines(&[
            json!({"type":"session","version":3,"id":sid}),
            json!({"type":"message","id":"leaf","message":{"role":"user","content":"hello"}}),
        ]);
        assert!(extract_arc_input(&missing_parent, sid, "leaf").is_err());
    }

    #[test]
    fn initial_exchange_requires_goal_and_preserves_repeated_user_messages() {
        let header = json!({"type":"session","id":"sid"});
        let user = json!({"type":"message","id":"u","parentId":null,"message":{"role":"user","content":"Goal"}});
        let assistant = json!({"type":"message","id":"a","parentId":"u","message":{"role":"assistant","content":"Reported progress"}});
        assert!(extract_arc_input(&lines(&[header.clone(), user.clone()]), "sid", "u").is_err());
        let orphan = json!({"type":"message","id":"a","parentId":null,"message":{"role":"assistant","content":"Claim"}});
        assert!(extract_arc_input(&lines(&[header.clone(), orphan]), "sid", "a").is_err());
        let repeat = json!({"type":"message","id":"r","parentId":"a","message":{"role":"user","content":"Goal"}});
        let input =
            extract_arc_input(&lines(&[header, user, assistant, repeat]), "sid", "r").unwrap();
        assert_eq!(input.text.matches("[user] Goal").count(), 2);
        assert!(input.text.contains("bounded excerpt"));
    }

    #[test]
    fn source_header_and_records_have_fixed_size_limits() {
        let sid = "11111111-1111-4111-8111-111111111111";
        let oversized_header = lines(&[
            json!({"type":"session","version":3,"id":sid,"padding":"x".repeat(MAX_RECORD_BYTES)}),
            json!({"type":"message","id":"leaf","parentId":null,"message":{"role":"user","content":"hello"}}),
        ]);
        assert!(extract_arc_input(&oversized_header, sid, "leaf").is_err());
        let oversized_source = vec![b'x'; MAX_SOURCE_BYTES + 1];
        assert!(extract_arc_input(&oversized_source, sid, "leaf").is_err());
    }

    #[test]
    #[serial_test::serial]
    fn admitted_synthetic_source_requires_settled_exact_cursor_and_regular_owned_path() {
        let temp = tempfile::tempdir().unwrap();
        let _app = crate::session::test_support::isolate_app_dir_at(temp.path());
        let fixture = test_support::Fixture::new(temp.path());
        let source = fixture.session.pi_sidecar_source().expect("Pi source");
        let launch = &fixture.session.active_execution.as_ref().unwrap().launch_id;
        assert!(
            source
                .read_file(
                    &fixture.session.id,
                    &format!("session_id.{launch}"),
                    128,
                    None
                )
                .is_some(),
            "published SID"
        );
        assert!(source::read_owned_transcript(
            &fixture.transcript,
            fixture.session.agent_session_id.as_deref().unwrap()
        )
        .is_ok());
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(fixture.socket.parent().unwrap())
                .unwrap()
                .mode()
                & 0o077,
            0,
            "private socket directory"
        );
        assert_eq!(
            read_arc_source(&fixture.session)
                .unwrap()
                .input
                .covered_leaf,
            "initial"
        );
        fixture.publish("initial", false);
        assert!(matches!(
            read_arc_source(&fixture.session),
            Err(SourceError::Stale)
        ));
        fixture.publish("initial", true);
        let path = fixture.transcript.with_extension("saved");
        std::fs::rename(&fixture.transcript, &path).unwrap();
        std::os::unix::fs::symlink(&path, &fixture.transcript).unwrap();
        assert!(matches!(
            read_arc_source(&fixture.session),
            Err(SourceError::Unavailable)
        ));
        std::fs::remove_file(&fixture.transcript).unwrap();
        std::fs::rename(path, &fixture.transcript).unwrap();
        fixture.publish("missing-ancestor", true);
        assert!(matches!(
            read_arc_source(&fixture.session),
            Err(SourceError::Stale)
        ));
        let mut disabled = fixture.session.clone();
        disabled.work_arc_enabled = false;
        std::fs::remove_file(&fixture.transcript).unwrap();
        assert!(matches!(
            read_arc_source(&disabled),
            Err(SourceError::Disabled)
        ));
    }

    #[test]
    fn unconsented_session_does_not_observe_a_pi_source() {
        let mut session = crate::session::Instance::new("pi-arc", "/tmp/pi-arc");
        session.tool = "pi".into();
        assert!(!session.work_arc_enabled);
        assert!(matches!(
            read_arc_source(&session),
            Err(SourceError::Disabled)
        ));
        session.work_arc_enabled = true;
        assert!(matches!(
            read_arc_source(&session),
            Err(SourceError::Unavailable)
        ));
    }

    #[test]
    fn swapping_agent_drops_work_arc_consent_and_derived_cue() {
        let mut session = crate::session::Instance::new("pi-arc", "/tmp/pi-arc");
        session.tool = "pi".into();
        session.work_arc_enabled = true;
        session.swap_tool("claude");
        assert!(!session.work_arc_enabled);
        assert!(session.work_arc.cleared);
    }

    #[test]
    fn new_agent_conversation_does_not_inherit_work_arc_consent() {
        let mut session = crate::session::Instance::new("pi-arc", "/tmp/pi-arc");
        session.set_agent_conversation(Some("one".into()), None, None);
        session.work_arc_enabled = true;
        session.set_agent_conversation(Some("two".into()), None, None);
        assert!(!session.work_arc_enabled);
    }

    #[test]
    fn opt_in_cannot_cross_a_concurrent_agent_conversation_change() {
        let mut before = crate::session::Instance::new("pi-arc", "/tmp/pi-arc");
        before.set_agent_conversation(Some("first".into()), None, None);
        let mut after = before.clone();
        after.work_arc_enabled = true;
        let mut peer = before.clone();
        peer.set_agent_conversation(Some("second".into()), None, None);
        peer.merge_user_action_diff(&before, &after);
        assert!(!peer.work_arc_enabled);
        let mut unchanged = before.clone();
        unchanged.merge_user_action_diff(&before, &after);
        assert!(unchanged.work_arc_enabled);
        let mut revoked = before.clone();
        revoked.work_arc.clear();
        revoked.merge_user_action_diff(&before, &after);
        assert!(
            !revoked.work_arc_enabled,
            "concurrent revocation wins over stale opt-in"
        );
        let mut unrelated = before.clone();
        let mut peer_enabled = before;
        peer_enabled.work_arc_enabled = true;
        unrelated.unread = true;
        peer_enabled.merge_user_action_diff(&unrelated, &unrelated);
        assert!(
            peer_enabled.work_arc_enabled,
            "unrelated action preserves peer consent"
        );
    }

    #[tokio::test]
    async fn generation_rejects_a_response_for_a_different_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::Builder::new()
            .prefix("arc-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = dir.path().join("s");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let n = stream.read(&mut request).await.unwrap();
            assert!(std::str::from_utf8(&request[..n])
                .unwrap()
                .contains("Synthetic input"));
            let reply = serde_json::json!({
                "ok": true, "nonce": "wrong", "selected_route": "router/auto",
                "provider": "test", "model": "model",
                "text": r#"{"goal":"G","now":"N","remaining":"R","next":"X"}"#,
            });
            stream
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });
        let bound = BoundArcInput {
            input: ArcInput {
                covered_leaf: "leaf".into(),
                text: "Synthetic input".into(),
            },
            session_id: "aoe-session".into(),
            pi_id: "11111111-1111-4111-8111-111111111111".into(),
            launch_id: "22222222-2222-4222-8222-222222222222".into(),
            generation: 3,
            socket_path: path,
        };
        assert!(matches!(
            request_arc(bound).await,
            Err(ArcFailure {
                error: SourceError::Stale,
                usage: None
            })
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn generation_uses_bounded_socket_and_preserves_route_and_usage() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let dir = tempfile::Builder::new()
            .prefix("arc-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = dir.path().join("s");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert!(
                line.len() > 26_000,
                "wire escaping must not shrink the decoded budget"
            );
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["input"].as_str().unwrap(), "\u{1}\"\\".repeat(8000));
            assert_eq!(request["op"], "generate");
            assert_eq!(
                request["session_id"],
                "11111111-1111-4111-8111-111111111111"
            );
            assert_eq!(request["source"], "22222222-2222-4222-8222-222222222222");
            assert_eq!(request["leaf_id"], "leaf");
            let reply = serde_json::json!({
                "ok": true, "nonce": request["nonce"], "selected_route": "router/auto",
                "provider": "anthropic", "model": "physical-model", "usage": {"input": 10},
                "text": r#"{"goal":"Goal","now":"Progress","remaining":"Unknown","next":"Review"}"#,
            });
            stream
                .get_mut()
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });
        let bound = BoundArcInput {
            input: ArcInput {
                covered_leaf: "leaf".into(),
                text: "\u{1}\"\\".repeat(8000),
            },
            session_id: "aoe-session".into(),
            pi_id: "11111111-1111-4111-8111-111111111111".into(),
            launch_id: "22222222-2222-4222-8222-222222222222".into(),
            generation: 3,
            socket_path: path,
        };
        let result = request_arc(bound).await.unwrap();
        assert_eq!(result.arc.next, "Review");
        assert_eq!(result.selected_route, "router/auto");
        assert_eq!(result.provider, "anthropic");
        assert_eq!(result.model, "physical-model");
        assert_eq!(result.usage.as_ref().unwrap()["input"], 10);
        assert_eq!(result.bound.session_id, "aoe-session");
        assert_eq!(result.bound.generation, 3);
        server.await.unwrap();
    }

    #[test]
    fn usage_requires_measurement_and_drops_unbounded_payloads() {
        assert!(measured_usage(&json!({"input":0,"output":0,"cost":{"total":0}})).is_none());
        assert_eq!(
            measured_usage(&json!({"input":12,"raw":"PRIVATE_PAYLOAD"})),
            Some(json!({"input":12}))
        );
        assert!(measured_usage(&json!({"input":-3,"cost":{"total":-1}})).is_none());
    }

    #[test]
    fn arc_fields_are_bounded_and_require_all_four_values() {
        let valid = r#"{"goal":"Add a dashboard","now":"Tests reported passing","remaining":"Unknown","next":"Review the diff"}"#;
        let arc = WorkArc::from_json(valid).expect("four fields");
        assert_eq!(arc.remaining, "Unknown");
        assert!(WorkArc::from_json(r#"{"goal":"x","now":"y","remaining":"z"}"#).is_err());
        let long = format!(
            "{{\"goal\":\"{}\",\"now\":\"y\",\"remaining\":\"z\",\"next\":\"w\"}}",
            "word ".repeat(81)
        );
        assert!(WorkArc::from_json(&long).is_err());
    }
}
