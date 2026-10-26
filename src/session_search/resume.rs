//! Build the AoE instance that natively resumes a search hit.

use crate::session::config::SessionConfig;
use crate::session::{
    ConversationBinding, ConversationProvenance, ExecutionBinding, Instance, ResumeIntent,
};

use super::target::ResumeSpec;

/// Mirrors the terminal branch of `aoe session import`, for any resumable tool.
pub fn instance(spec: &ResumeSpec, session_config: &SessionConfig) -> Instance {
    let mut inst = Instance::new(&spec.title, &spec.cwd);
    inst.tool = spec.tool.clone();
    crate::session::builder::apply_agent_launch_config(&mut inst, session_config, "", "", None);
    inst.source_profile = spec.aoe_profile.clone();
    inst.resume_intent = ResumeIntent::Use(spec.session_id.clone());
    inst.resume_binding = Some(ConversationBinding {
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
    });
    inst
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
        }
    }

    #[test]
    fn instance_resumes_claude_and_pi_natively() {
        let cfg = SessionConfig::default();
        for (tool, id, store) in [
            ("claude", "c-123", "/home/u/.ledger/homes/work/claude"),
            ("pi", "p-456", "/home/u/.ledger/homes/work/pi"),
        ] {
            let inst = instance(&spec(tool, id, store), &cfg);
            assert_eq!(inst.tool, tool);
            assert_eq!(inst.title, "fix the parser");
            assert_eq!(inst.source_profile, "work");
            assert_eq!(inst.resume_intent, ResumeIntent::Use(id.into()));
            let binding = inst.resume_binding.expect("resume binding");
            assert_eq!(binding.session_id, id);
            let exec = binding.execution.expect("execution binding");
            assert_eq!(exec.agent, tool);
            assert_eq!(exec.stores, vec![PathBuf::from(store)]);
            assert_eq!(exec.cwd, PathBuf::from("/nonexistent/session-search/repo"));
        }
    }
}
