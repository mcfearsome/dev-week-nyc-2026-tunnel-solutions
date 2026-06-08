use crate::pairing::Role;
use async_trait::async_trait;
use bytes::Bytes;
use redis::{aio::ConnectionManager, AsyncCommands, Client, ExistenceCheck, SetExpiry, SetOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::{mpsc, Mutex};

/// What gets delivered to a local socket's writer pump.
pub enum Delivery {
    Frame(Bytes),
    Close,
}
pub type Tx = mpsc::UnboundedSender<Delivery>;

#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError {
    Occupied,
    AtCapacity,
}

#[async_trait]
pub trait Backplane: Send + Sync {
    /// Claim `role` for `id`; `tx` delivers frames/close to THIS side's socket.
    async fn register(&self, id: &str, role: Role, tx: Tx) -> Result<(), RegisterError>;
    /// Deliver `frame` to the PEER role.
    async fn forward(&self, id: &str, from: Role, frame: Bytes);
    /// Release this side; signal the peer to close.
    async fn deregister(&self, id: &str, role: Role);
}

pub fn max_tunnels() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("MAX_TUNNELS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256)
    })
}

#[derive(Default)]
struct Slot {
    agent: Option<Tx>,
    viewer: Option<Tx>,
}

impl Slot {
    fn get(&self, r: Role) -> &Option<Tx> {
        match r {
            Role::Agent => &self.agent,
            Role::Viewer => &self.viewer,
        }
    }
    fn set(&mut self, r: Role, tx: Option<Tx>) {
        match r {
            Role::Agent => self.agent = tx,
            Role::Viewer => self.viewer = tx,
        }
    }
    fn peer(&self, from: Role) -> Option<Tx> {
        self.get(match from {
            Role::Agent => Role::Viewer,
            Role::Viewer => Role::Agent,
        })
        .clone()
    }
    fn empty(&self) -> bool {
        self.agent.is_none() && self.viewer.is_none()
    }
}

pub struct LocalBackplane {
    map: Mutex<HashMap<String, Slot>>,
    max: usize,
}

impl LocalBackplane {
    pub fn new(max: usize) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            max,
        }
    }
}

#[async_trait]
impl Backplane for LocalBackplane {
    async fn register(&self, id: &str, role: Role, tx: Tx) -> Result<(), RegisterError> {
        let mut m = self.map.lock().await;
        if !m.contains_key(id) && m.len() >= self.max {
            return Err(RegisterError::AtCapacity);
        }
        let slot = m.entry(id.to_string()).or_default();
        if slot.get(role).is_some() {
            return Err(RegisterError::Occupied);
        }
        slot.set(role, Some(tx));
        Ok(())
    }

    async fn forward(&self, id: &str, from: Role, frame: Bytes) {
        let peer = { self.map.lock().await.get(id).and_then(|s| s.peer(from)) };
        if let Some(p) = peer {
            let _ = p.send(Delivery::Frame(frame));
        }
    }

