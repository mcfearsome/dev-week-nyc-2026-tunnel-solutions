## Inspiration

Local-first AI tooling is having a moment. MCP (the Model Context Protocol) lets you run agents and tools right on your own machine, keeping the model and your data local. That's great for privacy — until a teammate needs to actually talk to your running agent. Today your options are bad: open an inbound port, stand up ngrok and expose *everything*, or copy-paste results around. None of them let you say "you can use exactly these two tools, for the next two minutes, and then this door disappears."

We built tunnel.solutions to close that gap — and then realized the primitive we'd built (a tunnel that *understands the protocol it carries* and scopes at the capability level) generalizes well beyond MCP tools. So we used it to solve a second, gnarlier problem: sending a large file to a teammate without eating the upload bandwidth or handing out an un-revocable link. That second product, **tnls**, sends files over BitTorrent gated by the same capability tunnel.

## What it does

Two products, one capability-scoped, ephemeral primitive. The relay is **live at https://tunnel.locker**.

**1. tunnel.locker — a scoped tunnel to your local MCP server (the `tunnel` CLI).**
Wrap your local MCP server with one command, hand a teammate a link, and they get a viewer that lists only the tools you've allowed. Everything else stays invisible *and* unreachable.

- **MCP-aware scoping**: we intercept the MCP `tools/list` response and drop any tool not in your `--scope` allowlist. Out-of-scope `tools/call` requests are rejected agent-side with a clear JSON-RPC error — the server never even sees them.
- **Real revocation**: a TTL is enforced on *every* call, not just at handshake. When the clock runs out, the link is dead.
- **Ephemeral by design**: nothing is persisted. The relay holds no state past the session; closing the tunnel tears down the pairing instantly.
- **No inbound ports**: the agent dials *outbound* to the relay over a multiplexed WebSocket, so you never expose a listening port.

**2. tnls — capability-scoped file *sending* over BitTorrent (the `tnls` CLI), built on top of the tunnel.**
`tnls share ./big.mov` seeds the file and opens a scoped `tunnel.locker` link. A teammate runs `tnls get <link>` and downloads it — but the bytes move **peer-to-peer over BitTorrent**, never through our relay. The tunnel is the *control plane* (it gates *which* file a teammate may fetch, and for *how long*); BitTorrent is the *data plane*. So you don't pay for the bandwidth, multiple downloaders share the load, and "revocation" still means something: cut the link and no new peer can discover the magnet.

The clever part: the tunnel doesn't just gate access, it **bootstraps the swarm**. The seeder advertises its own peer address through the scoped `request_file` call, so the downloader connects *directly* instead of waiting on the BitTorrent DHT — the transfer starts immediately.

## How we built it

A Rust Cargo workspace, four crates plus a static viewer:

- **`tunnel-locker-core`** — the pure, I/O-free heart: the HMAC capability token, the scope filter, the per-call enforcement decision, TTL parsing, and the wire frames. Exhaustively unit-tested with zero network or process dependencies, so the part that must be *correct* is proven before any socket exists.
- **`relay`** (Axum) — pairs a viewer WebSocket to an agent WebSocket by `tunnel_id` and shuttles **opaque** frames. Deliberately dumb: it never holds the signing secret and (originally) kept no state past the session. It's the only deployed component; it also serves the landing page and privacy-preserving aggregate analytics.
- **`tunnel-locker`** (the `tunnel` CLI) — spawns the local MCP server as a child, speaks MCP JSON-RPC over its stdio with id-correlated request/response, mints the capability token, dials the relay, and is the **sole enforcement point** for scope + TTL.
- **`tnls`** (the `tnls` CLI) — the file sharer. Uses `librqbit` (a pure-Rust BitTorrent library) to create/seed a torrent and fetch from a magnet. It exposes an *on-demand MCP server* (`tnls mcp-serve`) with `list_shares`/`request_file` tools, and `tnls share` simply points the existing `tunnel` agent at it — so the file sharer is *just another MCP server behind the same tunnel*, no tunnel changes required. `tnls get` is a headless viewer that speaks the relay's viewer protocol to retrieve the magnet, then downloads it.

