use clap::Parser;
use tunnel_locker::cli::{parse_scope, Cli, Command};
use tunnel_locker::session::{self, OpenArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // rustls 0.23 needs a process-wide crypto provider installed before any wss:// dial.
    let _ = rustls::crypto::ring::default_provider().install_default();
    match Cli::parse().command {
        Command::Open {
            server,
            server_args,
            ttl,
            scope,
            relay,
        } => {
            let ttl = tnls_core::parse_ttl(&ttl).map_err(anyhow::Error::msg)?;
            session::open(OpenArgs {
                server,
                server_args,
                ttl,
                scope: parse_scope(&scope),
                relay,
            })
            .await
        }
        Command::Close { tunnel_id } => session::close(tunnel_id.as_deref()),
    }
}
