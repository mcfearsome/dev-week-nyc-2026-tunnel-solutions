# Demo Video Plan — tunnel.solutions

**Target length:** ~2:30 (Devpost sweet spot; hard cap 3:00).
**Tone:** confident, fast, terminal-forward. Let the live product do the talking — minimal slides.
**Thesis to land:** *tnls is a pluggable host for capability-scoped MCP tunnels — scope at the capability level, and every capability (like sending a file over BitTorrent) is just a plugin.*

**Capture setup:** 1440×900 (or 1080p) screen recording. Big terminal font (≥18pt), dark theme. Use the **live** `https://tunnel.locker` so the URL bar is part of the proof. Two terminal panes side-by-side (sharer + teammate) reads great. Record system audio off; do a clean voice-over (VO) pass over the screen capture.

> **⚠️ Command state — read before recording.** This plan uses the **target host/plugin commands** (matching the reframed submission). The codebase is mid-migration; until the refactor lands, record with the **current** equivalents:
>
> | Target (this plan / submission) | Recordable today (on `main`) |
> |---|---|
> | `tnls plugins` | *(no equivalent yet — skip, or narrate over the crate tree)* |
> | `tnls demo serve --scope read` | `tunnel open ./target/debug/mcp-demo --ttl 2m --scope read` |
> | `tnls open ./srv --scope read` | `tunnel open ./srv --ttl 2m --scope read` |
> | `tnls rendezvous share ./f` | `tnls share ./f` |
> | `tnls rendezvous get <link>` | `tnls get <link>` |
> | `tnls close` | `tunnel close` |
>
> Record now with the right column for an accurate take; re-shoot with the left column once the host/plugin migration merges. The on-screen *behavior* (scope demo, file transfer, TTL death) is identical either way.

---

## The arc (5 beats)

| # | Beat | ~time | What it proves |
|---|------|-------|----------------|
| 1 | Hook + problem | 0:00–0:20 | Why the status quo (ngrok/ports) is wrong for MCP |
| 2 | The host + the wedge, live | 0:20–1:10 | A pluggable host; scope a plugin to `read`, `shell` is refused, TTL kills it |
| 3 | Another plugin: files | 1:10–1:50 | `rendezvous`: same host, BitTorrent moves the bytes, not you |
| 4 | Trust model + scale | 1:50–2:15 | One agent enforces; plugins only narrow; live, multi-machine on Redis |
| 5 | Close | 2:15–2:30 | Tagline + URL + repo |

---

## Shot-by-shot storyboard

### Beat 1 — Hook + problem (0:00–0:20)
- **Visual:** the `tunnel.locker` landing hero (the animated terminal reveal — record it *playing*). Then a quick cut to a browser address bar typing `tunnel.locker`.
- **VO:** "Local-first AI keeps your model and data on your machine — until a teammate needs to talk to your running agent. Today that means opening a port, or ngrok-ing your whole machine. Neither lets you say: *exactly these tools, for two minutes, then the door's gone.*"
- **Capture note:** the hero terminal already tells the story — let it finish (`read ✓ ok`, `shell ⊘ refused`).

### Beat 2 — The host + the wedge, live (0:20–1:10) — *the single unbroken take*
- **Visual A (establish the host):** `tnls plugins` → prints the discovered plugins (`demo`, `rendezvous`) and their commands. *(Pre-refactor: skip this and instead `ls crates/` or just open the demo directly.)*
- **VO:** "tnls is a *host* for capability-scoped tunnels. It has plugins — and the simplest is `demo`, a tiny MCP server with `read` and `shell`."
- **Visual B (terminal):** run, on the real relay:
  ```
  tnls demo serve --relay wss://tunnel.locker --ttl 2m --scope read
  ```
  Banner: `handshake ✓ — 2 tools advertised`, `scope read (1 of 2 in scope)`, `link → https://tunnel.locker/t/…#…`.
