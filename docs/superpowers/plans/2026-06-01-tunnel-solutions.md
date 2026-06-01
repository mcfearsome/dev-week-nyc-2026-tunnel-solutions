# tunnel.solutions Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an MCP-aware, ephemeral, capability-scoped tunnel: a teammate opens a link to your locally-running MCP server, sees only the tools you allow, and the link dies on TTL or `tunnel close`.

**Architecture:** A deliberately dumb `relay` (Axum) pairs a viewer WebSocket to an agent WebSocket by `tunnel_id` and shuttles opaque frames. The `agent` CLI (`tunnel`) spawns the MCP server as a child, speaks real MCP JSON-RPC over its stdio, mints an HMAC capability token, dials the relay outbound, and is the **sole enforcement point** for scope + TTL. The `viewer` is a static HTML page speaking a tiny custom JSON protocol. All correctness-critical logic lives as pure functions in a shared `protocol` crate.

**Tech Stack:** Rust (Cargo workspace), `tokio`, `axum` 0.7 (relay WS server), `tokio-tungstenite` (agent WS client), `serde`/`serde_json`, `hmac`+`sha2`+`base64`+`subtle` (token), `humantime` (TTL parse), `clap` (CLI), `nix` (signals). Viewer: vanilla JS, no build step.

**Spec:** `docs/superpowers/specs/2026-06-01-tunnel-solutions-design.md` — read it before starting.

**Conventions for the executor:**
- Run all commands from the repo root unless stated otherwise.
- After each task's tests pass, commit with the message shown. Commit on `main` (solo hackathon).
- `cargo test -p <crate>` runs one crate's tests; `cargo test --workspace` runs all.
- Terse code comments. Match existing style as files grow.
- Keep `axum` on `0.7.x` and `tokio-tungstenite` on `0.23.x`. axum 0.8 changes route syntax (`:id` → `{id}`, and panics on `:id`); tungstenite 0.24 changes `Message::Text` from `String` to `Utf8Bytes`. The `"0.7"`/`"0.23"` carets + committed `Cargo.lock` already enforce this — don't loosen them.

---

## File Structure

| Path | Responsibility |
|------|----------------|
| `Cargo.toml` | Workspace manifest: members + shared dependency versions. |
| `crates/protocol/src/lib.rs` | Module wiring + re-exports. |
| `crates/protocol/src/token.rs` | `Claims`, `mint`, `verify`, `TokenError` (HMAC-SHA256, constant-time). |
| `crates/protocol/src/frame.rs` | `Tool`, `ViewerFrame`, `AgentFrame`, `ErrorCode` (the viewer↔agent wire protocol). |
| `crates/protocol/src/scope.rs` | `Claims::allows`, `filter_tools`, `decide_call`/`CallDecision` (the wedge, as pure fns). |
| `crates/protocol/src/ttl.rs` | `parse_ttl` (humantime wrapper). |
| `crates/relay/src/main.rs` | Axum app: `/agent/:id`, `/viewer/:id`, `/t/:id`, `/healthz`. |
| `crates/relay/src/pairing.rs` | `TunnelSlot` map + bidirectional forward + cascade teardown. |
| `crates/relay/tests/pairing.rs` | In-process WS round-trip integration test. |
| `crates/mcp-demo/src/main.rs` | stdio JSON-RPC loop. |
| `crates/mcp-demo/src/dispatch.rs` | Pure `dispatch(req) -> Option<Value>`: `initialize`, `tools/list`, `tools/call` (`read`, `shell`). |
| `crates/agent/src/main.rs` | CLI entry: `open` / `close` dispatch. |
| `crates/agent/src/cli.rs` | `clap` arg definitions + `--scope`/`--ttl` parsing. |
| `crates/agent/src/mcp.rs` | `McpChild`: spawn server, MCP handshake, id-correlated request/response over stdio. |
| `crates/agent/src/session.rs` | The `open` event loop: relay WS, enforcement, TTL timer, shutdown. |
| `crates/agent/src/pidfile.rs` | Write/read/remove pidfile; `close` reads it and SIGTERMs. |
| `viewer/index.html` | Static viewer: connect, render filtered tools, raw-call box, errors, countdown. |
| `demo/notes.txt` | Sample file returned by the `read` tool. |
| `demo.sh` | Convenience launcher for the demo (relay + tunnel). |

---

## Chunk 1: Workspace + `protocol` crate (the pure core)

Produces a fully-tested library with zero I/O. Everything correctness-critical is here.

### Task 1.1: Workspace skeleton compiles

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/protocol/Cargo.toml`, `crates/protocol/src/lib.rs`
- Create: `crates/relay/Cargo.toml`, `crates/relay/src/main.rs`
- Create: `crates/agent/Cargo.toml`, `crates/agent/src/main.rs`
- Create: `crates/mcp-demo/Cargo.toml`, `crates/mcp-demo/src/main.rs`

- [ ] **Step 1: Write workspace `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/protocol", "crates/relay", "crates/agent", "crates/mcp-demo"]

