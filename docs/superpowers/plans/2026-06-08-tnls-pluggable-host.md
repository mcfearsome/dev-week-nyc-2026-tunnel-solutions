# tnls Pluggable Host — Implementation Plan (Spec 1)

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tnls` the main binary — a pluggable host that dispatches to subprocess `tnls-<name>` plugins, with MCP on both stdio ends built on the official `rmcp` SDK; file-sharing becomes the `rendezvous` plugin and `mcp-demo` becomes the `demo` plugin.

**Architecture:** A thin host (`tnls`) owns tunnel creation (`tnls-tunnel`'s `session::open` — the sole capability **agent**) and dispatches via clap `external_subcommand` to `tnls-<name>` binaries discovered next to itself. Each plugin emits a static `describe` manifest (`tnls-plugin` types) declaring which commands are tunneled vs. plain. Plugins are `rmcp` servers; the agent's `McpChild` is an `rmcp` client over `TokioChildProcess`. The relay is untouched (that is Spec 2).

**Tech Stack:** Rust (edition 2021), `rmcp` 1.7.0 (+`schemars`), `clap` 4 (derive + `external_subcommand`), `tokio`, `librqbit =9.0.0-rc.0` (rendezvous only), `rustls` (ring provider).

**Spec:** `docs/superpowers/specs/2026-06-08-tnls-pluggable-host-design.md` — read it before starting. This plan implements that spec; where this plan and the spec disagree, the spec wins (open an issue).

**Conventions for the implementer:**
- Run every command from the **workspace root** unless told otherwise.
- "Green" means `cargo build --workspace` **and** `cargo test --workspace` both succeed. The CI gates are stricter and must also pass before a chunk is "done": `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`.
- Commit at each step that says "Commit." Small commits are the point.
- The crate **package** name uses hyphens (`tnls-core`); the **import path** uses underscores (`tnls_core`). Both appear below — do not mix them up.

---

## End-state file map

This is where things land after all chunks. Build toward it; do not create it all at once.

```
Cargo.toml                      workspace members: tnls-core, tnls-tunnel, tnls-plugin, tnls,
                                plugins/rendezvous, plugins/demo, relay
crates/
  tnls-core/                    (rename of tunnel-locker-core) — internals UNCHANGED
    src/{lib,token,scope,frame,ttl}.rs
  tnls-tunnel/                  (rename of tunnel-locker) — LIBRARY ONLY after Chunk 5
    src/lib.rs                  pub mod session; pub mod mcp; pub mod pidfile;
    src/session.rs              OpenArgs { + env }; open(); close(); banner says "tnls close"
    src/mcp.rs                  McpChild — an rmcp client over TokioChildProcess (Chunk 2)
    src/pidfile.rs              UNCHANGED
    (src/main.rs + src/cli.rs   the `tunnel` bin — kept through Chunk 4, deleted in Chunk 5)
  tnls-plugin/                  NEW — the describe contract, shared host<->plugins
    src/lib.rs                  Manifest, Command, Tunnel (serde) + tiny helpers
  tnls/                         the HOST binary (bin: tnls)
    src/main.rs                 install rustls provider; parse Cli; route
    src/cli.rs                  Cli: global --relay/--ttl/--scope; Open/Close/Plugins; External catch-all; parse_scope
    src/discover.rs             resolve tnls-<name> next to current_exe then $PATH; list plugins
    src/dispatch.rs             run `<plugin> describe`; route tunneled -> session::open, plain -> exec
  plugins/
    demo/                       bin: tnls-demo  (rename of mcp-demo, now an rmcp server)
      src/main.rs               clap: serve | describe ; runs the rmcp server
      src/server.rs             Demo: rmcp ServerHandler with read + shell tools
    rendezvous/                 bin: tnls-rendezvous  (absorbs the file-sharing surface)
      src/main.rs               clap: share | get | seed | fetch | describe ; install rustls provider
      src/server.rs             ShareServer: rmcp ServerHandler (list_shares/request_file/status) + _seeder field
      src/bittorrent.rs         (moved from crates/tnls) seed/fetch/create_share/NetOpts
      src/get.rs                (moved) headless viewer client (parse_link, retrieve, run_get)
      src/share.rs              seed + gather_peers + serve  (self-contained; no session::open call)
  relay/                        UNCHANGED in this plan (Spec 2)
viewer/                         UNCHANGED
```

**Why `crates/tnls` is gutted, not deleted:** the existing `crates/tnls` directory is reused as the host. Its file-sharing modules (`bittorrent.rs`, `get.rs`, `share.rs`, `mcp_serve.rs`) move to `plugins/rendezvous` (Chunk 4); what remains is replaced by the host (Chunk 5).

---

## Chunk overview

Each chunk ends green and is committed. Order preserves "green at each step" and matches spec §6.

1. **Chunk 1 — Crate renames + workspace wiring.** Pure mechanical rename (`tunnel-locker-core`→`tnls-core`, `tunnel-locker`→`tnls-tunnel`). The `tunnel` bin is **kept** for now. No behavior change.
2. **Chunk 2 — rmcp `McpChild` client.** Rewrite `tnls-tunnel/src/mcp.rs` onto an `rmcp` client over `TokioChildProcess`; add `OpenArgs.env`; fix the banner string. Existing agent-bridge tests prove it against the still-hand-rolled `mcp-demo`.
3. **Chunk 3 — `tnls-plugin` crate + `demo` plugin.** Create the `describe` contract crate; rebuild `mcp-demo` as the `tnls-demo` rmcp server with a `describe` command. First plugin end-to-end.
4. **Chunk 4 — `rendezvous` plugin.** Move the file-sharing surface into `plugins/rendezvous`; make `mcp_serve` an rmcp `ShareServer`; rewrite `share` to self-seed-and-serve; add `describe`.
5. **Chunk 5 — the `tnls` host.** Build `cli`/`discover`/`dispatch`; add `open`/`close`/`plugins`; install the rustls provider; **delete the `tunnel` bin**; relocate the agent-bridge e2e to `tnls-tunnel/tests`; wire the workspace.
6. **Chunk 6 — docs, scripts, final gate.** Update `demo.sh`, `README.md`, the `ci.yml` comment, `CLAUDE.md`; run the full CI gate incl. `cargo audit`.

---

## Chunk 1: Crate renames + workspace wiring

**Goal:** Rename the two `tunnel-locker*` crates to the `tnls-*` family with **zero behavior change**. Each crate's rename lands as **one self-contained commit** — directory move + `Cargo.toml` edit + workspace-member update + every importer edit together — so the workspace compiles at every commit (two commits total, one per crate). The existing test suite is the safety net (no new tests).

**Files:**
- Move: `crates/tunnel-locker-core/` → `crates/tnls-core/`; `crates/tunnel-locker/` → `crates/tnls-tunnel/`
- Modify: `Cargo.toml` (workspace members), the two moved `Cargo.toml`s, and every file importing `tunnel_locker_core` / `tunnel_locker`

### Task 1.1: Rename `tunnel-locker-core` → `tnls-core`

- [ ] **Step 1: Move the crate directory**

```bash
git mv crates/tunnel-locker-core crates/tnls-core
```

- [ ] **Step 2: Rename the package**

In `crates/tnls-core/Cargo.toml`, change the package name:

```toml
[package]
name = "tnls-core"
# (leave version, edition, etc. unchanged)
```

- [ ] **Step 3: Update the workspace member path**

In the root `Cargo.toml`, in `[workspace] members`, replace `"crates/tunnel-locker-core"` with `"crates/tnls-core"`.

- [ ] **Step 4: Update every dependant's `Cargo.toml`**

These crates depend on the core crate — update the dependency key/path in each (the tunnel-locker directory is still at its **old** path here; it gets renamed in Task 1.2):
- `crates/tunnel-locker/Cargo.toml`: `tunnel-locker-core = { path = "../tunnel-locker-core", version = "0.1.0" }` → `tnls-core = { path = "../tnls-core", version = "0.1.0" }`
- `crates/tnls/Cargo.toml`: `tunnel-locker-core = { path = "../tunnel-locker-core" }` → `tnls-core = { path = "../tnls-core" }`

- [ ] **Step 5: Update every `tunnel_locker_core::` import path**

Find them all (note the underscore form for code, hyphen form for manifests):

Run: `rg -l 'tunnel_locker_core' -g '*.rs'`

Expected — **5 files** (the tunnel-locker dir is still at its old path; renamed in Task 1.2): `crates/tunnel-locker/src/session.rs`, `crates/tunnel-locker/src/mcp.rs`, `crates/tunnel-locker/src/main.rs` (the `tunnel_locker_core::parse_ttl` call — easy to miss), `crates/tnls/src/get.rs`, `crates/tnls/src/main.rs`. Replace `tunnel_locker_core` with `tnls_core` in each (imports and fully-qualified paths like `tunnel_locker_core::parse_ttl`). If a 6th file appears in the `rg` output, fix it too.

- [ ] **Step 6: Build to verify the rename compiles**

Run: `cargo build --workspace`
Expected: builds clean. If "unresolved import `tunnel_locker_core`", you missed a reference — re-run the `rg` from Step 5.

- [ ] **Step 7: Run the full suite (unchanged behavior)**

Run: `cargo test --workspace`
Expected: PASS — same tests as before, nothing dropped.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: rename tunnel-locker-core -> tnls-core"
```

### Task 1.2: Rename `tunnel-locker` → `tnls-tunnel` (keep the `tunnel` bin)

