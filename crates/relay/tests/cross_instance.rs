use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as TMsg;

async fn serve(backplane: Arc<relay::backplane::LocalBackplane>) -> u16 {
    // Launch a Rocket relay sharing the injected backplane on an ephemeral port; an `on_liftoff`
    // fairing reports the OS-assigned port (Rocket writes it into config after binding) back.
    let (tx, rx) = tokio::sync::oneshot::channel::<u16>();
    let figment = rocket::Config::figment()
        .merge(("address", "127.0.0.1"))
        .merge(("port", 0));
    let rocket = relay::build_rocket_with(backplane)
        .configure(figment)
        .attach(rocket::fairing::AdHoc::on_liftoff("test port", move |r| {
            let port = r.config().port;
            Box::pin(async move {
                let _ = tx.send(port);
            })
        }));
    tokio::spawn(async move {
        let _ = rocket.launch().await;
    });
    rx.await.expect("relay should report its bound port")
}

#[tokio::test]
async fn frame_bridges_across_two_instances() {
    let shared = Arc::new(relay::backplane::LocalBackplane::new(256)); // stands in for Redis
    let a = serve(shared.clone()).await; // instance A
    let b = serve(shared.clone()).await; // instance B (same backplane)

    // agent connects to A, viewer to B
    let (mut agent, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{a}/agent/tid"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (mut viewer, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{b}/viewer/tid"))
            .await
            .unwrap();

    // viewer (on B) → agent (on A), bridged through the shared backplane
    viewer
        .send(TMsg::Text(r#"{"type":"hello"}"#.into()))
        .await
        .unwrap();
    let got = tokio::time::timeout(Duration::from_secs(2), agent.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(got.into_text().unwrap(), r#"{"type":"hello"}"#);
}