[workspace.dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "process", "io-util", "io-std", "net", "signal", "sync", "time"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
futures-util = "0.3"
```

- [ ] **Step 2: Write each crate's `Cargo.toml`**

`crates/protocol/Cargo.toml`:
```toml
[package]
name = "protocol"
version = "0.1.0"
edition = "2021"

[dependencies]
serde.workspace = true
serde_json.workspace = true
hmac = "0.12"
sha2 = "0.10"
base64 = "0.22"
subtle = "2"
humantime = "2"
thiserror = "1"
```

`crates/relay/Cargo.toml`:
```toml
[package]
name = "relay"
version = "0.1.0"
edition = "2021"

[dependencies]
protocol = { path = "../protocol" }
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
futures-util.workspace = true
axum = { version = "0.7", features = ["ws"] }

[dev-dependencies]
tokio-tungstenite = "0.23"
```

`crates/agent/Cargo.toml`:
```toml
[package]
name = "agent"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "tunnel"
path = "src/main.rs"

[dependencies]
protocol = { path = "../protocol" }
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
futures-util.workspace = true
tokio-tungstenite = "0.23"
clap = { version = "4", features = ["derive"] }
getrandom = "0.2"
hex = "0.4"
nix = { version = "0.29", features = ["signal", "process"] }
```

`crates/mcp-demo/Cargo.toml`:
```toml
[package]
name = "mcp-demo"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
```

- [ ] **Step 3: Write placeholder source files so the workspace builds**

`crates/protocol/src/lib.rs`:
```rust
// Pure, I/O-free core. Modules added in later tasks.
```

`crates/relay/src/main.rs`:
```rust
fn main() {
    println!("relay placeholder");
}
```

`crates/agent/src/main.rs`:
```rust
fn main() {
    println!("tunnel placeholder");
}
```

`crates/mcp-demo/src/main.rs`:
```rust
fn main() {
    println!("mcp-demo placeholder");
}
```

- [ ] **Step 4: Verify the workspace builds**

Run: `cargo build --workspace`
Expected: compiles, four crates built (downloads deps on first run).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/
git commit -m "chore: scaffold cargo workspace (protocol, relay, agent, mcp-demo)"
```

### Task 1.2: Token mint/verify (HMAC-SHA256)

**Files:**
- Create: `crates/protocol/src/token.rs`
- Modify: `crates/protocol/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/protocol/src/token.rs`:
```rust
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Capability claims signed into the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub tunnel_id: String,
    pub scope: Vec<String>,
    pub exp: u64, // unix seconds
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("malformed token")]
    Malformed,
    #[error("bad signature")]
    BadSignature,
    #[error("expired")]
    Expired,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(exp: u64) -> Claims {
        Claims { tunnel_id: "abc123".into(), scope: vec!["read".into()], exp }
    }

    #[test]
    fn roundtrip_returns_same_claims() {
        let secret = b"super-secret-key";
        let c = claims(10_000);
        let token = mint(secret, &c);
        assert_eq!(verify(secret, &token, 9_000).unwrap(), c);
    }

    #[test]
    fn expired_token_rejected() {
        let secret = b"super-secret-key";
        let token = mint(secret, &claims(10_000));
        assert_eq!(verify(secret, &token, 10_000), Err(TokenError::Expired));
        assert_eq!(verify(secret, &token, 10_001), Err(TokenError::Expired));
    }

    #[test]
    fn tampered_payload_rejected() {
        let secret = b"super-secret-key";
        let token = mint(secret, &claims(10_000));
        let (_p, sig) = token.split_once('.').unwrap();
        // Swap in a different payload, keep the old signature.
        let forged_payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Claims {
                tunnel_id: "abc123".into(),
                scope: vec!["read".into(), "shell".into()], // privilege escalation attempt
                exp: 10_000,
            }).unwrap());
        let forged = format!("{forged_payload}.{sig}");
        assert_eq!(verify(secret, &forged, 9_000), Err(TokenError::BadSignature));
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = mint(b"key-a", &claims(10_000));
        assert_eq!(verify(b"key-b", &token, 9_000), Err(TokenError::BadSignature));
    }

    #[test]
    fn garbage_is_malformed() {
        assert_eq!(verify(b"k", "not-a-token", 0), Err(TokenError::Malformed));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p protocol token`
Expected: FAIL — `mint`/`verify` not found (won't compile yet).

- [ ] **Step 3: Implement `mint` and `verify`**

Add above the `#[cfg(test)]` block in `token.rs`:
```rust
/// `b64url(payload_json) + "." + b64url(HMAC_SHA256(secret, payload_json))`.
pub fn mint(secret: &[u8], claims: &Claims) -> String {
    let payload = serde_json::to_vec(claims).expect("claims serialize");
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&payload);
    let sig = mac.finalize().into_bytes();
    format!("{}.{}", URL_SAFE_NO_PAD.encode(&payload), URL_SAFE_NO_PAD.encode(sig))
}

/// Verify signature (constant-time) then check `exp > now`. Signature is checked
/// before parsing claims so a forged token never reaches `serde_json`.
pub fn verify(secret: &[u8], token: &str, now: u64) -> Result<Claims, TokenError> {
    let (p_b64, s_b64) = token.split_once('.').ok_or(TokenError::Malformed)?;
    let payload = URL_SAFE_NO_PAD.decode(p_b64).map_err(|_| TokenError::Malformed)?;
    let sig = URL_SAFE_NO_PAD.decode(s_b64).map_err(|_| TokenError::Malformed)?;

    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&payload);
    let expected = mac.finalize().into_bytes();
    if expected.as_slice().ct_eq(&sig).unwrap_u8() != 1 {
        return Err(TokenError::BadSignature);
    }

    let claims: Claims = serde_json::from_slice(&payload).map_err(|_| TokenError::Malformed)?;
    if claims.exp <= now {
        return Err(TokenError::Expired);
    }
    Ok(claims)
}
```

Add to `crates/protocol/src/lib.rs`:
```rust
pub mod token;
pub use token::{mint, verify, Claims, TokenError};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p protocol token`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/protocol/
git commit -m "feat(protocol): HMAC capability token mint/verify with TTL"
```

### Task 1.3: Frames (viewer↔agent wire protocol)

**Files:**
- Create: `crates/protocol/src/frame.rs`
- Modify: `crates/protocol/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/protocol/src/frame.rs`:
```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP tool shape, passed through to the viewer. `inputSchema` stays camelCase on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

/// viewer -> agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ViewerFrame {
    Hello { token: String },
    List,
    Call { id: u64, tool: String, #[serde(default)] args: Value },
}

/// agent -> viewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentFrame {
    Ready { scope: Vec<String>, expires_in_ms: u64 },
    Tools { tools: Vec<Tool> },
    Result { id: u64, content: Value },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        code: ErrorCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    Expired,
    OutOfScope,
    ToolError,
    BadRequest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hello_parses_from_viewer_json() {
        let v: ViewerFrame = serde_json::from_str(r#"{"type":"hello","token":"t"}"#).unwrap();
        assert_eq!(v, ViewerFrame::Hello { token: "t".into() });
    }

    #[test]
    fn call_defaults_missing_args() {
        let v: ViewerFrame = serde_json::from_str(r#"{"type":"call","id":7,"tool":"read"}"#).unwrap();
        assert_eq!(v, ViewerFrame::Call { id: 7, tool: "read".into(), args: Value::Null });
    }

    #[test]
    fn error_code_serializes_snake_case() {
        let f = AgentFrame::Error { id: Some(7), code: ErrorCode::OutOfScope, tool: Some("shell".into()), message: None };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains(r#""type":"error""#), "got {s}");
        assert!(s.contains(r#""code":"out_of_scope""#), "got {s}");
        assert!(!s.contains(r#""message""#), "None fields must be omitted: {s}");
    }

    #[test]
    fn tool_roundtrips_with_camelcase_schema() {
        let t = Tool { name: "read".into(), description: Some("Read a file".into()), input_schema: json!({"type":"object"}) };
        let s = serde_json::to_string(&t).unwrap();
        assert!(s.contains(r#""inputSchema""#), "got {s}");
        assert_eq!(serde_json::from_str::<Tool>(&s).unwrap(), t);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p protocol frame`
Expected: FAIL — module not wired into `lib.rs`.

- [ ] **Step 3: Wire the module**

Add to `crates/protocol/src/lib.rs`:
```rust
pub mod frame;
pub use frame::{AgentFrame, ErrorCode, Tool, ViewerFrame};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p protocol frame`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/protocol/
git commit -m "feat(protocol): viewer<->agent frame enums"
```

### Task 1.4: Scope + the enforcement decision (the wedge)

**Files:**
- Create: `crates/protocol/src/scope.rs`
- Modify: `crates/protocol/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/protocol/src/scope.rs`:
```rust
use crate::frame::Tool;
use crate::token::Claims;

impl Claims {
    pub fn allows(&self, tool: &str) -> bool {
        self.scope.iter().any(|s| s == tool)
    }
}

/// Keep only tools whose name is in scope. Produces the filtered `tools/list`.
pub fn filter_tools(all: &[Tool], scope: &[String]) -> Vec<Tool> {
    all.iter().filter(|t| scope.iter().any(|s| s == &t.name)).cloned().collect()
}

/// The per-call enforcement decision. Order matters: TTL is checked before scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallDecision {
    Forward,
    Expired,
    OutOfScope,
}

pub fn decide_call(claims: &Claims, now: u64, tool: &str) -> CallDecision {
    if now >= claims.exp {
        return CallDecision::Expired;
    }
    if !claims.allows(tool) {
        return CallDecision::OutOfScope;
    }
    CallDecision::Forward
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claims() -> Claims {
        Claims { tunnel_id: "t".into(), scope: vec!["read".into()], exp: 1000 }
    }

    fn tool(name: &str) -> Tool {
        Tool { name: name.into(), description: None, input_schema: json!({}) }
    }

    #[test]
    fn filter_keeps_only_in_scope() {
        let all = vec![tool("read"), tool("shell")];
        let kept = filter_tools(&all, &["read".into()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "read");
    }

    #[test]
    fn empty_scope_filters_everything() {
        let all = vec![tool("read"), tool("shell")];
        assert!(filter_tools(&all, &[]).is_empty());
    }

    #[test]
    fn in_scope_and_valid_forwards() {
        assert_eq!(decide_call(&claims(), 999, "read"), CallDecision::Forward);
    }

    #[test]
    fn out_of_scope_refused() {
        assert_eq!(decide_call(&claims(), 999, "shell"), CallDecision::OutOfScope);
    }

    #[test]
    fn expired_beats_out_of_scope() {
        // Both conditions true; expiry must win (revocation is absolute).
        assert_eq!(decide_call(&claims(), 1000, "shell"), CallDecision::Expired);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p protocol scope`
Expected: FAIL — module not wired.

- [ ] **Step 3: Wire the module**

Add to `crates/protocol/src/lib.rs`:
```rust
pub mod scope;
pub use scope::{decide_call, filter_tools, CallDecision};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p protocol scope`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/protocol/
git commit -m "feat(protocol): scope filter + per-call decision (TTL beats scope)"
```

### Task 1.5: TTL parsing

**Files:**
- Create: `crates/protocol/src/ttl.rs`
- Modify: `crates/protocol/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/protocol/src/ttl.rs`:
```rust
use std::time::Duration;

/// Parse `--ttl` values like `2m`, `15m`, `90s`, `1h30m`.
pub fn parse_ttl(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| format!("invalid duration '{s}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minutes() {
        assert_eq!(parse_ttl("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_ttl("15m").unwrap(), Duration::from_secs(900));
    }

    #[test]
    fn parses_seconds_and_compound() {
        assert_eq!(parse_ttl("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_ttl("1h30m").unwrap(), Duration::from_secs(5400));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_ttl("soon").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p protocol ttl`
Expected: FAIL — module not wired.

- [ ] **Step 3: Wire the module**

Add to `crates/protocol/src/lib.rs`:
```rust
pub mod ttl;
pub use ttl::parse_ttl;
```

- [ ] **Step 4: Run all protocol tests**

Run: `cargo test -p protocol`
Expected: PASS (all tasks: ~17 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/protocol/
git commit -m "feat(protocol): parse_ttl"
```

**Chunk 1 done:** the entire correctness core is implemented and tested with no I/O. `cargo test -p protocol` is green.

---

## Chunk 2: `relay` (dumb WS pairing) — Phase 1 infrastructure

The relay never parses our protocol; it pairs two sockets by `tunnel_id` and forwards
opaque frames. Splitting `lib.rs` (logic) from `main.rs` (binding) lets the integration test
import `build_app()`.

### Task 2.1: Relay skeleton boots

**Files:**
- Create: `crates/relay/src/lib.rs`
- Create: `crates/relay/src/pairing.rs`
- Modify: `crates/relay/src/main.rs`
- Create: `viewer/index.html` (stub, replaced in Chunk 3)

- [ ] **Step 1: Stub the pairing module**

Create `crates/relay/src/pairing.rs`:
```rust
use axum::extract::ws::WebSocket;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub type Tx = mpsc::UnboundedSender<axum::extract::ws::Message>;
pub type Registry = Arc<Mutex<HashMap<String, TunnelSlot>>>;

#[derive(Default)]
pub struct TunnelSlot {
    pub agent_tx: Option<Tx>,
    pub viewer_tx: Option<Tx>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Agent,
    Viewer,
}

// Real implementation lands in Task 2.2. Stub closes the socket immediately.
pub async fn run_side(_reg: Registry, _id: String, _role: Role, _socket: WebSocket) {}
```

- [ ] **Step 2: Write `lib.rs` with `build_app()`**

Create `crates/relay/src/lib.rs`:
```rust
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
```

- [ ] **Step 3: Rewrite `main.rs` to bind and serve**

`crates/relay/src/main.rs`:
```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = "127.0.0.1:8787";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("relay listening on http://{addr}");
    axum::serve(listener, relay::build_app()).await?;
    Ok(())
}
```

- [ ] **Step 4: Create the stub viewer**

`viewer/index.html`:
```html
<!doctype html>
<meta charset="utf-8">
<title>tunnel.solutions</title>
<body style="font-family:monospace">stub viewer — replaced in Chunk 3</body>
```

- [ ] **Step 5: Verify it boots**

Run (in one terminal): `cargo run -p relay`
Expected: prints `relay listening on http://127.0.0.1:8787`.
In another terminal: `curl -s http://127.0.0.1:8787/healthz` → `ok`.
Then Ctrl-C the relay.

- [ ] **Step 6: Commit**

```bash
git add crates/relay/ viewer/index.html
git commit -m "feat(relay): axum skeleton (healthz, viewer page, ws routes)"
```

### Task 2.2: Bidirectional pairing + teardown

**Files:**
- Modify: `crates/relay/src/pairing.rs`
- Create: `crates/relay/tests/pairing.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/relay/tests/pairing.rs`:
```rust
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as TMsg;

async fn spawn_relay() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, relay::build_app()).await.unwrap();
    });
    port
}

#[tokio::test]
async fn frames_forward_both_ways() {
    let port = spawn_relay().await;
    let base = format!("ws://127.0.0.1:{port}");

    let (mut agent, _) = tokio_tungstenite::connect_async(format!("{base}/agent/tid")).await.unwrap();
    // Give the agent's upgrade task time to register before the viewer sends.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (mut viewer, _) = tokio_tungstenite::connect_async(format!("{base}/viewer/tid")).await.unwrap();

    // viewer -> agent
    viewer.send(TMsg::Text(r#"{"type":"hello","token":"x"}"#.into())).await.unwrap();
    let got = agent.next().await.unwrap().unwrap();
    assert_eq!(got.into_text().unwrap(), r#"{"type":"hello","token":"x"}"#);

    // agent -> viewer
    agent.send(TMsg::Text(r#"{"type":"tools","tools":[]}"#.into())).await.unwrap();
    let got = viewer.next().await.unwrap().unwrap();
    assert_eq!(got.into_text().unwrap(), r#"{"type":"tools","tools":[]}"#);
}

#[tokio::test]
async fn agent_disconnect_closes_viewer() {
    let port = spawn_relay().await;
    let base = format!("ws://127.0.0.1:{port}");
    let (agent, _) = tokio_tungstenite::connect_async(format!("{base}/agent/tid2")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (mut viewer, _) = tokio_tungstenite::connect_async(format!("{base}/viewer/tid2")).await.unwrap();

    drop(agent); // agent leaves
    // viewer's stream should end (None) once the relay tears down the pairing.
    let ended = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(_)) = viewer.next().await {}
    })
    .await;
    assert!(ended.is_ok(), "viewer socket should close after agent disconnect");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p relay --test pairing`
Expected: FAIL — stub `run_side` forwards nothing; `agent.next()` hangs then the test times out / errors.

- [ ] **Step 3: Implement real `run_side`**

Replace the stub in `crates/relay/src/pairing.rs` (keep the type defs above it):
```rust
use axum::extract::ws::Message;
use futures_util::{SinkExt, StreamExt};

pub async fn run_side(reg: Registry, id: String, role: Role, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Register this side; reject a duplicate role for the same tunnel.
    {
        let mut map = reg.lock().await;
        let slot = map.entry(id.clone()).or_default();
        let occupied = match role {
            Role::Agent => slot.agent_tx.is_some(),
            Role::Viewer => slot.viewer_tx.is_some(),
        };
        if occupied {
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
        match role {
            Role::Agent => slot.agent_tx = Some(tx),
            Role::Viewer => slot.viewer_tx = Some(tx),
        }
    }

    // Writer pump: our channel -> our socket.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Reader loop: inbound -> peer's channel.
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(_) | Message::Binary(_) => {
                let peer = {
                    let map = reg.lock().await;
                    map.get(&id).and_then(|slot| match role {
                        Role::Agent => slot.viewer_tx.clone(),
                        Role::Viewer => slot.agent_tx.clone(),
                    })
                };
                // No peer yet => drop (single viewer/agent, no buffering).
                if let Some(peer) = peer {
                    if peer.send(msg).is_err() {
                        break;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {} // axum auto-replies ping/pong
        }
    }

    // Teardown: remove the slot entirely; dropping the peer Tx cascades it shut.
    {
        let mut map = reg.lock().await;
        map.remove(&id);
    }
    writer.abort();
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p relay --test pairing`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/relay/
git commit -m "feat(relay): bidirectional pairing + cascade teardown"
```

**Chunk 2 done:** the dumb relay forwards frames both ways and tears down on disconnect, proven by an in-process WS test.

---

## Chunk 3: Phase 1 end-to-end echo (the "bytes flowing" checkpoint)

Per the spec's build order, prove viewer→relay→agent→relay→viewer through a real browser
**before** touching MCP. This agent + viewer are intentionally throwaway scaffolding; Chunks
5–7 replace them.

### Task 3.1: Minimal echo agent

**Files:**
- Modify: `crates/agent/src/main.rs`

- [ ] **Step 1: Write the echo agent**

`crates/agent/src/main.rs`:
```rust
// PHASE 1 SCAFFOLD: connect as the agent role and echo frames. Replaced in Chunk 5.
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let relay = std::env::var("RELAY").unwrap_or_else(|_| "ws://127.0.0.1:8787".into());
    let id = "demo";
    let url = format!("{relay}/agent/{id}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await?;
    println!("agent connected → {url}");
    println!("link → http://127.0.0.1:8787/t/{id}");
    while let Some(Ok(msg)) = ws.next().await {
        if let Message::Text(t) = msg {
            println!("recv: {t}");
            ws.send(Message::Text(format!("echo:{t}"))).await?;
        }
    }
    println!("relay closed the socket");
    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p agent`
Expected: builds (binary name `tunnel`).

- [ ] **Step 3: Commit**

```bash
git add crates/agent/
git commit -m "feat(agent): phase-1 echo scaffold"
```

### Task 3.2: Minimal echo viewer

**Files:**
- Modify: `viewer/index.html`

- [ ] **Step 1: Write the minimal viewer**

`viewer/index.html`:
```html
<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>tunnel.solutions — echo</title>
<body style="font-family:ui-monospace,monospace;background:#0b0e14;color:#cbd5e1;padding:2rem">
  <h1 style="color:#7dd3fc">tunnel.solutions <span style="color:#64748b">echo check</span></h1>
  <input id="msg" value="ping" style="background:#111827;color:#e5e7eb;border:1px solid #334155;padding:.4rem">
  <button id="send" style="background:#0ea5e9;border:0;color:#001;padding:.45rem .8rem;cursor:pointer">send →</button>
  <pre id="log" style="margin-top:1rem;white-space:pre-wrap"></pre>
<script>
  const id = location.pathname.split("/").pop() || "demo";
  const log = (s) => document.getElementById("log").textContent += s + "\n";
  const ws = new WebSocket(`ws://${location.host}/viewer/${id}`);
  ws.onopen = () => log("✓ connected as viewer/" + id);
  ws.onmessage = (e) => log("← " + e.data);
  ws.onclose = () => log("× socket closed (link dead)");
  document.getElementById("send").onclick = () => {
    const m = document.getElementById("msg").value;
    ws.send(m);
    log("→ " + m);
  };
</script>
</body>
</html>
```

- [ ] **Step 2: Manual end-to-end verification (the Phase 1 gate)**

1. Terminal 1: `cargo run -p relay`
2. Terminal 2: `cargo run -p agent` (prints the link with id `demo`)
3. Browser: open `http://127.0.0.1:8787/t/demo`
4. Confirm `✓ connected as viewer/demo`, click **send →**, observe `← echo:ping` in the page and `recv: ping` in terminal 2.
5. Ctrl-C the agent (terminal 2); the browser shows `× socket closed (link dead)`.

Expected: the echo round-trips through the relay, and agent disconnect closes the viewer. **Do not proceed until this works.**

- [ ] **Step 3: Commit**

```bash
git add viewer/index.html
git commit -m "feat(viewer): phase-1 echo page"
```

**Chunk 3 done:** dumb tunnel proven end-to-end through a real browser. Phase 1 complete.

---

## Chunk 4: `mcp-demo` (real MCP server: `read` + `shell`)

A real MCP server over newline-delimited stdio. `shell` is fully functional **at the
server** — the demo's point is that the *tunnel* refuses it under `--scope read`. Likewise
`read` accepts an arbitrary `path` (no server-side sandbox) by design: enforcement lives at
the tunnel layer, not in the tool.

### Task 4.1: Dispatch logic (pure envelope + tool execution)

**Files:**
- Create: `crates/mcp-demo/src/dispatch.rs`
- Create: `demo/notes.txt`

- [ ] **Step 1: Write the failing tests**

Create `crates/mcp-demo/src/dispatch.rs`:
```rust
use serde_json::{json, Value};

pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "mcp-demo", "version": "0.1.0" }
    })
}

pub fn tool_list() -> Value {
    json!({
        "tools": [
            {
                "name": "read",
                "description": "Read a UTF-8 text file and return its contents.",
                "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
            },
            {
                "name": "shell",
                "description": "Run a shell command and return its output.",
                "inputSchema": { "type": "object", "properties": { "cmd": { "type": "string" } }, "required": ["cmd"] }
            }
        ]
    })
}

fn text_content(text: &str) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ] })
}

pub async fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "read" => {
            let path = args.get("path").and_then(|v| v.as_str()).ok_or("missing 'path'")?;
            let body = tokio::fs::read_to_string(path).await.map_err(|e| format!("read failed: {e}"))?;
            Ok(text_content(&body))
        }
        "shell" => {
            let cmd = args.get("cmd").and_then(|v| v.as_str()).ok_or("missing 'cmd'")?;
            let out = tokio::process::Command::new("sh").arg("-c").arg(cmd).output().await
                .map_err(|e| format!("spawn failed: {e}"))?;
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.stderr.is_empty() {
                s.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            Ok(text_content(&s))
        }
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Route one JSON-RPC request. Returns None for notifications (no `id`).
pub async fn handle(req: &Value) -> Option<Value> {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = req.get("id").cloned();
    match method {
        "initialize" => Some(ok(id?, initialize_result())),
        "tools/list" => Some(ok(id?, tool_list())),
        "tools/call" => {
            let id = id?;
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match call_tool(name, &args).await {
                Ok(content) => Some(ok(id, content)),
                Err(e) => Some(rpc_err(id, -32000, &e)),
            }
        }
        "notifications/initialized" => None,
        _ => id.map(|id| rpc_err(id, -32601, "method not found")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_has_read_then_shell() {
        let tl = tool_list();
        let names: Vec<&str> = tl["tools"].as_array().unwrap().iter()
            .map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["read", "shell"]);
        assert_eq!(tl["tools"][0]["inputSchema"]["type"], "object");
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let resp = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})).await.unwrap();
        assert_eq!(resp["result"]["serverInfo"]["name"], "mcp-demo");
        assert_eq!(resp["id"], 1);
    }

    #[tokio::test]
    async fn initialized_notification_yields_no_response() {
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await.is_none());
    }

    #[tokio::test]
    async fn unknown_method_is_minus_32601() {
        let resp = handle(&json!({"jsonrpc":"2.0","id":9,"method":"bogus"})).await.unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn shell_executes() {
        let out = call_tool("shell", &json!({"cmd":"echo hi"})).await.unwrap();
        assert_eq!(out["content"][0]["text"].as_str().unwrap().trim(), "hi");
    }

    #[tokio::test]
    async fn read_returns_file_contents() {
        let p = std::env::temp_dir().join("mcp-demo-read-test.txt");
        tokio::fs::write(&p, "hello-file").await.unwrap();
        let out = call_tool("read", &json!({"path": p.to_str().unwrap()})).await.unwrap();
        assert_eq!(out["content"][0]["text"], "hello-file");
    }
}
```

- [ ] **Step 2: Create the sample file**

`demo/notes.txt`:
```
tunnel.solutions — shared notes
================================
This file is reachable through the `read` tool while the tunnel is open.
The `shell` tool exists on the server but is OUT OF SCOPE for this link.
```

- [ ] **Step 3: Wire the module so tests compile**

Replace `crates/mcp-demo/src/main.rs` temporarily so the crate has the module:
```rust
mod dispatch;

fn main() {
    println!("mcp-demo placeholder");
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mcp-demo`
Expected: PASS (6 tests). (They were written test-first; the impl is in the same file, so confirm green.)

- [ ] **Step 5: Commit**

```bash
git add crates/mcp-demo/ demo/notes.txt
git commit -m "feat(mcp-demo): dispatch logic for read + shell"
```

### Task 4.2: stdio JSON-RPC loop

**Files:**
- Modify: `crates/mcp-demo/src/main.rs`

- [ ] **Step 1: Write the stdio loop**

`crates/mcp-demo/src/main.rs`:
```rust
mod dispatch;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // ignore non-JSON lines
        };
        if let Some(resp) = dispatch::handle(&req).await {
            let mut out = serde_json::to_string(&resp)?;
            out.push('\n');
            stdout.write_all(out.as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Manual smoke test**

Run (pipes two requests in, expects two response lines):
```bash
printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  | cargo run -q -p mcp-demo
```
Expected: two JSON lines; the second contains `"read"` and `"shell"`.

- [ ] **Step 3: Commit**

```bash
git add crates/mcp-demo/
git commit -m "feat(mcp-demo): newline-delimited stdio JSON-RPC loop"
```

**Chunk 4 done:** a real MCP server the agent can spawn and drive.

---

## Chunk 5: agent ↔ MCP bridge (`McpChild`)

Spawn the server, run the MCP handshake, and correlate responses by JSON-RPC `id` (a
background reader → `oneshot` per request, because notifications interleave).

### Task 5.1: `McpChild` with id-correlated requests

**Files:**
- Create: `crates/agent/src/lib.rs`
- Create: `crates/agent/src/mcp.rs`
- Create: `crates/agent/tests/mcp_bridge.rs`
- Modify: `crates/agent/Cargo.toml` (add `relay` dev-dep for later chunks; add `axum` dev-dep for the e2e test in Chunk 6)

- [ ] **Step 1: Add the lib target and the `mcp` module**

Create `crates/agent/src/lib.rs`:
```rust
pub mod mcp;
```

Create `crates/agent/src/mcp.rs`:
```rust
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

/// A spawned MCP server we speak JSON-RPC to over its stdio.
pub struct McpChild {
    child: Child,
    stdin: ChildStdin,
    pending: Pending,
    next_id: AtomicU64,
    pub tools: Vec<protocol::Tool>,
}

impl McpChild {
    /// Spawn, run the MCP handshake (`initialize` → `initialized` → `tools/list`),
    /// and cache the tool list. Fails loudly if the child can't be driven.
    pub async fn spawn(cmd: &str, args: &[String]) -> Result<Self> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning MCP server '{cmd}'"))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("child has no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("child has no stdout"))?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        // Background reader: dispatch responses to waiters by id.
        {
            let pending = pending.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                        if let Some(tx) = pending.lock().await.remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                    // No id => notification; ignore.
                }
            });
        }

        let mut me = McpChild { child, stdin, pending, next_id: AtomicU64::new(1), tools: vec![] };

        me.request("initialize", json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "tunnel", "version": "0.1.0" }
        })).await.context("MCP initialize")?;
        me.notify("notifications/initialized", json!({})).await?;

        let list = me.request("tools/list", json!({})).await.context("MCP tools/list")?;
        me.tools = serde_json::from_value(list.get("tools").cloned().unwrap_or(json!([])))
            .context("parsing tools/list")?;
        Ok(me)
    }

    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a request and await its `result` (surfacing any JSON-RPC `error`).
    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.alloc_id();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let line = format!("{}\n", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;

        let resp = tokio::time::timeout(Duration::from_secs(10), rx).await
            .map_err(|_| anyhow!("MCP request '{method}' timed out"))?
            .map_err(|_| anyhow!("MCP reader dropped before responding"))?;
        if let Some(e) = resp.get("error") {
            return Err(anyhow!("MCP error: {e}"));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let line = format!("{}\n", json!({ "jsonrpc": "2.0", "method": method, "params": params }));
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Invoke a tool; returns the MCP tool `result` object (`{ content: [...] }`).
    pub async fn call_tool(&mut self, name: &str, args: Value) -> Result<Value> {
        self.request("tools/call", json!({ "name": name, "arguments": args })).await
    }

    pub async fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}
```

- [ ] **Step 2: Add dev-dependencies**

In `crates/agent/Cargo.toml`, add:
```toml
[dev-dependencies]
relay = { path = "../relay" }
axum = { version = "0.7", features = ["ws"] }
```

- [ ] **Step 3: Write the bridge integration test**

Create `crates/agent/tests/mcp_bridge.rs`:
```rust
use serde_json::json;

fn mcp_demo_bin() -> String {
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "mcp-demo"])
        .status()
        .expect("cargo build -p mcp-demo");
    assert!(status.success(), "building mcp-demo failed");
    format!("{}/../../target/debug/mcp-demo", env!("CARGO_MANIFEST_DIR"))
}

#[tokio::test]
async fn handshake_lists_tools_and_calls_shell() {
    let mut child = agent::mcp::McpChild::spawn(&mcp_demo_bin(), &[]).await.unwrap();

    let names: Vec<String> = child.tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, vec!["read".to_string(), "shell".to_string()]);

    let out = child.call_tool("shell", json!({"cmd":"echo bridged"})).await.unwrap();
    assert_eq!(out["content"][0]["text"].as_str().unwrap().trim(), "bridged");

    child.kill().await;
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p agent --test mcp_bridge`
Expected: PASS (1 test). The agent drives the real `mcp-demo` over stdio.

- [ ] **Step 5: Commit**

```bash
git add crates/agent/
git commit -m "feat(agent): MCP child bridge with id-correlated requests"
```

**Chunk 5 done:** the agent speaks real MCP to a spawned server.

---

## Chunk 6: agent session — enforcement, TTL, lifecycle, CLI

Replace the Phase 1 echo with the real `open`/`close` commands and the enforcement loop.

### Task 6.1: CLI + pidfile modules

**Files:**
- Create: `crates/agent/src/cli.rs`
- Create: `crates/agent/src/pidfile.rs`
- Modify: `crates/agent/src/lib.rs`

- [ ] **Step 1: Write `cli.rs` with a parse test**

Create `crates/agent/src/cli.rs`:
```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tunnel", about = "Ephemeral, capability-scoped tunnel for a local MCP server")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Open a tunnel to a local MCP server (runs in the foreground).
    Open {
        /// Path to the MCP server binary.
        server: String,
        /// Args forwarded to the MCP server (put them after `--`).
        server_args: Vec<String>,
        #[arg(long, default_value = "15m")]
        ttl: String,
        /// Comma-separated tool allowlist, e.g. `read,search`.
        #[arg(long, default_value = "")]
        scope: String,
        #[arg(long, default_value = "ws://127.0.0.1:8787")]
        relay: String,
    },
    /// Revoke a running tunnel (SIGTERM). Defaults to the most recent.
    Close {
        #[arg(long)]
        tunnel_id: Option<String>,
    },
}

/// Split a `--scope a,b , c` string into trimmed, non-empty names.
pub fn parse_scope(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()).map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_splits_and_trims() {
        assert_eq!(parse_scope("read, search"), vec!["read", "search"]);
        assert!(parse_scope("").is_empty());
        assert!(parse_scope("  ").is_empty());
    }

    #[test]
    fn open_parses_flags_not_as_server_args() {
        let cli = Cli::try_parse_from(["tunnel", "open", "./srv", "--ttl", "2m", "--scope", "read"]).unwrap();
        match cli.command {
            Command::Open { server, server_args, ttl, scope, .. } => {
                assert_eq!(server, "./srv");
                assert!(server_args.is_empty());
                assert_eq!(ttl, "2m");
                assert_eq!(scope, "read");
            }
            _ => panic!("expected open"),
        }
    }
}
```

- [ ] **Step 2: Write `pidfile.rs`**

Create `crates/agent/src/pidfile.rs`:
```rust
use anyhow::{anyhow, Result};
use std::path::PathBuf;

fn pidfile_path(tunnel_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("tunnel-{tunnel_id}.pid"))
}
fn latest_path() -> PathBuf {
    std::env::temp_dir().join("tunnel-latest.pid")
}

/// Body format (3 lines): pid, tunnel_id, link.
pub fn write(tunnel_id: &str, link: &str) -> Result<()> {
    let body = format!("{}\n{tunnel_id}\n{link}\n", std::process::id());
    std::fs::write(pidfile_path(tunnel_id), &body)?;
    std::fs::write(latest_path(), &body)?;
    Ok(())
}

pub fn remove(tunnel_id: &str) {
    let _ = std::fs::remove_file(pidfile_path(tunnel_id));
    if let Ok(body) = std::fs::read_to_string(latest_path()) {
        if body.lines().nth(1) == Some(tunnel_id) {
            let _ = std::fs::remove_file(latest_path());
        }
    }
}

/// Returns (pid, tunnel_id) from a specific or the latest pidfile.
pub fn read(tunnel_id: Option<&str>) -> Result<(i32, String)> {
    let path = tunnel_id.map(pidfile_path).unwrap_or_else(latest_path);
    let body = std::fs::read_to_string(&path)
        .map_err(|_| anyhow!("no tunnel pidfile at {} (is a tunnel open?)", path.display()))?;
    let mut lines = body.lines();
    let pid: i32 = lines.next().ok_or_else(|| anyhow!("empty pidfile"))?.parse()?;
    let id = lines.next().unwrap_or("").to_string();
    Ok((pid, id))
}
```

- [ ] **Step 3: Wire modules**

Update `crates/agent/src/lib.rs` (the `session` module is wired in Task 6.2 when its file exists, so every step here stays green):
```rust
pub mod cli;
pub mod mcp;
pub mod pidfile;
```

- [ ] **Step 4: Run the cli tests**

Run: `cargo test -p agent --lib`
Expected: PASS — `cli` (2 tests) passes; `mcp` and `pidfile` compile.

### Task 6.2: The enforcement session (`open`) + `close`

**Files:**
- Create: `crates/agent/src/session.rs`
- Modify: `crates/agent/src/lib.rs` (add `pub mod session;`)
- Rewrite: `crates/agent/src/main.rs`

- [ ] **Step 1: Write `session.rs`**

Create `crates/agent/src/session.rs`:
```rust
use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    decide_call, filter_tools, mint, verify, AgentFrame, CallDecision, Claims, ErrorCode,
    TokenError, Tool, ViewerFrame,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::mcp::McpChild;
use crate::pidfile;

pub struct OpenArgs {
    pub server: String,
    pub server_args: Vec<String>,
    pub ttl: Duration,
    pub scope: Vec<String>,
    pub relay: String,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn random_id() -> String {
    let mut b = [0u8; 4];
    getrandom::getrandom(&mut b).expect("rng");
    hex::encode(b)
}

fn relay_host(relay: &str) -> String {
    relay.trim_start_matches("ws://").trim_start_matches("wss://").trim_end_matches('/').to_string()
}

fn err_frame(id: Option<u64>, code: ErrorCode, tool: Option<String>, msg: &str) -> AgentFrame {
    AgentFrame::Error { id, code, tool, message: Some(msg.to_string()) }
}

pub async fn open(args: OpenArgs) -> Result<()> {
    // 1. Spawn + handshake the child BEFORE dialing the relay (fail loud, leave nothing open).
    let mut child = McpChild::spawn(&args.server, &args.server_args).await?;
    let child_tools = child.tools.clone();

    // 2. Identity, secret, token.
    let tunnel_id = random_id();
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).map_err(|e| anyhow!("rng: {e}"))?;
    let exp = now_secs() + args.ttl.as_secs();
    let token = mint(&secret, &Claims { tunnel_id: tunnel_id.clone(), scope: args.scope.clone(), exp });

    // 3. Dial the relay (agent role).
    let host = relay_host(&args.relay);
    let agent_url = format!("{}/agent/{tunnel_id}", args.relay.trim_end_matches('/'));
    let (ws, _) = tokio_tungstenite::connect_async(&agent_url).await
        .map_err(|e| anyhow!("connecting to relay {agent_url}: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    // 4. Pidfile + banner.
    let link = format!("http://{host}/t/{tunnel_id}#{token}");
    pidfile::write(&tunnel_id, &link)?;
    print_banner(&args, &child_tools, &host, &tunnel_id, &token);

    // 5. Outbound frames -> relay sink.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<AgentFrame>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sink.send(Message::Text(serde_json::to_string(&frame).unwrap())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let shutdown = Arc::new(Notify::new());

    // 6. TTL timer: emit Expired, then trigger shutdown.
    {
        let out_tx = out_tx.clone();
        let shutdown = shutdown.clone();
        let secs = exp.saturating_sub(now_secs());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            let _ = out_tx.send(err_frame(None, ErrorCode::Expired, None, "ttl expired"));
            tokio::time::sleep(Duration::from_millis(150)).await; // let it flush
            eprintln!("\n  ttl expired — tunnel revoked. link is dead.");
            shutdown.notify_one();
        });
    }

    // 7. SIGINT/SIGTERM -> shutdown.
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).unwrap();
            let mut intr = signal(SignalKind::interrupt()).unwrap();
            tokio::select! {
                _ = term.recv() => {}
                _ = intr.recv() => {}
            }
            eprintln!("\n  revoked. link is dead.");
            shutdown.notify_one();
        });
    }

    // 8. Event loop. Verified claims cached after a good Hello; exp re-checked per call.
    let mut verified: Option<Claims> = None;
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            msg = stream.next() => {
                let txt = match msg {
                    Some(Ok(Message::Text(txt))) => txt,
                    Some(Ok(_)) => continue,       // ignore binary/ping/pong/close-as-message
                    None | Some(Err(_)) => break,  // relay/viewer gone → tear down
                };
                let frame: ViewerFrame = match serde_json::from_str(&txt) {
                    Ok(f) => f,
                    Err(_) => { let _ = out_tx.send(err_frame(None, ErrorCode::BadRequest, None, "unparseable frame")); continue; }
                };
                handle_frame(frame, &secret, exp, &child_tools, &mut verified, &mut child, &out_tx).await;
            }
        }
    }

    // Teardown (one path for TTL, signal, and relay-close).
    child.kill().await;
    pidfile::remove(&tunnel_id);
    writer.abort();
    Ok(())
}

async fn handle_frame(
    frame: ViewerFrame,
    secret: &[u8],
    exp: u64,
    child_tools: &[Tool],
    verified: &mut Option<Claims>,
    child: &mut McpChild,
    out_tx: &mpsc::UnboundedSender<AgentFrame>,
) {
    match frame {
        ViewerFrame::Hello { token } => match verify(secret, &token, now_secs()) {
            Ok(c) => {
                let expires_in_ms = exp.saturating_sub(now_secs()) * 1000;
                let _ = out_tx.send(AgentFrame::Ready { scope: c.scope.clone(), expires_in_ms });
                let _ = out_tx.send(AgentFrame::Tools { tools: filter_tools(child_tools, &c.scope) });
                *verified = Some(c);
            }
            Err(TokenError::Expired) => { let _ = out_tx.send(err_frame(None, ErrorCode::Expired, None, "token expired")); }
            Err(_) => { let _ = out_tx.send(err_frame(None, ErrorCode::Unauthorized, None, "invalid token")); }
        },
        ViewerFrame::List => match verified {
            Some(c) => { let _ = out_tx.send(AgentFrame::Tools { tools: filter_tools(child_tools, &c.scope) }); }
            None => { let _ = out_tx.send(err_frame(None, ErrorCode::Unauthorized, None, "say hello first")); }
        },
        ViewerFrame::Call { id, tool, args } => {
            let Some(c) = verified.clone() else {
                let _ = out_tx.send(err_frame(Some(id), ErrorCode::Unauthorized, None, "say hello first"));
                return;
            };
            match decide_call(&c, now_secs(), &tool) {
                CallDecision::Expired => { let _ = out_tx.send(err_frame(Some(id), ErrorCode::Expired, None, "ttl expired")); }
                CallDecision::OutOfScope => { let _ = out_tx.send(err_frame(Some(id), ErrorCode::OutOfScope, Some(tool), "tool not in scope")); }
                CallDecision::Forward => match child.call_tool(&tool, args).await {
                    Ok(result) => {
                        let content = result.get("content").cloned().unwrap_or(result);
                        let _ = out_tx.send(AgentFrame::Result { id, content });
                    }
                    Err(e) => { let _ = out_tx.send(err_frame(Some(id), ErrorCode::ToolError, Some(tool), &e.to_string())); }
                },
            }
        }
    }
}

pub fn close(tunnel_id: Option<&str>) -> Result<()> {
    let (pid, id) = pidfile::read(tunnel_id)?;
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid), Signal::SIGTERM).map_err(|e| anyhow!("signalling pid {pid}: {e}"))?;
    println!("  revoked tunnel {id} (pid {pid}). link is dead.");
    Ok(())
}

