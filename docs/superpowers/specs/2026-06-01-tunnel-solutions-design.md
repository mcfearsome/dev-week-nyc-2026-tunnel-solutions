# tunnel.solutions — Design Spec

**Date:** 2026-06-01
**Status:** Approved (pre-implementation)
**Context:** Hackathon build for DeveloperWeek NY 2026. Domain: `tunnel.solutions`.

## 1. Problem & Wedge

Local-first AI tooling keeps the model and data on your machine. Occasionally a teammate
needs to talk to your *running* MCP server. Existing tunnels (ngrok, Cloudflare Tunnel,
frp) expose a port; they have no idea what's on the other end.

`tunnel` is different in exactly one load-bearing way: **the far end is an MCP server with a
tool list**, so the tunnel can be *capability-scoped*. It is **MCP-aware, ephemeral, and
capability-scoped**: it filters the advertised `tools/list` to an allowlist, refuses any
out-of-scope `tools/call` agent-side, and enforces a TTL on every call so revocation is real
rather than cosmetic. Everything else is plumbing in service of that demo.

### The wedge, stated as three enforced invariants
1. `tools/list` returned to the viewer is filtered to the `--scope` allowlist.
2. `tools/call` for an out-of-scope tool is rejected **agent-side**, before the child sees it.
3. TTL `exp` is checked on **every** call, not just at handshake; expiry kills the link live.

## 2. Trust Model (the keystone invariant)

The relay is deliberately dumb: it pairs two WebSockets by `tunnel_id` and shuttles opaque
frames. It never holds the signing secret and never parses our application protocol.
Therefore **all enforcement lives at the agent** — the only component that holds the secret
and sits between the viewer and the real MCP server. A compromised or malicious relay cannot
widen scope or extend a TTL because it never possesses the authority to do so; it only moves
bytes. Preserving this boundary is the security story.

## 3. Architecture

```
viewer ──wss──> relay <──wss── agent ──stdio──> mcp-demo (local MCP server)
   (custom JSON)  (pairs by    (enforces scope    (real MCP JSON-RPC,
                   tunnel_id)    + TTL, filters)    newline-delimited)
```

Two **distinct** wire protocols meet at the agent, which is the translator:

- **A. viewer ↔ agent** — a small custom JSON protocol, shuttled opaquely by the relay.
- **B. agent ↔ mcp-demo** — real MCP (JSON-RPC 2.0) over newline-delimited stdio.

Making the viewer speak the custom protocol (rather than raw MCP end-to-end) keeps the
viewer ~60 lines of vanilla JS with no `initialize`/id-management/capability-negotiation to
get wrong on a live take, and keeps the relay genuinely dumb. The agent must intercept and
filter regardless, so nothing is lost.

## 4. Workspace Layout

```
tunnel-solutions/
├── Cargo.toml                  # [workspace] members + shared dependency versions
├── README.md
├── crates/
│   ├── protocol/               # shared truth: frames, token, scope/ttl — pure, no I/O
│   ├── relay/                  # axum WS pairing service        (binary: relay)
│   ├── agent/                  # the CLI                        (binary: tunnel)
│   └── mcp-demo/               # sample MCP server, read + shell (binary: mcp-demo)
├── viewer/index.html           # static, no build step
├── demo/notes.txt              # a file for `read` to return
└── docs/superpowers/specs/     # this document
```

`protocol` holds the only logic that must be *correct* — token verify, scope check, TTL —
as pure functions with no I/O. It receives the heaviest tests.

## 5. The `protocol` Crate (shared core)

### 5.1 Token

- **Claims:** `{ tunnel_id: String, scope: Vec<String>, exp: u64 /* unix seconds */ }`.
- **Format:** `token = b64url(payload_json) + "." + b64url(HMAC_SHA256(secret, payload_json))`.
- **Secret:** random 32 bytes, generated on `open`, held in memory only, never persisted,
  never sent to the relay.
- **API:**
  - `mint(secret: &[u8], claims: &Claims) -> String`
  - `verify(secret: &[u8], token: &str, now: u64) -> Result<Claims, TokenError>`
    — recomputes HMAC with a **constant-time** compare, then checks `exp > now`.
  - `TokenError` variants: `Malformed`, `BadSignature`, `Expired`.

