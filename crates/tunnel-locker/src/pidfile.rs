use anyhow::{anyhow, Result};
use std::path::PathBuf;

fn pidfile_path(tunnel_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("tunnel-{tunnel_id}.pid"))
}
fn latest_path() -> PathBuf {
    std::env::temp_dir().join("tunnel-latest.pid")
}

/// Body format (3 lines): pid, tunnel_id, link.
pub fn write(tunnel_id: &str, link: &str) -> Result<()> {
    let body = format!("{}\n{tunnel_id}\n{link}\n", std::process::id());
    std::fs::write(pidfile_path(tunnel_id), &body)?;
    std::fs::write(latest_path(), &body)?;
    Ok(())
}

pub fn remove(tunnel_id: &str) {
    let _ = std::fs::remove_file(pidfile_path(tunnel_id));
    if let Ok(body) = std::fs::read_to_string(latest_path()) {
        if body.lines().nth(1) == Some(tunnel_id) {
            let _ = std::fs::remove_file(latest_path());
        }
    }
}

/// Returns (pid, tunnel_id) from a specific or the latest pidfile.
pub fn read(tunnel_id: Option<&str>) -> Result<(i32, String)> {
    let path = tunnel_id.map(pidfile_path).unwrap_or_else(latest_path);
    let body = std::fs::read_to_string(&path)
        .map_err(|_| anyhow!("no tunnel pidfile at {} (is a tunnel open?)", path.display()))?;
    let mut lines = body.lines();
    let pid: i32 = lines.next().ok_or_else(|| anyhow!("empty pidfile"))?.parse()?;
    let id = lines.next().unwrap_or("").to_string();
    Ok((pid, id))
}
