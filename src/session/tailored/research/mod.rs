//! Native prompt review and durable, single-attempt backend handoff.

mod developer;
mod dispatch;
mod owned_pane;
#[cfg(test)]
mod tests;

use super::CaptureAccount;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Orx,
    Evo,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceCategory {
    Grafana,
    Otel,
    Zulip,
    Slack,
    Supplied,
    Public,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceScope {
    pub category: SourceCategory,
    pub scope: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub account: CaptureAccount,
    pub context: String,
    pub sources: Vec<SourceScope>,
    pub developed_prompt: String,
    pub backend: Option<Backend>,
    pub workspace: String,
    pub permitted_scope: String,
    pub evaluation: String,
    pub unknowns_resolution: String,
    pub public_disclosure: String,
    pub consequences_reviewed: bool,
}
impl Inputs {
    pub fn new(account: CaptureAccount) -> Self {
        Self {
            account,
            context: String::new(),
            sources: vec![],
            developed_prompt: String::new(),
            backend: None,
            workspace: String::new(),
            permitted_scope: String::new(),
            evaluation: String::new(),
            unknowns_resolution: String::new(),
            public_disclosure: "No private context in public queries".into(),
            consequences_reviewed: false,
        }
    }
    pub fn validate(&self) -> Result<()> {
        for value in [
            &self.context,
            &self.developed_prompt,
            &self.workspace,
            &self.permitted_scope,
            &self.evaluation,
            &self.unknowns_resolution,
            &self.public_disclosure,
        ] {
            text(value, 32_768)?;
        }
        if self.sources.len() > 32 {
            bail!("At most 32 explicitly scoped sources are allowed");
        }
        for source in &self.sources {
            text(&source.scope, 4096)?;
            if source.scope.trim().is_empty() {
                bail!("Every source needs an explicit scope");
            }
            let allowed = match source.category {
                SourceCategory::Grafana | SourceCategory::Otel | SourceCategory::Zulip => {
                    self.account == CaptureAccount::Personal
                }
                SourceCategory::Slack => self.account == CaptureAccount::Work,
                SourceCategory::Supplied | SourceCategory::Public => true,
            };
            if !allowed {
                bail!("Source category is outside the selected account boundary");
            }
        }
        Ok(())
    }
}
fn text(value: &str, max: usize) -> Result<()> {
    if value.len() > max || value.contains('\0') {
        bail!("Text exceeds {max} bytes or contains NUL");
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Draft,
    Developing,
    Review,
    Running,
    Submitted,
    Uncertain,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Development {
    pub developed_prompt: String,
    pub recommendation: Backend,
    pub reason: String,
    pub unknowns: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Receipt {
    pub profile: String,
    pub process_id: u32,
    pub backend_id: String,
    pub return_path: String,
    pub project_id: Option<String>,
    pub turn_id: Option<String>,
    pub pane: Option<owned_pane::OwnedPane>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Draft {
    pub id: String,
    pub original_prompt: String,
    pub inputs: Inputs,
    pub revision: u64,
    pub state: State,
    pub recommendation: Option<Backend>,
    pub reason: String,
    pub unknowns: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub hidden: bool,
    pub frozen_prompt: Option<String>,
    pub receipt: Option<Receipt>,
    pub error: Option<String>,
}
impl Draft {
    pub fn new(id: String, original_prompt: String, inputs: Inputs) -> Result<Self> {
        let parsed = uuid::Uuid::parse_str(&id).context("Research request ID must be a UUID")?;
        if parsed.to_string() != id {
            bail!("Research request ID must be a canonical lowercase hyphenated UUID");
        }
        text(&original_prompt, 16_384)?;
        if original_prompt.trim().is_empty() {
            bail!("Enter a research question");
        }
        inputs.validate()?;
        Ok(Self {
            id,
            original_prompt,
            inputs,
            revision: 0,
            state: State::Draft,
            recommendation: None,
            reason: String::new(),
            unknowns: vec![],
            created_at: chrono::Utc::now(),
            hidden: false,
            frozen_prompt: None,
            receipt: None,
            error: None,
        })
    }
    fn editable(&self, revision: u64) -> Result<()> {
        if self.revision != revision {
            bail!("Research revision changed; reopen before continuing");
        }
        if !matches!(self.state, State::Draft | State::Review | State::Failed) {
            bail!("Request is busy or already attempted; never resubmit an uncertain run");
        }
        Ok(())
    }
    pub fn edit(&mut self, revision: u64, inputs: Inputs) -> Result<()> {
        self.editable(revision)?;
        inputs.validate()?;
        // Context changes need fresh development, not a stale recommendation.
        if inputs.account != self.inputs.account
            || inputs.context != self.inputs.context
            || serde_json::to_value(&inputs.sources)? != serde_json::to_value(&self.inputs.sources)?
        {
            self.state = State::Draft;
            self.recommendation = None;
        }
        self.inputs = inputs;
        self.revision += 1;
        self.error = None;
        Ok(())
    }
    fn apply_development(&mut self, result: Development) -> Result<()> {
        developer::validate(&result)?;
        self.inputs.developed_prompt = result.developed_prompt;
        self.inputs.backend = None;
        self.inputs.consequences_reviewed = false;
        self.recommendation = Some(result.recommendation);
        self.reason = result.reason;
        self.unknowns = result.unknowns;
        self.revision += 1;
        self.state = State::Review;
        self.error = None;
        Ok(())
    }
    fn authorize(&mut self, revision: u64) -> Result<String> {
        self.editable(revision)?;
        self.inputs.validate()?;
        if self.state != State::Review || self.recommendation.is_none() {
            bail!("Develop the prompt before Run");
        }
        let backend = self
            .inputs
            .backend
            .context("Choose ORX or EVO explicitly")?;
        if self.inputs.developed_prompt.trim().is_empty()
            || !self.inputs.consequences_reviewed
            || self.inputs.permitted_scope.trim().is_empty()
            || self.inputs.public_disclosure.trim().is_empty()
        {
            bail!("Review the developed prompt, permitted scope, disclosure and consequences before Run");
        }
        let workspace = std::path::Path::new(&self.inputs.workspace);
        if !workspace.is_absolute() || !workspace.is_dir() {
            bail!("Review an existing absolute workspace path");
        }
        if backend == Backend::Evo && self.inputs.evaluation.trim().is_empty() {
            bail!("EVO requires reviewed evaluation criteria");
        }
        if !self.unknowns.is_empty() && self.inputs.unknowns_resolution.trim().is_empty() {
            bail!("Resolve consequential unknowns before Run");
        }
        let prompt = format!("Account/source boundaries are mandatory. No integration crawling or implied connector access. Backend selection does not widen access.\nOriginal input (retained separately):\n{}\n\nReviewed request:\n{}\n\nReviewed constraints (host-owned):\n{}\n\nOperation: {}", self.original_prompt, self.inputs.developed_prompt, serde_json::to_string_pretty(&self.inputs)?, if backend == Backend::Evo { "discovery and baseline ONLY. Do not invoke optimize, ship, merge or push. Optimization needs subsequent explicit review. Any optimize budget later means per-branch serial experiment depth, not dollars." } else { "Research within the reviewed scope. Do not change account or disclose private context to public search." });
        text(&prompt, 65_536).context("The exact reviewed envelope must fit 65536 UTF-8 bytes; shorten the review without truncating it")?;
        self.frozen_prompt = Some(prompt.clone());
        self.state = State::Running;
        self.revision += 1;
        Ok(prompt)
    }
}

#[derive(Clone)]
pub struct Store {
    path: PathBuf,
}
impl Store {
    pub fn open() -> Result<Self> {
        Ok(Self::at(
            crate::session::get_app_dir()?.join("research.json"),
        ))
    }
    pub(super) fn at(path: PathBuf) -> Self {
        Self { path }
    }
    fn update<R>(&self, f: impl FnOnce(&mut BTreeMap<String, Draft>) -> Result<R>) -> Result<R> {
        crate::session::locked_update_if_changed(
            &self.path,
            |s| Ok(serde_json::from_str(s)?),
            |s| Ok(serde_json::to_string_pretty(s)?),
            f,
        )?
    }
    pub fn list(&self) -> Result<Vec<Draft>> {
        self.update(|rows| {
            let mut rows: Vec<_> = rows.values().cloned().collect();
            rows.sort_by_key(|d| std::cmp::Reverse(d.created_at));
            Ok(rows)
        })
    }
    pub fn get(&self, id: &str) -> Result<Draft> {
        self.update(|rows| rows.get(id).cloned().context("Research request not found"))
    }
    pub fn create(&self, draft: Draft) -> Result<Draft> {
        self.update(|rows| {
            if let Some(old) = rows.get(&draft.id) {
                if old.original_prompt != draft.original_prompt {
                    bail!("Research request ID already exists");
                }
                return Ok(old.clone());
            }
            rows.insert(draft.id.clone(), draft.clone());
            Ok(draft)
        })
    }
    pub fn change<R>(&self, id: &str, f: impl FnOnce(&mut Draft) -> Result<R>) -> Result<R> {
        self.update(|rows| f(rows.get_mut(id).context("Research request not found")?))
    }
    pub fn operate(&self, op: Operation) -> Result<Draft> {
        let id = op.id.clone();
        match op.operation.as_str() {
            "create" => {
                return self.create(Draft::new(
                    id,
                    op.original_prompt.context("Missing original prompt")?,
                    op.inputs.context("Missing inputs")?,
                )?)
            }
            "edit" => self.change(&id, |d| {
                d.edit(
                    op.revision.context("Missing revision")?,
                    op.inputs.context("Missing inputs")?,
                )
            })?,
            "visibility" => self.change(&id, |d| {
                d.hidden = op.hidden.context("Missing visibility")?;
                Ok(())
            })?,
            "develop" => {
                let draft = self.change(&id, |d| {
                    d.editable(op.revision.context("Missing revision")?)?;
                    d.state = State::Developing;
                    d.revision += 1;
                    Ok(d.clone())
                })?;
                let result = developer::develop(&draft);
                self.finish_development(&id, draft.revision, result)?;
            }
            "recover-development" => self.change(&id, |d| {
                if d.state != State::Developing || Some(d.revision) != op.revision { bail!("Only the current developing revision can be abandoned"); }
                d.state = State::Draft; d.revision += 1; d.recommendation = None; d.inputs.backend = None; d.inputs.consequences_reviewed = false;
                d.error = Some("Previous development abandoned. Input retained; late output will be ignored. Develop again explicitly.".into());
                Ok(())
            })?,

            "run" => {
                let draft = self.get(&id)?;
                if matches!(
                    draft.state,
                    State::Running | State::Submitted | State::Uncertain
                ) {
                    return Ok(draft);
                }
                // Qualification is read-only and must precede committing dispatch intent.
                dispatch::qualify(&draft)?;
                let frozen = self.change(&id, |d| {
                    d.authorize(op.revision.context("Missing revision")?)?;
                    Ok(d.clone())
                })?;
                let result = dispatch::run(&frozen, self);
                self.change(&id, |d| {
                    match result { Ok(receipt) => { d.receipt = Some(receipt); d.state = State::Submitted; d.error = None; }, Err(error) => { d.state = State::Uncertain; d.error = Some(format!("Run outcome uncertain. Do not retry. Inspect the retained backend identity/logs: {error:#}")); } }
                    Ok(())
                })?;
            }
            _ => bail!("Unknown research operation"),
        }
        self.get(&id)
    }
    fn finish_development(
        &self,
        id: &str,
        revision: u64,
        result: Result<Development>,
    ) -> Result<()> {
        self.change(id, |d| {
            if d.state != State::Developing || d.revision != revision {
                return Ok(());
            }
            match result {
                Ok(result) => d.apply_development(result)?,
                Err(error) => {
                    d.state = State::Failed;
                    d.error = Some(format!("Prompt development failed: {error:#}"));
                }
            }
            Ok(())
        })
    }
    pub fn history(&self) -> Result<Vec<super::history::HistoryEntry>> {
        Ok(self.list()?.iter().map(Draft::history).collect())
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub operation: String,
    pub id: String,
    pub revision: Option<u64>,
    pub original_prompt: Option<String>,
    pub inputs: Option<Inputs>,
    pub hidden: Option<bool>,
}

pub use dispatch::{output, reply};

impl Draft {
    pub fn history(&self) -> super::history::HistoryEntry {
        super::history::HistoryEntry {
            id: self.id.clone(),
            profile: "research".into(),
            title: "Tailored research".into(),
            account: self.inputs.account,
            input_preview: self
                .original_prompt
                .chars()
                .filter(|c| {
                    !c.is_control()
                        && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                })
                .take(160)
                .collect(),
            status: format!("{:?}", self.state).to_lowercase(),
            created_at: self.created_at,
            hidden: self.hidden,
            research: true,
        }
    }
}
