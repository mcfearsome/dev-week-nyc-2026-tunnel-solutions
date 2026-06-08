//! Pure, side-effect-free helpers for driving the `tnls` CLI: locating the host
//! binary and parsing the lines it prints. Kept free of egui and of process I/O so
//! the parsing is exhaustively unit-testable; the threading lives in `app.rs`.

use std::path::PathBuf;

/// Locate the `tnls` host binary: `$TNLS_BIN`, then a sibling of this GUI binary
/// (the workspace `target/<profile>/` layout puts them side by side), then `$PATH`.
pub fn resolve_tnls() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TNLS_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    resolve_tnls_in(&dirs)
}

/// Find `tnls` within `dirs` (testable core of [`resolve_tnls`]).
pub fn resolve_tnls_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join("tnls")).find(|p| p.is_file())
}

/// Pull a tunnel link out of a banner line, e.g. the host's
/// `  link  →    https://host/t/<id>#<token>`. A link is any whitespace-delimited
/// token that is an http(s) URL carrying both a `/t/` path and a `#fragment`.
pub fn extract_link(line: &str) -> Option<String> {
    line.split_whitespace()
        .find(|tok| {
            (tok.starts_with("http://") || tok.starts_with("https://"))
                && tok.contains("/t/")
                && tok.contains('#')
        })
        .map(str::to_string)
}

/// The tunnel id embedded in a link (`…/t/<id>#…`).
pub fn tunnel_id_from_link(link: &str) -> Option<&str> {
    let after = link.split("/t/").nth(1)?;
    let id = after.split('#').next()?.trim_end_matches('/');
    (!id.is_empty()).then_some(id)
}

/// A parsed download-progress line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub pct: u8,
    pub downloaded: u64,
    pub total: u64,
}

/// Parse a `tnls rendezvous get` progress line: `"  50%  131072/524288 bytes"`.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let t = line.trim();
    if !t.ends_with("bytes") {
        return None;
    }
    let mut it = t.split_whitespace();
    let pct = it.next()?.strip_suffix('%')?.parse::<u8>().ok()?;
    let (d, total) = it.next()?.split_once('/')?;
    Some(Progress {
        pct,
        downloaded: d.parse().ok()?,
        total: total.parse().ok()?,
    })
}

/// Detect the completion line `done → Some("…/file")` (or `done → None`) and pull
/// out the output path when one is present.
pub fn parse_done(line: &str) -> Option<Option<String>> {
    let t = line.trim();
    let rest = t.strip_prefix("done")?.trim_start();
    let rest = rest.strip_prefix('→')?.trim();
    // The CLI prints `{:?}` of an `Option<PathBuf>`, so a path arrives quoted.
    let path = rest
        .find('"')
        .and_then(|a| rest.rfind('"').filter(|b| *b > a).map(|b| (a, b)))
        .map(|(a, b)| rest[a + 1..b].to_string());
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_link_from_banner_line() {
        let line = "  link  →    https://tunnel.locker/t/3f9a8c#eyJhbGc.sig";
        assert_eq!(
            extract_link(line).as_deref(),
            Some("https://tunnel.locker/t/3f9a8c#eyJhbGc.sig")
        );
    }

    #[test]
    fn extracts_local_http_link() {
        assert_eq!(
            extract_link("link → http://127.0.0.1:8787/t/abcd#tok").as_deref(),
            Some("http://127.0.0.1:8787/t/abcd#tok")
        );
    }

    #[test]
    fn ignores_non_link_lines() {
        assert!(extract_link("  serving — ctrl-c to revoke").is_none());
        assert!(extract_link("https://example.com/no-tunnel").is_none());
        assert!(extract_link("seeding payload.bin (512 bytes)").is_none());
    }

    #[test]
    fn pulls_tunnel_id() {
        assert_eq!(
            tunnel_id_from_link("https://tunnel.locker/t/3f9a8c#sig"),
            Some("3f9a8c")
        );
        assert_eq!(
            tunnel_id_from_link("http://127.0.0.1:8787/t/abcd/#tok"),
            Some("abcd")
        );
        assert_eq!(tunnel_id_from_link("nonsense"), None);
    }

    #[test]
    fn parses_progress_line() {
        assert_eq!(
            parse_progress("   50%  131072/524288 bytes"),
            Some(Progress {
                pct: 50,
                downloaded: 131072,
                total: 524288
            })
        );
        assert_eq!(
            parse_progress("  100%  524288/524288 bytes").map(|p| p.pct),
            Some(100)
        );
    }

    #[test]
    fn rejects_non_progress_lines() {
        assert!(parse_progress("magnet acquired — downloading…").is_none());
        assert!(parse_progress("done → Some(\"x\")").is_none());
        assert!(parse_progress("  50%  abc/def bytes").is_none());
    }

    #[test]
    fn parses_done_with_path() {
        assert_eq!(
            parse_done("done → Some(\"./tnls-downloads/payload.bin\")"),
            Some(Some("./tnls-downloads/payload.bin".to_string()))
        );
    }

    #[test]
    fn parses_done_without_path() {
        assert_eq!(parse_done("done → None"), Some(None));
    }

    #[test]
    fn done_rejects_other_lines() {
        assert!(parse_done("  50%  1/2 bytes").is_none());
    }
}
