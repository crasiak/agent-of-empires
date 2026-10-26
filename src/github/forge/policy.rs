use super::*;

fn object<'a>(
    value: &'a Value,
    required: &[&str],
    allowed: &[&str],
) -> Result<&'a serde_json::Map<String, Value>> {
    let object = value.as_object().context("Missing policy object")?;
    ensure!(
        required.iter().all(|k| object.contains_key(*k))
            && object.keys().all(|k| allowed.contains(&k.as_str())),
        "Partial/unknown policy fields"
    );
    Ok(object)
}
fn boolean(value: &Value, key: &str) -> Result<bool> {
    value[key]
        .as_bool()
        .with_context(|| format!("Missing/null policy boolean {key}"))
}
fn reviews(value: &Value, classic: bool) -> Result<()> {
    let keys = if classic {
        vec![
            "dismiss_stale_reviews",
            "require_code_owner_reviews",
            "require_last_push_approval",
            "required_approving_review_count",
        ]
    } else {
        vec![
            "dismiss_stale_reviews_on_push",
            "require_code_owner_review",
            "require_last_push_approval",
            "required_approving_review_count",
            "required_review_thread_resolution",
        ]
    };
    let allowed = if classic {
        vec![
            "url",
            "dismissal_restrictions",
            "bypass_pull_request_allowances",
            "dismiss_stale_reviews",
            "require_code_owner_reviews",
            "require_last_push_approval",
            "required_approving_review_count",
        ]
    } else {
        vec![
            "dismiss_stale_reviews_on_push",
            "require_code_owner_review",
            "require_last_push_approval",
            "required_approving_review_count",
            "required_review_thread_resolution",
            "allowed_merge_methods",
            "required_reviewers",
            "dismissal_restriction",
        ]
    };
    object(value, &keys, &allowed)?;
    for key in keys
        .iter()
        .filter(|k| **k != "required_approving_review_count")
    {
        boolean(value, key)?;
    }
    ensure!(
        value["required_approving_review_count"]
            .as_u64()
            .is_some_and(|n| n <= 10),
        "Unknown review threshold"
    );
    let owner_key = if classic {
        "require_code_owner_reviews"
    } else {
        "require_code_owner_review"
    };
    ensure!(
        !boolean(value, owner_key)? && !boolean(value, "require_last_push_approval")?,
        "Private CODEOWNERS/last-pusher eligibility is not supported"
    );
    for key in [
        "dismissal_restrictions",
        "bypass_pull_request_allowances",
        "required_reviewers",
        "dismissal_restriction",
    ] {
        if let Some(v) = value.get(key) {
            ensure!(
                v.is_null() || v.as_array().is_some_and(Vec::is_empty),
                "Private/custom review eligibility unsupported"
            );
        }
    }
    if let Some(methods) = value.get("allowed_merge_methods") {
        ensure!(
            methods
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "squash")),
            "Squash is not allowed"
        );
    }
    Ok(())
}
fn checks(value: &Value, classic: bool) -> Result<()> {
    let list = if classic {
        object(
            value,
            &["strict", "contexts", "checks"],
            &["url", "strict", "contexts", "contexts_url", "checks"],
        )?;
        boolean(value, "strict")?;
        ensure!(
            value["contexts"]
                .as_array()
                .is_some_and(|a| a.len() <= 100 && a.iter().all(Value::is_string)),
            "Incomplete required contexts"
        );
        value["checks"]
            .as_array()
            .context("Missing required checks")?
    } else {
        object(
            value,
            &[
                "required_status_checks",
                "strict_required_status_checks_policy",
            ],
            &[
                "required_status_checks",
                "strict_required_status_checks_policy",
                "do_not_enforce_on_create",
            ],
        )?;
        boolean(value, "strict_required_status_checks_policy")?;
        if value.get("do_not_enforce_on_create").is_some() {
            boolean(value, "do_not_enforce_on_create")?;
        }
        value["required_status_checks"]
            .as_array()
            .context("Missing required status checks")?
    };
    ensure!(list.len() <= 100, "Too many required checks");
    for check in list {
        let app = if classic { "app_id" } else { "integration_id" };
        object(
            check,
            if classic {
                &["context", "app_id"]
            } else {
                &["context"]
            },
            &["context", app],
        )?;
        bounded_text(
            check["context"]
                .as_str()
                .filter(|s| !s.is_empty())
                .context("Missing context")?,
            200,
        )?;
        if let Some(v) = check.get(app) {
            ensure!(
                v.is_null() || v.as_i64().is_some_and(|n| n >= -1),
                "Unknown check provider"
            );
        }
    }
    if classic {
        let contexts = value["contexts"].as_array().context("Missing contexts")?;
        ensure!(
            contexts
                .iter()
                .all(|c| list.iter().any(|v| v["context"] == *c)),
            "Contexts/checks policy mismatch"
        );
    }
    Ok(())
}
pub(super) fn validate_policy(rules: &[Value], classic: &Value) -> Result<()> {
    ensure!(rules.len() <= 1000, "Too many active rules");
    for rule in rules {
        object(
            rule,
            &[
                "type",
                "ruleset_id",
                "ruleset_source",
                "ruleset_source_type",
            ],
            &[
                "type",
                "parameters",
                "ruleset_id",
                "ruleset_source",
                "ruleset_source_type",
            ],
        )?;
        ensure!(
            rule["ruleset_id"].as_u64().is_some_and(|id| id > 0)
                && rule["ruleset_source"].is_string()
                && matches!(
                    rule["ruleset_source_type"].as_str(),
                    Some("Repository" | "Organization" | "Enterprise")
                ),
            "Unknown rule provenance"
        );
        match rule["type"].as_str() {
            Some("pull_request") => reviews(&rule["parameters"],false)?,
            Some("required_status_checks") => checks(&rule["parameters"],false)?,
            Some("non_fast_forward" | "deletion" | "required_linear_history") => ensure!(rule.get("parameters").is_none(),"Unknown rule parameters"),
            _ => bail!("Unsupported active rule (queue/workflow/deployment/custom rules are never bypassed)"),
        }
    }
    let allowed = [
        "url",
        "enabled",
        "required_status_checks",
        "enforce_admins",
        "required_pull_request_reviews",
        "restrictions",
        "required_linear_history",
        "allow_force_pushes",
        "allow_deletions",
        "block_creations",
        "required_conversation_resolution",
        "name",
        "protection_url",
        "required_signatures",
        "lock_branch",
        "allow_fork_syncing",
    ];
    object(
        classic,
        &[
            "required_status_checks",
            "required_pull_request_reviews",
            "enforce_admins",
            "restrictions",
            "required_linear_history",
            "required_conversation_resolution",
            "lock_branch",
        ],
        &allowed,
    )?;
    if !classic["required_status_checks"].is_null() {
        checks(&classic["required_status_checks"], true)?;
    }
    if !classic["required_pull_request_reviews"].is_null() {
        reviews(&classic["required_pull_request_reviews"], true)?;
    }
    ensure!(
        classic["restrictions"].is_null(),
        "Private push restrictions unsupported"
    );
    for key in [
        "enforce_admins",
        "required_linear_history",
        "allow_force_pushes",
        "allow_deletions",
        "block_creations",
        "required_conversation_resolution",
        "required_signatures",
        "lock_branch",
        "allow_fork_syncing",
    ] {
        if let Some(v) = classic.get(key) {
            object(v, &["enabled"], &["enabled", "url"])?;
            let enabled = boolean(v, "enabled")?;
            if matches!(key, "lock_branch" | "required_signatures") {
                ensure!(!enabled, "Locked/signed-only base unsupported");
            }
        }
    }
    Ok(())
}

