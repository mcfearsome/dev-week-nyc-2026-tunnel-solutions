# tnls — Capability-Scoped File Sending (design spec)

**Date:** 2026-06-07
**Status:** Approved (pre-implementation)
**Scope:** A new `tnls` CLI + native GUI that sends large files 1→1 over BitTorrent, gated by a
capability-scoped tunnel (reuses tunnel.locker). v1 = ephemeral 1→1 send.

## 1. Premise & wedge

Sending a large file to a teammate normally means either eating the upload bandwidth yourself
(WeTransfer/S3/a relay) or handing out an unscoped, un-revocable link. `tnls` splits the two
planes:

- **Control plane** — a capability-scoped MCP tunnel (the existing tunnel.locker). It grants
  *which* files a teammate may fetch and *for how long* (TTL), and is revocable.
- **Data plane** — BitTorrent (librqbit). The file bytes move peer-to-peer; the relay never
  carries them, so you don't eat the bandwidth and multiple downloaders share the load.

**Honest limit:** once a magnet is handed out and a peer joins the swarm, those bytes cannot
be un-sent. "Revocation" cuts off **new** discovery (the live link stops handing out the
magnet), not an in-flight download. The spec and UI say this plainly.

## 2. Architecture

```
  teammate (tnls get / GUI)                              your machine (tnls share / GUI)
  ┌─────────────────────────┐   scoped MCP link        ┌──────────────────────────────┐
  │ headless viewer:        │◄──(tnls.to relay)───────►│ tunnel agent (session::open)  │
  │  hello{token}           │   CONTROL: the magnet,    │  spawns → tnls mcp-serve       │
  │  call request_file      │   never the bytes         │  (returns magnet, scoped)      │
  └───────────┬─────────────┘                           │ + seeds via librqbit (bg task) │
              │                                          └───────────────┬───────────────┘
              │              BitTorrent swarm (DATA)                     │
              └───────────────◄═══ file bytes, peer-to-peer ═══─────────┘
                       DHT + public tracker + UPnP · relay sees no file data
```

The relay and the tunnel agent are **unchanged**. `tnls` is just another MCP server the agent
wraps, plus a programmatic viewer on the receive side.

## 3. Crate layout

New `crates/tnls/` — binary `tnls`. In the existing workspace.

| File | Responsibility |
|------|----------------|
| `crates/tnls/src/main.rs` | clap dispatch: `share` / `get` / `gui` / `mcp-serve`. |
| `crates/tnls/src/cli.rs` | arg definitions. |
| `crates/tnls/src/bittorrent.rs` | librqbit: `seed(path) -> Share{magnet,infohash,name,size}` + background seeding; `fetch(magnet, out_dir) -> ProgressHandle`. Pure data plane, no tunnel. |
| `crates/tnls/src/mcp_serve.rs` | MCP stdio server (same shape as `mcp-demo`): `list_shares`, `request_file`, `status`. Stateless — share details passed as flags on spawn. |
| `crates/tnls/src/share.rs` | `tnls share`: seed + `session::open` on `current_exe mcp-serve …`. |
| `crates/tnls/src/get.rs` | `tnls get`: parse link → headless viewer (relay viewer WS + custom JSON) → magnet → fetch. |
| `crates/tnls/src/gui.rs` | egui/eframe window (share + receive panes). |

**Dependencies:** `tunnel-locker` (path — reuse `session::open`, `OpenArgs`), `librqbit`,
`eframe`+`egui`, `clap`, `tokio`, `serde`/`serde_json`, `anyhow`, `tokio-tungstenite`
(rustls TLS — for the `get` viewer over wss), `futures-util`.

