//! Send-message, paste-image, and read-output endpoints.

use super::*;

use crate::server::session_service::SessionService;

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub message: String,
    /// Whether to auto-revive a dead/stopped session before sending. Defaults
    /// to `true`; set to `false` for fail-loud behavior (parity with the
    /// `--no-revive` CLI flag).
    #[serde(default = "default_revive")]
    pub revive: bool,
    /// Require positive idle evidence before terminal input. Does not grant tool approval.
    #[serde(default)]
    pub require_idle: bool,
}

fn default_revive() -> bool {
    true
}

/// Terminal delivery failures shared by HTTP and plugin callers.
#[derive(Debug)]
pub(crate) enum SendKeysError {
    NotFound,
    NotRunning,
    Transient(Status),
    StructuredView,
    Blocked(crate::session::StartBlocked),
    Gone,
    Tmux(anyhow::Error),
    Preparation {
        response: Box<axum::response::Response>,
        detail: serde_json::Value,
    },
    Internal,
}

pub(crate) struct TerminalSendError {
    pub error: SendKeysError,
    committed_cursor: Option<Box<(axum::http::HeaderValue, axum::http::HeaderValue)>>,
}

/// Revive through the native lifecycle command, then send under immutable pane ownership.
pub(crate) async fn send_terminal_message(
    service: &SessionService,
    id: &str,
    message: String,
    revive: bool,
) -> Result<bool, TerminalSendError> {
    let mut committed_cursor = None;
    send_terminal_message_inner(service, id, message, revive, false, &mut committed_cursor)
        .await
        .map_err(|error| TerminalSendError {
            error,
            committed_cursor: committed_cursor.map(Box::new),
        })
}

async fn send_terminal_message_inner(
    service: &SessionService,
    id: &str,
    message: String,
    revive: bool,
    require_idle: bool,
    committed_cursor: &mut Option<(axum::http::HeaderValue, axum::http::HeaderValue)>,
) -> Result<bool, SendKeysError> {
    let state = service.native_state().map_err(SendKeysError::Tmux)?;
    if state.read_only {
        return Err(SendKeysError::Preparation {
            response: Box::new(crate::server::api::read_only_response()),
            detail: serde_json::json!({"error": "read_only"}),
        });
    }
    if let Some(response) = crate::server::api::cityhall_block(&state) {
        return Err(SendKeysError::Preparation {
            response: Box::new(response),
            detail: serde_json::json!({"error": "cityhall_mode"}),
        });
    }
    let before = service
        .instances
        .read()
        .await
        .iter()
        .find(|row| row.id == id)
        .cloned()
        .ok_or(SendKeysError::NotFound)?;
    before.ensure_startable().map_err(SendKeysError::Blocked)?;
    if before.is_structured() {
        return Err(SendKeysError::StructuredView);
    }
    if matches!(before.status, Status::Creating | Status::Deleting) {
        return Err(SendKeysError::Transient(before.status));
    }
    if revive {
        let response = super::lifecycle::prepare_agent_session(
            state.clone(),
            id.to_owned(),
            Ok(None),
            super::lifecycle::AgentPreparation::Ensure,
            None,
        )
        .await;
        if !response.status().is_success() {
            let (parts, body) = response.into_parts();
            let bytes = axum::body::to_bytes(body, 64 * 1024)
                .await
                .map_err(|_| SendKeysError::Internal)?;
            let detail = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            return Err(SendKeysError::Preparation {
                response: Box::new(axum::response::Response::from_parts(
                    parts,
                    axum::body::Body::from(bytes),
                )),
                detail,
            });
        }
        if service
            .instances
            .read()
            .await
            .iter()
            .any(|row| row.id == id && row.lifecycle_generation != before.lifecycle_generation)
        {
            *committed_cursor = response
                .headers()
                .get(crate::daemon::RUNTIME_EPOCH_HEADER)
                .cloned()
                .zip(
                    response
                        .headers()
                        .get(crate::daemon::RUNTIME_REVISION_HEADER)
                        .cloned(),
                );
        }
    }
    let namespace = state.profile_namespace.read().await;
    if *state.canonical_health.read().await != crate::daemon::RuntimeHealth::Healthy {
        return Err(SendKeysError::Tmux(
            crate::session::NativeStoreUnavailable.into(),
        ));
    }
    let _submission = service
        .prompt_submission_for_session(id)
        .await
        .ok_or(SendKeysError::NotFound)?;
    let lock = service.instance_lock(id).await;
    let guard = lock.lock().await;
    let mut instance = service
        .instances
        .read()
        .await
        .iter()
        .find(|row| row.id == id)
        .cloned()
        .ok_or(SendKeysError::NotFound)?;
    let revived = instance.lifecycle_generation != before.lifecycle_generation;
    let worker_state = state.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
        use crate::session::SessionStore;
        let native = crate::server::session_store::NativeSessionStore::open(
            worker_state,
            &instance.source_profile,
            None,
        )?;
        let store: &dyn SessionStore = &native;
        let expected = (instance.lifecycle_generation, instance.title.clone());
        let _ownership = instance.acquire_auxiliary_locks_in(&native)?;
        anyhow::ensure!(
            instance.lifecycle_generation == expected.0 && instance.title == expected.1,
            crate::session::LifecycleReservationError::Superseded
        );
        instance.ensure_startable()?;
        anyhow::ensure!(
            !instance.is_structured(),
            "Session no longer has a terminal agent"
        );
        if require_idle {
            let pane_id = verified_idle_pane(&mut instance)?;
            crate::tmux::Session::send_text_to_pane(
                &pane_id,
                &message,
                instance.send_keys_enter_delay(),
            )?;
        } else {
            anyhow::ensure!(
                super::ensure::ready_agent_session(&instance)?.is_some(),
                TerminalNotRunning
            );
            let session = instance.tmux_session()?;
            session.wait_until_ready(
                std::time::Duration::from_secs(5),
                crate::agents::ready_marker(&instance.tool),
            );
            session.send_keys_with_delay(&message, instance.send_keys_enter_delay())?;
        }
        store.update(|rows, _| {
            let row = rows
                .iter_mut()
                .find(|row| row.id == instance.id)
                .ok_or(crate::session::SessionGone)?;
            anyhow::ensure!(
                row.lifecycle_generation == instance.lifecycle_generation,
                crate::session::LifecycleReservationError::Superseded
            );
            row.touch_after_input();
            Ok(())
        })?;
        Ok(revived)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|result| result);
    drop(guard);
    drop(_submission);
    drop(namespace);
    result.map_err(|error| {
        if let Some(blocked) = error.downcast_ref::<crate::session::StartBlocked>() {
            SendKeysError::Blocked(*blocked)
        } else if error.is::<crate::session::SessionGone>() {
            SendKeysError::Gone
        } else if error.is::<TerminalNotRunning>() {
            SendKeysError::NotRunning
        } else if error.is::<VerifiedIdleUnavailable>() {
            SendKeysError::Preparation {
                response: Box::new(crate::server::api::api_error_with_code_header(
                    crate::daemon::ApiErrorCode::VerifiedIdleUnavailable,
                    "Verified idle agent unavailable; retain or copy feedback and retry when ready",
                )),
                detail: serde_json::json!({"error": "verified_idle_unavailable"}),
            }
        } else if let Some(response) = super::lifecycle::lifecycle_rejection(&state, &error) {
            SendKeysError::Preparation {
                response: Box::new(response),
                detail: serde_json::json!({"error": "lifecycle_busy"}),
            }
        } else {
            SendKeysError::Tmux(error)
        }
    })
}