fn print_banner(args: &OpenArgs, child_tools: &[Tool], host: &str, id: &str, token: &str) {
    let in_scope = child_tools.iter().filter(|t| args.scope.iter().any(|s| s == &t.name)).count();
    let total = child_tools.len();
    let scope_str = if args.scope.is_empty() { "(none — deny all)".into() } else { args.scope.join(",") };
    println!();
    println!("  tunnel.solutions");
    println!("  ─────────────────");
    println!("  spawn      {} … ok", args.server);
    println!("  handshake  ✓   MCP initialize — {total} tools advertised");
    println!("  scope      {scope_str}    ({in_scope} of {total} in scope)");
    println!("  ttl        {}", humantime::format_duration(args.ttl));
    println!("  token      ✓   minted");
    println!("  relay      {}  connected", args.relay);
    println!("  link  →    http://{host}/t/{id}#{token}");
    println!();
    println!("  serving — ctrl-c or `tunnel close` to revoke");
    println!();
}
```

- [ ] **Step 2: Add `humantime` to the agent**

In `crates/agent/Cargo.toml` `[dependencies]`, add:
```toml
humantime = "2"
```

- [ ] **Step 3: Rewrite `main.rs` to dispatch**

`crates/agent/src/main.rs`:
```rust
use agent::cli::{parse_scope, Cli, Command};
use agent::session::{self, OpenArgs};
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Open { server, server_args, ttl, scope, relay } => {
            let ttl = protocol::parse_ttl(&ttl).map_err(anyhow::Error::msg)?;
            session::open(OpenArgs { server, server_args, ttl, scope: parse_scope(&scope), relay }).await
        }
        Command::Close { tunnel_id } => session::close(tunnel_id.as_deref()),
    }
}
```

Then wire the session module — add `pub mod session;` to `crates/agent/src/lib.rs` (the file now exists). `protocol` is already an agent dependency, and `main.rs` consumes the `agent` lib by its crate name.

- [ ] **Step 4: Build and run unit tests**

Run: `cargo build -p agent && cargo test -p agent --lib`
Expected: builds; cli tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agent/
git commit -m "feat(agent): enforcement session (open/close), TTL timer, signals, banner"
```

