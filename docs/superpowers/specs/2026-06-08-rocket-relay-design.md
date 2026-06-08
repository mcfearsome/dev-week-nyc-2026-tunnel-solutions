# Relay on Rocket — Whole-Relay Rewrite (design spec)

**Date:** 2026-06-08
**Status:** Approved (pre-implementation)
**Scope:** Rewrite the entire `relay` crate from Axum onto **Rocket** (`rocket` 0.5.1 + `rocket_ws`
0.1.1), including the security-critical WebSocket pairing path. One binary, one deploy (unchanged
Fly topology). This is a framework swap **under a fixed security contract** — every relay invariant
in `CLAUDE.md` is preserved and re-proven by the existing tests. Companion to the tnls pluggable-host
spec (`2026-06-08-tnls-pluggable-host-design.md`); the two are independent in product code and share
only one test seam (§9).

---

## 1. Motivation

The relay today is an Axum service that does two unrelated jobs in one binary: it serves the
human-facing **website** (landing, stats pages, the viewer shell) over plain HTTP GET, and it runs
the **WebSocket pairing core** that shuttles opaque frames between a viewer and an agent. The
decision (out of brainstorming) is to move the whole thing — *including* the WS core — onto Rocket,
so the website and the relay share one modern framework. The cost of touching the WS core is that
the move must be proven not to weaken the security contract; this spec makes that proof explicit.

## 2. What changes vs. what ports unchanged

The relay's three modules are not equally coupled to the framework — most of the security-sensitive
logic is already transport-agnostic and moves untouched.

| Module | Coupling today | In the rewrite |
|---|---|---|
| `backplane.rs` (`Backplane` trait, `Local`/`Redis`, pair-by-`tunnel_id`, `Bytes` over mpsc) | none (pure `async_trait` + `Bytes`) | **unchanged** |
| `stats.rs` (counters, salted-IP dedupe, snapshot, flush) | none | **unchanged** |
| `pairing.rs` (`run_side`) | `axum::extract::ws::{Message, WebSocket}` | **re-targeted** to `rocket_ws::Message` (§3) |
| `lib.rs` (router + GET handlers + state) | Axum `Router`, extractors | **rewritten** as a Rocket app (§5) |
| `main.rs` (launch, `$PORT`) | `axum::serve` | **rewritten** Rocket launch (§5) |
| `tests/*` assertions | none (black-box `tokio_tungstenite` client) | **unchanged**; only the launch helper swaps (§8) |

The capacity limits, no-persistence guarantee, and aggregate-only stats all live in the two modules
that port unchanged — which is what makes this rewrite low-risk despite touching the WS core.

## 3. The pairing rewrite — the one security-critical edit

`rocket_ws` provides a `ws::WebSocket` request guard whose handler returns a `ws::Channel`; inside
the channel closure you receive a duplex value implementing `futures::Stream + Sink` of
`rocket_ws::Message`. `run_side`'s logic is **identical** to today — split into sink/stream, run a
writer pump (`Delivery::Frame(bytes)` → `Message::Text`, `Delivery::Close` → break) and a reader
loop (`Message::Text`/`Binary` → `stats.frame_relayed()` + `backplane.forward(id, role, bytes)`,
`Message::Close` → break), then `backplane.deregister`.

**Decision: re-target `run_side` directly to `rocket_ws::Message`,** invoked from inside the channel
closure:

```rust
#[get("/agent/<id>")]
fn agent_ws(id: String, ws: ws::WebSocket, st: &State<AppState>) -> ws::Channel<'static> {
    let ws = ws.config(ws::Config {           // §4 — MANDATORY cap
        max_message_size: Some(1 << 20),
        max_frame_size:   Some(1 << 20),
        ..Default::default()
    });
    let (bp, stats) = (st.backplane.clone(), st.stats.clone());
    ws.channel(move |stream| Box::pin(run_side(bp, stats, id, Role::Agent, stream)))
}
```

