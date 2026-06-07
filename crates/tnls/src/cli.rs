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
    /// Get a shared file from a scoped tunnel link.
    Get {
        link: String,
        #[arg(long, default_value = "./tnls-downloads")]
        out: String,
    },
    /// Seed a file and share it over a capability-scoped tunnel link.
    Share {
        path: String,
        #[arg(long, default_value = "wss://tunnel.locker")]
        relay: String,
        #[arg(long, default_value = "30m")]
        ttl: String,
    },
    /// Internal: the MCP server the tunnel spawns.
    #[command(hide = true)]
    McpServe {
        #[arg(long)] magnet: String,
        #[arg(long)] name: String,
        #[arg(long)] size: u64,
    },
}
