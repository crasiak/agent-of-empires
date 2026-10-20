use super::*;
use std::cell::{Cell, RefCell};
use std::io::{self, BufRead, Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::rc::Rc;

#[derive(Clone)]
struct Reply(Rc<RefCell<ChildStdin>>);
impl Write for Reply {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

struct Packets<R, F> {
    input: R,
    fixture: F,
    bytes: Vec<u8>,
    offset: usize,
}
impl<R: BufRead, F: FnMut(&Value) -> Result<bool>> BufRead for Packets<R, F> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.offset == self.bytes.len() {
            let mut line = String::new();
            if self.input.read_line(&mut line)? == 0 {
                return Ok(&[]);
            }
            let value: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
            if value.get("result").is_some() || value["packet"].get("fixture").is_some() {
                if !(self.fixture)(&value).map_err(io::Error::other)? {
                    return Ok(&[]);
                }
                continue;
            }
            self.bytes = serde_json::to_vec(&value["packet"]).map_err(io::Error::other)?;
            self.bytes.push(b'\n');
            self.offset = 0;
        }
        Ok(&self.bytes[self.offset..])
    }
    fn consume(&mut self, count: usize) {
        self.offset += count;
    }
}
impl<R: BufRead, F: FnMut(&Value) -> Result<bool>> Read for Packets<R, F> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let bytes = self.fill_buf()?;
        let count = bytes.len().min(output.len());
        output[..count].copy_from_slice(&bytes[..count]);
        self.consume(count);
        Ok(count)
    }
}

#[test]
fn persisted_sdk_completion_uses_one_continuous_host_bridge() {
    let Some(source) = std::env::var_os("AOE_PI_SOURCE_ROOT") else {
        eprintln!("Set AOE_PI_SOURCE_ROOT to qualify the actual terminal SDK/host boundary");
        return;
    };
    let source = std::path::PathBuf::from(source);
    let mut failures = Vec::new();
    for scenario in [
        "poll-before-tool",
        "turn-before-poll",
        "late-before-final",
        "late-after-final",
        "host-missing",
        "host-negative",
        "host-lost",
        "pagination-complete",
        "pagination-lost",
        "pagination-obsolete",
    ] {
        let (app, root, store, mut window) = setup(2);
        Book {
            version: 4,
            windows: vec![],
        }
        .save(&store)
        .unwrap();
        let transport = Store::transport(app.path(), "default", "test", true).unwrap();
        let mut child = Command::new("node")
            .arg("--import")
            .arg(source.join("node_modules/tsx/dist/loader.mjs"))
            .env("TSX_TSCONFIG_PATH", source.join("tsconfig.json"))
            .args(["assets/session/aoe-afk-completion-fixture.mjs", scenario])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut reply = Reply(Rc::new(RefCell::new(child.stdin.take().unwrap())));
        let mut input = io::BufReader::new(child.stdout.take().unwrap());
        let mut hello = String::new();
        input.read_line(&mut hello).unwrap();
        let hello: Value = serde_json::from_str(&hello).unwrap();
        window.binding = serde_json::from_value(hello["binding"].clone()).unwrap();
        window.generation = hello["generation"].as_str().unwrap().into();
        window.issued_at_ms = now_ms();
        window.expires_at_ms = window.issued_at_ms + 60_000;
        let completed = Cell::new(false);
        let output = Reply(Rc::clone(&reply.0));
        let packets = Packets {
            input,
            bytes: vec![],
            offset: 0,
            fixture: |value: &Value| -> Result<bool> {
                if let Some(result) = value.get("result") {
                    ensure!(
                        result["passed"] == true && result["calls"] == 3,
                        "bad completion result"
                    );
                    completed.set(true);
                    return Ok(false);
                }
                let output = match value["packet"]["fixture"].as_str().unwrap() {
                    "enroll" => {
                        let mut tx = authority::SessionTransaction::open(&store)?;
                        let mut next = tx.snapshot().clone();
                        next.sessions
                            .get_mut("test")
                            .unwrap()
                            .book
                            .windows
                            .push(window.clone());
                        next.usage.enroll(app.path(), &window.id, &window.binding, &window.generation,
                            Some(serde_json::from_value(json!({"version":1,"basis":"observed-native-entries","tokens":100,"usd":null}))?), true)?;
                        tx.publish(next)?.warn();
                        drop(tx);
                        let view = snapshot(&store)?;
                        transport.write(
                            "runtime-probe.json",
                            &RuntimeProbe {
                                version: super::super::super::BRIDGE_VERSION,
                                binding: window.binding.clone(),
                                challenge: uuid::Uuid::new_v4().to_string(),
                                window: Some(window.id.clone()),
                                grant_hash: Some(hash(serde_json::to_vec(&window.grant)?)),
                                budgeted: false,
                                usage_policy_hash: view["usage_policy_hash"]
                                    .as_str()
                                    .map(str::to_owned),
                            },
                        )?;
                        Value::Null
                    }
                    "inspect" => snapshot(&store)?,
                    "negative" => {
                        let mut tx = authority::SessionTransaction::open(&store)?;
                        let mut next = tx.snapshot().clone();
                        next.usage.windows.get_mut(&window.id).unwrap().gap =
                            Some(crate::session::afk::usage::Gap::LateWork);
                        tx.publish(next)?.warn();
                        Value::Null
                    }
                    _ => anyhow::bail!("unknown completion fixture operation"),
                };
                serde_json::to_writer(&mut reply, &json!({"ok":true,"value":output}))?;
                reply.write_all(b"\n")?;
                reply.flush()?;
                Ok(true)
            },
        };
        let served = super::super::super::bridge::serve(
            &transport,
            Some((&store, app.path())),
            &window.binding,
            &window.generation,
            packets,
            output,
            || Ok(()),
        );
        drop(reply);
        let status = child.wait().unwrap();
        if served.is_err() || !status.success() || !completed.get() {
            failures.push(format!(
                "{scenario}: {served:?}; {status}; completed={}",
                completed.get()
            ));
            continue;
        }
        assert_eq!(
            std::fs::read_to_string(root.path().join("scratch.txt")).unwrap(),
            "hello\n"
        );
        assert_eq!(Book::load(&store).unwrap().windows[0].reservations.len(), 2);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
