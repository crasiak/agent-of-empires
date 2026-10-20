//! A pending publication is a veto, never another authority snapshot.
use super::*;
use crate::session::afk::delegation::hash;
use std::fmt;

pub(super) const PENDING: &str = "publication.pending.json";

#[derive(Debug)]
pub(in crate::session::afk) struct Parked {
    pub committed: Option<bool>,
}
impl fmt::Display for Parked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AFK profile publication is pending or quarantined (data commit: {}); authority unavailable; retain failed-publication evidence for explicit review, never delete the journal or automatically retry activation", match self.committed { Some(true) => "committed", Some(false) => "not published", None => "unknown" })
    }
}
impl std::error::Error for Parked {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub(in crate::session::afk) struct Committed {
    pub cleanup_durable: bool,
}
const CLEANUP_WARNING: &str = "AFK snapshot committed; publication-veto removal durability is uncertain, restart may park the profile";
thread_local! { static WARNINGS: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) }; }
impl Committed {
    pub fn warn(self) {
        if !self.cleanup_durable {
            tracing::warn!("{CLEANUP_WARNING}");
            WARNINGS.with(|slot| {
                if let Some(warnings) = slot.borrow_mut().as_mut() {
                    if !warnings.iter().any(|w| w == CLEANUP_WARNING) {
                        warnings.push(CLEANUP_WARNING.into());
                    }
                }
            });
        }
    }
}
pub(in crate::session::afk) fn collect<T>(
    run: impl FnOnce() -> Result<T>,
) -> (Result<T>, Vec<String>) {
    struct Restore(Option<Option<Vec<String>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                WARNINGS.with(|s| *s.borrow_mut() = previous);
            }
        }
    }
    let mut restore = Restore(Some(WARNINGS.with(|s| s.replace(Some(Vec::new())))));
    let result = run();
    let warnings = WARNINGS.with(|s| s.take().unwrap_or_default());
    let mut previous = restore.0.take().unwrap();
    if let Some(outer) = &mut previous {
        for warning in &warnings {
            if !outer.contains(warning) {
                outer.push(warning.clone());
            }
        }
    }
    WARNINGS.with(|s| *s.borrow_mut() = previous);
    (result, warnings)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: u32,
    transaction: String,
    profile: String,
    prior_revision: Option<u64>,
    prior_hash: Option<String>,
    proposed_hash: String,
    proposal: Snapshot,
}

pub(in crate::session::afk) fn publication_bound(proposal: &Snapshot) -> Result<usize> {
    Ok(serde_json::to_vec(&Pending {
        version: 1,
        transaction: "00000000-0000-4000-8000-000000000000".into(),
        profile: proposal.profile.clone(),
        prior_revision: Some(u64::MAX),
        prior_hash: Some("0".repeat(64)),
        proposed_hash: "0".repeat(64),
        proposal: proposal.clone(),
    })?
    .len())
}

pub(super) fn clear(store: &Store) -> Result<()> {
    ensure!(
        store.dir.entry_stat(Path::new(PENDING))?.is_none(),
        Parked { committed: None }
    );
    Ok(())
}

