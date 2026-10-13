use super::*;

pub(in crate::session::afk) struct TargetFacts {
    pub hash: String,
    pub expires_at_ms: i64,
    pub ambiguous: bool,
}

impl Book {
    pub(in crate::session::afk) fn review_target(
        &self,
        window: &str,
        decision: Option<&str>,
    ) -> Result<TargetFacts> {
        let w = self
            .windows
            .iter()
            .find(|w| w.id == window)
            .context("unknown review window")?;
        let hash = match decision {
            Some(id) => w
                .records
                .iter()
                .find(|r| r.id == id)
                .context("unknown review decision")?
                .payload_hash
                .clone(),
            None => hash(serde_json::to_vec(&w.grant)?),
        };
        Ok(TargetFacts {
            hash,
            expires_at_ms: w.expires_at_ms,
            ambiguous: w.ambiguous(),
        })
    }

    pub(in crate::session::afk) fn review_window(&self, id: &str, now: i64) -> Result<Value> {
        let original = self
            .windows
            .iter()
            .find(|w| w.id == id)
            .context("unknown review window")?;
        let expired = original.bodies_expired(now);
        let mut w = original.clone();
        if expired {
            for record in &mut w.records {
                record.claims = None;
            }
            w.prune_checkpoint_bodies();
        }
        let mut view = w.view(now);
        let object = view.as_object_mut().context("invalid window projection")?;
        object.remove("binding");
        object.remove("generation");
        // Stored lifecycle and action observations remain independent of wall-clock expiry.
        object.insert("state".into(), json!(w.state));
        object.insert("expired".into(), json!(now >= w.expires_at_ms));
        object.insert("body_retention_elapsed".into(), json!(expired));
        object.insert("ambiguous".into(), json!(w.ambiguous()));
        object.insert(
            "unresolved_question_guard".into(),
            json!(w.unresolved_question()),
        );
        object.insert("unresolved_nudge".into(), json!(w.unresolved_nudge()));
        object.insert(
            "grant".into(),
            json!({
                "task":w.grant.task,"scope":w.grant.scope,"requests":w.grant.requests,
                "settlement_nudges":w.grant.settlement_nudges,
                "question_deferrals":w.grant.question_deferrals,"assurance":w.grant.assurance
            }),
        );
        if let Some(action) = view["action"].as_object_mut() {
            action.remove("path");
        }
        for record in view["records"]
            .as_array_mut()
            .context("invalid record projection")?
        {
            let retained = !record["claims"].is_null();
            record["body_status"] = json!(if retained {
                "retained"
            } else {
                "expired_or_unavailable"
            });
        }
        if let Some(checkpoints) = view["checkpoints"].as_array_mut() {
            for checkpoint in checkpoints {
                if let Some(claims) = checkpoint["claims"].as_object_mut() {
                    // A next-step path is structured workspace data, not review rationale.
                    claims.remove("next_step");
                }
            }
        }
        Ok(view)
    }
}
