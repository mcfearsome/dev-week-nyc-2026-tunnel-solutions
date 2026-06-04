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

// Real implementation lands in Task 2.2. Stub closes the socket immediately.
pub async fn run_side(_reg: Registry, _id: String, _role: Role, _socket: WebSocket) {}
