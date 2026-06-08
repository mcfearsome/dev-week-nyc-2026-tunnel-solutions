use anyhow::{anyhow, bail, Result};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::TokioChildProcess,
    RoleClient, ServiceExt,
};
use serde_json::{json, Map, Value};
use tnls_core::Tool;
use tokio::process::Command;

/// A spawned MCP server we speak JSON-RPC to over its stdio, via the official `rmcp` client.
/// The hand-rolled request/response plumbing is gone — rmcp owns the framing and handshake.
pub struct McpChild {
    service: RunningService<RoleClient, ()>,
    /// Tools advertised at handshake, in the wire shape the agent forwards to the viewer.
    pub tools: Vec<Tool>,
}

impl McpChild {
    /// Spawn `cmd args` (with `env`) as an MCP server, run the rmcp `initialize` handshake,
    /// and cache the advertised tool list. Fails loudly if the child can't be driven.
    pub async fn spawn(cmd: &str, args: &[String], env: &[(String, String)]) -> Result<Self> {
        let mut command = Command::new(cmd);
        command.args(args);
        for (k, v) in env {
            command.env(k, v);
        }
        let transport = TokioChildProcess::new(command)
            .map_err(|e| anyhow!("spawning MCP server '{cmd}': {e}"))?;
        let service =
            ().serve(transport)
                .await
                .map_err(|e| anyhow!("MCP handshake with '{cmd}': {e}"))?;
        let tools = service
            .list_all_tools()
            .await
            .map_err(|e| anyhow!("listing tools from '{cmd}': {e}"))?
            .into_iter()
            .map(convert_tool)
            .collect();
        Ok(Self { service, tools })
    }

    /// Call `tool` with JSON `args`. Returns `{ "content": [...] }` (what the agent forwards
    /// to the viewer as an `AgentFrame::Result`); a tool-reported error becomes `Err`, which
    /// `session.rs` maps to `ErrorCode::ToolError`.
    pub async fn call_tool(&self, tool: &str, args: Value) -> Result<Value> {
        let arguments = match args {
            Value::Object(m) => Some(m),
            Value::Null => None,
            other => Some(Map::from_iter([("value".to_string(), other)])),
        };
        let params = if let Some(args) = arguments {
            CallToolRequestParams::new(tool.to_string()).with_arguments(args)
        } else {
            CallToolRequestParams::new(tool.to_string())
        };
        let result: CallToolResult = self
            .service
            .call_tool(params)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        if result.is_error == Some(true) {
            bail!(
                "{}",
                serde_json::to_string(&result.content).unwrap_or_default()
            );
        }
        Ok(json!({ "content": result.content }))
    }

    /// Stop the child: close the rmcp service, which closes the transport; rmcp's
    /// `TokioChildProcess` cleans up the OS process on drop.
    pub async fn kill(&mut self) {
        let _ = self.service.close().await;
    }
}

/// `rmcp::model::Tool` → the wire `Tool` the viewer sees. Name and (camelCase) `inputSchema`
/// must be preserved verbatim — `filter_tools`/`decide_call` match on `name`, and the e2e
/// asserts the advertised order.
fn convert_tool(t: rmcp::model::Tool) -> Tool {
    Tool {
        name: t.name.to_string(),
        description: t.description.map(|d| d.to_string()),
        input_schema: Value::Object((*t.input_schema).clone()),
    }
}
