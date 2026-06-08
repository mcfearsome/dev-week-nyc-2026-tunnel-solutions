pub mod backplane;
pub mod pairing;
pub mod stats;

use backplane::{max_tunnels, Backplane, LocalBackplane};
use pairing::{run_side, Role};
use rocket::fairing::AdHoc;
use rocket::futures::StreamExt;
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::content::RawHtml;
use rocket::serde::json::Json;
use rocket::{get, routes, Build, Rocket, State};
use rocket_ws as ws;
use stats::Stats;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Managed application state. Cloned out of `&State<AppState>` before being moved into the
/// `'static` WebSocket channel closures (the closure can't hold the `&State` borrow).
#[derive(Clone)]
pub struct AppState {
    pub backplane: Arc<dyn Backplane>,
    pub stats: Arc<Stats>,
}

/// 1 MiB — guard against a frame-bomb OOM. MANDATORY on the WS routes: `rocket_ws::Config`
/// defaults to 64 MiB message / 16 MiB frame, so omitting this silently regresses 64x.
const MAX_WS_MESSAGE: usize = 1 << 20;

/// The capped WS config applied to `/agent` and `/viewer` (invariant gate item 2).
fn ws_config() -> ws::Config {
    ws::Config {
        max_message_size: Some(MAX_WS_MESSAGE),
        max_frame_size: Some(MAX_WS_MESSAGE),
        ..Default::default()
    }
}

/// Real client IP for the salted-hash unique-visitor dedupe — never stored or logged raw. Behind
/// Cloudflare's proxy Fly sees CF's edge IP, so prefer `cf-connecting-ip` (the real visitor), then
/// Fly's header (direct), then XFF, then "unknown". The single typed place an IP enters memory;
/// the guard is infallible (always succeeds).
pub struct ClientIp(pub String);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for ClientIp {
    type Error = Infallible;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let ip = req
            .headers()
            .get_one("cf-connecting-ip")
            .or_else(|| req.headers().get_one("fly-client-ip"))
            .or_else(|| req.headers().get_one("x-forwarded-for"))
            .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
            .unwrap_or_else(|| "unknown".into());
        Outcome::Success(ClientIp(ip))
    }
}

#[get("/")]
fn landing_page(ip: ClientIp, st: &State<AppState>) -> RawHtml<&'static str> {
    st.stats.page_view(&ip.0);
    // Embedded at compile time so the relay binary is self-contained (no CWD/file deps).
    RawHtml(include_str!("../../../viewer/landing.html"))
}

#[get("/rendezvous")]
fn rendezvous_page(ip: ClientIp, st: &State<AppState>) -> RawHtml<&'static str> {
    st.stats.page_view(&ip.0);
    // The `rendezvous` plugin's landing page; embedded at compile time like the others.
    RawHtml(include_str!("../../../viewer/rendezvous.html"))
}

#[get("/healthz")]
fn healthz() -> &'static str {
    "ok"
}

#[get("/whoami")]
fn whoami(ip: ClientIp) -> String {
    ip.0 // "unknown" when no proxy header (local); the caller's public IP behind Fly
}

#[get("/stats")]
fn stats_page() -> RawHtml<&'static str> {
    RawHtml(include_str!("../../../viewer/stats.html"))
}

#[get("/stats.json")]
fn stats_json(st: &State<AppState>) -> Json<serde_json::Value> {
    Json(st.stats.snapshot())
}

#[get("/t/<_id>")]
fn viewer_page(_id: &str, st: &State<AppState>) -> RawHtml<&'static str> {
    st.stats.tunnel_link_opened();
    RawHtml(include_str!("../../../viewer/index.html"))
}

#[get("/agent/<id>")]
fn agent_ws(id: String, ws: ws::WebSocket, st: &State<AppState>) -> ws::Channel<'static> {
    let ws = ws.config(ws_config());
    let (bp, stats) = (st.backplane.clone(), st.stats.clone());
    ws.channel(move |stream| Box::pin(run_side(bp, stats, id, Role::Agent, stream)))
}

#[get("/viewer/<id>")]
fn viewer_ws(id: String, ws: ws::WebSocket, st: &State<AppState>) -> ws::Channel<'static> {
    let ws = ws.config(ws_config());
    let (bp, stats) = (st.backplane.clone(), st.stats.clone());
    ws.channel(move |stream| Box::pin(run_side(bp, stats, id, Role::Viewer, stream)))
}

/// Aggregate-only usage report from an agent on shutdown: one JSON message `{calls, blocks}`.
/// A dedicated stats path — NOT the opaque tunnel-forwarding path, so the dumb-relay invariant
/// for tunnel content is untouched.
#[get("/report")]
fn report_ws(ws: ws::WebSocket, st: &State<AppState>) -> ws::Channel<'static> {
    let stats = st.stats.clone();
    ws.channel(move |mut stream| {
        Box::pin(async move {
            if let Some(Ok(ws::Message::Text(t))) = stream.next().await {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    let calls = v.get("calls").and_then(|x| x.as_u64()).unwrap_or(0);
                    let blocks = v.get("blocks").and_then(|x| x.as_u64()).unwrap_or(0);
                    stats.report(calls, blocks);
                }
            }
            Ok(()) // socket closes on return
        })
    })
}

/// Build the Rocket app with the default in-memory `LocalBackplane` and the default figment.
pub fn build_rocket() -> Rocket<Build> {
    build_rocket_with(Arc::new(LocalBackplane::new(max_tunnels())))
}

/// Build the Rocket app with an injected backplane and the default figment (tests use this to
/// share one backplane across two instances). Returns `Rocket<Build>` so callers control launch.
pub fn build_rocket_with(backplane: Arc<dyn Backplane>) -> Rocket<Build> {
    configure(rocket::build(), backplane)
}

/// Apply the relay's routes, managed state, and flush fairing onto an existing `Rocket<Build>`.
/// Lets `main.rs` start from a custom `$PORT` figment while tests start from the default one;
/// both share the exact same routes/state/fairing wiring.
pub fn configure(rocket: Rocket<Build>, backplane: Arc<dyn Backplane>) -> Rocket<Build> {
    let mut salt = [0u8; 8];
    let _ = getrandom::getrandom(&mut salt);
    let stats = Arc::new(Stats::new(
        std::env::var("STATS_PATH").ok().map(PathBuf::from),
        u64::from_le_bytes(salt),
    ));

    // Periodic flush to the persistence path, tied to the server lifecycle (no-op when STATS_PATH
    // is unset, e.g. local dev). Replaces the bare tokio::spawn the Axum build used.
    let flush_stats = stats.clone();
    let flush_fairing = AdHoc::on_liftoff("stats flush", move |_rocket| {
        Box::pin(async move {
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    flush_stats.flush();
                }
            });
        })
    });

    let state = AppState { backplane, stats };

    rocket
        .manage(state)
        .mount(
            "/",
            routes![
                landing_page,
                rendezvous_page,
                healthz,
                whoami,
                stats_page,
                stats_json,
                viewer_page,
                agent_ws,
                viewer_ws,
                report_ws,
            ],
        )
        .attach(flush_fairing)
}