    async fn deregister(&self, id: &str, role: Role) {
        let mut m = self.map.lock().await;
        if let Some(slot) = m.get_mut(id) {
            if let Some(p) = slot.peer(role) {
                let _ = p.send(Delivery::Close);
            }
            slot.set(role, None);
            if slot.empty() {
                m.remove(id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Redis wire tag types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(tag = "k")]
enum WireMsg {
    #[serde(rename = "f")]
    Frame { b: String },
    #[serde(rename = "c")]
    Close,
}

// ---------------------------------------------------------------------------
// RedisBackplane
// ---------------------------------------------------------------------------

/// TTL (seconds) for presence keys; heartbeat refreshes at TTL/2.
const PRESENCE_TTL: i64 = 30;

/// Shared dispatch table (Arc so it's shared between the backplane + subscriber task).
type DispatchMap = std::sync::Arc<Mutex<HashMap<String, Tx>>>;

pub struct RedisBackplane {
    cmd: ConnectionManager,
    /// Local sockets on THIS instance: `id → Slot`.
    local: Mutex<HashMap<String, Slot>>,
    /// Dispatch table shared with the subscriber task: `channel → Tx`.
    dispatch: DispatchMap,
    instance_id: String,
    max: usize,
}

impl RedisBackplane {
    /// Connect to Redis and spawn the subscriber + heartbeat tasks.
    pub async fn connect(url: &str, max: usize) -> anyhow::Result<Self> {
        let client = Client::open(url)?;
        let cmd = client.get_connection_manager().await?;

        // Generate a random instance ID.
        let mut raw = [0u8; 8];
        getrandom::getrandom(&mut raw).ok();
        let instance_id = format!("{:016x}", u64::from_le_bytes(raw));

        // Shared dispatch table: subscriber task and backplane both hold a clone.
        let dispatch: DispatchMap = std::sync::Arc::new(Mutex::new(HashMap::new()));

        // Subscriber task: PSUBSCRIBE tunnel:* and route incoming messages.
        {
            let mut pubsub = client.get_async_pubsub().await?;
            pubsub.psubscribe("tunnel:*").await?;
            let dispatch_sub = dispatch.clone();
            tokio::spawn(async move {
                use futures_util::StreamExt;
                let mut stream = pubsub.into_on_message();
                while let Some(msg) = stream.next().await {
                    let channel = msg.get_channel_name().to_string();
                    let payload = msg.get_payload_bytes().to_vec();
                    let d = dispatch_sub.lock().await;
                    if let Some(tx) = d.get(&channel) {
                        match serde_json::from_slice::<WireMsg>(&payload) {
                            Ok(WireMsg::Frame { b }) => {
                                let _ = tx.send(Delivery::Frame(Bytes::from(b)));
                            }
                            Ok(WireMsg::Close) => {
                                let _ = tx.send(Delivery::Close);
                            }
                            Err(_) => {} // malformed — drop
                        }
                    }
                }
            });
        }

        // Heartbeat task: refresh presence keys every TTL/2 seconds.
        // Dispatch keys are "tunnel:<id>:to_agent" / "tunnel:<id>:to_viewer";
        // presence keys are "tunnel:<id>:agent" / "tunnel:<id>:viewer".
        {
            let mut hb_cmd = cmd.clone();
            let hb_dispatch = dispatch.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs((PRESENCE_TTL / 2) as u64))
                        .await;
                    let keys: Vec<String> = hb_dispatch
                        .lock()
                        .await
                        .keys()
                        .map(|ch| ch.replacen(":to_", ":", 1))
                        .collect();
                    for key in keys {
                        let _: Result<bool, _> = hb_cmd.expire(&key, PRESENCE_TTL).await;
                    }
                }
            });
        }

        Ok(Self {
            cmd,
            local: Mutex::new(HashMap::new()),
            dispatch,
            instance_id,
            max,
        })
    }

    /// Redis presence key: `tunnel:<id>:<role>`.
    fn presence_key(id: &str, role: Role) -> String {
        format!("tunnel:{}:{}", id, Self::role_str(role))
    }

    /// Redis pub/sub channel delivering frames TO `role`: `tunnel:<id>:to_<role>`.
    fn channel_for(id: &str, role: Role) -> String {
        format!("tunnel:{}:to_{}", id, Self::role_str(role))
    }

    fn role_str(role: Role) -> &'static str {
        match role {
            Role::Agent => "agent",
            Role::Viewer => "viewer",
        }
    }

    fn peer_role(from: Role) -> Role {
        match from {
            Role::Agent => Role::Viewer,
            Role::Viewer => Role::Agent,
        }
    }
}

#[async_trait]
impl Backplane for RedisBackplane {
    async fn register(&self, id: &str, role: Role, tx: Tx) -> Result<(), RegisterError> {
        // Per-instance capacity check.
        {
            let local = self.local.lock().await;
            if !local.contains_key(id) && local.len() >= self.max {
                return Err(RegisterError::AtCapacity);
            }
        }

        // Global duplicate-role rejection via SET NX EX.
        let presence = Self::presence_key(id, role);
        let opts = SetOptions::default()
            .conditional_set(ExistenceCheck::NX)
            .with_expiration(SetExpiry::EX(PRESENCE_TTL as u64));
        let set_ok: bool = self
            .cmd
            .clone()
            .set_options(&presence, &self.instance_id, opts)
            .await
            .unwrap_or(false);
        if !set_ok {
            return Err(RegisterError::Occupied);
        }

        // Register locally for fast-path delivery.
        {
            let mut local = self.local.lock().await;
            let slot = local.entry(id.to_string()).or_default();
            slot.set(role, Some(tx.clone()));
        }
        // Register in the shared dispatch table for cross-instance delivery.
        let channel = Self::channel_for(id, role);
        self.dispatch.lock().await.insert(channel, tx);

        Ok(())
    }

