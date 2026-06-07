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
