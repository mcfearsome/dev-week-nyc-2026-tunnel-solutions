use relay::backplane::{max_tunnels, Backplane, LocalBackplane, RedisBackplane};
use std::sync::Arc;

/// Build the `$PORT` figment: Fly injects `PORT=8080`, but Rocket reads `ROCKET_PORT`/
/// `ROCKET_ADDRESS`, so we merge address/port explicitly. Default 8787 keeps local dev/demo
/// unchanged; bind `0.0.0.0` so the container is reachable.
fn launch_figment() -> rocket::figment::Figment {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    rocket::Config::figment()
        .merge(("address", "0.0.0.0"))
        .merge(("port", port))
}

#[rocket::main]
async fn main() -> anyhow::Result<()> {
    // Pick the backplane: RedisBackplane (multi-instance) when REDIS_URL is set and reachable,
    // else LocalBackplane (single-instance). Degrade loudly, never silently take the relay down.
    let backplane: Arc<dyn Backplane> = if let Ok(url) = std::env::var("REDIS_URL") {
        match RedisBackplane::connect(&url, max_tunnels()).await {
            Ok(bp) => {
                println!("relay: using RedisBackplane (multi-instance)");
                Arc::new(bp)
            }
            Err(e) => {
                eprintln!(
                    "relay: REDIS_URL is set but Redis connect failed ({e}); \
                     falling back to LocalBackplane (single-instance). Fix Redis to enable scale-out."
                );
                Arc::new(LocalBackplane::new(max_tunnels()))
            }
        }
    } else {
        Arc::new(LocalBackplane::new(max_tunnels()))
    };

    // Start from the `$PORT` figment, then apply the shared relay wiring (routes/state/fairing).
    let rocket = relay::configure(rocket::custom(launch_figment()), backplane);
    rocket.launch().await?;
    Ok(())
}
