//! The Breakout request: which Sessions to open, in what layout, from which host.

use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 1;
/// More Sessions than this are refused.
pub const MAX_SESSIONS: usize = 9;
/// More Sessions than this ask the user to confirm first.
pub const CONFIRM_ABOVE: usize = 6;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakoutLayout {
    /// One window per Session, tiled across the screen.
    #[default]
    Windows,
    /// One window, split into a grid.
    Splits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakoutSession {
    pub id: String,
    pub title: String,
    /// Empty means the default profile.
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakoutRequest {
    pub protocol: u32,
    pub nonce: String,
    /// The host the Sessions run on when it is not this machine.
    pub host: Option<String>,
    pub layout: BreakoutLayout,
    pub sessions: Vec<BreakoutSession>,
}

/// One terminal surface to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    /// Unique window title, used to find the window again.
    pub title: String,
    /// Shell command line the surface runs.
    pub command: String,
}

/// A session id or profile name: `[A-Za-z0-9_][A-Za-z0-9_-]{0,63}`. No leading
/// `-`, so it can never be read as an option by `aoe` or `ssh`.
fn is_name(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes()[0] != b'-'
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A host name or address: `[A-Za-z0-9][A-Za-z0-9.-]{0,252}`.
fn is_host(value: &str) -> bool {
    (1..=253).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

impl BreakoutRequest {
    /// Reject anything that could not be turned into a safe command line.
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol != PROTOCOL {
            return Err(format!("unsupported protocol {}", self.protocol));
        }
        let nonce_ok = (8..=64).contains(&self.nonce.len())
            && self.nonce.bytes().all(|b| b.is_ascii_hexdigit());
        if !nonce_ok {
            return Err("malformed nonce".to_string());
        }
        if let Some(host) = &self.host {
            if !is_host(host) {
                return Err("malformed host".to_string());
            }
        }
        if self.sessions.is_empty() {
            return Err("no sessions".to_string());
        }
        if self.sessions.len() > MAX_SESSIONS {
            return Err(format!("more than {MAX_SESSIONS} sessions"));
        }
        let mut seen = std::collections::HashSet::new();
        for session in &self.sessions {
            if !is_name(&session.id) {
                return Err("malformed session id".to_string());
            }
            if !session.profile.is_empty() && !is_name(&session.profile) {
                return Err("malformed profile".to_string());
            }
            if !seen.insert(session.id.as_str()) {
                return Err("duplicate session".to_string());
            }
        }
        Ok(())
    }
}

/// Printable title text for a window: no control characters, at most 60
/// characters. A leading host tag character becomes its fullwidth form, so a
/// Session title can never make a local window look host-bound.
fn display_title(title: &str) -> String {
    let text: String = title.chars().filter(|c| !c.is_control()).take(60).collect();
    match text.strip_prefix(super::HOST_TAG) {
        Some(rest) => format!("\u{ff20}{rest}"),
        None => text,
    }
}

/// Quote one argument for `/bin/sh`.
fn sh_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The surfaces a validated request opens. `local_exe` is the running aoe binary,
/// used when the Sessions are on this machine. Call only after `validate` passed.
pub fn surfaces(request: &BreakoutRequest, local_exe: &str) -> Vec<Surface> {
    request
        .sessions
        .iter()
        .map(|session| Surface {
            title: super::window_title(
                request.host.as_deref(),
                &display_title(&session.title),
                &session.id,
            ),
            command: attach_command(request.host.as_deref(), session, local_exe),
        })
        .collect()
}

/// The one place the attach command line is built. Ids, profiles and hosts are
/// validated to a shell-safe alphabet with no leading `-`, so only the local
/// binary path needs quoting.
fn attach_command(host: Option<&str>, session: &BreakoutSession, local_exe: &str) -> String {
    let mut command = match host {
        Some(host) => format!("ssh -t {host} aoe"),
        None => sh_quote(local_exe),
    };
    if !session.profile.is_empty() {
        command.push_str(&format!(" --profile {}", session.profile));
    }
    command.push_str(&format!(" session attach {}", session.id));
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str, profile: &str) -> BreakoutSession {
        BreakoutSession {
            id: id.to_string(),
            title: title.to_string(),
            profile: profile.to_string(),
        }
    }

    fn request(host: Option<&str>, sessions: Vec<BreakoutSession>) -> BreakoutRequest {
        BreakoutRequest {
            protocol: PROTOCOL,
            nonce: "0123456789abcdef".to_string(),
            host: host.map(str::to_string),
            layout: BreakoutLayout::Windows,
            sessions,
        }
    }

    #[test]
    fn a_plain_request_validates() {
        let r = request(
            Some("bender"),
            vec![session("a1b2c3d4e5f60718", "Vikings", "work")],
        );
        assert_eq!(r.validate(), Ok(()));
        assert_eq!(
            request(None, vec![session("abc", "T", "")]).validate(),
            Ok(())
        );
    }

    #[test]
    fn unsafe_requests_are_rejected() {
        let ok = session("abc", "T", "work");
        let cases: Vec<(&str, BreakoutRequest)> = vec![
            ("no sessions", request(None, vec![])),
            ("too many", request(None, vec![ok.clone(); 10])),
            (
                "id with a space",
                request(None, vec![session("a b", "T", "")]),
            ),
            (
                "id with a quote",
                request(None, vec![session("a'b", "T", "")]),
            ),
            (
                "profile with a slash",
                request(None, vec![session("abc", "T", "a/b")]),
            ),
            (
                "host with a space",
                request(Some("bender; rm"), vec![ok.clone()]),
            ),
            (
                "host starting with a dash",
                request(Some("-oProxyCommand=x"), vec![ok.clone()]),
            ),
            ("empty host", request(Some(""), vec![ok.clone()])),
            (
                "short nonce",
                BreakoutRequest {
                    nonce: "abc".into(),
                    ..request(None, vec![ok.clone()])
                },
            ),
            (
                "non-hex nonce",
                BreakoutRequest {
                    nonce: "zzzzzzzzzz".into(),
                    ..request(None, vec![ok.clone()])
                },
            ),
            (
                "wrong protocol",
                BreakoutRequest {
                    protocol: 2,
                    ..request(None, vec![ok.clone()])
                },
            ),
            ("duplicate ids", request(None, vec![ok.clone(), ok.clone()])),
        ];
        for (name, r) in cases {
            assert!(r.validate().is_err(), "{name}");
        }
    }

    /// Values that must never reach a command line as an id, profile or host.
    const HOSTILE: &[&str] = &[
        "-x",
        "--help",
        "-oProxyCommand=touch /tmp/p",
        "a b",
        "a\tb",
        "a\nb",
        "a\rb",
        "a\0b",
        "a'b",
        "a\"b",
        "a\\b",
        "$(id)",
        "`id`",
        "a;b",
        "a|b",
        "a&b",
        "a>b",
        "a*",
        "~root",
        "a=b",
        "a@b",
        "a:b",
        "a/b",
        "caf\u{e9}",
        "\u{ff41}bc",
        "a\u{200b}b",
        "",
    ];

    #[test]
    fn hostile_ids_are_rejected() {
        for id in HOSTILE {
            let r = request(None, vec![session(id, "T", "")]);
            assert!(r.validate().is_err(), "{id:?}");
        }
    }

    #[test]
    fn hostile_profiles_are_rejected() {
        // An empty profile is the default profile, not a hostile one.
        for profile in HOSTILE.iter().filter(|p| !p.is_empty()) {
            let r = request(None, vec![session("abc", "T", profile)]);
            assert!(r.validate().is_err(), "{profile:?}");
        }
    }

    #[test]
    fn hostile_hosts_are_rejected() {
        // Dots are allowed in a host, so only check values with something else.
        for host in HOSTILE.iter().filter(|h| !h.contains('.')) {
            let r = request(Some(host), vec![session("abc", "T", "")]);
            assert!(r.validate().is_err(), "{host:?}");
        }
        for host in [".bender", "-bender", "_bender", "bender_1", "user@bender"] {
            let r = request(Some(host), vec![session("abc", "T", "")]);
            assert!(r.validate().is_err(), "{host:?}");
        }
    }

    #[test]
    fn hostile_nonces_are_rejected() {
        for nonce in HOSTILE.iter().chain(&["0123456", "0123456g", "0123 4567"]) {
            let r = BreakoutRequest {
                nonce: (*nonce).to_string(),
                ..request(None, vec![session("abc", "T", "")])
            };
            assert!(r.validate().is_err(), "{nonce:?}");
        }
    }

    #[test]
    fn field_lengths_are_bounded() {
        let at = |n: usize| "a".repeat(n);
        let id = |id: &str| request(None, vec![session(id, "T", "")]).validate();
        let profile = |p: &str| request(None, vec![session("abc", "T", p)]).validate();
        let host = |h: &str| request(Some(h), vec![session("abc", "T", "")]).validate();
        let nonce = |n: &str| {
            BreakoutRequest {
                nonce: n.to_string(),
                ..request(None, vec![session("abc", "T", "")])
            }
            .validate()
        };
        assert_eq!(id(&at(64)), Ok(()));
        assert!(id(&at(65)).is_err());
        assert_eq!(profile(&at(64)), Ok(()));
        assert!(profile(&at(65)).is_err());
        assert_eq!(host(&at(253)), Ok(()));
        assert!(host(&at(254)).is_err());
        assert_eq!(nonce(&at(8)), Ok(()));
        assert_eq!(nonce(&at(64)), Ok(()));
        assert!(nonce(&at(65)).is_err());
        assert!(nonce(&at(7)).is_err());
    }

    #[test]
    fn ordinary_names_still_validate() {
        let r = request(
            Some("bender.lan"),
            vec![
                session("a1b2-c3d4_e5", "T", "my-work_2"),
                session("_private", "T", "_p"),
                session("ABC123", "T", "Default"),
            ],
        );
        assert_eq!(r.validate(), Ok(()));
        let splits = BreakoutRequest {
            layout: BreakoutLayout::Splits,
            ..request(Some("10.0.0.7"), vec![session("abc", "T", "")])
        };
        assert_eq!(splits.validate(), Ok(()));
    }

    #[test]
    fn the_size_limits_are_inclusive() {
        let many = |n: usize| {
            (0..n)
                .map(|i| session(&format!("s{i}"), "T", ""))
                .collect::<Vec<_>>()
        };
        assert_eq!(request(None, many(MAX_SESSIONS)).validate(), Ok(()));
        assert!(request(None, many(MAX_SESSIONS + 1)).validate().is_err());
    }

    #[test]
    fn unknown_fields_are_rejected_on_the_wire() {
        let json = r#"{"protocol":1,"nonce":"0123456789abcdef","host":null,"layout":"windows","sessions":[],"command":"rm -rf /"}"#;
        assert!(serde_json::from_str::<BreakoutRequest>(json).is_err());
        let json = r#"{"protocol":1,"nonce":"0123456789abcdef","host":null,"layout":"windows","sessions":[{"id":"abc","title":"T","profile":"","command":"x"}]}"#;
        assert!(serde_json::from_str::<BreakoutRequest>(json).is_err());
    }

    #[test]
    fn the_wire_format_round_trips() {
        let json = r#"{"protocol":1,"nonce":"0123456789abcdef","host":"bender","layout":"splits","sessions":[{"id":"abc","title":"T","profile":"work"}]}"#;
        let r: BreakoutRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.layout, BreakoutLayout::Splits);
        assert_eq!(r.host.as_deref(), Some("bender"));
        assert_eq!(serde_json::to_string(&r).unwrap(), json);
    }

    #[test]
    fn local_surfaces_run_this_binary() {
        let r = request(
            None,
            vec![
                session("a1b2c3d4e5f60718", "Vikings", "work"),
                session("ffff", "Celts", ""),
            ],
        );
        let got = surfaces(&r, "/Users/me/my bin/aoe");
        assert_eq!(
            got[0].command,
            "'/Users/me/my bin/aoe' --profile work session attach a1b2c3d4e5f60718"
        );
        assert_eq!(got[0].title, "aoe breakout \u{b7} Vikings \u{b7} a1b2c3d4");
        assert_eq!(
            got[1].command, "'/Users/me/my bin/aoe' session attach ffff",
            "default profile needs no flag"
        );
        assert_eq!(got[1].title, "aoe breakout \u{b7} Celts \u{b7} ffff");
    }

    #[test]
    fn every_title_starts_with_the_title_prefix() {
        let r = request(None, vec![session("abc", "T", "")]);
        let title = &surfaces(&r, "aoe")[0].title;
        assert!(title.starts_with(super::super::TITLE_PREFIX), "{title}");
        assert_eq!(super::super::TITLE_PREFIX, "aoe breakout \u{b7} ");
    }

    #[test]
    fn remote_surfaces_go_through_ssh() {
        let r = request(
            Some("bender"),
            vec![session("a1b2c3d4e5f60718", "Vikings", "work")],
        );
        let got = surfaces(&r, "/ignored/aoe");
        assert_eq!(
            got[0].command,
            "ssh -t bender aoe --profile work session attach a1b2c3d4e5f60718"
        );
    }

    #[test]
    fn a_binary_path_with_a_single_quote_is_quoted() {
        let r = request(None, vec![session("abc", "T", "")]);
        let got = surfaces(&r, "/Users/o'brien/bin/aoe");
        assert_eq!(
            got[0].command,
            r"'/Users/o'\''brien/bin/aoe' session attach abc"
        );
    }

    #[cfg(unix)]
    #[test]
    fn quoted_binary_paths_survive_the_shell_unchanged() {
        for path in [
            "/Users/me/my bin/aoe",
            "/Users/o'brien/aoe",
            "/tmp/''/aoe",
            "/tmp/$(touch x)/`id`/aoe",
            "/tmp/a\"b\\c/aoe",
            "/tmp/new\nline/aoe",
            "/tmp/caf\u{e9}/aoe",
            "-rf",
        ] {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf %s {}", sh_quote(path)))
                .output()
                .unwrap();
            assert!(out.status.success(), "{path:?}");
            assert_eq!(String::from_utf8(out.stdout).unwrap(), path);
        }
    }

    #[test]
    fn the_title_never_reaches_the_command() {
        let title = "$(touch /tmp/p) `id` ; rm -rf / '\"";
        let r = request(Some("bender"), vec![session("abc", title, "work")]);
        let got = surfaces(&r, "aoe");
        assert_eq!(
            got[0].command,
            "ssh -t bender aoe --profile work session attach abc"
        );
        assert_eq!(
            got[0].title,
            format!("aoe breakout \u{b7} @bender \u{b7} {title} \u{b7} abc")
        );
    }

    #[test]
    fn titles_are_made_printable() {
        let r = request(None, vec![session("abc", "a\u{1b}[31mb\n", "")]);
        assert_eq!(
            surfaces(&r, "aoe")[0].title,
            "aoe breakout \u{b7} a[31mb \u{b7} abc"
        );
    }

    #[test]
    fn long_titles_are_cut_by_character() {
        let r = request(None, vec![session("abc", &"\u{e9}".repeat(100), "")]);
        let want = format!("aoe breakout \u{b7} {} \u{b7} abc", "\u{e9}".repeat(60));
        assert_eq!(surfaces(&r, "aoe")[0].title, want);
    }
}
