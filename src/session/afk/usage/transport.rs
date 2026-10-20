use super::*;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Clock {
    pub boot: String,
    pub millis: u64,
}
impl Clock {
    pub fn now() -> Option<Self> {
        static BOOT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        Some(Self {
            boot: BOOT.get_or_init(crate::process::boot_id).clone()?,
            millis: crate::process::monotonic_millis()?,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    binding: Binding,
    generation: String,
    incarnation: String,
    boot: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::session::afk) struct Lease {
    pub binding: Binding,
    pub generation: String,
    pub incarnation: String,
    pub source: String,
    pub cursor: Cursor,
    pub clock: Clock,
}
impl Lease {
    fn matches(&self, hello: &Hello, source: &Source, key: &str, now: &Clock) -> bool {
        self.binding == source.binding
            && self.generation == source.generation
            && self.source == key
            && self.cursor == source.cursor
            && source.complete()
            && self.binding == hello.binding
            && self.generation == hello.generation
            && self.incarnation == hello.incarnation
            && hello.boot.as_ref() == Some(&now.boot)
            && self.clock.boot == now.boot
            && now
                .millis
                .checked_sub(self.clock.millis)
                .is_some_and(|age| age < LEASE_MS)
    }
}
#[derive(Clone, Debug, Serialize)]
pub(in crate::session::afk) struct Challenge {
    pub version: u32,
    pub id: String,
    pub after: Cursor,
    pub identity: Option<Identity>,
}
pub(in crate::session::afk) const UNRESOLVED: &str = "usage-unresolved.json";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Unpublished {
    version: u32,
    identity: Identity,
    high_water: usize,
    heads: BTreeMap<usize, String>,
    parked: bool,
}
impl Unpublished {
    fn new(report: &Report) -> Self {
        Self {
            version: 1,
            identity: report.identity.clone(),
            high_water: report.head.count,
            heads: BTreeMap::from([(report.head.count, report.head.hash.clone())]),
            parked: false,
        }
    }
    fn gap(source: &Source) -> Self {
        Self {
            version: 1,
            identity: source.identity.clone(),
            high_water: source.cursor.count,
            heads: BTreeMap::from([(source.cursor.count, source.cursor.hash.clone())]),
            parked: true,
        }
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.high_water <= MAX_ENTRIES
                && !self.heads.is_empty()
                && self.heads.len() <= REPORT_ENTRIES
                && (self.parked || self.heads.contains_key(&self.high_water)),
            "invalid negative usage evidence"
        );
        text(&self.identity.native_id)?;
        digest(&self.identity.file)?;
        if let Some(parent) = &self.identity.parent {
            digest(parent)?;
        }
        for (count, hash) in &self.heads {
            ensure!(
                *count <= self.high_water,
                "negative evidence exceeds announced head"
            );
            digest(hash)?;
        }
        Ok(())
    }
    fn consider(&mut self, binding: &Binding, report: &Report) -> Result<()> {
        self.validate()?;
        if report.validate().is_err()
            || report.identity.validate(binding).is_err()
            || self.identity != report.identity
            || report.head.count < self.high_water
            || self
                .heads
                .get(&report.head.count)
                .is_some_and(|h| *h != report.head.hash)
        {
            self.parked = true;
            return Ok(());
        }
        self.high_water = self.high_water.max(report.head.count);
        if self.heads.len() >= REPORT_ENTRIES && !self.heads.contains_key(&report.head.count) {
            self.parked = true;
        } else {
            self.heads
                .insert(report.head.count, report.head.hash.clone());
        }
        self.validate()
    }
    fn resolved(&self, source: &Source) -> bool {
        self.validate().is_ok()
            && !self.parked
            && self.identity == source.identity
            && source.complete()
            && source.cursor.count >= self.high_water
            && self
                .heads
                .iter()
                .all(|(count, hash)| source.prefix(*count).is_ok_and(|c| c.hash == *hash))
    }
}
pub(in crate::session::afk) fn negative_evidence(app: &Path, source: &Source) -> Result<bool> {
    let store =
        Store::existing_transport(app, &source.binding.profile, &source.binding.instance_id)?
            .context("usage evidence transport missing")?;
    let Some(pending) = store.read_raw::<Unpublished>(UNRESOLVED)? else {
        return Ok(false);
    };
    pending.validate()?;
    if pending.parked {
        return Ok(true);
    }
    Ok(pending.identity.native_id == source.identity.native_id
        || pending.identity.file == source.identity.file
        || pending.identity.parent.is_some()
        || source.identity.parent.is_some())
}

pub(in crate::session::afk) struct Server {
    incarnation: String,
    challenge: Option<Challenge>,
    publication_limit: usize,
}
impl Server {
    pub fn new() -> Self {
        Self {
            incarnation: uuid::Uuid::new_v4().to_string(),
            challenge: None,
            publication_limit: authority::PROFILE_BYTES,
        }
    }
    #[cfg(test)]
    pub fn limit_publication_for_test(&mut self, limit: usize) {
        self.publication_limit = limit;
    }
    pub fn observe(
        &mut self,
        transport: &Store,
        runtime: &Store,
        binding: &Binding,
        generation: &str,
        observation: Option<Observation>,
        validate: impl Fn() -> Result<()>,
    ) -> Result<Value> {
        let mut tx = authority::SessionTransaction::open(runtime)?;
        validate()?;
        let mut next = tx.snapshot().clone();
        let mut diagnostic = None;
        let mut unresolved = match transport
            .read_raw::<Unpublished>(UNRESOLVED)
            .and_then(|value| {
                if let Some(pending) = &value {
                    pending.validate()?;
                }
                Ok(value)
            }) {
            Ok(value) => value,
            Err(error) => {
                diagnostic = Some(format!("unreadable negative usage evidence: {error}"));
                None
            }
        };
        let mut unresolved_bad = diagnostic.is_some();
        let accepted = observation.as_ref().is_some_and(|o| {
            o.report.as_ref().is_some_and(|r| {
                self.challenge
                    .as_ref()
                    .is_some_and(|c| r.challenge == c.id && r.after == c.after)
            })
        });
        transport.write_raw("usage-lease.json", &Value::Null)?;
        let clock = Clock::now();
        let hello = Hello {
            binding: binding.clone(),
            generation: generation.into(),
            incarnation: self.incarnation.clone(),
            boot: clock.as_ref().map(|c| c.boot.clone()),
        };
        transport.write_raw("usage-reporter.json", &hello)?;
        if accepted {
            if let (Some(pending), Some(report)) = (
                &mut unresolved,
                observation.as_ref().and_then(|o| o.report.as_ref()),
            ) {
                let before = pending.clone();
                pending.consider(binding, report)?;
                if *pending != before {
                    transport.write_raw(UNRESOLVED, pending)?;
                }
            }
        }
        if let Some(observation) = &observation {
            if observation.report.is_some() && !accepted {
                diagnostic = Some("usage challenge or committed prefix changed".to_owned());
            } else if let Err(error) =
                next.usage
                    .ingest(binding, generation, observation, &mut next.sessions)
            {
                if !unresolved_bad {
                    if let Some((_, source)) = tx.snapshot().usage.current_source(binding) {
                        let pending = unresolved.get_or_insert_with(|| Unpublished::gap(source));
                        pending.parked = true;
                        transport.write_raw(UNRESOLVED, pending)?;
                    }
                }
                return Err(error);
            }
        }
        if next.usage != tx.snapshot().usage
            || next
                .sessions
                .iter()
                .any(|(id, s)| s.epoch != tx.snapshot().sessions[id].epoch)
        {
            next.usage.sync_lifetime(&mut next.sessions)?;
            let eligible = next.validate(&binding.profile).and_then(|()| {
                ensure!(
                    authority::journal::publication_bound(&next)? <= self.publication_limit,
                    "usage publication capacity unavailable"
                );
                Ok(())
            });
            if let Err(error) = eligible {
                diagnostic = Some(error.to_string());
                if !unresolved_bad {
                    let proven = observation
                        .as_ref()
                        .and_then(|o| o.report.as_ref())
                        .filter(|r| {
                            accepted
                                && r.validate().is_ok()
                                && r.identity.validate(binding).is_ok()
                                && next.usage.current_source(binding).is_some_and(|(_, s)| {
                                    s.identity == r.identity && s.gap.is_none()
                                })
                        });
                    if let Some(report) = proven {
                        let pending = unresolved.get_or_insert_with(|| Unpublished::new(report));
                        pending.consider(binding, report)?;
                        transport.write_raw(UNRESOLVED, pending)?;
                    } else if let Some((_, source)) = tx.snapshot().usage.current_source(binding) {
                        let pending = unresolved.get_or_insert_with(|| Unpublished::gap(source));
                        pending.parked = true;
                        transport.write_raw(UNRESOLVED, pending)?;
                    }
                }
            } else {
                tx.publish(next)?.warn();
            }
        }
        let source = tx.snapshot().usage.current_source(binding);
        if accepted
            && diagnostic.is_none()
            && unresolved
                .as_ref()
                .is_some_and(|p| source.is_some_and(|(_, s)| p.resolved(s)))
        {
            match transport
                .dir
                .remove_file(std::path::Path::new(UNRESOLVED))
                .and_then(|()| transport.dir.sync())
            {
                Ok(()) => unresolved = None,
                Err(error) => {
                    unresolved_bad = true;
                    diagnostic = Some(format!(
                        "negative usage evidence cleanup unavailable: {error}"
                    ));
                }
            }
        }
        if unresolved.is_some() || unresolved_bad {
            diagnostic.get_or_insert_with(|| {
                "unresolved usage evidence; not fresh or repaired by an older prefix".into()
            });
        }
        let mut fresh = false;
        if accepted {
            if let (Some((key, source)), Some(report), Some(clock)) = (
                source,
                observation.as_ref().and_then(|o| o.report.as_ref()),
                clock,
            ) {
                if source.cursor == report.head
                    && source.binding == *binding
                    && source.generation == generation
                    && source.complete()
                    && diagnostic.is_none()
                {
                    transport.write_raw(
                        "usage-lease.json",
                        &Lease {
                            binding: binding.clone(),
                            generation: generation.into(),
                            incarnation: self.incarnation.clone(),
                            source: key.clone(),
                            cursor: source.cursor.clone(),
                            clock,
                        },
                    )?;
                    fresh = true;
                }
            }
        }
        let challenge = Challenge {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            after: source.map_or_else(Cursor::default, |(_, s)| s.cursor.clone()),
            identity: source.map(|(_, s)| s.identity.clone()),
        };
        self.challenge = Some(challenge.clone());
        Ok(
            serde_json::json!({"challenge":challenge,"fresh":fresh,"diagnostic":diagnostic,"coverage":if source.is_none(){"unavailable"}else if fresh{"fresh_source_watermark"}else{"unavailable_or_stale"}}),
        )
    }
}
pub(in crate::session::afk) fn fresh(
    app: &std::path::Path,
    key: &str,
    source: &Source,
    now: &Clock,
) -> Result<bool> {
    let Some(transport) =
        Store::existing_transport(app, &source.binding.profile, &source.binding.instance_id)?
    else {
        return Ok(false);
    };
    if transport
        .dir
        .entry_stat(std::path::Path::new(UNRESOLVED))?
        .is_some()
    {
        return Ok(false);
    }
    let Some(hello) = transport.read_raw::<Hello>("usage-reporter.json")? else {
        return Ok(false);
    };
    let Some(lease) = transport
        .read_raw::<Option<Lease>>("usage-lease.json")?
        .flatten()
    else {
        return Ok(false);
    };
    Ok(lease.matches(&hello, source, key, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn freshness_binds_boot_incarnation_watermark_and_monotonic_deadline() {
        let binding = Binding {
            instance_id: "a".into(),
            profile: "p".into(),
            native_id: "n".into(),
            launch_id: "l".into(),
        };
        let source = Source::new(
            binding.clone(),
            uuid::Uuid::new_v4().to_string(),
            Identity {
                native_id: "n".into(),
                file: "a".repeat(64),
                parent: None,
            },
        );
        let key = source.identity.key(&binding).unwrap();
        let hello = Hello {
            binding,
            generation: source.generation.clone(),
            incarnation: uuid::Uuid::new_v4().to_string(),
            boot: Some("boot".into()),
        };
        let lease = Lease {
            binding: hello.binding.clone(),
            generation: hello.generation.clone(),
            incarnation: hello.incarnation.clone(),
            source: key.clone(),
            cursor: source.cursor.clone(),
            clock: Clock {
                boot: "boot".into(),
                millis: 1000,
            },
        };
        for (millis, boot, expected) in [
            (1000, "boot", true),
            (5999, "boot", true),
            (6000, "boot", false),
            (999, "boot", false),
            (1000, "reboot", false),
        ] {
            assert_eq!(
                lease.matches(
                    &hello,
                    &source,
                    &key,
                    &Clock {
                        boot: boot.into(),
                        millis
                    }
                ),
                expected
            );
        }
        let now = Clock {
            boot: "boot".into(),
            millis: 1001,
        };
        let mut restarted = hello.clone();
        restarted.incarnation = uuid::Uuid::new_v4().to_string();
        assert!(!lease.matches(&restarted, &source, &key, &now));
        let mut advanced = source.clone();
        advanced.cursor.count += 1;
        assert!(!lease.matches(&hello, &advanced, &key, &now));
        advanced = source.clone();
        advanced.gap = Some(Gap::Closing);
        assert!(!lease.matches(&hello, &advanced, &key, &now));
    }
}
