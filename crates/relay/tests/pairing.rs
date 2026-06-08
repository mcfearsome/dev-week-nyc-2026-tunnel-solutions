use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as TMsg;

/// Launch a Rocket relay on an ephemeral port and return the bound port. Port 0 lets the OS pick;
/// an `on_liftoff` fairing reads the actual bound port (Rocket writes it into the config after
/// binding) back through a oneshot.
async fn spawn_relay() -> u16 {
    spawn_relay_with(relay::build_rocket()).await
}

async fn spawn_relay_with(rocket: rocket::Rocket<rocket::Build>) -> u16 {
    let (tx, rx) = tokio::sync::oneshot::channel::<u16>();
    let figment = rocket::Config::figment()
        .merge(("address", "127.0.0.1"))
        .merge(("port", 0));
    let rocket = rocket
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

/// Invariant gate item 2: the 1 MiB frame cap (`rocket_ws::Config` defaults to a dangerous
/// 64 MiB message / 16 MiB frame). A frame larger than 1 MiB sent by the viewer must be REJECTED
/// by the relay — never forwarded to the paired agent — and the over-cap socket torn down.
#[tokio::test]
async fn oversized_frame_is_rejected_not_forwarded() {
    let port = spawn_relay().await;
    let base = format!("ws://127.0.0.1:{port}");

    let (mut agent, _) = tokio_tungstenite::connect_async(format!("{base}/agent/cap-tid"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (mut viewer, _) = tokio_tungstenite::connect_async(format!("{base}/viewer/cap-tid"))
        .await
        .unwrap();

    // 2 MiB payload — comfortably over the 1 MiB cap. (The client's own read limits don't apply
    // to this outbound send; the relay enforces the cap on its receive side.)
    let huge = "x".repeat(2 * 1024 * 1024);
    let _ = viewer.send(TMsg::Text(huge.clone())).await;

    // The agent must NEVER receive the oversized frame. Its stream should instead end (None) as
    // the relay tears the pairing down, or yield nothing within the window. A forwarded 2 MiB
    // text would be an invariant breach.
    let outcome = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(item) = agent.next().await {
            match item {
                Ok(TMsg::Text(t)) if t.len() >= 1 << 20 => {
                    panic!(
                        "over-cap frame ({} bytes) was forwarded to the agent",
                        t.len()
                    );
                }
                Ok(TMsg::Text(t)) => {
                    panic!("unexpected text forwarded to agent: {} bytes", t.len())
                }
                Ok(TMsg::Binary(b)) => {
                    panic!("unexpected binary forwarded to agent: {} bytes", b.len())
                }
                Ok(_) => {}      // control frames (close/ping/pong) are fine
                Err(_) => break, // connection torn down — acceptable
            }
        }
    })
    .await;
    assert!(
        outcome.is_ok(),
        "agent socket should close after the relay rejects the over-cap frame (not hang)"
    );
}
