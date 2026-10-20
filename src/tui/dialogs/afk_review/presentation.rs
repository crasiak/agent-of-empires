use super::{annotation_detail, safe_lines, safe_value, shown_value, usage_detail};
use crate::session::afk::review::{self, Report, SelectedWindow};
use serde_json::Value;

pub(super) fn records(window: &Value) -> &[Value] {
    window["records"].as_array().map_or(&[], Vec::as_slice)
}

pub(super) fn shared<'a>(report: &'a Report, window: &Value) -> Option<&'a Value> {
    let reference = window.get("shared_usage_reference")?.as_object()?;
    let profile = reference.get("profile")?.as_str()?;
    let operation = reference.get("operation_id")?.as_str()?;
    report.shared_usage.iter().find(|value| {
        value["profile"].as_str() == Some(profile)
            && value["operation_id"].as_str() == Some(operation)
    })
}

pub(super) fn effect(window: &Value, decision: Option<&str>) -> &'static str {
    let action = &window["action"];
    if decision.is_some_and(|id| action["decision"].as_str() != Some(id)) {
        return "No effect observation recorded for this decision";
    }
    match action["outcomes"]
        .as_array()
        .and_then(|values| values.last())
        .and_then(Value::as_str)
    {
        Some("observed_applied") => "Applied (observed after the attempt)",
        Some("unknown") => "UNKNOWN; attempted effect not confirmed",
        Some(_) => "Unrecognized effect observation; inspect diagnostics",
        None => "No effect observation recorded; not proof of no effect",
    }
}

fn present(value: &Value) -> bool {
    !value.is_null()
        && value != &Value::Bool(false)
        && value.as_array().is_none_or(|values| !values.is_empty())
        && value.as_str().is_none_or(|text| !text.is_empty())
}

pub(super) fn usage_gap(usage: &Value, is_shared: bool) -> bool {
    if !usage.is_object()
        || present(&usage["unavailable"])
        || present(&usage["gap"])
        || present(&usage["gaps"])
        || usage["negative_evidence_pending"] == true
        || matches!(
            usage["coverage"].as_str(),
            Some("not_collected" | "unavailable" | "partial_or_unavailable")
        )
    {
        return true;
    }
    if is_shared {
        return usage["coverage"] != "stored_reported_interval";
    }
    let measurement = &usage["measurement"];
    !measurement.is_object()
        || measurement["tokens_complete"] != true
        || measurement["valuation_complete"] != true
        || present(&measurement["gaps"])
        || usage["source_complete"] != true
        || (usage["closing"] == true && usage["cutoff"].is_null())
}

#[derive(Default)]
pub(super) struct Attention {
    pub human: bool,
    pub effect: bool,
    pub denied: bool,
    pub usage: bool,
    pub missing: bool,
}
impl Attention {
    pub fn any(&self) -> bool {
        self.human || self.effect || self.denied || self.usage || self.missing
    }
}

pub(super) fn attention(report: &Report, row: Option<&SelectedWindow>) -> Attention {
    let Some(row) = row else {
        return Attention {
            missing: true,
            effect: true,
            usage: true,
            ..Attention::default()
        };
    };
    let Some(window) = &row.window else {
        return Attention {
            missing: true,
            effect: true,
            usage: true,
            ..Attention::default()
        };
    };
    let shared = shared(report, window);
    let missing_shared = !window["shared_usage_reference"].is_null() && shared.is_none();
    Attention {
        human: !human_attention(window).is_empty(),
        effect: window["action"]["outcomes"]
            .as_array()
            .and_then(|v| v.last())
            .is_none_or(|v| v != "observed_applied"),
        denied: present(&window["usage"]["blocked"])
            || shared.is_some_and(|s| present(&s["usage"]["blocked"])),
        usage: usage_gap(&window["usage"], false)
            || missing_shared
            || shared.is_some_and(|s| usage_gap(&s["usage"], true)),
        missing: !row.errors.is_empty()
            || missing_shared
            || records(window).iter().any(|r| !r["claims"].is_object()),
    }
}

