use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

fn binding() -> Binding {
    Binding {
        instance_id: "test-session".into(),
        profile: "default".into(),
        native_id: "native".into(),
        launch_id: uuid::Uuid::new_v4().to_string(),
    }
}
fn policy(binding: Binding) -> Policy {
    Policy {
        version: VERSION,
        binding,
        revision: 1,
        window: uuid::Uuid::new_v4().to_string(),
        generation: uuid::Uuid::new_v4().to_string(),
        enabled: true,
        issued_at_ms: 1000,
        duration_ms: 60000,
        expires_at_ms: Some(61000),
        mode: MODE.into(),
        allowance: 0,
    }
}
fn acknowledgement(p: &Policy, probe: &Probe) -> Ack {
    Ack {
        version: VERSION,
        binding: p.binding.clone(),
        challenge: probe.challenge.clone(),
        generation: p.generation.clone(),
        revision: p.revision,
        window: Some(p.window.clone()),
        expires_at_ms: p.expires_at_ms,
        mode: MODE.into(),
        allowance: 0,
        state: State::ControlOnly,
    }
}

#[test]
fn policy_and_ack_require_exact_current_control_only_authority() {
    let p = policy(binding());
    p.validate().unwrap();
    let probe = Probe {
        version: VERSION,
        binding: p.binding.clone(),
        challenge: uuid::Uuid::new_v4().to_string(),
        revision: p.revision,
    };
    let ack = acknowledgement(&p, &probe);
    assert!(valid_ack(&ack, &probe, Some(&p), 1001));
    assert!(!valid_ack(&ack, &probe, Some(&p), 61000));
    for mutate in [
        (|a: &mut Ack| a.version += 1) as fn(&mut Ack),
        |a| a.allowance = 1,
        |a| a.mode = "autonomous".into(),
        |a| a.binding.instance_id = "other".into(),
        |a| a.binding.profile = "other".into(),
        |a| a.binding.native_id = "child".into(),
        |a| a.binding.launch_id = "other".into(),
        |a| a.generation = uuid::Uuid::new_v4().to_string(),
        |a| a.revision += 1,
        |a| a.challenge = uuid::Uuid::new_v4().to_string(),
        |a| a.window = None,
        |a| a.expires_at_ms = None,
    ] {
        let mut changed = ack.clone();
        mutate(&mut changed);
        assert!(!valid_ack(&changed, &probe, Some(&p), 1001));
    }
    for mutate in [
        (|p: &mut Policy| p.version = 2) as fn(&mut Policy),
        |p| p.allowance = 1,
        |p| p.duration_ms = 0,
        |p| p.expires_at_ms = Some(999),
        |p| p.revision = 0,
    ] {
        let mut changed = p.clone();
        mutate(&mut changed);
        assert!(changed.validate().is_err());
    }
    let mut off = p.clone();
    off.enabled = false;
    off.revision = 2;
    off.expires_at_ms = None;
    off.duration_ms = 0;
    let off_probe = Probe {
        revision: 2,
        ..probe
    };
    assert!(!valid_ack(&ack, &off_probe, Some(&off), 1001));
}

