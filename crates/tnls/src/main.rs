use anyhow::Result;
use clap::Parser;
use tnls::cli::{parse_scope, Cli, Cmd};
use tnls::{discover, dispatch};
use tnls_tunnel::session::{self, OpenArgs};

#[tokio::main]
async fn main() -> Result<()> {
    // rustls 0.23 needs a process-wide provider before the agent dials the relay over wss://.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Open {
            server,
            server_args,
        } => {
            let ttl = tnls_core::parse_ttl(cli.ttl.as_deref().unwrap_or("15m"))
                .map_err(anyhow::Error::msg)?;
            session::open(OpenArgs {
                server,
                server_args,
                ttl,
                scope: parse_scope(&cli.scope.unwrap_or_default()),
                relay: cli.relay,
                env: vec![],
                ..Default::default()
            })
            .await
        }
        Cmd::Close { tunnel_id } => session::close(tunnel_id.as_deref()),
        Cmd::Plugins => {
            for name in discover::list_names() {
                println!("{name}");
            }
            Ok(())
        }
        Cmd::External(args) => dispatch::run(args, cli.relay, cli.ttl, cli.scope).await,
    }
}
