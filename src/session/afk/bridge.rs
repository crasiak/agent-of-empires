use super::*;
use std::io::{BufRead, Write};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Poll {
        #[serde(default)]
        usage: Option<usage::Observation>,
        #[serde(default)]
        wake: Option<wake::Capability>,
    },
    UsageRecover {
        window: String,
    },
    QuestionRecover {
        window: String,
        identity: delegation::questions::QuestionIdentity,
    },
    Runtime {
        command: delegation::Request,
    },
    RuntimeAck {
        ack: delegation::RuntimeAck,
    },
    Ack {
        ack: Box<Ack>,
    },
}

pub fn run_bridge(raw: &str, generation: &str) -> Result<()> {
    ensure!(
        raw.len() <= MAX_BYTES && uuid::Uuid::parse_str(generation).is_ok(),
        "invalid AFK bridge bootstrap"
    );
    let bootstrap: Bootstrap = serde_json::from_str(raw)?;
    ensure!(
        bootstrap.version == BRIDGE_VERSION
            && bootstrap.app_dir == super::super::get_app_dir_path()?,
        "AFK bootstrap/app namespace mismatch"
    );
    authority::ready(&bootstrap.app_dir)?;
    let runtime = Store::authority(
        &bootstrap.app_dir,
        &bootstrap.binding.profile,
        &bootstrap.binding.instance_id,
        false,
    ).context("AFK authority unavailable; run normal `aoe migrate` in this app namespace and inspect missing/corrupt imports")?;
    current_instance(&bootstrap.binding)?;
    let store = Store::transport(
        &bootstrap.app_dir,
        &bootstrap.binding.profile,
        &bootstrap.binding.instance_id,
        true,
    )?;
    serve(
        &store,
        Some((&runtime, &bootstrap.app_dir)),
        &bootstrap.binding,
        generation,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        || current_instance(&bootstrap.binding).map(|_| ()),
    )
}