### 5.2 Scope

- `Claims::allows(tool: &str) -> bool` — exact-match membership in `scope`. No globbing
  (YAGNI for the demo).
- `filter_tools(all: &[Tool], scope: &[String]) -> Vec<Tool>` — keep only tools whose
  `name` is in scope. This is the function that produces the filtered `tools/list`.

### 5.3 TTL

- `parse_ttl(s: &str) -> Result<Duration, _>` via `humantime` (`2m`, `15m`, `90s`).
- Per-call enforcement re-checks `now < exp` (see §7.3). Signature is verified once at
  `hello`; `exp` is checked again on every call.

### 5.4 Frames (viewer ↔ agent custom protocol)

Serde-tagged enums (`#[serde(tag = "type")]`), lowercase variants:

```
viewer → agent:
  Hello  { token: String }
  List
  Call   { id: u64, tool: String, args: serde_json::Value }

agent → viewer:
  Ready  { scope: Vec<String>, expires_in_ms: u64 }  // RELATIVE ms remaining = (exp - now)*1000
  Tools  { tools: Vec<Tool> }                      // already filtered
  Result { id: u64, content: serde_json::Value }   // MCP content array, passed through
  Error  { id: Option<u64>, code: ErrorCode, tool: Option<String>, message: Option<String> }
```

`ErrorCode` = `unauthorized | expired | out_of_scope | tool_error | bad_request`.

**Notation & units.** Throughout this doc, `Error{expired}` is shorthand for
`Error{code: expired}` — the bare word is an `ErrorCode` value, not a positional field.
`bad_request` is emitted when the agent receives a frame it cannot parse, or a `Call` whose
`tool`/`args` are structurally invalid (e.g. the raw-call box submits malformed JSON); it
refuses that one frame and keeps the session live. `expires_in_ms` is **relative**
milliseconds remaining at send time — `(claims.exp.saturating_sub(now)) * 1000` — and the
viewer counts down from it locally; the absolute `exp` (unix seconds) never leaves the agent,
which also sidesteps server↔client clock skew.

`Tool` = `{ name: String, description: Option<String>, input_schema: serde_json::Value }`
(mirrors the MCP tool shape we pass through to the viewer).

## 6. relay (axum WS pairing)

### 6.1 Routes
- `GET /agent/:id`  — WS upgrade, agent role.
- `GET /viewer/:id` — WS upgrade, viewer role.
- `GET /t/:id`      — serves `viewer/index.html` (the human-facing link target).
- `GET /healthz`    — `200 "ok"`.

### 6.2 Pairing
Shared state: `Mutex<HashMap<String /*tunnel_id*/, TunnelSlot>>` where
`TunnelSlot { agent_tx: Option<mpsc::UnboundedSender<Message>>, viewer_tx: Option<…> }`.
Each `*_tx` is "messages to be **sent to** that side's socket".

Each handler, after `ws.split()` into `(sink, stream)`:
1. Creates its own `(tx, rx)`; stores `tx` in the slot under its role.
2. Spawns a **writer** task: drains `rx` → writes to its `sink`.
3. Runs a **reader** loop: each inbound message → look up the *other* side's `tx` in the
   slot → forward verbatim (`Text`/`Binary` passed through; `Close` ends the loop).

Teardown: when a reader loop ends (socket closed/errored), remove the tunnel slot and drop
the other side's `tx`. Dropping it closes the peer writer's `rx`, which closes the peer
sink, which ends the peer reader — a clean cascade. **No state survives the session.**

Single agent + single viewer per `tunnel_id`. A second agent/viewer for an occupied id is
rejected (close with a reason). No reconnection logic.

## 7. agent (`tunnel` CLI)

### 7.1 Commands
- `tunnel open <server-cmd> [args…] [--ttl <dur>] [--scope a,b] [--relay <ws-url>]`
  — foreground; everything after `<server-cmd>` up to the first recognised flag is child argv.
- `tunnel close [--tunnel-id <id>]` — reads the pidfile and sends SIGTERM; defaults to latest.

