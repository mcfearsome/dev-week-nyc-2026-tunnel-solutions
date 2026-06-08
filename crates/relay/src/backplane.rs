use crate::pairing::Role;
use async_trait::async_trait;
use bytes::Bytes;
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
