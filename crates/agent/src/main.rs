// PHASE 1 SCAFFOLD: connect as the agent role and echo frames. Replaced in Chunk 5.
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let relay = std::env::var("RELAY").unwrap_or_else(|_| "ws://127.0.0.1:8787".into());
    let id = "demo";
    let url = format!("{relay}/agent/{id}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await?;
    println!("agent connected → {url}");
    println!("link → http://127.0.0.1:8787/t/{id}");
    while let Some(Ok(msg)) = ws.next().await {
        if let Message::Text(t) = msg {
            println!("recv: {t}");
            ws.send(Message::Text(format!("echo:{t}"))).await?;
        }
    }
    println!("relay closed the socket");
    Ok(())
}