**Reuse trick:** `tnls share` calls `tunnel_locker::session::open(OpenArgs {
server: std::env::current_exe()?, server_args: ["mcp-serve", "--magnet", M, …], relay,
scope: ["list_shares","request_file"], ttl })`. The agent spawns *this same `tnls` binary* in
MCP mode as the scoped child. No PATH dependency, no tunnel refactor. (If `session::open`/
`OpenArgs` need a field or a quiet flag, that's a small, additive change to `tunnel-locker`.)

## 4. BitTorrent core (`bittorrent.rs`) — Phase 1

librqbit (v9 line) `Session`:

- **seed(path):** create a torrent from `path` (file), obtain infohash + a magnet link
  (with a public tracker, e.g. `udp://tracker.opentrackr.org:1337/announce`, appended), and
  add it to the session referencing the existing data so the session seeds it. DHT enabled;
  request a UPnP port map. Returns `Share { magnet, infohash, name, size_bytes }` and keeps
  the session alive (seeding) until dropped/stopped.
- **fetch(magnet, out_dir):** add the magnet to a session pointing at `out_dir`; return a
  handle exposing `Progress { downloaded, total, peers, down_speed, done }` by polling the
  torrent's live stats.

The exact librqbit v9-rc API (session construction, create-torrent, add-torrent, stats
accessors) is pinned during planning against the crate docs; the design only commits to the
two capabilities above, which librqbit supports.

## 5. MCP server (`tnls mcp-serve`) — Phase 2

Stateless stdio JSON-RPC MCP server (mirrors `mcp-demo`'s structure). Spawned by the tunnel
agent with the share details as flags. Methods: `initialize`, `notifications/initialized`,
`tools/list`, `tools/call`. Tools:

- `list_shares` → `[{ name, size }]` (one entry in v1).
- `request_file` (args `{}` or `{name}`) → text content containing the **magnet** for the
  shared file. This is the capability-gated action.
- `status` → swarm stats text (best-effort; may be a placeholder in v1 since seeding lives in
  the parent — see §7).

The tunnel opens with `--scope list_shares,request_file` so a viewer can do only those.

## 6. The two flows

### 6.1 `tnls share <path>` (Phase 2)
1. `seed(path)` → `Share`; spawn the librqbit seed on a background tokio task in **this**
   (parent) process.
2. `session::open(server = current_exe, args = ["mcp-serve", "--magnet", M, "--name", N,
   "--size", S], relay = wss://tnls.to, scope = [list_shares, request_file], ttl)`. The agent
   spawns `tnls mcp-serve` as the MCP child, dials the relay, prints
   `https://tnls.to/t/<id>#<token>`.
3. Parent blocks in the tunnel event loop while seeding continues on the background task.
   Ctrl-C / TTL / `tunnel close` tears down: tunnel closes, seed task aborts, process exits.

### 6.2 `tnls get <link>` (Phase 2) — a headless viewer
1. Parse `https://tnls.to/t/<id>#<token>` → `id`, `token`; derive `wss://tnls.to/viewer/<id>`.
2. Connect (rustls wss), send `{"type":"hello","token":…}`, await `ready` + `tools`.
3. Send `{"type":"call","id":1,"tool":"request_file","args":{}}`; await `result` → extract the
   magnet from the MCP content. (On `error{out_of_scope|expired|unauthorized}` → fail with a
   clear message.)
4. `fetch(magnet, out_dir)` → render progress to the terminal; verify on completion.

`tnls get` is the first non-browser viewer of the tunnel protocol — it reuses the relay + the
agent's enforcement exactly; the agent validates token+scope before returning the magnet.

## 7. Seeding lifecycle

Seeding runs in the **parent** (`tnls share`) process as a background tokio task, not in the
`mcp-serve` child (the child is spawned/killed by the tunnel agent and only serves the static
magnet). This keeps the share's data plane tied to the `share` command's lifetime: kill
`share` → seeding stops. `status` from the child therefore can't see live swarm stats in v1
(separate process); v1 `status` returns a static "seeding" note, and the **sharer's** own
terminal/GUI shows real peer/upload stats from the parent's seed handle.

## 8. GUI (`tnls gui`, egui/eframe)

Native window, pure Rust, single binary. Two panes:

- **Share:** a drop target (`ctx.input(|i| i.raw.dropped_files)`); on drop, start a share and
  show the `tnls.to` link + a copy button + live seed status (peers, uplink) from the parent
  seed handle.
- **Receive:** a text field for a link + "fetch"; shows an `egui::ProgressBar`, speed, peers.

**Async bridge:** a tokio runtime runs on a background thread. The egui frame loop owns an
`Arc<Mutex<UiState>>` (progress/links/status) that workers update, and an `mpsc` command
channel (start-share / start-fetch). egui polls `UiState` each frame and calls
`ctx.request_repaint()` while work is active.

## 9. Connectivity & infra

Best-effort: librqbit with DHT on, a public tracker in the magnet, and a UPnP port-map
request. Connects when at least one peer is reachable (UPnP success or a public IP); hard
double-NAT may require a manual port-forward (documented). Relay-assisted hole-punching is
future work.

`tnls.to` is added as a **second domain on the existing Fly relay app**: `fly certs add
tnls.to` + the A/AAAA records, no new deployment (the relay is domain-agnostic). `tnls`
defaults `--relay wss://tnls.to`.

## 10. Phased build (each phase → its own plan; the demo grows each phase)

- **Phase 1 — BitTorrent core.** `bittorrent.rs` seed + fetch + progress, proven by an
  integration test (seed a temp file in session A, fetch its magnet in session B, bytes
  match). No tunnel. *Gate: bytes move peer-to-peer.*
- **Phase 2 — MCP + tunnel wiring.** `mcp_serve.rs`, `share.rs`, `get.rs`. *Gate: `tnls share`
  prints a `tnls.to` link; `tnls get <link>` downloads; the magnet is reachable only through
  the live, scoped link.*
- **Phase 3 — egui GUI** wrapping both with progress.

## 11. The demo (single unbroken take, by end of Phase 2/3)

1. `tnls share ./demo.bin` → prints `https://tnls.to/t/<id>#<token>`, "seeding."
2. Second machine: `tnls get https://tnls.to/t/<id>#<token>` → progress bar → 100%, checksum
   matches the source.
3. GUI: drag-drop to share; paste the link to fetch; watch the progress bar.
4. Capability angle: the magnet is obtainable **only** through the live link; Ctrl-C the share
   (or let the TTL lapse) → a fresh `tnls get` is refused (link dead), proving scoped,
   revocable discovery.

## 12. Testing

- `bittorrent.rs`: integration test — seed temp file (session A) → fetch magnet (session B) →
  assert byte-equality. (Marked `#[ignore]` if it needs network/UPnP; provide a loopback-only
  variant if librqbit supports it.)
- `mcp_serve.rs`: pure `dispatch(req) -> Option<Value>` unit tests (tool list, request_file
  returns the magnet, unknown method).
- `get.rs`: link parsing (`/t/<id>#<token>` → id+token) + viewer-frame (de)serialization unit
  tests.
- Tunnel path: already covered by `tunnel-locker`'s tests.
- GUI: manual.

## 13. Non-goals (v1)

Multi-file catalogs / persistence; browser/WebTorrent receive; relay-assisted byte transfer;
resume-across-restart shares; at-rest file encryption (BitTorrent payload is unencrypted
beyond transport — noted). Magnet revocation of already-distributed bytes is impossible by
nature; we revoke discovery and state so.
