use super::*;
use std::{fmt, path::Path};

#[derive(Debug)]
pub(in crate::session::afk) struct Denied(pub Denial);
impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "observed usage guard {:?}; no refund or automatic resumption",
            self.0
        )
    }
}
impl std::error::Error for Denied {}

impl State {
    pub fn current_source(&self, binding: &Binding) -> Option<(&String, &Source)> {
        let mut sources = self.sources.iter().filter(|(_, s)| {
            s.binding.profile == binding.profile
                && s.binding.instance_id == binding.instance_id
                && s.binding.native_id == binding.native_id
        });
        let one = sources.next()?;
        sources.next().is_none().then_some(one)
    }
    pub fn ingest(
        &mut self,
        binding: &Binding,
        generation: &str,
        observation: &Observation,
        sessions: &mut BTreeMap<String, authority::Session>,
    ) -> Result<()> {
        ensure!(
            observation.report.is_none() || observation.gap.is_none(),
            "ambiguous usage observation"
        );
        if let Some(window) = &observation.window {
            ensure!(
                uuid::Uuid::parse_str(window).is_ok(),
                "invalid usage window"
            );
        }
        if let Some(report) = &observation.report {
            let key = report.identity.key(binding)?;
            let existing = self.current_source(binding).map(|(k, _)| k.clone());
            if report.validate().is_err()
                || report.identity.validate(binding).is_err()
                || existing.as_ref().is_some_and(|k| k != &key)
            {
                if let Some(key) = existing {
                    self.sources
                        .get_mut(&key)
                        .unwrap()
                        .gap
                        .get_or_insert(Gap::SourceChanged);
                } else {
                    return Ok(());
                }
            } else {
                let source = self.sources.entry(key).or_insert_with(|| {
                    Source::new(binding.clone(), generation.into(), report.identity.clone())
                });
                if source.reconcile(binding, generation, report).is_err() {
                    source.gap.get_or_insert(Gap::SourceChanged);
                }
            }
        }
        let source_key = self.current_source(binding).map(|(key, _)| key.clone());
        let Some(key) = source_key else {
            return Ok(());
        };
        if let Some(gap @ (Gap::SourceChanged | Gap::HistoryLimit)) = observation.gap {
            self.sources.get_mut(&key).unwrap().gap.get_or_insert(gap);
        }
        let source = &self.sources[&key];
        for (id, window) in &mut self.windows {
            if window.source.as_ref() != Some(&key) {
                continue;
            }
            let session = sessions
                .get(&window.binding.instance_id)
                .context("usage session disappeared")?;
            let facts = session
                .book
                .batch_facts(id)
                .context("usage window disappeared")?;
            let effect_completed = session.book.effect_completed(id);
            window.closing |=
                facts.ended || now_ms() >= facts.expires_at_ms || window.blocked.is_some();
            if let Some(gap) = source.gap {
                window.gap.get_or_insert(gap);
                continue;
            }
            if let Some(gap) = observation.gap {
                window.gap.get_or_insert(gap);
                continue;
            }
            if let Some(cutoff) = &window.cutoff {
                let mut ordinary = false;
                if source.entries[cutoff.count..].iter().any(|e| {
                    if e.kind == Kind::User {
                        ordinary = true;
                        false
                    } else {
                        matches!(
                            e.kind,
                            Kind::Usage | Kind::Compaction | Kind::BranchSummary | Kind::Nested
                        ) || (e.kind == Kind::Assistant && !ordinary)
                    }
                }) {
                    window.gap.get_or_insert(Gap::LateWork);
                }
                continue;
            }
            let same = source.binding == window.binding && source.generation == window.generation;
            if !same {
                window.gap.get_or_insert(Gap::Restart);
                window.closing = true;
                continue;
            }
            let Some(start) = &window.baseline else {
                continue;
            };
            let through = window.observed_through.as_ref().unwrap_or(start);
            if window.gap.is_some() {
                continue;
            }
            let related = observation.window.as_deref() == Some(id.as_str());
            let complete = observation
                .report
                .as_ref()
                .is_some_and(|r| r.head == source.cursor);
            if let Some(offset) = source.entries[start.count..]
                .iter()
                .position(|e| e.kind == Kind::User)
            {
                window.closing = true;
                let count = start.count + offset;
                if through.count > count {
                    window.gap.get_or_insert(Gap::Closing);
                    continue;
                }
                let end = source.prefix(count)?;
                window.observed_through = Some(end.clone());
                if related
                    && complete
                    && observation.boundary == Boundary::BeforeRequest
                    && source.entries[count + 1..]
                        .iter()
                        .all(|e| matches!(e.kind, Kind::Metadata | Kind::User))
                {
                    window.cutoff = Some(end);
                } else {
                    window.gap.get_or_insert(Gap::Closing);
                }
            } else if window.closing {
                if related
                    && complete
                    && (observation.boundary == Boundary::Settled
                        || (!effect_completed
                            && matches!(
                                observation.boundary,
                                Boundary::BeforeRequest | Boundary::TurnEnd
                            )))
                {
                    window.observed_through = Some(source.cursor.clone());
                    window.cutoff = Some(source.cursor.clone());
                } else if !related && observation.boundary == Boundary::BeforeRequest {
                    window.gap.get_or_insert(Gap::Closing);
                }
            } else {
                window.observed_through = Some(source.cursor.clone());
            }
        }
        Ok(())
    }
    pub fn enroll(
        &mut self,
        app: &Path,
        id: &str,
        binding: &Binding,
        generation: &str,
        policy: Option<Policy>,
        required: bool,
    ) -> Result<()> {
        if let Some(policy) = &policy {
            policy.validate()?;
        }
        ensure!(
            !self.windows.contains_key(id),
            "usage baseline already frozen"
        );
        let current = self.current_source(binding).filter(|(_, s)| {
            s.binding == *binding && s.generation == generation && s.gap.is_none()
        });
        let ready = current.and_then(|(key, s)| {
            transport::Clock::now().and_then(|clock| {
                transport::fresh(app, key, s, &clock)
                    .ok()
                    .filter(|v| *v)
                    .map(|_| (key.clone(), s.cursor.clone()))
            })
        });
        ensure!(
            !(required || policy.is_some()) || ready.is_some(),
            Denied(Denial::Unavailable)
        );
        let (source, baseline) = ready.map_or((None, None), |(k, c)| (Some(k), Some(c)));
        for old in self
            .windows
            .values_mut()
            .filter(|w| w.binding.instance_id == binding.instance_id && w.cutoff.is_none())
        {
            old.closing = true;
            old.gap.get_or_insert(Gap::Closing);
        }
        self.windows.insert(
            id.into(),
            Window {
                binding: binding.clone(),
                generation: generation.into(),
                source,
                baseline: baseline.clone(),
                cutoff: None,
                observed_through: baseline,
                closing: false,
                gap: None,
                policy,
                blocked: None,
                ordinary_recovered: false,
            },
        );
        Ok(())
    }
    fn qualified_measure(
        &self,
        app: &Path,
        id: &str,
        policy: &Policy,
        aggregate: bool,
        terminal: bool,
    ) -> Result<Measurement> {
        let window = self.windows.get(id).context(Denied(Denial::Unavailable))?;
        ensure!(window.gap.is_none(), Denied(Denial::Unavailable));
        let key = window
            .source
            .as_ref()
            .context(Denied(Denial::Unavailable))?;
        let source = self.sources.get(key).context(Denied(Denial::Unavailable))?;
        ensure!(
            source.complete() && (!aggregate || source.identity.parent.is_none()),
            Denied(Denial::Unavailable)
        );
        ensure!(
            !transport::negative_evidence(app, source).unwrap_or(true),
            Denied(Denial::Unavailable)
        );
        if window.cutoff.is_none() {
            ensure!(
                !window.closing
                    && !terminal
                    && source.binding == window.binding
                    && source.generation == window.generation,
                Denied(Denial::Unavailable)
            );
            let clock = transport::Clock::now().context(Denied(Denial::Unavailable))?;
            ensure!(
                transport::fresh(app, key, source, &clock).unwrap_or(false),
                Denied(Denial::Stale)
            );
            ensure!(
                window.observed_through.as_ref() == Some(&source.cursor),
                Denied(Denial::Unavailable)
            );
        }
        measure(
            source,
            window
                .baseline
                .as_ref()
                .context(Denied(Denial::Unavailable))?,
            window
                .observed_through
                .as_ref()
                .context(Denied(Denial::Unavailable))?,
            Some(policy),
        )
    }
}

