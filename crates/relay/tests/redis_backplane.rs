/// Integration test for RedisBackplane cross-instance delivery.
///
/// Skipped unless `REDIS_URL` is set in the environment.
/// Run manually with:
///   REDIS_URL=redis://127.0.0.1:6379 cargo test -p relay --test redis_backplane -- --ignored
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as TMsg;

async fn serve_redis(url: &str) -> u16 {
    let bp = relay::backplane::RedisBackplane::connect(url, 256)
        .await
        .expect("connect to Redis");
    let app = relay::build_app_with(Arc::new(bp));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(l, app).await.unwrap();
    });
    port
}

/// Two in-process relay instances connected to the same Redis.
/// Agent connects to instance A, viewer to instance B; verifies that a frame
/// sent by the viewer is received by the agent (cross-instance delivery).
#[tokio::test]
#[ignore = "requires REDIS_URL to be set"]
async fn cross_instance_frame_delivery() {
    let url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");

    let port_a = serve_redis(&url).await;
    let port_b = serve_redis(&url).await;

    // Agent on instance A.
    let (mut agent, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port_a}/agent/redis-test-tid"))
            .await
            .expect("agent connect");

    // Give the registration time to land in Redis before viewer connects.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Viewer on instance B.
    let (mut viewer, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port_b}/viewer/redis-test-tid"))
            .await
            .expect("viewer connect");

    // Viewer → agent (cross-instance).
    viewer
        .send(TMsg::Text(r#"{"type":"hello"}"#.into()))
        .await
        .unwrap();
    let got = tokio::time::timeout(Duration::from_secs(3), agent.next())
        .await
        .expect("timeout waiting for frame on agent")
        .unwrap()
        .unwrap();
    assert_eq!(got.into_text().unwrap(), r#"{"type":"hello"}"#);

    // Agent → viewer (cross-instance, reverse direction).
    agent
        .send(TMsg::Text(r#"{"type":"ack"}"#.into()))
        .await
        .unwrap();
    let got = tokio::time::timeout(Duration::from_secs(3), viewer.next())
        .await
        .expect("timeout waiting for frame on viewer")
        .unwrap()
        .unwrap();
    assert_eq!(got.into_text().unwrap(), r#"{"type":"ack"}"#);
}
