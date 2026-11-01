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
                    let log = answer::log::AnswerLog::new(&store.dir, &period)?;
                    // A delivery with no decision before it is not evidence of anything.
                    if !log.has_decision(&answer_id)? {
                        return Ok(
                            serde_json::json!({ "recorded": false, "reason": "unknown_answer" }),
                        );
                    }
                    log.delivered(&answer_id, delivered)?;
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

fn load_rules(
    run: &answer::run::RunInput,
    budget: &answer::run::GitBudget,
) -> Result<answer::Rules> {
    answer::Rules::load(
        Some(&answer::run::repo_root(&run.cwd, budget)),
        answer::rules::user_rules_file().as_deref(),
        &run.rules,
    )
}

/// A parse error is a value, not a refusal: the consent dialog shows it to the user.
fn rules_check(run: &answer::run::RunInput, format_warnings: &[String]) -> serde_json::Value {
    if run.policy == answer::Policy::Yolo {
        return serde_json::json!({
            "valid": true, "count": 0, "sources": [], "format_warnings": format_warnings, "rule_warnings": [],
        });
    }
    match load_rules(run, &answer::run::GitBudget::standard()) {
        Ok(rules) => serde_json::json!({
            "valid": true,
            "count": rules.rules.len(),
            "sources": rules.sources,
            "digest": rules.digest,
            "format_warnings": format_warnings,
            "rule_warnings": rules.warnings,
        }),
        Err(error) => serde_json::json!({
            "valid": false,
            "error": format!("{error:#}"),
            "format_warnings": format_warnings,
            "rule_warnings": [],
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
    // One deadline for every git call this decide makes.
    let budget = answer::run::GitBudget::standard();
    let log = answer::log::AnswerLog::new(&store.dir, &run.period)?;
    let menu = match (menu, text) {
        (Some(input), None) => answer::Menu::from_input(input, &answer::RoleWords::default())?,
        (None, Some(text)) => match formats.recognise(text.source, &text.text) {
            Some(menu) => menu,
            None => {
                // The record comes first, so a sample that can't be saved loses nothing else.
                let id = log.decision(&serde_json::json!({
                    "run": run.period, "item": run.item, "period": run.period, "menu": null,
                    "proposal": null, "decision": "stop", "choice": null, "text": null,
                    "policy": run.policy, "rule": null, "rule_origin": null, "checks": [],
                    "warnings": [], "reason": "unrecognised_menu", "sample": null,
                    "delivered": "not_applicable",
                }))?;
                let sample = log.unrecognised(&text.text).ok();
                if let Some(sample) = &sample {
                    log.sample_saved(&id, sample)?;
                }
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
        answer::Policy::Rules => load_rules(&run, &budget)
            .ok()
            .filter(|r| run.rules_digest.as_deref() == Some(r.digest.as_str())),
    };
    let decision = match &rules {
        Some(rules) => answer::decide(
            &menu,
            &answer::run::context(&run, &budget),
            rules,
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
    // A `stop` rule's id is in its reason; every other rule's is in `rule`.
    let rule_origin = rules.as_ref().and_then(|rules| {
        let id = rule
            .as_deref()
            .or_else(|| reason.strip_prefix("rule_stop:"))?;
        rules.origin_of(id)
    });
    let record = serde_json::json!({
        "run": run.period, "item": run.item, "period": run.period, "menu": menu,
        "proposal": proposal.as_ref().and_then(|p| p.key.clone()), "decision": kind,
        "choice": choice, "text": text, "policy": run.policy, "rule": rule, "rule_origin": rule_origin,
        "checks": checks, "warnings": warnings, "reason": reason, "delivered": delivered,
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
    const PERIOD: &str = "11111111-1111-1111-1111-111111111111";

    fn binding() -> Binding {
        Binding {
            instance_id: "test".into(),
            profile: "default".into(),
            native_id: "native".into(),
            launch_id: "launch".into(),
        }
    }

    /// The whole reply line for one packet, `ok` flag included.
    fn exchange(store: &Store, packet: serde_json::Value) -> (serde_json::Value, usize) {
        let mut line = serde_json::to_vec(&packet).unwrap();
        line.push(b'\n');
        let mut out = Vec::new();
        serve(
            store,
            None,
            &binding(),
            "generation",
            std::io::Cursor::new(line),
            &mut out,
            || Ok(()),
        )
        .unwrap();
        (serde_json::from_slice(&out).unwrap(), out.len())
    }

    fn value_of(store: &Store, packet: serde_json::Value) -> serde_json::Value {
        let (reply, _) = exchange(store, packet);
        assert_eq!(reply["ok"], true, "{reply}");
        reply["value"].clone()
    }

    fn yes_no_menu() -> serde_json::Value {
        serde_json::json!({
            "source": "pi_dialog", "format": "pi.confirm", "kind": "confirm", "question": "Continue?",
            "options": [{"key": "y", "label": "Yes"}, {"key": "n", "label": "No"}],
        })
    }

    fn run_for(
        cwd: &std::path::Path,
        policy: &str,
        rules: &[&str],
        digest: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "period": PERIOD, "goal": "g", "policy": policy, "rules": rules,
            "rules_digest": digest, "cwd": cwd, "requests_used": 0, "request_limit": 4,
        })
    }

    /// The period's answer log, its lines folded into one object per answer id.
    fn folded(store: &Store) -> std::collections::BTreeMap<String, serde_json::Value> {
        let text =
            std::fs::read_to_string(store.dir.path().join(format!("afk-answers/{PERIOD}.jsonl")))
                .unwrap();
        let mut by_id = std::collections::BTreeMap::new();
        for line in text.lines() {
            let line: serde_json::Value = serde_json::from_str(line).unwrap();
            let id = line["answer_id"].as_str().unwrap().to_string();
            let entry = by_id.entry(id).or_insert_with(|| serde_json::json!({}));
            for (k, v) in line.as_object().unwrap() {
                entry[k] = v.clone();
            }
        }
        by_id
    }

    #[test]
    #[serial_test::serial]
    fn rules_check_reports_valid_invalid_yolo_and_warnings() {
        let _home = crate::session::test_support::isolate_app_dir();
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        let check = |policy: &str, rules: &[&str]| {
            value_of(
                &store,
                serde_json::json!({"op": "rules_check", "run": run_for(cwd.path(), policy, rules, "")}),
            )
        };
        let valid = check("rules", &["id='a', kind='confirm', answer='approve'"]);
        assert_eq!(valid["valid"], true, "{valid}");
        assert_eq!(valid["count"], 1);
        assert_eq!(valid["sources"], serde_json::json!(["inline rule 1"]));
        assert_eq!(valid["digest"].as_str().unwrap().len(), 64);
        assert_eq!(valid["rule_warnings"], serde_json::json!([]));
        // A rule that does not parse is a value for the consent dialog, not a refusal.
        let invalid = check("rules", &["id='a', answer='nope'"]);
        assert_eq!(invalid["valid"], false, "{invalid}");
        assert!(invalid["error"].as_str().unwrap().contains("not allowed"));
        assert_eq!(invalid["rule_warnings"], serde_json::json!([]));
        let yolo = check("yolo", &["id='a', answer='nope'"]);
        assert_eq!(yolo["valid"], true);
        assert_eq!(yolo["count"], 0);
        assert_eq!(yolo["rule_warnings"], serde_json::json!([]));
        // A repo stop rule that an inline rule's id shadows is reported.
        std::fs::write(
            cwd.path().join("afk-rules.toml"),
            "[[rule]]\nid='guard'\nanswer='stop'\n",
        )
        .unwrap();
        let shadowed = check("rules", &["id='guard', answer='approve'"]);
        let warnings = shadowed["rule_warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1, "{shadowed}");
        assert!(warnings[0]
            .as_str()
            .unwrap()
            .contains("shadowed_stop_rules"));
    }

    #[test]
    #[serial_test::serial]
    fn rules_decisions_through_the_bridge_log_their_rule_origin() {
        let _home = crate::session::test_support::isolate_app_dir();
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        let rules = [
            "id='stop-all', kind='plan', answer='stop'",
            "id='any', kind='confirm', answer='approve'",
        ];
        let checked = value_of(
            &store,
            serde_json::json!({"op": "rules_check", "run": run_for(cwd.path(), "rules", &rules, "")}),
        );
        let digest = checked["digest"].as_str().unwrap().to_string();
        let run = run_for(cwd.path(), "rules", &rules, &digest);
        let answered = value_of(
            &store,
            serde_json::json!({"op": "decide", "run": run, "menu": yes_no_menu()}),
        );
        assert_eq!(answered["decision"]["decision"], "answer", "{answered}");
        assert_eq!(answered["decision"]["rule"], "any");
        let mut plan = yes_no_menu();
        plan["kind"] = "plan".into();
        let stopped = value_of(
            &store,
            serde_json::json!({"op": "decide", "run": run, "menu": plan}),
        );
        assert_eq!(
            stopped["decision"]["reason"], "rule_stop:stop-all",
            "{stopped}"
        );
        let delivered = value_of(
            &store,
            serde_json::json!({
                "op": "answer_delivered", "period": PERIOD,
                "answer_id": answered["answer_id"], "delivered": "ok",
            }),
        );
        assert_eq!(delivered["recorded"], true);
        let log = folded(&store);
        let answer = &log[answered["answer_id"].as_str().unwrap()];
        assert_eq!(answer["rule"], "any");
        assert_eq!(answer["rule_origin"], "inline rule 2");
        assert_eq!(answer["decision"], "answer");
        assert_eq!(answer["delivered"], "ok");
        let stop = &log[stopped["answer_id"].as_str().unwrap()];
        assert_eq!(stop["rule_origin"], "inline rule 1");
        assert_eq!(stop["delivered"], "not_applicable");
    }

    #[test]
    #[serial_test::serial]
    fn an_unrecognised_stop_record_has_the_same_keys_as_any_other() {
        let _home = crate::session::test_support::isolate_app_dir();
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        let run = run_for(cwd.path(), "yolo", &[], "");
        let unrecognised = value_of(
            &store,
            serde_json::json!({
                "op": "decide", "run": run,
                "text": {"source": "agent_text", "text": "nothing to answer here"},
            }),
        );
        let answered = value_of(
            &store,
            serde_json::json!({"op": "decide", "run": run, "menu": yes_no_menu()}),
        );
        let log = folded(&store);
        let stop = log[unrecognised["answer_id"].as_str().unwrap()]
            .as_object()
            .unwrap()
            .clone();
        let answer = log[answered["answer_id"].as_str().unwrap()]
            .as_object()
            .unwrap()
            .clone();
        for key in [
            "choice",
            "text",
            "rule",
            "rule_origin",
            "checks",
            "warnings",
        ] {
            assert!(stop.contains_key(key), "stop record lacks {key}");
        }
        let missing: Vec<_> = answer.keys().filter(|k| !stop.contains_key(*k)).collect();
        assert!(missing.is_empty(), "stop record lacks {missing:?}");
        assert!(stop["choice"].is_null() && stop["rule"].is_null() && stop["text"].is_null());
        assert_eq!(stop["warnings"], serde_json::json!([]));
        assert_eq!(stop["checks"], serde_json::json!([]));
        // The sample is linked to its record by a follow-up line, which folds in.
        let sample = unrecognised["sample"].as_str().unwrap();
        assert_eq!(stop["sample"], sample);
        assert!(store.dir.path().join("afk-answers").join(sample).is_file());
    }

    #[test]
    #[serial_test::serial]
    fn a_delivery_for_an_unknown_answer_is_refused_as_a_value() {
        let _home = crate::session::test_support::isolate_app_dir();
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        let deliver = |id: &str| {
            value_of(
                &store,
                serde_json::json!({
                    "op": "answer_delivered", "period": PERIOD, "answer_id": id, "delivered": "ok",
                }),
            )
        };
        let stranger = uuid::Uuid::new_v4().to_string();
        let refused = deliver(&stranger);
        assert_eq!(refused["recorded"], false, "{refused}");
        assert_eq!(refused["reason"], "unknown_answer");
        let decided = value_of(
            &store,
            serde_json::json!({"op": "decide", "run": run_for(cwd.path(), "yolo", &[], ""), "menu": yes_no_menu()}),
        );
        // Another period's decision is no evidence either.
        let other = value_of(
            &store,
            serde_json::json!({
                "op": "answer_delivered", "period": "22222222-2222-2222-2222-222222222222",
                "answer_id": decided["answer_id"], "delivered": "ok",
            }),
        );
        assert_eq!(other["recorded"], false);
        assert_eq!(
            deliver(decided["answer_id"].as_str().unwrap())["recorded"],
            true
        );
        assert!(!folded(&store).contains_key(&stranger));
    }

    #[test]
    #[serial_test::serial]
    fn the_answer_log_is_not_appended_through_a_hard_link_or_a_non_regular_file() {
        let _home = crate::session::test_support::isolate_app_dir();
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        let decide = || {
            exchange(
                &store,
                serde_json::json!({"op": "decide", "run": run_for(cwd.path(), "yolo", &[], ""), "menu": yes_no_menu()}),
            )
            .0
        };
        assert_eq!(decide()["ok"], true);
        let log = store.dir.path().join(format!("afk-answers/{PERIOD}.jsonl"));
        let before = std::fs::read(&log).unwrap();
        let link = store.dir.path().join("afk-answers/elsewhere.jsonl");
        std::fs::hard_link(&log, &link).unwrap();
        let refused = decide();
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(std::fs::read(&log).unwrap(), before, "nothing was appended");
        std::fs::remove_file(&link).unwrap();
        assert_eq!(decide()["ok"], true, "a single link is fine again");
        std::fs::remove_file(&log).unwrap();
        std::fs::create_dir(&log).unwrap();
        let refused = decide();
        assert_eq!(refused["ok"], false, "{refused}");
    }

    #[test]
    #[serial_test::serial]
    fn the_largest_decide_reply_fits_one_frame() {
        let _home = crate::session::test_support::isolate_app_dir();
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(root.path(), "test", true).unwrap();
        // Control bytes escape to six bytes each: the worst case for every text field.
        let worst = |n: usize| format!("\"\\{}", "\u{1}".repeat(n - 2));
        let menu = serde_json::json!({
            "source": "pi_dialog", "format": worst(128), "kind": "choice",
            "question": worst(2048), "subject": worst(2048),
            "options": (0..32).map(|i| serde_json::json!({
                "key": format!("{i:02}{}", "\u{1}".repeat(62)), "label": worst(256),
            })).collect::<Vec<_>>(),
            "free_text": true, "warnings": vec![worst(512); 8], "deadline_ms": u64::MAX,
            "raw": worst(4096),
        });
        let (reply, bytes) = exchange(
            &store,
            serde_json::json!({
                "op": "decide", "run": run_for(cwd.path(), "yolo", &[], ""), "menu": menu,
                "proposal": {"reason": worst(1024)},
            }),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["value"]["decision"]["decision"], "answer");
        assert!(
            bytes > 100 * 1024 && bytes < delegation::FRAME_BYTES,
            "{bytes} bytes vs frame {}",
            delegation::FRAME_BYTES
        );
    }
}
