use super::*;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    app: tempfile::TempDir,
    runtime: Store,
    binding: Binding,
    generation: String,
    boot: String,
}
impl Fixture {
    fn new() -> Self {
        let app = tempfile::tempdir().unwrap();
        authority::migration::initialize_test(app.path());
        let runtime = Store::authority(app.path(), "alpha", "wake-test", true).unwrap();
        Self {
            app,
            runtime,
            binding: Binding {
                profile: "alpha".into(),
                instance_id: "wake-test".into(),
                native_id: uuid::Uuid::new_v4().to_string(),
                launch_id: uuid::Uuid::new_v4().to_string(),
            },
            generation: uuid::Uuid::new_v4().to_string(),
            boot: uuid::Uuid::new_v4().to_string(),
        }
    }
    fn peer(&self) -> Peer<'_> {
        Peer {
            app: self.app.path(),
            runtime: &self.runtime,
            binding: &self.binding,
            generation: &self.generation,
        }
    }
    fn clock(&self, millis: u64) -> Clock {
        Clock {
            boot: self.boot.clone(),
            millis,
        }
    }
    fn observe(&self, server: &mut Server, ready: bool, millis: u64) -> Value {
        server.observe_with(
            self.peer(),
            Some(Capability {
                version: 1,
                delivery: CAPABILITY.into(),
                ready,
            }),
            || Some(self.clock(millis)),
            || Ok(()),
        )
    }
    fn slot(&self) -> Store {
        slot(self.app.path(), &self.binding, false)
            .unwrap()
            .unwrap()
    }
    fn post(&self, clear: bool) {
        let slot = self.slot();
        let _lock = slot.lock_existing(TIMEOUT).unwrap();
        let receiver = slot.read::<Receiver>("receiver.json").unwrap().unwrap();
        publish_request(
            &slot,
            &self.binding,
            &receiver,
            "configured\nwake",
            clear,
            self.clock(1000),
            || Ok(()),
        )
        .unwrap();
    }
}

#[test]
fn wake_claim_is_terminal_scoped_and_does_not_mutate_authority() {
    for (at_post, at_claim, host_clear, millis, expected) in [
        (true, true, true, 1100, "attempted"),
        (false, true, true, 1100, "suppressed"),
        (true, false, true, 1100, "suppressed"),
        (true, true, false, 1100, "suppressed"),
        (true, true, true, 31000, "suppressed"),
    ] {
        let f = Fixture::new();
        let mut server = Server::new();
        let before = f.runtime.bytes("snapshot.json").unwrap();
        assert_eq!(f.observe(&mut server, at_post, 1000)["state"], "empty");
        f.post(host_clear);
        let reply = f.observe(&mut server, at_claim, millis);
        assert_eq!(reply["state"], expected);
        assert_eq!(reply.get("text").is_some(), expected == "attempted");
        let slot = f.slot();
        let terminal = slot.bytes("request.json").unwrap();
        let next = f.observe(&mut server, true, millis + 1);
        assert!(next.get("text").is_none());
        assert_eq!(slot.bytes("request.json").unwrap(), terminal);
        assert_eq!(f.runtime.bytes("snapshot.json").unwrap(), before);
        let receiver = slot.read::<Receiver>("receiver.json").unwrap().unwrap();
        assert!(publish_request(
            &slot,
            &f.binding,
            &receiver,
            "different text",
            true,
            f.clock(millis + 1),
            || Ok(())
        )
        .is_err());
        let mut sid_changed = f.binding.clone();
        sid_changed.native_id = uuid::Uuid::new_v4().to_string();
        assert!(publish_request(
            &slot,
            &sid_changed,
            &receiver,
            "different SID",
            true,
            f.clock(millis + 1),
            || Ok(())
        )
        .is_err());
    }
}