- **VO:** "I tunnel it, scoped to `read` only. Out goes a disposable link."
- **Visual C (browser — the money shot):** open the link as "the teammate." The viewer shows **only `read`** (no `shell`). Click **call** on `read` (`demo/notes.txt`) → green result. Then in the **raw-call box**, type `shell` and **send** → red **`⊘ refused 'shell' is out of scope`**.
- **VO:** "The teammate sees only what I allowed. `read` works. But try `shell` — even by hand — and it's refused *agent-side*. The server never sees the call. And every call re-checks a TTL…"
- **Visual D:** let the `ttl` tick to 0 (or `tnls close` in a third pane). The viewer flips to **`⏱ link expired — tunnel revoked`**.
- **VO:** "…so when it expires, the link is genuinely dead. Revocation that's real, not cosmetic."

### Beat 3 — Another plugin: files (1:10–1:50)
- **Visual (two panes):**
  - Left: `tnls rendezvous share ./demo-asset.bin --relay wss://tunnel.locker` → prints `https://tunnel.locker/t/…#…`.
  - Right: `tnls rendezvous get 'https://tunnel.locker/t/…#…' --out ./dl` → `magnet acquired — downloading…` → a progress bar to 100% → `done`.
- **VO:** "Same host, different plugin. `rendezvous` sends a *file* — but the bytes move peer-to-peer over BitTorrent. The relay never touches the file, so I don't eat the bandwidth, and the tunnel even hands the downloader the peer address so it connects instantly instead of waiting on the DHT. A new capability is just a new plugin that declares the scope it needs."
- **Capture note:** if a clean cross-machine transfer is flaky to record, record the **hermetic test** (`cargo test -p tnls --test end_to_end`, or post-refactor `-p tnls-rendezvous`) finishing green and show the byte-equality assertion — or narrate over the progress bar.

### Beat 4 — Trust model + scale (1:50–2:15)
- **Visual:** the landing page "how it works" flow (the 4-node diagram with **agent** highlighted), then a terminal showing `fly status -a tunnel-locker` with **two machines, both `started`**.
- **VO:** "There's exactly one security boundary: the host is the agent — it holds the secret and enforces scope and TTL. The relay is deliberately dumb; it moves opaque bytes and holds nothing. Plugins can only ever *narrow* — they never see the secret or the relay. And it's not one box: it runs multi-machine behind a Redis backplane, live in production."
- **Capture note:** optionally flash the `/stats` page (live, privacy-preserving counters) for half a second — "no trackers, aggregate-only."

### Beat 5 — Close (2:15–2:30)
- **Visual:** the landing hero again, URL `tunnel.locker` + the GitHub repo on screen.
- **VO:** "tunnel.solutions — a pluggable host for capability-scoped, ephemeral MCP tunnels. Live at tunnel dot locker. Built in Rust."
- **End card:** `tunnel.locker` · `github.com/mcfearsome/tunnel-solutions`

---

## Pre-record checklist
- [ ] Relay is live + healthy: `curl https://tunnel.locker/healthz` → `ok` (it is).
- [ ] Build the binaries you'll demo with **no first-run compile lag on camera** — current: `cargo build --release -p tunnel-locker -p mcp-demo -p tnls`; post-refactor: `cargo build --release --workspace`.
- [ ] A real file to share for Beat 3 (a few MB so the progress bar is visible but the transfer is quick); for a guaranteed-clean transfer on one machine, pre-seed and fetch with `--out`.
- [ ] Terminal: large font, dark theme, short prompt. Browser: no extensions/bookmarks bar, clean profile.
- [ ] Have the link copy-paste ready between panes (the token is long — don't type it on camera).
- [ ] Confirm which command set you're recording (see the **Command state** table) and dry-run Beat 2 once so the TTL timing feels deliberate.

## Reusable assets (already captured, in `docs/devpost/media/`)
- `01-landing-hero.png` — title/thumbnail shot.
- `02-landing-full.png` — full landing page (story at a glance).
- `03-stats.png` — live analytics page.
- `04-viewer-scope-enforcement.png` — the money shot (`read ✓`, `shell ⊘ refused`), captured live.

## Optional 30-second teaser cut
Hero animation (0–5s) → Beat 2 Visual C only: `read ✓` then `shell ⊘ refused` (5–22s) → end card (22–30s). VO: one line — "A pluggable host for capability-scoped MCP tunnels: share exactly the tools you allow, for exactly as long as you allow."
