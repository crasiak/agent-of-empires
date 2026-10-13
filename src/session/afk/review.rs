//! Passive, exact stored selections. No runtime probes, repair, or authority writes.
use super::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub mod annotations;
const SOURCE_BYTES: usize = 64 * 1024 * 1024;
pub const EXPORT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SELECTIONS: usize = 16;
/// UTF-8 bytes admitted by review and annotations, without renaming historical profiles.
pub const PROFILE_NAME_BYTES: usize = 240;

#[cfg(test)]
thread_local! {
    pub(in crate::session::afk) static AFTER_AUTHORITY_READ: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub profile: String,
    pub session_id: String,
    pub window_id: String,
}
impl Selection {
    fn validate(&self) -> Result<()> {
        validate_profile(&self.profile)?;
        authority::scope(&self.profile, Some(&self.session_id))?;
        canonical_uuid(&self.window_id)
    }
}
impl std::str::FromStr for Selection {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let parts: Vec<_> = value.split('/').collect();
        ensure!(
            parts.len() == 3,
            "selection must be exact PROFILE/SESSION_ID/WINDOW_UUID"
        );
        let selection = Self {
            profile: parts[0].into(),
            session_id: parts[1].into(),
            window_id: parts[2].into(),
        };
        selection.validate()?;
        Ok(selection)
    }
}
fn validate_profile(profile: &str) -> Result<()> {
    ensure!(
        profile.len() <= PROFILE_NAME_BYTES,
        "review profile exceeds 240 UTF-8 bytes"
    );
    authority::scope(profile, None)
}
fn canonical_uuid(value: &str) -> Result<()> {
    ensure!(
        value.len() == 36 && uuid::Uuid::parse_str(value)?.to_string() == value,
        "canonical UUID required"
    );
    Ok(())
}
#[derive(Clone, Debug, Serialize)]
pub struct SelectedWindow {
    pub selection: Selection,
    pub source_revision: Option<u64>,
    pub window: Option<Value>,
    pub annotations: Vec<annotations::Annotation>,
    pub errors: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub version: u32,
    pub capture_started_at_ms: i64,
    pub capture_finished_at_ms: i64,
    pub complete: bool,
    pub selections: Vec<SelectedWindow>,
    /// Each original batch occurs once. Window usage and these totals must not be summed.
    pub shared_usage: Vec<Value>,
    pub observation: &'static str,
    pub privacy: &'static str,
}
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Format {
    Json,
    Markdown,
}

pub fn read_annotation_request(path: &Path) -> Result<annotations::Request> {
    batch::read_private_bounded(path, 16 * 1024)
}

pub fn collect(selections: &[Selection]) -> Result<Report> {
    let mut report = collect_at(&crate::session::get_app_dir_path()?, selections, now_ms())?;
    report.capture_finished_at_ms = now_ms();
    Ok(report)
}

pub(in crate::session::afk) fn read_authority(
    app: &Path,
    profile: &str,
    metadata: &authority::migration_v3::Metadata,
    budget: &mut usize,
) -> Result<authority::Snapshot> {
    let store = authority::profile_store(app, profile, false)?;
    authority::unparked(&store)?;
    let snapshot: authority::Snapshot = store
        .read_budgeted("snapshot.json", budget)
        .map_err(|_| {
            anyhow::anyhow!("authority snapshot unreadable, corrupt, unsafe or over read capacity")
        })?
        .context("missing profile authority")?;
    authority::unparked(&store)?;
    ensure!(
        snapshot.profile == profile,
        "profile authority scope mismatch"
    );
    metadata.validate_snapshot(&snapshot)?;
    Ok(snapshot)
}
fn pending(app: &Path, profile: &str, session: &str) -> Result<bool> {
    Ok(
        Store::existing_transport(app, profile, session)?.is_some_and(|store| {
            !matches!(
                store
                    .dir
                    .entry_stat(Path::new(usage::transport::UNRESOLVED)),
                Ok(None)
            )
        }),
    )
}

