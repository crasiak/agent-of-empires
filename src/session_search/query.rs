//! Typed `session-search query --json` results (contract `session-search-query-v1`).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

const CONTRACT: &str = "session-search-query-v1";
const LIMIT: &str = "50";

pub enum Scope {
    All,
    /// Sessions in this repo.
    Here(String),
    Recent,
}

pub struct Request {
    pub generation: u64,
    pub text: String,
    pub scope: Scope,
}

pub struct Hit {
    pub session: Session,
    pub snippets: Vec<Snippet>,
}

pub struct Session {
    pub session_key: String,
    pub title: Option<String>,
    pub harness: String,
    pub repo: String,
    pub last_ts: String,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub profile: Option<String>,
    pub transcript_path: Option<String>,
    pub run_id: Option<String>,
    pub harness_session_id: Option<String>,
    pub aoe_instance_id: Option<String>,
    pub resume: Option<Resume>,
}

#[derive(Deserialize)]
pub struct Resume {
    pub kind: String,
    pub command: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub jump: Option<String>,
}

pub struct Snippet {
    pub role: String,
    pub ts: String,
    pub text: String,
    pub redactions: u32,
}

#[derive(Debug)]
pub enum QueryError {
    NotInstalled,
    NotRunning(String),
    Timeout,
    BadOutput(String),
    Failed(String),
}

#[derive(Deserialize)]
struct Wire {
    contract: String,
    hits: Vec<WireHit>,
}

#[derive(Deserialize)]
struct WireHit {
    session: WireSession,
    #[serde(default)]
    chunks: Vec<WireChunk>,
}

#[derive(Deserialize)]
struct WireSession {
    session_key: String,
    #[serde(default)]
    session_ref: WireRef,
    #[serde(default)]
    title: Option<String>,
    harness: String,
    repo: String,
    last_ts: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(default)]
    resume: Option<Resume>,
}

#[derive(Deserialize, Default)]
struct WireRef {
    #[serde(default)]
    harness_session_id: Option<String>,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    aoe_instance_id: Option<String>,
}

#[derive(Deserialize)]
struct WireChunk {
    snippet: String,
    role: String,
    ts: String,
    #[serde(default)]
    redactions: u32,
}

pub fn parse(stdout: &[u8]) -> Result<Vec<Hit>, QueryError> {
    let wire: Wire =
        serde_json::from_slice(stdout).map_err(|e| QueryError::BadOutput(e.to_string()))?;
    if wire.contract != CONTRACT {
        return Err(QueryError::BadOutput(format!(
            "unexpected contract {:?}",
            wire.contract
        )));
    }
    Ok(wire
        .hits
        .into_iter()
        .map(|hit| {
            let s = hit.session;
            Hit {
                session: Session {
                    session_key: s.session_key,
                    title: s.title,
                    harness: s.harness,
                    repo: s.repo,
                    last_ts: s.last_ts,
                    cwd: s.cwd,
                    branch: s.branch,
                    profile: s.profile,
                    transcript_path: s.transcript_path,
                    run_id: s.session_ref.run_id,
                    harness_session_id: s.session_ref.harness_session_id,
                    aoe_instance_id: s.session_ref.aoe_instance_id,
                    resume: s.resume,
                },
                snippets: hit
                    .chunks
                    .into_iter()
                    .map(|c| Snippet {
                        role: c.role,
                        ts: c.ts,
                        text: c.snippet,
                        redactions: c.redactions,
                    })
                    .collect(),
            }
        })
        .collect())
}

