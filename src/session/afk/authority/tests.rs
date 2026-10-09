use super::*;
use std::sync::{Arc, Barrier};

#[test]
fn sibling_writers_share_one_lock_and_identical_ids_in_other_profiles_are_isolated() {
    let app = tempfile::tempdir().unwrap();
    migration::initialize_test(app.path());
    for id in ["one", "two"] {
        Store::authority(app.path(), "alpha", id, true).unwrap();
    }
    Store::authority(app.path(), "beta", "one", true).unwrap();
    let start = Arc::new(Barrier::new(3));
    let peers: Vec<_> = ["one", "two"]
        .into_iter()
        .map(|id| {
            let app = app.path().to_owned();
            let start = start.clone();
            std::thread::spawn(move || {
                let store = Store::authority(&app, "alpha", id, false).unwrap();
                start.wait();
                for _ in 0..12 {
                    let _lock = store.lock(TIMEOUT).unwrap();
                    let mut session = store.session().unwrap();
                    session.revoke().unwrap();
                    store.publish_session(session).unwrap();
                }
            })
        })
        .collect();
    start.wait();
    for peer in peers {
        peer.join().unwrap();
    }
    for id in ["one", "two"] {
        assert_eq!(
            Store::authority(app.path(), "alpha", id, false)
                .unwrap()
                .session()
                .unwrap()
                .epoch,
            12
        );
    }
    assert_eq!(
        Store::authority(app.path(), "beta", "one", false)
            .unwrap()
            .session()
            .unwrap()
            .epoch,
        0
    );
    assert_eq!(
        std::fs::read_dir(app.path().join("afk")).unwrap().count(),
        0
    );
}

#[test]
fn profile_quota_and_publication_failure_preserve_previous_snapshot() {
    let app = tempfile::tempdir().unwrap();
    migration::initialize_test(app.path());
    let store = Store::authority(app.path(), "alpha", "one", true).unwrap();
    let before = store.bytes("snapshot.json").unwrap().unwrap();
    let _lock = store.lock(TIMEOUT).unwrap();
    let mut session = store.session().unwrap();
    session.evidence.push(Evidence {
        namespace: "afk-runtime-v2".into(),
        instance: "one".into(),
        version: 2,
        sha256: "a".repeat(64),
        original: "x".repeat(PROFILE_BYTES),
        profiles: vec!["alpha".into()],
    });
    assert!(store.publish_session(session).is_err());
    assert_eq!(store.bytes("snapshot.json").unwrap().unwrap(), before);
    let mut session = store.session().unwrap();
    session.revoke().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        store.dir.path().join("snapshot.json"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(store.publish_session(session).is_err());
    std::fs::set_permissions(
        store.dir.path().join("snapshot.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(store.bytes("snapshot.json").unwrap().unwrap(), before);
}

#[test]
fn fresh_creation_requires_readiness_and_exact_scope() {
    let app = tempfile::tempdir().unwrap();
    assert!(Store::authority(app.path(), "alpha", "one", true).is_err());
    assert_eq!(std::fs::read_dir(app.path()).unwrap().count(), 0);
    migration::initialize_test(app.path());
    for (profile, id) in [("../outside", "one"), ("", "one"), ("alpha", "../two")] {
        assert!(Store::authority(app.path(), profile, id, true).is_err());
    }
    let store = Store::authority(app.path(), "alpha", "one", true).unwrap();
    let mut policy = super::super::tests::policy(super::super::tests::binding());
    policy.binding.profile = "beta".into();
    assert!(store.write("policy.json", &policy).is_err());
    assert!(store.session().unwrap().policy.is_none());
}
