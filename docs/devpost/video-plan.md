# Demo Video Plan — tunnel.solutions

**Target length:** ~2:30 (Devpost sweet spot; hard cap 3:00).
**Tone:** confident, fast, terminal-forward. Let the live product do the talking — minimal slides.
**Thesis to land:** *we built a tunnel that understands the protocol it carries, so it scopes at the capability level — and the same primitive sends files over BitTorrent and runs multi-machine in production.*

**Capture setup:** 1440×900 (or 1080p) screen recording. Big terminal font (≥18pt), dark theme. Use the **live** `https://tunnel.locker` so the URL bar is part of the proof. Two terminal panes side-by-side (sharer + teammate) reads great. Record system audio off; do a clean voice-over (VO) pass over the screen capture.

---

## The arc (5 beats)

| # | Beat | ~time | What it proves |
|---|------|-------|----------------|
| 1 | Hook + problem | 0:00–0:20 | Why the status quo (ngrok/ports) is wrong for MCP |
| 2 | The wedge, live | 0:20–1:05 | Capability scoping: `read` works, `shell` is refused, TTL kills it |
| 3 | Same primitive → files | 1:05–1:45 | `tnls`: scoped link, BitTorrent moves the bytes, not you |
| 4 | Trust model + scale | 1:45–2:15 | Dumb relay / smart agent; live, multi-machine on Redis |
| 5 | Close | 2:15–2:30 | Tagline + URL + repo |

---

## Shot-by-shot storyboard

### Beat 1 — Hook + problem (0:00–0:20)
- **Visual:** the `tunnel.locker` landing hero (the animated terminal reveal — record it *playing*, don't freeze it). Then a quick cut to a browser address bar typing `tunnel.locker`.
- **VO:** "Local-first AI keeps your model and data on your machine — until a teammate needs to talk to your running agent. Today that means opening a port, or ngrok-ing your whole machine. Neither lets you say: *exactly these tools, for two minutes, then the door's gone.*"
- **Capture note:** the hero's terminal animation already tells the story — let it finish (`read ✓ ok`, `shell ⊘ refused`).

### Beat 2 — The wedge, live (0:20–1:05) — *the single unbroken take*
- **Visual A (terminal):** type and run, on the real relay:
  ```
  tunnel open ./mcp-demo --relay wss://tunnel.locker --ttl 2m --scope read
  ```
  Banner appears: `handshake ✓ — 2 tools advertised`, `scope read (1 of 2 in scope)`, `link → https://tunnel.locker/t/…#…`.
- **VO:** "One command wraps my local MCP server. It advertises two tools — but I'm only sharing `read`. Out goes a disposable link."
- **Visual B (browser — the money shot):** open the link as "the teammate." The viewer shows **only `read`** (no `shell`). Click **call** on `read` (`demo/notes.txt`) → green result. Then in the **raw-call box**, type `shell` and **send** → red **`⊘ refused 'shell' is out of scope`**.
- **VO:** "The teammate sees only what I allowed. `read` works. But try `shell` — even by hand — and it's refused *agent-side*. The server never even sees the call. And every call re-checks a TTL…"
- **Visual C:** let the `ttl` counter tick to 0 (or cut to a third terminal and run `tunnel close`). The viewer flips to **`⏱ link expired — tunnel revoked`** / **`× link closed`**.
- **VO:** "…so when it expires, the link is genuinely dead. Revocation that's real, not cosmetic."

### Beat 3 — Same primitive → files (1:05–1:45)
- **Visual (two panes):**
  - Left: `tnls share ./demo-asset.bin --relay wss://tunnel.locker` → prints `https://tunnel.locker/t/…#…`.
  - Right: `tnls get 'https://tunnel.locker/t/…#…' --out ./dl` → `magnet acquired — downloading…` → a progress bar climbing to 100% → `done`.
- **VO:** "Here's the twist: the same capability tunnel sends *files*. `tnls share` opens a scoped link — but the bytes move peer-to-peer over BitTorrent. The relay never touches the file, so I don't eat the bandwidth, and the tunnel even hands the downloader the peer address so it connects instantly instead of waiting on the DHT."
- **Capture note:** if a clean cross-machine transfer is flaky to record, record the **hermetic test** (`cargo test -p tnls --test end_to_end`) finishing green and show the byte-equality assertion — or just narrate over the progress bar.

### Beat 4 — Trust model + scale (1:45–2:15)
- **Visual:** the landing page "how it works" flow (the 4-node diagram with **agent** highlighted), then a terminal showing `fly status -a tunnel-locker` with **two machines, both `started`**.
- **VO:** "The relay is deliberately dumb — it pairs sockets and moves opaque bytes, holding no secret and no state. *All* enforcement lives at the agent, so a compromised relay can't widen scope or extend a TTL. And it's not one box: it runs multi-machine behind a Redis backplane — live, in production, scaling horizontally."
- **Capture note:** optionally flash the `/stats` page (live, privacy-preserving counters) for half a second — "no trackers, aggregate-only."

### Beat 5 — Close (2:15–2:30)
- **Visual:** the landing hero again, URL `tunnel.locker` + the GitHub repo on screen.
- **VO:** "tunnel.solutions — a capability-scoped, ephemeral tunnel that understands the protocol it carries. Live at tunnel dot locker. Built in Rust."
- **End card:** `tunnel.locker` · `github.com/mcfearsome/tunnel-solutions`

---

## Pre-record checklist
- [ ] Relay is live + healthy: `curl https://tunnel.locker/healthz` → `ok` (it is).
- [ ] `cargo build --release -p tunnel-locker -p mcp-demo -p tnls` (no first-run compile lag on camera).
- [ ] A real file to share for Beat 3 (a few MB so the progress bar is visible but the transfer is quick); for a guaranteed-clean transfer on one machine, you can pre-seed and fetch with `--out`.
- [ ] Terminal: large font, dark theme, prompt cleaned up (short `$`). Browser: no extensions/bookmarks bar, clean profile.
- [ ] Have the link copy-paste ready between panes (the token is long — don't type it on camera).
- [ ] Do one dry run of Beat 2 end-to-end so the TTL timing feels deliberate.

## Reusable assets (already captured, in `docs/devpost/media/`)
- `01-landing-hero.png` — title/thumbnail shot.
- `02-landing-full.png` — full landing page (story at a glance).
- `03-stats.png` — live analytics page.
- `04-viewer-scope-enforcement.png` — the money shot (`read ✓`, `shell ⊘ refused`), captured live.

## Optional 30-second teaser cut
Hero animation (0–5s) → Beat 2 Visual B only: `read ✓` then `shell ⊘ refused` (5–22s) → end card (22–30s). VO: one line — "A tunnel that understands the protocol it carries: share exactly the tools you allow, for exactly as long as you allow."
