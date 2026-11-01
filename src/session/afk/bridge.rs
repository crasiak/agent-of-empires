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
    WorkPeriodReply {
        reply: work_period::Inspection,
    },
    Ack {
        ack: Box<Ack>,
    },
    Recognise {
        source: answer::Source,
        text: String,
    },
    Decide {
        run: answer::run::RunInput,
        #[serde(default)]
        menu: Option<answer::MenuInput>,
        #[serde(default)]
        text: Option<TextMenu>,
        #[serde(default)]
        proposal: Option<answer::Proposal>,
    },
    AnswerDelivered {
        period: String,
        answer_id: String,
        delivered: answer::log::Delivered,
    },
    RulesCheck {
        run: answer::run::RunInput,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextMenu {
    source: answer::Source,
    text: String,
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
    let (formats, format_warnings) = answer::Formats::load(answer::formats::user_dir().as_deref());
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
                    view["work_period_capability"] = serde_json::json!(1);
                    view["work_period_request"] = serde_json::to_value(work_period::request(
                        store,
                        binding,
                        generation,
                        now_ms(),
                    )?)?;
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
                Command::WorkPeriodReply { reply } => {
                    validate()?;
                    work_period::acknowledge(store, binding, generation, reply, now_ms())?;
                    Ok(serde_json::json!({"acknowledged": true}))
                }
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
                Command::Recognise { source, text } => {
                    Ok(serde_json::json!({ "menu": formats.recognise(source, &text) }))
                }
                Command::RulesCheck { run } => Ok(rules_check(&run, &format_warnings)),
                Command::Decide {
                    run,
                    menu,
                    text,
                    proposal,
                } => decide(store, &formats, run, menu, text, proposal),
                Command::AnswerDelivered {
                    period,
                    answer_id,
                    delivered,
                } => {
                    answer::log::AnswerLog::new(&store.dir, &period)?
                        .delivered(&answer_id, delivered)?;
                    Ok(serde_json::json!({ "recorded": true }))
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

fn load_rules(run: &answer::run::RunInput) -> Result<answer::Rules> {
    answer::Rules::load(
        Some(&answer::run::repo_root(&run.cwd)),
        answer::rules::user_rules_file().as_deref(),
        &run.rules,
    )
}

/// A parse error is a value, not a refusal: the consent dialog shows it to the user.
fn rules_check(run: &answer::run::RunInput, format_warnings: &[String]) -> serde_json::Value {
    if run.policy == answer::Policy::Yolo {
        return serde_json::json!({ "valid": true, "count": 0, "sources": [], "format_warnings": format_warnings });
    }
    match load_rules(run) {
        Ok(rules) => serde_json::json!({
            "valid": true,
            "count": rules.rules.len(),
            "sources": rules.sources,
            "digest": rules.digest,
            "format_warnings": format_warnings,
        }),
        Err(error) => serde_json::json!({
            "valid": false,
            "error": format!("{error:#}"),
            "format_warnings": format_warnings,
        }),
    }
}

fn decide(
    store: &Store,
    formats: &answer::Formats,
    run: answer::run::RunInput,
    menu: Option<answer::MenuInput>,
    text: Option<TextMenu>,
    proposal: Option<answer::Proposal>,
) -> Result<serde_json::Value> {
    let log = answer::log::AnswerLog::new(&store.dir, &run.period)?;
    let menu = match (menu, text) {
        (Some(input), None) => answer::Menu::from_input(input, &answer::RoleWords::default())?,
        (None, Some(text)) => match formats.recognise(text.source, &text.text) {
            Some(menu) => menu,
            None => {
                let sample = log.unrecognised(&text.text)?;
                let id = log.decision(&serde_json::json!({
                    "run": run.period, "item": run.item, "period": run.period, "menu": null,
                    "proposal": null, "decision": "stop", "policy": run.policy,
                    "reason": "unrecognised_menu", "sample": sample, "delivered": "not_applicable",
                }))?;
                return Ok(serde_json::json!({
                    "answer_id": id,
                    "menu": null,
                    "decision": {"decision": "stop", "reason": "unrecognised_menu", "checks": []},
                    "sample": sample,
                }));
            }
        },
        _ => anyhow::bail!("decide needs exactly one of menu or text"),
    };
    // Rules are fixed at consent: sources that no longer load to the consented digest stop.
    let rules = match run.policy {
        answer::Policy::Yolo => Some(answer::Rules::empty()),
        answer::Policy::Rules => load_rules(&run)
            .ok()
            .filter(|r| run.rules_digest.as_deref() == Some(r.digest.as_str())),
    };
    let decision = match rules {
        Some(rules) => answer::decide(
            &menu,
            &answer::run::context(&run),
            &rules,
            proposal.as_ref(),
        ),
        None => answer::Decision::Stop {
            reason: "rules_changed".into(),
            checks: vec![],
        },
    };
    let no_warnings = Vec::new();
    let (kind, choice, text, rule, reason, warnings, checks, delivered) = match &decision {
        answer::Decision::Answer {
            choice,
            text,
            rule,
            reason,
            warnings,
            checks,
        } => (
            "answer", choice, text, rule, reason, warnings, checks, "pending",
        ),
        answer::Decision::Stop { reason, checks } => (
            "stop",
            &None,
            &None,
            &None,
            reason,
            &no_warnings,
            checks,
            "not_applicable",
        ),
    };
    let record = serde_json::json!({
        "run": run.period, "item": run.item, "period": run.period, "menu": menu,
        "proposal": proposal.as_ref().and_then(|p| p.key.clone()), "decision": kind,
        "choice": choice, "text": text, "policy": run.policy, "rule": rule, "checks": checks,
        "warnings": warnings, "reason": reason, "delivered": delivered,
    });
    let id = log.decision(&record)?;
    Ok(serde_json::json!({ "answer_id": id, "menu": menu, "decision": decision }))
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
    #[test]
    fn bridge_recognises_decides_and_records_answers() {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        let binding = Binding {
            instance_id: "test".into(),
            profile: "default".into(),
            native_id: "native".into(),
            launch_id: "launch".into(),
        };
        let period = "11111111-1111-1111-1111-111111111111";
        let run = serde_json::json!({
            "period": period, "goal": "g", "policy": "yolo", "cwd": cwd.path(),
            "requests_used": 0, "request_limit": 4,
        });
        let send = |packet: serde_json::Value| {
            let mut line = serde_json::to_vec(&packet).unwrap();
            line.push(b'\n');
            let mut out = Vec::new();
            serve(
                &store,
                None,
                &binding,
                "generation",
                std::io::Cursor::new(line),
                &mut out,
                || Ok(()),
            )
            .unwrap();
            let reply: serde_json::Value = serde_json::from_slice(&out).unwrap();
            assert_eq!(reply["ok"], true, "{reply}");
            reply["value"].clone()
        };
        let recognised = send(serde_json::json!({
            "op": "recognise", "source": "agent_text", "text": "Proceed with install? (y/N)",
        }));
        assert!(!recognised["menu"].is_null(), "{recognised}");
        let decided = send(serde_json::json!({
            "op": "decide", "run": run,
            "menu": {
                "source": "pi_dialog", "format": "pi.confirm", "kind": "confirm", "question": "Continue?",
                "options": [{"key": "1", "label": "Yes"}, {"key": "2", "label": "No"}],
            },
        }));
        assert_eq!(decided["decision"]["decision"], "answer", "{decided}");
        assert_eq!(decided["decision"]["choice"], "1");
        let recorded = send(serde_json::json!({
            "op": "answer_delivered", "period": period,
            "answer_id": decided["answer_id"], "delivered": "ok",
        }));
        assert_eq!(recorded["recorded"], true);
        let unrecognised = send(serde_json::json!({
            "op": "decide", "run": run,
            "text": {"source": "agent_text", "text": "nothing to answer here"},
        }));
        assert!(unrecognised["menu"].is_null());
        assert_eq!(unrecognised["decision"]["reason"], "unrecognised_menu");
        assert!(unrecognised["sample"].is_string(), "{unrecognised}");
    }
    #[test]
    #[serial_test::serial]
    fn rules_are_fixed_at_consent_and_a_changed_source_stops_the_answer() {
        // An isolated HOME, so the real ~/.config/aoe/afk/rules.toml is never a rule source.
        let _home = crate::session::test_support::isolate_app_dir();
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        let binding = Binding {
            instance_id: "test".into(),
            profile: "default".into(),
            native_id: "native".into(),
            launch_id: "launch".into(),
        };
        let send = |packet: serde_json::Value| {
            let mut line = serde_json::to_vec(&packet).unwrap();
            line.push(b'\n');
            let mut out = Vec::new();
            serve(
                &store,
                None,
                &binding,
                "generation",
                std::io::Cursor::new(line),
                &mut out,
                || Ok(()),
            )
            .unwrap();
            let reply: serde_json::Value = serde_json::from_slice(&out).unwrap();
            assert_eq!(reply["ok"], true, "{reply}");
            reply["value"].clone()
        };
        let rule = "id='any', kind='confirm', answer='approve'";
        let run = |policy: &str, digest: &str| {
            serde_json::json!({
                "period": "11111111-1111-1111-1111-111111111111", "goal": "g", "policy": policy,
                "rules": [rule], "rules_digest": digest, "cwd": cwd.path(),
                "requests_used": 0, "request_limit": 4,
            })
        };
        let decide = |run: serde_json::Value| {
            send(serde_json::json!({
                "op": "decide", "run": run,
                "menu": {
                    "source": "pi_dialog", "format": "pi.confirm", "kind": "confirm", "question": "Continue?",
                    "options": [{"key": "y", "label": "Yes"}, {"key": "n", "label": "No"}],
                },
            }))["decision"]
                .clone()
        };
        let checked = send(serde_json::json!({"op": "rules_check", "run": run("rules", "")}));
        let digest = checked["digest"].as_str().unwrap_or_default().to_string();
        assert_eq!(digest.len(), 64, "{checked}");
        assert_eq!(decide(run("rules", &digest))["decision"], "answer");
        // The agent writes itself a repo rules file mid-period.
        std::fs::write(
            cwd.path().join("afk-rules.toml"),
            "[[rule]]\nid='mine'\nanswer='approve'\n",
        )
        .unwrap();
        let changed = decide(run("rules", &digest));
        assert_eq!(changed["decision"], "stop", "{changed}");
        assert_eq!(changed["reason"], "rules_changed");
        let rechecked = send(serde_json::json!({"op": "rules_check", "run": run("rules", "")}));
        assert_ne!(rechecked["digest"], checked["digest"]);
        // An inline rule differing from consent is a change too; yolo never checks the digest.
        let mut other = run("rules", rechecked["digest"].as_str().unwrap());
        other["rules"] = serde_json::json!(["id='other', answer='approve'"]);
        assert_eq!(decide(other)["reason"], "rules_changed");
        assert_eq!(decide(run("yolo", "stale"))["decision"], "answer");
    }
}