The `tunnel` binary is intentionally **kept** here (its `src/main.rs` + `src/cli.rs` stay) — it is deleted in Chunk 5 when `tnls open` replaces it. No test execs it, so keeping it is free.

- [ ] **Step 1: Move the crate directory**

```bash
git mv crates/tunnel-locker crates/tnls-tunnel
```

- [ ] **Step 2: Rename the package, keep the bin**

In `crates/tnls-tunnel/Cargo.toml`:

```toml
[package]
name = "tnls-tunnel"

[[bin]]
name = "tunnel"      # unchanged for now; removed in Chunk 5
path = "src/main.rs"
```

- [ ] **Step 3: Update the workspace member path**

Root `Cargo.toml`: replace `"crates/tunnel-locker"` with `"crates/tnls-tunnel"`.

- [ ] **Step 4: Update dependants' `Cargo.toml`**

- `crates/tnls/Cargo.toml`: `tunnel-locker = { path = "../tunnel-locker" }` → `tnls-tunnel = { path = "../tnls-tunnel" }`

- [ ] **Step 5: Update every `tunnel_locker::` import path**

Run: `rg -lP 'tunnel_locker(?!_core)' -g '*.rs'` (PCRE lookahead: the crate, not the core crate)

Expected files: `crates/tnls/src/share.rs` (`tunnel_locker::session::open` → `tnls_tunnel::session::open`), `crates/tnls-tunnel/src/main.rs` (`tunnel_locker::{cli, session}` → `tnls_tunnel::{cli, session}`), `crates/tnls-tunnel/tests/*.rs` (`tunnel_locker::session::...` → `tnls_tunnel::session::...`). Replace `tunnel_locker` with `tnls_tunnel` in each.

- [ ] **Step 6: Build**

Run: `cargo build --workspace`
Expected: clean (the `tunnel` bin still builds — that is fine).

- [ ] **Step 7: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 8: Confirm the CI gate locally**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: no diffs, no warnings. (If `--locked` complains, run `cargo build` once to refresh `Cargo.lock`, then re-run.)

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor: rename tunnel-locker -> tnls-tunnel (bin kept transiently)"
```

**End of Chunk 1:** the workspace is the `tnls-*` family, all tests green, the `tunnel` bin still works. No behavior changed.

---

## Chunk 2: rmcp `McpChild` client

**Goal:** Replace the hand-rolled JSON-RPC client in `tnls-tunnel/src/mcp.rs` with a client built on the official `rmcp` SDK over `TokioChildProcess`, **keeping `McpChild`'s public surface stable** so `session.rs`'s enforcement loop is barely touched. Add `OpenArgs.env` (so the host can pass `TNLS_RELAY` to a child) and fix the banner string. The existing agent-bridge tests are the regression net: they spawn the still-hand-rolled `mcp-demo` and must stay green, proving the rmcp client speaks to any protocol-compliant server (spec §4.2, §6.2).

> **rmcp 1.7.0 note for the implementer:** the API names below match `rmcp` 1.7.0 as documented at `docs.rs/rmcp/1.7.0` (`ServiceExt::serve`, `RunningService<RoleClient, ()>`, `service.list_all_tools()`, `service.call_tool(CallToolRequestParam { name, arguments })`, `CallToolResult { content, is_error }`, `rmcp::model::Tool { name, description, input_schema }`, `transport::TokioChildProcess::new`). If a path or method name differs in the exact patch release you resolve, fix the import — the *shapes* are correct. Keep `McpChild`'s three public items (`spawn`, the `tools` field, `call_tool`, `kill`) exactly as specified so `session.rs` compiles unchanged except for the one `spawn` call.

**Files:**
- Modify: `crates/tnls-tunnel/Cargo.toml` (add `rmcp`)
- Rewrite: `crates/tnls-tunnel/src/mcp.rs`
- Modify: `crates/tnls-tunnel/src/session.rs` (`OpenArgs.env`; pass env to `spawn`; banner string)
- Modify (add `env: vec![]` to `OpenArgs` literals): `crates/tnls-tunnel/src/main.rs`, `crates/tnls/src/share.rs`, `crates/tnls-tunnel/tests/end_to_end.rs`
- Modify (if it calls `McpChild::spawn` or builds `OpenArgs`): `crates/tnls-tunnel/tests/mcp_bridge.rs`

### Task 2.1: Add the `rmcp` dependency

- [ ] **Step 1: Add `rmcp` to `tnls-tunnel`**

In `crates/tnls-tunnel/Cargo.toml`, under `[dependencies]`:

```toml
rmcp = { version = "1.7", features = ["client", "transport-child-process"] }
```

- [ ] **Step 2: Verify it resolves**

Run: `cargo fetch -p rmcp` (or `cargo build -p tnls-tunnel` — it will still build the OLD mcp.rs).
Expected: `rmcp` 1.7.x resolves. If the feature names differ, run `cargo add rmcp --dry-run` to see available features and pick the client + child-process transport features.

### Task 2.2: Rewrite `mcp.rs` as an rmcp client

- [ ] **Step 1: Replace the entire contents of `crates/tnls-tunnel/src/mcp.rs`**

```rust
use anyhow::{anyhow, bail, Result};
use rmcp::{
    model::{CallToolRequestParam, CallToolResult},
    service::RunningService,
    transport::TokioChildProcess,
    RoleClient, ServiceExt,
};
use serde_json::{json, Map, Value};
use tnls_core::Tool;
use tokio::process::Command;

/// A spawned MCP server we speak JSON-RPC to over its stdio, via the official `rmcp` client.
/// The hand-rolled request/response plumbing is gone — rmcp owns the framing and handshake.
pub struct McpChild {
    service: RunningService<RoleClient, ()>,
    /// Tools advertised at handshake, in the wire shape the agent forwards to the viewer.
    pub tools: Vec<Tool>,
}

impl McpChild {
    /// Spawn `cmd args` (with `env`) as an MCP server, run the rmcp `initialize` handshake,
    /// and cache the advertised tool list. Fails loudly if the child can't be driven.
    pub async fn spawn(cmd: &str, args: &[String], env: &[(String, String)]) -> Result<Self> {
        let mut command = Command::new(cmd);
        command.args(args);
        for (k, v) in env {
            command.env(k, v);
        }
        let transport = TokioChildProcess::new(command)
            .map_err(|e| anyhow!("spawning MCP server '{cmd}': {e}"))?;
        let service = ()
            .serve(transport)
            .await
            .map_err(|e| anyhow!("MCP handshake with '{cmd}': {e}"))?;
        let tools = service
            .list_all_tools()
            .await
            .map_err(|e| anyhow!("listing tools from '{cmd}': {e}"))?
            .into_iter()
            .map(convert_tool)
            .collect();
        Ok(Self { service, tools })
    }

    /// Call `tool` with JSON `args`. Returns `{ "content": [...] }` (what the agent forwards
    /// to the viewer as an `AgentFrame::Result`); a tool-reported error becomes `Err`, which
    /// `session.rs` maps to `ErrorCode::ToolError`.
    pub async fn call_tool(&self, tool: &str, args: Value) -> Result<Value> {
        let arguments = match args {
            Value::Object(m) => Some(m),
            Value::Null => None,
            other => Some(Map::from_iter([("value".to_string(), other)])),
        };
        let result: CallToolResult = self
            .service
            .call_tool(CallToolRequestParam {
                name: tool.to_string().into(),
                arguments,
            })
            .await
            .map_err(|e| anyhow!("{e}"))?;
        if result.is_error == Some(true) {
            bail!("{}", serde_json::to_string(&result.content).unwrap_or_default());
        }
        Ok(json!({ "content": result.content }))
    }

    /// Stop the child: cancel the rmcp service, which closes the transport; rmcp's
    /// `TokioChildProcess` cleans up the OS process on drop.
    pub async fn kill(&mut self) {
        let _ = self.service.cancel().await;
    }
}

