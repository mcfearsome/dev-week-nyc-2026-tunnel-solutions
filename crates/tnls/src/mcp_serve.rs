use serde_json::{json, Value};

/// Static share details handed to mcp-serve on spawn.
#[derive(Clone)]
pub struct ShareInfo {
    pub magnet: String,
    pub name: String,
    pub size: u64,
}

pub fn tool_list() -> Value {
    json!({ "tools": [
        { "name": "list_shares", "description": "List the file(s) shared in this tunnel.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "request_file", "description": "Get the magnet link for the shared file.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "status", "description": "Share status.",
          "inputSchema": { "type": "object", "properties": {} } },
    ]})
}

fn text(s: &str) -> Value { json!({ "content": [ { "type": "text", "text": s } ] }) }

/// Pure dispatch. Returns None for notifications.
pub fn handle(req: &Value, share: &ShareInfo) -> Option<Value> {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = req.get("id").cloned();
    let ok = |id: Value, r: Value| json!({ "jsonrpc":"2.0","id":id,"result":r });
    match method {
        "initialize" => Some(ok(id?, json!({
            "protocolVersion":"2024-11-05","capabilities":{"tools":{}},
            "serverInfo":{"name":"tnls","version":"0.1.0"} }))),
        "tools/list" => Some(ok(id?, tool_list())),
        "tools/call" => {
            let id = id?;
            let name = req.get("params").and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or("");
            let content = match name {
                "list_shares" => text(&format!("{} ({} bytes)", share.name, share.size)),
                "request_file" => text(&share.magnet),
                "status" => text("seeding (live stats not available in v1)"),
                other => return Some(json!({ "jsonrpc":"2.0","id":id,
                    "error":{"code":-32601,"message":format!("unknown tool {other}")} })),
            };
            Some(ok(id, content))
        }
        "notifications/initialized" => None,
        _ => id.map(|id| json!({ "jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"} })),
    }
}

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Run the MCP stdio server until stdin closes.
pub async fn serve(share: ShareInfo) -> anyhow::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() { continue; }
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(resp) = handle(&req, &share) {
            let mut out = serde_json::to_string(&resp)?; out.push('\n');
            stdout.write_all(out.as_bytes()).await?; stdout.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn share() -> ShareInfo { ShareInfo { magnet: "magnet:?xt=urn:btih:abc".into(), name: "f.bin".into(), size: 9 } }

    #[test]
    fn request_file_returns_the_magnet() {
        let resp = handle(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"request_file"}}), &share()).unwrap();
        assert_eq!(resp["result"]["content"][0]["text"], "magnet:?xt=urn:btih:abc");
    }
    #[test]
    fn tools_list_has_request_file() {
        let list = tool_list();
        let names: Vec<&str> = list["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"request_file"));
    }
    #[test]
    fn unknown_tool_errors() {
        let resp = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nope"}}), &share()).unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }
}
