//! Private, bounded one-cycle delegation ledger. Model claims are not host authority.
use super::*;
use crate::session::AnchoredDir;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path};

pub(crate) const SESSION_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const FRAME_BYTES: usize = 256 * 1024;
const WINDOW_BYTES: usize = 1024 * 1024;
const RECORD_BYTES: usize = 8 * 1024;
const FILE_BYTES: usize = 64 * 1024;
const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
#[cfg(test)]
thread_local! { static FAIL_AFTER_ATTEMPT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
const PROTOCOL: u32 = 2;
const COVERAGE: &str = "stock-sdk-main-loop-reservations";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    version: u32,
    task: String,
    scope: String,
    files: Vec<Target>,
    requests: u8,
    assurance: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    path: String,
    capability: Capability,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Capability {
    Read,
    Create,
    Replace,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    id: String,
    question: String,
    alternatives: Vec<Alternative>,
    criteria: Vec<Criterion>,
    evidence: Vec<String>,
    assumptions: Vec<String>,
    uncertainties: Vec<String>,
    recommendation: String,
    rationale: String,
    rollback: String,
    reversible: bool,
    risk: Risk,
    human_required: bool,
    depends_on: Vec<String>,
    action: Option<Action>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Alternative {
    name: String,
    pros: String,
    cons: String,
    scores: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Criterion {
    name: String,
    weight: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Risk {
    Low,
    High,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    path: String,
    expected_hash: Option<String>,
    content: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    id: String,
    payload_hash: String,
    claims: Option<Decision>,
    request: u8,
    recorded_at_ms: i64,
    disposition: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Permit {
    decision: String,
    path: String,
    expected_hash: Option<String>,
    payload_hash: String,
    content: Option<String>,
    preimage: Option<String>,
    record_request: u8,
    attempted_at_ms: Option<i64>,
    outcomes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    version: u32,
    binding: Binding,
    generation: String,
    id: String,
    revision: u64,
    root: PathBuf,
    control_root: PathBuf,
    root_identity: (u64, u64),
    grant: Grant,
    issued_at_ms: i64,
    expires_at_ms: i64,
    state: String,
    confirmed: bool,
    reservations: Vec<String>,
    reads: u8,
    records: Vec<Record>,
    permit: Option<Permit>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Book {
    version: u32,
    windows: Vec<Window>,
}

fn hash(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn bounded(value: &impl Serialize, limit: usize) -> Result<()> {
    ensure!(
        serde_json::to_vec(value)?.len() <= limit,
        "delegation capacity exceeded"
    );
    Ok(())
}
fn text(value: &str) -> Result<()> {
    ensure!(
        !value
            .chars()
            .any(|c| (c.is_control() && c != '\n' && c != '\t') || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}')),
        "unsafe control character"
    );
    let lower = value.to_ascii_lowercase();
    ensure!(
        ![
            "private key",
            "-----begin",
            "authorization:",
            "bearer ",
            "api_key",
            "api-key",
            "password=",
            "secret=",
            "sk-",
            "ghp_",
            "github_pat_",
            "akia"
        ]
        .iter()
        .any(|s| lower.contains(s)),
        "detected credential material"
    );
    Ok(())
}
fn private_text(value: &Value) -> Result<()> {
    match value {
        Value::String(s) => text(s)?,
        Value::Array(values) => {
            for v in values {
                private_text(v)?;
            }
        }
        Value::Object(values) => {
            for (k, v) in values {
                text(k)?;
                private_text(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn target_path(path: &str) -> Result<()> {
    ensure!(!path.is_empty() && path.len() <= 256, "invalid target path");
    text(path)?;
    ensure!(!path.chars().any(char::is_control), "invalid target path");
    for component in Path::new(path).components() {
        let Component::Normal(part) = component else {
            anyhow::bail!("non-relative target");
        };
        let name = part
            .to_str()
            .context("non-UTF8 target")?
            .to_ascii_lowercase();
        ensure!(
            !name.starts_with('.')
                && ![
                    "node_modules",
                    "target",
                    "profiles",
                    "credentials",
                    "secrets",
                    "auth.json",
                    "package.json",
                    "agents.md",
                    "claude.md"
                ]
                .contains(&name.as_str())
                && !["secret", "password", "credential", "token"]
                    .iter()
                    .any(|s| name.contains(s)),
            "protected target"
        );
    }
    ensure!(
        matches!(
            Path::new(path).extension().and_then(|s| s.to_str()),
            Some("txt" | "md" | "rs" | "json")
        ),
        "unsupported file type"
    );
    Ok(())
}
impl Grant {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == PROTOCOL && self.assurance == COVERAGE,
            "unsupported assurance or grant version"
        );
        ensure!(
            (1..=8).contains(&self.requests) && (1..=16).contains(&self.files.len()),
            "invalid finite limits"
        );
        ensure!(
            !self.task.is_empty() && !self.scope.is_empty(),
            "task and scope required"
        );
        bounded(self, RECORD_BYTES)?;
        text(&self.task)?;
        text(&self.scope)?;
        for (i, file) in self.files.iter().enumerate() {
            target_path(&file.path)?;
            ensure!(
                !self.files[..i].iter().any(|f| f.path == file.path),
                "duplicate grant target"
            );
        }
        Ok(())
    }
}
impl Book {
    fn load(store: &Store) -> Result<Self> {
        let book = store.read::<Self>("ledger.json")?.unwrap_or(Self {
            version: PROTOCOL,
            windows: vec![],
        });
        book.validate()?;
        Ok(book)
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.version == PROTOCOL, "unsupported ledger version");
        bounded(self, SESSION_BYTES)?;
        for (i, w) in self.windows.iter().enumerate() {
            bounded(w, WINDOW_BYTES)?;
            w.grant.validate()?;
            ensure!(
                w.version == PROTOCOL
                    && uuid::Uuid::parse_str(&w.id).is_ok()
                    && uuid::Uuid::parse_str(&w.generation).is_ok(),
                "invalid ledger identity"
            );
            ensure!(
                w.revision > 0
                    && w.revision <= 9_007_199_254_740_991
                    && (i == 0 || self.windows[i - 1].revision < w.revision)
                    && w.root.is_absolute(),
                "invalid window revision/root"
            );
            ensure!(
                !self.windows[..i].iter().any(|old| old.id == w.id),
                "duplicate window"
            );
            ensure!(
                w.reservations.len() <= usize::from(w.grant.requests)
                    && w.reads <= 16
                    && w.records.len() <= 4,
                "corrupt counters"
            );
            ensure!(
                w.expires_at_ms
                    .checked_sub(w.issued_at_ms)
                    .is_some_and(|duration| (60_000..=86_400_000).contains(&duration))
                    && matches!(w.state.as_str(), "pending" | "owned" | "ended"),
                "invalid lifecycle"
            );
            for (index, request) in w.reservations.iter().enumerate() {
                ensure!(
                    uuid::Uuid::parse_str(request).is_ok()
                        && !w.reservations[..index].contains(request),
                    "corrupt reservations"
                );
            }
            ensure!(
                w.state != "pending" || w.reservations.is_empty(),
                "pending window has reservations"
            );
            for (index, record) in w.records.iter().enumerate() {
                ensure!(
                    uuid::Uuid::parse_str(&record.id).is_ok()
                        && !w.records[..index].iter().any(|r| r.id == record.id)
                        && record.request > 0
                        && usize::from(record.request) <= w.reservations.len(),
                    "corrupt record identity/boundary"
                );
                if let Some(claims) = &record.claims {
                    bounded(claims, RECORD_BYTES)?;
                    ensure!(
                        hash(serde_json::to_vec(claims)?) == record.payload_hash,
                        "corrupt decision"
                    );
                    ensure!(record.id == claims.id, "record identity changed");
                    private_text(&serde_json::to_value(claims)?)?;
                }
            }
            if let Some(p) = &w.permit {
                ensure!(
                    p.content
                        .as_ref()
                        .is_none_or(|s| s.len() <= FILE_BYTES && hash(s) == p.payload_hash)
                        && p.preimage.as_ref().is_none_or(|s| s.len() <= FILE_BYTES)
                        && p.outcomes.len() <= 2,
                    "corrupt permit"
                );
                ensure!(
                    w.records.iter().any(|r| r.id == p.decision
                        && r.request == p.record_request
                        && r.disposition == "admitted"),
                    "orphan permit"
                );
                ensure!(
                    p.preimage.as_ref().map(hash) == p.expected_hash
                        || (p.content.is_none() && p.preimage.is_none()),
                    "corrupt preimage"
                );
                ensure!(
                    (p.attempted_at_ms.is_none() && p.outcomes.is_empty())
                        || (p.attempted_at_ms.is_some()
                            && (p.outcomes == ["unknown"]
                                || p.outcomes == ["unknown", "observed_applied"])),
                    "corrupt action outcomes"
                );
                if let Some(claims) = w
                    .records
                    .iter()
                    .find(|r| r.id == p.decision)
                    .and_then(|r| r.claims.as_ref())
                {
                    let action = claims
                        .action
                        .as_ref()
                        .context("admitted record lacks action")?;
                    ensure!(
                        action.path == p.path
                            && action.expected_hash == p.expected_hash
                            && hash(&action.content) == p.payload_hash
                            && !claims.human_required
                            && claims.depends_on.is_empty()
                            && claims.reversible
                            && claims.risk == Risk::Low,
                        "permit differs from admitted decision"
                    );
                }
            }
        }
        Ok(())
    }
    fn save(&self, store: &Store) -> Result<()> {
        self.validate()?;
        store.write("ledger.json", self)
    }
    fn prune(&mut self, now: i64) {
        for w in &mut self.windows {
            let ambiguous = w.permit.as_ref().is_some_and(|p| {
                p.attempted_at_ms.is_some() && !p.outcomes.iter().any(|o| o == "observed_applied")
            });
            if now.saturating_sub(w.expires_at_ms) > RETENTION_MS && !ambiguous {
                for r in &mut w.records {
                    r.claims = None;
                }
                if let Some(p) = &mut w.permit {
                    p.content = None;
                    p.preimage = None;
                }
                w.state = "ended".into();
            }
        }
    }
}
impl Window {
    fn live(&self, binding: &Binding, generation: &str, now: i64) -> Result<()> {
        ensure!(
            self.binding == *binding && self.generation == generation,
            "stale delegation generation/binding"
        );
        ensure!(
            self.confirmed
                && self.state != "ended"
                && now >= self.issued_at_ms
                && now < self.expires_at_ms,
            "delegation ended or expired"
        );
        Ok(())
    }
    fn request(&self, request: &str) -> Result<u8> {
        ensure!(
            self.state == "owned" && self.reservations.last().is_some_and(|r| r == request),
            "not the current admitted request"
        );
        Ok(self.reservations.len() as u8)
    }
    fn workspace(&self) -> Result<AnchoredDir> {
        let root = AnchoredDir::open(&self.root)?;
        let (dev, ino) = root.identity()?;
        ensure!(
            (dev as u64, ino) == self.root_identity,
            "workspace replaced"
        );
        Ok(root)
    }
    fn file(&self, path: &str) -> Result<(AnchoredDir, String, Option<String>, u32)> {
        target_path(path)?;
        ensure!(
            self.grant.files.iter().any(|f| f.path == path),
            "target outside grant"
        );
        ensure!(
            !self.root.join(path).starts_with(&self.control_root),
            "control store is not a workspace target"
        );
        let root = self.workspace()?;
        let path = Path::new(path);
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or(Ok(root), |p| self.workspace()?.child(p))?;
        let leaf = path.file_name().unwrap().to_str().unwrap().to_owned();
        let Some(stat) = parent.entry_stat(Path::new(&leaf))? else {
            return Ok((parent, leaf, None, 0o600));
        };
        ensure!(
            stat.st_mode & libc::S_IFMT == libc::S_IFREG
                && stat.st_mode & 0o111 == 0
                && stat.st_nlink == 1
                && stat.st_uid == unsafe { libc::geteuid() },
            "unsafe/executable file"
        );
        let file = parent
            .open_regular(Path::new(&leaf), FILE_BYTES)?
            .context("unsafe or oversized target")?;
        let meta = file.metadata()?;
        ensure!(
            meta.nlink() == 1
                && meta.mode() & 0o111 == 0
                && meta.ino() == stat.st_ino
                && meta.uid() == unsafe { libc::geteuid() },
            "target changed"
        );
        let mut bytes = Vec::new();
        file.take((FILE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= FILE_BYTES, "oversized target");
        let body = String::from_utf8(bytes).context("only UTF8 files supported")?;
        text(&body)?;
        ensure!(!body.starts_with("#!"), "executable content");
        Ok((parent, leaf, Some(body), meta.mode() & 0o666))
    }
    fn view(&self, now: i64) -> Value {
        let mut records = self.records.clone();
        for record in &mut records {
            if let Some(claims) = &mut record.claims {
                claims.action = None;
            }
        }
        json!({"version":PROTOCOL,"window":self.id,"revision":self.revision,"binding":self.binding,"generation":self.generation,
            "state":if now >= self.expires_at_ms {"ended"} else {&self.state},"grant":self.grant,
            "confirmed":self.confirmed,"grant_hash":hash(serde_json::to_vec(&self.grant).unwrap()),"expires_at_ms":self.expires_at_ms,"issued_at_ms":self.issued_at_ms,
            "reservations_used":self.reservations.len(),"reads_used":self.reads,"records":records,
            "action":self.permit.as_ref().map(|p| json!({"decision":p.decision,"path":p.path,"expected_hash":p.expected_hash,"payload_hash":p.payload_hash,"attempted_at_ms":p.attempted_at_ms,"outcomes":p.outcomes})),
            "coverage":COVERAGE,"background_coverage":"independent/unknown; not physical requests or billing",
            "privacy":"heuristic credential filtering; native Pi/provider history has separate retention"})
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Reserve {
        window: String,
        request: String,
    },
    Stop {
        window: String,
    },
    Read {
        window: String,
        request: String,
        path: String,
    },
    Record {
        window: String,
        request: String,
        decision: Value,
    },
    Apply {
        window: String,
        request: String,
        decision: String,
    },
}
pub(super) fn snapshot(store: &Store) -> Result<Value> {
    let book = Book::load(store)?;
    Ok(book
        .windows
        .last()
        .map_or(Value::Null, |w| w.view(now_ms())))
}
pub(super) fn stop(store: &Store) -> Result<()> {
    let _lock = store.lock(TIMEOUT)?;
    let mut book = Book::load(store)?;
    if let Some(w) = book.windows.last_mut() {
        w.state = "ended".into();
        book.save(store)?;
    }
    Ok(())
}
#[cfg(test)]
pub(super) fn execute(
    store: &Store,
    binding: &Binding,
    generation: &str,
    command: Request,
    now: i64,
) -> Result<Value> {
    execute_checked(store, binding, generation, command, now, || Ok(()))
}
pub(super) fn execute_checked(
    store: &Store,
    binding: &Binding,
    generation: &str,
    command: Request,
    now: i64,
    validate: impl Fn() -> Result<()>,
) -> Result<Value> {
    let started = Instant::now();
    let _lock = store.lock(TIMEOUT)?;
    validate()?;
    let mut book = Book::load(store)?;
    let id = match &command {
        Request::Reserve { window, .. }
        | Request::Stop { window }
        | Request::Read { window, .. }
        | Request::Record { window, .. }
        | Request::Apply { window, .. } => window,
    };
    let w = book.windows.last_mut().context("no delegation")?;
    ensure!(w.id == *id, "stale window");
    if matches!(command, Request::Stop { .. }) {
        ensure!(
            w.binding == *binding && w.generation == generation,
            "stale stop"
        );
        w.state = "ended".into();
        book.save(store)?;
        return Ok(json!({"ended":true}));
    }
    w.live(
        binding,
        generation,
        now.saturating_add(started.elapsed().as_millis() as i64),
    )?;
    let result = match command {
        Request::Reserve { request, .. } => {
            ensure!(
                uuid::Uuid::parse_str(&request).is_ok(),
                "invalid request identity"
            );
            ensure!(
                !w.reservations.contains(&request),
                "request reservation cannot be replayed"
            );
            ensure!(
                w.reservations.len() < usize::from(w.grant.requests),
                "request reservations exhausted"
            );
            w.state = "owned".into();
            w.reservations.push(request);
            json!({"request_number":w.reservations.len(),"grant":w.grant})
        }
        Request::Read { request, path, .. } => {
            w.request(&request)?;
            ensure!(w.reads < 16, "read operations exhausted");
            w.reads += 1;
            // Consumption is durable even when the read subsequently fails.
            book.save(store)?;
            let w = book.windows.last().unwrap();
            let (_, _, body, _) = w.file(&path)?;
            return Ok(json!({"path":path,"hash":body.as_ref().map(hash),"content":body}));
        }
        Request::Record {
            request, decision, ..
        } => {
            let number = w.request(&request)?;
            bounded(&decision, RECORD_BYTES)?;
            let decision: Decision = serde_json::from_value(decision)?;
            bounded(&decision, RECORD_BYTES)?;
            let payload_hash = hash(serde_json::to_vec(&decision)?);
            if let Some(existing) = w.records.iter().find(|r| r.id == decision.id) {
                ensure!(
                    existing.payload_hash == payload_hash,
                    "conflicting decision id"
                );
                return Ok(
                    json!({"decision":existing.id,"disposition":existing.disposition,"record_request":existing.request}),
                );
            }
            ensure!(
                w.records.len() < 4 && uuid::Uuid::parse_str(&decision.id).is_ok(),
                "record limit/identity"
            );
            private_text(&serde_json::to_value(&decision)?)?;
            ensure!(
                !decision.question.is_empty()
                    && !decision.rationale.is_empty()
                    && !decision.rollback.is_empty()
                    && (2..=4).contains(&decision.alternatives.len())
                    && (1..=4).contains(&decision.criteria.len()),
                "incomplete weighted record"
            );
            ensure!(
                decision
                    .criteria
                    .iter()
                    .all(|c| (1..=5).contains(&c.weight))
                    && decision
                        .alternatives
                        .iter()
                        .all(|a| a.scores.len() == decision.criteria.len()
                            && a.scores.iter().all(|s| (1..=5).contains(s)))
                    && decision
                        .alternatives
                        .iter()
                        .any(|a| a.name == decision.recommendation),
                "invalid weighted recommendation"
            );
            let eligible = !decision.human_required
                && decision.depends_on.is_empty()
                && decision.reversible
                && decision.risk == Risk::Low;
            let disposition = if !eligible {
                "human_required"
            } else if decision.action.is_none() {
                "deferred"
            } else {
                ensure!(w.permit.is_none(), "one cycle already admitted");
                let action = decision.action.as_ref().unwrap();
                ensure!(
                    action.content.len() <= FILE_BYTES && !action.content.starts_with("#!"),
                    "unsafe payload"
                );
                text(&action.content)?;
                let (_, _, preimage, _) = w.file(&action.path)?;
                let capability = if preimage.is_some() {
                    Capability::Replace
                } else {
                    Capability::Create
                };
                ensure!(
                    w.grant
                        .files
                        .iter()
                        .any(|f| f.path == action.path && f.capability == capability),
                    "capability not granted"
                );
                ensure!(
                    preimage.as_ref().map(hash) == action.expected_hash,
                    "expected hash mismatch"
                );
                w.permit = Some(Permit {
                    decision: decision.id.clone(),
                    path: action.path.clone(),
                    expected_hash: action.expected_hash.clone(),
                    payload_hash: hash(&action.content),
                    content: Some(action.content.clone()),
                    preimage,
                    record_request: number,
                    attempted_at_ms: None,
                    outcomes: vec![],
                });
                "admitted"
            };
            let receipt =
                json!({"decision":decision.id,"disposition":disposition,"record_request":number});
            w.records.push(Record {
                id: decision.id.clone(),
                payload_hash,
                claims: Some(decision),
                request: number,
                recorded_at_ms: now,
                disposition: disposition.into(),
            });
            receipt
        }
        Request::Apply {
            request, decision, ..
        } => {
            let number = w.request(&request)?;
            let p = w.permit.as_ref().context("no action permit")?;
            ensure!(
                p.decision == decision && number > p.record_request,
                "apply requires a later admitted request, not a sibling"
            );
            ensure!(
                p.attempted_at_ms.is_none(),
                "action already attempted; no replay"
            );
            let (parent, leaf, body, mode) = w.file(&p.path)?;
            ensure!(
                body.as_ref().map(hash) == p.expected_hash,
                "target changed since record"
            );
            let content = p.content.clone().context("payload expired")?;
            let path = p.path.clone();
            let expected = p.expected_hash.clone();
            validate()?;
            w.live(
                binding,
                generation,
                now.saturating_add(started.elapsed().as_millis() as i64),
            )?;
            let p = w.permit.as_mut().unwrap();
            p.attempted_at_ms = Some(now);
            p.outcomes.push("unknown".into());
            w.state = "ended".into();
            // This durable commitment serializes revocation. A failed acknowledgement never refunds it.
            book.save(store)?;
            #[cfg(test)]
            if FAIL_AFTER_ATTEMPT.with(|flag| flag.replace(false)) {
                anyhow::bail!("injected crash after durable attempt");
            }
            let w = book.windows.last().unwrap();
            let effect = parent.publish_file(
                Path::new(&leaf),
                &mut Cursor::new(content),
                std::fs::Permissions::from_mode(mode),
                body.is_some(),
                Some(crate::session::anchored_fs::FilePublication {
                    staging: &parent,
                    validate: &|| {
                        let (current_parent, _, current_body, _) = w.file(&path)?;
                        ensure!(
                            current_parent.identity()? == parent.identity()?
                                && current_body.as_ref().map(hash) == expected,
                            "target changed before publication"
                        );
                        Ok(())
                    },
                }),
            );
            if matches!(effect, Ok(true)) {
                book.windows
                    .last_mut()
                    .unwrap()
                    .permit
                    .as_mut()
                    .unwrap()
                    .outcomes
                    .push("observed_applied".into());
                book.save(store)?;
                return Ok(json!({"outcome":"observed_applied"}));
            }
            anyhow::bail!("action outcome unknown; permit consumed, inspect private ledger")
        }
        Request::Stop { .. } => unreachable!(),
    };
    book.windows.last().unwrap().live(
        binding,
        generation,
        now.saturating_add(started.elapsed().as_millis() as i64),
    )?;
    book.save(store)?;
    Ok(result)
}

pub(crate) fn initialize() -> Result<()> {
    let app = AnchoredDir::open(&super::super::get_app_dir()?)?;
    let dir = app.create_child(Path::new("afk-runtime-v2"))?;
    let stat = dir.metadata()?;
    ensure!(
        stat.st_mode & 0o777 == 0o700 && stat.st_uid == unsafe { libc::geteuid() },
        "unsafe runtime namespace"
    );
    Ok(())
}

#[cfg(test)]
mod tests;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeProbe {
    version: u32,
    binding: Binding,
    challenge: String,
    window: Option<String>,
    grant_hash: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeAck {
    version: u32,
    binding: Binding,
    challenge: String,
    window: Option<String>,
    grant_hash: Option<String>,
    generation: String,
    sdk: String,
    coverage: String,
}
pub(super) fn acknowledge(
    store: &Store,
    binding: &Binding,
    generation: &str,
    ack: RuntimeAck,
) -> Result<()> {
    let probe = store
        .read::<RuntimeProbe>("runtime-probe.json")?
        .context("no runtime probe")?;
    ensure!(
        probe.version == PROTOCOL
            && ack.version == PROTOCOL
            && ack.binding == *binding
            && probe.binding == *binding
            && ack.generation == generation
            && ack.sdk == "0.87.1"
            && ack.coverage == COVERAGE
            && ack.challenge == probe.challenge
            && ack.window == probe.window
            && ack.grant_hash == probe.grant_hash,
        "incompatible/stale runtime acknowledgement"
    );
    store.write("runtime-ack.json", &ack)
}
fn exchange_runtime(
    store: &Store,
    binding: &Binding,
    window: Option<&Window>,
) -> Result<RuntimeAck> {
    let probe = RuntimeProbe {
        version: PROTOCOL,
        binding: binding.clone(),
        challenge: uuid::Uuid::new_v4().to_string(),
        window: window.map(|w| w.id.clone()),
        grant_hash: window.map(|w| hash(serde_json::to_vec(&w.grant).unwrap())),
    };
    store.write("runtime-probe.json", &probe)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(ack) = store.read::<RuntimeAck>("runtime-ack.json")? {
            if ack.challenge == probe.challenge
                && ack.version == PROTOCOL
                && ack.binding == *binding
                && ack.window == probe.window
                && ack.grant_hash == probe.grant_hash
                && ack.sdk == "0.87.1"
                && ack.coverage == COVERAGE
                && uuid::Uuid::parse_str(&ack.generation).is_ok()
            {
                return Ok(ack);
            }
        }
        ensure!(
            Instant::now() < deadline,
            "no fresh delegated capability acknowledgement; executable activation requires active ordinary work with an observable stop signal, not idle standby"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
pub fn delegate(instance: &Instance, minutes: u32, grant_file: &Path) -> Result<Value> {
    ensure!((1..=1440).contains(&minutes), "invalid expiry");
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(grant_file)?;
    ensure!(file.metadata()?.is_file(), "grant must be a regular file");
    let mut bytes = Vec::new();
    file.take((RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= RECORD_BYTES, "grant too large");
    let grant: Grant = serde_json::from_slice(&bytes)?;
    grant.validate()?;
    let binding = Binding::for_instance(instance)?;
    current_instance(&binding)?;
    let app_dir = super::super::get_app_dir()?;
    let control = Store::open(&app_dir, &instance.id, true)?;
    let runtime = Store::runtime(&app_dir, &instance.id, true)?;
    let _control_lock = control.lock(TIMEOUT)?;
    let ack = exchange_runtime(&control, &binding, None)?;
    let root = std::fs::canonicalize(&instance.project_path)?;
    ensure!(
        !root.starts_with(std::fs::canonicalize(&app_dir)?)
            && !["/nix", "/etc", "/var", "/System", "/Library"]
                .iter()
                .any(|p| root.starts_with(p)),
        "protected workspace root"
    );
    ensure!(
        !root
            .components()
            .any(|c| c.as_os_str().to_str().is_some_and(|name| [
                ".git",
                ".pi",
                ".ssh",
                ".config",
                ".local",
                ".nix-profile",
                "profiles",
                "node_modules"
            ]
            .contains(&name))),
        "protected workspace path"
    );
    let anchored = AnchoredDir::open(&root)?;
    let (dev, ino) = anchored.identity()?;
    let issued_at_ms = now_ms();
    let mut w = Window {
        version: PROTOCOL,
        binding,
        generation: ack.generation,
        id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
        root,
        control_root: std::fs::canonicalize(&app_dir)?,
        root_identity: (dev as u64, ino),
        grant,
        issued_at_ms,
        expires_at_ms: issued_at_ms + i64::from(minutes) * 60_000,
        state: "pending".into(),
        confirmed: false,
        reservations: vec![],
        reads: 0,
        records: vec![],
        permit: None,
    };
    for target in &w.grant.files {
        w.file(&target.path)?;
    }
    {
        let _lock = runtime.lock(TIMEOUT)?;
        let mut book = Book::load(&runtime)?;
        if let Some(previous) = book.windows.last_mut() {
            w.revision = previous
                .revision
                .checked_add(1)
                .context("revision overflow")?;
            previous.state = "ended".into();
        }
        book.prune(issued_at_ms);
        book.windows.push(w.clone());
        book.save(&runtime)?;
    }
    // Capability acknowledgement does not take the ledger lock.
    if let Err(error) = exchange_runtime(&control, &w.binding, Some(&w)) {
        stop(&runtime)?;
        return Err(error);
    }
    {
        let _lock = runtime.lock(TIMEOUT)?;
        current_instance(&w.binding)?;
        let mut book = Book::load(&runtime)?;
        let latest = book.windows.last_mut().context("window disappeared")?;
        ensure!(
            latest.id == w.id && latest.state == "pending",
            "window ended or changed before confirmation; start ordinary work, then request a fresh delegation"
        );
        latest.confirmed = true;
        book.save(&runtime)?;
    }
    snapshot(&runtime)
}
pub fn audit(instance: &Instance) -> Result<Value> {
    let store = Store::runtime(&super::super::get_app_dir()?, &instance.id, true)?;
    let _lock = store.lock(TIMEOUT)?;
    let mut book = Book::load(&store)?;
    book.prune(now_ms());
    book.save(&store)?;
    let views: Vec<_> = book.windows.iter().map(|w| w.view(now_ms())).collect();
    let output = json!({"windows":views});
    bounded(&output, SESSION_BYTES)?;
    Ok(output)
}

pub(super) fn inspect(instance: &Instance, store: &Store) -> Result<Value> {
    let book = Book::load(store)?;
    let Some(w) = book.windows.last() else {
        return Ok(Value::Null);
    };
    let mut view = w.view(now_ms());
    let observation = (|| -> Result<RuntimeAck> {
        let binding = Binding::for_instance(instance)?;
        current_instance(&binding)?;
        ensure!(binding == w.binding, "different current binding");
        let control = Store::open(&super::super::get_app_dir()?, &instance.id, false)?;
        let _lock = control.lock(TIMEOUT)?;
        exchange_runtime(&control, &binding, Some(w))
    })();
    match observation {
        Ok(ack) if ack.generation == w.generation => {
            view["observation"] = json!("fresh capability acknowledgement; pending activation requires live ordinary work, not idle standby");
            view["observed_at_ms"] = json!(now_ms());
        }
        Ok(_) => {
            view["state"] = json!("invalidated");
            view["observation"] =
                json!("runtime generation changed; fresh operator delegation required");
        }
        Err(_) => {
            view["observation"] =
                json!("unavailable: host requested state is not runtime confirmation");
        }
    }
    Ok(view)
}
