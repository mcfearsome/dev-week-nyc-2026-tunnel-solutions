use crate::backplane::{Backplane, Delivery};
use crate::stats::Stats;
use bytes::Bytes;
use rocket::futures::{SinkExt, StreamExt};
use rocket_ws::stream::DuplexStream;
use rocket_ws::Message;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Agent,
    Viewer,
}

/// Fires `backplane.deregister` on **every** exit path of `run_side` after a successful
/// register — normal `Close`, stream end (`None`), a protocol/IO error (incl. an over-cap
/// frame), and the channel future being dropped/cancelled on client disconnect. Without this,
/// a half-open pairing would leak a backplane slot and the peer would never get its
/// `Delivery::Close`. `deregister` is `async`, so we spawn it (the backplane handle is `Arc`,
/// cheap to clone). Dropping our stored `tx` inside `deregister` also ends the writer pump.
struct DeregisterGuard {
    bp: Arc<dyn Backplane>,
    id: String,
    role: Role,
}

impl Drop for DeregisterGuard {
    fn drop(&mut self) {
        let bp = self.bp.clone();
        let id = std::mem::take(&mut self.id);
        let role = self.role;
        tokio::spawn(async move {
            bp.deregister(&id, role).await;
        });
    }
}

/// Bridge one socket (agent or viewer) to its peer through the backplane.
///
/// Opaque pass-through: inbound `Text`/`Binary` frames are forwarded VERBATIM as `Bytes` to the
/// peer (only counted via `stats.frame_relayed()`); their content is never parsed, validated,
/// mutated, or routed on. This is the security-critical dumb-relay invariant.
///
/// Returns `rocket_ws::result::Result<()>` per the `Channel` contract; we always return `Ok`
/// (a clean close) — teardown is handled by `DeregisterGuard`, not the return value.
pub async fn run_side(
    bp: Arc<dyn Backplane>,
    stats: Arc<Stats>,
    id: String,
    role: Role,
    stream: DuplexStream,
) -> rocket_ws::result::Result<()> {
    let (mut sink, mut stream) = stream.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Delivery>();

    if bp.register(&id, role, tx).await.is_err() {
        let _ = sink.send(Message::Close(None)).await; // Occupied or AtCapacity
        return Ok(()); // nothing registered → no slot to release
    }
    if matches!(role, Role::Agent) {
        stats.tunnel_opened();
    }

    // After a successful register, guarantee teardown on every exit path below.
    let _guard = DeregisterGuard {
        bp: bp.clone(),
        id: id.clone(),
        role,
    };

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

    // Reader loop: inbound → backplane. A frame exceeding the 1 MiB cap (set on the route via
    // `ws.config`) surfaces here as `Err(..)` from tungstenite, which tears the pairing down.
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(t)) => {
                stats.frame_relayed();
                bp.forward(&id, role, Bytes::from(t)).await;
            }
            Ok(Message::Binary(b)) => {
                stats.frame_relayed();
                bp.forward(&id, role, Bytes::from(b)).await;
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(_) => break, // protocol/IO error (incl. over-cap frame) → tear down
        }
    }

    writer.abort();
    Ok(())
    // `_guard` drops here → spawns `bp.deregister`, closing the peer and freeing the slot.
}
