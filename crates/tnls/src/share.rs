use crate::bittorrent::{create_share, seed, NetOpts};
use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

const DEFAULT_TRACKER: &str = "udp://tracker.opentrackr.org:1337/announce";

pub struct ShareArgs {
    pub path: String,
    pub relay: String,
    pub ttl: Duration,
    /// Path to the `tnls` binary the tunnel spawns for `mcp-serve`. `None` → this running
    /// binary (`current_exe`). The e2e test sets this to the built `tnls`, because an
    /// in-process test would otherwise make the agent spawn the *test* binary.
    pub mcp_exe: Option<String>,
}

pub async fn run_share(args: ShareArgs) -> Result<()> {
    let path = Path::new(&args.path);
    let meta = create_share(path, &[DEFAULT_TRACKER.to_string()]).await?;
    let data_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    // Seed in the background for the lifetime of this process.
    let seed_meta = meta.clone();
    let _seeder = seed(&seed_meta, &data_dir, NetOpts {
        disable_dht: false, listen_port: None, enable_upnp: true, initial_peers: vec![],
    }).await.context("start seeding")?;

    println!("seeding {} ({} bytes) — opening a scoped link…", meta.name, meta.size);

    // Reuse the tunnel: point the agent at the `tnls` binary in mcp-serve mode.
    let exe = match &args.mcp_exe {
        Some(p) => p.clone(),
        None => std::env::current_exe()?.to_string_lossy().into_owned(),
    };
    tunnel_locker::session::open(tunnel_locker::session::OpenArgs {
        server: exe,
        server_args: vec![
            "mcp-serve".into(),
            "--magnet".into(), meta.magnet.clone(),
            "--name".into(), meta.name.clone(),
            "--size".into(), meta.size.to_string(),
        ],
        ttl: args.ttl,
        scope: vec!["list_shares".into(), "request_file".into()],
        relay: args.relay,
    }).await?; // blocks until the tunnel is revoked (Ctrl-C / TTL); _seeder drops, seeding stops.
    Ok(())
}
