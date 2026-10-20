//! Optional, one-shot restart automation. Never a human-input or AFK authority path.
use super::*;
use serde_json::{json, Value};
use std::path::Path;
use usage::transport::Clock;

const ROOT: &str = "automatic-wake";
const CAPABILITY: &str = "custom-message-v1";
const TEXT_BYTES: usize = 4096;
const LIFETIME_MS: u64 = 30_000;
const HELLO_MS: u64 = 5_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    version: u32,
}

pub(crate) fn initialize(app: &Path) -> Result<()> {
    let parent = crate::session::AnchoredDir::open(app)?.child(Path::new(authority::NAMESPACE))?;
    store::private_dir(&parent)?;
    let dir = parent.create_child(Path::new(ROOT))?;
    store::private_dir(&dir)?;
    let store = Store {
        dir,
        scope: None,
        limit: MAX_BYTES,
    };
    let _lock = store.lock(TIMEOUT)?;
    if let Some(p) = store.read::<Prepared>("version.json")? {
        ensure!(p.version == 1, "unsupported automatic wake preparation");
    } else {
        store.write("version.json", &Prepared { version: 1 })?;
    }
    store.dir.sync()?;
    parent.sync()?;
    Ok(())
}

fn slot(app: &Path, binding: &Binding, create: bool) -> Result<Option<Store>> {
    authority::scope(&binding.profile, Some(&binding.instance_id))?;
    authority::ready(app)?;
    let root = crate::session::AnchoredDir::open(app)?
        .child(Path::new(authority::NAMESPACE))?
        .child(Path::new(ROOT))?;
    store::private_dir(&root)?;
    let prepared = Store {
        dir: root,
        scope: None,
        limit: MAX_BYTES,
    };
    ensure!(
        prepared
            .read::<Prepared>("version.json")?
            .is_some_and(|p| p.version == 1),
        "automatic wake not prepared by migration"
    );
    let mut dir = prepared.dir;
    for component in [&binding.profile, &binding.instance_id] {
        if !create && dir.entry_stat(Path::new(component))?.is_none() {
            return Ok(None);
        }
        dir = if create {
            dir.create_child(Path::new(component))?
        } else {
            dir.child(Path::new(component))?
        };
        store::private_dir(&dir)?;
    }
    Ok(Some(Store {
        dir,
        scope: None,
        limit: MAX_BYTES,
    }))
}
fn id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}
fn binding_valid(binding: &Binding) -> Result<()> {
    authority::scope(&binding.profile, Some(&binding.instance_id))?;
    ensure!(
        id(&binding.native_id) && id(&binding.launch_id),
        "automatic wake native identity unavailable"
    );
    Ok(())
}
fn age(now: &Clock, then: &Clock) -> Option<u64> {
    if now.boot != then.boot || !id(&now.boot) {
        return None;
    }
    now.millis.checked_sub(then.millis)?.checked_add(1)
}
fn clear(session: &authority::Session, binding: &Binding) -> bool {
    !session.policy.as_ref().is_some_and(|p| p.enabled)
        && !session.book.has_native_history(&binding.native_id)
}
fn text_valid(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= TEXT_BYTES
        && !text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Capability {
    version: u32,
    delivery: String,
    ready: bool,
}
impl Capability {
    fn supported(&self) -> bool {
        self.version == 1 && self.delivery == CAPABILITY
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receiver {
    version: u32,
    delivery: String,
    binding: Binding,
    generation: String,
    incarnation: String,
    clock: Clock,
    ready: bool,
}
impl Receiver {
    fn validate(&self) -> Result<()> {
        binding_valid(&self.binding)?;
        ensure!(
            self.version == 1
                && self.delivery == CAPABILITY
                && id(&self.generation)
                && id(&self.incarnation)
                && id(&self.clock.boot),
            "invalid automatic wake receiver"
        );
        Ok(())
    }
    fn matches(&self, binding: &Binding, now: &Clock) -> bool {
        self.binding == *binding && age(now, &self.clock).is_some_and(|a| a < HELLO_MS)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Pending,
    Attempted,
    Suppressed,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    id: String,
    binding: Binding,
    generation: String,
    incarnation: String,
    issued: Clock,
    eligible: bool,
    state: State,
    reason: Option<String>,
    text: String,
}
impl Request {
    fn validate(&self) -> Result<()> {
        binding_valid(&self.binding)?;
        ensure!(
            self.version == 1
                && id(&self.id)
                && id(&self.generation)
                && id(&self.incarnation)
                && id(&self.issued.boot)
                && text_valid(&self.text),
            "invalid automatic wake request"
        );
        ensure!(
            match self.state {
                State::Pending => self.reason.is_none(),
                State::Attempted => self.reason.as_deref() == Some("delivery_unknown"),
                State::Suppressed =>
                    matches!(self.reason.as_deref(), Some("expired" | "not_clear")),
            },
            "invalid automatic wake disposition"
        );
        Ok(())
    }
}

/// Locks are acquired in lifecycle/input -> authority -> wake order; waiting is outside them.
pub(crate) fn post(instance: &Instance, text: &str) -> Result<()> {
    ensure!(
        text_valid(text),
        "automatic wake text must be 1..4096 UTF8 bytes without unsafe controls"
    );
    ensure!(
        instance.tool == "pi"
            && instance.command.is_empty()
            && !instance.is_structured()
            && !instance.is_sandboxed(),
        "unsupported automatic Pi wake route"
    );
    let app = crate::session::get_app_dir_path()?;
    let mut recipient = instance.clone();
    let expected_launch = instance
        .active_execution
        .as_ref()
        .context("restart launch unavailable")?
        .launch_id
        .clone();
    let expected_profile = instance.effective_profile();
    let file_watch = crate::file_watch::FileWatchService::noop();
    let identity_deadline = Instant::now() + crate::session::sync::CLI_SESSION_ID_CAPTURE_TIMEOUT;
    let binding = loop {
        crate::session::sync::drain_and_persist_session_ids(
            std::slice::from_mut(&mut recipient),
            &file_watch,
        );
        ensure!(
            recipient
                .active_execution
                .as_ref()
                .is_some_and(|e| e.launch_id == expected_launch)
                && recipient.effective_profile() == expected_profile,
            "restart wake sender changed during identity capture"
        );
        if let Ok(binding) = Binding::for_instance(&recipient) {
            if current_instance(&binding).is_ok() {
                break binding;
            }
        }
        ensure!(
            Instant::now() < identity_deadline,
            "final restart identity not published; automatic wake suppressed"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    binding_valid(&binding)?;
    let deadline = Instant::now() + FRESHNESS;
    let observed = loop {
        let candidate = (|| -> Result<Option<Receiver>> {
            current_instance(&binding)?;
            let Some(store) = slot(&app, &binding, false)? else {
                return Ok(None);
            };
            let Some(receiver) = store.read::<Receiver>("receiver.json")? else {
                return Ok(None);
            };
            receiver.validate()?;
            let now = Clock::now().context("continuous wake clock unavailable")?;
            Ok(receiver.matches(&binding, &now).then_some(receiver))
        })();
        if let Ok(Some(receiver)) = candidate {
            break receiver;
        }
        ensure!(
            Instant::now() < deadline,
            "no current qualified automatic wake receiver; no terminal-input fallback"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    let _input = instance.lock_for_input()?;
    current_instance(&binding)?;
    let runtime = Store::authority(&app, &binding.profile, &binding.instance_id, false)?;
    let _authority = runtime.lock_existing(TIMEOUT)?;
    let session = runtime.session()?;
    let store = slot(&app, &binding, false)?.context("wake receiver disappeared")?;
    let _wake = store.lock_existing(TIMEOUT)?;
    let receiver = store
        .read::<Receiver>("receiver.json")?
        .context("wake receiver disappeared")?;
    receiver.validate()?;
    let now = Clock::now().context("continuous wake clock unavailable")?;
    ensure!(
        receiver.matches(&binding, &now)
            && receiver.incarnation == observed.incarnation
            && receiver.generation == observed.generation,
        "wake receiver changed before publication"
    );
    publish_request(
        &store,
        &binding,
        &observed,
        text,
        clear(&session, &binding) && receiver.ready,
        now,
        || current_instance(&binding).map(|_| ()),
    )
}
fn publish_request(
    store: &Store,
    binding: &Binding,
    receiver: &Receiver,
    text: &str,
    host_clear: bool,
    now: Clock,
    validate: impl Fn() -> Result<()>,
) -> Result<()> {
    if let Some(old) = store.read::<Request>("request.json")? {
        old.validate()?;
        ensure!(
            old.binding.profile == binding.profile
                && old.binding.instance_id == binding.instance_id,
            "foreign automatic wake slot"
        );
        ensure!(
            old.binding.launch_id != binding.launch_id,
            "automatic wake already posted for this launch; no retry"
        );
    }
    validate()?;
    let request = Request {
        version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        binding: binding.clone(),
        generation: receiver.generation.clone(),
        incarnation: receiver.incarnation.clone(),
        issued: now,
        eligible: receiver.ready && host_clear,
        state: State::Pending,
        reason: None,
        text: text.into(),
    };
    request.validate()?;
    store.write("request.json", &request)
}

pub(super) struct Peer<'a> {
    pub app: &'a Path,
    pub runtime: &'a Store,
    pub binding: &'a Binding,
    pub generation: &'a str,
}
pub(super) struct Server {
    incarnation: String,
    parked: bool,
}
impl Server {
    pub fn park(&mut self) {
        self.parked = true;
    }
    pub fn new() -> Self {
        Self {
            incarnation: uuid::Uuid::new_v4().to_string(),
            parked: false,
        }
    }
    pub fn observe(
        &mut self,
        peer: Peer<'_>,
        capability: Option<Capability>,
        validate: impl Fn() -> Result<()>,
    ) -> Value {
        self.observe_with(peer, capability, Clock::now, validate)
    }
    fn observe_with(
        &mut self,
        peer: Peer<'_>,
        capability: Option<Capability>,
        clock: impl Fn() -> Option<Clock>,
        validate: impl Fn() -> Result<()>,
    ) -> Value {
        let Some(capability) = capability.filter(Capability::supported) else {
            return Value::Null;
        };
        if self.parked {
            return json!({"version":1,"state":"unavailable"});
        }
        match self.decide(peer, capability, clock, validate) {
            Ok(value) => value,
            Err(error) => {
                self.parked = true;
                tracing::warn!(target:"afk.wake", %error, "automatic wake unavailable; this receiver will not retry");
                json!({"version":1,"state":"unavailable"})
            }
        }
    }
    fn decide(
        &mut self,
        peer: Peer<'_>,
        capability: Capability,
        clock: impl Fn() -> Option<Clock>,
        validate: impl Fn() -> Result<()>,
    ) -> Result<Value> {
        binding_valid(peer.binding)?;
        ensure!(id(peer.generation), "invalid wake receiver generation");
        let _authority = peer.runtime.lock_existing(TIMEOUT)?;
        validate()?;
        let session = peer.runtime.session()?;
        let store = slot(peer.app, peer.binding, true)?.context("wake slot unavailable")?;
        let _wake = store.lock(TIMEOUT)?;
        validate()?;
        let now = clock().context("continuous wake clock unavailable")?;
        let receiver = Receiver {
            version: 1,
            delivery: CAPABILITY.into(),
            binding: peer.binding.clone(),
            generation: peer.generation.into(),
            incarnation: self.incarnation.clone(),
            clock: now.clone(),
            ready: capability.ready,
        };
        store.write("receiver.json", &receiver)?;
        let Some(mut request) = store.read::<Request>("request.json")? else {
            return Ok(json!({"version":1,"state":"empty"}));
        };
        request.validate()?;
        if request.binding != *peer.binding
            || request.generation != peer.generation
            || request.incarnation != self.incarnation
        {
            return Ok(json!({"version":1,"state":"unmatched"}));
        }
        if request.state != State::Pending {
            return Ok(
                json!({"version":1,"state":request.state,"id":request.id,"reason":request.reason}),
            );
        }
        let now = clock().context("continuous wake clock unavailable")?;
        let remaining = age(&now, &request.issued)
            .and_then(|a| LIFETIME_MS.checked_sub(a))
            .filter(|left| *left > 0);
        let permitted = request.eligible && capability.ready && clear(&session, peer.binding);
        request.state = if remaining.is_some() && permitted {
            State::Attempted
        } else {
            State::Suppressed
        };
        request.reason = Some(
            if remaining.is_none() {
                "expired"
            } else if permitted {
                "delivery_unknown"
            } else {
                "not_clear"
            }
            .into(),
        );
        validate()?;
        store.write("request.json", &request)?;
        if request.state == State::Attempted {
            Ok(
                json!({"version":1,"state":"attempted","id":request.id,"binding":request.binding,"generation":request.generation,"incarnation":request.incarnation,"remaining_ms":remaining,"text":request.text}),
            )
        } else {
            Ok(json!({"version":1,"state":"suppressed","id":request.id,"reason":request.reason}))
        }
    }
}

#[cfg(test)]
mod tests;
