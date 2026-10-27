//! Research operations share the native revision and single-attempt store.
use super::*;
use crate::session::tailored::research::{self, Operation, Store};

fn guard(state: &AppState, mutate: bool) -> Option<axum::response::Response> {
    if mutate && state.read_only {
        return Some(crate::server::api::read_only_response());
    }
    crate::server::api::cityhall_block(state)
}
pub async fn research_operation(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Operation>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    if let Some(response) = guard(&state, true) {
        return response;
    }
    let Json(op) = match body {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    match tokio::task::spawn_blocking(move || Store::open()?.operate(op)).await {
        Ok(Ok(draft)) => Json(serde_json::json!({"draft":draft})).into_response(),
        Ok(Err(error)) => api_error(StatusCode::CONFLICT, "research_operation_failed", format!("{error:#}")),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "research_operation_uncertain", "Operation stopped unexpectedly. Reopen the existing research request; do not start a replacement run."),
    }
}
pub async fn research_get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    if let Some(response) = guard(&state, false) {
        return response;
    }
    match tokio::task::spawn_blocking(move || Store::open()?.get(&id)).await {
        Ok(Ok(draft)) => Json(serde_json::json!({"draft":draft})).into_response(),
        _ => api_error(
            StatusCode::NOT_FOUND,
            "research_not_found",
            "Research request unavailable",
        ),
    }
}
pub async fn research_output(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    if let Some(response) = guard(&state, false) {
        return response;
    }
    match tokio::task::spawn_blocking(move || research::output(&Store::open()?.get(&id)?)).await {
        Ok(Ok(output)) => Json(serde_json::json!({"output":output})).into_response(),
        Ok(Err(error)) => api_error(
            StatusCode::CONFLICT,
            "research_pane_unavailable",
            format!("{error:#}"),
        ),
        _ => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "research_output_failed",
            "Cannot read owned discovery pane",
        ),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchReply {
    pub answer: String,
}
pub async fn research_reply(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Result<Json<ResearchReply>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    if let Some(response) = guard(&state, true) {
        return response;
    }
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    match tokio::task::spawn_blocking(move || {
        research::reply(&Store::open()?.get(&id)?, &body.answer)
    })
    .await
    {
        Ok(Ok(())) => Json(serde_json::json!({"ok":true})).into_response(),
        Ok(Err(error)) => api_error(
            StatusCode::CONFLICT,
            "research_reply_failed",
            format!("{error:#}"),
        ),
        _ => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "research_reply_uncertain",
            "Reply outcome unknown; inspect the pane before sending again",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[serial_test::serial]
    async fn read_only_and_cityhall_block_all_writes_before_parsing_or_processes() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(dir.path());
        for (read_only, cityhall) in [(true, false), (false, true)] {
            let mut state = crate::server::test_support::build_test_app_state(vec![]);
            Arc::get_mut(&mut state).unwrap().read_only = read_only;
            Arc::get_mut(&mut state).unwrap().cityhall_mode = cityhall;
            let op = Operation {
                operation: "run".into(),
                id: "does-not-exist".into(),
                revision: None,
                original_prompt: None,
                inputs: None,
                hidden: None,
            };
            assert_eq!(
                research_operation(State(state.clone()), Ok(Json(op)))
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                research_reply(
                    State(state.clone()),
                    Path("missing".into()),
                    Ok(Json(ResearchReply {
                        answer: "anything".into()
                    }))
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            if cityhall {
                assert_eq!(
                    research_get(State(state.clone()), Path("missing".into()))
                        .await
                        .status(),
                    StatusCode::FORBIDDEN
                );
                assert_eq!(
                    research_output(State(state), Path("missing".into()))
                        .await
                        .status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        assert!(!dir.path().join("research.json").exists());
    }
    #[tokio::test]
    #[serial_test::serial]
    async fn create_edit_reopen_and_visibility_are_durable_without_model_or_backend() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(dir.path());
        let state = crate::server::test_support::build_test_app_state(vec![]);
        let id = uuid::Uuid::new_v4().to_string();
        let inputs = research::Inputs::new(crate::session::tailored::CaptureAccount::Work);
        for (operation, revision, hidden) in [
            ("create", None, None),
            ("edit", Some(0), None),
            ("visibility", None, Some(true)),
        ] {
            let op = Operation {
                operation: operation.into(),
                id: id.clone(),
                revision,
                original_prompt: Some("Original question".into()),
                inputs: Some(inputs.clone()),
                hidden,
            };
            assert_eq!(
                research_operation(State(state.clone()), Ok(Json(op)))
                    .await
                    .status(),
                StatusCode::OK
            );
        }
        let response = research_get(State(state.clone()), Path(id.clone())).await;
        let bytes = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["draft"]["original_prompt"], "Original question");
        assert_eq!(json["draft"]["state"], "draft");
        assert_eq!(json["draft"]["hidden"], true);
        assert_eq!(
            research_output(State(state), Path(id)).await.status(),
            StatusCode::CONFLICT
        );
    }
}
