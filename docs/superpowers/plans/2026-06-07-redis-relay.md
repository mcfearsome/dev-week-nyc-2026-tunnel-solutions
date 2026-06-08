# Redis Multi-Machine Relay Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the relay's single-machine constraint: pair tunnels across instances via a pluggable `Backplane` (in-memory today, Redis pub/sub for scale), preserving today's zero-dependency local behavior and all existing metrics.

**Architecture:** Extract the pairing seam out of `run_side` into a `Backplane` trait (`register`/`forward`/`deregister`). `LocalBackplane` = current in-memory behavior. `RedisBackplane` = cross-instance via Redis presence keys + pub/sub. `run_side` keeps the writer pump + `stats` calls; it routes pairing through the backplane. `build_app()` stays sync+Local (backward compat); `build_app_with(backplane)` injects Redis (prod) or a shared Local (cross-instance test).

**Tech Stack:** Rust, Axum, `async-trait`, `bytes`, `redis` (async/tokio, pub/sub), Fly Upstash Redis.

**Spec:** `docs/superpowers/specs/2026-06-07-redis-relay-design.md`. Read it first.

**Verified current code (`crates/relay/src/pairing.rs`):**
- `run_side(reg: Registry, stats: Arc<Stats>, id: String, role: Role, socket: WebSocket)`.
- `Tx = mpsc::UnboundedSender<axum::extract::ws::Message>`, `Registry = Arc<Mutex<HashMap<String, TunnelSlot>>>`, `TunnelSlot { agent_tx, viewer_tx }`, `Role { Agent, Viewer }`.
- Capacity cap via `over_capacity(len, id_present, max)` + `max_tunnels()` (env `MAX_TUNNELS`, default 256).
- Metrics: `stats.tunnel_opened()` on agent register; `stats.frame_relayed()` per forwarded frame.
- Teardown today: `map.remove(&id)` (drops both Tx → cascade). `lib.rs` `build_app()` builds `AppState { registry, stats }` and `agent_ws/viewer_ws` call `run_side(s.registry, s.stats, …)`.

---

## File Structure

| Path | Change |
|------|--------|
| `crates/relay/src/backplane.rs` | New: `Delivery`, `Tx`, `RegisterError`, `Backplane` trait, `LocalBackplane`, `RedisBackplane`. |
| `crates/relay/src/pairing.rs` | `run_side` rewritten to route pairing through `Arc<dyn Backplane>`; keep `stats` + capacity + writer pump. `Role` moves here or to backplane (keep in `pairing`, re-export). |
| `crates/relay/src/lib.rs` | `AppState { backplane, stats }`; `build_app()` (sync, Local) + `build_app_with(Arc<dyn Backplane>)`; handlers pass `s.backplane.clone()`. |
| `crates/relay/src/main.rs` | If `REDIS_URL` set → connect Redis, `build_app_with(RedisBackplane)`; else `build_app()`. |
| `crates/relay/tests/cross_instance.rs` | New: two in-process apps sharing one `LocalBackplane` → frame bridges across them. |
| `crates/relay/Cargo.toml` | + `async-trait`, `bytes`, `redis`. |
| `fly.toml` | scale-out (drop count=1) + `REDIS_URL`. |

---

## Chunk 1: The backplane seam (Local + cross-instance, no Redis)

### Task 1: `Backplane` trait + `LocalBackplane`

**Files:** Create `crates/relay/src/backplane.rs`; modify `lib.rs` (add `pub mod backplane;`), `Cargo.toml`.

- [ ] **Step 1: Add deps** — `crates/relay/Cargo.toml`: `async-trait = "0.1"`, `bytes = "1"`.

- [ ] **Step 2: Write `backplane.rs` with `LocalBackplane` + unit tests**

