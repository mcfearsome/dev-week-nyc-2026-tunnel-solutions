# tunnel.locker — Traffic Analytics Design

**Date:** 2026-06-07
**Status:** Approved (pre-implementation)
**Scope:** A landing page + privacy-preserving, server-side traffic/usage analytics on the
deployed relay, so the project has real visitor + usage numbers for the hackathon submission.

## Principle

No third-party trackers, no cookies, no stored IPs — consistent with the product's privacy
thesis. The relay instruments itself; aggregate counts only. "Zero third-party tracking" is a
submission talking point, not a compromise.

## 1. Landing page (`GET /`)

Terminal-styled static HTML (matches the viewer), embedded via `include_str!`. Content: the
wedge ("why this isn't ngrok"), the `tunnel open … --scope read` demo snippet + banner,
`cargo install tunnel-locker`, a GitHub link, and a **live stats teaser** fetched from
`/stats.json`. New file `viewer/landing.html` (served by the relay).

## 2. Relay counters (`crates/relay/src/stats.rs`)

A `Stats` value in `AppState` (`Arc<Stats>`). Counters (relay-observable — no frame parsing,
dumb-relay invariant preserved):

| Counter | Incremented when |
|---------|------------------|
| `page_views` | `GET /` |
| `unique_visitors` | distinct `hash(Fly-Client-IP, secret_salt)` ever seen (see below) |
| `tunnel_links_opened` | `GET /t/:id` |
| `tunnels_opened` | an agent WS registers a new tunnel |
| `frames_relayed` | each Text/Binary frame forwarded in `run_side` |
| `tool_calls` | agent-reported (see §5) |
| `out_of_scope_blocks` | agent-reported (see §5) |

**Privacy-preserving uniques:** a per-deployment random `secret_salt` (generated once,
persisted). On a landing visit, compute `h = siphash(Fly-Client-IP, secret_salt)` and insert
into a persisted `HashSet<u64>`; `unique_visitors = set.len()`. **The raw IP is never stored**
— only a one-way hash, and the salt is secret to this deployment. Cumulative + restart-durable.

Simple counters are `AtomicU64`; the unique-set + salt live behind a `Mutex`.

## 3. Endpoints

- `GET /stats.json` — snapshot of all counters as JSON.
- `GET /stats` — small terminal-styled HTML page rendering the numbers (auto-refresh), public
  and screenshot-ready.
- `GET /` — the landing page (§1).

## 4. Persistence (Fly volume)

Aggregate counts only — consistent with "nothing per-session survives" (no tool I/O, no
session data, no IPs). The relay loads `STATS_PATH` (default unset → in-memory only for local
dev) on boot and flushes JSON there every ~10s via a background task (and best-effort on
shutdown). On Fly: a 1 GB volume mounted at `/data`, `STATS_PATH=/data/stats.json`.

Persisted shape: `{ salt, page_views, unique_hashes: [u64], tunnel_links_opened,
tunnels_opened, frames_relayed, tool_calls, out_of_scope_blocks }`.

## 5. Agent-reported usage (`tool_calls` / `out_of_scope_blocks`)

The relay can't see these without parsing tunnel frames (which would break its dumb/trustless
design), so the **agent** reports aggregate integers — no content. Implementation reuses the
existing WS+TLS stack (no new HTTP-client dependency):

- Agent tracks `calls` (Forward decisions) and `blocks` (out-of-scope refusals) during a session.
- On shutdown (TTL / close / signal), best-effort connect to `<relay>/report/<id>` (WS), send
  one JSON message `{ "calls": N, "blocks": M }`, close. Short timeout; failures ignored.
- Relay `GET /report/:id` (WS) reads one message and adds to the `tool_calls` /
  `out_of_scope_blocks` counters. This is a dedicated stats path, NOT the opaque-forwarding
  path — the dumb-relay invariant for tunnel content is untouched.

Documented as aggregate-only agent telemetry.

## 6. Testing

- Unit-test `Stats`: counter increments, the hashed-unique dedupe (same IP+salt → one unique;
  different IP → two), JSON snapshot round-trip, load-from-persisted.
- Verify live: `curl https://tunnel-locker.fly.dev/stats.json` before/after a visit + a tunnel,
  confirm increments; confirm `/` and `/stats` render.

## 7. Deploy

`fly volumes create stats_data --size 1 --region iad`, add the mount + `STATS_PATH` to
`fly.toml`, redeploy, verify. Single machine already pinned (volume binds to it).

## 8. Non-goals

Geo/referrer/per-visitor breakdowns (that's what hosted analytics is for — skipped to stay
tracker-free), bot filtering beyond the unique-IP hash, real-time streaming.