#[test]
fn failed_terminal_publication_cannot_be_reconsidered_or_retargeted() {
    for ready in [false, true] {
        for after_rename in [false, true] {
            let f = Fixture::new();
            let mut server = Server::new();
            f.observe(&mut server, true, 1000);
            f.post(true);
            let slot = f.slot();
            let path = slot.dir.path().to_owned();
            let validations = Cell::new(0);
            let value = server.observe_with(
                f.peer(),
                Some(Capability {
                    version: 1,
                    delivery: CAPABILITY.into(),
                    ready,
                }),
                || Some(f.clock(1100)),
                || {
                    validations.set(validations.get() + 1);
                    if validations.get() == 3 {
                        if after_rename {
                            crate::session::anchored_fs::FAIL_SYNC_ONCE.set(Some(path.clone()));
                        } else {
                            std::fs::set_permissions(
                                &path,
                                std::fs::Permissions::from_mode(0o500),
                            )?;
                        }
                    }
                    Ok(())
                },
            );
            crate::session::anchored_fs::FAIL_SYNC_ONCE.set(None);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(value["state"], "unavailable");
            assert!(value.get("text").is_none());
            let bytes = slot.bytes("request.json").unwrap();
            assert_eq!(f.observe(&mut server, true, 1200)["state"], "unavailable");
            assert_eq!(slot.bytes("request.json").unwrap(), bytes);
            let mut replacement = Server::new();
            assert_ne!(replacement.incarnation, server.incarnation);
            let value = f.observe(&mut replacement, true, 1200);
            assert_eq!(value["state"], "unmatched");
            assert!(value.get("text").is_none());
            assert_eq!(slot.bytes("request.json").unwrap(), bytes);
        }
    }
}

#[test]
fn wake_rechecks_presence_binding_boot_and_publication_veto() {
    for mode in [
        "presence",
        "binding",
        "boot",
        "journal",
        "capability",
        "preparation",
    ] {
        let f = Fixture::new();
        let mut server = Server::new();
        f.observe(&mut server, true, 1000);
        f.post(true);
        let slot = f.slot();
        let before = slot.bytes("request.json").unwrap();
        if mode == "presence" {
            let _lock = f.runtime.lock_existing(TIMEOUT).unwrap();
            let mut s = f.runtime.session().unwrap();
            s.policy = Some(Policy {
                version: 1,
                binding: f.binding.clone(),
                revision: 1,
                window: uuid::Uuid::new_v4().to_string(),
                generation: f.generation.clone(),
                enabled: true,
                expires_at_ms: Some(61000),
                issued_at_ms: 1000,
                duration_ms: 60000,
                mode: MODE.into(),
                allowance: 0,
            });
            f.runtime.publish_session(s).unwrap();
        }
        if mode == "journal" {
            f.runtime
                .write_raw(
                    "publication.pending.json",
                    &json!({"pending":"unqualified"}),
                )
                .unwrap();
        }
        if mode == "preparation" {
            std::fs::remove_file(
                f.app
                    .path()
                    .join(authority::NAMESPACE)
                    .join(ROOT)
                    .join("version.json"),
            )
            .unwrap();
        }
        let n = Cell::new(0);
        let response = server.observe_with(
            f.peer(),
            (mode != "capability").then(|| Capability {
                version: 1,
                delivery: CAPABILITY.into(),
                ready: true,
            }),
            || {
                let mut c = f.clock(1100);
                if mode == "boot" {
                    c.boot = uuid::Uuid::new_v4().to_string();
                }
                Some(c)
            },
            || {
                n.set(n.get() + 1);
                if mode == "binding" && n.get() == 3 {
                    anyhow::bail!("replacement launch");
                }
                Ok(())
            },
        );
        assert!(response.get("text").is_none(), "{mode}");
        if ["presence", "boot"].contains(&mode) {
            assert_eq!(response["state"], "suppressed");
        } else {
            assert_eq!(slot.bytes("request.json").unwrap(), before);
        }
        if mode == "preparation" {
            assert!(!f
                .app
                .path()
                .join(authority::NAMESPACE)
                .join(ROOT)
                .join("version.json")
                .exists());
        }
    }
}

