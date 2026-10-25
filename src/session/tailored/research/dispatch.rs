use super::*;
use std::{
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

fn account(account: CaptureAccount) -> &'static str {
    match account {
        CaptureAccount::Personal => "personal",
        CaptureAccount::Work => "work",
    }
}
fn profile(draft: &Draft) -> &'static str {
    match (draft.inputs.backend, draft.inputs.account) {
        (Some(Backend::Orx), CaptureAccount::Personal) => "research",
        (Some(Backend::Orx), CaptureAccount::Work) => "research-work",
        (Some(Backend::Evo), CaptureAccount::Personal) => "evo",
        (Some(Backend::Evo), CaptureAccount::Work) => "evo-work",
        _ => "",
    }
}
fn launcher(backend: Backend) -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("Cannot find home")?
        .join(match backend {
            Backend::Orx => ".ledger/launchers/orx-research/orx-research",
            Backend::Evo => ".ledger/launchers/evo/evo-claude",
        }))
}
fn validate_capabilities(
    bytes: &[u8],
    backend: Backend,
    selected: CaptureAccount,
    expected: &str,
) -> Result<()> {
    #[derive(Deserialize)]
    struct Capabilities {
        schema: String,
        accounts: BTreeMap<String, Route>,
        operation: String,
    }
    #[derive(Deserialize)]
    struct Route {
        profile: String,
    }
    let cap: Capabilities = serde_json::from_slice(bytes)
        .context("Launcher lacks managed AoE capabilities; provision the new launcher first")?;
    let (schema, operation) = match backend {
        Backend::Orx => ("aoe-orx-launcher-v1", "server"),
        Backend::Evo => ("aoe-evo-launcher-v1", "discover"),
    };
    if cap.schema != schema
        || cap.operation != operation
        || cap
            .accounts
            .get(account(selected))
            .is_none_or(|r| r.profile != expected)
    {
        bail!("Launcher does not qualify the selected account/operation; no fallback is allowed");
    }
    Ok(())
}
fn marker(path: &Path, backend: Backend) -> Result<()> {
    let schema = match backend {
        Backend::Orx => "aoe-orx-launcher-v1",
        Backend::Evo => "aoe-evo-launcher-v1",
    };
    let mut source = String::new();
    std::fs::File::open(path)
        .context("Managed research launcher is missing")?
        .take(128 * 1024)
        .read_to_string(&mut source)?;
    if !source.contains(schema) || !source.contains("--aoe-capabilities") {
        bail!("Installed launcher does not support managed AoE research. Provision the qualified launcher first; legacy launchers are never invoked.");
    }
    Ok(())
}
struct ReapOnDrop(Option<std::process::Child>);
impl Drop for ReapOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}
pub(super) fn qualify(draft: &Draft) -> Result<()> {
    let mut review = draft.clone();
    review.authorize(draft.revision)?;
    let backend = draft.inputs.backend.context("Choose a backend")?;
    let cwd = tempfile::tempdir()?;
    let path = launcher(backend)?;
    marker(&path, backend)?;
    let bytes = developer::bounded(
        developer::command(&path)
            .current_dir(cwd.path())
            .arg("--aoe-capabilities"),
        Duration::from_secs(10),
        16_384,
    )?;
    validate_capabilities(&bytes, backend, draft.inputs.account, profile(draft))?;
    developer::verify(profile(draft), "claude-code", false, cwd.path())
}
fn run_dir(draft: &Draft) -> Result<PathBuf> {
    let dir = crate::session::get_app_dir()?
        .join("research-runs")
        .join(&draft.id);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}
