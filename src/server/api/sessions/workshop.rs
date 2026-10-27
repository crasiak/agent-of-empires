//! Workshop operations run off the async executor and share the native Store.
use super::*;
use crate::session::tailored::workshop::{EffectOutcomeUncertain, Operation, Store};

fn guard(state: &AppState, mutate: bool) -> Option<axum::response::Response> {
    if mutate && state.read_only {
        return Some(crate::server::api::read_only_response());
    }
    crate::server::api::cityhall_block(state)
}
pub async fn workshop_operation(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Operation>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    if let Some(response) = guard(&state, true) {
        return response;
    }
    let Json(operation) = match body {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    match tokio::task::spawn_blocking(move || Store::open()?.operate(operation)).await {
        Ok(Ok(request)) => Json(serde_json::json!({"request":request})).into_response(),
        Ok(Err(error)) => operation_error(error),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "workshop_operation_uncertain", "Outcome unknown. Reopen the same Workshop UUID and reconcile; do not create a replacement or replay a commit."),
    }
}
fn operation_error(error: anyhow::Error) -> axum::response::Response {
    if error.downcast_ref::<EffectOutcomeUncertain>().is_some() {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "workshop_operation_uncertain", "Effect outcome unknown. Reopen the same UUID and inspect retained intent; never replay a write.")
    } else {
        api_error(
            StatusCode::CONFLICT,
            "workshop_operation_failed",
            format!("{error:#}"),
        )
    }
}

pub async fn workshop_get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    if let Some(response) = guard(&state, false) {
        return response;
    }
    match tokio::task::spawn_blocking(move || Store::open()?.get(&id)).await {
        Ok(Ok(request)) => Json(serde_json::json!({"request":request})).into_response(),
        _ => api_error(
            StatusCode::NOT_FOUND,
            "workshop_not_found",
            "Workshop request unavailable",
        ),
    }
}
pub async fn workshop_defaults(State(state): State<Arc<AppState>>) -> axum::response::Response {
    if let Some(response) = guard(&state, false) {
        return response;
    }
    match tokio::task::spawn_blocking(|| {
        crate::session::config::Config::load().map(|config| config.skills.workshop_repository)
    })
    .await
    {
        Ok(Ok(repository)) => Json(serde_json::json!({"repository":repository})).into_response(),
        _ => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "workshop_defaults_unavailable",
            "Cannot read the configured Workshop repository; no path was guessed",
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn post_effect_persistence_error_is_http_unknown_but_preflight_refusal_remains_conflict()
    {
        let response = operation_error(
            EffectOutcomeUncertain::new(anyhow::anyhow!("settlement storage unavailable")).into(),
        );
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["error"], "workshop_operation_uncertain");
        assert_eq!(
            operation_error(anyhow::anyhow!("stale approval before effect")).status(),
            StatusCode::CONFLICT
        );
    }
    #[tokio::test]
    #[serial_test::serial]
    async fn workshop_denies_readonly_cityhall_before_any_store_or_process_effect() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(dir.path());
        for (readonly, cityhall) in [(true, false), (false, true)] {
            let mut state = crate::server::test_support::build_test_app_state(vec![]);
            Arc::get_mut(&mut state).unwrap().read_only = readonly;
            Arc::get_mut(&mut state).unwrap().cityhall_mode = cityhall;
            let values = [
                serde_json::json!({"operation":"commit","id":"missing","revision":0,"review_digest":"none"}),
                serde_json::json!({"operation":"preview-publish","id":"missing","revision":0,"remote":"https://github.com/a/b.git","repository_id":1,"base_branch":"main","title":"x","body":""}),
                serde_json::json!({"operation":"publish","id":"missing","revision":0,"preview_digest":"none","disclosure_confirmed":true}),
                serde_json::json!({"operation":"continue-publish","id":"missing","revision":0}),
                serde_json::json!({"operation":"preview-land","id":"missing","revision":0}),
                serde_json::json!({"operation":"land","id":"missing","revision":0,"policy_digest":"none","current_base_and_rules":true,"exclusive_pr_custody":true}),
                serde_json::json!({"operation":"reconcile-publication","id":"missing","revision":0}),
                serde_json::json!({"operation":"preview-import","id":"missing","revision":0,"source_name":"fixture","targets":[]}),
                serde_json::json!({"operation":"approve-import","id":"missing","revision":0,"preview_digest":"none"}),
                serde_json::json!({"operation":"preview-deploy","id":"missing","revision":0}),
                serde_json::json!({"operation":"deploy","id":"missing","revision":0,"candidate_digest":"none"}),
                serde_json::json!({"operation":"materialize","id":"missing","revision":0,"candidate_digest":"none"}),
                serde_json::json!({"operation":"preview-rollout","id":"missing","revision":0}),
                serde_json::json!({"operation":"apply-rollout","id":"missing","revision":0,"plan_digest":"none"}),
                serde_json::json!({"operation":"reconcile-deploy","id":"missing","revision":0}),
            ];
            for value in values {
                let operation: Operation = serde_json::from_value(value).unwrap();
                assert_eq!(
                    workshop_operation(State(state.clone()), Ok(Json(operation)))
                        .await
                        .status(),
                    StatusCode::FORBIDDEN
                );
            }
            if cityhall {
                assert_eq!(
                    workshop_get(State(state.clone()), Path("missing".into()))
                        .await
                        .status(),
                    StatusCode::FORBIDDEN
                );
                assert_eq!(
                    workshop_defaults(State(state)).await.status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        assert!(!dir.path().join("workshop.json").exists());
    }
    #[tokio::test]
    #[serial_test::serial]
    async fn workshop_api_reopens_keyed_request_and_refuses_unapproved_land() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(dir.path());
        let repo_path = dir.path().join("repo");
        let repo = git2::Repository::init(&repo_path).unwrap();
        let tree = repo
            .find_tree(repo.treebuilder(None).unwrap().write().unwrap())
            .unwrap();
        let sig = git2::Signature::now("fixture", "fixture@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
            .unwrap();
        let state = crate::server::test_support::build_test_app_state(vec![]);
        let id = uuid::Uuid::new_v4().to_string();
        let op = Operation::Create {
            id: id.clone(),
            envelope: crate::session::tailored::workshop::CreateEnvelope {
                account: crate::session::tailored::CaptureAccount::Personal,
                idea: "fixture".into(),
                repository: repo_path.canonicalize().unwrap(),
                slug: "fixture".into(),
                source_scope: "public fixture".into(),
            },
        };
        for _ in 0..2 {
            assert_eq!(
                workshop_operation(State(state.clone()), Ok(Json(op.clone())))
                    .await
                    .status(),
                StatusCode::OK
            );
        }
        assert_eq!(
            workshop_get(State(state.clone()), Path(id.clone()))
                .await
                .status(),
            StatusCode::OK
        );
        let history = list_tailored_sessions(State(state.clone())).await;
        let bytes = axum::body::to_bytes(history.into_body(), 128 * 1024)
            .await
            .unwrap();
        let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(data["entries"][0]["workshop"]["id"], id);
        assert_eq!(
            workshop_operation(
                State(state),
                Ok(Json(Operation::Land {
                    id,
                    revision: 0,
                    policy_digest: "not authority".into(),
                    current_base_and_rules: false,
                    exclusive_pr_custody: false
                }))
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
    }
}
