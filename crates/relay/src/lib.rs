pub mod pairing;

use axum::{
    extract::{ws::WebSocketUpgrade, Path, State},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use pairing::{run_side, Registry, Role};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub registry: Registry,
}

pub fn build_app() -> Router {
    let state = AppState { registry: Arc::new(Mutex::new(HashMap::new())) };
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/t/:id", get(viewer_page))
        .route("/agent/:id", get(agent_ws))
        .route("/viewer/:id", get(viewer_ws))
        .with_state(state)
}

async fn viewer_page() -> impl IntoResponse {
    match tokio::fs::read_to_string("viewer/index.html").await {
        Ok(html) => Html(html).into_response(),
        Err(_) => (
            axum::http::StatusCode::NOT_FOUND,
            "viewer/index.html not found (run relay from the repo root)",
        )
            .into_response(),
    }
}

async fn agent_ws(Path(id): Path<String>, State(s): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| run_side(s.registry, id, Role::Agent, socket))
}

async fn viewer_ws(Path(id): Path<String>, State(s): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| run_side(s.registry, id, Role::Viewer, socket))
}
