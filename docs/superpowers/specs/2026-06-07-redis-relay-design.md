# Redis Multi-Machine Relay (design spec)

**Date:** 2026-06-07
**Status:** Approved (pre-implementation)
**Scope:** Remove the relay's single-machine constraint by pairing tunnels across instances via
a Redis backplane, behind a pluggable abstraction that preserves today's zero-dependency local
behavior.

## 1. Problem

The relay keeps the tunnel registry (`tunnel_id → paired sockets`) in process memory, so an
agent and viewer must land on the **same** machine — which forced `fly scale count 1`. To run
multiple instances behind Fly's load balancer, sockets that landed on different instances must
still pair.

## 2. Keystone: a pluggable `Backplane`

Introduce a `Backplane` trait with two implementations, selected at startup by the `REDIS_URL`
env var:

- **`LocalBackplane`** (no `REDIS_URL`) — the current in-memory, single-instance behavior.
  Local dev, every existing test, and the demo keep working **unchanged**.
- **`RedisBackplane`** (`REDIS_URL` set) — cross-instance pairing for production scale.

`build_app` takes an `Arc<dyn Backplane>` (injectable). `main.rs` builds Local or Redis from
env. This makes the change additive and testable rather than a core rewrite. The relay stays
"dumb" (it still never parses our protocol, holds no secret); only *where the pairing state
lives* changes.

## 3. Backplane interface

The trait boundary uses **serialized frames** (`bytes::Bytes`), not Axum's `ws::Message` (which
can't cross Redis). `run_side` converts an inbound `Message::Text`/`Binary` → `Bytes` before
`forward`, and converts a delivered `Bytes` back into `Message::Text` for its own socket.

```
type Tx = mpsc::UnboundedSender<Delivery>;          // delivers TO a local socket
enum Delivery { Frame(Bytes), Close }               // Close = peer gone → this side shuts down

register(tunnel_id, role, local_tx: Tx) -> Result<(), Occupied>
    Claim `role` for `tunnel_id` GLOBALLY. `local_tx` is how the backplane hands frames (and
    the Close signal) to THIS side's socket. `Occupied` iff the role is currently held
    anywhere; a role whose presence has expired/cleared counts as free.
forward(tunnel_id, from_role, frame: Bytes)
    Deliver to the PEER role — local fast-path if the peer's `local_tx` is on this instance,
    else route to wherever the peer is. No peer present yet → drop (single agent/viewer).
deregister(tunnel_id, role)
    Release this side and push `Delivery::Close` to the peer's `local_tx` (cascade teardown).
```

`run_side` (in `pairing.rs`) becomes: `register` (reject on `Occupied`); a **writer pump**
draining `local_rx` — `Frame(b)` → `sink.send(Message::Text(b))`, `Close` → close the sink and
exit; a **reader loop** converting each inbound socket message to `Bytes` and calling
`forward`; `deregister` on exit. The viewer↔agent **frame contents are untouched** — only the
transport wrapper changes.

**Lifecycle:** `register`/`deregister` are per-role. The tunnel's shared entry is cleaned up
once both roles are gone. Re-registering a role whose peer is still present is allowed (the new
side pairs with the survivor); a role whose own presence is still live returns `Occupied`. This
generalizes today's all-or-nothing `map.remove` to per-role across instances.

## 4. RedisBackplane mechanics

**Connection model:** each instance holds (a) a pooled command connection for
`SET`/`DEL`/`PUBLISH`, and (b) **one shared subscriber task** on its own dedicated connection
(`SUBSCRIBE` monopolizes a connection, so it can't share the command pool). The subscriber owns
a `HashMap<channel, Tx>` dispatch table: on each Pub/Sub message it looks up the channel and
hands the payload to the matching `local_tx`. This is one subscriber per instance multiplexing
all of that instance's tunnels — not a connection per tunnel.

Per tunnel, per direction channels (`tunnel:<id>:to_agent` / `to_viewer`) + presence keys:

- **Register:** `SET tunnel:<id>:<role> = <instance_id> NX EX <ttl>` — `NX` = **global
  duplicate-role rejection**; `EX` + a heartbeat refreshing the key = **crash cleanup** (a
  dead instance's presence expires, freeing the role). Then insert `tunnel:<id>:to_<role> →
  local_tx` into the dispatch table and `SUBSCRIBE tunnel:<id>:to_<role>`.
- **Forward(frame):** peer `local_tx` in the local map → deliver `Delivery::Frame(bytes)`
  directly (**same-instance fast path**, no Redis). Else `PUBLISH tunnel:<id>:to_<peer_role>` a
  *frame*-tagged message.
- **Deregister:** `DEL tunnel:<id>:<role>`; deliver `Delivery::Close` to the peer — locally if
  present, else `PUBLISH tunnel:<id>:to_<peer_role>` a *close*-tagged message; remove the
  dispatch entry and `UNSUBSCRIBE`.

**Wire tag:** published payloads carry a tag distinguishing `frame` from `close` (e.g. a JSON
envelope `{"k":"f","b":"<frame text>"}` / `{"k":"c"}`). The subscriber maps `frame → Frame`,
`close → Close` onto the local `Tx`, so cross-instance teardown reproduces the local cascade
exactly (peer's writer pump receives `Close` → closes its socket).

Only small control-plane frames cross Redis; bulk file data never touches the relay. Cross-
instance latency is one Redis hop per frame; same-instance traffic skips Redis entirely.

## 5. Infra

- Client: the `redis` crate (async/tokio, pub/sub via a dedicated connection).
- Target: **Fly Upstash Redis**, provisioned and attached to the app (`REDIS_URL` secret).
- `fly.toml`: drop the implicit `count = 1` — scale to N machines (e.g. 2). `min_machines_running`
  ≥ 1; `auto_stop` may stay off for warm pairing. The single-machine constraint is gone.

## 6. Testing — the trait pays off

- **Cross-instance pairing, hermetic, no real Redis:** construct **one** shared
  `Arc<LocalBackplane>` and pass it to **two** in-process `build_app` instances on two ephemeral
  ports. Connect the agent to app A, the viewer to app B; assert a frame sent by the viewer
  arrives at the agent (and back). The shared in-memory backplane stands in for Redis,
  exercising the exact cross-instance code path.
- **Duplicate-role rejection:** a second agent for the same tunnel (on either app) is refused.
- **Existing relay pairing test** (single `LocalBackplane`) keeps passing — backward compat.
- `RedisBackplane` against a real Redis is an optional `#[ignore]` integration test (run when a
  `REDIS_URL` is available); the in-memory test is the gate.

## 7. Touch points

`relay/src/backplane.rs` (new: trait + `LocalBackplane` + `RedisBackplane`), `relay/src/pairing.rs`
(`run_side` routes through the backplane), `relay/src/lib.rs` + `main.rs` (`build_app(Arc<dyn
Backplane>)`; select impl from `REDIS_URL`), `relay/Cargo.toml` (`redis`), `fly.toml` (scale-out
+ Redis), deploy steps (provision Upstash, set `REDIS_URL`).

## 8. Backward compatibility

No `REDIS_URL` → `LocalBackplane` → byte-for-byte today's behavior (single instance, in-memory,
zero external deps). The Phase 2 / rendezvous local demos and tests are unaffected. Redis is a
production opt-in.

## 9. Non-goals

Sticky-routing alternative (`fly-replay`); multi-viewer per tunnel; persisting tunnels across
relay restarts; sharding Redis; authenticating Redis beyond the connection secret;
back-pressure/flow-control across the backplane (frames are small and infrequent).
