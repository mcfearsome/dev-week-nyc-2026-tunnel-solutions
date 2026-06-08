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
    axum::serve(listener, relay::build_app()).await?;
    Ok(())
}