fn write_prompt(dir: &Path, prompt: &str) -> Result<PathBuf> {
    let path = dir.join("reviewed-prompt.txt");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    use std::io::Write;
    file.write_all(prompt.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}
pub(super) fn run(draft: &Draft, store: &Store) -> Result<Receipt> {
    match draft.inputs.backend.context("Missing backend")? {
        Backend::Evo => evo(draft, store),
        Backend::Orx => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(orx(draft, store)),
    }
}
fn clean_environment(environment: Vec<(std::ffi::OsString, std::ffi::OsString)>) -> Result<String> {
    let mut args = vec!["env".to_owned(), "-i".into(), "TERM=xterm-256color".into()];
    for (key, value) in environment {
        args.push(format!(
            "{}={}",
            key.into_string()
                .map_err(|_| anyhow::anyhow!("Non-UTF8 environment key"))?,
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("Non-UTF8 environment value"))?
        ));
    }
    Ok(shell_words::join(args))
}
fn evo(draft: &Draft, store: &Store) -> Result<Receipt> {
    let dir = run_dir(draft)?;
    let prompt = write_prompt(
        &dir,
        draft
            .frozen_prompt
            .as_deref()
            .context("Missing frozen prompt")?,
    )?;
    let name = super::owned_pane::session_name(&draft.id);
    let path = launcher(Backend::Evo)?;
    let command = shell_words::join([
        path.to_str().context("Non-UTF8 launcher path")?,
        "--aoe-account",
        account(draft.inputs.account),
        "--aoe-discover-prompt",
        prompt.to_str().context("Non-UTF8 prompt path")?,
        "--aoe-session-id",
        &draft.id,
    ]);
    let mut receipt = Receipt {
        profile: profile(draft).into(),
        process_id: 0,
        backend_id: draft.id.clone(),
        return_path: format!("tmux:{name}"),
        project_id: None,
        turn_id: None,
        pane: None,
    };
    store.change(&draft.id, |d| {
        d.receipt = Some(receipt.clone());
        Ok(())
    })?;
    // A clean environment prevents ancestor-agent flags and routing from leaking in.
    let clean = clean_environment(developer::environment())?;
    let pane = super::owned_pane::create(
        &draft.id,
        &draft.inputs.workspace,
        &format!("{clean} {command}"),
    )?;
    receipt.process_id = pane.process_id;
    receipt.pane = Some(pane);

    Ok(receipt)
}
async fn response_json(response: reqwest::Response) -> Result<serde_json::Value> {
    let status = response.status();
    let mut bytes = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > 128 * 1024 {
            bail!("Backend response exceeded limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        bail!("Backend returned HTTP {status}");
    }
    serde_json::from_slice(&bytes).context("Invalid backend response")
}
fn id(value: &serde_json::Value, key: &str) -> Result<String> {
    let value = value[key]["id"]
        .as_str()
        .context("Backend did not return an identity")?;
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("Invalid backend identity");
    }
    Ok(value.into())
}
async fn orx(draft: &Draft, store: &Store) -> Result<Receipt> {
    let dir = run_dir(draft)?;
    write_prompt(
        &dir,
        draft
            .frozen_prompt
            .as_deref()
            .context("Missing frozen prompt")?,
    )?;
    let log_path = dir.join("orx.log");
    let log = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&log_path)?;
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = reservation.local_addr()?.port().to_string();
    drop(reservation);
    let mut command = developer::command(&launcher(Backend::Orx)?);
    command
        .current_dir(&draft.inputs.workspace)
        .args([
            "--aoe-account",
            account(draft.inputs.account),
            "--aoe-run-id",
            &draft.id,
            "--",
            "up",
            "--port",
            &port,
            "--no-browser",
            "--no-agent",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    let mut owned = ReapOnDrop(Some(crate::process::spawn_process_group(&mut command)?));
    let child = owned.0.as_mut().context("Missing owned child")?;
    let mut receipt = Receipt {
        profile: profile(draft).into(),
        process_id: child.id(),
        backend_id: draft.id.clone(),
        return_path: log_path.to_string_lossy().into(),
        project_id: None,
        turn_id: None,
        pane: None,
    };
    // Retain ownership evidence even if startup or any POST has an ambiguous outcome.
    store.change(&draft.id, |d| {
        d.receipt = Some(receipt.clone());
        Ok(())
    })?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let base = loop {
        if child.try_wait()?.is_some() {
            bail!("Managed ORX exited; inspect {}", log_path.display());
        }
        let mut log = String::new();
        std::fs::File::open(&log_path)?
            .take(128 * 1024)
            .read_to_string(&mut log)?;
        if let Some(port) = log.lines().find_map(|line| {
            line.strip_prefix("orx up: dashboard on http://127.0.0.1:")
                .and_then(|p| p.parse::<u16>().ok())
                .filter(|p| p.to_string() == port)
        }) {
            break format!("http://127.0.0.1:{port}");
        }
        if Instant::now() >= deadline {
            crate::process::terminate_and_reap_process_group(child, Duration::from_millis(100))?;
            bail!("Managed ORX startup timed out");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    receipt.return_path = base.clone();
    store.change(&draft.id, |d| {
        d.receipt = Some(receipt.clone());
        Ok(())
    })?;
    // The host started this exact process on an OS-assigned loopback port, never a client URL.

    submit_orx(draft, store, receipt, &base, || {
        if child.try_wait()?.is_some() {
            bail!("Owned ORX process exited; refusing to submit to an unowned endpoint");
        }
        Ok(())
    })
    .await
}
async fn submit_orx(
    draft: &Draft,
    store: &Store,
    mut receipt: Receipt,
    base: &str,
    mut ensure_owned: impl FnMut() -> Result<()>,
) -> Result<Receipt> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(45))
        .build()?;
    ensure_owned()?;
    let health = client.get(format!("{base}/api/health")).send().await?;
    if !health.status().is_success() {
        bail!("Managed ORX health check failed");
    }
    ensure_owned()?;
    let project = response_json(client.post(format!("{base}/api/projects")).json(&serde_json::json!({"name":"Tailored research", "path":draft.inputs.workspace,"createFolder":false,"initializeGit":false,"githubSyncEnabled":false})).send().await?).await?;
    let project_id = id(&project, "project")?;
    receipt.project_id = Some(project_id.clone());
    store.change(&draft.id, |d| {
        d.receipt = Some(receipt.clone());
        Ok(())
    })?;
    ensure_owned()?;
    let chat = response_json(client.post(format!("{base}/api/chat/sessions")).json(&serde_json::json!({"projectId":project_id,"harness":"claude-code","model":"sonnet","permissionMode":"bypassPermissions","planMode":false})).send().await?).await?;
    receipt.backend_id = id(&chat, "session")?;
    store.change(&draft.id, |d| {
        d.receipt = Some(receipt.clone());
        Ok(())
    })?;
    ensure_owned()?;
    let turn = response_json(client.post(format!("{base}/api/chat/sessions/{}/message", receipt.backend_id)).json(&serde_json::json!({"text":draft.frozen_prompt,"model":"sonnet","permissionMode":"bypassPermissions","clientTurnId":draft.id})).send().await?).await?;
    if turn["ok"] != true {
        bail!("ORX did not acknowledge submission");
    }
    receipt.turn_id = Some(
        turn["turn"]["turnId"]
            .as_str()
            .context("ORX did not return turn identity")?
            .into(),
    );
    Ok(receipt)
}

fn pane(draft: &Draft) -> Result<&super::owned_pane::OwnedPane> {
    if draft.inputs.backend != Some(Backend::Evo)
        || !matches!(
            draft.state,
            State::Submitted | State::Uncertain | State::Running
        )
    {
        bail!("No owned EVO discovery pane");
    }
    let receipt = draft.receipt.as_ref().context("No launch identity")?;
    if receipt.backend_id != draft.id
        || receipt.profile != profile(draft)
        || receipt.return_path != format!("tmux:{}", super::owned_pane::session_name(&draft.id))
    {
        bail!("Mismatched research launch identity");
    }
    receipt
        .pane
        .as_ref()
        .context("No immutable pane identity; never adopt or restart a pane")
}
pub fn output(draft: &Draft) -> Result<String> {
    pane(draft)?.output()
}
pub fn reply(draft: &Draft, answer: &str) -> Result<()> {
    text(answer, 16_384)?;
    if answer.trim().is_empty()
        || answer
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        bail!("Enter a literal discovery answer without control characters");
    }
    validate_answer(answer)?;
    pane(draft)?.reply(answer)
}
fn validate_answer(answer: &str) -> Result<()> {
    for line in answer.lines() {
        let normalized = line.trim_start().to_lowercase();
        if [
            "/evo:optimize",
            "/evo:ship",
            "evo optimize",
            "evo ship",
            "$evo optimize",
            "$evo ship",
        ]
        .iter()
        .any(|command| {
            normalized
                .strip_prefix(command)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        }) {
            bail!("Optimization/shipping requires subsequent explicit review and is not available in this discovery interface");
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_capabilities_require_exact_account_and_profile() {
        let cap = br#"{"schema":"aoe-evo-launcher-v1","accounts":{"personal":{"profile":"evo"}},"operation":"discover"}"#;
        assert!(validate_capabilities(cap, Backend::Evo, CaptureAccount::Personal, "evo").is_ok());
        assert!(
            validate_capabilities(cap, Backend::Evo, CaptureAccount::Work, "evo-work").is_err()
        );
        assert!(
            validate_capabilities(cap, Backend::Orx, CaptureAccount::Personal, "research").is_err()
        );
        assert!(validate_capabilities(
            b"old launcher",
            Backend::Evo,
            CaptureAccount::Work,
            "evo-work"
        )
        .is_err());
    }
    #[test]
    fn legacy_markers_and_out_of_phase_replies_fail_before_any_process() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy");
        std::fs::write(&legacy, "#!/bin/sh\ntouch should-never-run\n").unwrap();
        assert!(marker(&legacy, Backend::Evo).is_err());
        assert!(!dir.path().join("should-never-run").exists());
        let draft = Draft::new(
            uuid::Uuid::new_v4().to_string(),
            "Question".into(),
            Inputs::new(CaptureAccount::Personal),
        )
        .unwrap();
        for answer in [
            "/evo:optimize budget=10",
            "evo optimize budget=2",
            "bad\u{1b}answer",
            "",
        ] {
            assert!(reply(&draft, answer).is_err());
        }
    }
    #[tokio::test]
    async fn orx_http_contract_retains_identity_and_never_retries_rejected_submission() {
        use axum::{
            routing::{get, post},
            Json, Router,
        };
        use std::sync::{Arc, Mutex};
        for reject in [false, true] {
            let received = Arc::new(Mutex::new(Vec::new()));
            let capture = received.clone();
            let router = Router::new()
                .route("/api/health", get(|| async { Json(serde_json::json!({"ok":true})) }))
                .route("/api/projects", post({ let capture = capture.clone(); move |Json(body): Json<serde_json::Value>| async move { capture.lock().unwrap().push(("project", body)); Json(serde_json::json!({"project":{"id":"project_1"}})) } }))
                .route("/api/chat/sessions", post({ let capture = capture.clone(); move |Json(body): Json<serde_json::Value>| async move { capture.lock().unwrap().push(("chat", body)); Json(serde_json::json!({"session":{"id":"chat_1"}})) } }))
                .route("/api/chat/sessions/chat_1/message", post(move |Json(body): Json<serde_json::Value>| { let capture = capture.clone(); async move { capture.lock().unwrap().push(("message", body)); (if reject { axum::http::StatusCode::BAD_GATEWAY } else { axum::http::StatusCode::OK }, Json(serde_json::json!({"ok":true,"turn":{"turnId":"turn_1","queued":false,"existing":false}}))) } }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let dir = tempfile::tempdir().unwrap();
            let store = Store::at(dir.path().join("research.json"));
            let mut draft = Draft::new(
                uuid::Uuid::new_v4().to_string(),
                "Original".into(),
                Inputs::new(CaptureAccount::Work),
            )
            .unwrap();
            draft.inputs.workspace = dir.path().to_string_lossy().into();
            draft.inputs.backend = Some(Backend::Orx);
            draft.frozen_prompt = Some("Exact reviewed prompt; $(not shell)\nUnicode 🌱".into());
            draft.state = State::Running;
            store.create(draft.clone()).unwrap();
            let receipt = Receipt {
                profile: "research-work".into(),
                process_id: 123,
                backend_id: draft.id.clone(),
                return_path: base.clone(),
                project_id: None,
                turn_id: None,
                pane: None,
            };
            let result = submit_orx(&draft, &store, receipt, &base, || Ok(())).await;
            if reject {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().turn_id.as_deref(), Some("turn_1"));
            }
            let requests = received.lock().unwrap();
            assert_eq!(requests.len(), 3);
            assert_eq!(requests[0].1["githubSyncEnabled"], false);
            assert_eq!(
                requests[1].1,
                serde_json::json!({"projectId":"project_1","harness":"claude-code","model":"sonnet","permissionMode":"bypassPermissions","planMode":false})
            );
            assert_eq!(
                requests[2].1["text"],
                draft.frozen_prompt.as_deref().unwrap()
            );
            assert_eq!(requests[2].1["clientTurnId"], draft.id);
            let saved = store.get(&draft.id).unwrap();
            assert_eq!(saved.receipt.as_ref().unwrap().backend_id, "chat_1");
            assert_eq!(
                saved.receipt.unwrap().project_id.as_deref(),
                Some("project_1")
            );
            server.abort();
            let _ = server.await;
        }
    }
    #[test]
    fn tmux_environment_preserves_the_same_qualified_xdg_paths_without_shell_interpolation() {
        let environment = vec![
            ("HOME".into(), "/tmp/home".into()),
            ("XDG_CONFIG_HOME".into(), "/tmp/config with spaces".into()),
            ("XDG_DATA_HOME".into(), "/tmp/data; $(not shell)".into()),
        ];
        assert_eq!(
            shell_words::split(&clean_environment(environment).unwrap()).unwrap(),
            [
                "env",
                "-i",
                "TERM=xterm-256color",
                "HOME=/tmp/home",
                "XDG_CONFIG_HOME=/tmp/config with spaces",
                "XDG_DATA_HOME=/tmp/data; $(not shell)"
            ]
        );
    }
    #[test]
    fn literal_discovery_targets_are_not_confused_with_phase_commands() {
        for answer in [
            "optimize startup latency",
            "do not optimize networking",
            "Use optimize as the benchmark label",
        ] {
            assert!(validate_answer(answer).is_ok());
        }
        for answer in [
            "/evo:optimize budget=3",
            "/evo:ship",
            "evo optimize",
            "$evo ship",
            "Target is ready\n/evo:optimize",
        ] {
            assert!(validate_answer(answer).is_err());
        }
    }
}
