//! Immutable revisions; the Session row's revision is the sole commit pointer.

use super::*;
use crate::session::anchored_fs::AnchoredDir;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArcField {
    pub observed_at: Option<DateTime<Utc>>,
    pub changed_at: Option<DateTime<Utc>>,
    pub source: ArcIdentity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArcDocument {
    version: u32,
    pub revision: String,
    pub previous: Option<String>,
    pub record: StoredArc,
    pub fields: [ArcField; 4],
}

impl ArcDocument {
    pub(crate) fn observe(record: StoredArc, previous: Option<&Self>, legacy: bool) -> Self {
        let fields = std::array::from_fn(|index| {
            let unchanged = previous.is_some_and(|previous| {
                let old = &previous.record.arc;
                let new = &record.arc;
                [&old.goal, &old.now, &old.remaining, &old.next][index]
                    == [&new.goal, &new.now, &new.remaining, &new.next][index]
            });
            if let Some(previous) = previous.filter(|_| unchanged) {
                previous.fields[index].clone()
            } else {
                ArcField {
                    observed_at: (!legacy).then_some(record.at),
                    changed_at: (!legacy).then_some(record.at),
                    source: record.identity.clone(),
                }
            }
        });
        Self {
            version: 1,
            revision: uuid::Uuid::new_v4().to_string(),
            previous: previous.map(|doc| doc.revision.clone()),
            record,
            fields,
        }
    }
}

fn revision_path(session_id: &str, revision: &str) -> Result<PathBuf> {
    crate::session::validate_instance_id(session_id)?;
    if uuid::Uuid::parse_str(revision)?.to_string() != revision {
        bail!("noncanonical work arc revision");
    }
    Ok(Path::new("work-arcs")
        .join(session_id)
        .join(format!("{revision}.json")))
}

pub(crate) fn read_document(root: &Path, session_id: &str, revision: &str) -> Result<ArcDocument> {
    let path = revision_path(session_id, revision)?;
    let anchor = AnchoredDir::open(root)?;
    let bytes = anchor
        .read_regular(&path, MAX_DOCUMENT_BYTES)?
        .context("missing or unsafe work arc revision")?;
    let document: ArcDocument = serde_json::from_slice(&bytes)?;
    if document.version != 1
        || document.revision != revision
        || document.record.identity.session_id != session_id
    {
        bail!("foreign or unsupported work arc revision");
    }
    WorkArc::from_json(&serde_json::to_string(&document.record.arc)?)
        .map_err(anyhow::Error::msg)?;
    if let Some(previous) = &document.previous {
        revision_path(session_id, previous)?;
        if previous == revision {
            bail!("cyclic work arc revision");
        }
    }
    if document
        .fields
        .iter()
        .any(|field| field.source.session_id != session_id)
    {
        bail!("foreign work arc field provenance");
    }
    Ok(document)
}

pub(crate) fn publish_document(root: &Path, document: &ArcDocument) -> Result<()> {
    let path = revision_path(&document.record.identity.session_id, &document.revision)?;
    if let Ok(existing) = read_document(
        root,
        &document.record.identity.session_id,
        &document.revision,
    ) {
        anyhow::ensure!(existing == *document, "work arc revision collision");
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(document)?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        bail!("work arc revision exceeds limit");
    }
    let anchor = AnchoredDir::open(root)?;
    anchor.ensure_dir(path.parent().context("revision parent")?)?;
    if !anchor.publish_file(
        &path,
        &mut bytes.as_slice(),
        std::fs::Permissions::from_mode(0o600),
        false,
        None,
    )? {
        let existing = read_document(
            root,
            &document.record.identity.session_id,
            &document.revision,
        )?;
        if existing != *document {
            bail!("work arc revision collision");
        }
    }
    Ok(())
}

pub(crate) fn hydrate(root: &Path, session: &mut crate::session::Instance) -> Result<()> {
    if let Some(revision) = &session.work_arc.revision {
        let document = read_document(root, &session.id, revision)?;
        session.work_arc.record = Some(document.record.clone());
        session.work_arc.document = Some(Box::new(document));
    }
    Ok(())
}

pub(crate) fn publish_state(root: &Path, session: &crate::session::Instance) -> Result<()> {
    match (&session.work_arc.revision, &session.work_arc.document) {
        (Some(revision), Some(document))
            if revision == &document.revision
                && document.record.identity.session_id == session.id
                && session.work_arc.record.as_ref() == Some(&document.record) =>
        {
            publish_document(root, document)
        }
        (None, None) if session.work_arc.record.is_none() => Ok(()),
        _ => bail!("work arc commit lacks its matching document"),
    }
}

pub(crate) fn history(
    root: &Path,
    session: &crate::session::Instance,
    limit: usize,
) -> Result<Vec<ArcDocument>> {
    let mut revision = session.work_arc.revision.clone();
    let mut documents = Vec::new();
    let mut seen = HashSet::new();
    while let Some(current) = revision.take().filter(|_| documents.len() < limit.min(20)) {
        if !seen.insert(current.clone()) {
            bail!("cyclic work arc history");
        }
        let document = read_document(root, &session.id, &current)?;
        revision = document.previous.clone();
        documents.push(document);
    }
    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn record(session_id: &str) -> StoredArc {
        StoredArc {
            identity: ArcIdentity {
                session_id: session_id.into(),
                pi_id: "conversation-one".into(),
                launch_id: "launch".into(),
                generation: 1,
                leaf: "first".into(),
            },
            at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            arc: WorkArc::from_json(
                r#"{"goal":"Stable goal","now":"Progress","remaining":"Checks","next":"Review"}"#,
            )
            .unwrap(),
            selected_route: "fake/route".into(),
            provider: "fake".into(),
            model: "synthetic".into(),
            usage: None,
        }
    }

    #[test]
    fn missing_foreign_and_redirected_revisions_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let document = ArcDocument::observe(record("session-one"), None, false);
        assert!(read_document(root.path(), "session-one", &document.revision).is_err());
        publish_document(root.path(), &document).unwrap();
        assert!(read_document(root.path(), "../escape", &document.revision).is_err());
        assert!(read_document(root.path(), "session-one", "../escape").is_err());
        let other = root.path().join("work-arcs/session-two");
        std::fs::create_dir(&other).unwrap();
        std::fs::copy(
            root.path()
                .join(revision_path("session-one", &document.revision).unwrap()),
            other.join(format!("{}.json", document.revision)),
        )
        .unwrap();
        assert!(read_document(root.path(), "session-two", &document.revision).is_err());
        std::os::unix::fs::symlink(
            root.path().join("work-arcs/session-one"),
            root.path().join("work-arcs/session-three"),
        )
        .unwrap();
        let mut third = document;
        third.record.identity.session_id = "session-three".into();
        assert!(publish_document(root.path(), &third).is_err());
    }

    #[test]
    fn field_times_preserve_unchanged_values_and_do_not_invent_legacy_dates() {
        let initial = ArcDocument::observe(record("session-one"), None, false);
        let mut changed = initial.record.clone();
        changed.at += Duration::minutes(6);
        changed.identity.leaf = "second".into();
        changed.arc.now = "More progress".into();
        let next = ArcDocument::observe(changed.clone(), Some(&initial), false);
        assert_eq!(next.fields[0], initial.fields[0]);
        assert_eq!(next.fields[1].observed_at, Some(changed.at));
        assert_eq!(next.fields[1].changed_at, Some(changed.at));
        assert_eq!(next.fields[1].source.leaf, "second");
        assert_eq!(next.previous.as_deref(), Some(initial.revision.as_str()));
        let legacy = ArcDocument::observe(record("session-one"), None, true);
        assert!(legacy
            .fields
            .iter()
            .all(|field| field.observed_at.is_none() && field.changed_at.is_none()));
    }

    #[test]
    fn failed_row_commit_leaves_only_an_orphan_not_a_false_latest() {
        let root = tempfile::tempdir().unwrap();
        let mut storage = crate::session::Storage::new_for_test_path(
            "failure",
            root.path().join("sessions.json"),
        );
        let mut session = crate::session::Instance::new("synthetic", "/tmp/synthetic");
        let initial = record(&session.id);
        let nonce = session
            .work_arc
            .reserve(&initial.identity, initial.at)
            .unwrap();
        session.work_arc.finish(&nonce, Some(initial.clone()), None);
        storage
            .update(|rows, _| {
                rows.push(session.clone());
                Ok(())
            })
            .unwrap();
        let pointer = session.work_arc.revision.clone().unwrap();
        storage.set_fail_writes_for_test(true);
        assert!(storage
            .update(|rows, _| {
                let mut later = initial.clone();
                later.identity.leaf = "uncommitted".into();
                later.at += Duration::minutes(6);
                later.arc.now = "Uncommitted".into();
                let nonce = rows[0].work_arc.reserve(&later.identity, later.at).unwrap();
                rows[0].work_arc.finish(&nonce, Some(later), None);
                Ok(())
            })
            .is_err());
        let stored = storage.load().unwrap().remove(0);
        assert_eq!(stored.work_arc.revision.as_deref(), Some(pointer.as_str()));
        assert_eq!(history(root.path(), &stored, 10).unwrap().len(), 1);
    }

    #[test]
    fn concurrent_writers_extend_one_committed_history_with_a_bounded_view() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("sessions.json");
        let storage = crate::session::Storage::new_for_test_path("concurrent-arcs", path.clone());
        let mut session = crate::session::Instance::new("synthetic", "/tmp/synthetic");
        let initial = record(&session.id);
        let nonce = session
            .work_arc
            .reserve(&initial.identity, initial.at)
            .unwrap();
        session.work_arc.finish(&nonce, Some(initial), None);
        storage
            .update(|rows, _| {
                rows.push(session);
                Ok(())
            })
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = ["writer-one", "writer-two"]
            .into_iter()
            .map(|name| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let storage =
                        crate::session::Storage::new_for_test_path("concurrent-arcs", path);
                    barrier.wait();
                    storage
                        .update(|rows, _| {
                            let mut latest = rows[0].work_arc.record.clone().unwrap();
                            latest.at += Duration::minutes(6);
                            latest.identity.leaf = name.into();
                            latest.arc.now = name.into();
                            let nonce = rows[0]
                                .work_arc
                                .reserve(&latest.identity, latest.at)
                                .unwrap();
                            rows[0].work_arc.finish(&nonce, Some(latest), None);
                            Ok(())
                        })
                        .unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let row = storage.load().unwrap().remove(0);
        let audit = history(root.path(), &row, 10).unwrap();
        assert_eq!(audit.len(), 3);
        assert_ne!(audit[0].record.arc.now, audit[1].record.arc.now);
        assert_eq!(audit[0].fields[0], audit[2].fields[0]);
        assert_eq!(history(root.path(), &row, 1).unwrap().len(), 1);
    }
}