    async fn forward(&self, id: &str, from: Role, frame: Bytes) {
        let peer = Self::peer_role(from);

        // Same-instance fast path.
        let local_tx = {
            let local = self.local.lock().await;
            local.get(id).and_then(|s| s.peer(from))
        };
        if let Some(tx) = local_tx {
            let _ = tx.send(Delivery::Frame(frame));
            return;
        }

        // Cross-instance: PUBLISH to the peer's channel.
        let channel = Self::channel_for(id, peer);
        if let Ok(payload) = serde_json::to_string(&WireMsg::Frame {
            b: String::from_utf8_lossy(&frame).into_owned(),
        }) {
            let _: Result<u64, _> = self.cmd.clone().publish(&channel, &payload).await;
        }
    }

    async fn deregister(&self, id: &str, role: Role) {
        let peer = Self::peer_role(role);
        let peer_channel = Self::channel_for(id, peer);

        // Remove presence key from Redis.
        let presence = Self::presence_key(id, role);
        let _: Result<u64, _> = self.cmd.clone().del(&presence).await;

        // Remove this role's dispatch entry.
        self.dispatch
            .lock()
            .await
            .remove(&Self::channel_for(id, role));

        // Close the peer — locally if present, else via PUBLISH.
        let local_peer_tx = {
            let mut local = self.local.lock().await;
            let peer_tx = local.get(id).and_then(|s| s.peer(role));
            if let Some(slot) = local.get_mut(id) {
                slot.set(role, None);
                if slot.empty() {
                    local.remove(id);
                }
            }
            peer_tx
        };

        if let Some(tx) = local_peer_tx {
            let _ = tx.send(Delivery::Close);
        } else if let Ok(payload) = serde_json::to_string(&WireMsg::Close) {
            let _: Result<u64, _> = self.cmd.clone().publish(&peer_channel, &payload).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch() -> (Tx, mpsc::UnboundedReceiver<Delivery>) {
        mpsc::unbounded_channel()
    }

    #[tokio::test]
    async fn forwards_to_peer_and_rejects_duplicate() {
        let bp = LocalBackplane::new(256);
        let (atx, _arx) = ch();
        let (vtx, mut vrx) = ch();
        bp.register("t", Role::Agent, atx).await.unwrap();
        bp.register("t", Role::Viewer, vtx).await.unwrap();
        // duplicate agent rejected
        let (a2, _) = ch();
        assert_eq!(
            bp.register("t", Role::Agent, a2).await,
            Err(RegisterError::Occupied)
        );
        // agent → viewer
        bp.forward("t", Role::Agent, Bytes::from_static(b"hi"))
            .await;
        match vrx.recv().await.unwrap() {
            Delivery::Frame(b) => assert_eq!(&b[..], b"hi"),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn deregister_closes_peer() {
        let bp = LocalBackplane::new(256);
        let (atx, mut arx) = ch();
        let (vtx, _vrx) = ch();
        bp.register("t", Role::Agent, atx).await.unwrap();
        bp.register("t", Role::Viewer, vtx).await.unwrap();
        bp.deregister("t", Role::Viewer).await;
        match arx.recv().await.unwrap() {
            Delivery::Close => {}
            _ => panic!("agent should get Close"),
        }
    }

    #[tokio::test]
    async fn capacity_rejects_new_when_full() {
        let bp = LocalBackplane::new(1);
        let (a, _) = ch();
        bp.register("t1", Role::Agent, a).await.unwrap();
        let (b, _) = ch();
        assert_eq!(
            bp.register("t2", Role::Agent, b).await,
            Err(RegisterError::AtCapacity)
        );
    }
}