`--ttl` default `15m`; `--scope` default empty → **deny-all** (explicit, safe default);
`--relay` default `ws://127.0.0.1:8787`.

### 7.2 `open` startup sequence
1. Parse flags; `parse_ttl`; split scope on `,`.
2. Generate `tunnel_id` (random 8-byte hex) and 32-byte secret (`getrandom`/`rand`).
3. Spawn the child MCP server (`tokio::process::Command`, piped stdin/stdout/stderr).
4. MCP handshake with the child: `initialize` → `notifications/initialized` → `tools/list`.
   Cache the child's real tool list.
5. Compute `exp = now + ttl`; `mint` the token.
6. Dial the relay at `<relay>/agent/<id>`.
7. Write pidfile `${TMPDIR}/tunnel-<id>.pid` (pid + tunnel_id + link) and update
   `${TMPDIR}/tunnel-latest.pid`.
8. Print the CLI banner (§7.5). Enter the event loop (§7.3).

**Startup failure (steps 3–4, before the relay is dialed).** If the child binary is missing,
the MCP `initialize` handshake never completes, or `tools/list` fails or times out (~5 s
budget), the agent prints a clear `anyhow` error and exits non-zero **without** dialing the
relay, writing a pidfile, or minting a token — a failed launch leaves nothing half-open. This
is the likeliest thing to break live, so it fails loud and early.

### 7.3 Event loop & enforcement
A background reader task on the child's stdout dispatches JSON-RPC responses to `oneshot`
channels keyed by `id` (notifications interleave, so id-correlation is required, not optional).

Handling viewer frames arriving over the relay socket:
- `Hello{token}` → `verify(secret, token, now)`. On error → `Error{unauthorized|expired}` and
  close. On success → cache `claims`; reply `Ready{scope, expires_in_ms}` then `Tools{filter_tools(child_tools, scope)}`.
- `List` → `Tools{filter_tools(child_tools, scope)}`.
- `Call{id, tool, args}` → **enforce in order**:
  1. `now >= claims.exp` → `Error{id, expired}`.
  2. `!claims.allows(tool)` → `Error{id, out_of_scope, tool}`.
  3. else forward MCP `tools/call` to the child; on reply → `Result{id, content}`; on MCP
     error → `Error{id, tool_error, message}`.

A `tokio::time::sleep_until(exp)` task fires at expiry: send `Error{expired}` to the viewer,
then tear down (drop relay socket). Expiry is therefore visible live, not only on next call.

### 7.4 Shutdown (three paths, one teardown)
TTL expiry, `tunnel close` (SIGTERM), and Ctrl-C (SIGINT) all converge on: drop the relay
socket (→ relay tears down the pairing → link dead) → kill the child → remove the pidfile.

### 7.5 CLI banner (matches landing-page terminal)
```
  tunnel.solutions
  ─────────────────
  spawn      ./mcp-demo … ok
  handshake  ✓   MCP initialize — 2 tools advertised
  scope      read              (1 of 2 in scope)
  ttl        2m0s              (expires 14:32:10)
  token      ✓   minted
  relay      ws://127.0.0.1:8787  connected
  link  →    http://127.0.0.1:8787/t/3f9a8c…#<token>

  serving — ctrl-c or `tunnel close` to revoke
```
On revoke/expiry: `  ttl expired — tunnel revoked. link is dead.` /
`  revoked. link is dead.`

The viewer link carries the token in the URL **fragment** (`#<token>`) so it is never sent
in an HTTP request line to the relay; the page reads `location.hash` and delivers it to the
agent inside the first `Hello` frame.

## 8. viewer (static HTML/JS)

Single `viewer/index.html`, no build step. On load: parse `tunnel_id` from the `/t/:id`
path and `token` from `location.hash`; open `ws://<host>/viewer/<id>`; send `Hello{token}`.

UI states: `connecting` → `live` → `dead` (expired/closed/unauthorized).

- On `Tools` → render a button per tool with minimal arg inputs derived from `input_schema`.
- **Raw call box** (first-class, not hidden): a tool-name field + JSON args field + "send".
  This exists specifically so the demo can attempt `shell` (which is absent from the filtered
  list) and show the agent refusing it. Hiding `shell` is necessary; *refusing an explicit
  attempt* is the proof.
