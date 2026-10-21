use super::*;
use crate::session::Instance;
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArcIdentity {
    pub session_id: String,
    pub pi_id: String,
    pub launch_id: String,
    pub generation: u64,
    pub leaf: String,
}

impl ArcIdentity {
    pub(crate) fn matches(&self, session: &Instance) -> bool {
        session.work_arc_enabled
            && self.session_id == session.id
            && session.agent_session_id.as_deref() == Some(&self.pi_id)
            && session.lifecycle_generation == self.generation
            && session
                .active_execution
                .as_ref()
                .is_some_and(|a| a.launch_id == self.launch_id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredArc {
    pub identity: ArcIdentity,
    pub at: DateTime<Utc>,
    pub arc: WorkArc,
    pub selected_route: String,
    pub provider: String,
    pub model: String,
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ArcCost {
    #[default]
    NoRequest,
    Unknown,
    Measured,
}

impl ArcCost {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NoRequest => "no request",
            Self::Unknown => "unknown usage",
            Self::Measured => "measured usage",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArcState {
    pub record: Option<StoredArc>,
    attempts: Vec<DateTime<Utc>>,
    pending: Option<String>,
    pub failed: bool,
    pub unavailable: bool,
    pub cleared: bool,
    pub epoch: u64,
    pub cost: ArcCost,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_usage: Option<Value>,
}

impl ArcState {
    pub(crate) fn clear(&mut self) {
        self.record = None;
        self.pending = None;
        self.failed = false;
        self.unavailable = false;
        self.cleared = true;
        self.last_attempt_usage = None;
        self.cost = if self.attempts.is_empty() {
            ArcCost::NoRequest
        } else {
            ArcCost::Unknown
        };
        self.epoch = self.epoch.saturating_add(1);
    }

    pub(crate) fn reserve(&mut self, identity: &ArcIdentity, now: DateTime<Utc>) -> Option<String> {
        self.attempts.retain(|at| *at > now - Duration::hours(24));
        if self.attempts.len() >= 20
            || self
                .attempts
                .last()
                .is_some_and(|at| now - *at < Duration::minutes(5))
            || self
                .record
                .as_ref()
                .is_some_and(|record| &record.identity == identity)
        {
            return None;
        }
        let nonce = uuid::Uuid::new_v4().to_string();
        self.attempts.push(now);
        self.pending = Some(nonce.clone());
        self.cost = ArcCost::Unknown;
        self.last_attempt_usage = None;
        self.cleared = false;
        Some(nonce)
    }

    pub(crate) fn finish(
        &mut self,
        nonce: &str,
        record: Option<StoredArc>,
        usage: Option<Value>,
    ) -> bool {
        if self.pending.as_deref() != Some(nonce) {
            return false;
        }
        self.pending = None;
        self.unavailable = false;
        self.failed = record.is_none();
        self.last_attempt_usage = usage
            .or_else(|| record.as_ref().and_then(|record| record.usage.clone()))
            .as_ref()
            .and_then(measured_usage);
        self.cost = if self.last_attempt_usage.is_some() {
            ArcCost::Measured
        } else {
            ArcCost::Unknown
        };
        if let Some(record) = record {
            self.record = Some(record);
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArcStatus {
    Off,
    Missing,
    Loading,
    Current,
    Stale,
    Failed,
    Cleared,
    Unavailable,
}

impl ArcStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Missing => "missing / no useful input",
            Self::Loading => "loading",
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Failed => "failed",
            Self::Cleared => "cleared",
            Self::Unavailable => "unavailable",
        }
    }
}

pub(crate) fn supported(session: &Instance) -> bool {
    session.tool == "pi" && !session.is_structured() && !session.is_sandboxed()
}

pub(crate) fn visible_record(session: &Instance) -> Option<&StoredArc> {
    session.work_arc.record.as_ref().filter(|r| {
        session.work_arc_enabled
            && r.identity.session_id == session.id
            && session.agent_session_id.as_deref() == Some(&r.identity.pi_id)
    })
}

pub(crate) fn projection(session: &Instance) -> ArcStatus {
    if !session.work_arc_enabled {
        return if session.work_arc.cleared {
            ArcStatus::Cleared
        } else {
            ArcStatus::Off
        };
    }
    if session.work_arc_status == Some(ArcStatus::Current)
        && !visible_record(session).is_some_and(|r| r.identity.matches(session))
    {
        return ArcStatus::Stale;
    }
    session.work_arc_status.unwrap_or_else(|| {
        if session.work_arc.unavailable {
            ArcStatus::Unavailable
        } else if session.work_arc.failed {
            ArcStatus::Failed
        } else if session.work_arc.record.is_some() {
            ArcStatus::Stale
        } else {
            ArcStatus::Missing
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ArcIdentity {
        ArcIdentity {
            session_id: "aoe".into(),
            pi_id: "pi".into(),
            launch_id: "launch".into(),
            generation: 2,
            leaf: "leaf".into(),
        }
    }

    #[test]
    fn durable_budget_survives_restart_disable_and_conversation_clear() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut state = ArcState::default();
        for n in 0..20 {
            let at = now + Duration::minutes(5 * n);
            assert!(state.reserve(&identity(), at).is_some());
            assert!(state
                .reserve(&identity(), at + Duration::seconds(299))
                .is_none());
            state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        }
        state.clear();
        assert!(state.record.is_none());
        assert!(state
            .reserve(&identity(), now + Duration::hours(23))
            .is_none());
        assert!(state
            .reserve(&identity(), now + Duration::hours(24))
            .is_some());
    }

    #[test]
    fn clear_and_newer_reservations_reject_late_results_without_raw_persistence() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut state = ArcState::default();
        let nonce = state.reserve(&identity(), now).unwrap();
        let record = StoredArc {
            identity: identity(), at: now,
            arc: WorkArc::from_json(r#"{"goal":"Goal","now":"Reported progress","remaining":"Unknown","next":"Review"}"#).unwrap(),
            selected_route: "route/auto".into(), provider: "fake".into(), model: "test".into(), usage: None,
        };
        assert!(state.finish(&nonce, Some(record.clone()), None));
        assert_eq!(state.record.as_ref().unwrap().identity.leaf, "leaf");
        assert!(!serde_json::to_string(&state).unwrap().contains("input"));
        state.clear();
        assert!(!state.finish(&nonce, Some(record.clone()), None));
        let later = state
            .reserve(&identity(), now + Duration::minutes(5))
            .unwrap();
        assert!(!state.finish(&nonce, Some(record), None));
        assert!(state.finish(&later, None, None));
        assert!(state.failed);
        assert!(state.record.is_none());
    }

    #[test]
    fn session_json_preserves_arc_state_and_defaults_legacy_rows() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut session = Instance::new("synthetic", "/tmp/synthetic");
        session.tool = "pi".into();
        session.work_arc_enabled = true;
        let mut state = ArcState::default();
        let nonce = state.reserve(&identity(), now).unwrap();
        let record = StoredArc {
            identity: identity(),
            at: now,
            arc: WorkArc::from_json(
                r#"{"goal":"Goal","now":"Reported progress","remaining":"Unknown","next":"Review"}"#,
            )
            .unwrap(),
            selected_route: "route/auto".into(),
            provider: "fake".into(),
            model: "test".into(),
            usage: None,
        };
        assert!(state.finish(&nonce, Some(record), None));
        let expected = serde_json::to_value(&state).unwrap();
        session.work_arc = serde_json::from_value(expected.clone()).unwrap();
        session.work_arc_status = Some(ArcStatus::Current);
        let mut row = serde_json::to_value(&session).unwrap();
        assert_eq!(row["work_arc"], expected);
        assert!(row["work_arc"]["record"].is_object());
        assert!(row.get("work_arc_status").is_none());
        let restored: Instance = serde_json::from_value(row.clone()).unwrap();
        assert!(restored.work_arc_enabled);
        assert_eq!(serde_json::to_value(&restored.work_arc).unwrap(), expected);
        assert!(restored.work_arc_status.is_none());

        row.as_object_mut().unwrap().remove("work_arc");
        row.as_object_mut().unwrap().remove("work_arc_enabled");
        let legacy: Instance = serde_json::from_value(row).unwrap();
        assert!(!legacy.work_arc_enabled);
        assert_eq!(
            serde_json::to_value(&legacy.work_arc).unwrap(),
            serde_json::to_value(ArcState::default()).unwrap()
        );
        assert_eq!(projection(&legacy), ArcStatus::Off);
    }

    #[test]
    fn projection_never_restores_current_from_disk_or_another_session() {
        let mut session = Instance::new("synthetic", "/tmp/synthetic");
        session.tool = "pi".into();
        assert_eq!(projection(&session), ArcStatus::Off);
        session.work_arc_enabled = true;
        assert_eq!(projection(&session), ArcStatus::Missing);
        session.work_arc_status = Some(ArcStatus::Loading);
        assert_eq!(projection(&session), ArcStatus::Loading);
        let restored: Instance =
            serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        assert_eq!(projection(&restored), ArcStatus::Missing);
        session.work_arc.clear();
        session.work_arc_enabled = false;
        assert_eq!(projection(&session), ArcStatus::Cleared);
    }
}
