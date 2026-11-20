//! Breakout: opening Sessions as tiled terminal surfaces outside the aoe TUI.

#[cfg(target_os = "macos")]
pub mod ghostty;
pub mod grid;
pub mod request;

use request::{BreakoutLayout, BreakoutRequest};

/// Every Breakout window title starts with this. It is the only tag: the sweep
/// finds windows by it, including ones no running TUI tracks.
pub const TITLE_PREFIX: &str = "aoe breakout \u{b7} ";

/// Separates a window title's display text from its trailing id characters.
pub(crate) const TITLE_SEPARATOR: &str = " \u{b7} ";

/// The id characters a window title ends with.
pub fn id_prefix(id: &str) -> String {
    id.chars().take(8).collect()
}

/// The window title for a Session, from its printable title text and id.
pub(crate) fn window_title(display_title: &str, id: &str) -> String {
    format!(
        "{TITLE_PREFIX}{display_title}{TITLE_SEPARATOR}{}",
        id_prefix(id)
    )
}

/// The text a window title of the Session with these id characters ends with.
/// Ids use a validated alphabet without spaces, so only that Session's titles match.
pub(crate) fn title_suffix(id_prefix: &str) -> String {
    format!("{TITLE_SEPARATOR}{id_prefix}")
}

/// Follows `TITLE_PREFIX` in the title of a splits window, before its ids.
const SPLITS_TAG: &str = "splits";

/// How every splits window title starts.
pub(crate) fn splits_head() -> String {
    format!("{TITLE_PREFIX}{SPLITS_TAG}{TITLE_SEPARATOR}")
}

/// The title of one window holding several Sessions: the splits head, then each
/// Session's id characters, separated like the end of a window title.
pub(crate) fn splits_title<'a>(ids: impl IntoIterator<Item = &'a str>) -> String {
    let prefixes: Vec<String> = ids.into_iter().map(id_prefix).collect();
    format!("{}{}", splits_head(), prefixes.join(TITLE_SEPARATOR))
}

/// Whether `title` is a splits window, which other Sessions may share.
pub fn is_splits_title(title: &str) -> bool {
    title.starts_with(&splits_head())
}

/// Whether `title` is a Breakout window title of the Session `id`.
pub fn title_is_for(title: &str, id: &str) -> bool {
    if let Some(ids) = title.strip_prefix(&splits_head()) {
        let prefix = id_prefix(id);
        return ids.split(TITLE_SEPARATOR).any(|part| part == prefix);
    }
    title.starts_with(TITLE_PREFIX) && title.ends_with(&title_suffix(&id_prefix(id)))
}

/// The titles of the windows a validated request opens: one per Session, or one
/// splits window for all of them.
pub fn window_titles(request: &BreakoutRequest) -> Vec<String> {
    match request.layout {
        BreakoutLayout::Windows => request::surfaces(request, "")
            .into_iter()
            .map(|surface| surface.title)
            .collect(),
        BreakoutLayout::Splits => vec![splits_title(
            request.sessions.iter().map(|session| session.id.as_str()),
        )],
    }
}

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
    #[cfg(test)]
    if let Some(forced) = test_support::FORCED.with(|f| f.get()) {
        return forced;
    }
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
            BreakoutLayout::Windows => ghostty::run_windows(&surfaces),
            BreakoutLayout::Splits => {
                let [title] = window_titles(request)
                    .try_into()
                    .map_err(|_| "a splits Breakout has one window".to_string())?;
                ghostty::run_splits(&surfaces, &title)
            }
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

/// Close every Breakout window on this machine whose trailing id characters are
/// not in `keep_id_prefixes`, tracked or not. Returns how many it closed.
pub fn sweep_local(keep_id_prefixes: &[String]) -> Result<usize, String> {
    #[cfg(target_os = "macos")]
    {
        ghostty::sweep(keep_id_prefixes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = keep_id_prefixes;
        Err("Breakout needs macOS with Ghostty".to_string())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::cell::Cell;

    thread_local! {
        /// Forces `supported_here` on this thread so TUI tests do not depend on Ghostty.
        pub(crate) static FORCED: Cell<Option<bool>> = const { Cell::new(None) };
    }
}

#[cfg(test)]
mod tests {
    use super::request::{BreakoutSession, PROTOCOL};
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

    #[test]
    fn titles_match_only_their_own_session() {
        let title = window_title("Vikings", "a1b2c3d4e5f60718");
        assert_eq!(title, "aoe breakout \u{b7} Vikings \u{b7} a1b2c3d4");
        assert!(title_is_for(&title, "a1b2c3d4e5f60718"));
        assert!(
            title_is_for(&title, "a1b2c3d4ffff"),
            "only 8 id characters are kept"
        );
        assert!(!title_is_for(&title, "b2c3d4"));
        assert!(!title_is_for(&title, "1b2c3d4"));
        let short = window_title("Celts", "ffff");
        assert!(title_is_for(&short, "ffff"));
        assert!(!title_is_for(&short, "ffffabcd"));
        assert!(!title_is_for("Vikings \u{b7} a1b2c3d4", "a1b2c3d4"));
    }

    fn request(layout: BreakoutLayout, ids: &[&str]) -> BreakoutRequest {
        BreakoutRequest {
            protocol: PROTOCOL,
            nonce: "abcdef12".to_string(),
            host: None,
            layout,
            sessions: ids
                .iter()
                .map(|id| BreakoutSession {
                    id: id.to_string(),
                    title: format!("T{id}"),
                    profile: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_splits_title_belongs_to_every_session_in_it() {
        let title = splits_title(["a1b2c3d4e5", "ffff", "0011223344"]);
        assert_eq!(
            title,
            "aoe breakout \u{b7} splits \u{b7} a1b2c3d4 \u{b7} ffff \u{b7} 00112233"
        );
        assert!(title.starts_with(TITLE_PREFIX));
        assert!(is_splits_title(&title));
        for id in ["a1b2c3d4", "ffff", "00112233aa"] {
            assert!(title_is_for(&title, id), "{id}");
        }
        assert!(!title_is_for(&title, "fff"));
        assert!(!title_is_for(&title, "ffffa"));
        assert!(!title_is_for(&title, "splits"));
        assert!(!is_splits_title(&window_title("Vikings", "a1b2c3d4")));
    }

    #[test]
    fn window_titles_follow_the_layout() {
        let ids = ["a1b2c3d4", "b2c3d4e5", "c3d4e5f6"];
        let windows = window_titles(&request(BreakoutLayout::Windows, &ids));
        assert_eq!(windows.len(), 3);
        assert!(windows.iter().zip(ids).all(|(t, id)| title_is_for(t, id)));
        let splits = window_titles(&request(BreakoutLayout::Splits, &ids));
        assert_eq!(splits, [splits_title(ids)]);
    }
}
