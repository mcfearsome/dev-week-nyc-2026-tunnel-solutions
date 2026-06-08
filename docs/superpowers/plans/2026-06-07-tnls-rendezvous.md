# tnls Rendezvous (Phase 2.5) Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry the seeder's peer address(es) through the tunnel's `request_file` so `tnls get` connects to the seeder directly via librqbit `initial_peers` instead of waiting on DHT — making real transfers (single-host, same-LAN, and UPnP-public) complete.

**Architecture:** `tnls share` picks a fixed listen port `P`, gathers reachable addresses (loopback + LAN IPs + the public IP from the relay's new `GET /whoami`), and passes them as `--peer` to `tnls mcp-serve`. `request_file` returns `{magnet, peers}`; `tnls get` parses it into a `RetrievedFile` and feeds `peers` to `fetch` as `initial_peers` (the exact Phase-1 mechanism).

**Tech Stack:** Rust, librqbit (existing `NetOpts.listen_port`/`initial_peers`), `reqwest` (already in the tree via librqbit, for the `/whoami` GET), `local-ip-address` (LAN enumeration), Axum (relay `/whoami`).

**Spec:** `docs/superpowers/specs/2026-06-07-tnls-rendezvous-design.md`. **Prereq:** tnls Phases 1–2 built.

**Verified facts:**
- `crates/relay/src/lib.rs` already has `fn client_ip(headers: &HeaderMap) -> String` (reads `fly-client-ip` → `x-forwarded-for`). `/whoami` reuses it.
- `crates/tnls/src/bittorrent.rs`: `NetOpts { disable_dht, listen_port: Option<u16>, enable_upnp, initial_peers: Vec<SocketAddr> }`; `seed` already honors `listen_port`. **No `bittorrent.rs` change needed.**
- `crates/tnls/src/mcp_serve.rs`: `ShareInfo { magnet, name, size }`, `handle(req, &ShareInfo)`, `serve(ShareInfo)`. `request_file` currently returns the bare magnet as text.
- `crates/tnls/src/get.rs`: `retrieve_magnet(link) -> Result<(String, Option<String>)>` (magnet, name); `run_get(link, out_dir)`.
- `crates/tnls/src/share.rs`: `run_share(ShareArgs { path, relay, ttl, mcp_exe })` — seeds with `listen_port: None` today.

---

## File Structure

| Path | Change |
|------|--------|
| `crates/relay/src/lib.rs` | + `GET /whoami` route returning `client_ip(&headers)`. |
| `crates/tnls/src/mcp_serve.rs` | `ShareInfo` gains `peers: Vec<String>`; `request_file` returns JSON `{magnet, peers}`. |
| `crates/tnls/src/cli.rs` | `McpServe` gains repeatable `--peer`. |
| `crates/tnls/src/get.rs` | `RetrievedFile` struct; parse JSON `{magnet, peers}` w/ bare-magnet fallback; `run_get` feeds `initial_peers`. |
| `crates/tnls/src/share.rs` | pick port `P`; gather addrs (loopback + LAN + `/whoami`); seed with `listen_port: Some(P)`; pass `--peer`. |
| `crates/tnls/Cargo.toml` | + `local-ip-address`, `reqwest` (rustls). |
| `crates/tnls/tests/end_to_end.rs` | upgrade to assert the file fully transfers via the loopback rendezvous peer. |

---

## Chunk 1: Rendezvous

### Task 1: relay `GET /whoami`

**Files:** Modify `crates/relay/src/lib.rs`

- [ ] **Step 1: Add the route + handler**

In `build_app`, add: `.route("/whoami", get(whoami))`. Add the handler (reusing the existing `client_ip`):
```rust
async fn whoami(headers: axum::http::HeaderMap) -> String {
    client_ip(&headers) // "" when no proxy header (local); the caller's public IP behind Fly
}
```

- [ ] **Step 2: Build + smoke**

Run: `cargo build -p relay`. Then verify the route exists:
```bash
(cargo run -q -p relay &) ; sleep 1
curl -s -H 'fly-client-ip: 203.0.113.7' http://127.0.0.1:8787/whoami   # → 203.0.113.7
curl -s http://127.0.0.1:8787/whoami ; echo "(empty locally is expected)"
pkill -f 'target/debug/relay'
```
Expected: the header value is echoed; empty without the header.

- [ ] **Step 3: Commit**
```bash
git add crates/relay/
git commit -m "feat(relay): GET /whoami (returns the caller's public IP for rendezvous)"
```

> The deployed relay must be **redeployed** for `/whoami` to be live (done at the end, with the rest).

### Task 2: `mcp-serve` advertises peers

**Files:** Modify `crates/tnls/src/mcp_serve.rs`, `crates/tnls/src/cli.rs`, `crates/tnls/src/main.rs`

- [ ] **Step 1: Update the failing test**

In `mcp_serve.rs`, change `ShareInfo` to include peers and update `request_file`. First the test — replace `request_file_returns_the_magnet`:
```rust
fn share() -> ShareInfo {
    ShareInfo { magnet: "magnet:?xt=urn:btih:abc".into(), name: "f.bin".into(), size: 9,
                peers: vec!["127.0.0.1:6881".into()] }
}
#[test]
fn request_file_returns_magnet_and_peers() {
    let resp = handle(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"request_file"}}), &share()).unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(v["magnet"], "magnet:?xt=urn:btih:abc");
    assert_eq!(v["peers"][0], "127.0.0.1:6881");
}
```

- [ ] **Step 2: Run → FAIL** (`ShareInfo` has no `peers`). `cargo test -p tnls mcp_serve`.

- [ ] **Step 3: Implement**

In `mcp_serve.rs`: add `pub peers: Vec<String>` to `ShareInfo`. Change the `request_file` arm of `handle` to:
```rust
"request_file" => {
    let body = serde_json::json!({ "magnet": share.magnet, "peers": share.peers });
    text(&body.to_string())
}
```
(`list_shares`/`status` arms unchanged.)

In `cli.rs`, the `McpServe` variant gains: `#[arg(long)] peer: Vec<String>,`. In `main.rs`, the `McpServe` handler builds `ShareInfo { magnet, name, size, peers }`.

- [ ] **Step 4: Run → PASS** (3 mcp_serve tests). Commit: `feat(tnls): mcp-serve advertises peer addresses`.

### Task 3: `get` parses peers → `initial_peers`

**Files:** Modify `crates/tnls/src/get.rs`

- [ ] **Step 1: Write the failing tests**

Add to `get.rs`'s test module:
```rust
#[test]
fn parses_json_magnet_with_peers() {
    let (m, peers) = parse_request_file_content(r#"{"magnet":"magnet:?xt=urn:btih:zz","peers":["127.0.0.1:6881","bad"]}"#);
    assert_eq!(m, "magnet:?xt=urn:btih:zz");
    assert_eq!(peers, vec!["127.0.0.1:6881".parse::<std::net::SocketAddr>().unwrap()]); // "bad" skipped
}
#[test]
fn falls_back_to_bare_magnet() {
    let (m, peers) = parse_request_file_content("magnet:?xt=urn:btih:zz&dn=x");
    assert_eq!(m, "magnet:?xt=urn:btih:zz&dn=x");
    assert!(peers.is_empty());
}
```

- [ ] **Step 2: Run → FAIL** (`parse_request_file_content` undefined).

- [ ] **Step 3: Implement**

Add to `get.rs`:
```rust
use std::net::SocketAddr;

#[derive(Debug)]
pub struct RetrievedFile { pub magnet: String, pub peers: Vec<SocketAddr>, pub name: Option<String> }

/// Parse the `request_file` content text: JSON `{magnet, peers}` or a bare `magnet:` string.
pub fn parse_request_file_content(text: &str) -> (String, Vec<SocketAddr>) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(magnet) = v.get("magnet").and_then(|m| m.as_str()) {
            let peers = v.get("peers").and_then(|p| p.as_array()).map(|a| {
                a.iter().filter_map(|x| x.as_str()).filter_map(|s| s.parse().ok()).collect()
            }).unwrap_or_default();
            return (magnet.to_string(), peers);
        }
    }
    (text.to_string(), Vec::new()) // bare magnet
}

fn name_from_magnet(magnet: &str) -> Option<String> {
    magnet.split("&dn=").nth(1).and_then(|s| s.split('&').next())
        .map(|s| urlencoding::decode(s).map(|c| c.into_owned()).unwrap_or_else(|_| s.to_string()))
}
```

Change `retrieve_magnet` to return `Result<RetrievedFile>`: at the point it currently extracts `content[0]["text"]`, call `let (magnet, peers) = parse_request_file_content(text); let name = name_from_magnet(&magnet); return Ok(RetrievedFile { magnet, peers, name });`.

Update `run_get`: `let rf = retrieve_magnet(link).await?;` then `fetch(&rf.magnet, out_dir, NetOpts { disable_dht: false, listen_port: None, enable_upnp: true, initial_peers: rf.peers })`.

- [ ] **Step 4: Run → PASS** (get tests: parse + the existing link-parse tests). Commit: `feat(tnls): get uses advertised peers as initial_peers`.

### Task 4: `share` gathers + advertises addresses

**Files:** Modify `crates/tnls/src/share.rs`, `crates/tnls/Cargo.toml`

- [ ] **Step 1: Add deps**

`crates/tnls/Cargo.toml` `[dependencies]`: `local-ip-address = "0.6"` and `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }` (reqwest is already compiled in the tree via librqbit).

- [ ] **Step 2: Implement address gathering + fixed port**

In `share.rs`, add a helper and use it in `run_share` (before seeding):
```rust
fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("0.0.0.0:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(0)
}

/// loopback + LAN IPv4 + (best-effort) public IP from the relay's /whoami, all at :port.
async fn gather_peers(port: u16, relay: &str) -> Vec<String> {
    let mut addrs = vec![format!("127.0.0.1:{port}")];
    if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
        for (_name, ip) in ifaces {
            if let std::net::IpAddr::V4(v4) = ip {
                if !v4.is_loopback() && !v4.is_link_local() {
                    addrs.push(format!("{v4}:{port}"));
                }
            }
        }
    }
    // public IP via the relay's /whoami (https for wss relay, http for ws)
    let scheme = if relay.starts_with("wss://") { "https" } else { "http" };
    let host = relay.trim_start_matches("ws://").trim_start_matches("wss://").trim_end_matches('/');
    if let Ok(resp) = reqwest::get(format!("{scheme}://{host}/whoami")).await {
        if let Ok(ip) = resp.text().await {
            let ip = ip.trim();
            if ip.parse::<std::net::Ipv4Addr>().is_ok() {
                addrs.push(format!("{ip}:{port}"));
            }
        }
    }
    addrs
}
```
In `run_share`, after `create_share`:
```rust
let port = pick_free_port();
let peers = gather_peers(port, &args.relay).await;
```
Change the `seed(...)` `NetOpts` to `listen_port: Some(port)`. Add the peers to the `server_args` for `mcp-serve` — one `--peer <addr>` pair per address:
```rust
let mut server_args = vec![
    "mcp-serve".into(),
    "--magnet".into(), meta.magnet.clone(),
    "--name".into(), meta.name.clone(),
    "--size".into(), meta.size.to_string(),
];
for a in &peers { server_args.push("--peer".into()); server_args.push(a.clone()); }
```
and pass `server_args` to `OpenArgs`.

- [ ] **Step 3: Build** `cargo build -p tnls`. Commit: `feat(tnls): share advertises loopback/LAN/public peers via the tunnel`.

### Task 5: e2e test — full transfer via rendezvous

**Files:** Modify `crates/tnls/tests/end_to_end.rs`

- [ ] **Step 1: Upgrade the test**

Rename `get_retrieves_the_magnet_through_the_tunnel` → `get_transfers_the_file_through_the_tunnel`. After obtaining the link, replace the magnet-only assertion with a full fetch using the advertised peers (the seeder now advertises `127.0.0.1:<P>`, so the download connects directly over loopback):
```rust
    let link = read_link(port).await;
    let rf = tokio::time::timeout(Duration::from_secs(20), tnls::get::retrieve_magnet(&link))
        .await.expect("retrieve timed out").expect("retrieve failed");
    assert!(rf.magnet.starts_with("magnet:?xt=urn:btih:"), "got {}", rf.magnet);
    assert!(rf.peers.iter().any(|p| p.ip().is_loopback()), "must advertise a loopback peer: {:?}", rf.peers);

    // download via the advertised loopback peer → completes directly (no DHT)
    let out = dir.join("dl");
    std::fs::create_dir_all(&out).unwrap();
    let dl = tnls::bittorrent::fetch(&rf.magnet, &out, tnls::bittorrent::NetOpts {
        disable_dht: true, listen_port: None, enable_upnp: false, initial_peers: rf.peers,
    }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(40), dl.wait())
        .await.expect("download timed out").expect("download errored");
    let got = std::fs::read(out.join("doc.bin")).unwrap();
    assert_eq!(got, b"phase two end to end", "transferred bytes must match the source");
```

> The seeder (`run_share`) still seeds with DHT on, but the getter connects via the advertised `127.0.0.1:P` `initial_peers`, so the transfer completes over loopback without any network — hermetic. If `run_share`'s DHT init ever makes this flaky, give `ShareArgs` a test-only `seed_net: Option<NetOpts>` override (default = real) and pass DHT-off in the test.

- [ ] **Step 2: Run → PASS** `cargo test -p tnls --test end_to_end -- --nocapture`. The file transfers end-to-end through the scoped tunnel + rendezvous.

- [ ] **Step 3: Full suite + commit** `cargo test -p tnls`. Commit: `test(tnls): e2e transfers the file via tunnel rendezvous`.

### Task 6: Redeploy the relay (for live `/whoami`)

- [ ] **Step 1:** `fly deploy --remote-only` (the relay now serves `/whoami`).
- [ ] **Step 2:** Verify: `curl -s https://tunnel.locker/whoami` → your public IP. (No commit; deploy only.)

**Chunk 1 done / gate:** `cargo test -p tnls` green incl. the upgraded e2e (real loopback transfer); `tnls share`→`tnls get` completes a transfer on one machine; `/whoami` live. Cross-internet completes when UPnP cooperates (best-effort, per spec).
