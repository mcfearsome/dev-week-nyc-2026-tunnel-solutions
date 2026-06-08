use serde_json::{json, Value};

pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "mcp-demo", "version": "0.1.0" }
    })
}

pub fn tool_list() -> Value {
    json!({
        "tools": [
            {
                "name": "read",
                "description": "Read a UTF-8 text file and return its contents.",
                "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
            },
            {
                "name": "shell",
                "description": "Run a shell command and return its output.",
                "inputSchema": { "type": "object", "properties": { "cmd": { "type": "string" } }, "required": ["cmd"] }
            }
        ]
    })
}

fn text_content(text: &str) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ] })
}

pub async fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "read" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("missing 'path'")?;
            let body = tokio::fs::read_to_string(path)
                .await
                .map_err(|e| format!("read failed: {e}"))?;
            Ok(text_content(&body))
        }
        "shell" => {
            let cmd = args
                .get("cmd")
                .and_then(|v| v.as_str())
                .ok_or("missing 'cmd'")?;
            let out = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .await
                .map_err(|e| format!("spawn failed: {e}"))?;
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.stderr.is_empty() {
                s.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            Ok(text_content(&s))
        }
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Route one JSON-RPC request. Returns None for notifications (no `id`).
pub async fn handle(req: &Value) -> Option<Value> {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = req.get("id").cloned();
    match method {
        "initialize" => Some(ok(id?, initialize_result())),
        "tools/list" => Some(ok(id?, tool_list())),
        "tools/call" => {
            let id = id?;
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match call_tool(name, &args).await {
                Ok(content) => Some(ok(id, content)),
                Err(e) => Some(rpc_err(id, -32000, &e)),
            }
        }
        "notifications/initialized" => None,
        _ => id.map(|id| rpc_err(id, -32601, "method not found")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_has_read_then_shell() {
        let tl = tool_list();
        let names: Vec<&str> = tl["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["read", "shell"]);
        assert_eq!(tl["tools"][0]["inputSchema"]["type"], "object");
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let resp = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .await
            .unwrap();
        assert_eq!(resp["result"]["serverInfo"]["name"], "mcp-demo");
        assert_eq!(resp["id"], 1);
    }

    #[tokio::test]
    async fn initialized_notification_yields_no_response() {
        assert!(
            handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn unknown_method_is_minus_32601() {
        let resp = handle(&json!({"jsonrpc":"2.0","id":9,"method":"bogus"}))
            .await
            .unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn shell_executes() {
        let out = call_tool("shell", &json!({"cmd":"echo hi"})).await.unwrap();
        assert_eq!(out["content"][0]["text"].as_str().unwrap().trim(), "hi");
    }

    #[tokio::test]
    async fn read_returns_file_contents() {
        let p = std::env::temp_dir().join("mcp-demo-read-test.txt");
        tokio::fs::write(&p, "hello-file").await.unwrap();
        let out = call_tool("read", &json!({"path": p.to_str().unwrap()}))
            .await
            .unwrap();
        assert_eq!(out["content"][0]["text"], "hello-file");
    }
}
