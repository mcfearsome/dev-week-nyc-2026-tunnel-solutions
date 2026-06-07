# tnls Phase 1 — BitTorrent Core Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new `tnls` crate that can create+seed a torrent from a local file and fetch it from a magnet over BitTorrent (librqbit), with progress — proven hermetically by a loopback seed↔fetch test. No tunnel yet (Phase 2).

**Architecture:** `crates/tnls/src/bittorrent.rs` wraps librqbit: `create_share(path)` makes the torrent + magnet; `seed(meta, data_dir, opts)` seeds existing data; `fetch(magnet, out_dir, opts)` downloads and exposes `Progress`. Both take an opts struct so the real CLI uses DHT+UPnP+a public tracker while the test uses DHT-off + `initial_peers` for a hermetic loopback transfer. A minimal `tnls seed` / `tnls fetch` CLI exercises it end-to-end.

**Tech Stack:** Rust, `librqbit` (9.x), `tokio`, `anyhow`, `clap`, `urlencoding`.

**Spec:** `docs/superpowers/specs/2026-06-07-tnls-file-sharer-design.md` (§4, §10 Phase 1, §12). Read it first.

**librqbit API reference (pinned from docs.rs — use exactly this):**
- `librqbit::create_torrent(path: &Path, CreateTorrentOptions { name, piece_length, trackers }, spawner: &BlockingSpawner) -> anyhow::Result<CreateTorrentResult>` — **3 args**. `BlockingSpawner` via `librqbit::spawn_utils::BlockingSpawner::default()` (if `spawn_utils` isn't visible at first build, add `librqbit-core = "=9.0.0-rc.0"` and import from there).
- `CreateTorrentResult::info_hash() -> librqbit::Id20`, `::as_bytes() -> anyhow::Result<bytes::Bytes>`
- `librqbit::Session::new_with_opts(out_dir: PathBuf, SessionOptions { disable_dht, disable_dht_persistence, persistence: Option<_>, listen_port_range: Option<Range<u16>>, enable_upnp_port_forwarding, ..Default::default() }) -> anyhow::Result<Arc<Session>>` (and `Session::new(dir).await` for all-defaults).
- `session.add_torrent(AddTorrent::from_url(magnet) | AddTorrent::from_bytes(bytes), Some(AddTorrentOptions { output_folder: Option<String>, overwrite: bool, initial_peers: Option<Vec<SocketAddr>>, ..Default::default() })) -> anyhow::Result<AddTorrentResponse>`; `.into_handle() -> Option<ManagedTorrentHandle>`.
- `.into_handle() -> Option<std::sync::Arc<librqbit::ManagedTorrent>>` — the exported type is **`ManagedTorrent`** (use `Arc<ManagedTorrent>`); `ManagedTorrentHandle` is NOT exported.
- `handle.wait_until_completed().await -> anyhow::Result<()>`; `handle.stats() -> TorrentStats { progress_bytes: u64, total_bytes: u64, uploaded_bytes: u64, finished: bool, live: Option<LiveStats>, .. }`; `LiveStats { download_speed: Speed, upload_speed: Speed, snapshot: StatsSnapshot, .. }` (`Speed.mbps: f64`).
- `Id20` has **no `Display`** — get the 40-char lowercase hex via `info_hash.as_string()`.

---

## File Structure

| Path | Responsibility |
|------|----------------|
| `crates/tnls/Cargo.toml` | Crate manifest (Phase 1 deps only). |
| `crates/tnls/src/main.rs` | clap dispatch: `seed` / `fetch` (Phase 1 dev commands). |
| `crates/tnls/src/cli.rs` | clap arg definitions. |
| `crates/tnls/src/bittorrent.rs` | `ShareMeta`, `create_share`, `seed`, `fetch`, `Progress`, handles. The whole data plane. |
| `crates/tnls/tests/loopback.rs` | Hermetic seed↔fetch integration test. |

`bittorrent.rs` is the one load-bearing file; keep it focused on the librqbit wrapping.

---

## Chunk 1: Phase 1 — BitTorrent core

### Task 1: Scaffold the `tnls` crate

**Files:**
- Create: `crates/tnls/Cargo.toml`, `crates/tnls/src/main.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Add the crate to the workspace**

In the root `Cargo.toml`, add `"crates/tnls"` to `[workspace].members`.

- [ ] **Step 2: Write `crates/tnls/Cargo.toml`**

```toml
[package]
name = "tnls"
version = "0.1.0"
edition = "2021"

[dependencies]
librqbit = "=9.0.0-rc.0"   # only a pre-release exists; caret "9" won't resolve it
tokio = { workspace = true }
anyhow = { workspace = true }
clap = { version = "4", features = ["derive"] }
urlencoding = "2"
bytes = "1"
```

- [ ] **Step 3: Placeholder `main.rs`**

```rust
fn main() {
    println!("tnls (phase 1 scaffold)");
}
```

- [ ] **Step 4: Verify it builds**

Run: `cargo build -p tnls`
Expected: compiles (downloads librqbit + deps on first run — may take a minute).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tnls/
git commit -m "chore(tnls): scaffold crate with librqbit"
```

### Task 2: `ShareMeta` + `create_share` + magnet building

**Files:**
- Create: `crates/tnls/src/bittorrent.rs`
- Modify: `crates/tnls/src/main.rs` (add `mod bittorrent;`)

- [ ] **Step 1: Write the failing tests**

Create `crates/tnls/src/bittorrent.rs`:
```rust
use anyhow::{Context, Result};
use std::path::Path;

/// Everything needed to seed a file and to hand a peer a magnet.
#[derive(Debug, Clone)]
pub struct ShareMeta {
    pub info_hash_hex: String,
    pub name: String,
    pub size: u64,
    pub magnet: String,
    pub torrent_bytes: bytes::Bytes,
}

/// Build a BTv1 magnet from an infohash hex, display name, and trackers.
pub fn build_magnet(info_hash_hex: &str, name: &str, trackers: &[String]) -> String {
    let mut m = format!("magnet:?xt=urn:btih:{info_hash_hex}");
    if !name.is_empty() {
        m.push_str(&format!("&dn={}", urlencoding::encode(name)));
    }
    for t in trackers {
        m.push_str(&format!("&tr={}", urlencoding::encode(t)));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnet_has_infohash_name_and_tracker() {
        let m = build_magnet("aabbccddeeff00112233445566778899aabbccdd", "my file.bin",
            &["udp://tracker.example:1337/announce".to_string()]);
        assert!(m.starts_with("magnet:?xt=urn:btih:aabbccddeeff00112233445566778899aabbccdd"), "{m}");
        assert!(m.contains("&dn=my%20file.bin"), "{m}");
        assert!(m.contains("&tr=udp%3A%2F%2Ftracker.example%3A1337%2Fannounce"), "{m}");
    }

    #[tokio::test]
    async fn create_share_is_deterministic_with_correct_meta() {
        let dir = std::env::temp_dir().join(format!("tnls-cs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.bin");
        std::fs::write(&path, b"hello tnls phase one").unwrap();

        let a = create_share(&path, &[]).await.unwrap();
        let b = create_share(&path, &[]).await.unwrap();

        assert_eq!(a.info_hash_hex.len(), 40, "infohash is 40 hex chars");
        assert_eq!(a.info_hash_hex, b.info_hash_hex, "deterministic infohash");
        assert_eq!(a.name, "hello.bin");
        assert_eq!(a.size, 20);
        assert!(a.magnet.contains(&a.info_hash_hex));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tnls bittorrent`
Expected: FAIL — `create_share` not defined.

- [ ] **Step 3: Implement `create_share`**

Add to `bittorrent.rs` (above the test module):
```rust
/// Create a torrent for `path` and assemble the magnet + torrent bytes for seeding.
pub async fn create_share(path: &Path, trackers: &[String]) -> Result<ShareMeta> {
    let size = std::fs::metadata(path).with_context(|| format!("stat {path:?}"))?.len();
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("file has no UTF-8 name")?
        .to_string();

    let spawner = librqbit::spawn_utils::BlockingSpawner::default();
    let result = librqbit::create_torrent(path, librqbit::CreateTorrentOptions::default(), &spawner)
        .await
        .context("create_torrent")?;

    // Id20 has no Display; as_string() returns the 40-char lowercase hex.
    let info_hash_hex = result.info_hash().as_string();
    let torrent_bytes = result.as_bytes().context("serialize torrent")?;
    let magnet = build_magnet(&info_hash_hex, &name, trackers);

    Ok(ShareMeta { info_hash_hex, name, size, magnet, torrent_bytes })
}
```

Add to `crates/tnls/src/main.rs`: `mod bittorrent;` (above `fn main`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tnls bittorrent`
Expected: PASS (2 tests). If `info_hash().to_string()` isn't 40-hex, fix the accessor and note it.

- [ ] **Step 5: Commit**

```bash
git add crates/tnls/
git commit -m "feat(tnls): create_share — torrent + magnet from a file"
```

### Task 3: `seed` + `fetch` + progress

**Files:**
- Modify: `crates/tnls/src/bittorrent.rs`

- [ ] **Step 1: Implement seed/fetch (covered by the integration test in Task 4)**

Add to `bittorrent.rs`:
```rust
use librqbit::{AddTorrent, AddTorrentOptions, ManagedTorrent, Session, SessionOptions};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

/// Options that differ between real use (internet) and the hermetic test.
#[derive(Clone, Default)]
pub struct NetOpts {
    pub disable_dht: bool,
    pub listen_port: Option<u16>,
    pub enable_upnp: bool,
    pub initial_peers: Vec<SocketAddr>,
}

fn session_opts(n: &NetOpts) -> SessionOptions {
    SessionOptions {
        disable_dht: n.disable_dht,
        disable_dht_persistence: true,
        persistence: None, // no on-disk session state
        listen_port_range: n.listen_port.map(|p| p..p + 1),
        enable_upnp_port_forwarding: n.enable_upnp,
        ..Default::default()
    }
}

/// Keeps the seeding session+torrent alive. Drop to stop seeding.
pub struct SeedHandle {
    _session: Arc<Session>,
    _handle: Arc<ManagedTorrent>,
}

/// Seed `meta`'s torrent, serving the existing file under `data_dir`.
pub async fn seed(meta: &ShareMeta, data_dir: &Path, net: NetOpts) -> Result<SeedHandle> {
    let session = Session::new_with_opts(data_dir.to_path_buf(), session_opts(&net))
        .await
        .context("seed session")?;
    let resp = session
        .add_torrent(
            AddTorrent::from_bytes(meta.torrent_bytes.clone()),
            Some(AddTorrentOptions {
                output_folder: Some(data_dir.to_string_lossy().into_owned()),
                overwrite: true, // use the existing file as the data
                initial_peers: (!net.initial_peers.is_empty()).then(|| net.initial_peers.clone()),
                ..Default::default()
            }),
        )
        .await
        .context("seed add_torrent")?;
    let handle = resp.into_handle().context("seed: no handle")?;
    Ok(SeedHandle { _session: session, _handle: handle })
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    pub finished: bool,
    pub down_speed_bps: f64,
    pub up_speed_bps: f64,
}

/// A live download. Poll `progress()`; `wait()` resolves on completion.
pub struct FetchHandle {
    _session: Arc<Session>,
    handle: Arc<ManagedTorrent>,
    out_dir: PathBuf,
    name: Option<String>,
}

impl FetchHandle {
    pub fn progress(&self) -> Progress {
        let s = self.handle.stats();
        Progress {
            downloaded: s.progress_bytes,
            total: s.total_bytes,
            finished: s.finished,
            down_speed_bps: s.live.as_ref().map(|l| l.download_speed.mbps as f64 * 125_000.0).unwrap_or(0.0),
            up_speed_bps: s.live.as_ref().map(|l| l.upload_speed.mbps as f64 * 125_000.0).unwrap_or(0.0),
        }
    }
    pub async fn wait(&self) -> Result<()> {
        self.handle.wait_until_completed().await.context("download")
    }
    /// Path of the downloaded file, if a single-file torrent name is known.
    pub fn output_path(&self) -> Option<PathBuf> {
        self.name.as_ref().map(|n| self.out_dir.join(n))
    }
}

/// Start downloading `magnet` into `out_dir`.
pub async fn fetch(magnet: &str, out_dir: &Path, net: NetOpts) -> Result<FetchHandle> {
    let session = Session::new_with_opts(out_dir.to_path_buf(), session_opts(&net))
        .await
        .context("fetch session")?;
    let resp = session
        .add_torrent(
            AddTorrent::from_url(magnet.to_string()),
            Some(AddTorrentOptions {
                output_folder: Some(out_dir.to_string_lossy().into_owned()),
                overwrite: true,
                initial_peers: (!net.initial_peers.is_empty()).then(|| net.initial_peers.clone()),
                ..Default::default()
            }),
        )
        .await
        .context("fetch add_torrent")?;
    // `dn` from the magnet gives the display name for output_path().
    let name = magnet.split("&dn=").nth(1).and_then(|s| s.split('&').next())
        .map(|s| urlencoding::decode(s).map(|c| c.into_owned()).unwrap_or_else(|_| s.to_string()));
    let handle = resp.into_handle().context("fetch: no handle")?;
    Ok(FetchHandle { _session: session, handle, out_dir: out_dir.to_path_buf(), name })
}
```

> **API confirmation notes (do a 1-line docs check if a field mismatches):**
> - `LiveStats.download_speed` is a `Speed`; this plan reads `.mbps` (megabits/s) → bytes/s. If `Speed`'s field differs, adjust the conversion (the test does not depend on speed).
> - `Session::new_with_opts(PathBuf, SessionOptions) -> Result<Arc<Session>>` and `add_torrent(&self, …)` per the example. If `add_torrent` needs `&mut`, wrap differently; the docs example calls it on `Arc<Session>`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p tnls`
Expected: compiles. Fix any field-name mismatches per the notes above and record what changed.

- [ ] **Step 3: Commit**

```bash
git add crates/tnls/
git commit -m "feat(tnls): seed + fetch + progress over librqbit"
```

### Task 4: Hermetic loopback seed↔fetch test (the Phase 1 gate)

**Files:**
- Create: `crates/tnls/tests/loopback.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/tnls/tests/loopback.rs`:
```rust
use std::time::Duration;
use tnls::bittorrent::{create_share, fetch, seed, NetOpts};

#[tokio::test]
async fn seed_then_fetch_moves_bytes_over_loopback() {
    // unique temp dirs
    let base = std::env::temp_dir().join(format!("tnls-loop-{}", std::process::id()));
    let seed_dir = base.join("seed");
    let fetch_dir = base.join("fetch");
    std::fs::create_dir_all(&seed_dir).unwrap();
    std::fs::create_dir_all(&fetch_dir).unwrap();

    // a ~512KB file with recognizable content
    let payload: Vec<u8> = (0..512 * 1024).map(|i| (i % 251) as u8).collect();
    let src = seed_dir.join("payload.bin");
    std::fs::write(&src, &payload).unwrap();

    // seed: DHT off, fixed loopback port
    let port = 47_111u16;
    let meta = create_share(&src, &[]).await.unwrap();
    let _seeder = seed(&meta, &seed_dir, NetOpts {
        disable_dht: true, listen_port: Some(port), enable_upnp: false, initial_peers: vec![],
    }).await.unwrap();

    // fetch: DHT off, connect straight to the seeder via initial_peers (metadata flows over BEP-9)
    let peer = format!("127.0.0.1:{port}").parse().unwrap();
    let dl = fetch(&meta.magnet, &fetch_dir, NetOpts {
        disable_dht: true, listen_port: None, enable_upnp: false, initial_peers: vec![peer],
    }).await.unwrap();

    // wait (bounded) for completion
    tokio::time::timeout(Duration::from_secs(30), dl.wait())
        .await
        .expect("download timed out — peers didn't connect")
        .expect("download errored");

    let got = std::fs::read(fetch_dir.join("payload.bin")).expect("fetched file missing");
    assert_eq!(got, payload, "fetched bytes must equal the source");

    let _ = std::fs::remove_dir_all(&base);
}
```

- [ ] **Step 2: Expose the module to the test**

Integration tests compile against the crate's lib, but `tnls` is currently bin-only. Add a thin lib target so `tnls::bittorrent` is importable:
- Create `crates/tnls/src/lib.rs` with `pub mod bittorrent;`.
- In `crates/tnls/src/main.rs`, **remove** the `mod bittorrent;` line — the module now lives in the lib. The placeholder `main` doesn't reference it yet (Task 5's real `main` imports it via `use tnls::bittorrent`). Leaving `mod bittorrent;` in `main.rs` would compile the module twice (once into the lib, once into the bin).

- [ ] **Step 3: Run the test**

Run: `cargo test -p tnls --test loopback -- --nocapture`
Expected: PASS within the 30s budget — the fetched file equals the source. If the two sessions don't connect on the runner (rare on loopback), see the fallback below before weakening anything.

**Fallback gate (only if loopback peering is impossible in the test env):** keep this test but mark it `#[ignore]`, and rely on Task 2's determinism test plus the Task 5 manual two-terminal check as the Phase 1 gate. Record the reason in the commit message.

- [ ] **Step 4: Commit**

```bash
git add crates/tnls/
git commit -m "test(tnls): hermetic loopback seed<->fetch moves bytes"
```

### Task 5: Minimal `seed` / `fetch` CLI (manual gate + Phase 2 seed)

**Files:**
- Create: `crates/tnls/src/cli.rs`
- Modify: `crates/tnls/src/main.rs`

- [ ] **Step 1: Write `cli.rs`**

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tnls", about = "capability-scoped file sending over BitTorrent")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Seed a file and print its magnet (Phase 1 dev command; foreground).
    Seed {
        path: String,
        /// Tracker(s) to embed in the magnet (repeatable).
        #[arg(long)]
        tracker: Vec<String>,
    },
    /// Fetch a file from a magnet into a directory.
    Fetch {
        magnet: String,
        #[arg(long, default_value = "./tnls-downloads")]
        out: String,
    },
}
```

- [ ] **Step 2: Wire `main.rs`**

```rust
use anyhow::Result;
use clap::Parser;
use std::path::Path;
use std::time::Duration;
use tnls::bittorrent::{create_share, fetch, seed, NetOpts};