#[derive(Debug, thiserror::Error)]
#[error("Verified idle agent unavailable")]
struct VerifiedIdleUnavailable;

fn verified_idle_pane(instance: &mut Instance) -> anyhow::Result<String> {
    anyhow::ensure!(
        !instance.expects_shell()
            && !instance.has_command_override()
            && !matches!(instance.status, Status::Stopped | Status::Starting),
        VerifiedIdleUnavailable
    );
    let panes = crate::tmux::batch_pane_metadata().map_err(|_| VerifiedIdleUnavailable)?;
    let mut candidates = panes.iter().filter(|(_, pane)| {
        pane.launch_report
            .as_ref()
            .is_some_and(|report| report.instance_id == instance.id)
    });
    let (name, pane) = candidates.next().ok_or(VerifiedIdleUnavailable)?;
    anyhow::ensure!(candidates.next().is_none(), VerifiedIdleUnavailable);
    let report = pane.launch_report.as_ref().ok_or(VerifiedIdleUnavailable)?;
    anyhow::ensure!(verified_agent_pane(instance, pane), VerifiedIdleUnavailable);
    verify_unique_launch_report(report)?;
    instance.status = Status::Unknown;
    instance.detection = Default::default();
    instance.update_status_once(Some(pane), Some(name));
    anyhow::ensure!(
        (instance.status == Status::Idle
            || (report.identity.agent == "pi"
                && instance.status == Status::Waiting
                && instance.detection.rule == Some("input_prompt")))
            && instance.unknown_since.is_none(),
        VerifiedIdleUnavailable
    );

    // A successful fresh capture plus visible built-in evidence excludes fallback and stale hooks.
    let output = crate::tmux::tmux_command()
        .args(["capture-pane", "-p", "-e", "-t", &pane.pane_id])
        .output()
        .map_err(|_| VerifiedIdleUnavailable)?;
    anyhow::ensure!(output.status.success(), VerifiedIdleUnavailable);
    let content = String::from_utf8(output.stdout).map_err(|_| VerifiedIdleUnavailable)?;
    anyhow::ensure!(
        positive_idle_capture(
            &report.identity.agent,
            &content,
            pane.pane_title.as_deref().unwrap_or("")
        ),
        VerifiedIdleUnavailable
    );
    Ok(pane.pane_id.clone())
}

