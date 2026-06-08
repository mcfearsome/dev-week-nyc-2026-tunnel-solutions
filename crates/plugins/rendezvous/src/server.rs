use crate::bittorrent::SeedHandle;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

/// Static share details the tools hand out.
#[derive(Clone)]
pub struct ShareInfo {
    pub magnet: String,
    pub name: String,
    pub size: u64,
    pub peers: Vec<String>,
}

/// The rmcp server. `_seeder` keeps the BitTorrent session alive for exactly as long as
/// this server is served (drop => seeding stops). `SeedHandle` is not `Clone`, so it lives
/// behind an `Arc` to let the handler derive `Clone` (rmcp requires it).
#[derive(Clone)]
pub struct ShareServer {
    share: ShareInfo,
    _seeder: Arc<SeedHandle>,
    tool_router: ToolRouter<Self>,
}

/// Tools take no arguments; an empty params struct keeps the rmcp macro happy.
#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

#[tool_router]
impl ShareServer {
    pub fn new(share: ShareInfo, seeder: SeedHandle) -> Self {
        Self {
            share,
            _seeder: Arc::new(seeder),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List the file(s) shared in this tunnel.")]
    async fn list_shares(&self, _p: Parameters<NoArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} ({} bytes)",
            self.share.name, self.share.size
        ))]))
    }

    #[tool(description = "Get the magnet link (and peer hints) for the shared file.")]
    async fn request_file(&self, _p: Parameters<NoArgs>) -> Result<CallToolResult, ErrorData> {
        let body = serde_json::json!({ "magnet": self.share.magnet, "peers": self.share.peers });
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Share status.")]
    async fn status(&self, _p: Parameters<NoArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::success(vec![Content::text("seeding")]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ShareServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Capability-scoped file sending over BitTorrent.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_file_body_has_magnet_and_peers() {
        // Mirror what request_file emits, asserting the wire shape get.rs parses.
        let share = ShareInfo {
            magnet: "magnet:?xt=urn:btih:abc".into(),
            name: "f.bin".into(),
            size: 9,
            peers: vec!["127.0.0.1:6881".into()],
        };
        let body = serde_json::json!({ "magnet": share.magnet, "peers": share.peers }).to_string();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["magnet"], "magnet:?xt=urn:btih:abc");
        assert_eq!(v["peers"][0], "127.0.0.1:6881");
    }
}
