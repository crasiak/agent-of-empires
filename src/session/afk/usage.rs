//! Bounded native-entry observations. Neither SDK cost nor context occupancy is billing authority.
use super::*;
use delegation::hash;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
mod accounting;
pub(super) use accounting::{check, latch, Denied};
pub(super) mod transport;

pub(super) const MAX_ENTRIES: usize = 8192;
pub(super) const REPORT_ENTRIES: usize = 128;
pub(super) const REPORT_BYTES: usize = 128 * 1024;
pub(super) const SAFE: u64 = 9_007_199_254_740_991;
pub(super) const LEASE_MS: u64 = 5000;

fn text(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control),
        "invalid usage identity"
    );
    Ok(())
}
fn entry_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c)),
        "invalid source entry id"
    );
    Ok(())
}
fn digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid usage digest"
    );
    Ok(())
}
pub(super) fn fingerprint(value: &impl Serialize) -> Result<String> {
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(hash(serde_json::to_vec(&value)?))
}
fn add(left: u64, right: u64) -> Result<u64> {
    left.checked_add(right)
        .filter(|n| *n <= SAFE)
        .context("usage exceeds wire-safe integer range")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub(super) struct Model {
    pub provider: String,
    pub model: String,
}
impl Model {
    fn validate(&self) -> Result<()> {
        text(&self.provider)?;
        text(&self.model)
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Counters {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub reasoning: u64,
}
impl Counters {
    pub fn total(&self) -> Result<u64> {
        ensure!(
            self.cache_write_1h <= self.cache_write && self.reasoning <= self.output,
            "invalid token subset"
        );
        add(
            add(self.input, self.output)?,
            add(self.cache_read, self.cache_write)?,
        )
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Metadata,
    User,
    Assistant,
    Usage,
    Compaction,
    BranchSummary,
    Nested,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum Gap {
    MissingUsage,
    Incomplete,
    InvalidUsage,
    ErrorOrAbort,
    NestedOverlap,
    Unattributed,
    Unpriced,
    SourceChanged,
    HistoryLimit,
    NotPersisted,
    Closing,
    LateWork,
    Restart,
    Unavailable,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: Kind,
    pub model: Option<Model>,
    pub counters: Option<Counters>,
    pub gap: Option<Gap>,
    pub sdk_cost_nano: Option<u64>,
}
impl Entry {
    fn validate(&self) -> Result<()> {
        entry_id(&self.id)?;
        if let Some(parent) = &self.parent_id {
            entry_id(parent)?;
        }
        if let Some(model) = &self.model {
            model.validate()?;
        }
        if let Some(c) = &self.counters {
            c.total()?;
        }
        ensure!(
            self.sdk_cost_nano.is_none_or(|n| n <= SAFE),
            "invalid SDK cost observation"
        );
        if matches!(self.kind, Kind::Metadata | Kind::User) {
            ensure!(
                self.model.is_none() && self.counters.is_none() && self.sdk_cost_nano.is_none(),
                "metadata fabricated usage"
            );
        } else if self.kind == Kind::Nested {
            ensure!(
                self.counters.is_none()
                    && self.sdk_cost_nano.is_none()
                    && self.gap == Some(Gap::NestedOverlap),
                "nested rollup cannot supply measured usage"
            );
        } else {
            ensure!(
                self.counters.is_some() || self.gap.is_some(),
                "missing usage is not zero"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    pub count: usize,
    pub hash: String,
}
impl Default for Cursor {
    fn default() -> Self {
        Self {
            count: 0,
            hash: hash("aoe-afk-usage-v1"),
        }
    }
}
impl Cursor {
    pub(super) fn append(&self, row: &Entry) -> Result<Self> {
        Ok(Self {
            count: self.count.checked_add(1).context("cursor overflow")?,
            hash: hash(format!("{}:{}", self.hash, fingerprint(row)?)),
        })
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.count <= MAX_ENTRIES, "usage history capacity exceeded");
        digest(&self.hash)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub native_id: String,
    pub file: String,
    pub parent: Option<String>,
}
impl Identity {
    fn validate(&self, binding: &Binding) -> Result<()> {
        ensure!(
            self.native_id == binding.native_id,
            "usage native identity mismatch"
        );
        digest(&self.file)?;
        if let Some(parent) = &self.parent {
            digest(parent)?;
        }
        Ok(())
    }
    pub fn key(&self, binding: &Binding) -> Result<String> {
        fingerprint(&(binding.profile.as_str(), binding.instance_id.as_str(), self))
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Boundary {
    Poll,
    BeforeRequest,
    BeforeTool,
    TurnEnd,
    Settled,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Report {
    pub version: u32,
    pub challenge: String,
    pub identity: Identity,
    pub after: Cursor,
    pub entries: Vec<Entry>,
    pub head: Cursor,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    pub window: Option<String>,
    pub boundary: Boundary,
    pub report: Option<Report>,
    pub gap: Option<Gap>,
}
impl Report {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1 && uuid::Uuid::parse_str(&self.challenge).is_ok(),
            "invalid usage report version/challenge"
        );
        self.after.validate()?;
        self.head.validate()?;
        ensure!(
            self.entries.len() <= REPORT_ENTRIES && serde_json::to_vec(self)?.len() <= REPORT_BYTES,
            "oversized usage report"
        );
        ensure!(
            self.after.count + self.entries.len() <= self.head.count,
            "report past source head"
        );
        for row in &self.entries {
            row.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub binding: Binding,
    pub generation: String,
    pub identity: Identity,
    pub entries: Vec<Entry>,
    pub cursor: Cursor,
    pub high_water: usize,
    pub announced: BTreeMap<usize, String>,
    pub gap: Option<Gap>,
}
impl Source {
    pub fn new(binding: Binding, generation: String, identity: Identity) -> Self {
        Self {
            binding,
            generation,
            identity,
            entries: vec![],
            cursor: Cursor::default(),
            high_water: 0,
            announced: BTreeMap::new(),
            gap: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.identity.validate(&self.binding)?;
        ensure!(
            uuid::Uuid::parse_str(&self.generation).is_ok(),
            "invalid usage generation"
        );
        ensure!(
            self.entries.len() <= MAX_ENTRIES && self.cursor.count == self.entries.len(),
            "invalid usage source length"
        );
        ensure!(
            self.high_water >= self.cursor.count && self.high_water <= MAX_ENTRIES,
            "invalid announced source extent"
        );
        for (count, promise) in &self.announced {
            ensure!(
                *count > self.cursor.count && *count <= self.high_water,
                "invalid unresolved source promise"
            );
            digest(promise)?;
        }
        ensure!(
            self.high_water == self.cursor.count || self.announced.contains_key(&self.high_water),
            "announced suffix was forgotten"
        );
        let mut ids = BTreeSet::new();
        let mut models = BTreeSet::new();
        let mut cursor = Cursor::default();
        for row in &self.entries {
            row.validate()?;
            ensure!(ids.insert(&row.id), "duplicate source entry identity");
            if let Some(model) = &row.model {
                models.insert(model);
            }
            cursor = cursor.append(row)?;
        }
        ensure!(models.len() <= 16, "usage model capacity exceeded");
        ensure!(cursor == self.cursor, "usage source hash mismatch");
        Ok(())
    }
    pub fn complete(&self) -> bool {
        self.gap.is_none() && self.high_water == self.cursor.count && self.announced.is_empty()
    }
    pub fn prefix(&self, count: usize) -> Result<Cursor> {
        ensure!(count <= self.entries.len(), "unknown usage prefix");
        self.entries[..count]
            .iter()
            .try_fold(Cursor::default(), |c, e| c.append(e))
    }
    pub fn reconcile(
        &mut self,
        binding: &Binding,
        generation: &str,
        report: &Report,
    ) -> Result<bool> {
        report.validate()?;
        report.identity.validate(binding)?;
        ensure!(self.gap.is_none(), "usage source parked");
        ensure!(
            self.identity == report.identity
                && self.binding.profile == binding.profile
                && self.binding.instance_id == binding.instance_id
                && self.binding.native_id == binding.native_id,
            "usage source identity changed"
        );
        ensure!(
            self.prefix(report.after.count)? == report.after,
            "usage source prefix changed"
        );
        ensure!(
            report.head.count >= self.high_water,
            "announced source extent regressed"
        );
        let mut next = self.clone();
        next.high_water = report.head.count;
        if let Some(old) = next.announced.get(&report.head.count) {
            ensure!(*old == report.head.hash, "announced source head changed");
        }
        if report.head.count > next.cursor.count {
            next.announced
                .insert(report.head.count, report.head.hash.clone());
        }
        for (offset, row) in report.entries.iter().enumerate() {
            let index = report.after.count + offset;
            if let Some(old) = next.entries.get(index) {
                ensure!(old == row, "source entry changed on replay");
            } else {
                next.cursor = next.cursor.append(row)?;
                next.entries.push(row.clone());
            }
        }
        if report.head.count <= next.entries.len() {
            ensure!(
                next.prefix(report.head.count)? == report.head,
                "source head changed"
            );
        }
        let resolved: Vec<_> = next
            .announced
            .range(..=next.cursor.count)
            .map(|(n, h)| (*n, h.clone()))
            .collect();
        for (count, promise) in resolved {
            ensure!(
                next.prefix(count)?.hash == promise,
                "previously announced source prefix changed"
            );
            next.announced.remove(&count);
        }
        if report.head == next.cursor {
            next.binding = binding.clone();
            next.generation = generation.into();
        }
        next.validate()?;
        let changed = *self != next;
        *self = next;
        Ok(changed)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Quote {
    pub model: Model,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_1h: Option<u64>,
}
impl Quote {
    fn validate(&self) -> Result<()> {
        self.model.validate()?;
        ensure!(
            [self.input, self.output, self.cache_read, self.cache_write]
                .into_iter()
                .all(|n| n <= SAFE)
                && self.cache_write_1h.is_none_or(|n| n <= SAFE),
            "invalid fixed quotation"
        );
        Ok(())
    }
    fn value(&self, c: &Counters) -> Result<u64> {
        c.total()?;
        let long = if c.cache_write_1h == 0 {
            0
        } else {
            self.cache_write_1h
                .context("missing one-hour cache quote")?
        };
        let weighted = [
            (c.input, self.input),
            (c.output, self.output),
            (c.cache_read, self.cache_read),
            (c.cache_write - c.cache_write_1h, self.cache_write),
            (c.cache_write_1h, long),
        ]
        .into_iter()
        .try_fold(0u128, |sum, (count, rate)| {
            sum.checked_add(u128::from(count) * u128::from(rate))
                .context("valuation overflow")
        })?;
        let nano = weighted.div_ceil(1_000_000);
        ensure!(
            nano <= u128::from(SAFE),
            "valuation exceeds wire-safe integer range"
        );
        Ok(nano as u64)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Valuation {
    pub currency: String,
    pub basis: String,
    pub limit_nano: u64,
    pub quotes: Vec<Quote>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Policy {
    pub version: u32,
    pub basis: String,
    pub tokens: Option<u64>,
    pub usd: Option<Valuation>,
}
impl Policy {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let policy: Self = batch::read_private_bounded(path, 8192)?;
        policy.validate()?;
        Ok(policy)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.basis == "observed-native-entries"
                && (self.tokens.is_some() || self.usd.is_some()),
            "explicit observed usage assurance required"
        );
        ensure!(
            self.tokens.is_none_or(|n| (1..=SAFE).contains(&n)),
            "invalid token threshold"
        );
        if let Some(usd) = &self.usd {
            ensure!(
                usd.currency == "USD"
                    && usd.basis == "operator-fixed-rates"
                    && (1..=SAFE).contains(&usd.limit_nano),
                "unsupported currency or price assurance"
            );
            ensure!(
                !usd.quotes.is_empty() && usd.quotes.len() <= 16,
                "invalid quotation count"
            );
            let mut models = BTreeSet::new();
            for quote in &usd.quotes {
                quote.validate()?;
                ensure!(models.insert(&quote.model), "duplicate price quotation");
            }
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 8192,
            "usage policy exceeds 8 KiB"
        );
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Denial {
    Exhausted,
    Unavailable,
    Stale,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Decision {
    pub reason: Denial,
    pub shared: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SharedGuard {
    pub policy: Policy,
    pub blocked: Option<Denial>,
}
impl SharedGuard {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            blocked: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct Measurement {
    pub reported_tokens: u64,
    pub tokens_complete: bool,
    pub operator_estimate_nano: Option<u64>,
    pub known_operator_estimate_nano: Option<u64>,
    pub valuation_complete: bool,
    pub sdk_recorded_nano: Option<u64>,
    pub gaps: BTreeSet<Gap>,
    pub basis: &'static str,
    pub billing_coverage: &'static str,
}
impl Measurement {
    pub fn deny(&self, policy: &Policy) -> Option<Denial> {
        if (policy.tokens.is_some() && !self.tokens_complete)
            || (policy.usd.is_some() && !self.valuation_complete)
        {
            return Some(Denial::Unavailable);
        }
        if policy.tokens.is_some_and(|cap| self.reported_tokens >= cap)
            || policy.usd.as_ref().is_some_and(|u| {
                self.operator_estimate_nano
                    .is_some_and(|n| n >= u.limit_nano)
            })
        {
            Some(Denial::Exhausted)
        } else {
            None
        }
    }
}
pub(super) fn measure(
    source: &Source,
    start: &Cursor,
    end: &Cursor,
    policy: Option<&Policy>,
) -> Result<Measurement> {
    ensure!(
        start.count <= end.count
            && source.prefix(start.count)? == *start
            && source.prefix(end.count)? == *end,
        "usage interval changed"
    );
    if let Some(policy) = policy {
        policy.validate()?;
    }
    let mut m = Measurement {
        reported_tokens: 0,
        tokens_complete: source.complete(),
        operator_estimate_nano: policy.and_then(|p| p.usd.as_ref()).map(|_| 0),
        known_operator_estimate_nano: None,
        valuation_complete: source.complete() && policy.is_some_and(|p| p.usd.is_some()),
        sdk_recorded_nano: Some(0),
        gaps: BTreeSet::new(),
        basis: "reported SDK components; operator quote estimates are not invoices",
        billing_coverage:
            "independent, hidden retries, descendants and subscription billing unknown",
    };
    if let Some(gap) = source.gap {
        m.gaps.insert(gap);
    }
    if source.high_water > source.cursor.count {
        m.gaps.insert(Gap::Incomplete);
    }
    for row in &source.entries[start.count..end.count] {
        if matches!(row.kind, Kind::Metadata | Kind::User) {
            continue;
        }
        if let Some(gap) = row.gap {
            m.gaps.insert(gap);
            m.tokens_complete = false;
            m.valuation_complete = false;
        }
        if let Some(c) = &row.counters {
            m.reported_tokens = add(m.reported_tokens, c.total()?)?;
            match (m.sdk_recorded_nano, row.sdk_cost_nano) {
                (Some(total), Some(cost)) => m.sdk_recorded_nano = Some(add(total, cost)?),
                _ => m.sdk_recorded_nano = None,
            }
            if let Some(usd) = policy.and_then(|p| p.usd.as_ref()) {
                match row
                    .model
                    .as_ref()
                    .and_then(|model| usd.quotes.iter().find(|q| q.model == *model))
                {
                    Some(quote) => match quote.value(c) {
                        Ok(cost) => {
                            m.operator_estimate_nano =
                                Some(add(m.operator_estimate_nano.unwrap(), cost)?)
                        }
                        Err(_) => {
                            m.valuation_complete = false;
                            m.gaps.insert(Gap::Unpriced);
                        }
                    },
                    None => {
                        m.valuation_complete = false;
                        m.gaps.insert(if row.model.is_none() {
                            Gap::Unattributed
                        } else {
                            Gap::Unpriced
                        });
                    }
                }
            }
        } else {
            m.tokens_complete = false;
            m.valuation_complete = false;
            m.sdk_recorded_nano = None;
            m.gaps.insert(Gap::MissingUsage);
        }
    }
    m.known_operator_estimate_nano = m.operator_estimate_nano;
    if !m.valuation_complete {
        m.operator_estimate_nano = None;
    }
    if !m.tokens_complete {
        m.sdk_recorded_nano = None;
    }
    Ok(m)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Window {
    pub binding: Binding,
    pub generation: String,
    pub source: Option<String>,
    pub baseline: Option<Cursor>,
    pub cutoff: Option<Cursor>,
    pub observed_through: Option<Cursor>,
    pub closing: bool,
    pub gap: Option<Gap>,
    pub policy: Option<Policy>,
    pub blocked: Option<Denial>,
    pub ordinary_recovered: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    pub sources: BTreeMap<String, Source>,
    pub windows: BTreeMap<String, Window>,
    pub batches: BTreeMap<String, SharedGuard>,
}
impl State {
    pub fn validate(
        &self,
        profile: &str,
        sessions: &BTreeMap<String, authority::Session>,
        batches: &BTreeMap<String, batch::state::Operation>,
    ) -> Result<()> {
        self.validate_refs(
            profile,
            |id| sessions.contains_key(id),
            |session, id| {
                sessions
                    .get(session)
                    .and_then(|s| s.book.batch_facts(id))
                    .map(|f| (f.binding, f.generation, f.ended))
            },
            |id| {
                batches.get(id).map(|b| {
                    b.members
                        .values()
                        .filter_map(|m| m.window.clone())
                        .collect()
                })
            },
        )
    }
    pub fn validate_refs(
        &self,
        profile: &str,
        session_exists: impl Fn(&str) -> bool,
        window_lookup: impl Fn(&str, &str) -> Option<(Binding, String, bool)>,
        members: impl Fn(&str) -> Option<Vec<String>>,
    ) -> Result<()> {
        ensure!(self.sources.len() <= 4096, "usage source capacity exceeded");
        let mut native_sources = BTreeSet::new();
        for (key, source) in &self.sources {
            source.validate()?;
            ensure!(
                *key == source.identity.key(&source.binding)?
                    && source.binding.profile == profile
                    && session_exists(&source.binding.instance_id)
                    && native_sources
                        .insert((&source.binding.instance_id, &source.binding.native_id)),
                "usage source outside profile/session"
            );
        }
        for (id, window) in &self.windows {
            let (binding, generation, ended) = window_lookup(&window.binding.instance_id, id)
                .context("usage window lacks authority history")?;
            ensure!(
                window.binding == binding && window.generation == generation,
                "usage window identity mismatch"
            );
            ensure!(
                window.closing == ended,
                "authority/accounting terminal state disagrees"
            );
            ensure!(
                window.cutoff.is_none() || ended,
                "closed accounting retains live authority"
            );
            ensure!(
                !window.ordinary_recovered || ended,
                "ordinary usage recovery retained live authority"
            );
            if let Some(policy) = &window.policy {
                policy.validate()?;
            }
            match (&window.source, &window.baseline) {
                (Some(key), Some(base)) => {
                    let source = self.sources.get(key).context("usage source missing")?;
                    ensure!(
                        source.binding.instance_id == window.binding.instance_id
                            && source.binding.native_id == window.binding.native_id
                            && source.prefix(base.count)? == *base,
                        "invalid usage baseline"
                    );
                    if let Some(observed) = &window.observed_through {
                        ensure!(
                            base.count <= observed.count
                                && source.prefix(observed.count)? == *observed,
                            "invalid observed usage watermark"
                        );
                    }
                    if let Some(end) = &window.cutoff {
                        ensure!(
                            window.closing
                                && base.count <= end.count
                                && source.prefix(end.count)? == *end
                                && window.observed_through.as_ref() == Some(end),
                            "invalid usage closing watermark"
                        );
                    }
                }
                (None, None) => ensure!(
                    window.cutoff.is_none() && window.observed_through.is_none(),
                    "uncollected interval invented a cutoff"
                ),
                _ => anyhow::bail!("incomplete usage baseline"),
            }
        }
        for (id, guard) in &self.batches {
            guard.policy.validate()?;
            for window in members(id).context("shared usage policy lacks batch")? {
                ensure!(
                    self.windows.contains_key(&window),
                    "shared usage member baseline missing"
                );
            }
        }
        Ok(())
    }
    pub fn sync_lifetime(
        &mut self,
        sessions: &mut BTreeMap<String, authority::Session>,
    ) -> Result<()> {
        for (id, window) in &mut self.windows {
            let session = sessions
                .get_mut(&window.binding.instance_id)
                .context("usage session missing")?;
            let facts = session
                .book
                .batch_facts(id)
                .context("usage window missing")?;
            if facts.ended {
                window.closing = true;
            }
            if window.closing && session.book.stop_for_usage(id) {
                session.epoch = session
                    .epoch
                    .checked_add(1)
                    .context("control epoch overflow")?;
            }
        }
        Ok(())
    }
    pub fn transition(&self, next: &Self) -> Result<()> {
        for (key, old) in &self.sources {
            let source = next.sources.get(key).context("usage source removed")?;
            ensure!(
                source.high_water >= old.high_water,
                "announced extent reset"
            );
            for (count, promise) in &old.announced {
                if *count <= source.cursor.count {
                    ensure!(
                        source.prefix(*count)?.hash == *promise,
                        "announced prefix changed"
                    );
                } else {
                    ensure!(
                        source.announced.get(count) == Some(promise),
                        "unresolved source promise removed"
                    );
                }
            }
            ensure!(
                source.identity == old.identity && source.entries.starts_with(&old.entries),
                "usage source changed or refunded"
            );
            ensure!(
                old.gap.is_none() || old.gap == source.gap,
                "parked source repaired implicitly"
            );
        }
        for (id, old) in &self.windows {
            let window = next.windows.get(id).context("usage interval removed")?;
            ensure!(
                old.binding == window.binding
                    && old.generation == window.generation
                    && old.source == window.source
                    && old.baseline == window.baseline
                    && old.policy == window.policy,
                "usage baseline/policy changed"
            );
            ensure!(
                old.observed_through.as_ref().is_none_or(|old| window
                    .observed_through
                    .as_ref()
                    .is_some_and(|next| next.count >= old.count)),
                "observed usage regressed"
            );
            ensure!(
                old.cutoff.is_none() || old.cutoff == window.cutoff,
                "usage cutoff moved"
            );
            ensure!(!old.closing || window.closing, "usage closure reopened");
            ensure!(
                !old.ordinary_recovered || window.ordinary_recovered,
                "ordinary recovery reverted"
            );
            ensure!(
                old.blocked.is_none() || old.blocked == window.blocked,
                "usage denial cleared"
            );
            ensure!(
                old.gap.is_none() || old.gap == window.gap,
                "usage attribution gap cleared"
            );
        }
        for (id, old) in &self.batches {
            let guard = next.batches.get(id).context("shared usage guard removed")?;
            ensure!(guard.policy == old.policy, "shared usage policy changed");
            ensure!(
                old.blocked.is_none() || old.blocked == guard.blocked,
                "shared usage denial cleared"
            );
        }
        Ok(())
    }
    pub fn session_view(&self, id: &str, native: Option<&str>) -> Value {
        let Some(native) = native else {
            return serde_json::json!({"coverage":"unavailable","reason":"native identity unobserved"});
        };
        let mut sources = self
            .sources
            .values()
            .filter(|s| s.binding.instance_id == id && s.binding.native_id == native);
        let Some(source) = sources.next() else {
            return serde_json::json!({"coverage":"not_collected"});
        };
        if sources.next().is_some() {
            return serde_json::json!({"coverage":"unavailable","reason":"ambiguous source identity"});
        }
        let measurement = measure(source, &Cursor::default(), &source.cursor, None);
        serde_json::json!({"coverage":if measurement.as_ref().is_ok_and(|m|m.tokens_complete){"stored_reported"}else{"partial_or_unavailable"},"native_id":native,"cursor":source.cursor,"announced_head":source.high_water,"pending_promises":source.announced.len(),"source_gap":source.gap,"forked_source":source.identity.parent.is_some(),"measurement":measurement.as_ref().ok(),"unavailable":measurement.err().map(|e|e.to_string()),"freshness":"stored; not a live lease observation","sdk_cost_basis":"recorded SDK estimate; price provenance and subscription billing unknown"})
    }
    pub fn window_view(&self, id: &str) -> Value {
        let Some(window) = self.windows.get(id) else {
            return serde_json::json!({"coverage":"not_collected","guard":null});
        };
        let source = window.source.as_ref().and_then(|key| self.sources.get(key));
        let measurement = (|| -> Result<_> {
            let source = source.context("source unavailable")?;
            let mut value = measure(
                source,
                window.baseline.as_ref().context("baseline unavailable")?,
                window
                    .observed_through
                    .as_ref()
                    .context("observed watermark unavailable")?,
                window.policy.as_ref(),
            )?;
            if let Some(gap) = window.gap {
                value.tokens_complete = false;
                value.valuation_complete = false;
                value.operator_estimate_nano = None;
                value.gaps.insert(gap);
            }
            if window.closing && window.cutoff.is_none() {
                value.tokens_complete = false;
                value.valuation_complete = false;
                value.operator_estimate_nano = None;
                value.gaps.insert(Gap::Closing);
            }
            Ok(value)
        })();
        serde_json::json!({"guard":window.policy,"baseline":window.baseline,"cutoff":window.cutoff,"closing":window.closing,"gap":window.gap,"blocked":window.blocked,"ordinary_recovered":window.ordinary_recovered,"observed_through":window.observed_through,"source_cursor":source.map(|s|&s.cursor),"source_complete":source.is_some_and(Source::complete),"measurement":measurement.as_ref().ok(),"unavailable":measurement.err().map(|e|e.to_string()),"freshness":"stored; not a live lease observation"})
    }
}

pub(super) fn shared_policy<'a>(
    snapshot: &'a authority::Snapshot,
    window: &str,
) -> Option<&'a Policy> {
    let operation = snapshot.batches.values().find(|o| {
        o.members
            .values()
            .any(|m| m.window.as_deref() == Some(window))
    })?;
    snapshot.usage.batches.get(&operation.id).map(|g| &g.policy)
}
pub(super) fn shared_denial(snapshot: &authority::Snapshot, window: &str) -> Option<Denial> {
    let op = snapshot.batches.values().find(|o| {
        o.members
            .values()
            .any(|m| m.window.as_deref() == Some(window))
    })?;
    snapshot.usage.batches.get(&op.id)?.blocked
}
pub(super) fn policy_hash(snapshot: &authority::Snapshot, window: &str) -> Result<Option<String>> {
    let local = snapshot
        .usage
        .windows
        .get(window)
        .and_then(|w| w.policy.as_ref());
    let shared = shared_policy(snapshot, window);
    if local.is_none() && shared.is_none() {
        Ok(None)
    } else {
        Ok(Some(fingerprint(&(local, shared))?))
    }
}

pub(super) fn batch_view(snapshot: &authority::Snapshot, id: &str) -> Value {
    let guard = snapshot.usage.batches.get(id);
    let Some(batch) = snapshot.batches.get(id) else {
        return Value::Null;
    };
    let mut value = snapshot.usage.members_view(
        batch.members.values().filter_map(|m| m.window.as_deref()),
        guard.map(|g| &g.policy),
    );
    value["blocked"] = serde_json::to_value(guard.and_then(|g| g.blocked)).unwrap_or(Value::Null);
    value
}
impl State {
    pub fn members_view<'a>(
        &self,
        members: impl IntoIterator<Item = &'a str>,
        policy: Option<&Policy>,
    ) -> Value {
        let mut tokens = 0;
        let mut cost = 0;
        let mut complete = true;
        let mut gaps = Vec::new();
        let mut natives = BTreeSet::new();
        let mut files = BTreeSet::new();
        for id in members {
            let result = (|| -> Result<_> {
                let w = self.windows.get(id).context("not collected")?;
                let s = self
                    .sources
                    .get(w.source.as_ref().context("source unavailable")?)
                    .context("source missing")?;
                ensure!(
                    s.identity.parent.is_none()
                        && natives.insert(&s.identity.native_id)
                        && files.insert(&s.identity.file),
                    "ambiguous copied/forked usage source"
                );
                let mut measured = measure(
                    s,
                    w.baseline.as_ref().context("baseline missing")?,
                    w.observed_through.as_ref().context("observation missing")?,
                    policy,
                )?;
                if w.gap.is_some() || (w.closing && w.cutoff.is_none()) {
                    measured.tokens_complete = false;
                    measured.valuation_complete = false;
                }
                Ok(measured)
            })();
            match result {
                Ok(m) => {
                    if let (Ok(t), Ok(c)) = (
                        add(tokens, m.reported_tokens),
                        add(cost, m.known_operator_estimate_nano.unwrap_or(0)),
                    ) {
                        tokens = t;
                        cost = c;
                    } else {
                        complete = false;
                        gaps.push("amount overflow".to_string());
                    }
                    complete &= m.tokens_complete
                        && (policy.and_then(|p| p.usd.as_ref()).is_none() || m.valuation_complete);
                }
                Err(e) => {
                    complete = false;
                    gaps.push(e.to_string());
                }
            }
        }
        serde_json::json!({"guard":policy,"reported_tokens":if complete{Some(tokens)}else{None},"known_reported_tokens":tokens,"known_operator_estimate_nano":policy.and_then(|p|p.usd.as_ref()).map(|_|cost),"operator_estimate_nano":if complete{policy.and_then(|p|p.usd.as_ref()).map(|_|cost)}else{None},"coverage":if complete{"stored_reported_interval"}else{"partial_or_unavailable"},"gaps":gaps,"freshness":"stored; not a live lease observation","billing":"not an invoice or account-wide total"})
    }
}

pub(super) fn pending_projection(value: &mut Value) {
    value["negative_evidence_pending"] = Value::Bool(true);
    value["coverage"] = Value::String("partial_or_unavailable".into());
    value["unavailable"] =
        Value::String("unreconciled observed evidence; stored amounts are incomplete".into());
    if let Some(m) = value.get_mut("measurement").and_then(Value::as_object_mut) {
        m.insert("tokens_complete".into(), Value::Bool(false));
        m.insert("valuation_complete".into(), Value::Bool(false));
        m.insert("operator_estimate_nano".into(), Value::Null);
        m.insert("sdk_recorded_nano".into(), Value::Null);
    }
    if value["window"].is_object() {
        pending_projection(&mut value["window"]);
    }
    if value.get("operator_estimate_nano").is_some() {
        value["operator_estimate_nano"] = Value::Null;
    }
    if value.get("reported_tokens").is_some() {
        value["reported_tokens"] = Value::Null;
    }
}

pub(super) fn recover(
    store: &Store,
    binding: &Binding,
    id: &str,
    validate: impl Fn() -> Result<()>,
) -> Result<Value> {
    let mut tx = authority::SessionTransaction::open(store)?;
    validate()?;
    let mut next = tx.snapshot().clone();
    let window = next
        .usage
        .windows
        .get_mut(id)
        .context("unknown usage interval")?;
    ensure!(
        window.binding.profile == binding.profile
            && window.binding.instance_id == binding.instance_id
            && window.binding.native_id == binding.native_id,
        "usage recovery identity mismatch"
    );
    if window.ordinary_recovered {
        return Ok(serde_json::json!({"ordinary_recovered":true}));
    }
    window.ordinary_recovered = true;
    window.closing = true;
    let session = next
        .sessions
        .get_mut(&binding.instance_id)
        .context("usage recovery session missing")?;
    if session.book.stop_for_usage(id) {
        session.epoch = session
            .epoch
            .checked_add(1)
            .context("control epoch overflow")?;
    }
    tx.publish(next)?.warn();
    Ok(serde_json::json!({"ordinary_recovered":true,"authority_restored":false}))
}

#[cfg(test)]
mod tests;
