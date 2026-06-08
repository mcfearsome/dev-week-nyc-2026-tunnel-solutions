# tunnel.solutions

A **capability-scoped, ephemeral tunnel that understands the protocol it carries.** A teammate
opens a disposable link to your locally-running agent, sees only what you allow, and the link
evaporates on TTL or close.

One pluggable host (`tnls`) + plugins — two use-cases on one primitive:

| Binary | Crate | What it is |
|--------|-------|-----------|
| `tnls` | `tnls` | Host: `open`/`close`/`plugins` + plugin dispatch via `describe`. |
| `tnls-demo` | `tnls-demo` (`plugins/demo`) | Sample rmcp MCP server plugin exposing `read` + `shell` — scoped tunnel to your local MCP server. |
| `tnls-rendezvous` | `tnls-rendezvous` (`plugins/rendezvous`) | **Capability-scoped file sending over BitTorrent**: seed a file, hand out the magnet through a scoped link, bytes move peer-to-peer. |
| `tnls-rendezvous-gui` | `rendezvous-gui` (`crates/rendezvous-gui`) | Native **egui** front-end for `tnls rendezvous` — drag-to-send, paste-a-link-to-receive, live progress. A thin driver over the host CLI. |

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
viewer ──ws──> relay <──ws── agent ──stdio──> tnls-<name> plugin
 (browser /     (pairs by    (holds the secret;   (rmcp server over stdio:
  tnls-rendezvous) tunnel_id;  enforces scope        tnls-demo, or
                 opaque bytes) + TTL, filters)       tnls-rendezvous share)

                            ⇣ for tnls-rendezvous, the file bytes never touch the relay ⇣
        teammate  ◄════════════ BitTorrent swarm (data plane) ════════════►  you
```

The **relay** is deliberately dumb: it pairs a viewer socket to an agent socket by `tunnel_id`
and shuttles **opaque** frames, never holding the signing secret. All enforcement lives at the
**agent** — the only component holding the secret and sitting between the viewer and the real
MCP server. A compromised relay cannot widen scope or extend a TTL; it only moves bytes.

For **tnls-rendezvous**, that split becomes literal: the tunnel is the *control plane* (it gates which
file a teammate may fetch, and for how long) and **BitTorrent is the data plane** — the file
bytes move peer-to-peer and never pass through the relay. The tunnel even *bootstraps* the
swarm: the seeder advertises its peer address through the scoped `request_file` call, so the
downloader connects directly instead of waiting on the BitTorrent DHT.

## Demo — the MCP tunnel

```bash
# 1. start the relay (or just use the live one at wss://tunnel.locker)
cargo run -p relay

# 2. open a 2-minute, read-only tunnel to the sample MCP server (plugin)
cargo run -p tnls -- --ttl 2m --scope read demo serve
#   → prints a link like http://127.0.0.1:8787/t/<id>#<token>

# 3. open the link as a "teammate" in a browser:
#      - call `read` (path: demo/notes.txt)  → succeeds
#      - raw-call `shell` {"cmd":"echo hi"}   → refused, out of scope
#
# 4. let the 2-minute TTL lapse → the link goes dead live,
#    or run `cargo run -p tnls -- close` from a third terminal to revoke on demand.
```

Or just `./demo.sh` (boots the relay and opens the tunnel; Ctrl-C tears down).

## Demo — sending a file (tnls-rendezvous)

```bash
# seed a file and open a scoped link (defaults to --relay wss://tunnel.locker)
cargo run -p tnls -- rendezvous share ./big.mov --ttl 30m
#   → prints https://tunnel.locker/t/<id>#<token>

# on another machine, fetch it over BitTorrent through the scoped link
cargo run -p tnls -- rendezvous get 'https://tunnel.locker/t/<id>#<token>' --out ./downloads
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
| `tnls-core` | Pure core: HMAC token, scope filter, per-call decision, TTL parse, frames. Exhaustively unit-tested, no I/O. |
| `tnls-tunnel` | The tunnel **agent** library: `session::open` (secret, scope, TTL), `McpChild` (rmcp client over TokioChildProcess). |
| `tnls-plugin` | The `describe` manifest contract shared by host + plugins. |
| `tnls` | The host binary (`open`/`close`/`plugins` + plugin dispatch). |
| `plugins/demo` | `tnls-demo`: sample rmcp MCP server (read + shell). Subcommands: `serve` \| `describe`. |
| `plugins/rendezvous` | `tnls-rendezvous`: file sending over BitTorrent (librqbit). Subcommands: `share` \| `get` \| `seed` \| `fetch` \| `describe`. |
| `rendezvous-gui` | `tnls-rendezvous-gui`: native egui front-end driving `tnls rendezvous share`/`get` — send/receive with live progress. |
| `relay` | Rocket WS pairing service (binary: `relay`). In-memory, no state past session. The only deployed component. |
| `viewer/*.html` | Static viewer / landing / stats pages, no build step — `include_str!`'d into the relay binary. |

## Tests

```bash
cargo test --workspace
```

The correctness core (`tnls-core`) is unit-tested exhaustively; `relay` has an in-process WS
pairing test; `tnls-tunnel` has a real-socket end-to-end test proving `read` succeeds and `shell` is
refused (MCP is now `rmcp` on both stdio ends). File-sharing transfer is tested in `tnls-rendezvous`.

## Deploy

Fly.io app `tunnel-locker` (region `iad`). A multi-stage Dockerfile builds **only** the
`relay` binary; the viewer HTML is `include_str!`'d into it, and TLS is terminated by Fly so
the relay speaks plain `ws` internally on `$PORT`. Aggregate-only analytics (no trackers, no
cookies, no stored IPs) persist to a Fly volume.

## Status & future work

`tnls-rendezvous` (BitTorrent core, magnet-through-tunnel, and peer **rendezvous**) is built
and tested, and a native **egui GUI** (`tnls-rendezvous-gui`) wraps send/receive with live
progress. **Next:** NAT hole-punching for cross-internet transfers, payload encryption beyond
the transport (so even the relay operator can't read tool I/O), and multi-viewer sessions with
per-viewer scopes + audit logging.

Design specs and phased implementation plans live in [`docs/superpowers/`](docs/superpowers/).
