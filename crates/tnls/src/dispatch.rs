use crate::cli::parse_scope;
use crate::discover;
use anyhow::{anyhow, bail, Context, Result};
use tnls_plugin::Manifest;
use tnls_tunnel::session::{self, OpenArgs};

/// Where a resolved command routes.
#[derive(Debug)]
pub enum Route {
    /// Tunnel the plugin's serve command with this scope + ttl-string.
    Tunnel { scope: Vec<String>, ttl: String },
    /// Run the plugin command as a plain client (no tunnel).
    Plain,
}

/// Pure routing decision (unit-testable): given the manifest, the command name, and the
/// host's optional `--scope`/`--ttl` overrides, decide how to run it.
pub fn route(
    m: &Manifest,
    cmd: &str,
    scope_override: Option<&str>,
    ttl_override: Option<&str>,
) -> Result<Route> {
    let command = m.command(cmd).ok_or_else(|| {
        let known: Vec<&str> = m.commands.iter().map(|c| c.name.as_str()).collect();
        anyhow!(
            "unknown command '{cmd}' for plugin '{}'; known: {}",
            m.name,
            known.join(", ")
        )
    })?;
    Ok(match &command.tunnel {
        Some(t) => Route::Tunnel {
            scope: scope_override
                .map(parse_scope)
                .unwrap_or_else(|| t.scope.clone()),
            ttl: ttl_override.unwrap_or(&t.default_ttl).to_string(),
        },
        None => Route::Plain,
    })
}

/// `args` = [name, command, rest…]. `relay`/`ttl`/`scope` are the host globals.
pub async fn run(
    args: Vec<String>,
    relay: String,
    ttl: Option<String>,
    scope: Option<String>,
) -> Result<()> {
    let name = args.first().ok_or_else(|| anyhow!("no plugin named"))?;
    let bin = discover::resolve(name).ok_or_else(|| {
        anyhow!("no plugin '{name}' — looked for 'tnls-{name}' next to tnls and on $PATH. Run 'tnls plugins' to see what's installed.")
    })?;

    let out = tokio::process::Command::new(&bin)
        .arg("describe")
        .output()
        .await
        .with_context(|| format!("running '{} describe'", bin.display()))?;
    let manifest: Manifest = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing describe from '{name}'"))?;

    let cmd = args
        .get(1)
        .ok_or_else(|| anyhow!("usage: tnls {name} <command> [args…]"))?;
    let tail = args[1..].to_vec(); // [command, rest…] — forwarded verbatim

    match route(&manifest, cmd, scope.as_deref(), ttl.as_deref())? {
        Route::Tunnel { scope, ttl } => {
            let ttl = tnls_core::parse_ttl(&ttl).map_err(anyhow::Error::msg)?;
            session::open(OpenArgs {
                server: bin.to_string_lossy().into_owned(),
                server_args: tail,
                ttl,
                scope,
                relay: relay.clone(),
                env: vec![("TNLS_RELAY".to_string(), relay)],
            })
            .await
        }
        Route::Plain => {
            let status = tokio::process::Command::new(&bin)
                .args(&tail)
                .status()
                .await
                .with_context(|| format!("running '{}'", bin.display()))?;
            if !status.success() {
                bail!("{name} {cmd} exited with {status}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tnls_plugin::Command;

    fn manifest() -> Manifest {
        Manifest::new(
            "rendezvous",
            "x",
            vec![
                Command::tunneled("share", "s", &["list_shares", "request_file"], "30m"),
                Command::plain("get", "g"),
            ],
        )
    }

    #[test]
    fn tunneled_uses_describe_scope_and_ttl_by_default() {
        match route(&manifest(), "share", None, None).unwrap() {
            Route::Tunnel { scope, ttl } => {
                assert_eq!(scope, vec!["list_shares", "request_file"]);
                assert_eq!(ttl, "30m");
            }
            _ => panic!("share should tunnel"),
        }
    }

    #[test]
    fn overrides_win() {
        match route(&manifest(), "share", Some("read"), Some("5m")).unwrap() {
            Route::Tunnel { scope, ttl } => {
                assert_eq!(scope, vec!["read"]);
                assert_eq!(ttl, "5m");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn plain_command_does_not_tunnel() {
        assert!(matches!(
            route(&manifest(), "get", None, None).unwrap(),
            Route::Plain
        ));
    }

    #[test]
    fn unknown_command_errors_with_known_list() {
        let e = route(&manifest(), "nope", None, None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown command 'nope'") && e.contains("share"));
    }
}
