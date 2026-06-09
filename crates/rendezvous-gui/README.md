# tnls-rendezvous-gui

A native [egui](https://github.com/emilk/egui) front-end for `tnls rendezvous` — send a
file over a capability-scoped BitTorrent tunnel, or receive one from a link, without
touching the terminal.

```
┌──────────────────────────────┬──────────────────────────────┐
│ Send a file                  │ Receive a file               │
│  drag a file in / type path  │  paste a tunnel link         │
│  TTL [30m]   [ Share ]       │  into [./tnls-downloads]     │
│  → https://…/t/<id>#<token>  │  [ Fetch ]                   │
│  [Copy link] [Stop sharing]  │  ▓▓▓▓▓░░░ 62%  ↓ 4.1 MiB/s   │
└──────────────────────────────┴──────────────────────────────┘
```

## How it works

The GUI drives the `tnls` **libraries in-process** — no CLI shelling, no stdout scraping. A
background multi-thread tokio runtime owns the async work; the egui frame loop reads shared
state behind a mutex and workers request repaints as state changes.

- **Receive** calls `tnls_rendezvous::get::retrieve_magnet` (pull the magnet + advertised
  peers through the scoped tunnel) then `bittorrent::fetch`, and polls the live `FetchHandle`
  for a real progress bar and download speed. Bytes move peer-to-peer; the relay never
  carries them.
- **Send** calls `tnls_tunnel::session::open` directly: the tunnel agent runs *in this
  process*, hands the link back through an `on_link` callback, and revokes when the GUI
  notifies its `shutdown` (running the agent's real teardown — kill the seeder, drop the
  pidfile, kill the link). Stopping revokes new discovery of the magnet; in-flight downloads
  can't be un-sent, by BitTorrent's nature.

All tunnel enforcement — the HMAC capability token, the scope allowlist, the per-call TTL
re-check — lives in `tnls-tunnel`/`tnls-core`, which the GUI calls but does not reimplement.
The GUI never holds the signing secret.

Per the architecture, the send-side agent still spawns the `tnls-rendezvous` binary as the
scoped MCP server child that seeds and serves the magnet over stdio — that child *is* the
design, not a CLI the GUI parses. So the GUI needs to locate that binary (only that one): via
`$TNLS_RENDEZVOUS_BIN`, then a sibling of its own executable (the Cargo `target/<profile>/`
layout), then `$PATH`. If it can't find one the header says so.

## Running

```bash
cargo run -p rendezvous-gui
```

The relay defaults to `wss://tunnel.locker`; change it in the header field to point at a
local `cargo run -p relay` (e.g. `ws://127.0.0.1:8787`) for offline testing.

## Tests

`driver::resolve_in` (binary lookup) is unit-tested. The send-side library path the GUI
relies on — `session::open`'s `on_link` callback and external `shutdown` — is covered by
`tnls-tunnel`'s `on_link_fires_and_external_shutdown_tears_down` integration test; the
receive-side `retrieve_magnet` + `fetch` path is covered by `tnls-rendezvous`'s tests. The
window itself is verified manually — egui needs a display.