### Task 6.3: End-to-end scope-enforcement test (automates the demo)

**Files:**
- Create: `crates/agent/tests/end_to_end.rs`

- [ ] **Step 1: Write the end-to-end test**

Create `crates/agent/tests/end_to_end.rs`:
```rust
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as TMsg;

fn build(pkg: &str) {
    let st = std::process::Command::new(env!("CARGO")).args(["build", "-p", pkg]).status().unwrap();
    assert!(st.success(), "building {pkg} failed");
}

async fn next_json<S>(s: &mut S) -> Value
where
    S: StreamExt<Item = Result<TMsg, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), s.next()).await.unwrap().unwrap().unwrap();
        if let TMsg::Text(t) = m {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

async fn read_link() -> (String, String) {
    let path = std::env::temp_dir().join("tunnel-latest.pid");
    for _ in 0..50 {
        if let Ok(body) = std::fs::read_to_string(&path) {
            let mut l = body.lines();
            let _pid = l.next();
            let id = l.next().unwrap_or("").to_string();
            let link = l.next().unwrap_or("").to_string();
            if let Some((_, token)) = link.split_once('#') {
                if !id.is_empty() && !token.is_empty() {
                    return (id, token.to_string());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("agent never published a pidfile/link");
}

#[tokio::test]
async fn read_succeeds_shell_refused() {
    build("mcp-demo");

    // relay on an ephemeral port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, relay::build_app()).await.unwrap(); });
    let relay_url = format!("ws://127.0.0.1:{port}");

    // tunnel open --scope read
    let mcp = format!("{}/../../target/debug/mcp-demo", env!("CARGO_MANIFEST_DIR"));
    let open = agent::session::OpenArgs {
        server: mcp,
        server_args: vec![],
        ttl: Duration::from_secs(120),
        scope: vec!["read".into()],
        relay: relay_url.clone(),
    };
    tokio::spawn(async move { agent::session::open(open).await.unwrap(); });

    let (id, token) = read_link().await;
    let (mut v, _) = tokio_tungstenite::connect_async(format!("{relay_url}/viewer/{id}")).await.unwrap();

    // hello -> ready + filtered tools
    v.send(TMsg::Text(json!({"type":"hello","token":token}).to_string())).await.unwrap();
    assert_eq!(next_json(&mut v).await["type"], "ready");
    let tools = next_json(&mut v).await;
    let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["read"], "shell must be filtered from the list");

    // in-scope read succeeds
    let f = std::env::temp_dir().join("tunnel-e2e-read.txt");
    std::fs::write(&f, "secret-handshake").unwrap();
    v.send(TMsg::Text(json!({"type":"call","id":1,"tool":"read","args":{"path":f.to_str().unwrap()}}).to_string())).await.unwrap();
    let res = next_json(&mut v).await;
    assert_eq!(res["type"], "result");
    assert_eq!(res["content"][0]["text"], "secret-handshake");

    // out-of-scope shell refused agent-side
    v.send(TMsg::Text(json!({"type":"call","id":2,"tool":"shell","args":{"cmd":"echo pwned"}}).to_string())).await.unwrap();
    let refused = next_json(&mut v).await;
    assert_eq!(refused["type"], "error");
    assert_eq!(refused["code"], "out_of_scope");
    assert_eq!(refused["tool"], "shell");
}
```

