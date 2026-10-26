//! Account-scoped resources permitted in capture profiles.

pub(crate) fn allows_capture_resource(key: &str, work: bool) -> bool {
    match key {
        "skill/capture-idea"
        | "skill/linear-project-work"
        | "skill/librarian"
        | "skill/artifacts"
        | "pi-package/pi-mcp-adapter" => true,
        "skill/jira" | "skill/confluence" => work,
        _ => key.starts_with("models/"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_resources_are_scoped_without_allowing_daily_bundles() {
        for (resource, personal, work) in [
            ("skill/capture-idea", true, true),
            ("skill/linear-project-work", true, true),
            ("skill/librarian", true, true),
            ("skill/artifacts", true, true),
            ("skill/jira", false, true),
            ("skill/confluence", false, true),
            ("models/capture", true, true),
            ("pi-package/pi-mcp-adapter", true, true),
            ("skill/worktree", false, false),
            ("skill/confluence-extra", false, false),
            ("pi-package/pi-subagents", false, false),
            ("settings/daily", false, false),
        ] {
            assert_eq!(
                allows_capture_resource(resource, false),
                personal,
                "{resource}"
            );
            assert_eq!(allows_capture_resource(resource, true), work, "{resource}");
        }
    }
}
