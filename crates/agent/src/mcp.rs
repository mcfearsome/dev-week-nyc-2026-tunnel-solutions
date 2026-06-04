use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

/// A spawned MCP server we speak JSON-RPC to over its stdio.
pub struct McpChild {
    child: Child,
    stdin: ChildStdin,
    pending: Pending,
    next_id: AtomicU64,
    pub tools: Vec<protocol::Tool>,
}

impl McpChild {
    /// Spawn, run the MCP handshake (`initialize` → `initialized` → `tools/list`),
    /// and cache the tool list. Fails loudly if the child can't be driven.
    pub async fn spawn(cmd: &str, args: &[String]) -> Result<Self> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning MCP server '{cmd}'"))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("child has no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("child has no stdout"))?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        // Background reader: dispatch responses to waiters by id.
        {
            let pending = pending.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                        if let Some(tx) = pending.lock().await.remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                    // No id => notification; ignore.
                }
            });
        }

        let mut me = McpChild { child, stdin, pending, next_id: AtomicU64::new(1), tools: vec![] };

        me.request("initialize", json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "tunnel", "version": "0.1.0" }
        })).await.context("MCP initialize")?;
        me.notify("notifications/initialized", json!({})).await?;

        let list = me.request("tools/list", json!({})).await.context("MCP tools/list")?;
        me.tools = serde_json::from_value(list.get("tools").cloned().unwrap_or(json!([])))
            .context("parsing tools/list")?;
        Ok(me)
    }

    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a request and await its `result` (surfacing any JSON-RPC `error`).
    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.alloc_id();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let line = format!("{}\n", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;

        let resp = tokio::time::timeout(Duration::from_secs(10), rx).await
            .map_err(|_| anyhow!("MCP request '{method}' timed out"))?
            .map_err(|_| anyhow!("MCP reader dropped before responding"))?;
        if let Some(e) = resp.get("error") {
            return Err(anyhow!("MCP error: {e}"));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let line = format!("{}\n", json!({ "jsonrpc": "2.0", "method": method, "params": params }));
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Invoke a tool; returns the MCP tool `result` object (`{ content: [...] }`).
    pub async fn call_tool(&mut self, name: &str, args: Value) -> Result<Value> {
        self.request("tools/call", json!({ "name": name, "arguments": args })).await
    }

    pub async fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}