We do **not** abstract `run_side` behind a socket trait — the logic is ~50 lines, the project is
committed to Rocket, and a transport-trait would be YAGNI indirection over a single deployment.
`run_side`'s signature changes from `socket: axum::…::WebSocket` to the `rocket_ws` duplex stream;
its body is otherwise a mechanical `Message`-variant swap.

**Teardown is the one place to watch.** `run_side` must call `bp.deregister(&id, role)` on *every*
exit path — normal `Message::Close`, stream end (`None`), and the channel future being
dropped/cancelled on client disconnect — so a half-open pairing never leaks a backplane slot and the
peer always receives its `Delivery::Close`. Axum's `on_upgrade` task + `writer.abort()` give this
today; under `rocket_ws`'s `Channel` model the plan must confirm the same, gated by the existing
`agent_disconnect_closes_viewer` test (pairing.rs:46) passing against Rocket (§8). This is the one
behavioral risk the "mechanical swap" framing slightly underweights.

## 4. The 1 MiB frame cap — verified, and a defaults trap

`rocket_ws::Config` (verified against `docs.rs/rocket_ws` 0.1.1) exposes:

```
write_buffer_size:      usize          (default 128 KiB)
max_write_buffer_size:  usize
max_message_size:       Option<usize>  (default 64 MiB)
max_frame_size:         Option<usize>  (default 16 MiB)
accept_unmasked_frames: bool           (default false)
max_send_queue:         Option<usize>  (deprecated, no-op — ignore)
```

So we get **straight parity** with Axum's `max_message_size`/`max_frame_size` of 1 MiB. **But the
defaults are 64 MiB / 16 MiB — 64× the current guard** — so setting both to `1 << 20` on the
`/agent` and `/viewer` routes is *mandatory*, not optional; omitting it silently regresses the
frame-bomb protection. This is enforced as a gate item (§6.2) with a dedicated test (§8). The
`write_buffer_size` / `max_write_buffer_size` fields are intentionally left at their defaults — the
frame-bomb guard concerns *inbound* message/frame size (what the current Axum cap limits), not the
outbound write buffer.

## 5. Rocket routes, state, `$PORT`, flush

**Routes** (each Axum handler → Rocket, behavior preserved):

| Route | Rocket handler |
|---|---|
| `GET /` | `-> RawHtml<&'static str>` (`include_str!` landing); `stats.page_view(client_ip)` |
| `GET /healthz` | `-> &'static str` `"ok"` |
| `GET /whoami` | `-> String` (the `ClientIp` guard's value) |
| `GET /stats` | `-> RawHtml<&'static str>` (`include_str!` stats.html) |
| `GET /stats.json` | `-> Json<serde_json::Value>` (`stats.snapshot()`) |
| `GET /t/<id>` | `-> RawHtml<&'static str>` (`include_str!` index.html); `stats.tunnel_link_opened()` |
| `GET /agent/<id>` (WS) | `ws::WebSocket` → capped `Config` → `channel(run_side … Role::Agent)` |
| `GET /viewer/<id>` (WS) | same, `Role::Viewer` |
| `GET /report` (WS) | `ws::WebSocket` → channel reads one `Text` → `stats.report(calls, blocks)` |

The viewer HTML stays `include_str!`'d at compile time (the relay binary remains self-contained, no
CWD/file deps) — unchanged from today.

**`client_ip` → a `FromRequest` guard `ClientIp(String)`** reading `Fly-Client-IP`, then
`X-Forwarded-For`, else `"unknown"` — mirroring `lib.rs::client_ip` exactly, but as the single
typed place an IP enters memory before being salted-hashed (it is never stored or logged raw).

**State:** `AppState { backplane: Arc<dyn Backplane>, stats: Arc<Stats> }` registered with Rocket
`.manage(state)` and read via `&State<AppState>` in handlers (Rocket dispatches managed state by
type). The three WS handlers (`/agent`, `/viewer`, `/report`) must clone `backplane`/`stats` out of
`&State<AppState>` *before* moving them into the `'static` channel closure — the closure can't hold
the `&State` borrow across the channel boundary (the §3 sketch does this).