/// `rmcp::model::Tool` → the wire `Tool` the viewer sees. Name and (camelCase) `inputSchema`
/// must be preserved verbatim — `filter_tools`/`decide_call` match on `name`, and the e2e
/// asserts the advertised order (spec §7.3).
fn convert_tool(t: rmcp::model::Tool) -> Tool {
    Tool {
        name: t.name.to_string(),
        description: t.description.map(|d| d.to_string()),
        input_schema: Value::Object((*t.input_schema).clone()),
    }
}
```

> If `t.input_schema`'s type isn't `Arc<Map<String,Value>>` in your patch release, adjust the deref: the goal is `Value::Object(<the schema object>)`. Confirm against `docs.rs/rmcp/1.7.0` `model::Tool`.

- [ ] **Step 2: Build just this crate (callers not yet updated — expect ONE class of error)**

Run: `cargo build -p tnls-tunnel`
Expected: FAIL — `session.rs` still calls `McpChild::spawn(server, args)` with 2 args and references `OpenArgs` without `env`. That's the next task. (If you see errors *inside* `mcp.rs` itself, fix those first — they mean an rmcp API name needs adjusting per the note above.)

### Task 2.3: Thread `env` through `session.rs` and fix the banner

- [ ] **Step 1: Add `env` to `OpenArgs`**

In `crates/tnls-tunnel/src/session.rs`, add a field to `OpenArgs`:

```rust
pub struct OpenArgs {
    pub server: String,
    pub server_args: Vec<String>,
    pub ttl: Duration,
    pub scope: Vec<String>,
    pub relay: String,
    /// Extra env for the spawned MCP child (e.g. ("TNLS_RELAY", relay)). Empty for `open`.
    pub env: Vec<(String, String)>,
}
```

- [ ] **Step 2: Pass `env` to `spawn`**

In `session::open`, change the spawn call (currently `McpChild::spawn(&args.server, &args.server_args).await?`):

```rust
let mut child = McpChild::spawn(&args.server, &args.server_args, &args.env).await?;
```

- [ ] **Step 3: Fix the banner string**

In `print_banner` (same file), change the line containing `` `tunnel close` `` to `` `tnls close` `` (the revoke hint now names the new binary).

- [ ] **Step 4: Add `env: vec![]` to the remaining `OpenArgs` literals**

Find them: `rg -n 'OpenArgs\s*\{' --type rust`
Update each construction that doesn't yet set `env` to include `env: vec![]`:
- `crates/tnls-tunnel/src/main.rs` (the `tunnel` bin's `open` handler)
- `crates/tnls/src/share.rs` (the `run_share` call — it will be rewritten in Chunk 4, but must compile now)
- `crates/tnls-tunnel/tests/end_to_end.rs` (the e2e's `OpenArgs`)

- [ ] **Step 5: Update any direct `McpChild::spawn` call sites**

Find them: `rg -n 'McpChild::spawn' --type rust`
Any call outside `session.rs` (e.g. in `crates/tnls-tunnel/tests/mcp_bridge.rs`) gains a third arg `&[]` (empty env).

- [ ] **Step 6: Build the workspace**

Run: `cargo build --workspace`
Expected: clean. (`mcp-demo` is still the hand-rolled server — that's intended; the rmcp client talks to it fine.)

### Task 2.4: Prove it with the existing agent-bridge tests

These tests are the regression spec for the rewrite — no new tests needed here.

- [ ] **Step 1: Run the agent-bridge e2e + bridge unit test**

Run: `cargo test -p tnls-tunnel`
Expected: PASS — including `read_succeeds_shell_refused` (proves: handshake works, `tools/list` filters to `["read"]`, `read` returns content, out-of-scope `shell` is refused agent-side) and `mcp_bridge`'s assertions. This is the proof that the rmcp client is a faithful drop-in.

If `read_succeeds_shell_refused` fails on the tool list assertion (`names == ["read"]`), the rmcp client likely reordered or renamed tools in `convert_tool` — re-check that `name` is passed through verbatim.

- [ ] **Step 2: Full suite + CI gate**

Run: `cargo test --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: all green, no warnings. (Clippy may flag the deleted-code's former `#[allow(...)]` if any remain — remove now-unused allows in `mcp.rs`.)

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(tnls-tunnel): McpChild on the rmcp client; add OpenArgs.env"
```

**End of Chunk 2:** the agent's MCP client is now `rmcp`; the hand-rolled JSON-RPC in `mcp.rs` is gone; `session.rs`'s enforcement loop is unchanged; `OpenArgs.env` exists for the host to use. All tests green against the still-hand-rolled `mcp-demo`.

---

## Chunk 3: `tnls-plugin` contract crate + the `demo` plugin

**Goal:** Create the shared `describe` contract crate, then rebuild `mcp-demo` as the `tnls-demo` plugin — an `rmcp` server with a `describe` command. This is the first plugin end-to-end and the template every other plugin follows (spec §3.1, §4.1). After this chunk the agent-bridge e2e drives an **rmcp client ↔ rmcp server** (both ends now official SDK).

**Files:**
- Create: `crates/tnls-plugin/Cargo.toml`, `crates/tnls-plugin/src/lib.rs`
- Move + rewrite: `crates/mcp-demo/` → `crates/plugins/demo/` (package `tnls-demo`, bin `tnls-demo`)
  - `crates/plugins/demo/src/main.rs` (clap `serve`/`describe` + rmcp serve)
  - `crates/plugins/demo/src/server.rs` (the `Demo` rmcp `ServerHandler`)
  - delete `crates/plugins/demo/src/dispatch.rs` (its logic moves into `server.rs`)
- Modify: root `Cargo.toml` (members), `crates/tnls-tunnel/tests/end_to_end.rs` + `crates/tnls-tunnel/tests/mcp_bridge.rs` (spawn `tnls-demo serve`)

### Task 3.1: The `tnls-plugin` describe-contract crate

- [ ] **Step 1: Create `crates/tnls-plugin/Cargo.toml`**

```toml
[package]
name = "tnls-plugin"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 2: Add it to the workspace**

Root `Cargo.toml` `members`: add `"crates/tnls-plugin"`.

- [ ] **Step 3: Write the failing test** — `crates/tnls-plugin/src/lib.rs` (test module at the bottom)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_with_tunnel_and_plain() {
        let m = Manifest::new(
            "rendezvous",
            "Capability-scoped file sending over BitTorrent.",
            vec![
                Command::tunneled("share", "Seed + serve a file.", &["list_shares", "request_file"], "30m"),
                Command::plain("get", "Fetch a shared file."),
            ],
        );
        let json = m.to_json();
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, 1);
        assert_eq!(back.command("share").unwrap().tunnel.as_ref().unwrap().scope, vec!["list_shares", "request_file"]);
        assert!(back.command("get").unwrap().tunnel.is_none(), "plain command has no tunnel");
        assert!(json.contains(r#""tunnel":null"#), "plain command serializes tunnel:null");
    }
}
```

- [ ] **Step 4: Run it — expect a compile failure** (`Manifest`/`Command` not defined)

Run: `cargo test -p tnls-plugin`
Expected: FAIL to compile.

- [ ] **Step 5: Implement the types** — at the top of `crates/tnls-plugin/src/lib.rs`

```rust
//! The `describe` contract shared by the `tnls` host (deserializes) and every plugin
//! (serializes). Keep this crate tiny — it is the only thing both sides depend on.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub name: String,
    pub about: String,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub about: String,
    /// `Some` => tunneled (the agent path); `None`/absent => plain passthrough.
    #[serde(default)]
    pub tunnel: Option<Tunnel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    pub scope: Vec<String>,
    pub default_ttl: String,
}

impl Manifest {
    pub fn new(name: &str, about: &str, commands: Vec<Command>) -> Self {
        Self { schema: 1, name: name.into(), about: about.into(), commands }
    }
    pub fn command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.name == name)
    }
    /// The single JSON line a plugin's `describe` prints.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("manifest serializes")
    }
}

impl Command {
    pub fn tunneled(name: &str, about: &str, scope: &[&str], default_ttl: &str) -> Self {
        Self {
            name: name.into(),
            about: about.into(),
            tunnel: Some(Tunnel { scope: scope.iter().map(|s| s.to_string()).collect(), default_ttl: default_ttl.into() }),
        }
    }
    pub fn plain(name: &str, about: &str) -> Self {
        Self { name: name.into(), about: about.into(), tunnel: None }
    }
}
```

> Note: `tunnel: Option<Tunnel>` with `#[serde(default)]` deserializes a missing field as `None`. To force `"tunnel":null` to *appear* in output (the test asserts it), do **not** add `skip_serializing_if` — we want plain commands to serialize an explicit `null`.

- [ ] **Step 6: Run the test — expect PASS**

Run: `cargo test -p tnls-plugin`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(tnls-plugin): describe manifest contract types"
```

### Task 3.2: Move + re-skin `mcp-demo` as the `tnls-demo` plugin crate

- [ ] **Step 1: Move the crate**

```bash
mkdir -p crates/plugins
git mv crates/mcp-demo crates/plugins/demo
git rm crates/plugins/demo/src/dispatch.rs   # logic moves into server.rs
```

- [ ] **Step 2: Rewrite `crates/plugins/demo/Cargo.toml`**

```toml
[package]
name = "tnls-demo"
version = "0.1.0"
edition = "2021"
publish = false

[[bin]]
name = "tnls-demo"
path = "src/main.rs"

[dependencies]
rmcp = { version = "1.7", features = ["server", "transport-io", "macros"] }
schemars = "0.8"
tnls-plugin = { path = "../../tnls-plugin" }
clap = { version = "4", features = ["derive"] }
tokio = { workspace = true }
anyhow = { workspace = true }
```

> Confirm rmcp's macro feature name (`macros` enables `#[tool]`/`#[tool_router]`/`#[tool_handler]`; in some releases these are under the default `server` feature). Run `cargo add rmcp --dry-run` to see the feature list and adjust. **Also confirm the `schemars` major** rmcp 1.7.0 expects (0.8 vs 1.x): the plugin's `JsonSchema` derive must match rmcp's, or `Parameters<T>` schema generation won't line up. `cargo tree -p rmcp -i schemars` shows which major rmcp pulls in — pin the plugin's `schemars` to that same major (this applies equally to `tnls-rendezvous` in Chunk 4).

- [ ] **Step 3: Update the workspace member path**

Root `Cargo.toml`: replace `"crates/mcp-demo"` with `"crates/plugins/demo"`.

### Task 3.3: The `Demo` rmcp server

- [ ] **Step 1: Write `crates/plugins/demo/src/server.rs`**

Port `read` + `shell` (formerly in `dispatch.rs`) as rmcp tools. **Declare `read` before `shell`** — order is asserted by the agent-bridge e2e (spec §7.3).

