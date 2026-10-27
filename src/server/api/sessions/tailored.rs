//! History visibility is UI state, never a Session lifecycle mutation.

use super::*;
use crate::session::tailored::history;

#[derive(Serialize)]
struct HistorySession {
    #[serde(flatten)]
    entry: history::HistoryEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<SessionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    research: Option<crate::session::tailored::research::Draft>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workshop: Option<crate::session::tailored::workshop::Request>,
}

pub async fn list_tailored_sessions(
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    if let Some(response) = crate::server::api::cityhall_block(&state) {
        return response;
    }
    let hidden = match tokio::task::spawn_blocking(history::load_hidden).await {
        Ok(Ok(hidden)) => hidden,
        _ => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "history_unavailable",
                "Cannot read shared capture history visibility",
            )
        }
    };
    let instances = state.instances.read().await;
    let fullscreen = crate::claude_settings::read_tui_fullscreen();
    let mut entries: Vec<_> = instances
        .iter()
        .filter_map(|instance| {
            history::entry(instance, &hidden).map(|entry| HistorySession {
                entry,
                session: Some(SessionResponse::from_instance(instance, fullscreen)),
                research: None,
                workshop: None,
            })
        })
        .collect();
    drop(instances);
    match tokio::task::spawn_blocking(|| -> anyhow::Result<_> {
        let store = crate::session::tailored::research::Store::open()?;
        store.list()
    })
    .await
    {
        Ok(Ok(drafts)) => {
            for draft in drafts {
                let entry = draft.history();
                entries.push(HistorySession {
                    entry,
                    session: None,
                    research: Some(draft),
                    workshop: None,
                });
            }
        }
        _ => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "research_history_unavailable",
                "Cannot read research history",
            )
        }
    }
    match tokio::task::spawn_blocking(|| crate::session::tailored::workshop::Store::open()?.list())
        .await
    {
        Ok(Ok(requests)) => entries.extend(requests.into_iter().map(|request| HistorySession {
            entry: request.history(),
            session: None,
            research: None,
            workshop: Some(request),
        })),
        _ => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "workshop_history_unavailable",
                "Cannot read Workshop history",
            )
        }
    }
    entries.sort_by(|a, b| history::compare(&a.entry, &b.entry));
    Json(serde_json::json!({"entries": entries})).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TailoredVisibilityBody {
    hidden: bool,
}

pub async fn update_tailored_visibility(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Result<Json<TailoredVisibilityBody>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    if state.read_only {
        return crate::server::api::read_only_response();
    }
    if let Some(response) = crate::server::api::cityhall_block(&state) {
        return response;
    }
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let Some(instance) = find_instance(&state, &id).await else {
        return session_not_found();
    };
    if history::tailored_account(&instance).is_none() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "not_tailored",
            "This Session is not a Tailored Session",
        );
    }
    let profile = instance.effective_profile();
    match tokio::task::spawn_blocking(move || history::set_hidden(&profile, &id, body.hidden)).await
    {
        Ok(Ok(())) => Json(serde_json::json!({"hidden": body.hidden})).into_response(),
        _ => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "save_failed",
            "Cannot save capture history visibility",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> Instance {
        let mut instance = Instance::new("Capture", "/tmp/capture");
        instance.tool = "pi".into();
        instance.scratch = true;
        instance.source_profile = "test".into();
        instance.command = "ledger run pi --profile aoe-capture-personal --".into();
        instance
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn visibility_is_shared_reversible_and_never_mutates_the_session() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(temp.path());
        let instance = capture();
        let original = serde_json::to_value(&instance).unwrap();
        let state = crate::server::test_support::build_test_app_state(vec![instance.clone()]);
        for hidden in [true, false] {
            let response = update_tailored_visibility(
                State(state.clone()),
                Path(instance.id.clone()),
                Ok(Json(TailoredVisibilityBody { hidden })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let projection = history::entry(&instance, &history::load_hidden().unwrap()).unwrap();
            assert_eq!(projection.hidden, hidden);
            let stored = state.instances.read().await;
            assert_eq!(serde_json::to_value(&stored[0]).unwrap(), original);
            drop(stored);
            let response = list_tailored_sessions(State(state.clone())).await;
            let bytes = axum::body::to_bytes(response.into_body(), 128 * 1024)
                .await
                .unwrap();
            let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(data["entries"][0]["hidden"].as_bool(), Some(hidden));
            assert_eq!(data["entries"][0]["session"]["id"], instance.id);
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn readonly_cityhall_and_unrelated_sessions_refuse_history_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = crate::session::test_support::isolate_app_dir_at(temp.path());
        for (readonly, cityhall, tailored, expected) in [
            (true, false, true, StatusCode::FORBIDDEN),
            (false, true, true, StatusCode::FORBIDDEN),
            (false, false, false, StatusCode::BAD_REQUEST),
        ] {
            let mut instance = capture();
            if !tailored {
                instance.command = "pi".into();
            }
            let mut state =
                crate::server::test_support::build_test_app_state(vec![instance.clone()]);
            Arc::get_mut(&mut state).unwrap().read_only = readonly;
            Arc::get_mut(&mut state).unwrap().cityhall_mode = cityhall;
            let response = update_tailored_visibility(
                State(state.clone()),
                Path(instance.id.clone()),
                Ok(Json(TailoredVisibilityBody { hidden: true })),
            )
            .await;
            assert_eq!(response.status(), expected);
            assert!(history::load_hidden().unwrap().is_empty());
            if readonly {
                assert_eq!(
                    list_tailored_sessions(State(state.clone())).await.status(),
                    StatusCode::OK
                );
            }
            if cityhall {
                assert_eq!(
                    list_tailored_sessions(State(state.clone())).await.status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
    }
}
