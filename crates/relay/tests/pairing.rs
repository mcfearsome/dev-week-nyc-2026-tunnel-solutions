use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as TMsg;

async fn spawn_relay() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, relay::build_app()).await.unwrap();
    });
    port
}

#[tokio::test]
async fn frames_forward_both_ways() {
    let port = spawn_relay().await;
    let base = format!("ws://127.0.0.1:{port}");

    let (mut agent, _) = tokio_tungstenite::connect_async(format!("{base}/agent/tid"))
        .await
        .unwrap();
    // Give the agent's upgrade task time to register before the viewer sends.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (mut viewer, _) = tokio_tungstenite::connect_async(format!("{base}/viewer/tid"))
        .await
        .unwrap();

    // viewer -> agent
    viewer
        .send(TMsg::Text(r#"{"type":"hello","token":"x"}"#.into()))
        .await
        .unwrap();
    let got = agent.next().await.unwrap().unwrap();
    assert_eq!(got.into_text().unwrap(), r#"{"type":"hello","token":"x"}"#);

    // agent -> viewer
    agent
        .send(TMsg::Text(r#"{"type":"tools","tools":[]}"#.into()))
        .await
        .unwrap();
    let got = viewer.next().await.unwrap().unwrap();
    assert_eq!(got.into_text().unwrap(), r#"{"type":"tools","tools":[]}"#);
}

#[tokio::test]
async fn agent_disconnect_closes_viewer() {
    let port = spawn_relay().await;
    let base = format!("ws://127.0.0.1:{port}");
    let (agent, _) = tokio_tungstenite::connect_async(format!("{base}/agent/tid2"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (mut viewer, _) = tokio_tungstenite::connect_async(format!("{base}/viewer/tid2"))
        .await
        .unwrap();

    drop(agent); // agent leaves
                 // viewer's stream should end (None) once the relay tears down the pairing.
    let ended = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(_)) = viewer.next().await {}
    })
    .await;
    assert!(
        ended.is_ok(),
        "viewer socket should close after agent disconnect"
    );
}
