# tunnel.solutions

A **capability-scoped, ephemeral tunnel that understands the protocol it carries.** A teammate
opens a disposable link to your locally-running agent, sees only what you allow, and the link
evaporates on TTL or close.

Two products on one primitive:

| Binary | Crate | What it is |
|--------|-------|-----------|
| `tunnel` | `tunnel-locker` | A scoped tunnel to your local **MCP server** — filters `tools/list`, refuses out-of-scope `tools/call` agent-side, enforces a TTL on every call. |
| `tnls` | `tnls` | **Capability-scoped file sending over BitTorrent**, built *on top of* the tunnel: seed a file, hand out the magnet through a scoped link, bytes move peer-to-peer. |

**Live relay + landing page: https://tunnel.locker**

## Why this isn't ngrok

ngrok, Cloudflare Tunnel, and frp expose a *port* — they have no idea what's on the other
end. tunnel.solutions knows the far end is an **MCP server with a tool list** (or a file in a
swarm), so it scopes at the **capability** level instead of the network level: it filters the
advertised `tools/list` to a `--scope` allowlist, refuses any out-of-scope `tools/call`
agent-side before the server ever sees it, and enforces a TTL on every call so revocation is
real rather than cosmetic.

## Architecture

```
viewer ──ws──> relay <──ws── agent ──stdio──> local MCP server
 (browser /     (pairs by    (holds the secret;   (real MCP JSON-RPC:
  tnls get)      tunnel_id;    enforces scope        mcp-demo, or the hidden
                 opaque bytes) + TTL, filters)       `tnls mcp-serve`)

                            ⇣ for tnls, the file bytes never touch the relay ⇣
        teammate  ◄════════════ BitTorrent swarm (data plane) ════════════►  you
```

The **relay** is deliberately dumb: it pairs a viewer socket to an agent socket by `tunnel_id`
and shuttles **opaque** frames, never holding the signing secret. All enforcement lives at the
**agent** — the only component holding the secret and sitting between the viewer and the real
MCP server. A compromised relay cannot widen scope or extend a TTL; it only moves bytes.

For **tnls**, that split becomes literal: the tunnel is the *control plane* (it gates which
file a teammate may fetch, and for how long) and **BitTorrent is the data plane** — the file
bytes move peer-to-peer and never pass through the relay. The tunnel even *bootstraps* the
swarm: the seeder advertises its peer address through the scoped `request_file` call, so the
downloader connects directly instead of waiting on the BitTorrent DHT.

## Demo — the MCP tunnel

```bash
# 1. start the relay (or just use the live one at wss://tunnel.locker)
cargo run -p relay

# 2. open a 2-minute, read-only tunnel to the sample MCP server (another terminal)
cargo build -p mcp-demo
cargo run -p tunnel-locker -- open ./target/debug/mcp-demo --ttl 2m --scope read
#   → prints a link like http://127.0.0.1:8787/t/<id>#<token>

# 3. open the link as a "teammate" in a browser:
#      - call `read` (path: demo/notes.txt)  → succeeds
#      - raw-call `shell` {"cmd":"echo hi"}   → refused, out of scope
#
# 4. let the TTL lapse → the link goes dead live, or
#    cargo run -p tunnel-locker -- close   # revoke on demand (SIGTERM via pidfile)
```

Or just `./demo.sh` (boots the relay and opens the tunnel; Ctrl-C tears down).

## Demo — sending a file (tnls)

```bash
# seed a file and open a scoped link (defaults to --relay wss://tunnel.locker)
cargo run -p tnls -- share ./big.mov --ttl 30m
#   → prints https://tunnel.locker/t/<id>#<token>

# on another machine, fetch it over BitTorrent through the scoped link
cargo run -p tnls -- get 'https://tunnel.locker/t/<id>#<token>' --out ./downloads
```

The magnet is reachable **only** through the live, scoped link; close the share (or let the
TTL lapse) and no new peer can discover it.

## Token model

A capability token is an HMAC-SHA256-signed JSON payload `{ tunnel_id, scope, exp }`. The
32-byte secret is generated per session and held in memory only — the only credential, no
accounts. The signature is verified once at handshake; `exp` is re-checked on **every**
`tools/call`, so TTL expiry is enforced, not advisory. The token rides in the link's URL
fragment (`#…`) so it is never sent in an HTTP request line to the relay.

## Workspace

| Crate | Role |
|-------|------|
| `tunnel-locker-core` | Pure core: HMAC token, scope filter, per-call decision, TTL parse, frames. Exhaustively unit-tested, no I/O. |
| `relay` | Axum WS pairing service (binary: `relay`). Pairs by `tunnel_id` via a pluggable `Backplane` (in-memory, or Redis for multi-instance). The only deployed component. |
| `tunnel-locker` | The `tunnel` CLI: spawns the MCP child over stdio, bridges to the relay, enforces scope + TTL. |
| `tnls` | The `tnls` CLI: BitTorrent (librqbit) file sharing over a tunnel. Subcommands: `share` / `get` / `seed` / `fetch` / `mcp-serve` (internal). |
| `mcp-demo` | Sample MCP server exposing `read` + `shell` (binary: `mcp-demo`). |
| `viewer/*.html` | Static viewer / landing / stats pages, no build step — `include_str!`'d into the relay binary. |

## Tests

```bash
cargo test --workspace
```

`tunnel-locker-core` is unit-tested exhaustively; `relay` has in-process WS pairing **and**
cross-instance backplane tests; `tunnel-locker` and `tnls` have real-socket end-to-end tests
(scope enforcement, and a hermetic BitTorrent transfer asserting byte-for-byte equality).

## Deploy

Fly.io app `tunnel-locker` (region `iad`). A multi-stage Dockerfile builds **only** the
`relay` binary; the viewer HTML is `include_str!`'d into it, and TLS is terminated by Fly so
the relay speaks plain `ws` internally on `$PORT`. Aggregate-only analytics (no trackers, no
cookies, no stored IPs) persist to a Fly volume. Set `REDIS_URL` (a `rediss://` URL) to enable
the multi-instance Redis backplane and scale out; without it the relay runs single-instance
in-memory.

## Status & future work

`tnls` (BitTorrent core, magnet-through-tunnel, and peer **rendezvous**) is built and tested.
The **Redis multi-machine relay** backplane is built and verified cross-instance, with the
live rollout in progress. **Next:** NAT hole-punching for cross-internet transfers, a native
GUI (egui) for `tnls`, payload encryption beyond the transport (so even the relay operator
can't read tool I/O), and multi-viewer sessions with per-viewer scopes + audit logging.

Design specs and phased implementation plans live in [`docs/superpowers/`](docs/superpowers/).
