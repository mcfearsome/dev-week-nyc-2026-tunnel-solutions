//! Privacy-preserving, server-side traffic counters. No cookies, no stored IPs — only
//! one-way salted hashes for de-duplicating unique visitors. Aggregate counts only.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Mutex;

/// The persisted/serialized shape. `unique` holds salted one-way hashes of client IPs —
/// never the IPs themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Inner {
    salt: u64,
    page_views: u64,
    unique: HashSet<u64>,
    tunnel_links_opened: u64,
    tunnels_opened: u64,
    frames_relayed: u64,
    tool_calls: u64,
    out_of_scope_blocks: u64,
}

impl Inner {
    fn fresh(salt: u64) -> Self {
        Inner {
            salt,
            page_views: 0,
            unique: HashSet::new(),
            tunnel_links_opened: 0,
            tunnels_opened: 0,
            frames_relayed: 0,
            tool_calls: 0,
            out_of_scope_blocks: 0,
        }
    }
}

/// One-way, deterministic, salted hash of an IP. Same (ip, salt) → same value across restarts
/// (so persisted uniques stay stable); the secret salt makes the mapping deployment-specific
/// and non-reversible without it.
fn hash_ip(ip: &str, salt: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    salt.hash(&mut h);
    ip.hash(&mut h);
    h.finish()
}

pub struct Stats {
    inner: Mutex<Inner>,
    path: Option<PathBuf>,
}

impl Stats {
    /// Load from `path` if present, else start fresh with a random salt.
    pub fn new(path: Option<PathBuf>, salt: u64) -> Self {
        let inner = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Inner>(&s).ok())
            .unwrap_or_else(|| Inner::fresh(salt));
        Stats {
            inner: Mutex::new(inner),
            path,
        }
    }

    pub fn page_view(&self, client_ip: &str) {
        let mut g = self.inner.lock().unwrap();
        g.page_views += 1;
        let h = hash_ip(client_ip, g.salt);
        g.unique.insert(h);
    }

    pub fn tunnel_link_opened(&self) {
        self.inner.lock().unwrap().tunnel_links_opened += 1;
    }
    pub fn tunnel_opened(&self) {
        self.inner.lock().unwrap().tunnels_opened += 1;
    }
    pub fn frame_relayed(&self) {
        self.inner.lock().unwrap().frames_relayed += 1;
    }
    pub fn report(&self, calls: u64, blocks: u64) {
        let mut g = self.inner.lock().unwrap();
        g.tool_calls += calls;
        g.out_of_scope_blocks += blocks;
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let g = self.inner.lock().unwrap();
        serde_json::json!({
            "page_views": g.page_views,
            "unique_visitors": g.unique.len(),
            "tunnel_links_opened": g.tunnel_links_opened,
            "tunnels_opened": g.tunnels_opened,
            "frames_relayed": g.frames_relayed,
            "tool_calls": g.tool_calls,
            "out_of_scope_blocks": g.out_of_scope_blocks,
        })
    }

    /// Best-effort write to the persistence path (no-op if none configured).
    pub fn flush(&self) {
        if let Some(p) = &self.path {
            let g = self.inner.lock().unwrap();
            if let Ok(s) = serde_json::to_string(&*g) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats() -> Stats {
        Stats::new(None, 0xABCD)
    }

    #[test]
    fn counts_increment() {
        let s = stats();
        s.tunnel_link_opened();
        s.tunnel_opened();
        s.frame_relayed();
        s.frame_relayed();
        s.report(5, 2);
        let snap = s.snapshot();
        assert_eq!(snap["tunnel_links_opened"], 1);
        assert_eq!(snap["tunnels_opened"], 1);
        assert_eq!(snap["frames_relayed"], 2);
        assert_eq!(snap["tool_calls"], 5);
        assert_eq!(snap["out_of_scope_blocks"], 2);
    }

    #[test]
    fn unique_visitors_dedupe_by_ip() {
        let s = stats();
        s.page_view("1.2.3.4");
        s.page_view("1.2.3.4"); // same IP -> still one unique
        s.page_view("5.6.7.8");
        let snap = s.snapshot();
        assert_eq!(snap["page_views"], 3, "every hit counts as a view");
        assert_eq!(snap["unique_visitors"], 2, "two distinct IPs");
    }

    #[test]
    fn raw_ip_is_never_stored() {
        // The persisted form must contain only hashes, never the IP string.
        let s = stats();
        s.page_view("203.0.113.99");
        let g = s.inner.lock().unwrap();
        let json = serde_json::to_string(&*g).unwrap();
        assert!(
            !json.contains("203.0.113.99"),
            "raw IP leaked into persistence: {json}"
        );
    }

    #[test]
    fn persists_and_reloads() {
        let dir = std::env::temp_dir();
        let path = dir.join("tunnel-stats-test.json");
        let _ = std::fs::remove_file(&path);
        {
            let s = Stats::new(Some(path.clone()), 7);
            s.page_view("9.9.9.9");
            s.tunnel_opened();
            s.flush();
        }
        // Reload: counters survive.
        let s2 = Stats::new(Some(path.clone()), 999 /* ignored, file has salt 7 */);
        let snap = s2.snapshot();
        assert_eq!(snap["unique_visitors"], 1);
        assert_eq!(snap["tunnels_opened"], 1);
        let _ = std::fs::remove_file(&path);
    }
}