fn verify_unique_launch_report(
    expected: &crate::session::launch_identity::LaunchReport,
) -> anyhow::Result<()> {
    let output =
        crate::tmux::run_tmux_command_with_timeout(crate::tmux::tmux_query_command().args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_id}|#{@aoe_launch_identity}",
        ]))
        .map_err(|_| VerifiedIdleUnavailable)?;
    anyhow::ensure!(output.status.success(), VerifiedIdleUnavailable);
    let inventory = std::str::from_utf8(&output.stdout).map_err(|_| VerifiedIdleUnavailable)?;
    anyhow::ensure!(
        unique_launch_report(inventory, expected),
        VerifiedIdleUnavailable
    );
    Ok(())
}

fn unique_launch_report(
    inventory: &str,
    expected: &crate::session::launch_identity::LaunchReport,
) -> bool {
    use crate::session::launch_identity::{valid_pane_id, LaunchReport};
    let mut panes = std::collections::HashSet::new();
    let mut found = false;
    for line in inventory.lines() {
        let Some((pane_id, encoded)) = line.split_once('|') else {
            return false;
        };
        if !valid_pane_id(pane_id) || !panes.insert(pane_id) {
            return false;
        }
        if encoded.is_empty() {
            continue;
        }
        let Some(report) = LaunchReport::decode(encoded, pane_id) else {
            return false;
        };
        if report.instance_id == expected.instance_id {
            if found || &report != expected {
                return false;
            }
            found = true;
        }
    }
    found
}

fn verified_agent_pane(instance: &Instance, pane: &crate::tmux::PaneMetadata) -> bool {
    !pane.pane_dead
        && pane.pane_pid.is_some()
        && pane.session_kind.as_deref() == Some(crate::tmux::SessionKind::Agent.as_marker())
        && pane.launch_report.as_ref().is_some_and(|report| {
            report.instance_id == instance.id
                && report.pane_id == pane.pane_id
                && matches!(report.identity.agent.as_str(), "claude" | "codex" | "pi")
                && report.identity.agent == instance.tool
        })
        && pane.pane_current_command.as_deref().is_some_and(|command| {
            std::path::Path::new(command)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(instance.tool.as_str())
        })
}

fn positive_idle_capture(agent: &str, content: &str, title: &str) -> bool {
    let clean = crate::tmux::utils::strip_ansi(content);
    crate::tmux::detect::input_ready(agent, &clean, title)
}

#[derive(Debug, thiserror::Error)]
#[error("Terminal session is not running")]
struct TerminalNotRunning;

impl IntoResponse for TerminalSendError {
    fn into_response(self) -> axum::response::Response {
        let mut response = match self.error {
            SendKeysError::NotFound | SendKeysError::Gone => bare_not_found(),
            SendKeysError::NotRunning => crate::server::api::api_error_with_code_header(
                crate::daemon::ApiErrorCode::SessionNotRunning,
                "Terminal session is not running",
            ),
            SendKeysError::Preparation { response, .. } => *response,
            SendKeysError::Transient(status) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "session_transient", "status": format!("{status:?}"),
                })),
            )
                .into_response(),
            SendKeysError::StructuredView => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "acp_mode_unsupported"})),
            )
                .into_response(),
            SendKeysError::Blocked(blocked) => crate::server::api::start_blocked_response(blocked),
            SendKeysError::Tmux(error) => {
                tracing::warn!(target: "http.api.sessions", %error, "terminal send failed");
                if error.is::<crate::session::NativeStoreUnavailable>() {
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                } else {
                    api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "tmux_error",
                        "Terminal send failed",
                    )
                }
            }
            SendKeysError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal"})),
            )
                .into_response(),
        };
        if let Some(cursor) = self.committed_cursor {
            let (epoch, revision) = *cursor;
            response
                .headers_mut()
                .insert(crate::daemon::RUNTIME_EPOCH_HEADER, epoch);
            response
                .headers_mut()
                .insert(crate::daemon::RUNTIME_REVISION_HEADER, revision);
        }
        response
    }
}

pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    req: Result<Json<SendMessageRequest>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    if state.read_only {
        return crate::server::api::read_only_response();
    }
    // Terminal keystroke injection: CityHall sessions are structured-view only,
    // so close this explicitly rather than leaning on the downstream error.
    if let Some(resp) = crate::server::api::cityhall_block(&state) {
        return resp;
    }
    let Json(mut req) = match req {
        Ok(j) => j,
        Err(rej) => return rej.into_response(),
    };

    crate::server::live_ws::strip_paste_escapes(&mut req.message);
    let len = req.message.trim_end_matches(['\r', '\n']).len();
    req.message.truncate(len);
    if req.message.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "message_empty"})),
        )
            .into_response();
    }

    let revive = req.revive;
    let mut committed_cursor = None;
    let result = send_terminal_message_inner(
        &state.session_service,
        &id,
        req.message,
        revive,
        req.require_idle,
        &mut committed_cursor,
    )
    .await
    .map_err(|error| TerminalSendError {
        error,
        committed_cursor: committed_cursor.map(Box::new),
    });
    match result {
        Ok(_) => {
            tracing::info!(target: "session.lifecycle", session = %id, len, revive, "terminal message sent");
            match state.runtime.publish(&state).await {
                Ok(snapshot) => crate::server::runtime::mutation_response(
                    &snapshot.value.cursor,
                    (StatusCode::OK, Json(serde_json::json!({"sent": true}))),
                ),
                Err(error) => {
                    tracing::debug!(target: "server.runtime", %error, "runtime publish failed");
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                }
            }
        }
        Err(error) => error.into_response(),
    }
}

