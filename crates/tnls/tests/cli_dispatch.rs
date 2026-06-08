use std::process::Command;

#[test]
fn plugins_lists_demo_and_rendezvous() {
    // `CARGO_BIN_EXE_tnls` is injected for THIS crate's integration tests and points at the
    // built host binary — honoring any custom CARGO_TARGET_DIR. The sibling plugin binaries
    // land in the same dir, so build them, then run `tnls plugins`.
    for p in ["tnls-demo", "tnls-rendezvous"] {
        assert!(Command::new(env!("CARGO"))
            .args(["build", "-p", p])
            .status()
            .unwrap()
            .success());
    }
    let out = Command::new(env!("CARGO_BIN_EXE_tnls"))
        .arg("plugins")
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&out.stdout);
    assert!(listed.contains("demo"), "got: {listed}");
    assert!(listed.contains("rendezvous"), "got: {listed}");
}
