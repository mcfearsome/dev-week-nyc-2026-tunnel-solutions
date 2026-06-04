use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tunnel", about = "Ephemeral, capability-scoped tunnel for a local MCP server")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Open a tunnel to a local MCP server (runs in the foreground).
    Open {
        /// Path to the MCP server binary.
        server: String,
        /// Args forwarded to the MCP server (put them after `--`).
        server_args: Vec<String>,
        #[arg(long, default_value = "15m")]
        ttl: String,
        /// Comma-separated tool allowlist, e.g. `read,search`.
        #[arg(long, default_value = "")]
        scope: String,
        #[arg(long, default_value = "ws://127.0.0.1:8787")]
        relay: String,
    },
    /// Revoke a running tunnel (SIGTERM). Defaults to the most recent.
    Close {
        #[arg(long)]
        tunnel_id: Option<String>,
    },
}

/// Split a `--scope a,b , c` string into trimmed, non-empty names.
pub fn parse_scope(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()).map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_splits_and_trims() {
        assert_eq!(parse_scope("read, search"), vec!["read", "search"]);
        assert!(parse_scope("").is_empty());
        assert!(parse_scope("  ").is_empty());
    }

    #[test]
    fn open_parses_flags_not_as_server_args() {
        let cli = Cli::try_parse_from(["tunnel", "open", "./srv", "--ttl", "2m", "--scope", "read"]).unwrap();
        match cli.command {
            Command::Open { server, server_args, ttl, scope, .. } => {
                assert_eq!(server, "./srv");
                assert!(server_args.is_empty());
                assert_eq!(ttl, "2m");
                assert_eq!(scope, "read");
            }
            _ => panic!("expected open"),
        }
    }
}