/// Max decoded size of a pasted image (5 MiB). Claude Code caps image
/// attachments around this size; the route body limit in `build_router`
/// leaves headroom for base64's ~33% overhead plus JSON framing.
const MAX_PASTE_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Directory, relative to the session worktree, holding images pasted into
/// the live terminal. It lives inside the worktree so a Docker-sandboxed
/// pane, which mounts the worktree but cannot see the host temp dir, can
/// still read the file. A self-ignoring `.gitignore` keeps the blobs out of
/// git. See #2678.
const PASTE_IMAGE_DIR: &str = ".aoe-pasted-images";

#[derive(Deserialize)]
pub struct PasteImageRequest {
    /// Client-declared MIME. Advisory only: the extension and the
    /// accept/reject decision come from magic-byte sniffing, never this field.
    #[serde(default)]
    pub mime_type: String,
    /// Standard-base64 image bytes.
    pub data: String,
}

fn paste_image_extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// Write the decoded blob into the worktree's paste-image dir and return the
/// host path plus the generated file name. Sync (filesystem I/O); call from a
/// blocking pool.
fn write_paste_image(
    project_path: &str,
    bytes: &[u8],
    ext: &str,
) -> std::io::Result<(std::path::PathBuf, String)> {
    let dir = std::path::Path::new(project_path).join(PASTE_IMAGE_DIR);
    std::fs::create_dir_all(&dir)?;
    // A `.gitignore` of `*` also ignores itself, so the directory stays
    // invisible to `git add` with no git subprocess.
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, "*\n")?;
    }
    let file_name = format!("aoe-paste-{}.{}", uuid::Uuid::new_v4(), ext);
    let path = dir.join(&file_name);
    // create_new: uuid names never collide; fail loud if the impossible happens.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::io::Write::write_all(&mut f, bytes)?;
    Ok((path, file_name))
}

/// Map the host paste-image file to the path the tmux pane reads. Non-sandboxed
/// panes share the host filesystem, so the absolute host path is correct. A
/// sandboxed pane mounts the worktree under a container path (`/workspace/...`);
/// reuse `compute_volume_paths` so the pasted path matches that mount.
fn pane_visible_paste_path(project_path: &str, is_sandboxed: bool, file_name: &str) -> String {
    if is_sandboxed {
        if let Ok((_, working_dir)) = crate::session::config::container_config::compute_volume_paths(
            std::path::Path::new(project_path),
            project_path,
        ) {
            return format!("{working_dir}/{PASTE_IMAGE_DIR}/{file_name}");
        }
    }
    std::path::Path::new(project_path)
        .join(PASTE_IMAGE_DIR)
        .join(file_name)
        .to_string_lossy()
        .to_string()
}

/// Save a clipboard image pasted into the live terminal and return the path
/// the tmux pane can read, so the CLI agent (e.g. Claude Code) attaches it.
/// See #2678.
pub async fn paste_image(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    req: Result<Json<PasteImageRequest>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    use base64::Engine as _;

    if state.read_only {
        return crate::server::api::read_only_response();
    }
    // Allowed for the CityHall composer, but only against a structured session:
    // a terminal target would let a locked-down client write into another
    // session's worktree.
    if let Some(resp) = cityhall_block_non_structured(&state, &id).await {
        return resp;
    }
    let Json(req) = match req {
        Ok(j) => j,
        Err(rej) => return rej.into_response(),
    };

    let Some(instance) = crate::server::api::find_instance(&state, &id).await else {
        return bare_not_found();
    };

    let bytes = match base64::engine::general_purpose::STANDARD.decode(req.data.as_bytes()) {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_base64"})),
            )
                .into_response();
        }
    };
    if bytes.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "empty"})),
        )
            .into_response();
    }
    if bytes.len() > MAX_PASTE_IMAGE_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": "too_large"})),
        )
            .into_response();
    }
    let Some(mime) = crate::server::api::acp::sniff_image_mime(&bytes) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "not_an_image"})),
        )
            .into_response();
    };
    let ext = paste_image_extension(mime);

    let project_path = instance.project_path.clone();
    let is_sandboxed = instance.is_sandboxed();
    let write_project = project_path.clone();
    let (host_path, file_name) =
        match tokio::task::spawn_blocking(move || write_paste_image(&write_project, &bytes, ext))
            .await
        {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => {
                tracing::warn!(target: "http.api.sessions", "paste_image: write failed: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "write_failed"})),
                )
                    .into_response();
            }
            Err(e) => {
                tracing::warn!(target: "http.api.sessions", "paste_image: join failed: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "write_failed"})),
                )
                    .into_response();
            }
        };

    // Cancel only the timer; an in-progress removal must finish.
    let shutdown = state.shutdown.clone();
    state
        .runtime
        .work
        .spawn("server.paste_image_cleanup", async move {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(std::time::Duration::from_secs(3600)) => {}
            }
            let _ = tokio::fs::remove_file(&host_path).await;
        });

    let pane_path = pane_visible_paste_path(&project_path, is_sandboxed, &file_name);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "path": pane_path })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct OutputQuery {
    #[serde(default = "default_output_lines")]
    pub lines: u32,
    #[serde(default = "default_output_format")]
    pub format: String,
}

