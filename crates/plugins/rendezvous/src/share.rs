use crate::bittorrent::{create_share, seed, NetOpts};
use crate::server::{ShareInfo, ShareServer};
use anyhow::{Context, Result};
use rmcp::{transport::stdio, ServiceExt};
use std::path::Path;

const DEFAULT_TRACKER: &str = "udp://tracker.opentrackr.org:1337/announce";

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("0.0.0.0:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
}

/// loopback + LAN IPv4 + (best-effort) public IP from the relay's /whoami, all at :port.
/// `relay` is the TNLS_RELAY the host passes down; empty => skip the public-IP lookup.
async fn gather_peers(port: u16, relay: &str) -> Vec<String> {
    let mut addrs = vec![format!("127.0.0.1:{port}")];
    if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
        for (_n, ip) in ifaces {
            if let std::net::IpAddr::V4(v4) = ip {
                if !v4.is_loopback() && !v4.is_link_local() {
                    addrs.push(format!("{v4}:{port}"));
                }
            }
        }
    }
    if !relay.is_empty() {
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
    }
    addrs
}

/// Seed `path`, gather reachable peer addresses, then serve the share over stdio (rmcp).
/// Runs until stdin closes (the host kills it on tunnel teardown) — at which point the
/// returned service ends, `ShareServer` drops, and the seeder stops.
pub async fn run_share(path: String) -> Result<()> {
    let p = Path::new(&path);
    let meta = create_share(p, &[DEFAULT_TRACKER.to_string()]).await?;
    let data_dir = p.parent().unwrap_or(Path::new(".")).to_path_buf();

    let port = pick_free_port();
    let relay = std::env::var("TNLS_RELAY").unwrap_or_default();
    let peers = gather_peers(port, &relay).await;

    let seeder = seed(
        &meta,
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

    // To stderr (tnls inherits it) — the magnet rides the tunnel to the viewer, not stdout
    // (stdout is the MCP JSON-RPC channel).
    eprintln!("seeding {} ({} bytes)", meta.name, meta.size);

    let share = ShareInfo {
        magnet: meta.magnet,
        name: meta.name,
        size: meta.size,
        peers,
    };
    let service = ShareServer::new(share, seeder).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
