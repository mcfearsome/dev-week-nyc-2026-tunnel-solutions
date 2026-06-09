//! Locating the `tnls-rendezvous` binary.
//!
//! The GUI runs the *receive* side fully in-process (it calls `tnls_rendezvous::get` +
//! `bittorrent::fetch` directly). The *send* side drives the real tunnel agent in-process
//! via `tnls_tunnel::session::open`, which — per the architecture — spawns the
//! `tnls-rendezvous` binary as the scoped MCP server child that seeds + serves the magnet.
//! So we still need that binary's path; everything else is a library call.

use std::path::{Path, PathBuf};

/// Locate the `tnls-rendezvous` binary: `$TNLS_RENDEZVOUS_BIN`, then a sibling of this GUI
/// binary (the workspace `target/<profile>/` layout puts them side by side), then `$PATH`.
pub fn resolve_rendezvous() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TNLS_RENDEZVOUS_BIN") {
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
    resolve_in(&dirs, "tnls-rendezvous")
}

/// Find `name` within `dirs` (testable core of [`resolve_rendezvous`]).
pub fn resolve_in<P: AsRef<Path>>(dirs: &[P], name: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|d| d.as_ref().join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_in_finds_the_binary_in_order() {
        let dir = std::env::temp_dir().join(format!("tnls-gui-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("tnls-rendezvous");
        std::fs::write(&bin, b"x").unwrap();

        let missing = dir.join("nope");
        let dirs = [missing.as_path(), dir.as_path()];
        assert_eq!(resolve_in(&dirs, "tnls-rendezvous"), Some(bin));
        assert_eq!(resolve_in(&dirs, "absent"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