```rust
use crate::pairing::Role;
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::{mpsc, Mutex};

/// What gets delivered to a local socket's writer pump.
pub enum Delivery { Frame(Bytes), Close }
pub type Tx = mpsc::UnboundedSender<Delivery>;

#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError { Occupied, AtCapacity }

#[async_trait]
pub trait Backplane: Send + Sync {
    /// Claim `role` for `id`; `tx` delivers frames/close to THIS side's socket.
    async fn register(&self, id: &str, role: Role, tx: Tx) -> Result<(), RegisterError>;
    /// Deliver `frame` to the PEER role.
    async fn forward(&self, id: &str, from: Role, frame: Bytes);
    /// Release this side; signal the peer to close.
    async fn deregister(&self, id: &str, role: Role);
}

pub fn max_tunnels() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| std::env::var("MAX_TUNNELS").ok().and_then(|v| v.parse().ok()).unwrap_or(256))
}

#[derive(Default)]
struct Slot { agent: Option<Tx>, viewer: Option<Tx> }
impl Slot {
    fn get(&self, r: Role) -> &Option<Tx> { match r { Role::Agent => &self.agent, Role::Viewer => &self.viewer } }
    fn set(&mut self, r: Role, tx: Option<Tx>) { match r { Role::Agent => self.agent = tx, Role::Viewer => self.viewer = tx } }
    fn peer(&self, from: Role) -> Option<Tx> { self.get(match from { Role::Agent => Role::Viewer, Role::Viewer => Role::Agent }).clone() }
    fn empty(&self) -> bool { self.agent.is_none() && self.viewer.is_none() }
}

pub struct LocalBackplane { map: Mutex<HashMap<String, Slot>>, max: usize }
impl LocalBackplane { pub fn new(max: usize) -> Self { Self { map: Mutex::new(HashMap::new()), max } } }

#[async_trait]
impl Backplane for LocalBackplane {
    async fn register(&self, id: &str, role: Role, tx: Tx) -> Result<(), RegisterError> {
        let mut m = self.map.lock().await;
        if !m.contains_key(id) && m.len() >= self.max { return Err(RegisterError::AtCapacity); }
        let slot = m.entry(id.to_string()).or_default();
        if slot.get(role).is_some() { return Err(RegisterError::Occupied); }
        slot.set(role, Some(tx));
        Ok(())
    }
    async fn forward(&self, id: &str, from: Role, frame: Bytes) {
        let peer = { self.map.lock().await.get(id).and_then(|s| s.peer(from)) };
        if let Some(p) = peer { let _ = p.send(Delivery::Frame(frame)); }
    }
    async fn deregister(&self, id: &str, role: Role) {
        let mut m = self.map.lock().await;
        if let Some(slot) = m.get_mut(id) {
            if let Some(p) = slot.peer(role) { let _ = p.send(Delivery::Close); }
            slot.set(role, None);
            if slot.empty() { m.remove(id); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ch() -> (Tx, mpsc::UnboundedReceiver<Delivery>) { mpsc::unbounded_channel() }

    #[tokio::test]
    async fn forwards_to_peer_and_rejects_duplicate() {
        let bp = LocalBackplane::new(256);
        let (atx, _arx) = ch(); let (vtx, mut vrx) = ch();
        bp.register("t", Role::Agent, atx).await.unwrap();
        bp.register("t", Role::Viewer, vtx).await.unwrap();
        // duplicate agent rejected
        let (a2, _) = ch();
        assert_eq!(bp.register("t", Role::Agent, a2).await, Err(RegisterError::Occupied));
        // agent → viewer
        bp.forward("t", Role::Agent, Bytes::from_static(b"hi")).await;
        match vrx.recv().await.unwrap() { Delivery::Frame(b) => assert_eq!(&b[..], b"hi"), _ => panic!() }
    }

    #[tokio::test]
    async fn deregister_closes_peer() {
        let bp = LocalBackplane::new(256);
        let (atx, mut arx) = ch(); let (vtx, _vrx) = ch();
        bp.register("t", Role::Agent, atx).await.unwrap();
        bp.register("t", Role::Viewer, vtx).await.unwrap();
        bp.deregister("t", Role::Viewer).await;
        match arx.recv().await.unwrap() { Delivery::Close => {}, _ => panic!("agent should get Close") }
    }

    #[tokio::test]
    async fn capacity_rejects_new_when_full() {
        let bp = LocalBackplane::new(1);
        let (a, _) = ch(); bp.register("t1", Role::Agent, a).await.unwrap();
        let (b, _) = ch();
        assert_eq!(bp.register("t2", Role::Agent, b).await, Err(RegisterError::AtCapacity));
    }
}
```

Add `pub mod backplane;` to `lib.rs`.

- [ ] **Step 3:** `cargo test -p relay backplane` → PASS (3). Commit: `feat(relay): Backplane trait + LocalBackplane`.

### Task 2: Route `run_side` through the backplane

**Files:** Modify `crates/relay/src/pairing.rs`, `crates/relay/src/lib.rs`.

