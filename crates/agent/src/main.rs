use agent::cli::{parse_scope, Cli, Command};
use agent::session::{self, OpenArgs};
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Open { server, server_args, ttl, scope, relay } => {
            let ttl = protocol::parse_ttl(&ttl).map_err(anyhow::Error::msg)?;
            session::open(OpenArgs { server, server_args, ttl, scope: parse_scope(&scope), relay }).await
        }
        Command::Close { tunnel_id } => session::close(tunnel_id.as_deref()),
    }
}
