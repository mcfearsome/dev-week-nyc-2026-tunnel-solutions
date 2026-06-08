# tunnel.solutions

Rust workspace for **ephemeral, capability-scoped tunnels to a local MCP server** — and,
layered on top, **capability-scoped file sending over BitTorrent** (`tnls`). A teammate opens
a disposable link, sees only the tools/files you allow, and the link dies on TTL or close.

## One host + plugins (read this first)

`tnls` is the host binary. It owns `open`/`close`/`plugins` and dispatches to `tnls-<name>` plugin binaries via the `describe` manifest. The `tunnel` binary is retired.

| Binary | Crate | What it is |
|--------|-------|-----------|
| `tnls` | `tnls` | Host: `open`/`close`/`plugins` + plugin dispatch via `describe`. |
| `tnls-demo` | `tnls-demo` (`plugins/demo`) | Sample rmcp MCP server plugin (read + shell). |
| `tnls-rendezvous` | `tnls-rendezvous` (`plugins/rendezvous`) | File sending over BitTorrent plugin. |

The README documents `tnls`. For architecture details trust the crates + `docs/superpowers/`. Recent work: `tnls-rendezvous` (BitTorrent + peer rendezvous — built & tested); don't assume the README is fully current.

## Commands

| Command | Description |
|---------|-------------|
| `cargo build --workspace` | Build everything |
| `cargo test --workspace` | All tests (core unit tests + relay pairing + real-socket e2e) |
| `cargo fmt` / `cargo clippy --workspace` | Format / lint — CI runs these; `.github/workflows/ci.yml` |
| `cargo run -p relay` | Run the relay locally (binds `127.0.0.1:8787`, or `$PORT`) |
| `./demo.sh` | Boot relay + open a 2-min read-only tunnel via demo plugin (Ctrl-C tears down) |
| `cargo run -p tnls -- open <srv> --ttl 2m --scope read` | Open a tunnel to any MCP server (foreground) |
| `cargo run -p tnls -- close [--tunnel-id ID]` | Revoke (SIGTERM; defaults to most recent via $TMPDIR pidfile) |
| `cargo run -p tnls -- demo serve` | Open a tunnel to the demo plugin (read + shell) |
| `cargo run -p tnls -- rendezvous share <file>` | Seed + share a file over a scoped tunnel |
| `cargo run -p tnls -- rendezvous get <link> --out <dir>` | Fetch a shared file from a tunnel link |

## Architecture

```
viewer ──ws──> relay <──ws── agent ──stdio──> tnls-<name> plugin
 (browser /     (pairs by    (holds secret;    (rmcp server over stdio;
  tnls-rendezvous) tunnel_id,  enforces scope    tnls-demo, or
                 opaque bytes) + TTL, filters)   tnls-rendezvous share)

crates/
  tnls-core/           Pure logic: HMAC token, scope filter, per-call decision, TTL parse, frames. No I/O; exhaustively unit-tested.
  tnls-tunnel/         The tunnel agent library: session::open (secret, scope, TTL), McpChild (rmcp client over TokioChildProcess).
  tnls-plugin/         The `describe` manifest contract shared by host + plugins.
  tnls/                The host binary (bin: tnls): open/close/plugins + dispatches to tnls-<name> plugins via describe manifest.
  plugins/demo/        bin: tnls-demo — sample rmcp MCP server (read + shell). Subcommands: serve | describe.
  plugins/rendezvous/  bin: tnls-rendezvous — file sending over BitTorrent. Subcommands: share | get | seed | fetch | describe.
  relay/               Rocket WS pairing service (bin: relay). In-memory, no state past session. The only deployed component.
viewer/                Static HTML (index/landing/stats), no build step — include_str!'d into the relay binary.
docs/superpowers/      Design specs + phased implementation plans (see Design docs below).
```

## Security invariants (do not "improve" these away)

- **All enforcement lives at the agent, never the relay.** The relay only pairs a viewer
  socket to an agent socket by `tunnel_id` and shuttles opaque frames; it never holds the
  signing secret and keeps no state past the session. A compromised relay must not be able to
  widen scope or extend a TTL. Don't add trust, scope logic, or persistence to its forwarding path.
- The **capability token** is HMAC-SHA256 over `{tunnel_id, scope, exp}`; the 32-byte secret
  is per-session, in memory only. Signature verified once at handshake; `exp` re-checked on
  **every** `tools/call`.
- The token rides in the link's **URL fragment** (`#…`) so it never reaches the relay in an HTTP request line.
- **Analytics are aggregate-only** (`/report`, `/stats.json`): counts only — no per-session
  data, no tool I/O, no stored IPs (IP is salted-hashed in memory for visitor dedupe). Keep it that way.

## Gotchas

- **README now documents `tnls`** — the primary binary is `tnls` (crate `tnls`); the agent library is `tnls-tunnel`. Trust the crates + `docs/superpowers/` for full detail.
- **`librqbit` is pinned to `=9.0.0-rc.0`** (`crates/plugins/rendezvous/Cargo.toml`) — only a pre-release exists, so caret `9` won't resolve. Don't loosen without checking crates.io.
- **The relay defaults to in-memory single-instance** — by default the registry is a local HashMap (`LocalBackplane`), so viewer and agent must hit the *same* process, and `fly.toml` pins 1 machine. A **`RedisBackplane` is implemented** (`crates/relay/src/backplane.rs`) for multi-instance pairing via Redis pub/sub — set `REDIS_URL` to enable it.
- **`tnls-rendezvous share` is the self-seeding rmcp server the host spawns** — it seeds the file and serves a `ShareServer` over stdio; don't run it by hand (the host passes `TNLS_RELAY` to it).
- **Editing `viewer/*.html` needs a relay rebuild** — the HTML is `include_str!`'d into the binary at compile time.
- **CI is live** (GitHub Actions: fmt check, clippy, test matrix, audit, Fly deploy). No repo fmt/clippy config (defaults apply), but run `cargo fmt --all` / `cargo clippy --workspace` / `cargo test --workspace` locally before pushing — the **fmt check will fail CI** otherwise.

## Deploy

Fly.io app `tunnel-locker` (region `iad`). The Dockerfile builds **only** the `relay`
binary (`cargo build --release -p relay`); TLS is terminated by Fly, so the relay speaks
plain `ws` internally and binds `$PORT` (8080 on Fly). Aggregate stats persist to
`STATS_PATH=/data/stats.json` on a Fly volume. See `fly.toml` + `Dockerfile`.

## Design docs

`docs/superpowers/specs/` (designs, each with a **Status** header) and `.../plans/` (phased
plans). Current state: `tnls` Phases 1–2 (BitTorrent core + magnet-through-tunnel) and Phase 2.5
(peer **rendezvous** — the seeder advertises its address through the tunnel so transfers connect
directly instead of waiting on DHT) are **built and tested**. The **Redis relay** backplane is
**implemented and cross-instance-tested** (`crates/relay/src/backplane.rs`,
`crates/relay/tests/redis_backplane.rs`), and the relay now runs on **Rocket** (rewritten from Axum).