pub(super) fn serve(
    store: &Store,
    runtime: Option<(&Store, &std::path::Path)>,
    binding: &Binding,
    generation: &str,
    mut input: impl BufRead,
    mut output: impl Write,
    validate: impl Fn() -> Result<()>,
) -> Result<()> {
    let (runtime, app) = runtime.map_or((None, None), |(store, app)| (Some(store), Some(app)));
    let mut meter = usage::transport::Server::new();
    let mut automatic_wake = wake::Server::new();
    loop {
        let mut line = Vec::new();
        use std::io::Read;
        let n = input
            .by_ref()
            .take((delegation::FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        ensure!(
            n <= delegation::FRAME_BYTES && line.last() == Some(&b'\n'),
            "AFK bridge packet exceeds bounds or is incomplete"
        );
        let (result, warnings) = authority::publication_context(|| -> Result<serde_json::Value> {
            validate()?;
            let command: Command = serde_json::from_slice(&line)?;
            match command {
                Command::UsageRecover { window } => usage::recover(
                    runtime.context("runtime unavailable")?,
                    binding,
                    &window,
                    &validate,
                ),
                Command::QuestionRecover { window, identity } => delegation::questions::recover(
                    runtime.context("runtime unavailable")?,
                    binding,
                    &window,
                    &identity,
                    &validate,
                ),
                Command::Poll { usage, wake } => {
                    let usage_report = meter.observe(
                        store,
                        runtime.context("runtime unavailable")?,
                        binding,
                        generation,
                        usage,
                        &validate,
                    )?;
                    let mut view = delegation::poll(
                        runtime.context("runtime unavailable")?,
                        binding,
                        generation,
                        now_ms(),
                        &validate,
                    )?;
                    view["usage_report"] = usage_report;
                    if let (Some(app), Some(runtime)) = (app, runtime) {
                        view["automatic_wake"] = automatic_wake.observe(
                            wake::Peer {
                                app,
                                runtime,
                                binding,
                                generation,
                            },
                            wake,
                            &validate,
                        );
                    }
                    let revision = view["policy"]["revision"].as_u64().unwrap_or(0);
                    view["probe"] =
                        serde_json::to_value(matching_probe(store, binding, revision)?)?;
                    view["runtime_probe"] = serde_json::to_value(
                        store.read::<delegation::RuntimeProbe>("runtime-probe.json")?,
                    )?;
                    Ok(view)
                }
                Command::Runtime { command } => delegation::execute_checked(
                    runtime.context("runtime unavailable")?,
                    binding,
                    generation,
                    command,
                    now_ms(),
                    &validate,
                ),
                Command::RuntimeAck { ack } => {
                    let runtime = runtime.context("runtime unavailable")?;
                    let _lock = runtime.lock_existing(TIMEOUT)?;
                    validate()?;
                    runtime.session()?;
                    delegation::acknowledge(store, binding, generation, ack)?;
                    Ok(serde_json::json!({"acknowledged":true}))
                }
                Command::Ack { ack } => {
                    let runtime = runtime.context("runtime unavailable")?;
                    let _lock = runtime.lock_existing(TIMEOUT)?;
                    validate()?;
                    let policy = runtime.session()?.policy;
                    if let Some(policy) = &policy {
                        policy.validate()?;
                    }
                    let probe =
                        matching_probe(store, binding, policy.as_ref().map_or(0, |p| p.revision))?
                            .context("no current matching probe")?;
                    ensure!(
                        ack.generation == generation
                            && valid_ack(&ack, &probe, policy.as_ref(), now_ms()),
                        "stale or invalid acknowledgement"
                    );
                    store.write("ack.json", &ack)?;
                    Ok(serde_json::json!({"acknowledged":true}))
                }
            }
        });
        let value = match result {
            Ok(mut value) => {
                if !warnings.is_empty() {
                    if let Some(object) = value.as_object_mut() {
                        object.insert(
                            "publication_warnings".into(),
                            serde_json::to_value(warnings)?,
                        );
                    }
                }
                serde_json::json!({"ok":true,"value":value})
            }
            Err(error) => {
                automatic_wake.park();
                serde_json::json!({"ok":false,"code":if error.is::<batch::state::SharedExhausted>() {Some("shared_budget_exhausted")}else if let Some(denied)=error.downcast_ref::<usage::Denied>() {Some(match denied.0 {usage::Denial::Exhausted=>"usage_exhausted",usage::Denial::Stale=>"usage_stale",usage::Denial::Unavailable=>"usage_unavailable"})}else{None},"error":error.to_string()})
            }
        };
        serde_json::to_writer(&mut output, &value)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

fn matching_probe(store: &Store, binding: &Binding, revision: u64) -> Result<Option<Probe>> {
    Ok(store
        .read::<Probe>("request.json")?
        .filter(|p| p.version == VERSION && p.binding == *binding && p.revision == revision))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[serial_test::serial]
    fn early_helper_readiness_refuses_without_frames_or_files() {
        let _home = crate::session::test_support::isolate_app_dir();
        let app = crate::session::get_app_dir().unwrap();
        let bootstrap = Bootstrap {
            version: BRIDGE_VERSION,
            app_dir: app.clone(),
            aoe_bin: "/worktree/aoe".into(),
            wake_clock: None,
            binding: Binding {
                instance_id: "test".into(),
                profile: "alpha".into(),
                native_id: "native".into(),
                launch_id: "launch".into(),
            },
        };
        let raw = serde_json::to_string(&bootstrap).unwrap();
        authority::migration::initialize_test(&app);
        for state in ["pre-authority", "partial", "corrupt", "missing-import"] {
            std::fs::write(
                app.join(".schema_version"),
                if state == "pre-authority" {
                    super::super::schema::QUESTIONS
                } else {
                    super::super::schema::USAGE
                }
                .to_string(),
            )
            .unwrap();
            let path = app.join("afk-authority-v3/migration/manifest.json");
            let original = std::fs::read(&path).unwrap();
            if state == "partial" {
                let mut m: serde_json::Value = serde_json::from_slice(&original).unwrap();
                m["phase"] = serde_json::json!("imports");
                std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
            } else if state == "corrupt" {
                std::fs::write(&path, b"{}").unwrap();
            }
            let before = super::super::overview::tests::tree(&app);
            let error = run_bridge(&raw, &uuid::Uuid::new_v4().to_string()).unwrap_err();
            assert_eq!(
                before,
                super::super::overview::tests::tree(&app),
                "{state}: {error:#}"
            );
            assert!(!app.join("afk-authority-v3/profiles").exists());
            std::fs::write(&path, &original).unwrap();
            std::fs::write(
                app.join(".schema_version"),
                super::super::schema::USAGE.to_string(),
            )
            .unwrap();
        }
    }
    #[test]
    fn bridge_bounds_errors_and_eof_do_not_publish() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        let binding = Binding {
            instance_id: "test".into(),
            profile: "default".into(),
            native_id: "native".into(),
            launch_id: "launch".into(),
        };
        for packet in [
            b"{\"op\":\"poll\"}\n".to_vec(),
            b"invalid\n".to_vec(),
            b"{\"op\":\"ack\"}\n".to_vec(),
        ] {
            let mut out = Vec::new();
            serve(
                &store,
                None,
                &binding,
                "generation",
                std::io::Cursor::new(packet),
                &mut out,
                || anyhow::bail!("invalid current binding"),
            )
            .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&out).unwrap()["ok"],
                false
            );
            assert!(store.read::<Ack>("ack.json").unwrap().is_none());
        }
        let mut out = Vec::new();
        serve(
            &store,
            None,
            &binding,
            "generation",
            std::io::Cursor::new(Vec::<u8>::new()),
            &mut out,
            || Ok(()),
        )
        .unwrap();
        assert!(out.is_empty());
        assert!(serve(
            &store,
            None,
            &binding,
            "generation",
            std::io::Cursor::new(vec![b'x'; delegation::FRAME_BYTES + 1]),
            &mut out,
            || Ok(())
        )
        .is_err());
    }
}
