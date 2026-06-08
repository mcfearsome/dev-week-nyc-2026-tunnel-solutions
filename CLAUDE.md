# tunnel.solutions

Rust workspace for **ephemeral, capability-scoped tunnels to a local MCP server** — and,
layered on top, **capability-scoped file sending over BitTorrent** (`tnls`). A teammate opens
a disposable link, sees only the tools/files you allow, and the link dies on TTL or close.

## Two product surfaces (read this first)

The README documents only `tunnel-locker`. There are **two** CLIs:

| Binary | Crate | What it is | Status |
|--------|-------|-----------|--------|
| `tunnel` | `tunnel-locker` | Original: scoped tunnel to a local MCP server (filters `tools/list`, refuses out-of-scope `tools/call`, enforces TTL). | Stable |
| `tnls`   | `tnls`          | Newer: seed a file over BitTorrent + hand out the magnet through a scoped tunnel. Built **on top of** `tunnel-locker`. | Active dev |

Recent work: `tnls` (BitTorrent + peer rendezvous — built & tested) and the **Redis multi-machine relay** (built & verified cross-instance; live rollout in progress). Don't assume the README is current.

## Commands

| Command | Description |
|---------|-------------|
| `cargo build --workspace` | Build everything |
| `cargo test --workspace` | All tests (core unit tests + relay pairing + real-socket e2e) |
| `cargo fmt` / `cargo clippy --workspace` | Format / lint — no repo config, defaults apply, no CI runs these for you |
| `cargo run -p relay` | Run the relay locally (binds `127.0.0.1:8787`, or `$PORT`) |
| `./demo.sh` | Boot relay + open a 2-min read-only tunnel to `mcp-demo` (Ctrl-C tears down) |
| `cargo run -p tunnel-locker -- open <srv> --ttl 2m --scope read` | Open a tunnel (foreground) |
| `cargo run -p tunnel-locker -- close [--tunnel-id ID]` | Revoke (SIGTERM; defaults to most recent via $TMPDIR pidfile) |
| `cargo run -p tnls -- share <file> --ttl 30m` | Seed + share a file over a scoped tunnel |
| `cargo run -p tnls -- get <link> --out <dir>` | Fetch a shared file from a tunnel link |

## Architecture

```
viewer ──ws──> relay <──ws── agent ──stdio──> local MCP server
 (browser /     (pairs by    (holds secret;    (real MCP JSON-RPC;
  tnls get)      tunnel_id,   enforces scope    mcp-demo, or the hidden
                 opaque bytes) + TTL, filters)   `tnls mcp-serve`)

crates/
  tunnel-locker-core/  Pure logic: HMAC token, scope filter, per-call decision, TTL parse, frames. No I/O; exhaustively unit-tested.
  relay/               Axum WS pairing service (bin: relay). In-memory, no state past session. The only deployed component.
  tunnel-locker/       The `tunnel` CLI (bin: tunnel): spawns MCP child over stdio, bridges to relay, enforces scope + TTL.
  tnls/                The `tnls` CLI: BitTorrent (librqbit) file sharing over a tunnel. Subcommands: seed/fetch/share/get/mcp-serve.
  mcp-demo/            Sample MCP server (bin: mcp-demo) exposing `read` + `shell`. Used by demo.sh and e2e tests.
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

- **README is behind the code** — it covers `tunnel-locker` only and omits `tnls`. Trust the crates + `docs/superpowers/`.
- **`librqbit` is pinned to `=9.0.0-rc.0`** (`crates/tnls/Cargo.toml`) — only a pre-release exists, so caret `9` won't resolve. Don't loosen without checking crates.io.
- **The relay pairs via a pluggable `Backplane`** (`crates/relay/src/backplane.rs`): `LocalBackplane` (in-memory, single-instance, zero deps — the default when `REDIS_URL` is unset) and `RedisBackplane` (cross-instance via Redis pub/sub + presence keys, enabled by `REDIS_URL`). Cross-instance pairing is proven by `tests/cross_instance.rs` (one shared backplane, two in-process apps) and against a real `redis-server`. `main.rs` connects Redis with a 10s timeout and **falls back to Local + logs loudly** on failure, so a bad `REDIS_URL` degrades to single-instance rather than an outage. The live multi-machine rollout is in progress (rustls TLS → managed Redis); keep `fly.toml` at 1 machine until `REDIS_URL` (a `rediss://` URL) is set and the app is scaled.
- **`tnls mcp-serve` is internal** (hidden subcommand) — the tunnel spawns it during `tnls share`; never run it by hand.
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
directly instead of waiting on DHT) are **built and tested**; the **Redis relay** backplane is
**built and verified cross-instance**, with the live multi-machine rollout in progress.
