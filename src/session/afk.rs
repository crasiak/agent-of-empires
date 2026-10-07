//! Control-only AFK requests. This protocol never admits autonomous work.

mod bridge;
mod store;
#[cfg(test)]
mod tests;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{Instance, Storage};
use store::Store;

pub use bridge::run_bridge;
pub(crate) use store::initialize;

const VERSION: u32 = 1;
const MODE: &str = "control-only";
const MAX_BYTES: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(3);
pub const FRESHNESS: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    instance_id: String,
    profile: String,
    native_id: String,
    launch_id: String,
}

impl Binding {
    fn for_instance(instance: &Instance) -> Result<Self> {
        ensure!(
            instance.tool == "pi"
                && instance.command.is_empty()
                && !instance.is_structured()
                && !instance.is_sandboxed(),
            "unsupported: AFK requires an integrated direct host Pi launch"
        );
        instance.ensure_startable()?;
        ensure!(
            !matches!(
                instance.status,
                super::Status::Stopped | super::Status::Error
            ),
            "unsupported: session is not running"
        );
        let active = instance
            .active_execution
            .as_ref()
            .context("unsupported: no native launch binding")?;
        ensure!(active.binding.agent == "pi", "unsupported native execution");
        let native_id = instance
            .agent_session_id
            .clone()
            .context("unsupported: native conversation unknown")?;
        Ok(Self {
            instance_id: instance.id.clone(),
            profile: instance.effective_profile(),
            native_id,
            launch_id: active.launch_id.clone(),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bootstrap {
    pub(crate) version: u32,
    pub(crate) app_dir: PathBuf,
    pub(crate) aoe_bin: PathBuf,
    pub(crate) binding: Binding,
}

pub(crate) fn launch_arguments(
    instance: &Instance,
    native_id: &str,
    launch_id: &str,
) -> Result<String> {
    let app_dir = super::get_app_dir()?;
    let binding = Binding {
        instance_id: instance.id.clone(),
        profile: instance.effective_profile(),
        native_id: native_id.to_owned(),
        launch_id: launch_id.to_owned(),
    };
    Store::open(&app_dir, &instance.id, true)?;
    let asset = PathBuf::from("agent-extensions/aoe-afk.mjs");
    super::storage::replace_file_no_follow(
        &app_dir,
        &asset,
        include_bytes!("../../assets/session/aoe-afk.mjs"),
    )?;
    let bootstrap = Bootstrap {
        version: VERSION,
        app_dir: app_dir.clone(),
        aoe_bin: std::env::current_exe()?,
        binding,
    };
    Ok(format!(
        " -e {} --aoe-afk-binding {}",
        crate::session::environment::shell_escape(&app_dir.join(asset).to_string_lossy()),
        crate::session::environment::shell_escape(&serde_json::to_string(&bootstrap)?)
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u32,
    binding: Binding,
    revision: u64,
    window: String,
    generation: String,
    enabled: bool,
    expires_at_ms: Option<i64>,
    issued_at_ms: i64,
    duration_ms: u32,
    mode: String,
    allowance: u32,
}
impl Policy {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == VERSION && self.mode == MODE && self.allowance == 0,
            "unsupported AFK policy"
        );
        ensure!(
            self.revision > 0 && self.revision <= 9_007_199_254_740_991,
            "invalid policy revision"
        );
        ensure!(
            uuid::Uuid::parse_str(&self.window).is_ok(),
            "invalid window"
        );
        ensure!(
            !self.enabled
                || (self.expires_at_ms.is_some_and(|n| n > 0)
                    && uuid::Uuid::parse_str(&self.generation).is_ok()),
            "invalid enabled policy"
        );
        ensure!(
            self.enabled || (self.expires_at_ms.is_none() && self.duration_ms == 0),
            "off policy has an expiry"
        );
        ensure!(
            !self.enabled
                || ((60_000..=86_400_000).contains(&self.duration_ms)
                    && self.issued_at_ms.checked_add(i64::from(self.duration_ms))
                        == self.expires_at_ms),
            "invalid duration"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    version: u32,
    binding: Binding,
    challenge: String,
    revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    version: u32,
    binding: Binding,
    challenge: String,
    generation: String,
    revision: u64,
    window: Option<String>,
    expires_at_ms: Option<i64>,
    mode: String,
    allowance: u32,
    state: State,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Off,
    ControlOnly,
    Expired,
    Invalidated,
    Unsupported,
    Requested,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub state: State,
    pub requested_on: bool,
    pub revision: u64,
    pub expires_at_ms: Option<i64>,
    pub observed_at_ms: Option<i64>,
    pub detail: String,
}
impl Report {
    pub fn describe(&self) -> String {
        format!(
            "{:?}\nRequested: {}\nRevision: {}\nExpiry (Unix ms): {}\n{}\nAutonomous allowance: 0",
            self.state,
            if self.requested_on {
                "on (control-only)"
            } else {
                "off"
            },
            self.revision,
            self.expires_at_ms
                .map_or_else(|| "none".into(), |n| n.to_string()),
            self.detail
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Status,
    On { minutes: u32 },
    Off,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn report(
    policy: Option<&Policy>,
    state: State,
    observed: bool,
    detail: impl Into<String>,
) -> Report {
    Report {
        state,
        requested_on: policy.is_some_and(|p| p.enabled),
        revision: policy.map_or(0, |p| p.revision),
        expires_at_ms: policy.and_then(|p| p.expires_at_ms),
        observed_at_ms: observed.then(now_ms),
        detail: detail.into(),
    }
}

pub fn control(instance: &Instance, operation: Operation) -> Result<Report> {
    if let Operation::On { minutes } = operation {
        ensure!(
            (1..=1440).contains(&minutes),
            "select an expiry from 1 to 1440 minutes"
        );
    }
    let app_dir = super::get_app_dir()?;
    let store = Store::open(&app_dir, &instance.id, true)?;
    let _lock = store.lock(TIMEOUT)?;
    let mut policy = store
        .read::<Policy>("policy.json")
        .context("AFK policy needs explicit repair; refusing to invent a revision")?;
    if let Some(p) = &policy {
        p.validate()
            .context("AFK policy needs explicit repair; refusing to invent a revision")?;
        ensure!(
            matches!(operation, Operation::Off)
                || (p.binding.instance_id == instance.id
                    && p.binding.profile == instance.effective_profile()),
            "policy belongs to a different session/profile; request Off before re-enabling"
        );
    }
    let binding = Binding::for_instance(instance).and_then(|binding| {
        current_instance(&binding)?;
        Ok(binding)
    });
    if matches!(operation, Operation::Off) {
        let binding = binding.as_ref().cloned().unwrap_or_else(|_| Binding {
            instance_id: instance.id.clone(),
            profile: instance.effective_profile(),
            native_id: String::new(),
            launch_id: String::new(),
        });
        let revision = next_revision(policy.as_ref())?;
        policy = Some(Policy {
            version: VERSION,
            binding,
            revision,
            window: uuid::Uuid::new_v4().to_string(),
            generation: String::new(),
            enabled: false,
            expires_at_ms: None,
            issued_at_ms: now_ms(),
            duration_ms: 0,
            mode: MODE.into(),
            allowance: 0,
        });
        store.write("policy.json", policy.as_ref().unwrap())?;
    }
    let binding = match binding {
        Ok(binding) => binding,
        Err(error) => {
            return Ok(report(
                policy.as_ref(),
                if matches!(operation, Operation::Off) {
                    State::Requested
                } else {
                    State::Unsupported
                },
                false,
                format!("{error}; no acknowledgement"),
            ))
        }
    };
    let first = match exchange(&store, &binding, policy.as_ref()) {
        Ok(ack) => ack,
        Err(error) if matches!(operation, Operation::Off) => {
            return Ok(report(
                policy.as_ref(),
                State::Requested,
                false,
                format!("Off recorded; acknowledgement unavailable: {error}"),
            ))
        }
        Err(error) => return Err(error),
    };
    if let Operation::On { minutes } = operation {
        let Some(ack) = first else {
            return Ok(report(
                policy.as_ref(),
                State::Unavailable,
                false,
                "No fresh compatible acknowledgement; activation refused",
            ));
        };
        if ack.state == State::Unsupported {
            return Ok(report(
                policy.as_ref(),
                State::Unsupported,
                true,
                "Integration refused activation",
            ));
        }
        current_instance(&binding)?;
        let revision = next_revision(policy.as_ref())?;
        let issued_at_ms = now_ms();
        policy = Some(Policy {
            version: VERSION,
            binding: binding.clone(),
            revision,
            window: uuid::Uuid::new_v4().to_string(),
            generation: ack.generation,
            enabled: true,
            expires_at_ms: Some(issued_at_ms + i64::from(minutes) * 60_000),
            issued_at_ms,
            duration_ms: minutes * 60_000,
            mode: MODE.into(),
            allowance: 0,
        });
        store.write("policy.json", policy.as_ref().unwrap())?;
        return exchange_report(&store, &binding, policy.as_ref());
    }
    Ok(match first {
        Some(ack) => report(
            policy.as_ref(),
            ack.state,
            true,
            "Fresh integration acknowledgement; control-only, no delegated authority",
        ),
        None => report(
            policy.as_ref(),
            State::Unavailable,
            false,
            "No fresh acknowledgement; requested state is not confirmed",
        ),
    })
}

fn next_revision(policy: Option<&Policy>) -> Result<u64> {
    let revision = policy
        .map_or(0, |p| p.revision)
        .checked_add(1)
        .context("revision overflow")?;
    ensure!(revision <= 9_007_199_254_740_991, "revision overflow");
    Ok(revision)
}

fn exchange_report(store: &Store, binding: &Binding, policy: Option<&Policy>) -> Result<Report> {
    Ok(match exchange(store, binding, policy)? {
        Some(ack) => report(
            policy,
            ack.state,
            true,
            "Fresh integration acknowledgement; no delegated authority",
        ),
        None => report(
            policy,
            State::Requested,
            false,
            "Request recorded but not acknowledged; inspect again",
        ),
    })
}

fn valid_ack(ack: &Ack, probe: &Probe, policy: Option<&Policy>, now: i64) -> bool {
    if ack.version != VERSION
        || ack.mode != MODE
        || ack.allowance != 0
        || ack.binding != probe.binding
        || ack.challenge != probe.challenge
        || ack.revision != probe.revision
        || uuid::Uuid::parse_str(&ack.generation).is_err()
        || ack.window.as_deref() != policy.map(|p| p.window.as_str())
        || ack.expires_at_ms != policy.and_then(|p| p.expires_at_ms)
    {
        return false;
    }
    match ack.state {
        State::ControlOnly => policy.is_some_and(|p| {
            p.enabled
                && p.binding == probe.binding
                && p.generation == ack.generation
                && p.expires_at_ms.is_some_and(|n| n > now)
        }),
        State::Off => policy.is_none_or(|p| !p.enabled),
        State::Expired => policy.is_some_and(|p| p.enabled && p.generation == ack.generation),
        State::Invalidated | State::Unsupported => true,
        _ => false,
    }
}

fn exchange(store: &Store, binding: &Binding, policy: Option<&Policy>) -> Result<Option<Ack>> {
    let probe = Probe {
        version: VERSION,
        binding: binding.clone(),
        challenge: uuid::Uuid::new_v4().to_string(),
        revision: policy.map_or(0, |p| p.revision),
    };
    store.write("request.json", &probe)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(ack) = store.read::<Ack>("ack.json")? {
            if valid_ack(&ack, &probe, policy, now_ms()) {
                return Ok(Some(ack));
            }
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn current_instance(binding: &Binding) -> Result<Instance> {
    let storage = Storage::open_unwatched(&binding.profile)?;
    let (mut rows, _) = storage.load_with_groups()?;
    let mut instance = rows
        .drain(..)
        .find(|i| i.id == binding.instance_id)
        .context("session no longer exists")?;
    instance.source_profile.clone_from(&binding.profile);
    ensure!(
        Binding::for_instance(&instance)? == *binding,
        "native launch/conversation changed"
    );
    Ok(instance)
}