mod cli;
use cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Seed { path, tracker } => {
            let trackers = if tracker.is_empty() {
                vec!["udp://tracker.opentrackr.org:1337/announce".to_string()]
            } else { tracker };
            let meta = create_share(Path::new(&path), &trackers).await?;
            let data_dir = Path::new(&path).parent().unwrap_or(Path::new(".")).to_path_buf();
            let _seeder = seed(&meta, &data_dir, NetOpts {
                disable_dht: false, listen_port: None, enable_upnp: true, initial_peers: vec![],
            }).await?;
            println!("seeding {} ({} bytes)", meta.name, meta.size);
            println!("magnet → {}", meta.magnet);
            println!("ctrl-c to stop seeding.");
            std::future::pending::<()>().await; // seed until killed
            Ok(())
        }
        Command::Fetch { magnet, out } => {
            std::fs::create_dir_all(&out)?;
            let dl = fetch(&magnet, Path::new(&out), NetOpts {
                disable_dht: false, listen_port: None, enable_upnp: true, initial_peers: vec![],
            }).await?;
            loop {
                let p = dl.progress();
                let pct = if p.total > 0 { p.downloaded * 100 / p.total } else { 0 };
                println!("  {pct:>3}%  {}/{} bytes  ↓{:.0} KB/s",
                    p.downloaded, p.total, p.down_speed_bps / 1000.0);
                if p.finished { break; }
                tokio::time::sleep(Duration::from_millis(800)).await;
            }
            println!("done → {:?}", dl.output_path());
            Ok(())
        }
    }
}
```

- [ ] **Step 3: Build + manual two-terminal gate**

Run: `cargo build -p tnls`
Then (manual, the Phase 1 acceptance demo):
1. Terminal A: `cargo run -p tnls -- seed ./some-bigish-file.bin` → prints a `magnet:` link.
2. Terminal B: `cargo run -p tnls -- fetch '<magnet>' --out /tmp/tnls-dl` → progress climbs to 100%.
3. `cmp ./some-bigish-file.bin /tmp/tnls-dl/some-bigish-file.bin` → identical.

(On the same machine this exercises real DHT/tracker + loopback; across two machines it proves internet transfer when a peer is reachable.)

- [ ] **Step 4: Commit**

```bash
git add crates/tnls/
git commit -m "feat(tnls): minimal seed/fetch CLI (phase 1 gate)"
```

**Chunk 1 done / Phase 1 gate:** `cargo test -p tnls` is green (magnet + determinism unit tests, hermetic loopback transfer), and the `tnls seed | fetch` CLI moves a real file. Bytes move peer-to-peer with no tunnel. Ready for Phase 2 (MCP + tunnel wiring), which will wrap `create_share`/`seed` behind `tnls share` and add the headless-viewer `tnls get`.

---

## Notes for Phase 2 (not built here)
- `tnls share` = `create_share` + `seed` (background task) + `tunnel_locker::session::open(server = current_exe, args = ["mcp-serve", "--magnet", …], relay = wss://tnls.to, scope = [list_shares, request_file], ttl)`.
- `tnls mcp-serve` = stdio MCP server (mirror `mcp-demo`) returning the magnet from `request_file`.
- `tnls get <link>` = headless viewer (parse `/t/<id>#<token>`, ws to `/viewer/<id>`, `hello`→read until `ready`+`tools`→`call request_file`→magnet) + `fetch`.
