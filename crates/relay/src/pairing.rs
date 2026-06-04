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

pub async fn run_side(reg: Registry, id: String, role: Role, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Register this side; reject a duplicate role for the same tunnel.
    {
        let mut map = reg.lock().await;
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
