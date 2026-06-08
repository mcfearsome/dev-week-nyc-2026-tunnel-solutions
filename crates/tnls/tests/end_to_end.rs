use std::time::Duration;

fn build(pkg: &str) {
    let st = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", pkg])
        .status()
        .unwrap();
    assert!(st.success(), "building {pkg} failed");
}

/// Launch a Rocket relay on an OS-assigned port; an `on_liftoff` fairing reports the bound
/// port back once Rocket has bound the socket. Mirrors the relay crate's own test helper.
async fn spawn_relay() -> u16 {
    let (tx, rx) = tokio::sync::oneshot::channel::<u16>();
    let figment = rocket::Config::figment()
        .merge(("address", "127.0.0.1"))
        .merge(("port", 0));
    let rocket =
        relay::build_rocket()
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
async fn read_link(relay_port: u16) -> String {
    let path = std::env::temp_dir().join("tunnel-latest.pid");
    // Remove any stale pidfile from a previous run so we don't pick up the wrong link.
    let _ = std::fs::remove_file(&path);
    for _ in 0..60 {
        if let Ok(body) = std::fs::read_to_string(&path) {
            if let Some(link) = body.lines().nth(2) {
                // Verify the link belongs to OUR relay instance (port match).
                if link.contains(&format!(":{relay_port}/"))
                    && link.contains("/t/")
                    && link.contains('#')
                {
                    return link.to_string();
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("share never published a link");
}

#[tokio::test]
async fn get_transfers_the_file_through_the_tunnel() {
    build("tnls"); // the agent spawns target/debug/tnls mcp-serve

    // local relay
    let port = spawn_relay().await;
    let relay_url = format!("ws://127.0.0.1:{port}");

    // a temp file to "share"
    let dir = std::env::temp_dir().join(format!("tnls-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.bin");
    std::fs::write(&file, b"phase two end to end").unwrap();

    // run share in-process (it blocks; spawn it). It seeds + opens the tunnel.
    let tnls_bin = format!("{}/../../target/debug/tnls", env!("CARGO_MANIFEST_DIR"));
    let share = tnls::share::ShareArgs {
        path: file.to_string_lossy().into_owned(),
        relay: relay_url,
        ttl: Duration::from_secs(120),
        mcp_exe: Some(tnls_bin),
    };
    tokio::spawn(async move {
        let _ = tnls::share::run_share(share).await;
    });

    // get the link share published, then retrieve the magnet + peers through the tunnel
    let link = read_link(port).await;
    let rf = tokio::time::timeout(Duration::from_secs(20), tnls::get::retrieve_magnet(&link))
        .await
        .expect("retrieve timed out")
        .expect("retrieve failed");
    assert!(
        rf.magnet.starts_with("magnet:?xt=urn:btih:"),
        "got {}",
        rf.magnet
    );
    assert!(
        rf.peers.iter().any(|p| p.ip().is_loopback()),
        "must advertise a loopback peer: {:?}",
        rf.peers
    );

    // download via the advertised loopback peer → completes directly (no DHT).
    // The getter uses initial_peers=[127.0.0.1:P] + disable_dht=true, making this
    // hermetic: bytes flow entirely over loopback, no network/tracker needed.
    let out = dir.join("dl");
    std::fs::create_dir_all(&out).unwrap();
    let dl = tnls::bittorrent::fetch(
        &rf.magnet,
        &out,
        tnls::bittorrent::NetOpts {
            disable_dht: true,
            listen_port: None,
            enable_upnp: false,
            initial_peers: rf.peers,
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(40), dl.wait())
        .await
        .expect("download timed out")
        .expect("download errored");
    let got = std::fs::read(out.join("doc.bin")).unwrap();
    assert_eq!(
        got, b"phase two end to end",
        "transferred bytes must match the source"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