- [ ] **Step 1: Rewrite `run_side`** in `pairing.rs` (keep `Role`, drop `Tx`/`TunnelSlot`/`Registry`/`over_capacity`/`max_tunnels` — those move to `backplane.rs`; keep the `tests` mod only if it still references local items, otherwise remove it since capacity is now tested in backplane.rs):
```rust
use crate::backplane::{Backplane, Delivery};
use crate::stats::Stats;
use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role { Agent, Viewer }

pub async fn run_side(bp: Arc<dyn Backplane>, stats: Arc<Stats>, id: String, role: Role, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Delivery>();

    if bp.register(&id, role, tx).await.is_err() {
        let _ = sink.send(Message::Close(None)).await; // Occupied or AtCapacity
        return;
    }
    if matches!(role, Role::Agent) { stats.tunnel_opened(); }

    // Writer pump: Frame → socket; Close → shut down.
    let writer = tokio::spawn(async move {
        while let Some(d) = rx.recv().await {
            match d {
                Delivery::Frame(b) => {
                    if sink.send(Message::Text(String::from_utf8_lossy(&b).into_owned())).await.is_err() { break; }
                }
                Delivery::Close => break,
            }
        }
        let _ = sink.close().await;
    });

    // Reader loop: inbound → backplane.
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(t) => { stats.frame_relayed(); bp.forward(&id, role, Bytes::from(t)).await; }
            Message::Binary(b) => { stats.frame_relayed(); bp.forward(&id, role, Bytes::from(b)).await; }
            Message::Close(_) => break,
            _ => {}
        }
    }

    bp.deregister(&id, role).await;
    writer.abort();
}
```

- [ ] **Step 2: Update `lib.rs`** — `AppState { backplane: Arc<dyn Backplane>, stats: Arc<Stats> }`; add `build_app_with`; `build_app()` delegates with a fresh `LocalBackplane`:
```rust
use crate::backplane::{Backplane, LocalBackplane, max_tunnels};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState { pub backplane: Arc<dyn Backplane>, pub stats: Arc<Stats> }

pub fn build_app() -> Router { build_app_with(Arc::new(LocalBackplane::new(max_tunnels()))) }

pub fn build_app_with(backplane: Arc<dyn Backplane>) -> Router {
    // MOVE the ENTIRE existing stats setup from build_app into here (do not leave it behind,
    // or build_app_with — used by Redis + the cross-instance test — gets no stats):
    //   let mut salt = [0u8; 8]; let _ = getrandom::getrandom(&mut salt);
    //   let stats = Arc::new(Stats::new(std::env::var("STATS_PATH").ok().map(PathBuf::from),
    //                                   u64::from_le_bytes(salt)));
    //   { let stats = stats.clone(); tokio::spawn(async move { loop {
    //       tokio::time::sleep(Duration::from_secs(10)).await; stats.flush(); } }); }
    let state = AppState { backplane, stats };
    Router::new() /* …ALL existing routes unchanged… */ .with_state(state)
}
```
Change `agent_ws`/`viewer_ws` to call `run_side(s.backplane.clone(), s.stats, id, Role::Agent|Viewer, socket)`.

- [ ] **Step 3:** `cargo test -p relay` → the existing `tests/pairing.rs` (single instance) still passes (it uses `build_app()` = Local). Then confirm **all** `relay::build_app()` callers still compile: `cargo build --workspace --tests` (agent, tnls, **and** tunnel-locker tests each call `build_app()`). Commit: `refactor(relay): run_side routes pairing through Backplane`.

### Task 3: Cross-instance pairing test (hermetic, shared Local)

**Files:** Create `crates/relay/tests/cross_instance.rs`.

- [ ] **Step 1: Write the test** — two apps, one shared backplane, frame bridges:
```rust
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as TMsg;

async fn serve(backplane: Arc<relay::backplane::LocalBackplane>) -> u16 {
    let app = relay::build_app_with(backplane);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap(); });
    port
}

#[tokio::test]
async fn frame_bridges_across_two_instances() {
    let shared = Arc::new(relay::backplane::LocalBackplane::new(256)); // stands in for Redis
    let a = serve(shared.clone()).await; // instance A
    let b = serve(shared.clone()).await; // instance B (same backplane)

    // agent connects to A, viewer to B
    let (mut agent, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{a}/agent/tid")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (mut viewer, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{b}/viewer/tid")).await.unwrap();

    // viewer (on B) → agent (on A), bridged through the shared backplane
    viewer.send(TMsg::Text(r#"{"type":"hello"}"#.into())).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(2), agent.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(got.into_text().unwrap(), r#"{"type":"hello"}"#);
}
```
(This requires `pub use` of `LocalBackplane`/`build_app_with` from `relay` — ensure `backplane` mod + items are `pub`.)

- [ ] **Step 2:** `cargo test -p relay --test cross_instance` → PASS. Commit: `test(relay): cross-instance pairing via shared backplane`.

**Chunk 1 gate:** the backplane seam works; existing single-instance behavior preserved; cross-instance pairing proven hermetically with no Redis. `cargo test -p relay` green.

---

## Chunk 2: `RedisBackplane` + scale-out deploy

