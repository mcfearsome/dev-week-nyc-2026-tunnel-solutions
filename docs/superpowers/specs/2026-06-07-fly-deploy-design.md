# tunnel.solutions — Fly.io Deployment Design

**Date:** 2026-06-07
**Status:** Approved (pre-implementation)
**Scope:** Make the relay deployable and run it live on Fly.io at `tunnel.locker`.

## Goal

Get the `relay` running as a public service so a remote teammate can open a real
`https://tunnel.locker/t/<id>#<token>` link and use a locally-running MCP server through it.
Only the relay is hosted; the agent stays local (dials outbound), the viewer is served by the
relay. No database — the relay is stateless, one machine.

## 1. Code changes (deployability)

| File | Change |
|------|--------|
| `crates/relay/src/main.rs` | Bind `0.0.0.0:${PORT}` from env (default `8787`) instead of `127.0.0.1:8787`. Fly injects `PORT=8080`. Local default unchanged. |
| `crates/relay/src/lib.rs` | Serve the viewer via `include_str!("../../../viewer/index.html")` (compile-time embed) instead of runtime `tokio::fs::read_to_string`. Self-contained binary, no CWD/file dependency in the container. |
| `viewer/index.html` | Derive WS scheme from page protocol: `const proto = location.protocol === 'https:' ? 'wss' : 'ws'`. Browsers block `ws://` from an `https://` page, so this is mandatory. |
| `crates/agent/src/session.rs` | The printed link scheme tracks `--relay`: `wss://` → `https://` link, `ws://` → `http://`. Add a small `link_scheme(relay) -> &str` helper (currently hardcodes `http://`). |

Backward-compat: `cargo run -p relay` still binds `8787`; `demo.sh` and the agent's
`ws://127.0.0.1:8787` default are unchanged. The local demo and all existing tests keep working.

## 2. Minimal hardening (public open relay)

| Cap | Value | Where |
|-----|-------|-------|
| Max concurrent tunnels | `MAX_TUNNELS` env, default 256 | `relay`: reject a new `/agent` upgrade when the registry is full (close with a reason). |
| Max WS message size | 1 MiB | axum `WebSocketUpgrade::max_message_size` (+ `max_frame_size`) so one giant frame can't OOM the relay. |

Deliberately **not** an idle timeout (would kill legitimately-idle tunnels waiting on a
teammate). Per-IP limiting is fast-follow — it needs the `Fly-Client-IP` header behind Fly's
proxy.

## 3. Packaging

- **`Dockerfile`** (multi-stage): `rust:1-bookworm` builder runs `cargo build --release -p relay`;
  runtime `debian:bookworm-slim` copies just the `relay` binary. Viewer is embedded, so nothing
  else ships. `ENV PORT=8080`, `EXPOSE 8080`.
- **`.dockerignore`**: `target/`, `.git/`, `docs/`, `*.png`, `.playwright-mcp/`.
- **`fly.toml`**: app `tunnel-relay`, `primary_region = "iad"`, `[http_service]` with
  `internal_port = 8080`, `force_https = true`, `min_machines_running = 1`,
  `auto_stop_machines = "off"` (single warm machine — see §4), and a `[[http_service.checks]]`
  hitting `/healthz`. Fly proxies WebSockets natively over the http service.

## 4. Single-machine constraint

The tunnel registry is in-memory, so a viewer and its agent must reach the **same** machine.
`fly.toml` pins one machine (`min_machines_running = 1`, no scale-out). This is the known
horizontal-scale limit; scaling out later needs sticky-routing by `tunnel_id` or a shared
backplane. Acceptable and intended for launch.

## 5. Deploy & verify flow

1. **User:** `fly auth login` — done (`jesse@mcfearsome.dev`).
2. **Me:** `fly launch --no-deploy --copy-config --name tunnel-relay --region iad` (register the
   app from the committed `fly.toml`) → `fly deploy` (remote Docker build).
3. **Me:** verify `https://tunnel-relay.fly.dev/healthz` → `ok`; then run
   `tunnel open ./target/debug/mcp-demo --relay wss://tunnel-relay.fly.dev --scope read` locally
   and drive a browser to the public link to confirm **read succeeds / shell refused over the
   internet**.
4. **Custom domain:** `fly certs add tunnel.locker` → Fly returns the required A/AAAA records →
   **User adds them at the `tunnel.locker` registrar** → cert issues → verify
   `https://tunnel.locker/healthz` and a `--relay wss://tunnel.locker` tunnel.

## 6. Testing

- New Rust unit tests: `link_scheme` (`ws://…`→`http`, `wss://…`→`https`) and the max-tunnels
  rejection path.
- Existing 29 tests stay green (relay pairing test uses an ephemeral port via `build_app()`).
- Viewer `wss://` derivation is verified live in the browser against the deployed relay.

## 7. Non-goals (this pass)

Per-IP rate limiting, multi-machine horizontal scale, end-to-end payload encryption (the relay
still sees plaintext tool I/O — fine because **you** operate this relay). These remain noted
future work in the README.