```rust
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Clone)]
pub struct Demo {
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadParams {
    path: String,
}
#[derive(Deserialize, JsonSchema)]
struct ShellParams {
    cmd: String,
}

#[tool_router]
impl Demo {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    #[tool(description = "Read a UTF-8 text file and return its contents.")]
    async fn read(&self, Parameters(ReadParams { path }): Parameters<ReadParams>) -> Result<CallToolResult, ErrorData> {
        let body = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ErrorData::internal_error(format!("read failed: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(description = "Run a shell command and return its output.")]
    async fn shell(&self, Parameters(ShellParams { cmd }): Parameters<ShellParams>) -> Result<CallToolResult, ErrorData> {
        let out = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .await
            .map_err(|e| ErrorData::internal_error(format!("spawn failed: {e}"), None))?;
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.stderr.is_empty() {
            s.push_str(&String::from_utf8_lossy(&out.stderr));
        }
        Ok(CallToolResult::success(vec![Content::text(s)]))
    }
}

#[tool_handler]
impl ServerHandler for Demo {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Sample MCP server exposing read + shell.".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
```

> rmcp API names (`ToolRouter`, `Parameters`, `CallToolResult::success`, `Content::text`, `ErrorData::internal_error`, `ServerCapabilities::builder().enable_tools()`) follow the documented 1.7.0 pattern. If a path differs, fix the `use` — the structure (a `tool_router` field initialized by the generated `Self::tool_router()`, tools returning `Result<CallToolResult, ErrorData>`) is what matters.

- [ ] **Step 2: Port the behavior tests** into `server.rs`'s `#[cfg(test)] mod tests`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_returns_file_contents() {
        let p = std::env::temp_dir().join("tnls-demo-read-test.txt");
        tokio::fs::write(&p, "hello-file").await.unwrap();
        let out = Demo::new().read(Parameters(ReadParams { path: p.to_str().unwrap().into() })).await.unwrap();
        assert_eq!(out.content[0].as_text().unwrap().text, "hello-file");
    }

    #[tokio::test]
    async fn shell_executes() {
        let out = Demo::new().shell(Parameters(ShellParams { cmd: "echo hi".into() })).await.unwrap();
        assert_eq!(out.content[0].as_text().unwrap().text.trim(), "hi");
    }
}
```

> `Content::as_text()` returns the text payload; confirm the accessor name in rmcp 1.7.0 (it may be `out.content[0].raw.as_text()` or a `.as_text()` helper). The assertion intent: the first content item's text equals the expected string.

### Task 3.4: `main.rs` — clap `serve`/`describe` + rmcp stdio serve

- [ ] **Step 1: Write `crates/plugins/demo/src/main.rs`**

```rust
mod server;

use clap::{Parser, Subcommand};
use rmcp::{transport::stdio, ServiceExt};

#[derive(Parser)]
#[command(name = "tnls-demo", about = "Sample MCP server plugin (read + shell).")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve read/shell as an MCP server over stdio (spawned by tnls; tunneled).
    Serve,
    /// Print this plugin's describe manifest (called by tnls).
    Describe,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Describe => {
            println!("{}", describe().to_json());
            Ok(())
        }
        Cmd::Serve => {
            let service = server::Demo::new().serve(stdio()).await?;
            service.waiting().await?;
            Ok(())
        }
    }
}

/// The static manifest tnls reads. `serve` is tunneled, scoped to `read` only —
/// `shell` is deny-by-default until the user passes `--scope read,shell`.
fn describe() -> tnls_plugin::Manifest {
    tnls_plugin::Manifest::new(
        "demo",
        "Sample MCP server (read + shell).",
        vec![tnls_plugin::Command::tunneled(
            "serve",
            "Serve read/shell over a tunnel.",
            &["read"],
            "15m",
        )],
    )
}
```

- [ ] **Step 2: Add a describe sanity test** (in `main.rs`)

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn describe_serve_is_read_scoped() {
        let m = super::describe();
        let serve = m.command("serve").unwrap();
        let t = serve.tunnel.as_ref().expect("serve is tunneled");
        assert_eq!(t.scope, vec!["read"]);
        assert!(!t.scope.contains(&"shell".to_string()), "shell is deny-by-default");
    }
}
```

### Task 3.5: Point the agent-bridge tests at `tnls-demo serve`, build, verify

The demo binary changed name + now needs the `serve` subcommand. Update the two tests that spawn it.

- [ ] **Step 1: Find the spawns**

Run: `rg -n 'mcp-demo|mcp_demo' -g '*.rs'`
Expected matches in `crates/tnls-tunnel/tests/end_to_end.rs` and `crates/tnls-tunnel/tests/mcp_bridge.rs`.

- [ ] **Step 2: Update each spawn**

In both test files:
- `build("mcp-demo")` → `build("tnls-demo")`
- the child path `…/target/debug/mcp-demo` → `…/target/debug/tnls-demo`
- the `server_args` / spawn args: pass `vec!["serve".to_string()]` (was `vec![]`) so the binary runs in MCP-server mode.

- [ ] **Step 3: Build the workspace**

Run: `cargo build --workspace`
Expected: clean (`tnls-demo` builds; the tests reference the new path/args).

- [ ] **Step 4: Run the plugin's own tests + the agent-bridge e2e**

Run: `cargo test -p tnls-demo -p tnls-tunnel`
Expected: PASS. The e2e now exercises rmcp-client ↔ rmcp-server: `read` succeeds, the advertised list filters to `["read"]` (so `read` must come first and keep its name), and `shell` is refused agent-side. If the list assertion fails, check tool declaration order in `server.rs`.

- [ ] **Step 5: Full suite + gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo fmt --all --check`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(demo): tnls-demo plugin — rmcp server + describe manifest"
```

**End of Chunk 3:** `tnls-plugin` defines the describe contract; `tnls-demo` is a working `rmcp` plugin (`serve` tunneled @ scope `read`, plus `describe`); both MCP ends are now `rmcp`, proven by the agent-bridge e2e. The host doesn't exist yet, so `tnls demo serve` isn't wired — that's Chunk 5; for now `tnls-demo serve` is a runnable stdio MCP server on its own.

---

## Chunk 4: the `rendezvous` plugin

**Goal:** Move the file-sharing surface out of `crates/tnls` into `crates/plugins/rendezvous` (bin `tnls-rendezvous`); turn the old `mcp_serve.rs` into an `rmcp` `ShareServer` whose `_seeder` field ties seeding to the served session; rewrite `share` to **seed + gather peers + serve in one process** (no `session::open` call — the host owns the tunnel). `crates/tnls` is reduced to a **temporary stub** so the workspace stays green; Chunk 5 replaces the stub with the real host (spec §3.4, §4.1, §6 step 3).

**Files:**
- Create: `crates/plugins/rendezvous/Cargo.toml`
- `git mv` (unchanged): `crates/tnls/src/bittorrent.rs` + `crates/tnls/src/get.rs` → `crates/plugins/rendezvous/src/`
- Create: `crates/plugins/rendezvous/src/{server.rs, share.rs, main.rs}` (server replaces the old `mcp_serve.rs`)
- `git rm`: `crates/tnls/src/mcp_serve.rs` (its tools become `server.rs`)
- Move + adapt: `crates/tnls/tests/{loopback.rs, end_to_end.rs}` → `crates/plugins/rendezvous/tests/`
- Stub: `crates/tnls/src/{lib.rs, main.rs}` + trim `crates/tnls/Cargo.toml`
- Modify: root `Cargo.toml` (add `crates/plugins/rendezvous`)

### Task 4.1: Create the crate; move `bittorrent.rs` + `get.rs` unchanged

- [ ] **Step 1: Create `crates/plugins/rendezvous/Cargo.toml`**

```toml
[package]
name = "tnls-rendezvous"
version = "0.1.0"
edition = "2021"
publish = false

[[bin]]
name = "tnls-rendezvous"
path = "src/main.rs"

[dependencies]
librqbit = "=9.0.0-rc.0"   # only a pre-release exists; caret "9" won't resolve it
rmcp = { version = "1.7", features = ["server", "transport-io", "macros"] }
schemars = "0.8"
tnls-core = { path = "../../tnls-core" }
tnls-plugin = { path = "../../tnls-plugin" }
clap = { version = "4", features = ["derive"] }
tokio = { workspace = true }
anyhow = { workspace = true }
serde_json = { workspace = true }
futures-util = { workspace = true }
tokio-tungstenite = { version = "0.23", features = ["rustls-tls-webpki-roots"] }
rustls = { version = "0.23", features = ["ring"] }
urlencoding = "2"
bytes = "1"
local-ip-address = "0.6"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }

[dev-dependencies]
tnls-tunnel = { path = "../../tnls-tunnel" }
relay = { path = "../../relay" }
axum = { version = "0.7", features = ["ws"] }
```

> These deps are lifted from the current `crates/tnls/Cargo.toml`. The dev-deps (`tnls-tunnel` + `relay` + `axum`) are for the transfer e2e (Task 4.5); the `axum` dev-dep is the shared seam Spec 2 §9 removes when the relay goes Rocket. (No `tokio-tungstenite` *dev*-dep: the e2e drives `get` as a subprocess rather than a raw in-test WS viewer, and `get.rs` already pulls `tokio-tungstenite` as a normal dep.)

- [ ] **Step 2: Add to the workspace**

Root `Cargo.toml` `members`: add `"crates/plugins/rendezvous"`.

- [ ] **Step 3: Move `bittorrent.rs` and `get.rs` unchanged**