- [ ] **Step 2: Run the end-to-end test**

Run: `cargo test -p agent --test end_to_end -- --nocapture`
Expected: PASS. This automates acceptance criterion 3 (filtered list + agent-side refusal) across real sockets and a real MCP child.

- [ ] **Step 3: Commit**

```bash
git add crates/agent/
git commit -m "test(agent): end-to-end scope enforcement (read ok, shell refused)"
```

**Chunk 6 done:** the wedge works across the full stack, proven automatically.

---

## Chunk 7: Full viewer (filtered tools, raw-call box, errors, countdown)

Replace the Phase 1 echo page with the real UI. The **raw-call box** is mandatory: it's how
the demo attempts `shell` and shows the agent refusing it.

**Security note for the executor:** the viewer renders tool output (file contents, command
output) that is attacker-influenceable. Build DOM nodes and set `.textContent`; do **not**
assign `innerHTML` from any value derived from a frame. The code below does this throughout.

### Task 7.1: The viewer page

**Files:**
- Rewrite: `viewer/index.html`

- [ ] **Step 1: Write the full viewer**

`viewer/index.html`:
```html
<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>tunnel.solutions</title>
<style>
  :root { --bg:#0b0e14; --fg:#cbd5e1; --dim:#64748b; --accent:#7dd3fc; --ok:#34d399; --bad:#f87171; --warn:#fbbf24; --panel:#111827; --line:#1f2937; }
  * { box-sizing:border-box; }
  body { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; background:var(--bg); color:var(--fg); margin:0; padding:1.5rem; line-height:1.5; }
  h1 { color:var(--accent); font-size:1.2rem; margin:0 0 .25rem; }
  .sub { color:var(--dim); margin-bottom:1rem; }
  .bar { display:flex; gap:1rem; align-items:center; border:1px solid var(--line); background:var(--panel); padding:.5rem .8rem; border-radius:6px; margin-bottom:1rem; }
  .dot { width:.6rem; height:.6rem; border-radius:50%; background:var(--dim); }
  .dot.live { background:var(--ok); } .dot.dead { background:var(--bad); }
  .ttl { margin-left:auto; color:var(--warn); }
  .tool { border:1px solid var(--line); background:var(--panel); border-radius:6px; padding:.8rem; margin-bottom:.6rem; }
  .tool h3 { margin:0 0 .2rem; color:var(--accent); font-size:1rem; }
  .tool .desc { color:var(--dim); font-size:.85rem; margin-bottom:.5rem; }
  input, textarea { background:#0b1220; color:var(--fg); border:1px solid var(--line); border-radius:4px; padding:.4rem; font:inherit; width:100%; margin-bottom:.4rem; }
  button { background:var(--accent); color:#001018; border:0; border-radius:4px; padding:.45rem .9rem; font:inherit; cursor:pointer; }
  button:disabled { opacity:.4; cursor:not-allowed; }
  .raw { border:1px dashed var(--line); border-radius:6px; padding:.8rem; margin-top:1rem; }
  .raw h3 { margin:0 0 .2rem; color:var(--warn); font-size:.95rem; }
  #out .ev { border-left:2px solid var(--line); padding:.3rem .6rem; margin:.4rem 0; white-space:pre-wrap; word-break:break-word; }
  #out .ev.ok { border-color:var(--ok); } #out .ev.err { border-color:var(--bad); } #out .ev.warn { border-color:var(--warn); }
  .label { color:var(--dim); }
</style>
<body>
  <h1>tunnel.solutions</h1>
  <div class="sub">capability-scoped MCP tunnel — read-only viewer</div>

  <div class="bar">
    <span class="dot" id="dot"></span>
    <span id="status">connecting…</span>
    <span class="ttl" id="ttl"></span>
  </div>

  <div id="tools"></div>

  <div class="raw">
    <h3>raw call (proof of enforcement)</h3>
    <div class="desc label">attempt any tool by name — out-of-scope calls are refused agent-side.</div>
    <input id="rawTool" placeholder="tool name, e.g. shell">
    <textarea id="rawArgs" rows="2" placeholder='{"cmd":"echo hi"}'>{"cmd":"echo hi"}</textarea>
    <button id="rawSend">send →</button>
  </div>

  <h3 style="margin-top:1.2rem">output</h3>
  <div id="out"></div>

<script>
  const $ = (id) => document.getElementById(id);
  const id = location.pathname.split("/").filter(Boolean).pop() || "demo";
  const token = location.hash.slice(1);

  let nextId = 1;
  const pending = {};        // call id -> tool name (for labeling results)
  let dead = false;
  let remaining = 0, ticker = null;

  // Build a DOM node; text goes through textContent, never innerHTML.
  const el = (tag, o = {}) => {
    const n = document.createElement(tag);
    if (o.class) n.className = o.class;
    if (o.text != null) n.textContent = o.text;
    return n;
  };

  // Append an event row from plain text — no HTML parsing, no XSS surface.
  function out(label, body, cls = "") {
    const row = el("div", { class: "ev " + cls });
    row.appendChild(el("span", { class: "label", text: label }));
    if (body != null && body !== "") {
      row.appendChild(document.createTextNode("\n" + body));
    }
    $("out").prepend(row);
  }

  function setStatus(text, state) {
    $("status").textContent = text;
    $("dot").className = "dot" + (state ? " " + state : "");
  }
  function killUI(reason) {
    dead = true;
    setStatus(reason, "dead");
    document.querySelectorAll("button").forEach((b) => (b.disabled = true));
    if (ticker) clearInterval(ticker);
    $("ttl").textContent = "";
  }
  function startCountdown(ms) {
    remaining = Math.max(0, Math.floor(ms / 1000));
    const render = () => {
      const m = String(Math.floor(remaining / 60)).padStart(2, "0");
      const s = String(remaining % 60).padStart(2, "0");
      $("ttl").textContent = `ttl ${m}:${s}`;
      if (remaining <= 0) clearInterval(ticker);
      remaining--;
    };
    render();
    ticker = setInterval(render, 1000);
  }

  function renderTools(tools) {
    const root = $("tools");
    root.replaceChildren();
    if (!tools.length) {
      root.appendChild(el("div", { class: "label", text: "no tools in scope." }));
      return;
    }
    for (const t of tools) {
      const card = el("div", { class: "tool" });
      card.appendChild(el("h3", { text: t.name }));
      card.appendChild(el("div", { class: "desc", text: t.description || "" }));
      const props = (t.inputSchema && t.inputSchema.properties) || {};
      const inputs = [];
      for (const k of Object.keys(props)) {
        const i = el("input");
        i.placeholder = k;
        i.dataset.k = k;
        card.appendChild(i);
        inputs.push(i);
      }
      const btn = el("button", { text: "call →" });
      btn.onclick = () => {
        const args = {};
        inputs.forEach((i) => { if (i.value) args[i.dataset.k] = i.value; });
        call(t.name, args);
      };
      card.appendChild(btn);
      root.appendChild(card);
    }
  }

  function call(tool, args) {
    if (dead) return;
    const cid = nextId++;
    pending[cid] = tool;
    out("→ call", `${tool} ${JSON.stringify(args)}`);
    ws.send(JSON.stringify({ type: "call", id: cid, tool, args }));
  }

  const ws = new WebSocket(`ws://${location.host}/viewer/${id}`);
  ws.onopen = () => { setStatus("handshaking…", "live"); ws.send(JSON.stringify({ type: "hello", token })); };
  ws.onclose = () => { if (!dead) killUI("× link closed"); };
  ws.onmessage = (e) => {
    const f = JSON.parse(e.data);
    switch (f.type) {
      case "ready":
        setStatus(`live — scope [${f.scope.join(", ") || "none"}]`, "live");
        startCountdown(f.expires_in_ms);
        break;
      case "tools":
        renderTools(f.tools);
        break;
      case "result": {
        const tool = pending[f.id] || "?";
        const text = (f.content || []).map((c) => c.text ?? JSON.stringify(c)).join("\n");
        out("← " + tool, text, "ok");
        break;
      }
      case "error":
        handleError(f);
        break;
    }
  };

  function handleError(f) {
    const tool = f.tool ? ` '${f.tool}'` : "";
    switch (f.code) {
      case "out_of_scope":
        out("⊘ refused", `${tool} is out of scope`, "err");
        break;
      case "expired":
        out("⏱ expired", f.message || "ttl elapsed", "warn");
        killUI("⏱ link expired — tunnel revoked");
        break;
      case "unauthorized":
        out("✗ unauthorized", f.message || "", "err");
        killUI("✗ unauthorized");
        break;
      case "tool_error":
        out("⚠ tool error", `${tool} ${f.message || ""}`, "err");
        break;
      default:
        out("⚠ " + f.code, f.message || "", "err");
    }
  }

  $("rawSend").onclick = () => {
    const tool = $("rawTool").value.trim();
    if (!tool) return;
    let args;
    try { args = JSON.parse($("rawArgs").value || "{}"); }
    catch { out("⚠ local", "args is not valid JSON", "err"); return; }
    call(tool, args);
  };
