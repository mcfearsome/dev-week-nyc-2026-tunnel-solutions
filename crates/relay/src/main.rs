#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = "127.0.0.1:8787";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("relay listening on http://{addr}");
    axum::serve(listener, relay::build_app()).await?;
    Ok(())
}