pub(in crate::session::afk) fn collect_at(
    app: &Path,
    selections: &[Selection],
    now: i64,
) -> Result<Report> {
    ensure!(
        (1..=MAX_SELECTIONS).contains(&selections.len()),
        "review requires 1..16 exact selections"
    );
    let mut seen = BTreeSet::new();
    for selection in selections {
        selection.validate()?;
        ensure!(seen.insert(selection), "duplicate review selection");
    }
    let mut report = Report {
        version: 1, capture_started_at_ms: now, capture_finished_at_ms: now, complete: true,
        selections: vec![], shared_usage: vec![],
        observation: "Stored facts collected over an interval, not an atomic cross-profile snapshot or live runtime observation. Operator annotations are not approvals, answers, recovery, or host facts.",
        privacy: "Private review, not guaranteed secret-free. Structured workspace/launch data, action bodies, preimages and transcripts omitted. Free text uses heuristic credential filtering. Usage is not an invoice or account ceiling; do not sum window and shared projections.",
    };
    let mut budget = SOURCE_BYTES;
    let metadata = authority::migration_v3::overview_metadata(app, &mut budget);
    let note_index = annotations::read_index(app, &mut budget);
    let mut profiles = BTreeMap::new();
    let mut notes = BTreeMap::new();
    for profile in selections
        .iter()
        .map(|s| &s.profile)
        .collect::<BTreeSet<_>>()
    {
        let snapshot = match &metadata {
            Ok(metadata) => read_authority(app, profile, metadata, &mut budget),
            Err(error) => Err(anyhow::anyhow!(
                "authority not ready: {}",
                error.root_cause()
            )),
        };
        #[cfg(test)]
        if let Some(after_read) = AFTER_AUTHORITY_READ.with(|slot| slot.borrow_mut().take()) {
            after_read();
        }
        let annotation_rows = (|| -> Result<Vec<annotations::Annotation>> {
            let (store, index) = note_index
                .as_ref()
                .map_err(|e| anyhow::anyhow!("annotations unavailable: {}", e.root_cause()))?;
            let stored = annotations::read_profile(store, index, profile, &mut budget)?;
            let snapshot = snapshot
                .as_ref()
                .map_err(|_| anyhow::anyhow!("annotation target authority unavailable"))?;
            Ok(annotations::projected(stored, snapshot, now))
        })();
        notes.insert(profile.clone(), annotation_rows);
        profiles.insert(profile.clone(), snapshot);
    }
    let mut shared = BTreeSet::new();
    for selection in selections {
        let mut row = SelectedWindow {
            selection: selection.clone(),
            source_revision: None,
            window: None,
            annotations: vec![],
            errors: vec![],
        };
        if let Some(result) = notes.get(&selection.profile) {
            match result {
                Ok(notes) => {
                    row.annotations = notes
                        .iter()
                        .filter(|a| a.target.selection == *selection)
                        .cloned()
                        .collect()
                }
                Err(error) => row.errors.push(terminal_text(&error.to_string())),
            }
        }
        let window = (|| -> Result<Value> {
            let snapshot = profiles
                .get(&selection.profile)
                .context("profile not selected")?
                .as_ref()
                .map_err(|e| anyhow::anyhow!("{}", e.root_cause()))?;
            row.source_revision = Some(snapshot.revision);
            for annotation in &row.annotations {
                if let Err(error) = annotation.target.validate_snapshot(snapshot) {
                    row.errors.push(terminal_text(&format!(
                        "Annotation {} does not match captured authority revision {}: {}",
                        annotation.id, snapshot.revision, error
                    )));
                }
            }
            let session = snapshot
                .sessions
                .get(&selection.session_id)
                .context("unknown historical session")?;
            let mut view = session.book.review_window(&selection.window_id, now)?;
            view["usage"] = snapshot.usage.window_view(&selection.window_id);
            if pending(app, &selection.profile, &selection.session_id)? {
                usage::pending_projection(&mut view["usage"]);
            }
            for operation in snapshot.batches.values().filter(|op| {
                op.members
                    .values()
                    .any(|m| m.window.as_ref() == Some(&selection.window_id))
            }) {
                view["shared_usage_reference"] =
                    json!({"profile":selection.profile,"operation_id":operation.id});
                let key = (selection.profile.clone(), operation.id.clone());
                if !shared.contains(&key) {
                    let mut measured = usage::batch_view(snapshot, &operation.id);
                    for (id, member) in &operation.members {
                        if member.window.is_some() && pending(app, &selection.profile, id)? {
                            usage::pending_projection(&mut measured);
                        }
                    }
                    shared.insert(key);
                    report.shared_usage.push(json!({"profile":selection.profile,"operation_id":operation.id,
                        "shared_requests":operation.shared_requests,
                        "shared_charges":operation.shared_requests.map(|_| operation.tickets.len()),
                        "held_unbound":operation.shared_requests.map(|_| operation.tickets.iter().filter(|t| t.request.is_none()).count()),
                        "usage":measured,"scope":"all original batch windows; reported once, overlaps selected-window usage"}));
                }
            }
            escape_value(&mut view);
            Ok(view)
        })();
        match window {
            Ok(window) => row.window = Some(window),
            Err(error) => row.errors.push(terminal_text(&error.to_string())),
        }
        // Identity strings stay exact; renderers must escape their representation.
        for annotation in &mut row.annotations {
            annotation.body = annotation.body.as_deref().map(terminal_text);
        }
        report.complete &= row.errors.is_empty();
        report.selections.push(row);
    }
    for value in &mut report.shared_usage {
        escape_value(value);
    }
    Ok(report)
}