**`$PORT` (a real gotcha):** Fly injects `PORT=8080`, but Rocket reads `ROCKET_PORT` / `ROCKET_ADDRESS`.
So we build a figment explicitly:

```rust
let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
let figment = rocket::Config::figment()
    .merge(("address", "0.0.0.0"))
    .merge(("port", port));
rocket::custom(figment) /* .manage(state).mount(...).attach(flush_fairing).launch().await */;
```

This preserves the current behavior exactly (8080 on Fly, 8787 locally / for the demo).

**Background flush:** the 10-second `stats.flush()` loop becomes an `AdHoc::on_liftoff` fairing, tying
it to the server lifecycle (replacing the `tokio::spawn` in `build_app`). No-op when `STATS_PATH` is
unset, as today.

**Constructors:** `build_app()` / `build_app_with(backplane)` → `build_rocket()` /
`build_rocket_with(backplane)` returning `Rocket<Build>`, so tests can still inject a shared
backplane (what `cross_instance` depends on) and launch an instance on an ephemeral port.

## 6. Invariant-preservation gate

All of the following must hold against the Rocket build; they restate the `CLAUDE.md` relay
invariants as an acceptance checklist:

1. **Opaque pass-through.** `run_side` forwards `Bytes` verbatim between paired sockets; the relay
   never parses, validates, mutates, or routes-on the *content* of tunnel frames (it only counts
   them via `stats.frame_relayed()`). No scope logic, no trust, no token handling enters the
   forwarding path. The `/report` and `/stats.json` paths remain separate from the
   tunnel-forwarding path (a dedicated stats channel, exactly as today).
2. **1 MiB frame cap.** `max_message_size` and `max_frame_size` are both `Some(1 << 20)` on `/agent`
   and `/viewer`; a test asserts a frame > 1 MiB is rejected (closing the 64 MiB-default gap, §4).
3. **No state past the session.** The backplane stays in-memory (`LocalBackplane`) or ephemeral
   Redis pub-sub; no tunnel payload is persisted. Only aggregate `stats` persist to `STATS_PATH`,
   unchanged.
4. **Aggregate-only analytics, no stored IPs.** `stats.rs` ports byte-for-byte (its
   `raw_ip_is_never_stored` test comes along as proof). The `ClientIp` guard hashes in memory only;
   the raw IP is never stored or logged.
5. **Secret never at the relay.** Unchanged by construction — the relay has no token/secret/scope
   code; all enforcement is agent-side. The rewrite adds none.
6. **The existing security tests pass with assertions unchanged** (§8) — the behavioral proof that
   1–5 hold.

## 7. Migration plan

**Order (keeps `cargo test -p relay` green at each step):**

1. Add `rocket` + `rocket_ws` to `relay/Cargo.toml`, drop `axum`. Port `backplane.rs` + `stats.rs`
   **unchanged** (they already compile framework-free).
2. Re-target `run_side` to `rocket_ws::Message`; build `lib.rs`'s Rocket app — routes + `ClientIp`
   guard + managed state + the **capped** WS routes + the flush fairing + `build_rocket[_with]`.