pub struct Client {
    pub bin: PathBuf,
    pub timeout: Duration,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self {
            bin: PathBuf::from("session-search"),
            timeout: Duration::from_secs(5),
        }
    }

    pub fn args(req: &Request) -> Vec<String> {
        let mut args: Vec<String> = ["query", "--json", "--limit", LIMIT]
            .map(String::from)
            .into();
        match &req.scope {
            Scope::All => {}
            Scope::Here(repo) => args.extend([
                "--lens".into(),
                "here".into(),
                "--param".into(),
                format!("repo={repo}"),
            ]),
            Scope::Recent => args.extend(["--lens".into(), "recent".into()]),
        }
        args.push(req.text.clone());
        args
    }

    pub fn run(&self, req: &Request) -> Result<Vec<Hit>, QueryError> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(Self::args(req)).stdin(std::process::Stdio::null());
        // Not run_with_bounded_stdout: it discards stderr, which tells a stopped service apart.
        match crate::process::run_with_timeout_process_group(&mut cmd, self.timeout) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(QueryError::NotInstalled),
            Err(e) => Err(QueryError::Failed(e.to_string())),
            Ok(None) => Err(QueryError::Timeout),
            Ok(Some(out)) if out.status.success() => parse(&out.stdout),
            Ok(Some(out)) => {
                let first = String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();
                if first.contains("cannot reach the service") {
                    Err(QueryError::NotRunning(first))
                } else {
                    Err(QueryError::Failed(first))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_query_v1_and_keeps_way_back_fields() {
        let hits = parse(include_bytes!("testdata/query_v1.json")).unwrap();
        assert_eq!(hits.len(), 3);
        let c = &hits[0].session;
        assert_eq!(c.harness, "claude");
        assert_eq!(c.aoe_instance_id.as_deref(), Some("aoe1"));
        assert_eq!(c.run_id.as_deref(), Some("run_a"));
        assert_eq!(
            c.resume.as_ref().unwrap().session_id.as_deref(),
            Some("c-111")
        );
        assert_eq!(
            hits[2].session.resume.as_ref().unwrap().kind,
            "ledger-parent"
        );
        assert_eq!(hits[2].snippets[0].redactions, 1);
    }

    #[test]
    fn rejects_another_contract_and_garbage() {
        assert!(matches!(
            parse(br#"{"contract":"other","hits":[]}"#),
            Err(QueryError::BadOutput(_))
        ));
        assert!(matches!(parse(b"not json"), Err(QueryError::BadOutput(_))));
    }

    #[test]
    fn args_follow_the_scope() {
        let req = |scope| Request {
            generation: 1,
            text: "two words".into(),
            scope,
        };
        assert_eq!(
            Client::args(&req(Scope::All)),
            ["query", "--json", "--limit", "50", "two words"]
        );
        assert_eq!(
            Client::args(&req(Scope::Here("attic".into()))),
            [
                "query",
                "--json",
                "--limit",
                "50",
                "--lens",
                "here",
                "--param",
                "repo=attic",
                "two words"
            ]
        );
        assert_eq!(
            Client::args(&req(Scope::Recent)),
            [
                "query",
                "--json",
                "--limit",
                "50",
                "--lens",
                "recent",
                "two words"
            ]
        );
    }

    #[test]
    fn run_maps_cli_failures() {
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            p
        };
        let req = Request {
            generation: 1,
            text: "abc".into(),
            scope: Scope::All,
        };
        let client = |bin, ms| Client {
            bin,
            timeout: std::time::Duration::from_millis(ms),
        };
        assert!(matches!(
            client(dir.path().join("missing"), 2000).run(&req),
            Err(QueryError::NotInstalled)
        ));
        let down = script(
            "down",
            "echo 'session-search: cannot reach the service at /x/sock' >&2; exit 1",
        );
        assert!(matches!(
            client(down, 2000).run(&req),
            Err(QueryError::NotRunning(_))
        ));
        let slow = script("slow", "exec sleep 5");
        assert!(matches!(
            client(slow, 200).run(&req),
            Err(QueryError::Timeout)
        ));
        let ok = script(
            "ok",
            &format!(
                "cat {}",
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/session_search/testdata/query_v1.json"
                )
            ),
        );
        assert_eq!(client(ok, 2000).run(&req).unwrap().len(), 3);
    }
}
