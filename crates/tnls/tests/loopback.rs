use std::time::Duration;
use tnls::bittorrent::{create_share, fetch, seed, NetOpts};

#[tokio::test]
async fn seed_then_fetch_moves_bytes_over_loopback() {
    // unique temp dirs
    let base = std::env::temp_dir().join(format!("tnls-loop-{}", std::process::id()));
    let seed_dir = base.join("seed");
    let fetch_dir = base.join("fetch");
    std::fs::create_dir_all(&seed_dir).unwrap();
    std::fs::create_dir_all(&fetch_dir).unwrap();

    // a ~512KB file with recognizable content
    let payload: Vec<u8> = (0..512 * 1024).map(|i| (i % 251) as u8).collect();
    let src = seed_dir.join("payload.bin");
    std::fs::write(&src, &payload).unwrap();

    // seed: DHT off, fixed loopback port
    let port = 47_111u16;
    let meta = create_share(&src, &[]).await.unwrap();
    let _seeder = seed(&meta, &seed_dir, NetOpts {
        disable_dht: true, listen_port: Some(port), enable_upnp: false, initial_peers: vec![],
    }).await.unwrap();

    // fetch: DHT off, connect straight to the seeder via initial_peers (metadata flows over BEP-9)
    let peer = format!("127.0.0.1:{port}").parse().unwrap();
    let dl = fetch(&meta.magnet, &fetch_dir, NetOpts {
        disable_dht: true, listen_port: None, enable_upnp: false, initial_peers: vec![peer],
    }).await.unwrap();

    // wait (bounded) for completion
    tokio::time::timeout(Duration::from_secs(30), dl.wait())
        .await
        .expect("download timed out — peers didn't connect")
        .expect("download errored");

    let got = std::fs::read(fetch_dir.join("payload.bin")).expect("fetched file missing");
    assert_eq!(got, payload, "fetched bytes must equal the source");

    let _ = std::fs::remove_dir_all(&base);
}
