use crate::backplane::{Backplane, Delivery};
use crate::stats::Stats;
use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Agent,
    Viewer,
}

pub async fn run_side(
    bp: Arc<dyn Backplane>,
    stats: Arc<Stats>,
    id: String,
    role: Role,
    socket: WebSocket,
) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Delivery>();

    if bp.register(&id, role, tx).await.is_err() {
        let _ = sink.send(Message::Close(None)).await; // Occupied or AtCapacity
        return;
    }
    if matches!(role, Role::Agent) {
        stats.tunnel_opened();
    }

    // Writer pump: Frame → socket; Close → shut down.
    let writer = tokio::spawn(async move {
        while let Some(d) = rx.recv().await {
            match d {
                Delivery::Frame(b) => {
                    if sink
                        .send(Message::Text(String::from_utf8_lossy(&b).into_owned()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Delivery::Close => break,
            }
        }
        let _ = sink.close().await;
    });

    // Reader loop: inbound → backplane.
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(t) => {
                stats.frame_relayed();
                bp.forward(&id, role, Bytes::from(t)).await;
            }
            Message::Binary(b) => {
                stats.frame_relayed();
                bp.forward(&id, role, Bytes::from(b)).await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    bp.deregister(&id, role).await;
    writer.abort();
}
