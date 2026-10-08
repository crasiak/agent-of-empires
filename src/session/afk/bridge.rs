use super::*;
use std::io::{BufRead, Write};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Poll,
    Runtime { command: delegation::Request },
    RuntimeAck { ack: delegation::RuntimeAck },
    Ack { ack: Box<Ack> },
}

pub fn run_bridge(raw: &str, generation: &str) -> Result<()> {
    ensure!(
        raw.len() <= MAX_BYTES && uuid::Uuid::parse_str(generation).is_ok(),
        "invalid AFK bridge bootstrap"
    );
    let bootstrap: Bootstrap = serde_json::from_str(raw)?;
    ensure!(
        bootstrap.version == BRIDGE_VERSION && bootstrap.app_dir == super::super::get_app_dir()?,
        "AFK bootstrap/app namespace mismatch"
    );
    let store = Store::open(&bootstrap.app_dir, &bootstrap.binding.instance_id, false)?;
    let runtime = Store::runtime(&bootstrap.app_dir, &bootstrap.binding.instance_id, false)?;
    serve(
        &store,
        Some(&runtime),
        &bootstrap.binding,
        generation,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        || current_instance(&bootstrap.binding).map(|_| ()),
    )
}

pub(super) fn serve(
    store: &Store,
    runtime: Option<&Store>,
    binding: &Binding,
    generation: &str,
    mut input: impl BufRead,
    mut output: impl Write,
    validate: impl Fn() -> Result<()>,
) -> Result<()> {
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
        let result = (|| -> Result<serde_json::Value> {
            validate()?;
            let command: Command = serde_json::from_slice(&line)?;
            let policy = store.read::<Policy>("policy.json")?;
            if let Some(p) = &policy {
                p.validate()?;
            }
            let probe = store.read::<Probe>("request.json")?.filter(|r| {
                r.version == VERSION
                    && r.binding == *binding
                    && r.revision == policy.as_ref().map_or(0, |p| p.revision)
            });
            match command {
                Command::Poll => Ok(serde_json::json!({"policy":policy,"probe":probe,
                    "delegation":runtime.map(|s| delegation::settlement::reconcile(s, binding, generation, now_ms())).transpose()?,
                    "runtime_probe":store.read::<delegation::RuntimeProbe>("runtime-probe.json")?})),
                Command::Runtime { command } => delegation::execute_checked(
                    runtime.context("runtime unavailable")?,
                    binding,
                    generation,
                    command,
                    now_ms(),
                    &validate,
                ),
                Command::RuntimeAck { ack } => {
                    delegation::acknowledge(store, binding, generation, ack)?;
                    Ok(serde_json::json!({"acknowledged":true}))
                }
                Command::Ack { ack } => {
                    let probe = probe.context("no current matching probe")?;
                    ensure!(
                        ack.generation == generation
                            && valid_ack(&ack, &probe, policy.as_ref(), now_ms()),
                        "stale or invalid acknowledgement"
                    );
                    store.write("ack.json", &ack)?;
                    Ok(serde_json::json!({"acknowledged":true}))
                }
            }
        })();
        let value = match result {
            Ok(value) => serde_json::json!({"ok":true,"value":value}),
            Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
        };
        serde_json::to_writer(&mut output, &value)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