// Caller retains the profile lock until the outcome has been classified.
pub(in crate::session::afk) fn publish(
    store: &Store,
    prior: Option<&Snapshot>,
    proposal: &Snapshot,
) -> Result<Committed> {
    clear(store)?;
    ensure!(
        store.dir.path().file_name().and_then(|n| n.to_str()) == Some(proposal.profile.as_str()),
        "publication store/profile mismatch"
    );
    proposal.validate(&proposal.profile)?;
    let prior_hash = prior.map(serde_json::to_vec).transpose()?.map(hash);
    let current: Option<Snapshot> = store.read_raw("snapshot.json")?;
    if let Some(current) = &current {
        current.validate(&proposal.profile)?;
    }
    ensure!(
        current
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()?
            .map(hash)
            == prior_hash,
        "publication prior differs from current authority"
    );
    if let Some(prior) = prior {
        prior.transition(proposal)?;
        ensure!(
            prior.profile == proposal.profile,
            "publication profile changed"
        );
        ensure!(
            prior.revision.checked_add(1) == Some(proposal.revision),
            "publication revision must advance exactly once"
        );
    } else {
        ensure!(
            proposal.revision == 1,
            "initial profile revision must be one"
        );
    }
    let bytes = serde_json::to_vec(proposal)?;
    let pending = Pending {
        version: 1,
        transaction: uuid::Uuid::new_v4().to_string(),
        profile: proposal.profile.clone(),
        prior_revision: prior.map(|p| p.revision),
        prior_hash,
        proposed_hash: hash(&bytes),
        proposal: proposal.clone(),
    };
    let evidence = serde_json::to_vec(&pending)?;
    ensure!(
        evidence.len() <= PROFILE_BYTES,
        "profile publication including pending envelope exceeds 32 MiB"
    );
    #[cfg(test)]
    fault(store, Fault::Journal)?;
    store.write_exclusive(PENDING, &evidence).map_err(|e| {
        e.context(Parked {
            committed: Some(false),
        })
    })?;
    #[cfg(test)]
    AFTER_JOURNAL.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    #[cfg(test)]
    fault(store, Fault::Canonical)?;
    store
        .write_bytes("snapshot.json", &bytes)
        .map_err(|e| e.context(Parked { committed: None }))?;
    // Only the transaction that observed the durable data commit may remove its veto.
    let recorded = store.bytes(PENDING).map_err(|e| {
        e.context(Parked {
            committed: Some(true),
        })
    })?;
    ensure!(
        recorded.as_deref() == Some(&evidence),
        Parked {
            committed: Some(true)
        }
    );
    #[cfg(test)]
    if take_fault(Fault::Unlink) {
        return Err(anyhow::Error::new(Parked {
            committed: Some(true),
        })
        .context("injected journal unlink failure"));
    }
    store.dir.remove_file(Path::new(PENDING)).map_err(|e| {
        e.context(Parked {
            committed: Some(true),
        })
    })?;
    #[cfg(test)]
    fault(store, Fault::RemovalSync)?;
    let cleanup_durable = store.dir.sync().is_ok();
    Ok(Committed { cleanup_durable })
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::session::afk) enum Fault {
    Journal,
    Canonical,
    Unlink,
    RemovalSync,
}
#[cfg(test)]
thread_local! {
    static FAIL: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
    static AFTER_JOURNAL: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(in crate::session::afk) fn fail_next(at: Fault) {
    FAIL.with(|f| f.set(Some(at)));
}

#[cfg(test)]
fn take_fault(at: Fault) -> bool {
    FAIL.with(|f| {
        if f.get() == Some(at) {
            f.set(None);
            true
        } else {
            false
        }
    })
}
#[cfg(test)]
fn fault(store: &Store, at: Fault) -> Result<()> {
    if take_fault(at) {
        crate::session::anchored_fs::FAIL_SYNC_ONCE
            .with(|path| *path.borrow_mut() = Some(store.dir.path().to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn fixture() -> (tempfile::TempDir, Store, Snapshot, Snapshot) {
        let app = tempfile::tempdir().unwrap();
        let store = profile_store(app.path(), "alpha", true).unwrap();
        let prior = Snapshot::empty("alpha");
        store.write_raw("snapshot.json", &prior).unwrap();
        let mut proposal = prior.clone();
        proposal.revision += 1;
        proposal.sessions.insert("member".into(), Session::empty());
        (app, store, prior, proposal)
    }

    #[test]
    fn failed_publications_retain_veto_and_do_not_infer_rollback() {
        for at in [
            Fault::Journal,
            Fault::Canonical,
            Fault::Unlink,
            Fault::RemovalSync,
        ] {
            let (_app, store, prior, proposal) = fixture();
            let _lock = store.lock(TIMEOUT).unwrap();
            FAIL.with(|f| f.set(Some(at)));
            let result = publish(&store, Some(&prior), &proposal);
            if at == Fault::RemovalSync {
                assert_eq!(
                    result.unwrap(),
                    Committed {
                        cleanup_durable: false
                    }
                );
                clear(&store).unwrap();
                assert_eq!(
                    store.bytes("snapshot.json").unwrap().unwrap(),
                    serde_json::to_vec(&proposal).unwrap()
                );
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    error.downcast_ref::<Parked>().unwrap().committed,
                    match at {
                        Fault::Journal => Some(false),
                        Fault::Unlink => Some(true),
                        _ => None,
                    }
                );
                assert!(clear(&store).is_err());
                let pending = store.bytes(PENDING).unwrap().unwrap();
                let recorded: Pending = serde_json::from_slice(&pending).unwrap();
                assert_eq!(
                    recorded.proposed_hash,
                    hash(serde_json::to_vec(&proposal).unwrap())
                );
                let current = store.bytes("snapshot.json").unwrap();
                assert!(publish(&store, Some(&prior), &proposal).is_err());
                assert_eq!(store.bytes("snapshot.json").unwrap(), current);
                assert_eq!(store.bytes(PENDING).unwrap().unwrap(), pending);
            }
        }
    }

    #[test]
    fn known_commit_warning_is_returned_without_leaking_to_another_operation() {
        let (_app, store, prior, proposal) = fixture();
        let _lock = store.lock(TIMEOUT).unwrap();
        FAIL.with(|f| f.set(Some(Fault::RemovalSync)));
        let receipt = authority::publication_result(|| {
            publish(&store, Some(&prior), &proposal)?.warn();
            Ok(serde_json::json!({"authorized":true}))
        })
        .unwrap();
        assert_eq!(receipt["authorized"], true);
        assert_eq!(receipt["publication_warnings"].as_array().unwrap().len(), 1);
        clear(&store).unwrap();
        assert_eq!(
            store.bytes("snapshot.json").unwrap().unwrap(),
            serde_json::to_vec(&proposal).unwrap()
        );
        let next = authority::publication_result(|| Ok(serde_json::json!({"other":true}))).unwrap();
        assert!(next.get("publication_warnings").is_none());
    }

    #[test]
    fn uncertain_veto_cleanup_can_reappear_only_as_a_veto() {
        let (app, store, prior, proposal) = fixture();
        let captured = std::rc::Rc::new(std::cell::RefCell::new(None));
        let captured_copy = captured.clone();
        let pending_path = store.dir.path().join(PENDING);
        AFTER_JOURNAL.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                *captured_copy.borrow_mut() = Some(std::fs::read(&pending_path).unwrap());
            }))
        });
        {
            let _lock = store.lock(TIMEOUT).unwrap();
            FAIL.with(|f| f.set(Some(Fault::RemovalSync)));
            assert!(
                !publish(&store, Some(&prior), &proposal)
                    .unwrap()
                    .cleanup_durable
            );
        }
        let committed = store.bytes("snapshot.json").unwrap();
        store
            .write_exclusive(PENDING, captured.borrow().as_ref().unwrap())
            .unwrap();
        let fresh = profile_store(app.path(), "alpha", false).unwrap();
        assert!(clear(&fresh).is_err());
        let mut retry = proposal.clone();
        retry.revision += 1;
        let _lock = fresh.lock_existing(TIMEOUT).unwrap();
        assert!(publish(&fresh, Some(&proposal), &retry).is_err());
        assert_eq!(fresh.bytes("snapshot.json").unwrap(), committed);
    }

    #[test]
    fn stale_prior_initialization_and_foreign_scope_refuse_before_journaling() {
        for case in ["stale", "existing-initial", "scope"] {
            let (_app, store, prior, mut proposal) = fixture();
            let _lock = store.lock(TIMEOUT).unwrap();
            if case == "stale" {
                let mut advanced = prior.clone();
                advanced.revision = 3;
                advanced.sessions.insert("sibling".into(), Session::empty());
                store.write_raw("snapshot.json", &advanced).unwrap();
            } else if case == "existing-initial" {
                proposal = prior.clone();
            } else {
                store.dir.remove_file(Path::new("snapshot.json")).unwrap();
                proposal = Snapshot::empty("beta");
            }
            let before = store.bytes("snapshot.json").unwrap();
            assert!(
                publish(&store, (case == "stale").then_some(&prior), &proposal).is_err(),
                "{case}"
            );
            assert_eq!(store.bytes("snapshot.json").unwrap(), before);
            assert!(store.dir.entry_stat(Path::new(PENDING)).unwrap().is_none());
        }
    }

    #[test]
    fn cached_sibling_bridge_waits_for_publication_then_rechecks_veto() {
        for fail_canonical in [false, true] {
            let (app, store, mut prior, mut proposal) = fixture();
            prior.sessions.insert("reader".into(), Session::empty());
            proposal.sessions.insert("reader".into(), Session::empty());
            store.write_raw("snapshot.json", &prior).unwrap();
            drop(store.lock(TIMEOUT).unwrap());
            let transport = Store::open_namespace(
                app.path(),
                "reader",
                true,
                &format!("{NAMESPACE}/profiles/alpha/transport"),
                MAX_BYTES,
            )
            .unwrap();
            let (prepared_tx, prepared_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let path = app.path().to_owned();
            let writer = std::thread::spawn(move || {
                let store = profile_store(&path, "alpha", false).unwrap();
                let _lock = store.lock_existing(TIMEOUT).unwrap();
                AFTER_JOURNAL.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        prepared_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }))
                });
                if fail_canonical {
                    FAIL.with(|f| f.set(Some(Fault::Canonical)));
                }
                let result = publish(&store, Some(&prior), &proposal);
                assert_eq!(result.is_err(), fail_canonical);
                if let Ok(committed) = result {
                    assert!(committed.cleanup_durable);
                }
            });
            prepared_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let (contended_tx, contended_rx) = mpsc::channel();
            let path = app.path().to_owned();
            let reader = std::thread::spawn(move || {
                super::super::super::store::LOCK_CONTENDED
                    .with(|s| *s.borrow_mut() = Some(contended_tx));
                let mut runtime = profile_store(&path, "alpha", false).unwrap();
                runtime.scope = Some(("alpha".into(), "reader".into()));
                let binding = Binding {
                    instance_id: "reader".into(),
                    profile: "alpha".into(),
                    native_id: "native".into(),
                    launch_id: "launch".into(),
                };
                let mut output = Vec::new();
                super::super::super::bridge::serve(
                    &transport,
                    Some((&runtime, &path)),
                    &binding,
                    &uuid::Uuid::new_v4().to_string(),
                    std::io::Cursor::new(b"{\"op\":\"poll\"}\n"),
                    &mut output,
                    || Ok(()),
                )
                .unwrap();
                serde_json::from_slice::<Value>(&output).unwrap()
            });
            let waited = contended_rx.recv_timeout(Duration::from_secs(3)).is_ok();
            release_tx.send(()).unwrap();
            writer.join().unwrap();
            let reply = reader.join().unwrap();
            assert!(
                waited,
                "cached helper rejected an in-flight journal without waiting"
            );
            assert_eq!(reply["ok"], !fail_canonical, "{reply}");
            if fail_canonical {
                assert!(reply["error"].as_str().unwrap().contains("pending"));
            }
        }
    }

    #[test]
    fn competing_reader_and_restart_cannot_adopt_visible_uncertain_snapshot() {
        let (app, store, prior, proposal) = fixture();
        drop(store.lock(TIMEOUT).unwrap());
        let (prepared_tx, prepared_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let path = app.path().to_owned();
        let writer = std::thread::spawn(move || {
            let store = profile_store(&path, "alpha", false).unwrap();
            let _lock = store.lock_existing(TIMEOUT).unwrap();
            AFTER_JOURNAL.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    prepared_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }))
            });
            FAIL.with(|f| f.set(Some(Fault::Canonical)));
            assert!(publish(&store, Some(&prior), &proposal).is_err());
        });
        prepared_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (contended_tx, contended_rx) = mpsc::channel();
        let path = app.path().to_owned();
        let reader = std::thread::spawn(move || {
            super::super::super::store::LOCK_CONTENDED
                .with(|s| *s.borrow_mut() = Some(contended_tx));
            let store = profile_store(&path, "alpha", false).unwrap();
            let _lock = store.lock_existing(Duration::from_secs(5)).unwrap();
            assert!(clear(&store).is_err());
        });
        contended_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        reader.join().unwrap();
        let fresh = profile_store(app.path(), "alpha", false).unwrap();
        assert!(clear(&fresh).is_err());
        let visible: Snapshot = fresh.read_raw("snapshot.json").unwrap().unwrap();
        assert_eq!(visible.revision, 2);
        assert!(visible.sessions.contains_key("member"));
    }
}
