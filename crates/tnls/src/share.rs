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

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("0.0.0.0:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
}

/// loopback + LAN IPv4 + (best-effort) public IP from the relay's /whoami, all at :port.
async fn gather_peers(port: u16, relay: &str) -> Vec<String> {
    let mut addrs = vec![format!("127.0.0.1:{port}")];
    if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
        for (_name, ip) in ifaces {
            if let std::net::IpAddr::V4(v4) = ip {
                if !v4.is_loopback() && !v4.is_link_local() {
                    addrs.push(format!("{v4}:{port}"));
                }
            }
        }
    }
    // public IP via the relay's /whoami (https for wss relay, http for ws)
    let scheme = if relay.starts_with("wss://") {
        "https"
    } else {
        "http"
    };
    let host = relay
        .trim_start_matches("ws://")
        .trim_start_matches("wss://")
        .trim_end_matches('/');
    if let Ok(resp) = reqwest::get(format!("{scheme}://{host}/whoami")).await {
        if let Ok(ip) = resp.text().await {
            let ip = ip.trim();
            if ip.parse::<std::net::Ipv4Addr>().is_ok() {
                addrs.push(format!("{ip}:{port}"));
            }
        }
    }
    addrs
}

pub async fn run_share(args: ShareArgs) -> Result<()> {
    let path = Path::new(&args.path);
    let meta = create_share(path, &[DEFAULT_TRACKER.to_string()]).await?;
    let data_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    // Pick a free port and gather reachable peer addresses before seeding.
    let port = pick_free_port();
    let peers = gather_peers(port, &args.relay).await;

    // Seed in the background for the lifetime of this process.
    let seed_meta = meta.clone();
    let _seeder = seed(
        &seed_meta,
        &data_dir,
        NetOpts {
            disable_dht: false,
            listen_port: Some(port),
            enable_upnp: true,
            initial_peers: vec![],
        },
    )
    .await
    .context("start seeding")?;

    println!(
        "seeding {} ({} bytes) — opening a scoped link…",
        meta.name, meta.size
    );

    // Reuse the tunnel: point the agent at the `tnls` binary in mcp-serve mode.
    let exe = match &args.mcp_exe {
        Some(p) => p.clone(),
        None => std::env::current_exe()?.to_string_lossy().into_owned(),
    };
    let mut server_args = vec![
        "mcp-serve".into(),
        "--magnet".into(),
        meta.magnet.clone(),
        "--name".into(),
        meta.name.clone(),
        "--size".into(),
        meta.size.to_string(),
    ];
    for a in &peers {
        server_args.push("--peer".into());
        server_args.push(a.clone());
    }

    tunnel_locker::session::open(tunnel_locker::session::OpenArgs {
        server: exe,
        server_args,
        ttl: args.ttl,
        scope: vec!["list_shares".into(), "request_file".into()],
        relay: args.relay,
    })
    .await?; // blocks until the tunnel is revoked (Ctrl-C / TTL); _seeder drops, seeding stops.
    Ok(())
}