pub(super) fn outcome(window: &Value) -> String {
    let state = window["state"].as_str();
    let outcome = match state {
        Some("ended") => match window["terminal_reason"].as_str() {
            Some("completed") => "Completed",
            Some("failed") => "Failed",
            Some("deferred") => "Deferred",
            Some("exhausted") => "Stopped: allowance exhausted",
            Some("interrupted") => "Interrupted",
            Some("delivery_unknown") => "Delivery unknown",
            _ => "Ended; reason unavailable",
        },
        Some("pending") => "Pending (stored)",
        Some("owned") => "In progress (stored)",
        _ => "Unavailable",
    };
    let expiry = if matches!(state, Some("pending" | "owned")) {
        match window["expired"].as_bool() {
            Some(true) => "; expiry passed",
            None => "; expiry unavailable",
            Some(false) => "",
        }
    } else {
        ""
    };
    format!("Outcome: {outcome}{expiry}")
}

fn human_attention(window: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let gates = [
        ("question", "unresolved_question_guard"),
        ("nudge", "unresolved_nudge"),
    ]
    .into_iter()
    .filter_map(|(label, field)| match window[field].as_bool() {
        Some(false) => None,
        Some(true) => Some(format!("{label} unresolved")),
        None => Some(format!("{label} unavailable")),
    })
    .collect::<Vec<_>>();
    if !gates.is_empty() {
        lines.push(format!("Host gates: {}", gates.join("; ")));
    }
    if window["question"].is_object() && window["unresolved_question_guard"] != true {
        lines.push("Question history is not an answer observation".into());
    }
    let mut human = vec![];
    if window["ambiguous"] == true {
        human.push("ambiguity");
    }
    if window["ambiguous"].as_bool().is_none() {
        human.push("ambiguity unknown");
    }
    if records(window)
        .iter()
        .any(|r| r["disposition"] == "human_required")
    {
        human.push("host-required");
    }
    if records(window)
        .iter()
        .any(|r| r["claims"]["human_required"] == true)
    {
        human.push("model-required");
    }
    if records(window).iter().any(|r| !r["claims"].is_object()) {
        human.push("claims missing");
    }
    if !human.is_empty() {
        lines.push(format!("Human: {}", human.join("; ")));
    }
    lines
}

pub(super) fn pinned(
    report: &Report,
    row: Option<&SelectedWindow>,
    decision: Option<&str>,
) -> Vec<String> {
    let Some(window) = row.and_then(|r| r.window.as_ref()) else {
        return vec![
            "Selected window unavailable; annotations disabled".into(),
            "Effect: unavailable; not proof of no effect".into(),
            "Needs attention: human gates and usage unavailable".into(),
        ];
    };
    let effect = effect(window, decision);
    let mut lines = vec![outcome(window), format!("Effect: {effect}")];
    let mut warnings = human_attention(window);
    if row.is_some_and(|row| !row.errors.is_empty()) {
        warnings.push("Source incomplete; inspect diagnostics".into());
    }
    let local = &window["usage"];
    let shared = shared(report, window);
    let shared_missing = !window["shared_usage_reference"].is_null() && shared.is_none();
    let denial = match (
        present(&local["blocked"]),
        shared.is_some_and(|s| present(&s["usage"]["blocked"])),
    ) {
        (true, true) => "local + shared",
        (true, false) => "local",
        (false, true) => "shared",
        (false, false) => "none recorded",
    };
    let local_gap = usage_gap(local, false);
    let shared_gap = shared_missing || shared.is_some_and(|s| usage_gap(&s["usage"], true));
    let gaps = match (local_gap, shared_gap) {
        (true, true) => "local + shared",
        (true, false) => "local",
        (false, true) => "shared",
        (false, false) => "none recorded",
    };
    match (denial != "none recorded", gaps != "none recorded") {
        (true, true) => warnings.push(format!("Usage denied: {denial} | Accounting gaps: {gaps}")),
        (true, false) => warnings.push(format!("Usage denied: {denial}")),
        (false, true) => warnings.push(format!("Accounting gaps: {gaps}")),
        (false, false) => (),
    }
    if local["negative_evidence_pending"] == true
        || shared.is_some_and(|s| s["usage"]["negative_evidence_pending"] == true)
    {
        warnings.push("Usage evidence pending; stored amounts incomplete".into());
    }
    if shared_missing {
        warnings.push("Original shared usage unavailable".into());
    }
    if warnings.is_empty() {
        lines.push(if effect.starts_with("Applied") {
            "Needs attention: none recorded".into()
        } else {
            "Needs attention: effect observation missing or uncertain".into()
        });
    } else {
        lines.push("Needs attention".into());
        lines.extend(warnings);
    }
    lines
}

