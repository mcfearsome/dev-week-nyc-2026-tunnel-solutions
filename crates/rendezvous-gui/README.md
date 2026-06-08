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
│  [Copy link] [Stop sharing]  │  ▓▓▓▓▓░░░ 62%                 │
└──────────────────────────────┴──────────────────────────────┘
```

## What it is (and isn't)

This is a **thin driver over the `tnls` host CLI**, deliberately. All tunnel enforcement —
the HMAC capability token, the scope allowlist, the TTL re-check on every call — lives in the
`tnls` host and its agent, which is the security boundary. The GUI never holds the signing
secret and adds no crypto of its own: it just spawns `tnls rendezvous share|get`, reads the
lines they print, and SIGTERMs the child to revoke a share (which runs the host's real
teardown: kill the seeder, drop the pidfile, kill the link).

- **Send** runs `tnls --relay <relay> --ttl <ttl> rendezvous share <file>`, surfaces the
  printed link with a copy button, and keeps the share alive until you hit **Stop sharing**
  (or the TTL lapses). Stopping revokes new discovery of the magnet — in-flight downloads
  can't be un-sent, by BitTorrent's nature.
- **Receive** runs `tnls rendezvous get <link> --out <dir>` and renders its progress as a
  live bar. Bytes move peer-to-peer; the relay never carries them.

## Running

```bash
cargo run -p rendezvous-gui
```

It locates the `tnls` binary via `$TNLS_BIN`, then a sibling of its own executable (the
Cargo `target/<profile>/` layout puts them together), then `$PATH`. If it can't find one it
says so in the header — build the workspace (`cargo build --workspace`) or set `$TNLS_BIN`.

The relay defaults to `wss://tunnel.locker`; change it in the header field to point at a
local `cargo run -p relay` (e.g. `ws://127.0.0.1:8787`) for offline testing.

## Tests

The line parsers (`extract_link`, `parse_progress`, `parse_done`, `tunnel_id_from_link`) are
pure and unit-tested against the exact banner/progress formats the host emits. The window
itself is verified manually — egui needs a display.
