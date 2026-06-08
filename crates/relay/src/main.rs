use relay::backplane::{max_tunnels, RedisBackplane};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Bind from $PORT (Fly injects 8080); default 8787 keeps local dev/demo unchanged.
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("relay listening on http://{addr}");

    if let Ok(url) = std::env::var("REDIS_URL") {
        let bp = RedisBackplane::connect(&url, max_tunnels()).await?;
        println!("relay: using RedisBackplane");
        axum::serve(listener, relay::build_app_with(Arc::new(bp))).await?;
    } else {
        axum::serve(listener, relay::build_app()).await?;
    }

    Ok(())
}