fn usage_summary(usage: &Value, is_shared: bool) -> String {
    let mut facts = vec![];
    match usage["blocked"].as_str() {
        Some("exhausted") => facts.push("denied: budget exhausted"),
        Some("stale") => facts.push("denied: stale observation"),
        Some("unavailable") => facts.push("denied: observation unavailable"),
        Some(_) => facts.push("denied; inspect diagnostics"),
        None => (),
    }
    if usage["negative_evidence_pending"] == true {
        facts.push("evidence pending");
    }
    if usage["coverage"] == "not_collected" {
        facts.push("not collected");
    } else if present(&usage["unavailable"]) || !usage.is_object() {
        facts.push("source unavailable");
    } else if usage_gap(usage, is_shared) {
        facts.push("accounting incomplete");
    } else {
        facts.push("stored interval complete");
    }
    facts.join("; ")
}

pub(super) fn summary(report: &Report, row: &SelectedWindow) -> String {
    let Some(window) = &row.window else {
        return "Stored source unavailable. Open diagnostics for collection errors and retained operator notes.".into();
    };
    let count = records(window).len();
    let missing = records(window)
        .iter()
        .filter(|r| !r["claims"].is_object())
        .count();
    let mut text = format!(
        "{count} decisions; {} operator notes",
        row.annotations.len()
    );
    if missing > 0 {
        text.push_str(&format!("; {missing} claim bodies unavailable"));
    }
    if usage_gap(&window["usage"], false) || present(&window["usage"]["blocked"]) {
        text.push_str(&format!(
            "\nLocal usage: {}",
            usage_summary(&window["usage"], false)
        ));
    }
    if let Some(shared) = shared(report, window) {
        if usage_gap(&shared["usage"], true) || present(&shared["usage"]["blocked"]) {
            text.push_str(&format!(
                "\nOriginal shared usage: {}",
                usage_summary(&shared["usage"], true)
            ));
        }
    }
    text
}

fn words(value: &Value) -> String {
    match value {
        Value::Null => "unavailable".into(),
        Value::String(text) => review::terminal_text(text),
        Value::Array(values) => {
            if values.is_empty() {
                "none recorded".into()
            } else {
                values.iter().map(words).collect::<Vec<_>>().join("; ")
            }
        }
        _ => review::terminal_text(&value.to_string()),
    }
}