> The `redis` crate API (version, `ConnectionManager`, pub/sub) is confirmed at the start of this chunk against docs.rs — like librqbit in tnls Phase 1. Pin a recent version (e.g. `redis = { version = "0.27", features = ["tokio-comp", "connection-manager"] }`) and adjust to the actual API.

### Task 4: `RedisBackplane`

**Files:** Modify `crates/relay/src/backplane.rs`, `crates/relay/Cargo.toml`, `crates/relay/src/main.rs`.

- [ ] **Step 1:** Add `redis` to `Cargo.toml`.

- [ ] **Step 2:** Implement `RedisBackplane` per the spec:
  - **State:** a command `ConnectionManager` (multiplexed) for `SET`/`DEL`/`PUBLISH`; a `local: Mutex<HashMap<String, Slot>>` (same as Local — for the same-instance fast path + delivering frames to local sockets); a `dispatch: Mutex<HashMap<String /*channel*/, Tx>>` mapping `tunnel:<id>:to_<role>` → the local socket's `Tx`; `instance_id` + `max` + `ttl`.
  - **Subscriber task** (spawned in `new`): one dedicated pub/sub connection `PSUBSCRIBE tunnel:*`; on each message, look up its exact channel in `dispatch`; if present, decode the tagged payload (`{"k":"f","b":"…"}` → `Delivery::Frame(Bytes)`, `{"k":"c"}` → `Delivery::Close`) and send to that `Tx`. (Pattern-subscribe avoids dynamic SUBSCRIBE churn; every instance receives all tunnel frames but delivers only those it holds — acceptable for v1; per-channel SUBSCRIBE is a future optimization.)
  - **register:** `SET tunnel:<id>:<role> <instance> NX EX <ttl>` → `Occupied` if it returns nil; also check a local/`DBSIZE`-style cap → `AtCapacity` (v1: per-instance cap via `local.len()`). Insert into `local` (for fast path + to know "is the peer local") and `dispatch[tunnel:<id>:to_<role>] = tx`. Start a heartbeat (a per-key `EXPIRE` refresh, or a single sweep task refreshing all this instance's keys every `ttl/2`).
  - **forward:** if the peer's `Tx` is in `local` → deliver `Delivery::Frame` directly (fast path). Else `PUBLISH tunnel:<id>:to_<peer_role>` the frame-tagged payload.
  - **deregister:** `DEL tunnel:<id>:<role>`; deliver `Close` to the peer locally if present, else `PUBLISH … to_<peer_role>` a close-tagged payload; remove from `local` + `dispatch`.
  - Wire tag: a tiny JSON envelope `{"k":"f"|"c","b":<text>}`.

- [ ] **Step 3:** `main.rs`: if `std::env::var("REDIS_URL")` is `Ok` → `let bp = RedisBackplane::connect(&url, max_tunnels()).await?; axum::serve(listener, relay::build_app_with(Arc::new(bp)))` else `relay::build_app()`.

- [ ] **Step 4:** `cargo build -p relay`. Optional `#[ignore]` integration test `tests/redis_backplane.rs` that runs only when `REDIS_URL` is set (two `RedisBackplane` instances, register A/B, forward bridges). Commit: `feat(relay): RedisBackplane (pub/sub backplane for multi-instance)`.

### Task 5: Provision Redis + scale out

- [ ] **Step 1:** Provision Fly Upstash Redis and attach: `fly redis create` (or `fly ext redis create`), then set the secret: `fly secrets set REDIS_URL=<url> -a tunnel-locker`. (Manual/controller step — needs the Fly account.)
- [ ] **Step 2:** `fly.toml`: ensure no hard `count = 1`; set `min_machines_running = 2` (or scale via `fly scale count 2`). Keep `auto_stop_machines = "off"` so paired instances stay warm.
- [ ] **Step 3:** `fly deploy --remote-only` (this also brings the rendezvous `/whoami` live). Then `fly scale count 2`.
- [ ] **Step 4: Live verify multi-instance:** open a tunnel (`tunnel open … --relay wss://tunnel.locker`) and a viewer repeatedly; confirm pairing succeeds across refreshes now that ≥2 machines serve. `fly status` shows ≥2 machines, `fly logs` shows both taking connections.

**Chunk 2 gate:** `REDIS_URL` set → relay pairs across ≥2 Fly machines; unset → unchanged local behavior. The single-machine constraint is gone.

---

## Notes
- Keep the relay's "dumb" invariant: the backplane moves *opaque* frames; it never parses tunnel content.
- Our protocol frames are all `Text` JSON, so the `Bytes→Text` writer conversion is lossless in practice (binary frames, if ever sent, would be re-emitted as Text — documented, acceptable).
- Backward compat is the safety net: every existing test + the local demo run on `LocalBackplane` with no Redis.