</script>
</body>
</html>
```

- [ ] **Step 2: Manual verification against the live stack**

1. Terminal 1: `cargo run -p relay`
2. Terminal 2: `cargo run -p agent -- open ./target/debug/mcp-demo --ttl 2m --scope read`
   (run `cargo build -p mcp-demo` first if the binary isn't present)
3. Open the printed `link →` (includes `#<token>`) in a browser.
4. Confirm: status shows `live — scope [read]`, a `read` tool card appears, **no** `shell` card, and a `ttl mm:ss` countdown ticks down.
5. Call `read` with `path` = `demo/notes.txt` → file contents appear in green.
6. In the raw-call box, tool = `shell`, args = `{"cmd":"echo pwned"}`, send → red `⊘ refused 'shell' is out of scope`.

Expected: all of the above. This is the visible demo.

- [ ] **Step 3: Commit**

```bash
git add viewer/index.html
git commit -m "feat(viewer): filtered tools, raw-call box, error states, ttl countdown"
```

**Chunk 7 done:** the show is on screen — filtered tools, live refusal, countdown.

---

## Chunk 8: README, demo launcher, acceptance pass

### Task 8.1: `demo.sh` convenience launcher

**Files:**
- Create: `demo.sh`

- [ ] **Step 1: Write `demo.sh`**

