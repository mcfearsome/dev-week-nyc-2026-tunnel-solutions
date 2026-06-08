use std::time::Duration;
use tokio::task::JoinHandle;

fn rendezvous_bin() -> String {
    // Cargo injects the exact built-binary path for THIS crate's integration tests and
    // guarantees the bin is built before the test runs — more robust than counting `../`.
    env!("CARGO_BIN_EXE_tnls-rendezvous").to_string()
}

/// Wait for the link `share` publishes via the pidfile. If the share task (the agent
/// driving the seeding child) dies first, surface ITS error instead of a misleading
/// "never published a link" timeout — a seeder bind failure is then diagnosable.
async fn read_link(relay_port: u16, share: &mut JoinHandle<anyhow::Result<()>>) -> String {
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
        // If the share task already exited, the seeder/tunnel failed — report why.
        if share.is_finished() {
            match share.await {
                Ok(Ok(())) => panic!("share task exited before publishing a link"),
                Ok(Err(e)) => panic!("share task failed before publishing a link: {e:?}"),
                Err(e) => panic!("share task panicked before publishing a link: {e}"),
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("share never published a link (timed out; share task still running)");
}

#[tokio::test]
async fn get_transfers_the_file_through_the_tunnel() {
    // Cargo builds tnls-rendezvous before this integration test runs; the path comes
    // from CARGO_BIN_EXE_tnls-rendezvous (see rendezvous_bin).
    let bin = rendezvous_bin();

    // local relay
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, relay::build_app()).await.unwrap();
    });
    let relay_url = format!("ws://127.0.0.1:{port}");

    // a temp file to "share"
    let dir = std::env::temp_dir().join(format!("tnls-rdv-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.bin");
    std::fs::write(&file, b"phase two end to end").unwrap();

    // Open the tunnel by driving session::open directly (the host owns the tunnel;
    // the rendezvous share subcommand just seeds + serves over stdio). TNLS_DISABLE_DHT
    // makes the seeder hermetic (no DHT, no UPnP) so the whole transfer runs over
    // loopback — deterministic, no fixed-port (UDP 6881) bind and no network/tracker.
    let bin_clone = bin.clone();
    let relay_url_clone = relay_url.clone();
    let file_str = file.to_string_lossy().into_owned();
    let mut share: JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
        tnls_tunnel::session::open(tnls_tunnel::session::OpenArgs {
            server: bin_clone,
            server_args: vec!["share".to_string(), file_str],
            ttl: Duration::from_secs(120),
            scope: vec!["list_shares".to_string(), "request_file".to_string()],
            relay: relay_url_clone.clone(),
            env: vec![
                ("TNLS_RELAY".to_string(), relay_url_clone),
                ("TNLS_DISABLE_DHT".to_string(), "1".to_string()),
            ],
        })
        .await
    });

    // Wait for the link that share publishes via the pidfile (surfacing share errors).
    let link = read_link(port, &mut share).await;
    assert!(
        link.contains("/t/") && link.contains('#'),
        "bad link: {link}"
    );

    // Spawn `tnls-rendezvous get <link> --out <out_dir>` as a subprocess. TNLS_DISABLE_DHT
    // makes its fetch hermetic too (no DHT/UPnP, loopback initial_peers from the advertised
    // request_file peers), so with both halves hermetic the bytes flow over loopback and the
    // subprocess exits promptly once the download completes.
    let out = dir.join("dl");
    std::fs::create_dir_all(&out).unwrap();
    let status = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(&bin)
            .args(["get", &link, "--out", out.to_str().unwrap()])
            .env("TNLS_DISABLE_DHT", "1")
            .status(),
    )
    .await
    .expect("get subprocess timed out")
    .expect("get subprocess failed to spawn");

    assert!(status.success(), "tnls-rendezvous get exited with {status}");

    // Assert the transferred bytes equal the source.
    let got = std::fs::read(out.join("doc.bin")).unwrap();
    assert_eq!(
        got, b"phase two end to end",
        "transferred bytes must match the source"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
