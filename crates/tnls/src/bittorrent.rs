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

/// Create a torrent for `path` and assemble the magnet + torrent bytes for seeding.
pub async fn create_share(path: &Path, trackers: &[String]) -> Result<ShareMeta> {
    let size = std::fs::metadata(path).with_context(|| format!("stat {path:?}"))?.len();
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("file has no UTF-8 name")?
        .to_string();

    let spawner = librqbit::spawn_utils::BlockingSpawner::new(1);
    let result = librqbit::create_torrent(path, librqbit::CreateTorrentOptions::default(), &spawner)
        .await
        .context("create_torrent")?;

    // Id20 has no Display; as_string() returns the 40-char lowercase hex.
    let info_hash_hex = result.info_hash().as_string();
    let torrent_bytes = result.as_bytes().context("serialize torrent")?;
    let magnet = build_magnet(&info_hash_hex, &name, trackers);

    Ok(ShareMeta { info_hash_hex, name, size, magnet, torrent_bytes })
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
