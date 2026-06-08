mod server;

use clap::{Parser, Subcommand};
use rmcp::{transport::stdio, ServiceExt};

#[derive(Parser)]
#[command(name = "tnls-demo", about = "Sample MCP server plugin (read + shell).")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve read/shell as an MCP server over stdio (spawned by tnls; tunneled).
    Serve,
    /// Print this plugin's describe manifest (called by tnls).
    Describe,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Describe => {
            println!("{}", describe().to_json());
            Ok(())
        }
        Cmd::Serve => {
            let service = server::Demo::new().serve(stdio()).await?;
            service.waiting().await?;
            Ok(())
        }
    }
}

/// The static manifest tnls reads. `serve` is tunneled, scoped to `read` only —
/// `shell` is deny-by-default until the user passes `--scope read,shell`.
fn describe() -> tnls_plugin::Manifest {
    tnls_plugin::Manifest::new(
        "demo",
        "Sample MCP server (read + shell).",
        vec![tnls_plugin::Command::tunneled(
            "serve",
            "Serve read/shell over a tunnel.",
            &["read"],
            "15m",
        )],
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn describe_serve_is_read_scoped() {
        let m = super::describe();
        let serve = m.command("serve").unwrap();
        let t = serve.tunnel.as_ref().expect("serve is tunneled");
        assert_eq!(t.scope, vec!["read"]);
        assert!(
            !t.scope.contains(&"shell".to_string()),
            "shell is deny-by-default"
        );
    }
}
