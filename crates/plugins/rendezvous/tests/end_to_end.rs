use std::time::Duration;
use tokio::task::JoinHandle;

fn rendezvous_bin() -> String {
    // Cargo injects the exact built-binary path for THIS crate's integration tests and
    // guarantees the bin is built before the test runs — more robust than counting `../`.
    env!("CARGO_BIN_EXE_tnls-rendezvous").to_string()
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

/// Full real-binary integration: the host tunnels a host-spawned `tnls-rendezvous share` seeder
/// (via `session::open`) and a `tnls-rendezvous get` subprocess pulls the bytes over loopback.
/// `#[ignore]`d because the *cross-process* loopback transfer does not complete in sandboxed CI
/// runners (the `get` subprocess can't connect to the seeder subprocess's loopback peer → times
/// out). The BitTorrent transfer mechanism is covered in CI by the in-process
/// `bittorrent::tests::seed_then_fetch_moves_bytes_over_loopback`, and the agent/scope path by
/// `tnls-tunnel`'s `read_succeeds_shell_refused`. Run on a real host with:
/// `cargo test -p tnls-rendezvous -- --ignored`.
#[tokio::test]
#[ignore = "cross-process loopback BitTorrent transfer doesn't complete in sandboxed CI; run with --ignored on a real host"]
async fn get_transfers_the_file_through_the_tunnel() {
    // Cargo builds tnls-rendezvous before this integration test runs; the path comes
    // from CARGO_BIN_EXE_tnls-rendezvous (see rendezvous_bin).
    let bin = rendezvous_bin();

    // local relay (Rocket)
    let port = spawn_relay().await;
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
