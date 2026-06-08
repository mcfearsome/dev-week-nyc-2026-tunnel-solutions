use crate::stats::Stats;
use axum::extract::ws::WebSocket;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub type Tx = mpsc::UnboundedSender<axum::extract::ws::Message>;
pub type Registry = Arc<Mutex<HashMap<String, TunnelSlot>>>;

#[derive(Default)]
pub struct TunnelSlot {
    pub agent_tx: Option<Tx>,
    pub viewer_tx: Option<Tx>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Agent,
    Viewer,
}

use axum::extract::ws::Message;
use futures_util::{SinkExt, StreamExt};
use std::sync::OnceLock;

/// Max concurrent tunnels (distinct ids). Configurable via `MAX_TUNNELS` (default 256).
fn max_tunnels() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("MAX_TUNNELS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256)
    })
}

/// Reject a brand-new tunnel id once the registry is full; an existing id (e.g. a viewer
/// joining its already-registered agent) is always allowed through.
fn over_capacity(current_len: usize, id_present: bool, max: usize) -> bool {
    !id_present && current_len >= max
}

pub async fn run_side(reg: Registry, stats: Arc<Stats>, id: String, role: Role, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Register this side; reject a duplicate role for the same tunnel.
    {
        let mut map = reg.lock().await;
        // Global capacity cap: refuse a brand-new tunnel when the relay is full.
        if over_capacity(map.len(), map.contains_key(&id), max_tunnels()) {
            drop(map);
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
        let slot = map.entry(id.clone()).or_default();
        let occupied = match role {
            Role::Agent => slot.agent_tx.is_some(),
            Role::Viewer => slot.viewer_tx.is_some(),
        };
        if occupied {
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
        match role {
            Role::Agent => slot.agent_tx = Some(tx),
            Role::Viewer => slot.viewer_tx = Some(tx),
        }
    }

    // A registered agent means a new tunnel exists.
    if matches!(role, Role::Agent) {
        stats.tunnel_opened();
    }

    // Writer pump: our channel -> our socket.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Reader loop: inbound -> peer's channel.
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(_) | Message::Binary(_) => {
                stats.frame_relayed();
                let peer = {
                    let map = reg.lock().await;
                    map.get(&id).and_then(|slot| match role {
                        Role::Agent => slot.viewer_tx.clone(),
                        Role::Viewer => slot.agent_tx.clone(),
                    })
                };
                // No peer yet => drop (single viewer/agent, no buffering).
                if let Some(peer) = peer {
                    if peer.send(msg).is_err() {
                        break;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {} // axum auto-replies ping/pong
        }
    }

    // Teardown: remove the slot entirely; dropping the peer Tx cascades it shut.
    {
        let mut map = reg.lock().await;
        map.remove(&id);
    }
    writer.abort();
}

#[cfg(test)]
mod tests {
    use super::over_capacity;

    #[test]
    fn capacity_blocks_new_ids_when_full() {
        assert!(
            over_capacity(256, false, 256),
            "new id at capacity → reject"
        );
        assert!(over_capacity(300, false, 256), "over capacity → reject");
    }

    #[test]
    fn capacity_allows_existing_ids_and_room() {
        assert!(
            !over_capacity(256, true, 256),
            "existing id (viewer joining) → allow"
        );
        assert!(!over_capacity(10, false, 256), "room available → allow");
    }
}
