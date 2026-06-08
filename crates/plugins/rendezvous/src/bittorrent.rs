use anyhow::{Context, Result};
use librqbit::{
    AddTorrent, AddTorrentOptions, ListenerOptions, ManagedTorrent, Session, SessionOptions,
};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Everything needed to seed a file and to hand a peer a magnet.
#[derive(Debug, Clone)]
pub struct ShareMeta {
    #[allow(dead_code)] // used in tests and future consumers
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

/// Create a torrent for `path` and assemble the magnet + torrent bytes for seeding.
pub async fn create_share(path: &Path, trackers: &[String]) -> Result<ShareMeta> {
    let size = std::fs::metadata(path)
        .with_context(|| format!("stat {path:?}"))?
        .len();
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("file has no UTF-8 name")?
        .to_string();

    let spawner = librqbit::spawn_utils::BlockingSpawner::new(1);
    let result =
        librqbit::create_torrent(path, librqbit::CreateTorrentOptions::default(), &spawner)
            .await
            .context("create_torrent")?;

    // Id20 has no Display; as_string() returns the 40-char lowercase hex.
    let info_hash_hex = result.info_hash().as_string();
    let torrent_bytes = result.as_bytes().context("serialize torrent")?;
    let magnet = build_magnet(&info_hash_hex, &name, trackers);

    Ok(ShareMeta {
        info_hash_hex,
        name,
        size,
        magnet,
        torrent_bytes,
    })
}

/// Options that differ between real use (internet) and the hermetic test.
#[derive(Clone, Default)]
pub struct NetOpts {
    pub disable_dht: bool,
    pub listen_port: Option<u16>,
    pub enable_upnp: bool,
    pub initial_peers: Vec<SocketAddr>,
}

fn session_opts(n: &NetOpts) -> SessionOptions {
    // `dht: None` disables DHT entirely (vs Some(DhtSessionConfig) to enable it).
    // Listen port is set via `ListenerOptions::listen_addr` (IPv4 loopback-compatible).
    let listen = n.listen_port.map(|p| ListenerOptions {
        listen_addr: (Ipv4Addr::UNSPECIFIED, p).into(),
        enable_upnp_port_forwarding: n.enable_upnp,
        ipv4_only: true,
        ..ListenerOptions::default()
    });
    SessionOptions {
        dht: if n.disable_dht {
            None
        } else {
            Some(librqbit::DhtSessionConfig::default())
        },
        persistence: None,
        listen,
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
                overwrite: true,
                initial_peers: (!net.initial_peers.is_empty()).then(|| net.initial_peers.clone()),
                ..Default::default()
            }),
        )
        .await
        .context("seed add_torrent")?;
    let handle = resp.into_handle().context("seed: no handle")?;
    Ok(SeedHandle {
        _session: session,
        _handle: handle,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    pub finished: bool,
    #[allow(dead_code)] // available for callers that want speed metrics
    pub down_speed_bps: f64,
    #[allow(dead_code)] // available for callers that want speed metrics
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
            down_speed_bps: s
                .live
                .as_ref()
                .map(|l| l.download_speed.mbps * 125_000.0)
                .unwrap_or(0.0),
            up_speed_bps: s
                .live
                .as_ref()
                .map(|l| l.upload_speed.mbps * 125_000.0)
                .unwrap_or(0.0),
        }
    }
    #[allow(dead_code)] // used in #[cfg(test)] and future consumers
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
    let name = magnet
        .split("&dn=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .map(|s| {
            urlencoding::decode(s)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| s.to_string())
        });
    let handle = resp.into_handle().context("fetch: no handle")?;
    Ok(FetchHandle {
        _session: session,
        handle,
        out_dir: out_dir.to_path_buf(),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let port = 47_112u16; // different from loopback.rs to avoid conflicts
        let meta = create_share(&src, &[]).await.unwrap();
        let _seeder = seed(
            &meta,
            &seed_dir,
            NetOpts {
                disable_dht: true,
                listen_port: Some(port),
                enable_upnp: false,
                initial_peers: vec![],
            },
        )
        .await
        .unwrap();

        // fetch: DHT off, connect straight to the seeder via initial_peers
        let peer = format!("127.0.0.1:{port}").parse().unwrap();
        let dl = fetch(
            &meta.magnet,
            &fetch_dir,
            NetOpts {
                disable_dht: true,
                listen_port: None,
                enable_upnp: false,
                initial_peers: vec![peer],
            },
        )
        .await
        .unwrap();

        // wait (bounded) for completion
        tokio::time::timeout(std::time::Duration::from_secs(30), dl.wait())
            .await
            .expect("download timed out — peers didn't connect")
            .expect("download errored");

        let got = std::fs::read(fetch_dir.join("payload.bin")).expect("fetched file missing");
        assert_eq!(got, payload, "fetched bytes must equal the source");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn magnet_has_infohash_name_and_tracker() {
        let m = build_magnet(
            "aabbccddeeff00112233445566778899aabbccdd",
            "my file.bin",
            &["udp://tracker.example:1337/announce".to_string()],
        );
        assert!(
            m.starts_with("magnet:?xt=urn:btih:aabbccddeeff00112233445566778899aabbccdd"),
            "{m}"
        );
        assert!(m.contains("&dn=my%20file.bin"), "{m}");
        assert!(
            m.contains("&tr=udp%3A%2F%2Ftracker.example%3A1337%2Fannounce"),
            "{m}"
        );
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