3. Rewrite `main.rs` to launch Rocket via the `$PORT` figment.
4. Swap the test launch helpers (§8) and add the frame-cap test.
5. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`; re-run advisory `cargo audit` over the new dep tree
   (`rocket`, `rocket_ws`, and their transitive deps replacing `axum`).
6. Confirm **no change** to the Dockerfile (`cargo build --release -p relay`) or `fly.toml`
   (1 machine, binds `$PORT`); only the relay's internals moved.

## 8. Testing

The security-critical tests are **black-box**: `tests/pairing.rs` and `tests/cross_instance.rs`
connect with a real `tokio_tungstenite` *client* and assert frame-forwarding / disconnect-closes-peer
/ cross-backplane bridging. The client is framework-agnostic, so:

- **Assertions stay byte-for-byte.** Only the `spawn_relay` / `serve` helpers change: from
  `axum::serve(listener, relay::build_app())` to launching the Rocket instance
  (`build_rocket[_with]` on the ephemeral test port via the figment). `redis_backplane.rs` likewise
  swaps only its launch helper.
- **`stats.rs` units** port unchanged (incl. `raw_ip_is_never_stored`, `unique_visitors_dedupe`,
  `persists_and_reloads`).
- **New test — the frame cap (§4/§6.2):** a client sends a > 1 MiB frame to `/viewer/<id>` and the
  relay rejects/closes rather than forwarding it (the one behavior the Rocket defaults would
  silently regress).

Because the assertions are preserved, a green test run *is* the proof of §6.

## 9. Cross-spec coupling (the shared test seam)

The relay's test-launch constructor is used by the **workspace e2e too** — by **two** test files,
each carrying its own `axum` dev-dep:

1. `crates/tnls/tests/end_to_end.rs:39` — the `share → get` transfer e2e (Spec 1 §8 moves it to
   `plugins/rendezvous/tests`).
2. `crates/tunnel-locker/tests/end_to_end.rs:60` — `read_succeeds_shell_refused`, the **agent-bridge
   e2e** that proves `shell` is refused out-of-scope (Spec 1 §8 moves it to `tnls-tunnel/tests`).

Both call `axum::serve(relay::build_app())`, and both `tnls` and `tunnel-locker` (→ `tnls-tunnel`)
declare an `axum = { version = "0.7", features = ["ws"] }` dev-dep. The two specs are independent in
product code but share this seam, so:

- **Whichever spec lands second updates *both* relocated e2e launch helpers** from
  `axum::serve(relay::build_app())` to launching the Rocket instance (`build_app` → `build_rocket`).
- When the relay is on Rocket, **both** `plugins/rendezvous` **and** `tnls-tunnel` drop their `axum`
  dev-dependency and launch the relay via `build_rocket[_with]`.

This matters because the agent-bridge e2e is itself security-relevant (out-of-scope refusal); its
launch helper must move in lockstep or the workspace won't compile after the second spec lands. No
product-code dependency exists between the specs — only these two launch-helper updates need
sequencing.

## 10. Non-goals

- **Changing any relay behavior, route, or wire format.** This is a pure framework port; the viewer
  HTML, the frame protocol, the analytics endpoints, and the Fly topology are all unchanged.
- **Splitting the website off the relay.** The brainstorming decision was the whole relay on Rocket
  in one binary; a website/relay split is explicitly *not* this spec.
- **Touching the tunnel wire protocol or the agent.** Out of scope (that's the host spec).
- **HTTP/2, TLS in the relay, or new analytics.** TLS stays Fly-terminated; the relay speaks plain
  `ws`/HTTP internally, unchanged.

## 11. Touch points

`crates/relay/Cargo.toml` (`+rocket`, `+rocket_ws`, `−axum`), `crates/relay/src/lib.rs` (Rocket app,
routes, `ClientIp` guard, managed state, flush fairing, `build_rocket[_with]`),
`crates/relay/src/main.rs` (Rocket launch + `$PORT` figment), `crates/relay/src/pairing.rs`
(`run_side` → `rocket_ws::Message`), `crates/relay/tests/{pairing,cross_instance,redis_backplane}.rs`
(launch helpers only + the new frame-cap test), `Cargo.lock`. **Unchanged:**
`crates/relay/src/{backplane.rs, stats.rs}`, `viewer/*.html`, `Dockerfile`, `fly.toml`. The **two**
workspace e2e relay-spawn helpers (`plugins/rendezvous` + `tnls-tunnel`, each dropping its `axum`
dev-dep) update per §9 when the specs are sequenced.