fn failure(result: Result<Option<Denial>>) -> Option<Denial> {
    match result {
        Ok(value) => value,
        Err(error) => Some(
            error
                .downcast_ref::<Denied>()
                .map_or(Denial::Unavailable, |e| e.0),
        ),
    }
}
pub(in crate::session::afk) fn check(
    snapshot: &authority::Snapshot,
    app: &Path,
    id: &str,
) -> Option<Decision> {
    let local = snapshot.usage.windows.get(id);
    let operation = snapshot
        .batches
        .values()
        .find(|o| o.members.values().any(|m| m.window.as_deref() == Some(id)));
    let shared = operation.and_then(|o| snapshot.usage.batches.get(&o.id));
    let terminal = |id: &str| {
        snapshot
            .usage
            .windows
            .get(id)
            .and_then(|w| {
                snapshot
                    .sessions
                    .get(&w.binding.instance_id)?
                    .book
                    .batch_facts(id)
            })
            .is_none_or(|f| f.ended || now_ms() >= f.expires_at_ms)
    };
    if let Some(guard) = shared {
        let operation = operation.unwrap();
        let denial = guard.blocked.or_else(|| {
            failure((|| -> Result<_> {
                let policy = &guard.policy;
                let mut natives = BTreeSet::new();
                let mut files = BTreeSet::new();
                let mut tokens = 0;
                let mut cost = 0;
                for member in operation.members.values() {
                    let Some(window) = &member.window else {
                        continue;
                    };
                    let w = snapshot
                        .usage
                        .windows
                        .get(window)
                        .context(Denied(Denial::Unavailable))?;
                    let source = snapshot
                        .usage
                        .sources
                        .get(w.source.as_ref().context(Denied(Denial::Unavailable))?)
                        .context(Denied(Denial::Unavailable))?;
                    ensure!(
                        natives.insert(&source.identity.native_id)
                            && files.insert(&source.identity.file),
                        Denied(Denial::Unavailable)
                    );
                    let m = snapshot.usage.qualified_measure(
                        app,
                        window,
                        policy,
                        true,
                        terminal(window),
                    )?;
                    if m.deny(policy) == Some(Denial::Unavailable) {
                        return Ok(Some(Denial::Unavailable));
                    }
                    tokens = add(tokens, m.reported_tokens)?;
                    cost = add(cost, m.operator_estimate_nano.unwrap_or(0))?;
                }
                Ok(
                    if policy.tokens.is_some_and(|n| tokens >= n)
                        || policy.usd.as_ref().is_some_and(|u| cost >= u.limit_nano)
                    {
                        Some(Denial::Exhausted)
                    } else {
                        None
                    },
                )
            })())
        });
        if let Some(reason) = denial {
            return Some(Decision {
                reason,
                shared: Some(operation.id.clone()),
            });
        }
    }
    let local = local?;
    if let Some(reason) = local.blocked {
        return Some(Decision {
            reason,
            shared: None,
        });
    }
    let policy = local.policy.as_ref()?;
    failure(
        snapshot
            .usage
            .qualified_measure(app, id, policy, false, terminal(id))
            .map(|m| m.deny(policy)),
    )
    .map(|reason| Decision {
        reason,
        shared: None,
    })
}
pub(in crate::session::afk) fn latch(
    snapshot: &mut authority::Snapshot,
    id: &str,
    decision: &Decision,
) -> Result<()> {
    if let Some(operation) = &decision.shared {
        ensure!(
            snapshot
                .batches
                .get(operation)
                .is_some_and(|o| o.members.values().any(|m| m.window.as_deref() == Some(id))),
            "denial outside shared operation"
        );
        snapshot
            .usage
            .batches
            .get_mut(operation)
            .context("shared usage guard missing")?
            .blocked
            .get_or_insert(decision.reason);
    }
    let window = snapshot
        .usage
        .windows
        .get_mut(id)
        .context("usage window missing")?;
    window.blocked.get_or_insert(decision.reason);
    window.closing = true;
    snapshot.usage.sync_lifetime(&mut snapshot.sessions)
}