`demo.sh`:
```bash
#!/usr/bin/env bash
# Boot the relay, then open a 2-minute read-only tunnel to the demo MCP server.
set -euo pipefail

TTL="${TTL:-2m}"
SCOPE="${SCOPE:-read}"

cargo build -p relay -p mcp-demo -p agent

cargo run -q -p relay &
RELAY_PID=$!
trap 'kill "$RELAY_PID" 2>/dev/null || true' EXIT
sleep 1

echo "relay up (pid $RELAY_PID). opening tunnel: --ttl $TTL --scope $SCOPE"
cargo run -q -p agent -- open ./target/debug/mcp-demo --ttl "$TTL" --scope "$SCOPE"
```

- [ ] **Step 2: Make it executable + smoke it**

Run: `chmod +x demo.sh`
Then `./demo.sh` should build, boot the relay, and print the tunnel banner with a link. Ctrl-C tears both down. (Quick visual check, then Ctrl-C.)

- [ ] **Step 3: Commit**

```bash
git add demo.sh
git commit -m "chore: demo.sh launcher (relay + read-only tunnel)"
```

### Task 8.2: README

**Files:**
- Rewrite: `README.md`

- [ ] **Step 1: Write the README**

`README.md`:
````markdown
# tunnel.solutions

An **ephemeral, capability-scoped tunnel for a locally-running MCP server.** A teammate opens
a link to your running agent, sees only the tools you allow, and the link evaporates on TTL
or `tunnel close`.