The capability token is HMAC-SHA256 over `{ tunnel_id, scope, exp }`, generated per session, in memory only — the only credential, no accounts. It rides in the link's URL **fragment**, so it never reaches the relay in an HTTP request line. Built with tokio, axum, tokio-tungstenite, serde, hmac/sha2, and librqbit. Deployed on **Fly.io** (region `iad`); a multi-stage Dockerfile builds only the relay binary, the viewer HTML is `include_str!`'d into it, and TLS is terminated by Fly.

**The active refactor: horizontal scale via a Redis backplane.** The relay's in-memory pairing meant a viewer and agent had to land on the *same* machine — so we were pinned to one. We're lifting that with a pluggable `Backplane` trait: `LocalBackplane` (today's in-memory behavior, zero deps) and `RedisBackplane` (cross-instance pairing via Redis pub/sub + presence keys). The control-plane frames cross Redis; same-instance traffic short-circuits. Both implementations pass the *same* cross-instance pairing test — so we proved sockets on different machines can pair **before provisioning any Redis at all**, then verified the Redis path against a real `redis-server`. It's rolling out to production now (rustls TLS to managed Redis, with a connect-timeout that degrades to single-instance rather than ever taking the relay down).

## Challenges we ran into

- **Making revocation real, not cosmetic.** It's easy to check a token at handshake and forget about it; we re-check `exp` on *every* `tools/call` so an expired tunnel genuinely stops working mid-session.
- **Bridging stdio JSON-RPC into relay frames** while transparently filtering the tool list in flight, without breaking the MCP protocol — id-correlation was required because notifications interleave with responses.
- **The BitTorrent NAT/discovery problem.** A fresh swarm on the same host or LAN often never connects via DHT in a useful window. Our fix — having the *tunnel* introduce the peers — turned "magnet retrieved but stuck at 0%" into an immediate transfer.
- **Scaling a deliberately-dumb relay without making it smart.** The whole security story rests on the relay being a dumb byte-mover. Adding Redis without leaking state or protocol-awareness into it meant putting the change *behind a trait* and keeping the forwarding path opaque.
- **Real-world deploy gremlins:** `librqbit` ships only as a `9.0.0-rc.0` pre-release (caret versions won't resolve it); the Docker build context ballooned to gigabytes until we deployed from a clean `git archive`; and connecting to managed Redis required adding rustls TLS with *bundled* roots because the slim runtime container has no system CA store.

## Accomplishments that we're proud of

- The MCP demo runs as a **single unbroken take**: open a tunnel scoped to `read`, watch a teammate call `read` successfully but get refused on `shell`, then watch the link die live when the TTL expires.
- `tnls` actually **moves a file over BitTorrent through a scoped link** — verified by an end-to-end test that asserts byte-for-byte equality, with the transfer brokered entirely by the capability tunnel.
- It's **deployed and live** at `tunnel.locker`, with a polished landing page and privacy-preserving, aggregate-only analytics (no trackers, no cookies, no stored IPs).
- The Redis backplane is **proven cross-instance by the same test against two different backends** (in-memory and real Redis) — the trait abstraction let us de-risk the scariest part (cross-machine pairing) before spending a cent on infra.
- All of it is Rust, tested, and built through a disciplined spec → plan → review pipeline.

## What we learned

The interesting wedge in tunneling isn't moving bytes — ngrok, Cloudflare Tunnel, and frp already do that beautifully. The novelty is that the far end **understands the protocol it's tunneling**, so you can scope at the *capability* level instead of the *network* level. Once we had that primitive, the second lesson followed: a capability control plane can **bootstrap a separate data plane** — the same tunnel that authorizes a file fetch can also hand over the peer address that makes the BitTorrent transfer connect instantly. And on the engineering side: designing a clean *seam* (the `Backplane` trait) let us prove a risky distributed-systems change with a hermetic test before touching production infrastructure.

## What's next for tunnel.solutions

- Finish the **multi-machine relay** rollout (Redis backplane) so the service scales horizontally with zero single-machine bottleneck.
- **NAT hole-punching** so cross-internet file transfers connect as reliably as same-LAN ones, plus a quick native GUI (egui) wrapping `tnls` share/fetch with progress.
- **Payload encryption beyond the transport**, so even the relay operator can't read tool I/O.
- **Multi-viewer sessions** with per-viewer scopes, reconnection logic, and optional audit logging for teams that need a record of which scoped tools were invoked during a session.
