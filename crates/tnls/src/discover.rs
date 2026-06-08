use std::path::{Path, PathBuf};

/// Directories searched for `tnls-<name>`: the dir holding the running `tnls`, then `$PATH`.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs
}

/// Resolve a plugin binary by name within `dirs` (testable core of `resolve`).
pub fn resolve_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let bin = format!("tnls-{name}");
    dirs.iter()
        .map(|d| d.join(&bin))
        .find(|p| is_executable_file(p))
}

pub fn resolve(name: &str) -> Option<PathBuf> {
    resolve_in(&search_dirs(), name)
}

/// All discovered plugin names (deduped, first-found wins).
pub fn list_names() -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for dir in search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            if let Some(n) = e.file_name().to_str().and_then(|f| f.strip_prefix("tnls-")) {
                if is_executable_file(&e.path()) {
                    seen.insert(n.to_string());
                }
            }
        }
    }
    seen.into_iter().collect()
}

fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file()
        && std::fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_in_finds_executable_tnls_prefixed_binary() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tnls-foo");
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();

        let dirs = vec![dir.path().to_path_buf()];
        assert_eq!(resolve_in(&dirs, "foo"), Some(p));
        assert_eq!(resolve_in(&dirs, "missing"), None);
    }
}
