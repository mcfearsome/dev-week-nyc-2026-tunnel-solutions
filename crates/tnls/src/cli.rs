use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tnls",
    about = "Pluggable, capability-scoped tunnels to local MCP servers."
)]
pub struct Cli {
    /// Relay base URL (applies to `open` and plugin dispatch).
    #[arg(long, global = true, default_value = "ws://127.0.0.1:8787")]
    pub relay: String,
    /// Override the tunnel TTL (else the plugin's describe default, or 15m for `open`).
    #[arg(long, global = true)]
    pub ttl: Option<String>,
    /// Override the scope allowlist, comma-separated (else the plugin's describe scope).
    #[arg(long, global = true)]
    pub scope: Option<String>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Tunnel any local MCP server (the generic primitive; = the old `tunnel open`).
    Open {
        /// Path to the MCP server binary.
        server: String,
        /// Args forwarded to the server (after `--`).
        server_args: Vec<String>,
    },
    /// Revoke a running tunnel (SIGTERM; defaults to the most recent).
    Close {
        #[arg(long)]
        tunnel_id: Option<String>,
    },
    /// List discovered `tnls-*` plugins.
    Plugins,
    /// Dispatch to a plugin: `tnls <name> <command> [args…]`.
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// Split a `--scope a,b , c` string into trimmed, non-empty names.
pub fn parse_scope(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
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
    fn open_parses_server_and_global_ttl() {
        let cli = Cli::try_parse_from(["tnls", "open", "./srv", "--ttl", "2m"]).unwrap();
        assert_eq!(cli.ttl.as_deref(), Some("2m"));
        match cli.cmd {
            Cmd::Open {
                server,
                server_args,
            } => {
                assert_eq!(server, "./srv");
                assert!(server_args.is_empty());
            }
            _ => panic!("expected open"),
        }
    }

    #[test]
    fn plugin_tail_is_captured_verbatim() {
        // Globals BEFORE the plugin name are the host's; everything after belongs to the plugin.
        let cli = Cli::try_parse_from([
            "tnls",
            "--ttl",
            "1h",
            "rendezvous",
            "get",
            "lnk",
            "--out",
            "d",
        ])
        .unwrap();
        assert_eq!(cli.ttl.as_deref(), Some("1h"));
        match cli.cmd {
            Cmd::External(v) => assert_eq!(v, vec!["rendezvous", "get", "lnk", "--out", "d"]),
            _ => panic!("expected external"),
        }
    }

    #[test]
    fn ttl_after_plugin_name_belongs_to_plugin() {
        // `--ttl` AFTER the plugin name is captured into the tail (forwarded to the plugin), per spec §3.2.
        let cli = Cli::try_parse_from(["tnls", "rendezvous", "get", "lnk", "--ttl", "1h"]).unwrap();
        assert!(
            cli.ttl.is_none(),
            "host did not consume the post-name --ttl"
        );
        match cli.cmd {
            Cmd::External(v) => {
                assert!(v.contains(&"--ttl".to_string()) && v.contains(&"1h".to_string()))
            }
            _ => panic!("expected external"),
        }
    }
}
