use serde_json::json;

fn mcp_demo_bin() -> String {
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "mcp-demo"])
        .status()
        .expect("cargo build -p mcp-demo");
    assert!(status.success(), "building mcp-demo failed");
    format!("{}/../../target/debug/mcp-demo", env!("CARGO_MANIFEST_DIR"))
}

#[tokio::test]
async fn handshake_lists_tools_and_calls_shell() {
    let mut child = agent::mcp::McpChild::spawn(&mcp_demo_bin(), &[]).await.unwrap();

    let names: Vec<String> = child.tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, vec!["read".to_string(), "shell".to_string()]);

    let out = child.call_tool("shell", json!({"cmd":"echo bridged"})).await.unwrap();
    assert_eq!(out["content"][0]["text"].as_str().unwrap().trim(), "bridged");

    child.kill().await;
}