#[test]
fn bridge_failure_before_wake_observation_permanently_refuses_pending_delivery() {
    struct Output<'a> {
        bytes: Vec<u8>,
        first: Option<Box<dyn FnOnce() + 'a>>,
    }
    impl std::io::Write for Output<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if let Some(first) = self.first.take() {
                first();
            }
            Ok(())
        }
    }
    let f = Fixture::new();
    let fail = Cell::new(false);
    let mut output = Output {
        bytes: vec![],
        first: Some(Box::new(|| {
            let slot = f.slot();
            let _lock = slot.lock_existing(TIMEOUT).unwrap();
            let receiver = slot.read::<Receiver>("receiver.json").unwrap().unwrap();
            publish_request(
                &slot,
                &f.binding,
                &receiver,
                "pending wake",
                true,
                receiver.clock.clone(),
                || Ok(()),
            )
            .unwrap();
            fail.set(true);
        })),
    };
    let packet=b"{\"op\":\"poll\",\"wake\":{\"version\":1,\"delivery\":\"custom-message-v1\",\"ready\":true}}\n";
    super::super::bridge::serve(
        &Store::transport(
            f.app.path(),
            &f.binding.profile,
            &f.binding.instance_id,
            true,
        )
        .unwrap(),
        Some((&f.runtime, f.app.path())),
        &f.binding,
        &f.generation,
        std::io::Cursor::new(packet.repeat(3)),
        &mut output,
        || {
            if fail.replace(false) {
                anyhow::bail!("transient validation failure");
            }
            Ok(())
        },
    )
    .unwrap();
    let replies = output
        .bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|line| serde_json::from_slice::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[1]["ok"], false);
    assert_eq!(
        replies[2]["value"]["automatic_wake"]["state"], "unavailable",
        "earlier bridge failure must not let the same receiver reconsider pending wake"
    );
    assert!(replies[2]["value"]["automatic_wake"].get("text").is_none());
}

#[test]
fn only_the_target_helper_can_claim_under_contention() {
    let f = Fixture::new();
    let mut target = Server::new();
    f.observe(&mut target, true, 1000);
    f.post(true);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let root = f.app.path().to_owned();
    let binding = f.binding.clone();
    let generation = f.generation.clone();
    let now = f.clock(1100);
    let workers = [target, Server::new()]
        .into_iter()
        .map(|mut server| {
            let barrier = barrier.clone();
            let root = root.clone();
            let binding = binding.clone();
            let generation = generation.clone();
            let now = now.clone();
            std::thread::spawn(move || {
                let runtime =
                    Store::authority(&root, &binding.profile, &binding.instance_id, false).unwrap();
                barrier.wait();
                server.observe_with(
                    Peer {
                        app: &root,
                        runtime: &runtime,
                        binding: &binding,
                        generation: &generation,
                    },
                    Some(Capability {
                        version: 1,
                        delivery: CAPABILITY.into(),
                        ready: true,
                    }),
                    || Some(now.clone()),
                    || Ok(()),
                )
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let replies = workers
        .into_iter()
        .map(|w| w.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        replies.iter().filter(|v| v.get("text").is_some()).count(),
        1
    );
    assert_eq!(
        f.slot()
            .read::<Request>("request.json")
            .unwrap()
            .unwrap()
            .state,
        State::Attempted
    );
}

#[test]
fn new_launch_replaces_one_slot_but_stale_receiver_cannot_consume_it() {
    let mut f = Fixture::new();
    let mut old = Server::new();
    f.observe(&mut old, true, 1000);
    f.post(true);
    let old_binding = f.binding.clone();
    f.binding.launch_id = uuid::Uuid::new_v4().to_string();
    f.binding.native_id = uuid::Uuid::new_v4().to_string();
    f.generation = uuid::Uuid::new_v4().to_string();
    let mut new = Server::new();
    assert_eq!(f.observe(&mut new, true, 1000)["state"], "unmatched");
    f.post(true);
    let slot = f.slot();
    let before = slot.bytes("request.json").unwrap();
    let reply = old.observe_with(
        Peer {
            app: f.app.path(),
            runtime: &f.runtime,
            binding: &old_binding,
            generation: &f.generation,
        },
        Some(Capability {
            version: 1,
            delivery: CAPABILITY.into(),
            ready: true,
        }),
        || Some(f.clock(1100)),
        || Ok(()),
    );
    assert_eq!(reply["state"], "unmatched");
    assert_eq!(slot.bytes("request.json").unwrap(), before);
    assert!(f.observe(&mut new, true, 1100).get("text").is_some());
}