```bash
git mv crates/tnls/src/bittorrent.rs crates/plugins/rendezvous/src/bittorrent.rs
git mv crates/tnls/src/get.rs        crates/plugins/rendezvous/src/get.rs
```

`get.rs` already imports `tnls_core::{AgentFrame, ViewerFrame}` (renamed in Chunk 1) and `crate::bittorrent` — both still resolve inside the new crate. No edits needed. `bittorrent.rs` is self-contained.

### Task 4.2: `server.rs` — the `ShareServer` rmcp server

This replaces `mcp_serve.rs`. The seeder is a field, so dropping the server stops seeding (spec §3.4).

- [ ] **Step 1: Write `crates/plugins/rendezvous/src/server.rs`**

```rust
use crate::bittorrent::SeedHandle;
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

/// Static share details the tools hand out.
#[derive(Clone)]
pub struct ShareInfo {
    pub magnet: String,
    pub name: String,
    pub size: u64,
    pub peers: Vec<String>,
}

/// The rmcp server. `_seeder` keeps the BitTorrent session alive for exactly as long as
/// this server is served (drop => seeding stops). `SeedHandle` is not `Clone`, so it lives
/// behind an `Arc` to let the handler derive `Clone` (rmcp requires it).
#[derive(Clone)]
pub struct ShareServer {
    share: ShareInfo,
    _seeder: Arc<SeedHandle>,
    tool_router: ToolRouter<Self>,
}

/// Tools take no arguments; an empty params struct keeps the rmcp macro happy.
#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

#[tool_router]
impl ShareServer {
    pub fn new(share: ShareInfo, seeder: SeedHandle) -> Self {
        Self { share, _seeder: Arc::new(seeder), tool_router: Self::tool_router() }
    }

    #[tool(description = "List the file(s) shared in this tunnel.")]
    async fn list_shares(&self, _p: Parameters<NoArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} ({} bytes)",
            self.share.name, self.share.size
        ))]))
    }

    #[tool(description = "Get the magnet link (and peer hints) for the shared file.")]
    async fn request_file(&self, _p: Parameters<NoArgs>) -> Result<CallToolResult, ErrorData> {
        let body = serde_json::json!({ "magnet": self.share.magnet, "peers": self.share.peers });
        Ok(CallToolResult::success(vec![Content::text(body.to_string())]))
    }

    #[tool(description = "Share status.")]
    async fn status(&self, _p: Parameters<NoArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::success(vec![Content::text("seeding")]))
    }
}

#[tool_handler]
impl ServerHandler for ShareServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Capability-scoped file sending over BitTorrent.".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
```

- [ ] **Step 2: Port the `request_file` test** (was in `mcp_serve.rs`) into `server.rs` tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_file_body_has_magnet_and_peers() {
        // Mirror what request_file emits, asserting the wire shape get.rs parses.
        let share = ShareInfo {
            magnet: "magnet:?xt=urn:btih:abc".into(),
            name: "f.bin".into(),
            size: 9,
            peers: vec!["127.0.0.1:6881".into()],
        };
        let body = serde_json::json!({ "magnet": share.magnet, "peers": share.peers }).to_string();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["magnet"], "magnet:?xt=urn:btih:abc");
        assert_eq!(v["peers"][0], "127.0.0.1:6881");
    }
}
```

> A `SeedHandle` needs a live librqbit session, so we don't construct a full `ShareServer` in a unit test; the `request_file` *body* is what `get.rs` parses, and the transfer e2e (Task 4.5) exercises the real server end-to-end. Delete the unused `server()` helper if clippy complains.

### Task 4.3: `share.rs` — seed + gather peers + serve (self-contained)

- [ ] **Step 1: Write `crates/plugins/rendezvous/src/share.rs`**

This is the old `run_share` minus its `session::open` call — the host opens the tunnel; this process just seeds and serves.

```rust
use crate::bittorrent::{create_share, seed, NetOpts};
use crate::server::{ShareInfo, ShareServer};
use anyhow::{Context, Result};
use rmcp::{transport::stdio, ServiceExt};
use std::path::Path;

const DEFAULT_TRACKER: &str = "udp://tracker.opentrackr.org:1337/announce";

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("0.0.0.0:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
}

/// loopback + LAN IPv4 + (best-effort) public IP from the relay's /whoami, all at :port.
/// `relay` is the TNLS_RELAY the host passes down; empty => skip the public-IP lookup.
async fn gather_peers(port: u16, relay: &str) -> Vec<String> {
    let mut addrs = vec![format!("127.0.0.1:{port}")];
    if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
        for (_n, ip) in ifaces {
            if let std::net::IpAddr::V4(v4) = ip {
                if !v4.is_loopback() && !v4.is_link_local() {
                    addrs.push(format!("{v4}:{port}"));
                }
            }
        }
    }
    if !relay.is_empty() {
        let scheme = if relay.starts_with("wss://") { "https" } else { "http" };
        let host = relay.trim_start_matches("ws://").trim_start_matches("wss://").trim_end_matches('/');
        if let Ok(resp) = reqwest::get(format!("{scheme}://{host}/whoami")).await {
            if let Ok(ip) = resp.text().await {
                let ip = ip.trim();
                if ip.parse::<std::net::Ipv4Addr>().is_ok() {
                    addrs.push(format!("{ip}:{port}"));
                }
            }
        }
    }
    addrs
}