fn unsafe_terminal_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}')
}
/// Apply at presentation boundaries, including exact profile identities.
pub fn terminal_text(text: &str) -> String {
    text.chars()
        .map(|c| {
            if unsafe_terminal_char(c) {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}
fn terminal_safe_json(value: &impl Serialize) -> Result<Vec<u8>> {
    use std::fmt::Write;
    let json = serde_json::to_string_pretty(value)?;
    let mut output = String::with_capacity(json.len());
    for c in json.chars() {
        // String whitespace is already escaped by serde; preserve JSON formatting.
        if unsafe_terminal_char(c) && !matches!(c, '\n' | '\r' | '\t') {
            let mut units = [0; 2];
            for unit in c.encode_utf16(&mut units) {
                write!(output, "\\u{unit:04x}")?;
            }
        } else {
            output.push(c);
        }
    }
    Ok(output.into_bytes())
}
fn escape_value(value: &mut Value) {
    match value {
        Value::String(text) => *text = terminal_text(text),
        Value::Array(values) => values.iter_mut().for_each(escape_value),
        Value::Object(values) => values.values_mut().for_each(escape_value),
        _ => (),
    }
}
fn markdown_text(text: &str) -> String {
    let mut output = String::new();
    for c in terminal_text(text).chars() {
        match c {
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '&' => output.push_str("&amp;"),
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '(' | ')' | '#' | '+' | '-' | '.'
            | '!' | '|' => {
                output.push('\\');
                output.push(c);
            }
            _ => output.push(c),
        }
    }
    output
}
fn markdown_value(value: &Value, depth: usize, output: &mut String) {
    match value {
        Value::Object(values) if !values.is_empty() => {
            for (key, value) in values {
                output.push_str(&format!(
                    "{}- {}:\n",
                    "  ".repeat(depth),
                    markdown_text(key)
                ));
                markdown_value(value, depth + 1, output);
            }
        }
        Value::Array(values) if !values.is_empty() => {
            for (index, value) in values.iter().enumerate() {
                output.push_str(&format!("{}- Item {}:\n", "  ".repeat(depth), index + 1));
                markdown_value(value, depth + 1, output);
            }
        }
        _ => output.push_str(&format!(
            "{}- {}\n",
            "  ".repeat(depth),
            markdown_text(
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
                    .as_str()
            )
        )),
    }
}
pub fn render_annotation(annotation: &annotations::Annotation) -> Result<Vec<u8>> {
    let mut bytes = terminal_safe_json(annotation)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= EXPORT_BYTES,
        "annotation output exceeds 8 MiB after formatting"
    );
    Ok(bytes)
}
pub fn render(report: &Report, format: Format) -> Result<Vec<u8>> {
    let mut bytes = match format {
        Format::Json => terminal_safe_json(report)?,
        Format::Markdown => {
            let mut text = format!("# AFK return review\n\nComplete: {}\n\n", report.complete);
            markdown_value(&serde_json::to_value(report)?, 0, &mut text);
            text.into_bytes()
        }
    };
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= EXPORT_BYTES,
        "review output exceeds 8 MiB after formatting; nothing published"
    );
    Ok(bytes)
}
pub fn export(report: &Report, format: Format, output: &Path) -> Result<()> {
    export_at(&crate::session::get_app_dir_path()?, report, format, output)
}
pub(in crate::session::afk) fn export_at(
    app: &Path,
    report: &Report,
    format: Format,
    output: &Path,
) -> Result<()> {
    batch::publish_private(app, output, &render(report, format)?, EXPORT_BYTES)
}
