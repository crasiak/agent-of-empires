//! Build the AoE instance that natively resumes a search hit.

use anyhow::Context;

use crate::session::config::SessionConfig;
use crate::session::{
    ConversationBinding, ConversationProvenance, ExecutionBinding, Instance, ResumeIntent,
};

use super::target::ResumeSpec;

/// Mirrors the terminal branch of `aoe session import` for claude and pi.
/// Pi's binding is asserted from its exact transcript, as `set-session-id` does.
pub fn instance(spec: &ResumeSpec, session_config: &SessionConfig) -> anyhow::Result<Instance> {
    let mut inst = Instance::new(&spec.title, &spec.cwd);
    inst.tool = spec.tool.clone();
    crate::session::builder::apply_agent_launch_config(&mut inst, session_config, "", "", None);
    inst.source_profile = spec.aoe_profile.clone();
    let binding = if spec.tool == "pi" {
        let transcript = spec
            .transcript_path
            .as_deref()
            .context("Pi resume requires its exact transcript path")?;
        inst.asserted_resume_binding(&spec.session_id, Some(transcript))?
    } else {
        ConversationBinding {
            session_id: spec.session_id.clone(),
            execution: Some(ExecutionBinding {
                agent: spec.tool.clone(),
                stores: vec![spec.store.clone()],
                configuration: Vec::new(),
                cwd: crate::session::capture::canonicalize_or_raw(&spec.cwd),
                filesystem: "host".into(),
                cwd_filesystem: "host".into(),
                exported_default_store: None,
            }),
            provenance: ConversationProvenance::Imported,
            transcript_path: None,
        }
    };
    inst.resume_intent = ResumeIntent::Use(spec.session_id.clone());
    inst.resume_binding = Some(binding);
    Ok(inst)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn spec(tool: &str, id: &str, store: &str) -> ResumeSpec {
        ResumeSpec {
            tool: tool.into(),
            cwd: "/nonexistent/session-search/repo".into(),
            session_id: id.into(),
            aoe_profile: "work".into(),
            store: PathBuf::from(store),
            title: "fix the parser".into(),
            transcript_path: None,
        }
    }

    #[test]
    fn claude_resumes_from_its_store_and_pi_needs_a_transcript() {
        let cfg = SessionConfig::default();
        let store = "/home/u/.ledger/homes/work/claude";
        let inst = instance(&spec("claude", "c-123", store), &cfg).unwrap();
        assert_eq!(inst.tool, "claude");
        assert_eq!(inst.title, "fix the parser");
        assert_eq!(inst.source_profile, "work");
        assert_eq!(inst.resume_intent, ResumeIntent::Use("c-123".into()));
        let binding = inst.resume_binding.expect("resume binding");
        assert_eq!(binding.session_id, "c-123");
        assert_eq!(binding.provenance, ConversationProvenance::Imported);
        let exec = binding.execution.expect("execution binding");
        assert_eq!(exec.agent, "claude");
        assert_eq!(exec.stores, vec![PathBuf::from(store)]);
        assert_eq!(exec.cwd, PathBuf::from("/nonexistent/session-search/repo"));

        let pi = spec("pi", "p-456", "/home/u/.ledger/homes/work/pi");
        let error = instance(&pi, &cfg).expect_err("Pi without a transcript");
        assert!(
            error.to_string().contains("exact transcript path"),
            "{error}"
        );
    }
}