/// Seed `path`, gather reachable peer addresses, then serve the share over stdio (rmcp).
/// Runs until stdin closes (the host kills it on tunnel teardown) — at which point the
/// returned service ends, `ShareServer` drops, and the seeder stops.
pub async fn run_share(path: String) -> Result<()> {
    let p = Path::new(&path);
    let meta = create_share(p, &[DEFAULT_TRACKER.to_string()]).await?;
    let data_dir = p.parent().unwrap_or(Path::new(".")).to_path_buf();

    let port = pick_free_port();
    let relay = std::env::var("TNLS_RELAY").unwrap_or_default();
    let peers = gather_peers(port, &relay).await;

    let seeder = seed(
        &meta,
        &data_dir,
        NetOpts { disable_dht: false, listen_port: Some(port), enable_upnp: true, initial_peers: vec![] },
    )
    .await
    .context("start seeding")?;

    // To stderr (tnls inherits it) — the magnet rides the tunnel to the viewer, not stdout
    // (stdout is the MCP JSON-RPC channel).
    eprintln!("seeding {} ({} bytes)", meta.name, meta.size);

    let share = ShareInfo { magnet: meta.magnet, name: meta.name, size: meta.size, peers };
    let service = ShareServer::new(share, seeder).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

### Task 4.4: `main.rs` (clap + rustls provider) + `describe`

- [ ] **Step 1: Write `crates/plugins/rendezvous/src/main.rs`**

```rust
mod bittorrent;
mod get;
mod server;
mod share;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::Path;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "tnls-rendezvous", about = "Capability-scoped file sending over BitTorrent.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Seed a file and serve it as an MCP server over stdio (spawned by tnls; tunneled).
    Share { path: String },
    /// Fetch a shared file from a tunnel link (plain client; no tunnel).
    Get {
        link: String,
        #[arg(long, default_value = "./tnls-downloads")]
        out: String,
    },
    /// Seed a file and print its magnet (dev; foreground).
    Seed {
        path: String,
        #[arg(long)]
        tracker: Vec<String>,
    },
    /// Fetch a file from a magnet into a directory (dev).
    Fetch {
        magnet: String,
        #[arg(long, default_value = "./tnls-downloads")]
        out: String,
    },
    /// Print this plugin's describe manifest (called by tnls).
    Describe,
}

#[tokio::main]
async fn main() -> Result<()> {
    // rustls 0.23 needs a process-wide provider before any wss:// (get) or https (/whoami) dial.
    let _ = rustls::crypto::ring::default_provider().install_default();

    match Cli::parse().cmd {
        Cmd::Describe => {
            println!("{}", describe().to_json());
            Ok(())
        }
        Cmd::Share { path } => share::run_share(path).await,
        Cmd::Get { link, out } => get::run_get(&link, Path::new(&out)).await,
        Cmd::Seed { path, tracker } => seed_cmd(path, tracker).await,
        Cmd::Fetch { magnet, out } => fetch_cmd(magnet, out).await,
    }
}

/// `share` is the only tunneled command; the rest are plain client/dev commands.
fn describe() -> tnls_plugin::Manifest {
    tnls_plugin::Manifest::new(
        "rendezvous",
        "Capability-scoped file sending over BitTorrent.",
        vec![
            tnls_plugin::Command::tunneled(
                "share",
                "Seed a file and serve it through a scoped tunnel.",
                &["list_shares", "request_file"],
                "30m",
            ),
            tnls_plugin::Command::plain("get", "Fetch a shared file from a tunnel link."),
            tnls_plugin::Command::plain("seed", "Seed a file and print its magnet (dev)."),
            tnls_plugin::Command::plain("fetch", "Fetch a file from a magnet (dev)."),
        ],
    )
}

async fn seed_cmd(path: String, tracker: Vec<String>) -> Result<()> {
    use bittorrent::{create_share, seed, NetOpts};
    let trackers = if tracker.is_empty() {
        vec!["udp://tracker.opentrackr.org:1337/announce".to_string()]
    } else {
        tracker
    };
    let meta = create_share(Path::new(&path), &trackers).await?;
    let data_dir = Path::new(&path).parent().unwrap_or(Path::new(".")).to_path_buf();
    let _seeder = seed(&meta, &data_dir, NetOpts { disable_dht: false, listen_port: None, enable_upnp: true, initial_peers: vec![] }).await?;
    println!("seeding {} ({} bytes)\nmagnet → {}\nctrl-c to stop.", meta.name, meta.size, meta.magnet);
    std::future::pending::<()>().await;
    Ok(())
}

async fn fetch_cmd(magnet: String, out: String) -> Result<()> {
    use bittorrent::{fetch, NetOpts};
    std::fs::create_dir_all(&out)?;
    let dl = fetch(&magnet, Path::new(&out), NetOpts { disable_dht: false, listen_port: None, enable_upnp: true, initial_peers: vec![] }).await?;
    loop {
        let p = dl.progress();
        let pct = if p.total > 0 { p.downloaded * 100 / p.total } else { 0 };
        println!("  {pct:>3}%  {}/{} bytes", p.downloaded, p.total);
        if p.finished { break; }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    println!("done → {:?}", dl.output_path());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn describe_share_is_tunneled_get_is_plain() {
        let m = super::describe();
        assert!(m.command("share").unwrap().tunnel.is_some(), "share is tunneled");
        assert!(m.command("get").unwrap().tunnel.is_none(), "get is a plain client command");
        assert_eq!(m.command("share").unwrap().tunnel.as_ref().unwrap().scope, vec!["list_shares", "request_file"]);
    }
}
```

### Task 4.5: Stub `crates/tnls`, move the transfer e2e, build, verify

The file-sharing code is gone from `crates/tnls`, so reduce it to a stub that compiles; Chunk 5 makes it the host.

- [ ] **Step 1: Delete the now-moved module + stub `crates/tnls`**

```bash
git rm crates/tnls/src/mcp_serve.rs
```

Replace `crates/tnls/src/lib.rs` with an empty file (or `// host crate — see Chunk 5`). Replace `crates/tnls/src/main.rs` with a placeholder so the bin still builds:

```rust
fn main() {
    eprintln!("tnls host not yet built (Chunk 5)");
    std::process::exit(1);
}
```

Trim `crates/tnls/Cargo.toml` `[dependencies]` to just what the stub needs (it can be empty) and **remove** the moved deps (`librqbit`, `reqwest`, `local-ip-address`, `tokio-tungstenite`, `tnls-core`, `tnls-tunnel`, `bytes`, `urlencoding`, `serde_json`, `futures-util`) and the `[dev-dependencies]` (they move to rendezvous). Keep `clap`/`tokio`/`anyhow` only if the stub uses them (the placeholder above needs none).

> Also delete `crates/tnls/src/cli.rs` if present — the host's CLI is written fresh in Chunk 5. `git rm crates/tnls/src/cli.rs`.

- [ ] **Step 2: Move + adapt the transfer e2e tests**

```bash
git mv crates/tnls/tests/loopback.rs    crates/plugins/rendezvous/tests/loopback.rs
git mv crates/tnls/tests/end_to_end.rs  crates/plugins/rendezvous/tests/end_to_end.rs
```

Adapt them to the new shape: the e2e drives `tnls_tunnel::session::open` **directly** (as the `tnls-tunnel` e2e does) to tunnel the rendezvous `share` child, then runs `get` against the published link. Concretely, in `end_to_end.rs`:
- **build the binary first**: `cargo build -p tnls-rendezvous` (the existing `build()` helper pattern from the `tnls-tunnel` e2e), so `target/debug/tnls-rendezvous` exists for *both* `session::open`'s `server:` path and the `get` subprocess — explicit, so the test doesn't rely on cargo's same-crate build side effect and can't flake on "binary not found";
- spawn the relay via `relay::build_app()` + `axum::serve` (unchanged — Spec 2 swaps this later);
- call `tnls_tunnel::session::open(OpenArgs { server: "<target/debug/tnls-rendezvous>", server_args: vec!["share".into(), file], ttl, scope: vec!["list_shares".into(), "request_file".into()], relay: relay_url, env: vec![("TNLS_RELAY".into(), relay_url.clone())] })` in a spawned task;
- read the link from the pidfile (reuse the `read_link` helper pattern from the `tnls-tunnel` e2e);
- call `tnls_rendezvous`'s `get::run_get(&link, out_dir)` — but `get`/`run_get` live in the rendezvous **binary** crate, not a lib. Either (a) add a thin `src/lib.rs` to rendezvous re-exporting `get`/`bittorrent` for tests, or (b) drive `get` by spawning `tnls-rendezvous get <link> --out <dir>` as a subprocess and asserting the downloaded bytes. **Use (b)** — it tests the real plugin binary end-to-end and needs no lib surface.
- assert the downloaded file bytes equal the source (the loopback peer makes this a real, hermetic transfer — no DHT).

> `loopback.rs`: if it tested `bittorrent` internals directly, it needs the rendezvous lib surface — simplest is to **fold its assertions into `bittorrent.rs`'s own `#[cfg(test)]`** (same crate, no lib needed) and delete `loopback.rs`. Decide based on what `loopback.rs` actually imports.

- [ ] **Step 3: Build the workspace**

Run: `cargo build --workspace`
Expected: clean. `crates/tnls` is a stub; `tnls-rendezvous` builds with all the moved code.

- [ ] **Step 4: Run rendezvous tests (unit + transfer e2e)**

Run: `cargo test -p tnls-rendezvous`
Expected: PASS — `describe` test, `request_file` body test, `bittorrent`'s magnet/create tests, `get`'s `parse_link`/`parse_request_file_content` tests, and the real loopback **transfer** e2e (downloaded bytes == source).

- [ ] **Step 5: Full suite + gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo fmt --all --check`
Expected: green. (The stub `tnls` bin builds; its placeholder `main` isn't exercised by any test.)

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(rendezvous): file-sharing plugin — rmcp ShareServer, self-seeding share, get/seed/fetch"
```

**End of Chunk 4:** `tnls-rendezvous` is a complete plugin — `share` seeds-and-serves as one rmcp process (seeder tied to the session), `get`/`seed`/`fetch` are plain commands, `describe` declares them. The real share→get transfer is proven hermetically via `session::open`. `crates/tnls` is a stub awaiting the host (Chunk 5).

---

## Chunk 5: the `tnls` host

**Goal:** Replace the `crates/tnls` stub with the real host — clap with global flags + `open`/`close`/`plugins` + an `external_subcommand` catch-all; `discover` (find `tnls-<name>` next to `current_exe` then `$PATH`); `dispatch` (run `<plugin> describe`, route tunneled→`session::open`, plain→exec). Install the rustls provider. **Delete the `tunnel` bin.** After this chunk `tnls demo serve` and `tnls rendezvous share <f>` work end-to-end (spec §3.2, §3.3, §5).

**Files:**
- Modify: `crates/tnls/Cargo.toml` (host deps)
- Replace: `crates/tnls/src/main.rs`; Create: `crates/tnls/src/{cli.rs, discover.rs, dispatch.rs}`; `crates/tnls/src/lib.rs` exposes the testable modules
- Delete the `tunnel` bin: `git rm crates/tnls-tunnel/src/main.rs crates/tnls-tunnel/src/cli.rs`; edit `crates/tnls-tunnel/src/lib.rs` (drop `pub mod cli;`) + `crates/tnls-tunnel/Cargo.toml` (drop `[[bin]]`)

### Task 5.1: Host `Cargo.toml` + delete the `tunnel` bin

- [ ] **Step 1: Set `crates/tnls/Cargo.toml`**

```toml
[package]
name = "tnls"
version = "0.1.0"
edition = "2021"

[dependencies]
tnls-core = { path = "../tnls-core" }
tnls-tunnel = { path = "../tnls-tunnel" }
tnls-plugin = { path = "../tnls-plugin" }
clap = { version = "4", features = ["derive"] }
tokio = { workspace = true }
anyhow = { workspace = true }
serde_json = { workspace = true }
rustls = { version = "0.23", features = ["ring"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Delete the `tunnel` bin from `tnls-tunnel`**

```bash
git rm crates/tnls-tunnel/src/main.rs crates/tnls-tunnel/src/cli.rs
```
In `crates/tnls-tunnel/src/lib.rs`, remove the `pub mod cli;` line (keep `mcp`, `pidfile`, `session`).
In `crates/tnls-tunnel/Cargo.toml`, delete the `[[bin]] name = "tunnel" …` section **and** drop now-unused deps that only the bin used (`clap` — confirm with `cargo build -p tnls-tunnel` after; keep anything `session.rs`/`mcp.rs` still use).

> `parse_scope` (was in the deleted `tnls-tunnel/cli.rs`) is reborn in `tnls/src/cli.rs` (Task 5.2). Its two inline tests move there too (spec §8 "rewritten, not relocated").

### Task 5.2: `cli.rs` — the host command surface

- [ ] **Step 1: Write `crates/tnls/src/cli.rs`**

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tnls", about = "Pluggable, capability-scoped tunnels to local MCP servers.")]
pub struct Cli {
    /// Relay base URL (applies to `open` and plugin dispatch).
    #[arg(long, global = true, default_value = "ws://127.0.0.1:8787")]
    pub relay: String,
    /// Override the tunnel TTL (else the plugin's describe default, or 15m for `open`).
    #[arg(long, global = true)]
    pub ttl: Option<String>,
    /// Override the scope allowlist, comma-separated (else the plugin's describe scope).
    #[arg(long, global = true)]
    pub scope: Option<String>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Tunnel any local MCP server (the generic primitive; = the old `tunnel open`).
    Open {
        /// Path to the MCP server binary.
        server: String,
        /// Args forwarded to the server (after `--`).
        server_args: Vec<String>,
    },
    /// Revoke a running tunnel (SIGTERM; defaults to the most recent).
    Close {
        #[arg(long)]
        tunnel_id: Option<String>,
    },
    /// List discovered `tnls-*` plugins and their commands.
    Plugins,
    /// Dispatch to a plugin: `tnls <name> <command> [args…]`.
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// Split a `--scope a,b , c` string into trimmed, non-empty names.
pub fn parse_scope(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()).map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_splits_and_trims() {
        assert_eq!(parse_scope("read, search"), vec!["read", "search"]);
        assert!(parse_scope("").is_empty());
        assert!(parse_scope("  ").is_empty());
    }

    #[test]
    fn open_parses_server_and_global_ttl() {
        let cli = Cli::try_parse_from(["tnls", "open", "./srv", "--ttl", "2m"]).unwrap();
        assert_eq!(cli.ttl.as_deref(), Some("2m"));
        match cli.cmd {
            Cmd::Open { server, server_args } => {
                assert_eq!(server, "./srv");
                assert!(server_args.is_empty());
            }
            _ => panic!("expected open"),
        }
    }

    #[test]
    fn plugin_tail_is_captured_verbatim() {
        // Globals BEFORE the plugin name are the host's; everything after belongs to the plugin.
        let cli = Cli::try_parse_from(["tnls", "--ttl", "1h", "rendezvous", "get", "lnk", "--out", "d"]).unwrap();
        assert_eq!(cli.ttl.as_deref(), Some("1h"));
        match cli.cmd {
            Cmd::External(v) => assert_eq!(v, vec!["rendezvous", "get", "lnk", "--out", "d"]),
            _ => panic!("expected external"),
        }
    }

    #[test]
    fn ttl_after_plugin_name_belongs_to_plugin() {
        // `--ttl` AFTER the plugin name is captured into the tail (forwarded to the plugin), per spec §3.2.
        let cli = Cli::try_parse_from(["tnls", "rendezvous", "get", "lnk", "--ttl", "1h"]).unwrap();
        assert!(cli.ttl.is_none(), "host did not consume the post-name --ttl");
        match cli.cmd {
            Cmd::External(v) => assert!(v.contains(&"--ttl".to_string()) && v.contains(&"1h".to_string())),
            _ => panic!("expected external"),
        }
    }
}
```

> **This test is the oracle — and a red result is a design fork, not a flag flip.** The pattern (clap 4.6.x, `external_subcommand` + `global = true` flags) is valid, and `external_subcommand` is documented to capture the subcommand token + everything after it as a raw `Vec<String>` *before* global matching reaches those trailing tokens — so this should pass as written. If it does **not**: do not just drop `global = true`, because that forces `--ttl`/`--scope` to be redefined on `Open` *and* re-handled for the before-name path, which breaks `open_parses_server_and_global_ttl` and demo.sh's `tnls open ./srv --ttl 2m` ergonomics (spec §5). Verify empirically and keep this test as the oracle for spec §3.2.

### Task 5.3: `discover.rs` — find plugins

- [ ] **Step 1: Write `crates/tnls/src/discover.rs`**

```rust
use std::path::{Path, PathBuf};

/// Directories searched for `tnls-<name>`: the dir holding the running `tnls`, then `$PATH`.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs
}

/// Resolve a plugin binary by name within `dirs` (testable core of `resolve`).
pub fn resolve_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let bin = format!("tnls-{name}");
    dirs.iter().map(|d| d.join(&bin)).find(|p| is_executable_file(p))
}

pub fn resolve(name: &str) -> Option<PathBuf> {
    resolve_in(&search_dirs(), name)
}

/// All discovered plugin names (deduped, first-found wins).
pub fn list_names() -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for dir in search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            if let Some(n) = e.file_name().to_str().and_then(|f| f.strip_prefix("tnls-")) {
                if is_executable_file(&e.path()) {
                    seen.insert(n.to_string());
                }
            }
        }
    }
    seen.into_iter().collect()
}

fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file()
        && std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_in_finds_executable_tnls_prefixed_binary() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tnls-foo");
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();

        let dirs = vec![dir.path().to_path_buf()];
        assert_eq!(resolve_in(&dirs, "foo"), Some(p));
        assert_eq!(resolve_in(&dirs, "missing"), None);
    }
}
```

> `is_executable_file` uses `std::os::unix` — this targets the CI matrix (ubuntu + macos), both Unix. If Windows support is ever needed, gate it; out of scope here.

### Task 5.4: `dispatch.rs` — route a plugin command

- [ ] **Step 1: Write `crates/tnls/src/dispatch.rs`**

```rust
use crate::cli::parse_scope;
use crate::discover;
use anyhow::{anyhow, bail, Context, Result};
use tnls_plugin::Manifest;
use tnls_tunnel::session::{self, OpenArgs};

/// Where a resolved command routes.
pub enum Route {
    /// Tunnel the plugin's serve command with this scope + ttl-string.
    Tunnel { scope: Vec<String>, ttl: String },
    /// Run the plugin command as a plain client (no tunnel).
    Plain,
}

/// Pure routing decision (unit-testable): given the manifest, the command name, and the
/// host's optional `--scope`/`--ttl` overrides, decide how to run it.
pub fn route(m: &Manifest, cmd: &str, scope_override: Option<&str>, ttl_override: Option<&str>) -> Result<Route> {
    let command = m.command(cmd).ok_or_else(|| {
        let known: Vec<&str> = m.commands.iter().map(|c| c.name.as_str()).collect();
        anyhow!("unknown command '{cmd}' for plugin '{}'; known: {}", m.name, known.join(", "))
    })?;
    Ok(match &command.tunnel {
        Some(t) => Route::Tunnel {
            scope: scope_override.map(parse_scope).unwrap_or_else(|| t.scope.clone()),
            ttl: ttl_override.unwrap_or(&t.default_ttl).to_string(),
        },
        None => Route::Plain,
    })
}

/// `args` = [name, command, rest…]. `relay`/`ttl`/`scope` are the host globals.
pub async fn run(args: Vec<String>, relay: String, ttl: Option<String>, scope: Option<String>) -> Result<()> {
    let name = args.first().ok_or_else(|| anyhow!("no plugin named"))?;
    let bin = discover::resolve(name).ok_or_else(|| {
        anyhow!("no plugin '{name}' — looked for 'tnls-{name}' next to tnls and on $PATH. Run 'tnls plugins' to see what's installed.")
    })?;

    let out = tokio::process::Command::new(&bin)
        .arg("describe")
        .output()
        .await
        .with_context(|| format!("running '{} describe'", bin.display()))?;
    let manifest: Manifest = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing describe from '{name}'"))?;

    let cmd = args.get(1).ok_or_else(|| anyhow!("usage: tnls {name} <command> [args…]"))?;
    let tail = args[1..].to_vec(); // [command, rest…] — forwarded verbatim

    match route(&manifest, cmd, scope.as_deref(), ttl.as_deref())? {
        Route::Tunnel { scope, ttl } => {
            let ttl = tnls_core::parse_ttl(&ttl).map_err(anyhow::Error::msg)?;
            session::open(OpenArgs {
                server: bin.to_string_lossy().into_owned(),
                server_args: tail,
                ttl,
                scope,
                relay: relay.clone(),
                env: vec![("TNLS_RELAY".to_string(), relay)],
            })
            .await
        }
        Route::Plain => {
            let status = tokio::process::Command::new(&bin).args(&tail).status().await
                .with_context(|| format!("running '{}'", bin.display()))?;
            if !status.success() {
                bail!("{name} {cmd} exited with {status}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tnls_plugin::Command;

    fn manifest() -> Manifest {
        Manifest::new("rendezvous", "x", vec![
            Command::tunneled("share", "s", &["list_shares", "request_file"], "30m"),
            Command::plain("get", "g"),
        ])
    }

    #[test]
    fn tunneled_uses_describe_scope_and_ttl_by_default() {
        match route(&manifest(), "share", None, None).unwrap() {
            Route::Tunnel { scope, ttl } => {
                assert_eq!(scope, vec!["list_shares", "request_file"]);
                assert_eq!(ttl, "30m");
            }
            _ => panic!("share should tunnel"),
        }
    }

    #[test]
    fn overrides_win() {
        match route(&manifest(), "share", Some("read"), Some("5m")).unwrap() {
            Route::Tunnel { scope, ttl } => {
                assert_eq!(scope, vec!["read"]);
                assert_eq!(ttl, "5m");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn plain_command_does_not_tunnel() {
        assert!(matches!(route(&manifest(), "get", None, None).unwrap(), Route::Plain));
    }

    #[test]
    fn unknown_command_errors_with_known_list() {
        let e = route(&manifest(), "nope", None, None).unwrap_err().to_string();
        assert!(e.contains("unknown command 'nope'") && e.contains("share"));
    }
}
```

### Task 5.5: `main.rs` + `lib.rs`, wire and verify

- [ ] **Step 1: Write `crates/tnls/src/lib.rs`** (expose modules for the unit tests)

```rust
pub mod cli;
pub mod discover;
pub mod dispatch;
```

- [ ] **Step 2: Replace `crates/tnls/src/main.rs`**

```rust
use anyhow::Result;
use clap::Parser;
use tnls::cli::{parse_scope, Cli, Cmd};
use tnls::{discover, dispatch};
use tnls_tunnel::session::{self, OpenArgs};

#[tokio::main]
async fn main() -> Result<()> {
    // rustls 0.23 needs a process-wide provider before the agent dials the relay over wss://.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Open { server, server_args } => {
            let ttl = tnls_core::parse_ttl(cli.ttl.as_deref().unwrap_or("15m")).map_err(anyhow::Error::msg)?;
            session::open(OpenArgs {
                server,
                server_args,
                ttl,
                scope: parse_scope(&cli.scope.unwrap_or_default()),
                relay: cli.relay,
                env: vec![],
            })
            .await
        }
        Cmd::Close { tunnel_id } => session::close(tunnel_id.as_deref()),
        Cmd::Plugins => {
            for name in discover::list_names() {
                println!("{name}");
            }
            Ok(())
        }
        Cmd::External(args) => dispatch::run(args, cli.relay, cli.ttl, cli.scope).await,
    }
}
```

- [ ] **Step 3: Build the workspace**

Run: `cargo build --workspace`
Expected: clean. The `tunnel` bin is gone; `tnls` is the host.

- [ ] **Step 4: Unit tests (cli + discover + dispatch)**

Run: `cargo test -p tnls`
Expected: PASS — scope parsing, `open` parsing, the two arg-forwarding oracles, `resolve_in`, and the four `route` tests. If `ttl_after_plugin_name_belongs_to_plugin` fails, resolve per the note in Task 5.2.

- [ ] **Step 5: Integration — `tnls plugins` lists the built plugins**

Add `crates/tnls/tests/cli_dispatch.rs`:

```rust
use std::process::Command;

#[test]
fn plugins_lists_demo_and_rendezvous() {
    // `CARGO_BIN_EXE_tnls` is injected for THIS crate's integration tests and points at the
    // built host binary — honoring any custom CARGO_TARGET_DIR. The sibling plugin binaries
    // land in the same dir, so build them, then run `tnls plugins`.
    for p in ["tnls-demo", "tnls-rendezvous"] {
        assert!(Command::new(env!("CARGO")).args(["build", "-p", p]).status().unwrap().success());
    }
    let out = Command::new(env!("CARGO_BIN_EXE_tnls")).arg("plugins").output().unwrap();
    let listed = String::from_utf8_lossy(&out.stdout);
    assert!(listed.contains("demo"), "got: {listed}");
    assert!(listed.contains("rendezvous"), "got: {listed}");
}
```

Run: `cargo test -p tnls --test cli_dispatch`
Expected: PASS (`tnls plugins` finds `tnls-demo`/`tnls-rendezvous` next to `tnls` in `target/debug`).

- [ ] **Step 6: Full suite + gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo fmt --all --check`
Expected: green.

- [ ] **Step 7: Manual smoke (optional but recommended)**

```bash
cargo run -p relay &                       # relay on :8787
cargo run -p tnls -- demo serve --ttl 2m   # prints a scoped link; ctrl-c to revoke
# in another shell:
cargo run -p tnls -- plugins               # lists demo, rendezvous
```

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat(tnls): the pluggable host — open/close/plugins + describe dispatch; retire the tunnel bin"
```

**End of Chunk 5:** `tnls` is the main binary. `tnls open <srv>` tunnels any MCP server; `tnls <plugin> <cmd>` dispatches via `describe` (tunneled→agent, plain→exec); `tnls plugins` lists them. The `tunnel` bin is gone. `tnls demo serve` and `tnls rendezvous share <f>` work end-to-end. Docs/scripts still name the old binaries — Chunk 6.

---

## Chunk 6: docs, scripts, and the final gate

**Goal:** Bring the human-facing docs/scripts in line with the new binaries and crate layout, then run the full CI gate (incl. `cargo audit`). No product code changes — pure docs + a green sweep (spec §7).

**Files:** `demo.sh`, `README.md`, `.github/workflows/ci.yml` (one comment), `CLAUDE.md`.

### Task 6.1: `demo.sh`

- [ ] **Step 1: Update the build + open lines**

In `demo.sh`:
- line 8 `cargo build -p relay -p mcp-demo -p tunnel-locker` → `cargo build -p relay -p tnls -p tnls-demo`
- line 20 `cargo run -q -p tunnel-locker -- open ./target/debug/mcp-demo --ttl "$TTL" --scope "$SCOPE"` → `cargo run -q -p tnls -- --ttl "$TTL" --scope "$SCOPE" demo serve`

> Globals (`--ttl`/`--scope`) go **before** the `demo` plugin name (spec §3.2). The comment on line 2 ("...tunnel to the demo MCP server") still reads fine.

- [ ] **Step 2: Smoke it**

Run: `./demo.sh` — relay boots, a scoped link prints; Ctrl-C tears down. (Then stop it.)

### Task 6.2: `README.md`

- [ ] **Step 1: Replace the stale binary/crate references**

- Line ~5: ``the link evaporates on TTL or `tunnel close`.`` → `` `tnls close`.``
- The Demo block (the two `cargo …` lines that build `mcp-demo` and run `tunnel-locker -- open …`): replace with
  ```bash
  # open a 2-minute, read-only tunnel to the sample MCP server (plugin)
  cargo run -p tnls -- --ttl 2m --scope read demo serve
  ```
- The `cargo run -p tunnel-locker -- close` line → `cargo run -p tnls -- close`.
- The **Workspace** table: replace rows with the new layout —
  | Crate | Role |
  |---|---|
  | `tnls-core` | Pure core: token, scope filter, per-call decision, TTL parse, frames. |
  | `tnls-tunnel` | The tunnel **agent** library: `session::open` (secret, scope, TTL), `McpChild` (rmcp client). |
  | `tnls-plugin` | The `describe` manifest contract shared by host + plugins. |
  | `tnls` | The host binary: `open`/`close`/`plugins` + plugin dispatch. |
  | `plugins/demo` | `tnls-demo`: sample rmcp MCP server (read + shell). |
  | `plugins/rendezvous` | `tnls-rendezvous`: file sending over BitTorrent. |
  | `relay` | Axum WS pairing service (binary: `relay`). |
- The **Tests** paragraph: `tunnel-locker` → `tnls-tunnel`; note MCP is now `rmcp` and the file-sharing transfer is tested in `tnls-rendezvous`.

> The README still under-documents `tnls`/plugins overall — full README rework is out of scope for this plan; fix only the now-wrong binary/crate names so it doesn't actively mislead.

### Task 6.3: `.github/workflows/ci.yml`

- [ ] **Step 1: Update the stale artifact comment**

Line ~53: the comment naming `target/debug/{relay,tnls,mcp-demo}` → `target/debug/{relay,tnls,tnls-demo,tnls-rendezvous}` (the packages the integration tests now self-build). No job/step logic changes.

### Task 6.4: `CLAUDE.md`

- [ ] **Step 1: Bring the project doc in line with the new architecture**

Make these targeted updates (the doc is the source of truth other sessions read):
- **"Two product surfaces"** section → reframe as **one host (`tnls`) + plugins**. The binaries table becomes: `tnls` (host: `open`/`close`/`plugins` + dispatch), `tnls-demo` (plugin), `tnls-rendezvous` (plugin); the `tunnel` binary is retired.
- **Commands** table → `cargo run -p tnls -- open <srv>` / `tnls demo serve` / `tnls rendezvous share <f>` / `tnls rendezvous get <link>`; drop `tunnel`/`tnls share`/`tnls get`/`tnls mcp-serve`.
- **Architecture** crate list → `tnls-core`, `tnls-tunnel`, `tnls-plugin`, `tnls`, `plugins/{demo,rendezvous}`, `relay`; note MCP is `rmcp` on both stdio ends, and the host dispatches to `tnls-<name>` plugins via the `describe` manifest.
- **Gotchas:** the `librqbit` pin note now points at `crates/plugins/rendezvous/Cargo.toml`; replace the **"`tnls mcp-serve` is internal"** gotcha with "`tnls-rendezvous share` is the self-seeding rmcp server the host spawns; don't run it by hand"; **remove the "No CI" line** (CI exists — `.github/workflows/ci.yml` runs fmt/clippy/test/audit); update the **"README is behind the code"** gotcha (it claims the README covers `tunnel-locker` only — the README now documents `tnls`, and the crate is `tnls-tunnel`).

> Keep the **Security invariants** section verbatim — none of it changed; `tnls` is still the sole agent and the relay is untouched by this plan.

### Task 6.5: Final gate

- [ ] **Step 1: The full CI gate locally**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all green, no warnings.

- [ ] **Step 2: Advisory audit over the new dependency tree**

Run: `cargo audit`
Expected: no *new* vulnerabilities introduced by `rmcp`/`schemars`/etc. (advisory — `cargo audit` is `continue-on-error` in CI; note anything it flags).

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "docs: retarget demo.sh/README/CLAUDE.md/ci comment to tnls + plugins"
```

**End of Chunk 6 — and the plan:** the workspace is the `tnls` host + `tnls-plugin` contract + `tnls-demo`/`tnls-rendezvous` plugins on `rmcp`, the relay untouched, docs aligned, and the full CI gate green. Spec 2 (relay → Rocket) is the next plan; per its §9 it will update the `tnls-tunnel` and `tnls-rendezvous` e2e launch helpers and drop their `axum` dev-deps.