impl<T: Transport> Forge<T> {
    fn actual_pr(&self, published: &PublishedPr, require_open: bool) -> Result<Value> {
        published.destination.validate()?;
        source::validate_skill_path(&published.skill_path)?;
        oid(&published.head)?;
        oid(&published.skill_tree)?;
        branch(&published.head_ref)?;
        let value: Value = self.get(
            &published.destination,
            &format!("/pulls/{}", published.identity.number),
        )?;
        ensure!(
            published.identity.repository_id == published.destination.repository_id
                && value["id"].as_u64() == Some(published.identity.id)
                && value["node_id"].as_str() == Some(&published.identity.node_id)
                && value["number"].as_u64() == Some(published.identity.number)
                && value["html_url"].as_str() == Some(&published.identity.url),
            "Stable PR identity changed"
        );
        ensure!(
            value["head"]["repo"]["id"].as_u64() == Some(published.identity.repository_id)
                && value["base"]["repo"]["id"].as_u64() == Some(published.identity.repository_id)
                && value["head"]["sha"].as_str() == Some(&published.head)
                && value["head"]["ref"].as_str() == Some(&published.head_ref)
                && value["base"]["ref"].as_str() == Some(&published.destination.base_branch),
            "PR head/base/repository changed; fresh approval required"
        );
        ensure!(
            value["user"]["id"].as_u64() == Some(published.actor.id)
                && digest(&(
                    value["title"].as_str().context("Missing PR title")?,
                    value["body"].as_str().context("Missing PR body")?
                ))? == published.publication_content_digest,
            "Owned PR author/content changed"
        );
        oid(value["base"]["sha"]
            .as_str()
            .context("Unknown PR base head")?)?;
        ensure!(
            value.get("stack") == Some(&Value::Null)
                && value.get("auto_merge") == Some(&Value::Null),
            "Stacked/auto-merge/missing PR state is unsupported"
        );
        if require_open {
            ensure!(
                value["state"] == "open"
                    && value["draft"] == false
                    && value["merged"] == false
                    && value["mergeable"].as_bool() == Some(true),
                "PR not ready or mergeability unknown"
            );
        }
        Ok(value)
    }
    /// Complete readable policy only. No green-rollup or invented review-count readiness.
    pub fn observe_policy(&self, published: &PublishedPr) -> Result<PolicyObservation> {
        let repository = self.repository(&published.destination)?;
        ensure!(
            repository.default_branch == published.destination.base_branch,
            "Initial Land supports only explicit default-branch targets"
        );
        ensure!(
            self.actor()? == published.actor,
            "Authenticated host account changed"
        );
        let before = self.actual_pr(published, true)?;
        let rules = self.pages(
            &published.destination,
            &format!(
                "/rules/branches/{}",
                segment(&published.destination.base_branch)
            ),
        )?;
        let classic: Value = self.get(
            &published.destination,
            &format!(
                "/branches/{}/protection",
                segment(&published.destination.base_branch)
            ),
        )?;
        validate_policy(&rules, &classic)?;
        let after = self.actual_pr(published, true)?;
        ensure!(
            before["base"]["sha"] == after["base"]["sha"],
            "Base changed during policy observation; refresh preview"
        );
        let mut observation = PolicyObservation {
            published: published.clone(),
            base_head: after["base"]["sha"]
                .as_str()
                .context("Unknown base")?
                .into(),
            observed_at: chrono::Utc::now(),
            rules,
            classic,
            digest: String::new(),
        };
        observation.digest = observation.computed_digest()?;
        Ok(observation)
    }
    pub fn prepare_land(
        &self,
        observation: PolicyObservation,
        approval: LandApproval,
    ) -> Result<LandIntent> {
        ensure!(
            observation.computed_digest()? == observation.digest
                && approval.policy_digest == observation.digest
                && approval.current_base_and_rules
                && approval.exclusive_pr_custody,
            "Separate exact-head/current-rules Land approval required"
        );
        validate_policy(&observation.rules, &observation.classic)?;
        Ok(LandIntent {
            attempt: uuid::Uuid::new_v4().to_string(),
            observation,
            approval,
            async_uuid: None,
        })
    }
    /// Submit exactly once after persisting intent. Lost acknowledgment is never replayed.
    /// Requires human-confirmed exclusive custody. Pre/post observations detect drift,
    /// but cannot atomically prevent concurrent retargeting or stacking on GitHub.
    pub fn land(&self, intent: &LandIntent) -> Outcome<LandProgress> {
        if let Err(error) = intent.validate() {
            return Outcome::Refused(error.to_string());
        }
        if intent.async_uuid.is_some() {
            return Outcome::Refused("Already submitted; reconcile only".into());
        }
        let preflight = (|| {
            let current = self.observe_policy(&intent.observation.published)?;
            // The approval deliberately allows current base and policy, not a snapshot CAS.
            ensure!(
                current.published.head == intent.observation.published.head,
                "Head changed"
            );
            Ok(())
        })();
        if let Err(error) = preflight {
            return Outcome::Refused(format!("{error}"));
        }
        let result = (|| {
            let published = &intent.observation.published;
            let response=self.request(&published.destination,Method::Put,&format!("/pulls/{}/merge-async",published.identity.number),Some(json!({"sha":published.head,"merge_method":"squash","merge_action":"direct_merge","bypass_rules":false})))?;
            ensure!(
                matches!(response.status, 200 | 202) && !response.next_page,
                "Merge not confirmed ({}); inspect only",
                response.status
            );
            self.settle_async(intent, decode_async(&response.body)?)
        })();
        match result {
            Ok(value) => Outcome::Confirmed(value),
            Err(error) => Outcome::Unknown(error.to_string()),
        }
    }
    fn settle_async(&self, intent: &LandIntent, result: AsyncResult) -> Result<LandProgress> {
        match result {
            AsyncResult::Pending(details) => {
                ensure!(
                    details.merge_method == "squash"
                        && details.merge_action == "direct_merge"
                        && details.expected_head_sha == intent.observation.published.head
                        && details.bypass_rules != Some(true),
                    "Async merge options missing/different; inspect only"
                );
                ensure!(
                    uuid::Uuid::parse_str(&details.uuid)?.to_string() == details.uuid,
                    "Unknown async request UUID"
                );
                let mut next = intent.clone();
                next.async_uuid = Some(details.uuid);
                Ok(LandProgress::Pending(Box::new(next)))
            }
            AsyncResult::Merged(sha) => Ok(LandProgress::Landed(Box::new(
                self.landed_receipt(intent, Some(&sha))?,
            ))),
            AsyncResult::Enqueued => {
                bail!("Queue admission is not landing; no queue fallback authorized")
            }
            AsyncResult::Failed => bail!("GitHub merge failed; no retry/fallback authorized"),
        }
    }
    fn landed_receipt(
        &self,
        intent: &LandIntent,
        expected_merge: Option<&str>,
    ) -> Result<LandedSource> {
        intent.validate()?;
        let published = &intent.observation.published;
        self.repository(&published.destination)?;
        let actual = self.actual_pr(published, false)?;
        ensure!(
            actual["merged"] == true
                && actual["state"] == "closed"
                && actual["merged_at"]
                    .as_str()
                    .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()),
            "Actual PR not proven merged"
        );
        ensure!(
            actual["merged_by"]["id"].as_u64() == Some(published.actor.id),
            "PR custody/merging actor changed"
        );
        let merged = actual["merge_commit_sha"]
            .as_str()
            .context("Missing actual merged commit")?;
        oid(merged)?;
        ensure!(
            expected_merge.is_none_or(|expected| expected == merged),
            "Async/PR merged commit mismatch"
        );
        let commit: Value = self.get(&published.destination, &format!("/git/commits/{merged}"))?;
        ensure!(
            commit["sha"].as_str() == Some(merged)
                && commit["parents"].as_array().is_some_and(|p| p.len() == 1),
            "Merged commit is not the supported squash result"
        );
        let tree = commit["tree"]["sha"]
            .as_str()
            .context("Missing landed source tree")?;
        ensure!(
            self.skill_tree(&published.destination, tree, &published.skill_path)?
                == published.skill_tree,
            "Actual landed skill tree differs from approved skill"
        );
        Ok(LandedSource {
            identity: published.identity.clone(),
            approved_head: published.head.clone(),
            merged_commit: merged.into(),
            source_tree: tree.into(),
            skill_path: published.skill_path.clone(),
            skill_tree: published.skill_tree.clone(),
            content_digest: published.content_digest.clone(),
            land_intent_digest: digest(intent)?,
            policy_digest: intent.observation.digest.clone(),
            api_version: API_VERSION.into(),
        })
    }
    /// Read-only reconciliation. Even a missing/expired async UUID never resubmits merge.
    pub fn reconcile_land(&self, intent: &LandIntent) -> Outcome<LandProgress> {
        let result = (|| {
            intent.validate()?;
            if let Some(uuid) = &intent.async_uuid {
                ensure!(
                    uuid::Uuid::parse_str(uuid)?.to_string() == *uuid,
                    "Invalid async UUID"
                );
                let published = &intent.observation.published;
                let response = self.request(
                    &published.destination,
                    Method::Get,
                    &format!("/pulls/{}/merge-async/{uuid}", published.identity.number),
                    None,
                )?;
                if response.status == 200 && !response.next_page {
                    return self.settle_async(intent, decode_async(&response.body)?);
                }
            }
            Ok(LandProgress::Landed(Box::new(
                self.landed_receipt(intent, None)?,
            )))
        })();
        match result {
            Ok(value) => Outcome::Confirmed(value),
            Err(error) => Outcome::Unknown(error.to_string()),
        }
    }
    /// Bounded polling of persisted pending intent; no merge writes or unbounded retry loop.
    pub fn poll_land(&self, intent: &LandIntent) -> Outcome<LandProgress> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut current = intent.clone();
        for _ in 0..10 {
            match self.reconcile_land(&current) {
                Outcome::Confirmed(LandProgress::Pending(next)) => current = *next,
                result => return result,
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Outcome::Confirmed(LandProgress::Pending(Box::new(current)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AsyncEnvelope {
    status: String,
    details: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingDetails {
    message: String,
    uuid: String,
    merge_method: String,
    merge_action: String,
    expected_head_sha: String,
    bypass_rules: Option<bool>,
}
pub(super) enum AsyncResult {
    Pending(PendingDetails),
    Merged(String),
    Enqueued,
    Failed,
}
pub(super) fn decode_async(bytes: &[u8]) -> Result<AsyncResult> {
    let envelope: AsyncEnvelope = serde_json::from_slice(bytes)?;
    match envelope.status.as_str() {
        "pending" => {
            let details: PendingDetails = serde_json::from_value(envelope.details)?;
            bounded_text(&details.message, 4096)?;
            Ok(AsyncResult::Pending(details))
        }
        "merged" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Merged {
                message: String,
                sha: String,
            }
            let details: Merged = serde_json::from_value(envelope.details)?;
            bounded_text(&details.message, 4096)?;
            oid(&details.sha)?;
            Ok(AsyncResult::Merged(details.sha))
        }
        "enqueued" | "failed" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Message {
                message: String,
            }
            let details: Message = serde_json::from_value(envelope.details)?;
            bounded_text(&details.message, 4096)?;
            if envelope.status == "enqueued" {
                Ok(AsyncResult::Enqueued)
            } else {
                Ok(AsyncResult::Failed)
            }
        }
        _ => bail!("Unknown async merge status"),
    }
}