## Why this isn't ngrok

ngrok, Cloudflare Tunnel, and frp expose a *port* — they have no idea what's on the other
end. tunnel.solutions knows the far end is an **MCP server with a tool list**, so it is
**MCP-aware, ephemeral, and capability-scoped**: it filters the advertised `tools/list` to a
`--scope` allowlist, refuses any out-of-scope `tools/call` agent-side before the server ever
sees it, and enforces a TTL on every call so revocation is real rather than cosmetic.

## Architecture

```
viewer ──ws──> relay <──ws── agent ──stdio──> local MCP server
  (custom JSON)  (pairs by    (enforces scope    (real MCP JSON-RPC)
                  tunnel_id)    + TTL, filters)
```

The **relay** is deliberately dumb: it pairs a viewer socket to an agent socket by
`tunnel_id` and shuttles opaque frames, never holding the signing secret and no state past the
session. All enforcement lives at the **agent** — the only component holding the secret and
sitting between the viewer and the real MCP server. A compromised relay cannot widen scope or
extend a TTL; it only moves bytes.

## Demo

```bash
# 1. start the relay
cargo run -p relay

# 2. open a 2-minute, read-only tunnel to the sample MCP server (in another terminal)
cargo build -p mcp-demo
cargo run -p agent -- open ./target/debug/mcp-demo --ttl 2m --scope read
#   → prints a link like http://127.0.0.1:8787/t/<id>#<token>

# 3. open the link as a "teammate" in a browser:
#      - call `read` (path: demo/notes.txt)  → succeeds
#      - raw-call `shell` {"cmd":"echo hi"}   → refused, out of scope
#
# 4. let the 2-minute TTL lapse → the link goes dead live,
#    or run `cargo run -p agent -- close` from a third terminal to revoke on demand.
```

Or just: `./demo.sh` (boots the relay and opens the tunnel; Ctrl-C tears down).

## Token model

A capability token is an HMAC-SHA256-signed JSON payload `{ tunnel_id, scope, exp }`. The
32-byte secret is generated per session and held in memory only. The signature is verified
once at handshake; `exp` is re-checked on **every** `tools/call`, so TTL expiry is enforced,
not advisory. The token rides in the link's URL fragment (`#…`) so it is never sent in an
HTTP request line to the relay.

## Workspace

| Crate | Role |
|-------|------|
| `protocol` | Pure core: token, scope filter, per-call decision, TTL parse, frames. |
| `relay` | Axum WS pairing service (binary: `relay`). |
| `agent` | The CLI (binary: `tunnel`): spawns the MCP child, enforces scope + TTL. |
| `mcp-demo` | Sample MCP server exposing `read` + `shell` (binary: `mcp-demo`). |
| `viewer/index.html` | Static viewer, no build step. |

## Tests

```bash
cargo test --workspace
```

The correctness core (`protocol`) is unit-tested exhaustively; `relay` has an in-process WS
pairing test; `agent` has a real-socket end-to-end test proving `read` succeeds and `shell` is
refused.

## Non-goals & future work

No persistence (in-memory relay state is a feature — nothing survives the session), no
accounts (the capability token is the only credential), single viewer per tunnel, `ws://`
locally (a production relay terminates TLS in front). **Future work:** payload encryption
beyond the transport, so even the relay operator cannot read tool I/O.
````

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: README with demo script and the wedge"
```

### Task 8.3: Final acceptance pass

**Files:** none (verification only).

- [ ] **Step 1: Full test suite**

Run: `cargo test --workspace`
Expected: all green (protocol unit tests, relay pairing, agent bridge + end_to_end, mcp-demo).

- [ ] **Step 2: TTL expiry (live)**

1. `cargo run -p relay` (terminal 1)
2. `cargo run -p agent -- open ./target/debug/mcp-demo --ttl 10s --scope read` (terminal 2)
3. Open the link; watch the countdown. At 0, the page flips to `⏱ link expired — tunnel revoked`, buttons disable, and the agent process exits (terminal 2 prints `ttl expired`).

Expected: link dies within ~1s of zero.

- [ ] **Step 3: `tunnel close` (on demand)**

1. `cargo run -p relay` (terminal 1)
2. `cargo run -p agent -- open ./target/debug/mcp-demo --ttl 5m --scope read` (terminal 2)
3. Open the link (confirm live).
4. `cargo run -p agent -- close` (terminal 3) → prints `revoked tunnel <id>`.
5. Browser flips to `× link closed`; terminal 2 prints `revoked. link is dead.` and exits.

Expected: the link dies immediately on `close`.

- [ ] **Step 4: Acceptance criteria checklist**

Confirm each:
- [ ] `cargo run -p relay` starts the relay.
- [ ] `tunnel open ./target/debug/mcp-demo --ttl Xm --scope a,b` prints a working viewer link.
- [ ] Viewer sees only in-scope tools; an out-of-scope `tools/call` is refused agent-side.
- [ ] TTL expiry and `tunnel close` both kill the session immediately.
- [ ] README documents the demo script and names the wedge in two sentences.

- [ ] **Step 5: Tag the milestone (optional)**

```bash
git tag -a v0.1.0-demo -m "tunnel.solutions demo-ready"
```

**Chunk 8 done:** acceptance criteria met; the single-take demo runs end to end.

---

## Summary of the build

| Chunk | Outcome |
|-------|---------|
| 1 | `protocol` — token, scope, decision, TTL, frames, fully unit-tested. |
| 2 | `relay` — dumb WS pairing + teardown, integration-tested. |
| 3 | Phase 1 echo — dumb tunnel proven through a browser. |
| 4 | `mcp-demo` — real MCP server (`read` + `shell`). |
| 5 | agent ↔ MCP bridge — id-correlated stdio JSON-RPC. |
| 6 | agent session — enforcement, TTL, signals, CLI; end-to-end test. |
| 7 | viewer — filtered tools, raw-call box, errors, countdown. |
| 8 | README, `demo.sh`, acceptance pass. |

**The wedge, in three enforced code paths:** `filter_tools` (list), `decide_call` →
`OutOfScope` (call), `decide_call` → `Expired` (TTL). Everything else is plumbing.
