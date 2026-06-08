pub mod backplane;
pub mod pairing;
pub mod stats;

use axum::{
    extract::{
        ws::{Message, WebSocketUpgrade},
        Path, State,
    },
    http::HeaderMap,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use backplane::{max_tunnels, Backplane, LocalBackplane};
use futures_util::StreamExt;
use pairing::{run_side, Role};
use stats::Stats;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub backplane: Arc<dyn Backplane>,
    pub stats: Arc<Stats>,
}

pub fn build_app() -> Router {
    build_app_with(Arc::new(LocalBackplane::new(max_tunnels())))
}

pub fn build_app_with(backplane: Arc<dyn Backplane>) -> Router {
    let mut salt = [0u8; 8];
    let _ = getrandom::getrandom(&mut salt);
    let stats = Arc::new(Stats::new(
        std::env::var("STATS_PATH").ok().map(PathBuf::from),
        u64::from_le_bytes(salt),
    ));

    // Periodic flush to the persistence path (no-op when STATS_PATH is unset, e.g. local dev).
    {
        let stats = stats.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                stats.flush();
            }
        });
    }

    let state = AppState { backplane, stats };

    Router::new()
        .route("/", get(landing_page))
        .route("/healthz", get(|| async { "ok" }))
        .route("/whoami", get(whoami))
        .route("/stats", get(stats_page))
        .route("/stats.json", get(stats_json))
        .route("/t/:id", get(viewer_page))
        .route("/agent/:id", get(agent_ws))
        .route("/viewer/:id", get(viewer_ws))
        .route("/report", get(report_ws))
        .with_state(state)
}

/// Real client IP for the salted-hash visitor dedupe (never stored). Behind Cloudflare's proxy
/// Fly sees CF's edge IP, so prefer `cf-connecting-ip` (the real visitor); fall back to Fly's
/// header (direct), then XFF, then "unknown".
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("cf-connecting-ip")
        .or_else(|| headers.get("fly-client-ip"))
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

async fn whoami(headers: HeaderMap) -> String {
    client_ip(&headers) // "" when no proxy header (local); the caller's public IP behind Fly
}

async fn landing_page(State(s): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    s.stats.page_view(&client_ip(&headers));
    Html(include_str!("../../../viewer/landing.html"))
}

async fn stats_page() -> impl IntoResponse {
    Html(include_str!("../../../viewer/stats.html"))
}

async fn stats_json(State(s): State<AppState>) -> impl IntoResponse {
    Json(s.stats.snapshot())
}

async fn viewer_page(State(s): State<AppState>) -> impl IntoResponse {
    s.stats.tunnel_link_opened();
    // Embedded at compile time so the relay binary is self-contained (no CWD/file deps).
    Html(include_str!("../../../viewer/index.html"))
}

const MAX_WS_MESSAGE: usize = 1 << 20; // 1 MiB — guard against a frame-bomb OOM.

async fn agent_ws(
    Path(id): Path<String>,
    State(s): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| run_side(s.backplane, s.stats, id, Role::Agent, socket))
}

async fn viewer_ws(
    Path(id): Path<String>,
    State(s): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| run_side(s.backplane, s.stats, id, Role::Viewer, socket))
}

/// Aggregate-only usage report from an agent on shutdown: one JSON message `{calls, blocks}`.
/// A dedicated stats path — NOT the opaque tunnel-forwarding path, so the dumb-relay invariant
/// for tunnel content is untouched.
async fn report_ws(State(s): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        if let Some(Ok(Message::Text(t))) = socket.next().await {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                let calls = v.get("calls").and_then(|x| x.as_u64()).unwrap_or(0);
                let blocks = v.get("blocks").and_then(|x| x.as_u64()).unwrap_or(0);
                s.stats.report(calls, blocks);
            }
        }
        // Socket closes on drop.
    })
}
