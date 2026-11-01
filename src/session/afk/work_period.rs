//! Native consent owns work authority. This transport can only inspect or withdraw it.
use super::*;

const REQUEST: &str = "work-period-request.json";
const RESPONSE: &str = "work-period-reply.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Inspect,
    Stop,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    version: u32,
    id: String,
    binding: Binding,
    kind: Kind,
    generation: Option<String>,
    period_id: Option<String>,
    issued_at_ms: i64,
    expires_at_ms: i64,
}
impl Request {
    fn valid(&self, binding: &Binding, generation: &str, now: i64) -> bool {
        self.version == 1
            && self.binding == *binding
            && uuid::Uuid::parse_str(&self.id).is_ok()
            && self
                .expires_at_ms
                .checked_sub(self.issued_at_ms)
                .is_some_and(|n| (1..=5000).contains(&n))
            && now >= self.issued_at_ms.saturating_sub(1000)
            && now < self.expires_at_ms
            && match self.kind {
                Kind::Inspect => self.generation.is_none() && self.period_id.is_none(),
                Kind::Stop => {
                    self.generation.as_deref() == Some(generation)
                        && uuid::Uuid::parse_str(generation).is_ok()
                        && self
                            .period_id
                            .as_ref()
                            .is_none_or(|id| uuid::Uuid::parse_str(id).is_ok())
                }
            }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Inspected,
    Stopped,
    Refused,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Period {
    id: String,
    consent: String,
    active: bool,
    guarded: bool,
    status: String,
    reason: Option<String>,
    expires_at_ms: i64,
    request_limit: u32,
    requests_used: u32,
    tool_limit: u32,
    tools_used: u32,
    summary: Option<String>,
    next_step: Option<String>,
    completion_basis: Option<String>,
    settlement_status: String,
    /// Absent from extensions that predate menu answering.
    #[serde(default)]
    policy: Option<String>,
    #[serde(default)]
    answers: Option<Answers>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answers {
    rules: u32,
    yolo: u32,
    stops: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
    version: u32,
    id: String,
    binding: Binding,
    generation: String,
    control_revision: u64,
    pub observed_at_ms: i64,
    sdk: String,
    outcome: Outcome,
    eligible: bool,
    detail: String,
    period: Option<Period>,
}
impl Inspection {
    fn valid(&self, request: &Request, generation: &str, now: i64) -> bool {
        let text = |s: &str, max: usize| {
            s.len() <= max
                && !s
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        };
        self.version == 1
            && self.id == request.id
            && self.binding == request.binding
            && self.generation == generation
            && uuid::Uuid::parse_str(generation).is_ok()
            && self.control_revision <= 9_007_199_254_740_991
            && self.observed_at_ms >= request.issued_at_ms.saturating_sub(1000)
            && self.observed_at_ms <= now.saturating_add(1000)
            && now < request.expires_at_ms
            && text(&self.sdk, 64)
            && text(&self.detail, 1024)
            && (request.kind != Kind::Inspect || self.outcome == Outcome::Inspected)
            && (request.kind != Kind::Stop || self.outcome != Outcome::Inspected)
            && self.period.as_ref().is_none_or(|p| {
                uuid::Uuid::parse_str(&p.id).is_ok()
                    && p.consent == "native-interactive-confirm-v1"
                    && matches!(
                        p.status.as_str(),
                        "pending"
                            | "owned"
                            | "recovered"
                            | "completed"
                            | "blocked"
                            | "expired"
                            | "exhausted"
                            | "interrupted"
                    )
                    && (!p.active || p.guarded && matches!(p.status.as_str(), "pending" | "owned"))
                    && (p.status != "recovered" || !p.active && !p.guarded)
                    && (1..=512).contains(&p.request_limit)
                    && p.requests_used <= p.request_limit
                    && p.tool_limit == (p.request_limit * 16).min(4096)
                    && p.tools_used <= p.tool_limit
                    && p.expires_at_ms > 0
                    && p.reason.as_ref().is_none_or(|s| text(s, 128))
                    && p.summary.as_ref().is_none_or(|s| text(s, 2048))
                    && p.next_step.as_ref().is_none_or(|s| text(s, 1024))
                    && p.completion_basis
                        .as_deref()
                        .is_none_or(|s| s == "model_claim; inspect artifacts and validation")
                    && matches!(
                        p.settlement_status.as_str(),
                        "proved" | "pending" | "not_proved"
                    )
                    && p.policy
                        .as_deref()
                        .is_none_or(|s| matches!(s, "rules" | "yolo"))
            })
    }

    pub fn describe(&self) -> String {
        let Some(p) = &self.period else {
            return format!("Native AFK work period: none\n{}\nStart only in the attached Pi Session: /afk-work on\nAoE transport cannot grant work.", self.detail);
        };
        format!("Native AFK work period: {} ({})\nMain-loop requests: {}/{}; local tool admissions: {}/{}\nExpiry (Unix ms): {}; settlement: {}\nReason: {}\n{}\nNext: {}\nCompletion is a model claim, not independent acceptance. Descendant requests, retries and billing are not covered.\n{}",
            p.status, if p.active { "active native consent" } else { "no active authority; retained history" },
            p.requests_used, p.request_limit, p.tools_used, p.tool_limit, p.expires_at_ms, p.settlement_status,
            p.reason.as_deref().unwrap_or("current task"), p.summary.as_deref().unwrap_or("No checkpoint yet"),
            p.next_step.as_deref().unwrap_or("none recorded"), self.detail)
            + &match (&p.policy, &p.answers) {
                (Some(policy), Some(a)) => format!(
                    "\nAnswer policy: {policy}; answers: {} (rules {}, yolo {}), stops: {}",
                    a.rules.saturating_add(a.yolo),
                    a.rules,
                    a.yolo,
                    a.stops
                ),
                (Some(policy), None) => format!("\nAnswer policy: {policy}"),
                _ => String::new(),
            }
    }
}

pub(super) fn request(
    store: &Store,
    binding: &Binding,
    generation: &str,
    now: i64,
) -> Result<Option<Request>> {
    Ok(store
        .read::<Request>(REQUEST)?
        .filter(|r| r.valid(binding, generation, now)))
}

pub(super) fn acknowledge(
    store: &Store,
    binding: &Binding,
    generation: &str,
    reply: Inspection,
    now: i64,
) -> Result<()> {
    let request = request(store, binding, generation, now)?
        .context("no current work-period inspection/stop request")?;
    ensure!(
        reply.valid(&request, generation, now),
        "unmatched or invalid native work-period reply"
    );
    store.write(RESPONSE, &reply)
}

pub fn inspect(instance: &Instance) -> Result<Inspection> {
    exchange(instance, None)
}

pub fn stop(instance: &Instance, observed: &Inspection) -> Result<Inspection> {
    ensure!(
        now_ms().saturating_sub(observed.observed_at_ms) <= FRESHNESS.as_millis() as i64,
        "native observation is stale; inspect again"
    );
    exchange(instance, Some(observed))
}

fn exchange(instance: &Instance, stop: Option<&Inspection>) -> Result<Inspection> {
    let binding = Binding::for_instance(instance)?;
    current_instance(&binding)?;
    if let Some(observed) = stop {
        ensure!(
            observed.binding == binding,
            "native launch changed; no replacement target selected"
        );
    }
    let app = super::super::get_app_dir()?;
    authority::ready(&app)?;
    let store = Store::existing_transport(&app, &binding.profile, &binding.instance_id)?
        .context("no native work-period transport; launch a qualified integrated Session")?;
    let _lock = store.lock(TIMEOUT)?;
    let issued = now_ms();
    let request = Request {
        version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        binding: binding.clone(),
        kind: if stop.is_some() {
            Kind::Stop
        } else {
            Kind::Inspect
        },
        generation: stop.map(|s| s.generation.clone()),
        period_id: stop.and_then(|s| s.period.as_ref().map(|p| p.id.clone())),
        issued_at_ms: issued,
        expires_at_ms: issued.checked_add(3000).context("clock overflow")?,
    };
    current_instance(&binding)?;
    store
        .write(REQUEST, &request)
        .context("work-period transport publication unverified; inspect before another command")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        current_instance(&binding)?;
        if let Some(reply) = store.read::<Inspection>(RESPONSE)? {
            if reply.valid(&request, &reply.generation, now_ms())
                && request
                    .generation
                    .as_ref()
                    .is_none_or(|g| *g == reply.generation)
            {
                return Ok(reply);
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("no fresh compatible native reply; state is unconfirmed, no retry or replacement selected");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_has_no_grant_shape_and_replies_do_not_restore_authority() {
        let binding = Binding {
            instance_id: "test".into(),
            profile: "default".into(),
            native_id: uuid::Uuid::new_v4().to_string(),
            launch_id: uuid::Uuid::new_v4().to_string(),
        };
        let generation = uuid::Uuid::new_v4().to_string();
        let request = Request {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding: binding.clone(),
            kind: Kind::Inspect,
            generation: None,
            period_id: None,
            issued_at_ms: 1000,
            expires_at_ms: 4000,
        };
        let reply = Inspection {
            version: 1,
            id: request.id.clone(),
            binding: binding.clone(),
            generation: generation.clone(),
            control_revision: 0,
            observed_at_ms: 1100,
            sdk: "1.2.0".into(),
            outcome: Outcome::Inspected,
            eligible: false,
            detail: "Fresh ordinary recovery required".into(),
            period: Some(Period {
                id: uuid::Uuid::new_v4().to_string(),
                consent: "native-interactive-confirm-v1".into(),
                active: false,
                guarded: true,
                status: "owned".into(),
                reason: None,
                expires_at_ms: 61000,
                request_limit: 4,
                requests_used: 2,
                tool_limit: 64,
                tools_used: 3,
                summary: None,
                next_step: None,
                completion_basis: None,
                settlement_status: "not_proved".into(),
                policy: None,
                answers: None,
            }),
        };
        assert!(reply.valid(&request, &generation, 1200));
        assert!(reply.describe().contains("no active authority"));
        for mutation in ["start", "expired", "generation", "binding", "counter"] {
            let mut req = serde_json::to_value(&request).unwrap();
            let mut value = reply.clone();
            match mutation {
                "start" => {
                    req["kind"] = serde_json::json!("start");
                    assert!(serde_json::from_value::<Request>(req).is_err());
                }
                "expired" => assert!(!value.valid(&request, &generation, 4000)),
                "generation" => {
                    value.generation = uuid::Uuid::new_v4().to_string();
                    assert!(!value.valid(&request, &generation, 1200));
                }
                "binding" => {
                    value.binding.launch_id = uuid::Uuid::new_v4().to_string();
                    assert!(!value.valid(&request, &generation, 1200));
                }
                "counter" => {
                    value.period.as_mut().unwrap().requests_used = 5;
                    assert!(!value.valid(&request, &generation, 1200));
                }
                _ => unreachable!(),
            }
        }
        let mut answered = serde_json::to_value(&reply).unwrap();
        answered["period"]["policy"] = serde_json::json!("yolo");
        answered["period"]["answers"] = serde_json::json!({"rules": 0, "yolo": 2, "stops": 1});
        let parsed: Inspection = serde_json::from_value(answered.clone()).unwrap();
        assert!(parsed.valid(&request, &generation, 1200));
        assert!(parsed
            .describe()
            .contains("Answer policy: yolo; answers: 2 (rules 0, yolo 2), stops: 1"));
        let mut older = answered.clone();
        let period = older["period"].as_object_mut().unwrap();
        period.remove("policy");
        period.remove("answers");
        let older: Inspection = serde_json::from_value(older).unwrap();
        assert!(older.valid(&request, &generation, 1200));
        assert!(!older.describe().contains("Answer policy"));
        let mut bad = answered.clone();
        bad["period"]["policy"] = serde_json::json!("review");
        assert!(!serde_json::from_value::<Inspection>(bad).unwrap().valid(
            &request,
            &generation,
            1200
        ));
        let mut bad = answered;
        bad["period"]["answers"]["asks"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Inspection>(bad).is_err());
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        store.write(REQUEST, &request).unwrap();
        acknowledge(&store, &binding, &generation, reply, 1200).unwrap();
        assert!(request.valid(&binding, &generation, 1200));
        assert!(super::request(&store, &binding, &generation, 4000)
            .unwrap()
            .is_none());
        assert!(store.read::<Policy>("policy.json").unwrap().is_none());
        assert!(store
            .read::<delegation::Book>("ledger.json")
            .unwrap()
            .is_none());
    }
}
