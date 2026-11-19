//! `agent-of-empires send` subcommand implementation

use anyhow::{bail, Result};
use clap::Args;
use std::io::Read;

use crate::acp::client::http::PromptDispositionWire;
use crate::acp::client::{require_daemon, HttpClient, HttpError};
use crate::session::{StartBlocked, View};

#[derive(Args)]
pub struct SendArgs {
    /// Session ID or title
    identifier: String,

    /// Message to send to the agent
    #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
    message: Option<String>,

    /// Read UTF-8 feedback from stdin (at most 128 KiB)
    #[arg(long)]
    stdin: bool,

    /// Match only a full session ID in the explicitly selected profile
    #[arg(long)]
    exact: bool,

    /// Refuse terminal input unless a verified agent is positively observed idle
    #[arg(long)]
    require_idle: bool,

    /// Fail loud on dead/stopped sessions instead of auto-respawning. Default
    /// behavior is to revive the session so a `send` after a crash or stop
    /// just works; pass this for scripts that want the previous bail-out.
    #[arg(long = "no-revive")]
    no_revive: bool,
}

#[tracing::instrument(target = "cli.send", skip_all, fields(profile = %profile))]
pub async fn run(profile: &str, args: SendArgs) -> Result<()> {
    if args.exact && profile.is_empty() {
        bail!("Exact send requires a non-empty explicit profile");
    }
    let message = if args.stdin {
        read_feedback(std::io::stdin().lock())?
    } else {
        args.message.clone().expect("clap requires a message")
    };
    if message.trim().is_empty() {
        bail!("Message cannot be empty");
    }

    let client = HttpClient::new(require_daemon().await?)?;
    let profile = resolve_profile(&client, profile).await?;
    let sessions = client.list_sessions(None).await?.sessions;
    let sessions: Vec<_> = sessions.iter().filter(|s| s.profile == profile).collect();
    let session = resolve_target(&args.identifier, &sessions, args.exact)?;

    let result = if session.view == View::Structured {
        client
            .prompt(&session.id, &message, args.no_revive)
            .await
            .map(|dispatch| match dispatch.disposition {
                PromptDispositionWire::Sent => "Sent",
                PromptDispositionWire::Steered => "Steered into",
                PromptDispositionWire::Queued => "Queued",
            })
    } else {
        client
            .send_message_with_idle_guard(&session.id, &message, args.no_revive, args.require_idle)
            .await
            .map(|()| "Sent")
    };
    let verb = result.map_err(|e| refusal(e, &args.identifier))?;
    println!("{verb} message to '{}'", session.title);
    Ok(())
}

pub(crate) const MAX_FEEDBACK_BYTES: usize = 128 * 1024;

fn read_feedback(reader: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_FEEDBACK_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FEEDBACK_BYTES {
        bail!("Stdin message exceeds 128 KiB");
    }
    let message = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("Stdin must be UTF-8"))?;
    if message.trim().is_empty() {
        bail!("Message cannot be empty");
    }
    Ok(message)
}

fn resolve_target<'a, T: super::SessionRef>(
    identifier: &str,
    sessions: &'a [T],
    exact: bool,
) -> Result<&'a T> {
    if !exact {
        return super::resolve_session(identifier, sessions);
    }
    let mut matches = sessions.iter().filter(|session| session.id() == identifier);
    let session = matches
        .next()
        .ok_or_else(|| anyhow::anyhow!("No exact session ID in selected profile"))?;
    if matches.next().is_some() {
        bail!("Exact session ID is ambiguous");
    }
    Ok(session)
}

/// Validate against and default from the daemon, through the client that carries its credentials.
pub(super) async fn resolve_profile(client: &HttpClient, profile: &str) -> Result<String> {
    let daemon = client.daemon_client().await?;
    if profile.is_empty() {
        return Ok(daemon.default_profile().await?);
    }
    if !daemon
        .runtime_info()
        .await?
        .profiles
        .iter()
        .any(|p| p == profile)
    {
        bail!("Profile '{profile}' does not exist");
    }
    Ok(profile.to_string())
}