- On `Result` → render content. On `Error` → render `out_of_scope` / `expired` /
  `unauthorized` distinctly so the demo reads clearly on screen.

Aesthetic: dark, monospace, terminal-flavored; reuse the ✓ / → glyphs from the CLI.

## 9. mcp-demo (sample MCP server)

Real MCP server over stdio (newline-delimited JSON-RPC 2.0). Methods: `initialize`,
`notifications/initialized`, `tools/list`, `tools/call`.

- `read` — input `{ path: string }`; returns file contents as MCP text content
  (`{content:[{type:"text",text:…}]}`). Defaults to / sandboxed around `demo/notes.txt`.
- `shell` — input `{ cmd: string }`; runs the command, returns stdout as text content.

`shell` is **fully functional at the server**. The demo's entire point is that it is
reachable at the server but **refused at the tunnel** under `--scope read`. The contrast is
real, not staged.

## 10. Error Handling Summary

| Condition                | Frame to viewer                  | Session |
|--------------------------|----------------------------------|---------|
| Bad signature            | `Error{unauthorized}`            | closes  |
| Token/session expired    | `Error{expired}`                 | closes  |
| Out-of-scope `call`      | `Error{id, out_of_scope, tool}`  | stays   |
| Malformed frame / args   | `Error{id?, bad_request, msg}`   | stays   |
| Child MCP error          | `Error{id, tool_error, message}` | stays   |
| Transport disconnect     | (socket close)                   | cascade |
| Child process dies       | tear down                        | closes  |

## 11. Testing Approach

Heaviest where correctness matters — pure functions in `protocol`:
- token mint → verify roundtrip; tampered signature rejected; expired token rejected;
- `Claims::allows` scope membership; `parse_ttl` parsing;
- `filter_tools`: child `[read, shell]` ∩ scope `[read]` = `[read]`.

Plus one lightweight in-process WS round-trip test for relay pairing. End-to-end is the
manual single-take demo, with an optional `demo.sh` that boots relay + tunnel for convenience.

## 12. The Demo Script (must run in one unbroken take)

1. `cargo run -p relay` (terminal 1).
2. `cargo run -p agent -- open ./target/debug/mcp-demo --ttl 2m --scope read` (terminal 2).
3. Open the printed link in a browser ("teammate"): `read` → succeeds; `shell` (via the raw
   call box) → refused as out-of-scope.
4. Let the 2-minute TTL expire live → link goes dead, calls rejected.
   (Or `tunnel close` from terminal 3 to revoke on demand.)

## 13. Dependencies (per crate)

- **workspace shared:** `tokio` (rt-multi-thread, macros, process, io-util, signal, sync,
  time), `serde`, `serde_json`.
- **protocol:** `serde`, `serde_json`, `hmac`, `sha2`, `base64`, `humantime`,
  `subtle` (constant-time compare), `thiserror`.
- **relay:** `axum` (ws), `tokio`, `futures-util`, `tower-http` (static file serve),
  `tracing` (optional).
- **agent:** `tokio`, `tokio-tungstenite`, `futures-util`, `clap` (derive), `serde`,
  `serde_json`, `getrandom` (or `rand`), `anyhow`, `nix` (or `libc`) for `kill`,
  plus `protocol`.
- **mcp-demo:** `tokio`, `serde`, `serde_json`, `anyhow`.

## 14. Non-Goals (held firm)

No persistence/DB; no accounts/login/OAuth; single viewer, no reconnection; `ws://` only
(TLS is the production relay's job); transport-only encryption — payload encryption noted as
future work in the README. If a feature doesn't serve the §12 demo, it is cut.

## 15. Acceptance Criteria

- [ ] `cargo run -p relay` starts the relay.
- [ ] `tunnel open ./mcp-demo --ttl Xm --scope a,b` prints a working viewer link.
- [ ] Viewer sees only in-scope tools; an out-of-scope `tools/call` is refused agent-side.
- [ ] TTL expiry and `tunnel close` both kill the session immediately.
- [ ] README documents the demo script and names the wedge (MCP-aware, ephemeral,
      capability-scoped) in two sentences.
