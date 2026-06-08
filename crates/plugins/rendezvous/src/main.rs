mod bittorrent;
mod get;
mod server;
mod share;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::Path;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "tnls-rendezvous",
    about = "Capability-scoped file sending over BitTorrent."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Seed a file and serve it as an MCP server over stdio (spawned by tnls; tunneled).
    Share { path: String },
    /// Fetch a shared file from a tunnel link (plain client; no tunnel).
    Get {
        link: String,
        #[arg(long, default_value = "./tnls-downloads")]
        out: String,
    },
    /// Seed a file and print its magnet (dev; foreground).
    Seed {
        path: String,
        #[arg(long)]
        tracker: Vec<String>,
    },
    /// Fetch a file from a magnet into a directory (dev).
    Fetch {
        magnet: String,
        #[arg(long, default_value = "./tnls-downloads")]
        out: String,
    },
    /// Print this plugin's describe manifest (called by tnls).
    Describe,
}

#[tokio::main]
async fn main() -> Result<()> {
    // rustls 0.23 needs a process-wide provider before any wss:// (get) or https (/whoami) dial.
    let _ = rustls::crypto::ring::default_provider().install_default();

    match Cli::parse().cmd {
        Cmd::Describe => {
            println!("{}", describe().to_json());
            Ok(())
        }
        Cmd::Share { path } => share::run_share(path).await,
        Cmd::Get { link, out } => get::run_get(&link, Path::new(&out)).await,
        Cmd::Seed { path, tracker } => seed_cmd(path, tracker).await,
        Cmd::Fetch { magnet, out } => fetch_cmd(magnet, out).await,
    }
}

/// `share` is the only tunneled command; the rest are plain client/dev commands.
fn describe() -> tnls_plugin::Manifest {
    tnls_plugin::Manifest::new(
        "rendezvous",
        "Capability-scoped file sending over BitTorrent.",
        vec![
            tnls_plugin::Command::tunneled(
                "share",
                "Seed a file and serve it through a scoped tunnel.",
                &["list_shares", "request_file"],
                "30m",
            ),
            tnls_plugin::Command::plain("get", "Fetch a shared file from a tunnel link."),
            tnls_plugin::Command::plain("seed", "Seed a file and print its magnet (dev)."),
            tnls_plugin::Command::plain("fetch", "Fetch a file from a magnet (dev)."),
        ],
    )
}

async fn seed_cmd(path: String, tracker: Vec<String>) -> Result<()> {
    use bittorrent::{create_share, seed, NetOpts};
    let trackers = if tracker.is_empty() {
        vec!["udp://tracker.opentrackr.org:1337/announce".to_string()]
    } else {
        tracker
    };
    let meta = create_share(Path::new(&path), &trackers).await?;
    let data_dir = Path::new(&path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let _seeder = seed(
        &meta,
        &data_dir,
        NetOpts {
            disable_dht: false,
            listen_port: None,
            enable_upnp: true,
            initial_peers: vec![],
        },
    )
    .await?;
    println!(
        "seeding {} ({} bytes)\nmagnet → {}\nctrl-c to stop.",
        meta.name, meta.size, meta.magnet
    );
    std::future::pending::<()>().await;
    Ok(())
}

async fn fetch_cmd(magnet: String, out: String) -> Result<()> {
    use bittorrent::{fetch, NetOpts};
    std::fs::create_dir_all(&out)?;
    let dl = fetch(
        &magnet,
        Path::new(&out),
        NetOpts {
            disable_dht: false,
            listen_port: None,
            enable_upnp: true,
            initial_peers: vec![],
        },
    )
    .await?;
    loop {
        let p = dl.progress();
        let pct = if p.total > 0 {
            p.downloaded * 100 / p.total
        } else {
            0
        };
        println!("  {pct:>3}%  {}/{} bytes", p.downloaded, p.total);
        if p.finished {
            break;
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    println!("done → {:?}", dl.output_path());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn describe_share_is_tunneled_get_is_plain() {
        let m = super::describe();
        assert!(
            m.command("share").unwrap().tunnel.is_some(),
            "share is tunneled"
        );
        assert!(
            m.command("get").unwrap().tunnel.is_none(),
            "get is a plain client command"
        );
        assert_eq!(
            m.command("share").unwrap().tunnel.as_ref().unwrap().scope,
            vec!["list_shares", "request_file"]
        );
    }
}
