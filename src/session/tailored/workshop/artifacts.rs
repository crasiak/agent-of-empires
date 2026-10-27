use super::{deployment::*, *};
use anyhow::ensure;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn go_digest(value: &Value, blank_digest: bool) -> Result<String> {
    let mut value = value.clone();
    if blank_digest {
        value["digest"] = json!("");
    }
    let bytes = serde_json::to_string(&value)?
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    Ok(Sha256::digest(bytes.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn producer_digest(value: &Value) -> Result<String> {
    let actual = hash(value, "digest")?;
    ensure!(
        actual == go_digest(value, true)?,
        "Producer artifact digest changed"
    );
    Ok(actual)
}
fn pair(value: &Value) -> Result<String> {
    Ok(format!(
        "{}/{}",
        value["profile"].as_str().context("Missing profile")?,
        value["harness"].as_str().context("Missing harness")?
    ))
}
fn array<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>> {
    value[field]
        .as_array()
        .context("Missing complete producer array")
}
fn request_matches(d: &Deployment, request: &Value) -> Result<()> {
    ensure!(
        request["landed"] == d.landed && request["targets"] == serde_json::to_value(&d.targets)?,
        "Producer changed exact landed source/selected target scope"
    );
    Ok(())
}
fn validate_guard(preview: &Value) -> Result<()> {
    let guard = &preview["guard"];
    let request = &preview["request"];
    schema(guard, "ledger.skill-import-guard.v1")?;
    producer_digest(guard)?;
    ensure!(
        guard["targets"] == request["targets"]
            && guard["source"]["root"] == request["source_root"]
            && guard["source"]["name"] == request["source_name"]
            && guard["source"]["tree_hash"] == request["landed"]["tree_hash"]
            && guard["source"]["key"]["kind"] == "skill"
            && request["landed"]["path"]
                == format!(
                    ".agents/skills/{}",
                    guard["source"]["key"]["name"]
                        .as_str()
                        .context("Missing skill key")?
                )
            && guard["revision"] == preview["base"]["revision"],
        "Import guard source/target/epoch changed"
    );
    let mut selected = BTreeMap::new();
    let targets = array(request, "targets")?;
    for p in array(guard, "selected")? {
        let key = pair(p)?;
        ensure!(
            selected.insert(key, p).is_none()
                && targets.iter().any(|t| t["profile"] == p["profile"]
                    && t["harness"] == p["harness"]
                    && t["account"] == p["account"]),
            "Selected account/pair identity changed"
        );
        hash(p, "account_revision")?;
    }
    ensure!(
        selected.len() == targets.len(),
        "Incomplete selected account/pair set"
    );
    let mut affected = BTreeSet::new();
    for p in array(guard, "affected")? {
        let key = pair(p)?;
        ensure!(
            affected.insert(key.clone()) && selected.get(&key) == Some(&p),
            "Duplicate/unselected affected import pair"
        );
    }
    let mut consumers = BTreeSet::new();
    for c in array(guard, "consumers")? {
        ensure!(
            c["profile"].as_str().is_some_and(|name| !name.is_empty())
                && consumers.insert(c["profile"].clone().to_string())
                && c["denied"].is_boolean()
                && c["changing"].is_boolean(),
            "Incomplete current consumer snapshot"
        );
    }
    Ok(())
}
fn validate_import(d: &Deployment, receipt: &Value) -> Result<()> {
    schema(receipt, "cra1260.workshop-deploy-receipt.v1")?;
    let preview = d
        .import_preview
        .as_ref()
        .context("Missing approved import preview")?;
    validate_guard(preview)?;
    ensure!(
        receipt["preview_digest"] == preview["digest"]
            && receipt["state"] == "imported"
            && receipt["result"]["guard"] == preview["guard"],
        "Guarded import unknown/mismatching approval"
    );
    schema(&receipt["result"], "ledger.skill-import-receipt.v1")?;
    hash(&receipt["result"], "revision")?;
    let imported = &receipt["result"]["import"];
    let resources = array(imported, "resources")?;
    ensure!(
        imported["status"] == "completed"
            && imported["import_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
            && resources.len() == 1
            && array(imported, "collisions")?.is_empty()
            && array(imported, "rejected")?.is_empty(),
        "Import did not confirm one exact resource"
    );
    let resource = &resources[0];
    let source = &preview["guard"]["source"];
    ensure!(
        resource["key"] == source["key"]
            && resource["tree_hash"] == source["tree_hash"]
            && resource["resource_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
        "Import resource identity changed"
    );
    hash(resource, "tree_hash")?;
    hash(resource, "version_hash")?;
    let root = resource["source_root"]
        .as_str()
        .context("Missing imported source root")?;
    let path = resource["source_path"]
        .as_str()
        .context("Missing imported source path")?;
    ensure!(
        std::path::Path::new(root).is_absolute()
            && !std::path::Path::new(path).is_absolute()
            && !std::path::Path::new(path)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
        "Unsafe imported source location"
    );
    let resolved: PathBuf = std::path::Path::new(root).join(path).components().collect();
    ensure!(
        resolved.to_str() == source["root"].as_str(),
        "Imported source location changed"
    );
    Ok(())
}

fn candidate(d: &Deployment) -> Result<&Value> {
    d.candidate
        .as_ref()
        .context("Missing original approved candidate")
}
fn expected_pairs(candidate: &Value) -> Result<BTreeMap<String, Value>> {
    let mut pairs = BTreeMap::new();
    for p in array(&candidate["import_receipt"]["guard"], "affected")? {
        let key = pair(p)?;
        ensure!(!pairs.contains_key(&key), "Duplicate import pair");
        pairs.insert(
            key,
            json!({"profile":p["profile"],"harness":p["harness"],"action":"rematerialize"}),
        );
    }
    let mut preview = BTreeSet::new();
    for p in array(&candidate["preview"], "affected")? {
        let key = pair(p)?;
        ensure!(
            preview.insert(key.clone())
                && matches!(p["action"].as_str(), Some("materialize" | "rematerialize")),
            "Duplicate/unsupported config-preview pair"
        );
        pairs.insert(key, p.clone());
    }
    ensure!(
        !pairs.is_empty()
            && array(candidate, "materialize_pairs")?
                == &pairs.values().cloned().collect::<Vec<_>>(),
        "Import/config materialization union changed"
    );
    Ok(pairs)
}
fn validate_candidate(d: &Deployment, value: &Value) -> Result<()> {
    schema(value, "cra1260.workshop-deploy-candidate.v2")?;
    producer_digest(value)?;
    ensure!(
        value["directory"].as_str() == d.candidate_directory.to_str(),
        "Candidate private directory changed"
    );
    schema(&value["request"], "cra1260.workshop-deploy-request.v1")?;
    request_matches(d, &value["request"])?;
    let imported = d
        .import_receipt
        .as_ref()
        .context("Missing confirmed import")?;
    validate_import(d, imported)?;
    ensure!(
        value["import_receipt"] == imported["result"]
            && value["base"]["revision"] == imported["result"]["revision"],
        "Candidate does not retain original confirmed import/post-import epoch"
    );
    hash(&value["base"], "revision")?;
    let content = value["content"]
        .as_str()
        .context("Missing complete config content")?;
    let bytes_digest: String = Sha256::digest(content.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ensure!(
        hash(value, "bytes_digest")? == bytes_digest,
        "Candidate config bytes changed"
    );
    let resources = array(&imported["result"]["import"], "resources")?;
    ensure!(
        resources.len() == 1 && resources[0] == value["request"]["resource"],
        "Imported resource/version identity changed"
    );
    let pairs = expected_pairs(value)?;
    let selected: BTreeSet<_> = d
        .targets
        .iter()
        .map(|t| format!("{}/{}", t.profile, t.harness))
        .collect();
    ensure!(
        pairs.keys().all(|p| selected.contains(p)),
        "Unapproved affected materialization pair"
    );
    Ok(())
}
fn validate_publication(d: &Deployment, value: &Value) -> Result<()> {
    schema(value, "cra1260.workshop-publication-receipt.v2")?;
    let c = candidate(d)?;
    ensure!(
        value["candidate_digest"] == c["digest"] && value["state"] == "published",
        "Publication outcome not confirmed/synchronized"
    );
    hash(&value["summary"], "publication_revision")?;
    ensure!(
        value["summary"]["publication_revision"] != c["base"]["revision"],
        "Missing actual atomic post-CAS continuation baseline"
    );
    Ok(())
}
fn validate_materialization(d: &Deployment, value: &Value) -> Result<()> {
    schema(value, "cra1260.workshop-materialize-receipt.v2")?;
    let c = candidate(d)?;
    let publication = d
        .publication_receipt
        .as_ref()
        .context("Missing atomic publication receipt")?;
    validate_publication(d, publication)?;
    ensure!(
        value["candidate_digest"] == c["digest"]
            && value["state"] == "materialized"
            && value["publication_revision"] == publication["summary"]["publication_revision"],
        "Old/partial/unknown materialization or publication epoch changed"
    );
    let expected = expected_pairs(c)?;
    let mut seen = BTreeSet::new();
    for p in array(value, "pairs")? {
        let key = pair(p)?;
        ensure!(
            seen.insert(key.clone())
                && expected.contains_key(&key)
                && p["state"] == "materialized"
                && p.get("error").is_none(),
            "Duplicate/unapproved/unsuccessful materialization pair"
        );
        hash(p, "lock")?;
        hash(p, "materialization")?;
    }
    ensure!(
        seen == expected.keys().cloned().collect(),
        "Incomplete materialization pair set"
    );
    Ok(())
}
fn validate_rollout(d: &Deployment, plan: &Value, result: bool) -> Result<()> {
    schema(plan, "cra1260.workshop-rollout.v2")?;
    let c = candidate(d)?;
    let mat = d
        .materialize_receipt
        .as_ref()
        .context("Missing materialization")?;
    validate_materialization(d, mat)?;
    ensure!(
        plan["candidate_digest"] == c["digest"]
            && plan["publication_revision"] == mat["publication_revision"],
        "Native plan changed publication origin"
    );
    let selected: BTreeSet<_> = d
        .targets
        .iter()
        .flat_map(|t| t.sessions.iter().cloned())
        .collect();
    let mut seen = BTreeSet::new();
    let mut attempted = 0;
    for entry in array(plan, "targets")? {
        let id = entry["id"]
            .as_str()
            .context("Missing exact native session ID")?;
        ensure!(
            selected.contains(id) && seen.insert(id.to_owned()),
            "Native plan changed explicit session set"
        );
        ensure!(
            matches!(
                entry["state"].as_str(),
                Some("published-not-adopted" | "waiting-idle" | "unsupported" | "unknown")
            ) || result && entry["state"] == "adopted",
            "Unknown native target state"
        );
        if result {
            let previous = array(
                &d.rollouts.last().context("Missing approved plan")?.plan,
                "targets",
            )?
            .iter()
            .find(|p| p["id"] == id)
            .context("Native result not in approved plan")?;
            ensure!(
                entry["plan"] == previous["plan"],
                "Native result changed approved plan"
            );
            if entry.get("receipt").is_some() {
                attempted += 1;
                ensure!(
                    previous["state"] == "published-not-adopted" && attempted <= 1,
                    "Native attempt exceeded explicit one-target limit"
                );
            }
        }
        if result && entry["state"] == "adopted" {
            let original = d.rollouts.last().context("Missing approved plan")?;
            let previous = array(&original.plan, "targets")?
                .iter()
                .find(|p| p["id"] == id)
                .context("Native result not in approved plan")?;
            validate_adoption(previous, entry, &c["request"]["resource"]["key"]["name"])?;
        }
    }
    ensure!(seen == selected, "Incomplete native session result set");
    Ok(())
}

fn validate_adoption(previous: &Value, entry: &Value, skill: &Value) -> Result<()> {
    let target = &previous["plan"]["target"];
    let receipt = &entry["receipt"];
    let identity = &receipt["verification"]["identity"];
    ensure!(
        receipt["result"] == "refreshed"
            && identity["pid"].as_u64().is_some_and(|pid| pid > 1)
            && identity["generation"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
            && identity["instance_id"] == target["id"]
            && identity["native_id"] == target["native_id"]
            && identity["cwd"] == target["cwd"]
            && identity["profile"] == target["ledger_profile"]
            && identity["run_id"].as_str().is_some_and(|s| !s.is_empty())
            && identity["run_id"] != target["identity"]["run_id"]
            && identity["lock_hash"] == target["preview"]["expected_lock"],
        "Adoption lacks verified new run/same conversation/cwd/expected Lock"
    );
    let names = array(
        &receipt["verification"]["resource_discovery"],
        "skill_names",
    )?;
    ensure!(
        names.contains(skill)
            && array(&target["preview"], "effective_skill_names")?
                .iter()
                .all(|n| names.contains(n)),
        "Adoption lacks expected skill discovery/local grants"
    );
    Ok(())
}

pub(super) fn accept_artifact(d: &mut Deployment) -> Result<State> {
    match d.intent.stage {
        DeployStage::ImportPreview => {
            let p = read_artifact(&d.import_directory, "import-preview.json")?;
            schema(&p, "cra1260.workshop-import-preview.v1")?;
            producer_digest(&p)?;
            ensure!(
                p["directory"].as_str() == d.import_directory.to_str(),
                "Import private directory changed"
            );
            request_matches(d, &p["request"])?;
            ensure!(
                p["request"] == read_artifact(&d.directory, "import-request.json")?,
                "Import source/request identity changed"
            );
            schema(&p["guard"], "ledger.skill-import-guard.v1")?;
            producer_digest(&p["guard"])?;
            ensure!(
                p["guard"]["targets"] == p["request"]["targets"]
                    && p["guard"]["source"]["tree_hash"] == d.landed["tree_hash"]
                    && p["guard"]["revision"] == p["base"]["revision"],
                "Guard/source/epoch identity changed"
            );
            hash(&p["base"], "revision")?;
            validate_guard(&p)?;
            d.import_preview = Some(p);
            Ok(State::ImportPreview)
        }
        DeployStage::Import => {
            let receipt = read_artifact(&d.import_directory, "import-receipt.json")?;
            validate_import(d, &receipt)?;
            d.import_receipt = Some(receipt);
            Ok(State::Imported)
        }
        DeployStage::CandidatePreview => {
            let value = read_artifact(&d.candidate_directory, "candidate.json")?;
            validate_candidate(d, &value)?;
            d.candidate = Some(value);
            Ok(State::CandidatePreview)
        }
        DeployStage::Apply => {
            let value = read_artifact(&d.candidate_directory, "apply-receipt.json")?;
            validate_publication(d, &value)?;
            d.publication_receipt = Some(value);
            Ok(State::PublishedConfig)
        }
        DeployStage::Materialize => {
            let value = read_artifact(&d.candidate_directory, "materialize-receipt.json")?;
            validate_materialization(d, &value)?;
            d.materialize_receipt = Some(value);
            Ok(State::Materialized)
        }
        DeployStage::RolloutPreview => {
            let directory = d.directory.join(format!("rollout-{}", d.intent.attempt));
            let plan = read_artifact(&directory, "plan.json")?;
            validate_rollout(d, &plan, false)?;
            let plan_digest = go_digest(&plan, false)?;
            d.rollouts.push(RolloutAttempt {
                directory,
                plan_digest,
                plan,
                result: None,
            });
            Ok(State::RolloutPreview)
        }
        DeployStage::Rollout => {
            let rollout = d.rollouts.last().context("Missing approved native plan")?;
            let value = read_artifact(&rollout.directory, "results.json")?;
            ensure!(
                value["plan_digest"] == rollout.plan_digest,
                "Native result approval changed"
            );
            validate_rollout(d, &value, true)?;
            let unknown = array(&value, "targets")?
                .iter()
                .any(|p| p["state"] == "unknown");
            d.rollouts.last_mut().context("Missing native plan")?.result = Some(value);
            Ok(if unknown {
                State::DeployUnknown
            } else {
                State::RolloutComplete
            })
        }
        DeployStage::ImportApproval | DeployStage::CandidateApproval => {
            bail!("Interrupted approval is retained; cannot automatically invoke its next effect")
        }
    }
}

#[cfg(test)]
pub(super) fn check_real_completed_receipt_fences(d: &Deployment) {
    validate_candidate(d, d.candidate.as_ref().unwrap()).unwrap();
    validate_publication(d, d.publication_receipt.as_ref().unwrap()).unwrap();
    validate_materialization(d, d.materialize_receipt.as_ref().unwrap()).unwrap();
    for change in [
        "old-schema",
        "missing-epoch",
        "pre-cas-epoch",
        "missing-pair",
        "duplicate-pair",
        "extra-pair",
    ] {
        let mut receipt = d.materialize_receipt.clone().unwrap();
        match change {
            "old-schema" => receipt["schema"] = json!("cra1260.workshop-materialize-receipt.v1"),
            "missing-epoch" => {
                receipt
                    .as_object_mut()
                    .unwrap()
                    .remove("publication_revision");
            }
            "pre-cas-epoch" => {
                receipt["publication_revision"] =
                    d.candidate.as_ref().unwrap()["base"]["revision"].clone()
            }
            "missing-pair" => {
                receipt["pairs"].as_array_mut().unwrap().pop();
            }
            "duplicate-pair" => {
                let pair = receipt["pairs"][0].clone();
                receipt["pairs"].as_array_mut().unwrap().push(pair);
            }
            _ => {
                let mut pair = receipt["pairs"][0].clone();
                pair["profile"] = json!("unselected");
                receipt["pairs"].as_array_mut().unwrap().push(pair);
            }
        }
        assert!(validate_materialization(d, &receipt).is_err(), "{change}");
    }
    for change in ["old-schema", "missing-epoch", "pre-cas-epoch", "unknown"] {
        let mut receipt = d.publication_receipt.clone().unwrap();
        match change {
            "old-schema" => receipt["schema"] = json!("cra1260.workshop-publication-receipt.v1"),
            "missing-epoch" => {
                receipt["summary"]
                    .as_object_mut()
                    .unwrap()
                    .remove("publication_revision");
            }
            "pre-cas-epoch" => {
                receipt["summary"]["publication_revision"] =
                    d.candidate.as_ref().unwrap()["base"]["revision"].clone()
            }
            _ => receipt["state"] = json!("unknown"),
        }
        assert!(validate_publication(d, &receipt).is_err(), "{change}");
    }
    let mut unknown = d.import_receipt.clone().unwrap();
    unknown["state"] = json!("unknown");
    assert!(validate_import(d, &unknown).is_err());
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../../tests/fixtures/workshop/guarded-import.json"
        ))
        .unwrap()
    }
    #[test]
    fn real_go_guarded_producer_digests_and_two_pair_union_match_rust_reader() {
        let value = fixture();
        producer_digest(&value["preview"]).unwrap();
        producer_digest(&value["preview"]["guard"]).unwrap();
        producer_digest(&value["candidate"]).unwrap();
        let expected = expected_pairs(&value["candidate"]).unwrap();
        assert_eq!(
            expected.keys().map(String::as_str).collect::<Vec<_>>(),
            ["selected-child/codex", "selected/codex"]
        );
        assert_eq!(
            value["materialized"]["schema"],
            "cra1260.workshop-materialize-receipt.v2"
        );
        assert_ne!(
            value["materialized"]["publication_revision"],
            value["candidate"]["base"]["revision"]
        );
        for mutation in ["missing", "duplicate", "extra", "changed-action"] {
            let mut c = value["candidate"].clone();
            let pairs = c["materialize_pairs"].as_array_mut().unwrap();
            match mutation {
                "missing" => {
                    pairs.pop();
                }
                "duplicate" => {
                    pairs.push(pairs[0].clone());
                }
                "extra" => {
                    pairs.push(
                        json!({"profile":"unselected","harness":"codex","action":"materialize"}),
                    );
                }
                _ => {
                    pairs[0]["action"] = json!("remove");
                }
            }
            c["digest"] = json!(go_digest(&c, true).unwrap());
            producer_digest(&c).unwrap();
            assert!(
                expected_pairs(&c).is_err(),
                "{mutation}: outer digest must not bless inconsistent pair union"
            );
        }
    }
    #[test]
    fn actual_native_planner_receipt_requires_new_run_identity_lock_and_discovered_local_grants() {
        let value: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/workshop/native.json"
        ))
        .unwrap();
        let previous = &value["plan"]["targets"][0];
        let adopted = &value["result"]["targets"][0];
        validate_adoption(previous, adopted, &value["skill"]).unwrap();
        for mutation in [
            "run",
            "conversation",
            "cwd",
            "lock",
            "discovery",
            "profile",
            "pid",
            "generation",
        ] {
            let mut receipt = adopted.clone();
            match mutation {
                "run" => {
                    receipt["receipt"]["verification"]["identity"]["run_id"] =
                        previous["plan"]["target"]["identity"]["run_id"].clone()
                }
                "conversation" => {
                    receipt["receipt"]["verification"]["identity"]["native_id"] = json!("wrong")
                }
                "cwd" => receipt["receipt"]["verification"]["identity"]["cwd"] = json!("/wrong"),
                "lock" => {
                    receipt["receipt"]["verification"]["identity"]["lock_hash"] =
                        json!("0".repeat(64))
                }
                "discovery" => {
                    receipt["receipt"]["verification"]["resource_discovery"]["skill_names"] =
                        json!(["approved"])
                }
                "profile" => {
                    receipt["receipt"]["verification"]["identity"]["profile"] = json!("wrong")
                }
                "pid" => receipt["receipt"]["verification"]["identity"]["pid"] = json!(1),
                _ => receipt["receipt"]["verification"]["identity"]["generation"] = json!(""),
            }
            assert!(
                validate_adoption(previous, &receipt, &value["skill"]).is_err(),
                "{mutation}"
            );
        }
    }
    #[test]
    fn user_operations_cannot_supply_host_receipts_or_invent_policy() {
        for value in [
            json!({"operation":"land","id":"id","revision":0,"policy_digest":"client","current_base_and_rules":true,"exclusive_pr_custody":true,"landed":{"merged_commit":"claimed"}}),
            json!({"operation":"preview-import","id":"id","revision":0,"source_name":"fixture","targets":[],"source_root":"/arbitrary"}),
            json!({"operation":"deploy","id":"id","revision":0,"candidate_digest":"d","receipt_path":"/arbitrary"}),
            json!({"operation":"land","id":"id","revision":0,"policy":{"ready":true}}),
        ] {
            assert!(serde_json::from_value::<Operation>(value).is_err());
        }
    }
}