/// The client carries only a validated refusal code, so the wording lives here.
fn refusal(error: HttpError, identifier: &str) -> anyhow::Error {
    let HttpError::Server { body, .. } = &error else {
        return error.into();
    };
    match body.as_str() {
        "session_not_running" => anyhow::anyhow!(
            "Session is not running. Start it first with: aoe session start {identifier}"
        ),
        "verified_idle_unavailable" => anyhow::anyhow!(
            "Verified idle agent unavailable; retain or copy feedback and retry when ready"
        ),
        "session_archived" => anyhow::anyhow!(StartBlocked::Archived),
        "session_trashed" => anyhow::anyhow!(StartBlocked::Trashed),
        _ => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::ApiErrorCode;
    use crate::daemon::DaemonEndpoint;
    use crate::server::api::{api_error_with_code_header, start_blocked_response};
    use axum::{extract::Path, routing::post, Router};

    #[test]
    fn send_arguments_require_one_message_source() {
        use clap::Parser;
        for argv in [
            vec!["aoe", "send", "id", "hello"],
            vec![
                "aoe",
                "--profile",
                "work",
                "send",
                "--exact",
                "--stdin",
                "--no-revive",
                "--require-idle",
                "id",
            ],
        ] {
            assert!(crate::cli::Cli::try_parse_from(argv).is_ok());
        }
        for argv in [
            vec!["aoe", "send", "id"],
            vec!["aoe", "send", "--stdin", "id", "hello"],
        ] {
            assert!(crate::cli::Cli::try_parse_from(argv).is_err());
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn missing_or_empty_exact_profile_refuses_before_default_lookup_or_delivery() {
        use clap::Parser;
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let _home = crate::session::test_support::isolate_app_dir();
        let mut row = crate::session::Instance::new("default target", "/tmp");
        row.source_profile = "default".into();
        crate::server::test_support::seed_instances_on_disk_for_test("default", vec![row.clone()]);
        let state = crate::server::test_support::build_test_app_state_with_policy(
            vec![row.clone()],
            Vec::new(),
            Vec::new(),
            Some("secret".into()),
        );
        crate::server::test_support::refresh_canonical_metadata_for_test(&state).await;
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let app = crate::server::test_support::build_router_for_test(state).layer(
            axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let observed = observed.clone();
                    async move {
                        observed.fetch_add(1, Ordering::SeqCst);
                        next.run(request).await
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let url = format!("http://{address}");
        let _env = crate::session::test_support::EnvGuard::set(&[
            ("AOE_DAEMON_URL", url.as_str()),
            ("AOE_DAEMON_TOKEN", "secret"),
        ]);
        let client = HttpClient::new(DaemonEndpoint::new(
            url,
            Some("secret".into()),
            crate::daemon::Source::Env,
        ))
        .unwrap();
        assert_eq!(resolve_profile(&client, "").await.unwrap(), "default");
        assert!(client
            .list_sessions(None)
            .await
            .unwrap()
            .sessions
            .iter()
            .any(|session| { session.id == row.id && session.profile == "default" }));
        requests.store(0, Ordering::SeqCst);
        for argv in [
            vec![
                "aoe",
                "--profile",
                "",
                "send",
                "--exact",
                &row.id,
                "feedback",
            ],
            vec!["aoe", "send", "--exact", &row.id, "feedback"],
        ] {
            let cli = crate::cli::Cli::try_parse_from(argv).unwrap();
            let crate::cli::Commands::Send(args) = cli.command.unwrap() else {
                panic!("send command");
            };
            let error = run(cli.profile.as_deref().unwrap_or_default(), args)
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "Exact send requires a non-empty explicit profile"
            );
        }
        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "no daemon lookup or delivery"
        );
        server.abort();
        let _ = server.await;
    }

    #[test]
    fn stdin_is_bounded_and_errors_do_not_echo_feedback() {
        assert_eq!(read_feedback("  notes\n".as_bytes()).unwrap(), "  notes\n");
        assert!(read_feedback(" \n\t".as_bytes()).is_err());
        assert!(read_feedback(&[0xff][..]).is_err());
        let mut bytes = vec![b'x'; MAX_FEEDBACK_BYTES + 20];
        assert_eq!(
            read_feedback(&bytes[..MAX_FEEDBACK_BYTES]).unwrap().len(),
            MAX_FEEDBACK_BYTES
        );
        let mut reader = std::io::Cursor::new(&mut bytes);
        let error = read_feedback(&mut reader).unwrap_err().to_string();
        assert_eq!(error, "Stdin message exceeds 128 KiB");
        assert_eq!(reader.position(), (MAX_FEEDBACK_BYTES + 1) as u64);
    }

    #[test]
    fn exact_target_never_uses_prefix_title_path_or_another_profile() {
        let mut origin = crate::session::Instance::new("origin title", "/origin/path");
        origin.source_profile = "work".into();
        let other = crate::session::Instance::new(&origin.id, "/other/path");
        let work = vec![&origin];
        assert_eq!(
            resolve_target(&origin.id, &work, true).unwrap().id,
            origin.id
        );
        for identifier in [&origin.id[..8], &origin.title, &origin.project_path] {
            assert!(resolve_target(identifier, &work, true).is_err());
            assert!(resolve_target(identifier, &work, false).is_ok());
        }
        let wrong_profile = vec![&other];
        assert!(resolve_target(&origin.id, &wrong_profile, true).is_err());
        assert!(resolve_target(&origin.id, &wrong_profile, false).is_ok());
    }

    #[tokio::test]
    async fn guarded_feedback_uses_authenticated_json_only() {
        use axum::{http::HeaderMap, Json};
        let feedback = "quotes '\"; $(touch /tmp/not-executed)\nsecond line";
        let expected = feedback.to_owned();
        let app = Router::new().route("/api/sessions/{id}/send", post(
            move |Path(id): Path<String>, headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                let expected = expected.clone();
                async move {
                    assert_eq!(id, "full-session-id");
                    assert_eq!(headers["authorization"], "Bearer secret");
                    assert_eq!(body, serde_json::json!({"message": expected, "revive": false, "require_idle": true}));
                    Json(serde_json::json!({"sent": true}))
                }
            }
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = HttpClient::new(DaemonEndpoint::new(
            format!("http://{address}"),
            Some("secret".into()),
            crate::daemon::Source::Env,
        ))
        .unwrap();
        client
            .send_message_with_idle_guard("full-session-id", feedback, true, true)
            .await
            .unwrap();
        server.abort();
        let _ = server.await;
    }

    /// Drives the real client against the refusals the daemon actually emits.
    #[tokio::test]
    async fn refusals_survive_the_real_client() {
        let app = Router::new().route(
            "/api/sessions/{id}/send",
            post(|Path(id): Path<String>| async move {
                match id.as_str() {
                    "archived" => start_blocked_response(StartBlocked::Archived),
                    "trashed" => start_blocked_response(StartBlocked::Trashed),
                    "busy" => api_error_with_code_header(
                        ApiErrorCode::VerifiedIdleUnavailable,
                        "Verified idle agent unavailable",
                    ),
                    "dead" => api_error_with_code_header(
                        ApiErrorCode::SessionNotRunning,
                        "Terminal session is not running",
                    ),
                    _ => api_error_with_code_header(ApiErrorCode::StartFailed, "boom"),
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = HttpClient::new(DaemonEndpoint::new(
            format!("http://{address}"),
            Some("secret".to_string()),
            crate::daemon::Source::Env,
        ))
        .unwrap();

        for (id, expected) in [
            (
                "dead",
                "Session is not running. Start it first with: aoe session start dead",
            ),
            ("archived", "session is archived; unarchive it first"),
            ("trashed", "session is in trash; restore it first"),
            (
                "busy",
                "Verified idle agent unavailable; retain or copy feedback and retry when ready",
            ),
        ] {
            let error = client.send_message(id, "hi", true).await.unwrap_err();
            assert_eq!(refusal(error, id).to_string(), expected);
        }
        let error = client.send_message("other", "hi", true).await.unwrap_err();
        assert!(refusal(error, "other").to_string().contains("start_failed"));
        server.abort();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn profile_lookup_keeps_passphrase_credentials() {
        use crate::acp::client::passphrase_session::test_daemon::{TestDaemon, PASSPHRASE};

        let daemon = TestDaemon::start(PASSPHRASE).await;
        let client = HttpClient::new(daemon.endpoint.clone()).unwrap();

        assert_eq!(resolve_profile(&client, "").await.unwrap(), "default");
        assert_eq!(resolve_profile(&client, "work").await.unwrap(), "work");
        let error = resolve_profile(&client, "nope").await.unwrap_err();
        assert_eq!(error.to_string(), "Profile 'nope' does not exist");
        assert_eq!(daemon.logins(), 1);
    }
}
