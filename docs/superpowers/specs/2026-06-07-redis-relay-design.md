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

## 3. Backplane interface (semantics; exact signatures pinned in the plan)

```
register(tunnel_id, role, local_tx) -> Result<(), Occupied>
    Record that `role` for `tunnel_id` is served here; `local_tx` delivers frames TO this
    side's socket. Returns Occupied if the role is already taken for this tunnel (globally).
forward(tunnel_id, from_role, msg)
    Deliver `msg` to the PEER role — local fast-path if the peer is on this instance, else
    hand it to the peer wherever it is.
deregister(tunnel_id, role)
    Drop this side and signal the peer to close (cascade teardown).
```

`run_side` (in `pairing.rs`) is rewritten to: `register` (reject on `Occupied`), spawn the
writer pump draining `local_rx → socket` (unchanged), reader loop → `forward(...)` per inbound
frame, and `deregister` on exit. The viewer↔agent **frame contents are untouched**.

## 4. RedisBackplane mechanics

Per tunnel, per direction pub/sub channels and presence keys:

- **Register:** `SET tunnel:<id>:<role> = <instance_id> NX EX <ttl>` — `NX` gives **global
  duplicate-role rejection**; `EX`+a heartbeat task give **crash cleanup** (a dead instance's
  presence expires). Subscribe to `tunnel:<id>:to_<role>`; on a published frame, send it to
  `local_tx` (→ the socket).
- **Forward:** if the peer's `local_tx` is in this instance's local map → send directly (**same-
  instance fast path**, no Redis hop). Else `PUBLISH tunnel:<id>:to_<peer_role>` the frame.
- **Deregister:** `DEL tunnel:<id>:<role>`, `PUBLISH tunnel:<id>:to_<peer_role>` a close
  sentinel (→ peer closes), `UNSUBSCRIBE`.

Only small control-plane JSON frames cross Redis; bulk file data never touches the relay.
Cross-instance latency is one Redis hop per frame (acceptable for the control plane);
same-instance traffic skips Redis entirely.

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
