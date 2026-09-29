//! Per-session usage summary for the web overlay.

use axum::response::Response;

use super::*;

pub async fn session_usage(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(instance) = find_instance(&state, &id).await else {
        return bare_not_found();
    };
    let summary = tokio::task::spawn_blocking(move || load_summary(&instance.id))
        .await
        .unwrap_or_default();
    Json(summary).into_response()
}

/// A missing or unreadable log reads as "no events".
fn load_summary(instance_id: &str) -> crate::usage::UsageSummary {
    crate::usage::UsageStore::open_default()
        .and_then(|store| store.summary_for(instance_id))
        .unwrap_or_default()
}
