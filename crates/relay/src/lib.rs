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
    // Embedded at compile time so the relay binary is self-contained (no CWD/file deps).
    Html(include_str!("../../../viewer/index.html"))
}

const MAX_WS_MESSAGE: usize = 1 << 20; // 1 MiB — guard against a frame-bomb OOM.

async fn agent_ws(Path(id): Path<String>, State(s): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| run_side(s.registry, id, Role::Agent, socket))
}

async fn viewer_ws(Path(id): Path<String>, State(s): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| run_side(s.registry, id, Role::Viewer, socket))
}