fn default_output_lines() -> u32 {
    200
}

fn default_output_format() -> String {
    "text".to_string()
}

pub(crate) enum CaptureError {
    NotRunning,
    Tmux(anyhow::Error),
    /// The blocking capture task panicked.
    Panicked(String),
}

/// The last `lines` lines of a terminal session's pane, with ANSI escapes stripped
/// unless `ansi`.
pub(crate) async fn capture_terminal_output(
    instance: crate::session::Instance,
    lines: usize,
    ansi: bool,
) -> Result<String, CaptureError> {
    tokio::task::spawn_blocking(move || -> Result<String, CaptureError> {
        let tmux_session = instance.tmux_session().map_err(CaptureError::Tmux)?;
        if !tmux_session.exists() {
            return Err(CaptureError::NotRunning);
        }
        let raw = tmux_session
            .capture_pane(lines)
            .map_err(CaptureError::Tmux)?;
        if ansi {
            Ok(raw)
        } else {
            Ok(crate::tmux::utils::strip_ansi(&raw))
        }
    })
    .await
    .unwrap_or_else(|e| Err(CaptureError::Panicked(e.to_string())))
}

pub async fn read_output(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<OutputQuery>,
) -> impl IntoResponse {
    // Raw terminal pane content: CityHall hides the terminal UI and WS relay,
    // so this read must be closed too.
    if let Some(resp) = crate::server::api::cityhall_block(&state) {
        return resp;
    }
    let lines = (q.lines as usize).clamp(1, 2000);
    let want_ansi = match q.format.as_str() {
        "ansi" => true,
        "text" => false,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "format_invalid",
                    "allowed": ["text", "ansi"]
                })),
            )
                .into_response();
        }
    };

    let Some(instance) = crate::server::api::find_instance(&state, &id).await else {
        return bare_not_found();
    };

    match capture_terminal_output(instance, lines, want_ansi).await {
        Ok(content) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": id,
                "lines": lines,
                "format": q.format,
                "content": content,
            })),
        )
            .into_response(),
        Err(CaptureError::NotRunning) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "session_not_running"})),
        )
            .into_response(),
        Err(CaptureError::Tmux(e)) => {
            tracing::error!(target: "http.api.sessions", "read_output: tmux error for {id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "tmux_error"})),
            )
                .into_response()
        }
        Err(CaptureError::Panicked(e)) => {
            tracing::error!(target: "http.api.sessions", "read_output: blocking task panicked for {id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal"})),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod send_output_tests {
    use super::*;

    #[test]
    fn idle_evidence_rejects_fallback_viewers_and_dialogs() {
        for (agent, screen, title, expected) in [
            ("pi", "pi>\n", "", true),
            ("unknown-agent", "pi>\n", "", false),
            ("claude", "", "", false),
            ("claude", "turn over\n", "", false),
            ("pi", "Esc to interrupt\n", "", false),
            ("codex", "› 1. Yes\n  2. No\nEnter to select\n", "", false),
            ("claude", "What should Claude do instead?\n", "⠇", false),
            ("claude", "What should Claude do instead?\n", "", false),
            ("claude", "✻ Cooked for 39s\n", "", false),
            ("claude", "What should Claude do instead?\nSelect model\nEnter to set as default\nEsc to cancel\n", "", false),
            ("claude", "❯\nSelect model\nEnter to set as default\nEsc to cancel\n", "", false),
            ("claude", "What should Claude do instead?\n❯\nSelect model\nEnter to set as default\nEsc to cancel\n", "", false),
            ("claude", "❯\nShowing detailed transcript ctrl+o\n", "", false),
            ("claude", "❯\n", "", true),
            ("claude", "❯\n", "⠇", false),
            ("codex", "──── Worked for 1m 3s\n› Write tests\ngpt-5 high · 90% left\n", "", true),
            ("codex", "<< code review finished >>\n› Write tests\ngpt-5 high · 90% left\n", "", true),
            ("pi", "Working on files\npi>\n", "", false),
            ("pi", "╭─ ask_user question ─╮\n╰── v1.0 ─╯\npi>\n", "", false),
        ] {
            assert_eq!(positive_idle_capture(agent, screen, title), expected, "{agent}: {screen:?}");
        }
    }

    #[test]
    fn strict_input_evidence_does_not_reclassify_pi_or_trust_stale_idle_hooks() {
        let observation = crate::tmux::Observation::new("", None, true);
        assert_eq!(
            crate::tmux::detect::detect("pi", "pi>\n", &observation)
                .unwrap()
                .status,
            Some(Status::Waiting)
        );
        assert!(positive_idle_capture("pi", "pi>\n", ""));
        let observation = crate::tmux::Observation::new(
            "",
            Some(crate::tmux::detect::HookObservation {
                status: Status::Idle,
                age: Some(std::time::Duration::from_secs(86400)),
            }),
            true,
        );
        assert_eq!(
            crate::tmux::detect::detect("pi", "unknown screen\n", &observation)
                .unwrap()
                .status,
            Some(Status::Idle)
        );
        assert!(!positive_idle_capture("pi", "unknown screen\n", ""));
    }

    #[test]
    fn full_pane_report_inventory_refuses_malformed_or_ambiguous_observations() {
        use crate::session::launch_identity::{
            LaunchAccount, LaunchIdentity, LaunchReport, Launcher,
        };
        let report = LaunchReport {
            instance_id: "1234567890abcdef".into(),
            pane_id: "%1".into(),
            identity: LaunchIdentity {
                agent: "pi".into(),
                account: LaunchAccount::Unknown,
                launcher: Launcher::Direct,
                profile: "default".into(),
            },
        };
        let encoded = report.encode().unwrap();
        let selected = format!("%1|{encoded}\n");
        let mut second = report.clone();
        second.pane_id = "%2".into();
        let duplicate = second.encode().unwrap();
        second.instance_id = "fedcba0987654321".into();
        let other = second.encode().unwrap();
        for (inventory, expected) in [
            (selected.clone(), true),
            (format!("{selected}%2|\n"), true),
            (format!("{selected}%2|{other}\n"), true),
            (String::new(), false),
            (format!("{selected}%2|{duplicate}\n"), false),
            (format!("{selected}{selected}"), false),
            (format!("{selected}%2|{encoded}\n"), false),
            (format!("{selected}%2|malformed\n"), false),
            (format!("{selected}not-a-pane|\n"), false),
            (format!("{selected}%2\n"), false),
        ] {
            assert_eq!(unique_launch_report(&inventory, &report), expected);
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn guarded_terminal_send_requires_observed_idle_and_full_pane_ownership() {
        use crate::session::launch_identity::{
            LaunchAccount, LaunchIdentity, LaunchReport, Launcher,
        };
        use crate::tmux::test_helpers::{
            only_pane_id, tmux_available, wait_for_pane_command, TmuxTestSession,
        };
        if !tmux_available() {
            return;
        }
        let _home = crate::session::test_support::isolate_app_dir();
        let root = tempfile::tempdir_in("/tmp").unwrap();
        // A renamed shell exercises delivery shape, not process authentication.
        let agent_binary = root.path().join("pi");
        std::fs::copy("/bin/sh", &agent_binary).unwrap();
        let input_reader = format!(
            "'{}' -c 'while IFS= read -r line; do printf \"%s\\n\" \"$line\"; done'",
            agent_binary.display()
        );
        let past_only = format!("pi>\n{}", "unrecognized screen\n".repeat(80));
        for (screen, reported, unrelated, duplicate, admitted) in [
            ("pi>\n", false, false, false, false),
            ("unrecognized screen\n", true, false, false, false),
            ("Esc to interrupt\n", true, false, false, false),
            (
                "╭─ ask_user question ─╮\n╰── v1.0 ─╯\n",
                true,
                false,
                false,
                false,
            ),
            (past_only.as_str(), true, false, false, false),
            ("pi>\n", true, true, false, false),
            ("pi>\n", true, false, true, false),
            ("pi>\n", true, false, false, true),
        ] {
            let mut row = Instance::new("guarded", root.path().to_str().unwrap());
            row.tool = "pi".into();
            row.status = Status::Idle;
            row.source_profile = "default".into();
            let unique = TmuxTestSession::new("guarded_feedback");
            let name = crate::tmux::Session::generate_name(&row.id, unique.name());
            let session = TmuxTestSession::from_name(&name);
            let scene = root.path().join(format!("{}.screen", row.id));
            let received = root.path().join(format!("{}.received", row.id));
            std::fs::write(&scene, screen).unwrap();
            let reader = if unrelated { "/bin/cat" } else { &input_reader };
            let command = format!(
                "stty -echo; cat '{}'; exec {reader} > '{}'",
                scene.display(),
                received.display()
            );
            assert!(crate::tmux::tmux_command()
                .args(["new-session", "-d", "-s", &name, &command])
                .status()
                .unwrap()
                .success());
            let pane_id = only_pane_id(session.name());
            let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !received.exists() {
                assert!(
                    std::time::Instant::now() < ready_deadline,
                    "fake agent did not start reading"
                );
                tokio::task::yield_now().await;
            }
            wait_for_pane_command(&pane_id, if unrelated { "cat" } else { "pi" });
            loop {
                let output = crate::tmux::tmux_command()
                    .args(["capture-pane", "-p", "-t", &pane_id])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                let visible = String::from_utf8(output.stdout).unwrap();
                if visible.contains(screen.lines().last().unwrap()) {
                    if screen == past_only {
                        assert!(!visible.contains("pi>"));
                    }
                    break;
                }
                assert!(
                    std::time::Instant::now() < ready_deadline,
                    "fake agent screen not visible"
                );
                tokio::task::yield_now().await;
            }
            assert!(crate::tmux::tmux_command()
                .args(["set-option", "-t", &name, "@aoe_kind", "agent"])
                .status()
                .unwrap()
                .success());
            if reported {
                let mut report = LaunchReport {
                    instance_id: row.id.clone(),
                    pane_id: pane_id.clone(),
                    identity: LaunchIdentity {
                        agent: "pi".into(),
                        account: LaunchAccount::Unknown,
                        launcher: Launcher::Direct,
                        profile: "default".into(),
                    },
                };
                let encoded = report.encode().unwrap();
                assert!(crate::tmux::tmux_command()
                    .args([
                        "set-option",
                        "-p",
                        "-t",
                        &pane_id,
                        "@aoe_launch_identity",
                        &encoded
                    ])
                    .status()
                    .unwrap()
                    .success());
                if duplicate {
                    let duplicate_received = root.path().join(format!("{}.duplicate", row.id));
                    let duplicate_command =
                        format!("exec {input_reader} > '{}'", duplicate_received.display());
                    let output = crate::tmux::tmux_command()
                        .args([
                            "split-window",
                            "-d",
                            "-t",
                            &pane_id,
                            "-P",
                            "-F",
                            "#{pane_id}",
                            &duplicate_command,
                        ])
                        .output()
                        .unwrap();
                    assert!(output.status.success());
                    report.pane_id = String::from_utf8(output.stdout).unwrap().trim().to_owned();
                    wait_for_pane_command(&report.pane_id, "pi");
                    let encoded = report.encode().unwrap();
                    assert!(crate::tmux::tmux_command()
                        .args([
                            "set-option",
                            "-p",
                            "-t",
                            &report.pane_id,
                            "@aoe_launch_identity",
                            &encoded
                        ])
                        .status()
                        .unwrap()
                        .success());
                }
            }
            crate::tmux::refresh_session_cache();
            crate::server::test_support::seed_instances_on_disk_for_test(
                "default",
                vec![row.clone()],
            );
            let state = crate::server::test_support::build_test_app_state(vec![row.clone()]);
            crate::server::test_support::refresh_canonical_metadata_for_test(&state).await;
            if reported {
                let panes = crate::tmux::batch_pane_metadata().unwrap();
                let owned = panes.get(&name).unwrap();
                assert_eq!(verified_agent_pane(&row, owned), !unrelated);
                if duplicate {
                    assert_eq!(
                        panes
                            .values()
                            .filter(|pane| pane
                                .launch_report
                                .as_ref()
                                .is_some_and(|report| report.instance_id == row.id))
                            .count(),
                        1
                    );
                    assert!(
                        verify_unique_launch_report(owned.launch_report.as_ref().unwrap()).is_err()
                    );
                }
                for mutation in 0..8 {
                    let mut unsafe_pane = owned.clone();
                    match mutation {
                        0 => unsafe_pane.pane_dead = true,
                        1 => unsafe_pane.pane_current_command = Some("bash".into()),
                        2 => unsafe_pane.pane_current_command = None,
                        3 => {
                            unsafe_pane.session_kind =
                                Some(crate::tmux::SessionKind::Tool.as_marker().into())
                        }
                        4 => {
                            unsafe_pane.launch_report.as_mut().unwrap().instance_id =
                                "other-full-id".into()
                        }
                        5 => unsafe_pane.launch_report.as_mut().unwrap().pane_id = "%999999".into(),
                        6 => unsafe_pane.pane_current_command = Some("cat".into()),
                        _ => unsafe_pane.pane_current_command = Some("python".into()),
                    }
                    assert!(!verified_agent_pane(&row, &unsafe_pane));
                }
            }
            let result = send_terminal_message_inner(
                &state.session_service,
                &row.id,
                "literal;".into(),
                false,
                true,
                &mut None,
            )
            .await;
            if admitted {
                result.expect("guard must admit positively observed input");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    if std::fs::read_to_string(&received).unwrap_or_default() == "literal;\n" {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "feedback not delivered to verified pane"
                    );
                    tokio::task::yield_now().await;
                }
            } else {
                let response = TerminalSendError {
                    error: result.expect_err("guard must refuse"),
                    committed_cursor: None,
                }
                .into_response();
                assert_eq!(response.status(), StatusCode::CONFLICT);
                let body = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
                    "verified_idle_unavailable"
                );
                // The reader was ready before the request; retain a no-delivery observation window.
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
                loop {
                    assert_eq!(std::fs::read(&received).unwrap(), b"");
                    if duplicate {
                        assert_eq!(
                            std::fs::read(root.path().join(format!("{}.duplicate", row.id)))
                                .unwrap(),
                            b""
                        );
                    }
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            }
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn send_success_invalidates_a_reload_snapshot_from_before_the_mutation() {
        let _home = crate::session::test_support::isolate_app_dir();
        let mut live = Instance::new("send-race", "/tmp/send-race");
        live.source_profile = "send-race".into();
        crate::server::test_support::seed_instances_on_disk_for_test(
            "send-race",
            vec![live.clone()],
        );
        let state = crate::server::test_support::build_test_app_state(vec![live.clone()]);
        *state.canonical_metadata.write().await =
            crate::server::reload::load_all_profiles(&state.file_watch)
                .unwrap()
                .metadata;
        let read_epoch = state
            .mutation_epoch
            .load(std::sync::atomic::Ordering::SeqCst);
        let mut stale = live.clone();
        stale.title = "stale disk row".into();
        stale.last_accessed_at = None;

        let worker_state = state.clone();
        let id = live.id.clone();
        tokio::task::spawn_blocking(move || {
            use crate::session::SessionStore;
            let native = crate::server::session_store::NativeSessionStore::open(
                worker_state,
                "send-race",
                None,
            )
            .unwrap();
            let store: &dyn SessionStore = &native;
            store
                .update(|rows, _| {
                    rows.iter_mut()
                        .find(|row| row.id == id)
                        .unwrap()
                        .touch_after_input();
                    Ok(())
                })
                .unwrap();
        })
        .await
        .unwrap();
        assert_eq!(
            state
                .mutation_epoch
                .load(std::sync::atomic::Ordering::SeqCst),
            read_epoch + 1
        );

        let metadata = state.canonical_metadata.read().await.clone();
        crate::server::reload::reload_state_instances_from_disk(
            &state,
            (vec![stale]).into(),
            Vec::new(),
            crate::server::state::StatusSource::DiskOnly,
            read_epoch,
            Ok(metadata),
            Default::default(),
        )
        .await;

        let current = state.instances.read().await;
        assert_eq!(current[0].title, live.title);
        assert!(current[0].last_accessed_at.is_some());
    }

    #[tokio::test]
    async fn post_preparation_send_failure_preserves_receipt_and_http_error() {
        let response = TerminalSendError {
            error: SendKeysError::NotRunning,
            committed_cursor: Some(Box::new((
                "committed-epoch".parse().unwrap(),
                "19".parse().unwrap(),
            ))),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers()[crate::daemon::ERROR_CODE_HEADER],
            "session_not_running"
        );
        assert_eq!(
            response.headers()[crate::daemon::RUNTIME_EPOCH_HEADER],
            "committed-epoch"
        );
        assert_eq!(
            response.headers()[crate::daemon::RUNTIME_REVISION_HEADER],
            "19"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["error"],
            "session_not_running"
        );

        let response = TerminalSendError {
            error: SendKeysError::Preparation {
                response: Box::new((
                    StatusCode::CONFLICT,
                    crate::daemon::ApiErrorCode::ResumeFailed.header(),
                    Json(serde_json::json!({"error": "resume_failed", "resume_session_id": "sid"})),
                )
                    .into_response()),
                detail: serde_json::json!({"error": "resume_failed"}),
            },
            committed_cursor: None,
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers()[crate::daemon::ERROR_CODE_HEADER],
            "resume_failed"
        );
    }
}

#[cfg(test)]
mod paste_image_tests {
    use super::*;
    use tempfile::tempdir;

    const PNG_1PX: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89,
    ];

    #[test]
    fn write_paste_image_lands_in_worktree_and_ignores_itself() {
        let dir = tempdir().unwrap();
        let project = dir.path().to_string_lossy().to_string();

        let (path, name) = write_paste_image(&project, PNG_1PX, "png").unwrap();

        assert!(path.exists(), "image file must be written");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            PNG_1PX,
            "bytes must round-trip"
        );
        assert!(name.starts_with("aoe-paste-") && name.ends_with(".png"));
        let gitignore = dir.path().join(PASTE_IMAGE_DIR).join(".gitignore");
        assert_eq!(
            std::fs::read_to_string(gitignore).unwrap(),
            "*\n",
            "dir must self-ignore so pasted blobs never reach git"
        );
    }

    #[test]
    fn pane_path_is_host_path_or_container_mount() {
        let dir = tempdir().unwrap();
        let project = dir.path().to_string_lossy().to_string();
        let dir_name = dir.path().file_name().unwrap().to_string_lossy();

        assert_eq!(
            pane_visible_paste_path(&project, false, "aoe-paste-x.png"),
            dir.path()
                .join(PASTE_IMAGE_DIR)
                .join("aoe-paste-x.png")
                .to_string_lossy()
        );
        // A non-git worktree mounts under /workspace/<dir-name>; the pasted
        // path must be the container-visible path, not the host path.
        assert_eq!(
            pane_visible_paste_path(&project, true, "aoe-paste-x.png"),
            format!("/workspace/{dir_name}/{PASTE_IMAGE_DIR}/aoe-paste-x.png")
        );
    }
}
