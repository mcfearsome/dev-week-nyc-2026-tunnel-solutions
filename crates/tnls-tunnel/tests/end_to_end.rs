use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as TMsg;

fn build(pkg: &str) {
    let st = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", pkg])
        .status()
        .unwrap();
    assert!(st.success(), "building {pkg} failed");
}

async fn next_json<S>(s: &mut S) -> Value
where
    S: StreamExt<Item = Result<TMsg, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), s.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let TMsg::Text(t) = m {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

async fn read_link(expected_relay_port: u16) -> (String, String) {
    let path = std::env::temp_dir().join("tunnel-latest.pid");
    for _ in 0..50 {
        if let Ok(body) = std::fs::read_to_string(&path) {
            let mut l = body.lines();
            let _pid = l.next();
            let id = l.next().unwrap_or("").to_string();
            let link = l.next().unwrap_or("").to_string();
            // Verify this pidfile belongs to our relay instance (port check).
            if link.contains(&format!(":{expected_relay_port}/")) {
                if let Some((_, token)) = link.split_once('#') {
                    if !id.is_empty() && !token.is_empty() {
                        return (id, token.to_string());
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("agent never published a pidfile/link");
}

#[tokio::test]
async fn read_succeeds_shell_refused() {
    build("mcp-demo");

    // relay on an ephemeral port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, relay::build_app()).await.unwrap();
    });
    let relay_url = format!("ws://127.0.0.1:{port}");

    // tunnel open --scope read
    let mcp = format!("{}/../../target/debug/mcp-demo", env!("CARGO_MANIFEST_DIR"));
    let open = tnls_tunnel::session::OpenArgs {
        server: mcp,
        server_args: vec![],
        ttl: Duration::from_secs(120),
        scope: vec!["read".into()],
        relay: relay_url.clone(),
        env: vec![],
    };
    tokio::spawn(async move {
        tnls_tunnel::session::open(open).await.unwrap();
    });

    let (id, token) = read_link(port).await;
    let (mut v, _) = tokio_tungstenite::connect_async(format!("{relay_url}/viewer/{id}"))
        .await
        .unwrap();

    // hello -> ready + filtered tools
    v.send(TMsg::Text(
        json!({"type":"hello","token":token}).to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(next_json(&mut v).await["type"], "ready");
    let tools = next_json(&mut v).await;
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["read"], "shell must be filtered from the list");

    // in-scope read succeeds
    let f = std::env::temp_dir().join("tunnel-e2e-read.txt");
    std::fs::write(&f, "secret-handshake").unwrap();
    v.send(TMsg::Text(
        json!({"type":"call","id":1,"tool":"read","args":{"path":f.to_str().unwrap()}}).to_string(),
    ))
    .await
    .unwrap();
    let res = next_json(&mut v).await;
    assert_eq!(res["type"], "result");
    assert_eq!(res["content"][0]["text"], "secret-handshake");

    // out-of-scope shell refused agent-side
    v.send(TMsg::Text(
        json!({"type":"call","id":2,"tool":"shell","args":{"cmd":"echo pwned"}}).to_string(),
    ))
    .await
    .unwrap();
    let refused = next_json(&mut v).await;
    assert_eq!(refused["type"], "error");
    assert_eq!(refused["code"], "out_of_scope");
    assert_eq!(refused["tool"], "shell");
}
