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