pub(super) fn decisions(row: &SelectedWindow, decision: Option<&str>) -> String {
    let Some(window) = &row.window else {
        return "Window context unavailable; retained notes are in diagnostics.".into();
    };
    let mut text = String::new();
    if let Some(id) = decision {
        let Some(record) = records(window)
            .iter()
            .find(|r| r["id"].as_str() == Some(id))
        else {
            return "Selected decision unavailable; annotations disabled. No replacement selected."
                .into();
        };
        text.push_str(&format!(
            "Host disposition: {}\nClaim body: {}\n",
            words(&record["disposition"]),
            words(&record["body_status"])
        ));
        let claims = &record["claims"];
        if claims.is_object() {
            text.push_str(&format!(
                "Model recommendation: {} | human-required claim: {}\nQuestion: {}\n",
                words(&claims["recommendation"]),
                words(&claims["human_required"]),
                words(&claims["question"])
            ));
            let criteria = claims["criteria"].as_array().map_or(&[][..], Vec::as_slice);
            for criterion in criteria {
                text.push_str(&format!(
                    "Criterion {}: weight {}\n",
                    words(&criterion["name"]),
                    words(&criterion["weight"])
                ));
            }
            if let Some(alternatives) = claims["alternatives"].as_array() {
                for alternative in alternatives {
                    let scores = alternative["scores"]
                        .as_array()
                        .map(|scores| {
                            scores
                                .iter()
                                .enumerate()
                                .map(|(i, score)| {
                                    format!(
                                        "{}: {}",
                                        criteria.get(i).map_or_else(
                                            || "unnamed criterion".into(),
                                            |c| words(&c["name"])
                                        ),
                                        words(score)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                        .unwrap_or_else(|| "unavailable".into());
                    text.push_str(&format!(
                        "Alternative {} | scores {}\n  Pros: {}\n  Cons: {}\n",
                        words(&alternative["name"]),
                        scores,
                        words(&alternative["pros"]),
                        words(&alternative["cons"])
                    ));
                }
            }
            for field in [
                "evidence",
                "assumptions",
                "uncertainties",
                "rationale",
                "risk",
                "rollback",
                "depends_on",
            ] {
                text.push_str(&format!("{field}: {}\n", words(&claims[field])));
            }
        } else {
            text.push_str("Original claim body unavailable; no rationale reconstructed\n");
        }
    } else {
        text.push_str(&format!("Window context\nTask: {}\nScope: {}\n{} stored decisions; {} claim bodies unavailable\n", words(&window["grant"]["task"]), words(&window["grant"]["scope"]), records(window).len(), records(window).iter().filter(|r| !r["claims"].is_object()).count()));
    }
    text.push_str("\nOperator annotations (not answers, approvals or host observations):\n");
    for note in row
        .annotations
        .iter()
        .filter(|n| decision.is_none() || n.target.decision.as_deref() == decision)
    {
        let target = match &note.target.decision {
            None => "window grant".into(),
            Some(id) => records(window)
                .iter()
                .position(|r| r["id"].as_str() == Some(id))
                .map_or_else(
                    || "unavailable decision".into(),
                    |i| format!("decision {}", i + 1),
                ),
        };
        text.push_str(&format!(
            "{:?} for {target}{}: {}\n",
            note.kind,
            if note.prior.is_some() {
                " (correction of prior claim)"
            } else {
                ""
            },
            review::terminal_text(
                note.body
                    .as_deref()
                    .unwrap_or("[expired body; identity retained]")
            )
        ));
    }
    text.push_str(
        "Reviewed annotations do not resolve host guards; question history is not an answer.\n",
    );
    text
}

pub(super) fn diagnostics(report: &Report, row: &SelectedWindow, decision: Option<&str>) -> String {
    let mut text = format!("Profile: {}\nSession: {}\nWindow: {}\nSelected decision: {}\nCaptured source revision: {:?}\nCapture interval: {}..{} (not atomic)\n{}\n{}\n",
        review::terminal_text(&row.selection.profile), review::terminal_text(&row.selection.session_id), review::terminal_text(&row.selection.window_id), review::terminal_text(decision.unwrap_or("window context")), row.source_revision,
        report.capture_started_at_ms, report.capture_finished_at_ms, report.observation, report.privacy);
    for selected in &report.selections {
        for error in &selected.errors {
            text.push_str(&format!(
                "INCOMPLETE: {} ({}/{}/{})\n",
                review::terminal_text(error),
                review::terminal_text(&selected.selection.profile),
                review::terminal_text(&selected.selection.session_id),
                review::terminal_text(&selected.selection.window_id)
            ));
        }
    }
    if let Some(window) = &row.window {
        text.push_str(&format!("Host question guard unresolved: {}\nHost nudge unresolved: {}\nStored ambiguity: {}\nWindow action observations: {}\nQuestion facts: {}\nNudge observations: {}\n", shown_value(&window["unresolved_question_guard"]), shown_value(&window["unresolved_nudge"]), shown_value(&window["ambiguous"]), window["action"]["outcomes"], window["question"], window["nudges"]));
        text.push_str(&decisions(row, decision));
        text.push_str(&usage_detail(
            &window["usage"],
            &window["shared_usage_reference"],
        ));
        if let Some(shared) = shared(report, window) {
            let mut shared = shared.clone();
            safe_value(&mut shared);
            text.push_str(&format!(
                "Original shared operation (overlaps local; never sum):\n{}\n",
                serde_json::to_string_pretty(&shared).unwrap_or_default()
            ));
        } else if !window["shared_usage_reference"].is_null() {
            text.push_str("Original shared operation: unavailable\n");
        }
    }
    text.push_str(&annotation_detail(row, decision));
    let mut selected =
        serde_json::to_value(row).expect("redacted review selection is serializable");
    safe_value(&mut selected);
    text.push_str(&format!(
        "\nRedacted selected report facts (no raw authority or transcript):\n{}",
        serde_json::to_string_pretty(&selected).unwrap_or_default()
    ));
    safe_lines(&text)
}