#[test]
fn private_store_refuses_symlinks_modes_nonregular_and_oversized_files() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    symlink(other.path(), root.path().join("afk")).unwrap();
    assert!(Store::open(root.path(), "test", true).is_err());
    std::fs::remove_file(root.path().join("afk")).unwrap();
    let store = Store::open(root.path(), "test", true).unwrap();
    let dir = root.path().join("afk/test");
    for leaf in ["policy.json", "ack.json", "writer.lock"] {
        symlink(other.path().join("victim"), dir.join(leaf)).unwrap();
        assert!(store.read::<serde_json::Value>(leaf).is_err());
        assert!(store.write(leaf, &serde_json::json!({})).is_err());
        if leaf == "writer.lock" {
            assert!(store.lock(Duration::ZERO).is_err());
        }
        std::fs::remove_file(dir.join(leaf)).unwrap();
    }
    std::fs::create_dir(dir.join("policy.json")).unwrap();
    assert!(store.read::<Policy>("policy.json").is_err());
    std::fs::remove_dir(dir.join("policy.json")).unwrap();
    store.write("policy.json", &policy(binding())).unwrap();
    std::fs::set_permissions(
        dir.join("policy.json"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(store.read::<Policy>("policy.json").is_err());
    std::fs::set_permissions(
        dir.join("policy.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    std::fs::write(dir.join("policy.json"), vec![b'x'; MAX_BYTES + 1]).unwrap();
    assert!(store.read::<Policy>("policy.json").is_err());
    assert!(!other.path().join("victim").exists());
}

#[test]
fn concurrent_writers_keep_stable_lock_and_monotone_revision() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "test", true).unwrap();
    let p = policy(binding());
    store.write("policy.json", &p).unwrap();
    let lock = store.lock(TIMEOUT).unwrap();
    let path = root.path().to_path_buf();
    let (started, ready) = std::sync::mpsc::channel();
    let peer = std::thread::spawn(move || {
        let store = Store::open(&path, "test", false).unwrap();
        assert!(store.lock(Duration::ZERO).is_err());
        started.send(()).unwrap();
        let _lock = store.lock(TIMEOUT).unwrap();
        let mut p = store.read::<Policy>("policy.json").unwrap().unwrap();
        p.revision = next_revision(Some(&p)).unwrap();
        store.write("policy.json", &p).unwrap();
    });
    ready.recv_timeout(TIMEOUT).unwrap();
    let mut updated = p;
    updated.revision = 2;
    store.write("policy.json", &updated).unwrap();
    drop(lock);
    peer.join().unwrap();
    assert_eq!(
        store
            .read::<Policy>("policy.json")
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    assert!(root.path().join("afk/test/writer.lock").is_file());
}

#[test]
fn a_fresh_exchange_rejects_the_preexisting_response() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "test", true).unwrap();
    let mut p = policy(binding());
    p.issued_at_ms = now_ms();
    p.expires_at_ms = Some(p.issued_at_ms + 60000);
    let old = Probe {
        version: VERSION,
        binding: p.binding.clone(),
        challenge: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    };
    store.write("ack.json", &acknowledgement(&p, &old)).unwrap();
    let path = root.path().to_owned();
    let peer_policy = p.clone();
    let peer = std::thread::spawn(move || {
        let store = Store::open(&path, "test", false).unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(probe) = store.read::<Probe>("request.json").unwrap() {
                assert_ne!(probe.challenge, old.challenge);
                store
                    .write("ack.json", &acknowledgement(&peer_policy, &probe))
                    .unwrap();
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    });
    let ack = exchange(&store, &p.binding, Some(&p)).unwrap().unwrap();
    peer.join().unwrap();
    assert_eq!(ack.generation, p.generation);
}

#[test]
#[serial_test::serial]
fn unsupported_session_can_record_off_without_being_revived() {
    let _home = super::super::test_support::isolate_app_dir();
    let instance = Instance::new("unsupported", "/tmp");
    let result = control(&instance, Operation::Off).unwrap();
    assert_eq!(result.state, State::Requested);
    assert!(!result.requested_on);
    let stored = Store::open(&super::super::get_app_dir().unwrap(), &instance.id, false).unwrap();
    assert!(
        !stored
            .read::<Policy>("policy.json")
            .unwrap()
            .unwrap()
            .enabled
    );
    assert!(stored.read::<Probe>("request.json").unwrap().is_none());
}

#[test]
#[serial_test::serial]
fn off_replaces_valid_stale_profile_but_corrupt_policy_requires_repair() {
    let _home = super::super::test_support::isolate_app_dir();
    let instance = Instance::new("moved", "/tmp");
    let store = Store::open(&super::super::get_app_dir().unwrap(), &instance.id, true).unwrap();
    let mut old = policy(binding());
    old.binding.instance_id = instance.id.clone();
    old.binding.profile = "previous-profile".into();
    old.revision = 7;
    store.write("policy.json", &old).unwrap();
    let off = control(&instance, Operation::Off).unwrap();
    assert_eq!(off.revision, 8);
    assert!(!off.requested_on);
    let p = store.read::<Policy>("policy.json").unwrap().unwrap();
    assert_eq!(p.binding.profile, instance.effective_profile());
    store
        .write("policy.json", &serde_json::json!({"revision": "broken"}))
        .unwrap();
    let error = control(&instance, Operation::Off).unwrap_err();
    assert!(error.to_string().contains("explicit repair"));
    assert_eq!(
        store
            .read::<serde_json::Value>("policy.json")
            .unwrap()
            .unwrap()["revision"],
        "broken"
    );
}

#[test]
#[serial_test::serial]
fn native_and_launch_replacement_cannot_reuse_a_dialog_snapshot() {
    use crate::session::{ActiveExecution, ExecutionBinding, Status};
    let _home = super::super::test_support::isolate_app_dir();
    let mut original = Instance::new("pi", "/tmp");
    original.tool = "pi".into();
    original.status = Status::Running;
    original.source_profile = "afk-test".into();
    original.agent_session_id = Some(uuid::Uuid::new_v4().to_string());
    original.active_execution = Some(ActiveExecution {
        launch_id: uuid::Uuid::new_v4().to_string(),
        capture: None,
        container: None,
        binding: ExecutionBinding {
            agent: "pi".into(),
            stores: vec![],
            configuration: vec![],
            cwd: "/tmp".into(),
            cwd_filesystem: "host".into(),
            filesystem: "host".into(),
            exported_default_store: None,
        },
    });
    let storage = Storage::new_unwatched("afk-test").unwrap();
    for native_changed in [true, false] {
        let mut replaced = original.clone();
        if native_changed {
            replaced.agent_session_id = Some(uuid::Uuid::new_v4().to_string());
        } else {
            replaced.active_execution.as_mut().unwrap().launch_id =
                uuid::Uuid::new_v4().to_string();
        }
        storage
            .update(|rows, _| {
                *rows = vec![replaced];
                Ok(())
            })
            .unwrap();
        for operation in [Operation::On { minutes: 1 }, Operation::Status] {
            let result = control(&original, operation).unwrap();
            assert_eq!(result.state, State::Unsupported);
        }
        let off = control(&original, Operation::Off).unwrap();
        assert!(!off.requested_on);
        assert_eq!(off.state, State::Requested);
    }
}

#[test]
#[serial_test::serial]
fn bootstrap_is_explicit_argv_bound_and_independent_of_usage_environment() {
    let _home = super::super::test_support::isolate_app_dir();
    let instance = Instance::new("pi", "/tmp");
    let native = uuid::Uuid::new_v4().to_string();
    let launch = uuid::Uuid::new_v4().to_string();
    let args = shell_words::split(&launch_arguments(&instance, &native, &launch).unwrap()).unwrap();
    assert_eq!(args[0], "-e");
    assert_eq!(args[2], "--aoe-afk-binding");
    let boot: Bootstrap = serde_json::from_str(&args[3]).unwrap();
    assert_eq!(boot.binding.native_id, native);
    assert_eq!(boot.binding.launch_id, launch);
    assert_eq!(boot.binding.instance_id, instance.id);
    assert!(std::path::Path::new(&args[1]).is_file());
}
