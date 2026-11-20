//! Breakout: opening Sessions as tiled terminal surfaces outside the aoe TUI.

#[cfg(target_os = "macos")]
pub mod ghostty;
pub mod grid;
pub mod request;

#[cfg(target_os = "macos")]
use request::BreakoutLayout;
use request::BreakoutRequest;

/// What a Breakout did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BreakoutOutcome {
    pub opened: usize,
    pub tiled: bool,
    /// Window titles, kept so the windows can be closed later.
    pub titles: Vec<String>,
    /// Something the user should know, such as a missing permission.
    pub note: Option<String>,
}

/// Whether this machine can open Breakout surfaces itself.
pub fn supported_here() -> bool {
    #[cfg(target_os = "macos")]
    {
        ghostty::ghostty_present()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Validate the request and open its surfaces on this machine.
pub fn run_local(request: &BreakoutRequest) -> Result<BreakoutOutcome, String> {
    request.validate()?;
    #[cfg(target_os = "macos")]
    {
        let exe =
            std::env::current_exe().map_err(|e| format!("cannot find the aoe binary: {e}"))?;
        let surfaces = request::surfaces(request, &exe.to_string_lossy());
        match request.layout {
            // Splits falls back to windows until the splits driver replaces this arm.
            BreakoutLayout::Windows | BreakoutLayout::Splits => ghostty::run_windows(&surfaces),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Breakout needs macOS with Ghostty".to_string())
    }
}

/// Close windows a Breakout opened on this machine.
pub fn close_local(titles: &[String]) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        ghostty::close(titles)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = titles;
        Err("Breakout needs macOS with Ghostty".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::request::{BreakoutLayout, BreakoutSession, PROTOCOL};
    use super::*;

    #[test]
    fn run_local_validates_before_doing_anything() {
        let bad = BreakoutRequest {
            protocol: PROTOCOL,
            nonce: "not-hex".to_string(),
            host: None,
            layout: BreakoutLayout::Windows,
            sessions: vec![BreakoutSession {
                id: "-x".to_string(),
                title: "t".to_string(),
                profile: String::new(),
            }],
        };
        assert_eq!(run_local(&bad), Err("malformed nonce".to_string()));
    }
}
