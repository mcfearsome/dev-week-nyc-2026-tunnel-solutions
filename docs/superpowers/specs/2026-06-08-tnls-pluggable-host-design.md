# tnls — Pluggable MCP-Tunnel Host (design spec)

**Date:** 2026-06-08
**Status:** Approved (pre-implementation)
**Scope:** Refactor the workspace so **`tnls` becomes the main binary**: a thin, pluggable host
concerned with (1) creating simple MCP servers, (2) creating tunnels, and (3) dispatching to
subprocess plugins. The BitTorrent file-sharing product becomes the **`rendezvous`** plugin; the
sample server becomes the **`demo`** plugin. Both the plugin MCP servers and the agent's MCP client
are built on the **official MCP Rust SDK, `rmcp`**, replacing all hand-rolled MCP/JSON-RPC. The
relay is **untouched** here — its rewrite onto Rocket is a separate spec
(`2026-06-08-rocket-relay-design.md`, TBD).

---

## 1. Motivation

Today there are two CLIs: `tunnel` (`tunnel-locker` — a scoped, TTL'd tunnel to *any* local MCP
server) and `tnls` (`tnls` — file sending over BitTorrent, built on top of `tunnel`). The tunnel
primitive is **already a library** (`tunnel_locker::session::open`, taking
`{ server, server_args, ttl, scope, relay }`), and the file-sharing logic is **welded on top of
it** in `share.rs` (seed a file → gather peers → call `session::open` with a hard-coded scope
pointing at a hidden `mcp-serve`). MCP itself is hand-rolled twice on the server side
(`mcp_serve.rs`, `mcp-demo/dispatch.rs`) and once on the client side (`McpChild` in `mcp.rs`).

This refactor *names* the latent host/plugin structure **and** adopts the official SDK so we stop
hand-rolling the protocol. Three concerns map to homes:

| Concern | Home | What it is |
|---|---|---|
| create simple MCP servers | **`rmcp`** (official SDK) | plugins implement an `rmcp` `ServerHandler` with `#[tool]` methods; `demo` is the worked example. tnls owns *no* MCP machinery. |
| create tunnels | `tnls-tunnel` (lib) | `session::open` — the **agent** (secret, scope, TTL, relay bridge). Its `McpChild` is now an `rmcp` **client**. |
| pluggable | `tnls` (host bin) + `tnls-plugin` (contract) | clap external-subcommand dispatch to `tnls-<name>` binaries; `tnls-plugin` holds the shared `describe` manifest type. |

**Security invariant, preserved unchanged:** `tnls` is the **sole agent** — the only holder of the
per-session 32-byte HMAC secret and the only party that connects to the relay. Plugins never see
the secret and never talk to the relay. `rmcp` is used only for stdio MCP framing on the two child
endpoints; it never touches the token, the relay socket, or the scope/TTL decision (see §10).

## 2. Target crate topology

```
crates/
  tnls-core/        (rename of tunnel-locker-core) pure logic: HMAC token, scope filter,
                    per-call decision, TTL parse, frames. Internals untouched.
  tnls-tunnel/      (rename of tunnel-locker, BINARY REMOVED) library only:
                    session::open() — the agent; McpChild (now an rmcp CLIENT over
                    TokioChildProcess); pidfile. ⇐ "create tunnels"
  tnls-plugin/      NEW tiny library: the `describe` manifest serde types (Manifest / Command /
                    Tunnel). Shared so the host (deserialize) and plugins (serialize) can't drift.
  tnls/             THE HOST BINARY (bin: tnls). clap. Built-in `open`/`close`/`plugins`;
                    everything else dispatched to tnls-<name>. Owns the tunnel (the agent) via
                    tnls-tunnel. ⇐ "pluggable". Installs the rustls provider (§ below).
  plugins/
    rendezvous/     NEW bin: tnls-rendezvous. rmcp ServerHandler for the file-sharing tools +
                    bittorrent.rs + the get client. Commands: share (tunneled), get/seed/fetch
                    (plain), describe.
    demo/           (rename of mcp-demo) bin: tnls-demo. Minimal rmcp ServerHandler (read/shell)
                    + describe. Proves the trivial path.
  relay/            UNCHANGED (the Rocket rewrite is a separate spec).
```

There is **no `tnls-mcp` crate** — `rmcp` is the MCP library. The host `tnls` no longer depends on
`librqbit`, `reqwest`, `local-ip-address`, or any MCP code; its deps shrink to clap + `tnls-tunnel`
+ `tnls-core` + `tnls-plugin` + `rustls`.

**Per-crate `rmcp` usage and kept deps:**
- `tnls-tunnel`: add `rmcp` (features `client`, `transport-child-process`) for `McpChild`; keeps
  `tokio-tungstenite` (the agent↔**relay** WS is *not* rmcp), `rustls`, `getrandom`, `hex`, `nix`,
  `humantime`, `serde`/`serde_json`, `tnls-core`. The hand-rolled JSON-RPC client in `mcp.rs` (the
  `Pending` map, `next_id`, reader task) is deleted.
- `plugins/rendezvous`: `rmcp` (`server`, `transport-io`) + `schemars` (tool-arg schemas);
  `librqbit`, `tokio-tungstenite` + `rustls` (the `get` viewer client dials the relay over `wss`),
  `reqwest` (`/whoami` over HTTPS), `local-ip-address`, `tnls-core` (frames for `get`),
  `tnls-plugin`.
- `plugins/demo`: `rmcp` (`server`, `transport-io`) + `schemars`, `tnls-plugin`. No TLS, no rustls.
- `tnls-plugin`: `serde` + `serde_json` only (the describe contract is tnls's own, not MCP).

**rustls crypto provider — process-global, easy to drop on the floor.** The dropped `tunnel`
`main.rs` installs the rustls ring provider before any `wss://` dial
(`rustls::crypto::ring::default_provider().install_default()`, `tunnel-locker/main.rs:8`). This is
for the **relay** connection (tokio-tungstenite + rustls), *not* rmcp. Every binary that opens a TLS
connection must install it at the top of its own `main()` — concretely **both `tnls`** (it dials
the relay via `session::open`) **and `tnls-rendezvous`** (its `get` client dials the relay over
`wss://` and hits `/whoami` over HTTPS). So the install relocates into `tnls/main.rs` *and* is added
to `tnls-rendezvous`'s `main`; both crates take a `rustls` dep. Missing it is a runtime panic on
first TLS connect, not a compile error — the plan must call it out explicitly.

## 3. The plugin contract

### 3.1 `describe`

Every plugin binary answers one tnls-defined invocation, `tnls-<name> describe`, by printing
**static** JSON (a serialized `tnls_plugin::Manifest`) and exiting. No side effects, no setup:

```json
{
  "schema": 1,
  "name": "rendezvous",
  "about": "Capability-scoped file sending over BitTorrent.",
  "commands": [
    { "name": "share", "about": "Seed a file and serve it through a scoped tunnel.",
      "tunnel": { "scope": ["list_shares", "request_file"], "default_ttl": "30m" } },
    { "name": "get",   "about": "Fetch a shared file from a tunnel link.",
      "tunnel": null }
  ]
}
```

The `tnls-plugin` crate owns the `Manifest` / `Command` / `Tunnel` serde types; the host
deserializes them, plugins serialize them, so the shape can't drift. The entire contract is the
`tunnel` field of each command:

- **`tunnel: { scope, default_ttl }`** → a *tunneled* command. tnls opens a tunnel with that scope
  and ttl (each overridable by a global flag) and spawns `tnls-<name> <cmd> <args>` as the MCP
  **stdio child** (an `rmcp` server) — the agent path.
- **`tunnel: null`** (or absent) → a *plain* command. tnls `exec`-passthroughs
  `tnls-<name> <cmd> <args>` with inherited stdio. **No tunnel, no secret, no relay** — this is
  `get`'s path (a pure client that dials a *remote* tunnel as a headless viewer).

The `demo` manifest is one command, `serve`, `tunnel: { scope: ["read"], default_ttl: "15m" }` —
`shell` is **deny-by-default** until the user opts in with `--scope read,shell`.

### 3.2 Dispatch

```
tnls rendezvous share ./big.zip --ttl 1h
  → tnls-rendezvous describe → "share" is tunnel:{scope:[list_shares,request_file]}
  → session::open({ server:"<resolved tnls-rendezvous path>", server_args:["share","./big.zip"],
                    scope, ttl:1h, relay, env:{TNLS_RELAY:relay} })
        └ spawns `tnls-rendezvous share ./big.zip` (an rmcp server over stdio) as the MCP child;
          tnls is the agent.

tnls rendezvous get <link> --out ./dl
  → describe → "get" is tunnel:null
  → exec `tnls-rendezvous get <link> --out ./dl` (inherit stdio); tnls steps aside.
```

**Conceptual unity:** a tunneled plugin command is *exactly* `tnls open` with `server = the plugin
binary` and `scope = describe's scope`. The plugin dispatch path and the built-in `open` path are
the **same agent code path** — dispatch just resolves the binary + scope from `describe` first. No
new security-sensitive code is introduced by the plugin system.

**Argument forwarding — the one place clap needs care.** The host's clap uses
`external_subcommand`, which captures the plugin name and *everything after it* as a raw
`Vec<String>`, unparsed (configure the catch-all with `trailing_var_arg` + `allow_hyphen_values` so
plugin flags like `get`'s `--out` survive). The host treats that tail as **opaque** and forwards it
verbatim — as `server_args` to `session::open` for a tunneled command, or as the `exec` argv for a
plain command. It never parses plugin-specific flags. Consequently the host's own globals
(`--ttl` / `--scope` / `--relay`) are recognized only **before** the plugin name
(`tnls --ttl 1h rendezvous share ./f`); anything after the plugin name belongs to the plugin. The
contract has no ambiguity: the host owns the head (globals + plugin name + command), the plugin owns
the tail.

Unknown command in a plugin's `describe` → error listing the plugin's known commands (strict; no
silent passthrough of unknown commands).

### 3.3 Discovery

tnls resolves `tnls-<name>` by searching **`dir_of(current_exe())` first, then `$PATH`**. A cargo
workspace builds every binary into the same `target/<profile>/`, and `cargo install` drops them in
the same bin dir, so a plugin always lives next to `tnls` itself. This generalizes the
`current_exe()` trick `share.rs` already uses to locate its own `mcp-serve`, so
`cargo run -p tnls -- rendezvous share ./f` and an installed `tnls` both work with no PATH setup.

Resolution failure teaches rather than dumps a stack trace:

```
no plugin 'rendezvous' — looked for 'tnls-rendezvous' next to tnls and on $PATH.
Run 'tnls plugins' to see what's installed.
```

(Optional, YAGNI-guarded: an extra search dir via `$TNLS_PLUGIN_PATH`. Not required for v1.)

### 3.4 Seeder lifetime — the subtle part

Today `run_share` holds the BitTorrent **seeder** alive in the *parent* while `mcp-serve` runs as
a *separate* child (two processes: one seeds, one serves). In the new model tnls is the parent, so
the seeder needs a home.

**Decision: the serve child owns the seeder.** `tnls-rendezvous share ./f` is **one** long-lived
process that, on startup, creates the torrent → gathers peers (loopback + LAN + public IP via
`TNLS_RELAY`'s `/whoami`) → starts seeding → *then* runs the `rmcp` server answering
`list_shares`/`request_file`/`status`. The seeder handle is a **field of the rmcp `ServerHandler`
value**, so it lives exactly as long as the served session: tnls spawns the child, tunnels it, and
on teardown (TTL / Ctrl-C / viewer-gone) kills it → the rmcp server loop ends → the handler drops →
the seeder drops. One lifetime, one kill path.

This deliberately **drops the dynamic `prepare` step** floated during brainstorming. A `prepare`
that seeds, prints a spec, and exits would either kill the seeder on exit, or force a re-seed in
the serve process that picks a *different* port/UPnP mapping than the peers `prepare` already
advertised. Collapsing seed+serve into one process avoids that and makes the plugin command
**standalone-runnable** (`tnls-rendezvous share ./f` is a working stdio MCP server on its own),
which keeps e2e tests hermetic. `describe` stays purely static. (Dynamic prepare is explicit future
work — §9.)

## 4. MCP via `rmcp` (replaces the old hand-rolled `tnls-mcp`)

`rmcp` 1.7.0 is the official MCP Rust SDK. It owns the MCP machinery on both stdio endpoints; we
write tools and a thin client wrapper, nothing else.

### 4.1 Plugins are `rmcp` servers

A plugin implements an `rmcp` `ServerHandler` with `#[tool_router]` / `#[tool(description = …)]`
methods. Tool arguments arrive as a typed `Parameters<T>` (where `T: serde::Deserialize +
schemars::JsonSchema`) and the MCP `inputSchema` is **auto-generated by schemars** — strictly
better than the hand-written JSON schemas in `mcp_serve.rs` / `dispatch.rs`. The server runs over
stdio (`transport-io`). Sketch (demo):

```rust
#[derive(Deserialize, schemars::JsonSchema)]
struct ReadParams { path: String }

#[tool_router]
impl Demo {
    #[tool(description = "Read a UTF-8 text file and return its contents.")]
    async fn read(&self, Parameters(ReadParams { path }): Parameters<ReadParams>)
        -> Result<CallToolResult, ErrorData> { /* … */ }
    #[tool(description = "Run a shell command and return its output.")]
    async fn shell(&self, /* ShellParams */) -> Result<CallToolResult, ErrorData> { /* … */ }
}
// demo `serve` cmd:  Demo::default().serve(stdio()).await?.waiting().await?;
```

The **seeder-lifetime model from §3.4 is a struct field** on the rmcp handler:

```rust
struct ShareServer { share: ShareInfo, _seeder: Seeder }  // _seeder drops when the service ends
// rendezvous `share`:  seed → gather_peers → ShareServer{share,_seeder}.serve(stdio()).await?.waiting().await?
```

### 4.2 The agent is an `rmcp` client (inside `McpChild`)

`McpChild` (in `tnls-tunnel`) keeps its small, stable surface but swaps its internals to an `rmcp`
client over `TokioChildProcess`:

- `McpChild::spawn(cmd, args, env)` builds a `tokio::process::Command`, applies `env` (this is how
  `TNLS_RELAY` reaches the rendezvous child — see §5), wraps it in `TokioChildProcess`, and
  `.serve()`s an rmcp client. The handshake + framing are rmcp's job.
- `child.tools()` ← rmcp `list_all_tools()`, converted to `Vec<tnls_core::Tool>` at this boundary
  (a thin map of name/description/schema) so the viewer-facing `AgentFrame::Tools` and
  `filter_tools` keep operating on `tnls_core::Tool` unchanged.
- `child.call_tool(name, args)` → rmcp `call_tool(CallToolRequestParam { name, arguments })` —
  rmcp's **dynamic** call-by-name-with-JSON, exactly what a generic proxy needs (the agent never
  knows tool types at compile time). The returned `CallToolResult.content` is converted back to the
  JSON the `AgentFrame::Result` already carries; an rmcp tool `Err` / `isError` result maps to the
  existing `ErrorCode::ToolError` frame (`session.rs:264`), so viewer-visible error behavior is
  unchanged.

**The agent event loop is untouched.** `session.rs`'s `handle_frame` / `decide_call` /
`filter_tools` / per-call TTL re-check do not change; only `McpChild`'s guts move to rmcp. The
hand-rolled JSON-RPC (`Pending`, `next_id`, the reader task) in `mcp.rs` is deleted.

## 5. The `tnls` CLI surface

```
tnls open <server> [args…]      tunnel any local MCP server      (= old `tunnel open`)
tnls close [--tunnel-id ID]     revoke a running tunnel          (= old `tunnel close`)
tnls plugins                    list discovered tnls-* plugins (+ their describe commands)
tnls <name> <cmd> [args…]       dispatch to a plugin (external_subcommand)
```

**Global flags** `--relay` / `--ttl` / `--scope` are clap `global = true`, so they work *before* a
plugin name (`tnls --ttl 1h rendezvous share ./f`) and still *trail* the built-in `open`
(`tnls open ./srv --ttl 2m`, preserving the old ergonomics). After a plugin name they belong to the
plugin, not the host (§3.2). Because `describe` supplies `default_ttl` and scope, these are rarely
typed.

The `demo` plugin's sole command is `serve`, so it is invoked **`tnls demo serve`** (e.g.
`tnls demo serve --scope read,shell`); there is no bare-`tnls demo` default. The `CLAUDE.md`
command table (§7) shows this exact form.

`open`/`close` are thin wrappers over `tnls_tunnel::session::{open, close}` — the logic relocated
from `tunnel`'s `main.rs`. `parse_scope` moves from `tunnel-locker/cli.rs` to the host.

**Additive library change:** `tnls_tunnel::session::OpenArgs` gains `env: Vec<(String, String)>` so
tnls can pass `TNLS_RELAY` to the child. With rmcp this is clean: `session::open` forwards `env`
into `McpChild::spawn`, which sets it on the `tokio::process::Command` before wrapping it in
`TokioChildProcess` (no bespoke `spawn` JSON plumbing — rmcp owns the framing). Existing `open`
callers pass an empty vec.

## 6. Migration plan

| From | To | Kind |
|------|----|----|
| `crates/tunnel-locker-core` | `crates/tnls-core` | rename; internals untouched |
| `crates/tunnel-locker` (lib: session/mcp/pidfile) | `crates/tnls-tunnel` | rename; **drop `main.rs`+`cli.rs`**; `mcp.rs` rewritten onto rmcp client |
| `tunnel-locker/cli.rs::parse_scope` + `main.rs` open/close logic | `crates/tnls` (`open`/`close`) | move |
| `crates/mcp-demo` | `crates/plugins/demo` (bin `tnls-demo`) | rename; **rewritten as an rmcp server** |
| `crates/tnls/src/{bittorrent,get,share,mcp_serve}.rs` | `crates/plugins/rendezvous` | move; `mcp_serve.rs` → an rmcp server |
| — | `crates/tnls-plugin` | **new** tiny crate (describe contract types) |
| `crates/tnls` | gutted to the host (dispatch + open/close/plugins) | transform-in-place |
| `crates/relay` | **untouched** | — |

**Order (keeps `cargo test --workspace` green at each step):**

1. Renames (`tnls-core`, `tnls-tunnel`-minus-bin) — mechanical, compiler-checked.
2. Add `rmcp` to `tnls-tunnel`; **rewrite `McpChild` onto the rmcp client** (`TokioChildProcess`).
   The existing agent-bridge tests still spawn the (still hand-rolled) `mcp-demo` — an rmcp client
   speaks to any protocol-compliant server, so they stay green and prove the rewrite.
3. Add `crates/tnls-plugin` (describe types). Rewrite `mcp-demo` → `plugins/demo` as an rmcp server
   + `describe`.
4. Stand up `plugins/rendezvous` from the moved file-sharing code: `mcp_serve.rs` → an rmcp server,
   `share` rewritten to self-seed+serve (drop its `session::open` call).
5. Gut `tnls` → host (dispatch, discovery, `open`/`close`/`plugins`); retire the `tunnel` bin;
   update root `Cargo.toml`, `demo.sh`, `README.md`, the `ci.yml` comment, `CLAUDE.md`.
6. Re-land tests; satisfy the CI gates locally — `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings` (warnings are hard errors), and
   `cargo test --workspace --locked` on the matrix (`.github/workflows/ci.yml`). `--locked` means
   `Cargo.lock` must be regenerated for the new/renamed crates **and** the `rmcp`/`schemars` adds;
   re-run the advisory `cargo audit` over the new dependency tree.

## 7. Intentional behavior changes

Everything else is behavior-preserving; these are deliberate:

1. **CLI shape:** `tunnel open X` → `tnls open X`; `tnls share f` → `tnls rendezvous share f`;
   `tnls get L` → `tnls rendezvous get L`; `tnls demo serve` replaces
   `tunnel open ./target/debug/mcp-demo`. Every doc/script that hard-codes the old binaries or crate
   names updates: `demo.sh`; `README.md` (the Demo block, the Workspace crate table, the Tests
   paragraph); the comment in `.github/workflows/ci.yml` that names `target/debug/{relay,tnls,mcp-demo}`;
   and `CLAUDE.md` (command table, the "two product surfaces" framing → "one host + plugins", and
   its now-stale "No CI" note — CI exists). The agent banner in `session.rs` (`print_banner`) that
   prints "`tunnel close` to revoke" becomes "`tnls close`".
2. **`share` stops printing the magnet locally** — it's computed inside the `tnls-rendezvous share`
   child now and rides the tunnel to the viewer via `request_file` (the child may still echo it to
   stderr, which tnls inherits).
3. **Tool input schemas are now schemars-generated** rather than hand-written JSON. The advertised
   schemas may differ cosmetically from today's (e.g. added `"required"`/`"title"` fields); tests
   assert on tool *names* and behavior, not exact schema bytes. **Tool names and order remain
   load-bearing** and the rmcp port must preserve them: `mcp_bridge` asserts the advertised list is
   `["read","shell"]` and the read-only e2e asserts the post-filter list is `["read"]` — schemars /
   rmcp must not rename or reorder tools.

## 8. Testing

**The safety net sits where the risk does not move.** The two suites guarding the
security-critical code — `tnls-core`'s token/scope/TTL/frame units and `relay`'s
pairing/cross-instance/redis tests — are **untouched** by this spec (core is only renamed; the
relay isn't touched). These same suites also become the contract the Rocket-relay spec must keep
green.

- **Untouched:** `tnls-core/*` units, all `relay/tests/*`.
- **Relocated + import-renamed:** `tunnel-locker`'s `mcp_bridge.rs` / `end_to_end.rs` agent-bridge
  tests → `tnls-tunnel/tests`. They drive `session::open` the library and now exercise the rmcp
  `McpChild` end-to-end against a real child — they survive intact and *prove* the rmcp rewrite;
  `session.rs` units stay.
- **Rewritten, not relocated:** the `tunnel-locker/cli.rs` *inline* parser tests
  (`scope_splits_and_trims`, `open_parses_flags_not_as_server_args`) assert a `Command::Open` enum
  shape that no longer exists once the host's clap is redesigned (global flags + `open`/`close`/`plugins`
  + an `external_subcommand` catch-all). They are re-authored as new `tnls` parser tests covering
  `tnls open` parsing *and* the plugin-tail forwarding rule (§3.2).
- **Moved + ported to rmcp:** the file-sharing surface → `plugins/rendezvous/tests` (the share→get
  *real transfer* e2e + `get.rs` units); the `mcp_serve.rs` tool tests become tests of the
  rendezvous rmcp tools. `mcp-demo`'s `dispatch.rs` read/shell/tool tests → tests of the `demo`
  rmcp tools. We **no longer test the JSON-RPC envelope** (initialize/tools-list/unknown-method) —
  rmcp owns it; we test our tools' behavior and the agent↔plugin integration.
- **New, small:** `tnls` discovery + describe-routing tests; a 3-line per-plugin sync guard
  asserting every `describe` command parses in that plugin's CLI. Two assertions lock the
  contract's sharp edges: (a) **arg-forwarding** — `tnls rendezvous get <link> --ttl 1h` routes
  `--ttl 1h` into the *plugin tail*, not the host global (§3.2); (b) **seeder lifetime** — dropping
  a `ShareServer` stops the seeding session, pinning the RAII contract §3.4/§4.1 leans on
  (`bittorrent.rs` `SeedHandle` already drops to stop).

## 9. Non-goals / future work

- **Dynamic `prepare`** (a plugin computing scope/ttl/setup output at runtime that tnls consumes
  before opening the tunnel). Not needed by `demo` or `rendezvous`; `describe`'s `schema: 1` leaves
  room.
- **Dynamic-library / in-process plugins.** Subprocess only.
- **`rmcp` for the tunnel/relay wire protocol.** The viewer↔relay↔agent frames
  (`AgentFrame`/`ViewerFrame`) and `get.rs`'s headless viewer stay the project's own protocol; rmcp
  is scoped to the two stdio MCP endpoints only.
- **A richer plugin protocol beyond `schema: 1`** (versioning/negotiation). Out of scope until a
  non-first-party plugin exists.
- **`$TNLS_PLUGIN_PATH`** extra search dir (optional convenience; current_exe-dir + PATH suffice).
- **The whole-relay-in-Rocket rewrite** — its own spec, with an invariant-preservation acceptance
  gate (no scope logic / no persistence / opaque pass-through / 1 MiB frame cap / aggregate-only
  stats) and the existing relay tests required to pass unchanged.

## 10. Security notes

- tnls is the **sole agent**: only it mints/holds the secret and connects to the relay. Plugins are
  spawned over stdio exactly like any MCP server under `tnls open`; they never see the secret or the
  relay. The agent implementation (`session.rs` loop) is unchanged and audited in one place.
- **`rmcp` is confined to stdio MCP framing on the two child endpoints.** It never sees the token,
  the relay socket, or the scope/TTL decision. The agent's enforcement (`tnls-core`'s `decide_call`
  / `filter_tools`, the per-call TTL re-check) *wraps* the rmcp client: a buggy or hostile plugin
  (rmcp server) is still scope- and TTL-constrained by the agent exactly as before, because
  enforcement happens in `tnls` before any `call_tool` reaches the child.
- `describe` is **advisory metadata** that can only *narrow*: it selects scope (a capability
  allowlist of tool names) and routes tunnel-vs-plain. A plugin declaring a scope is equivalent to a
  user running `tnls open that-server --scope …` — the scope is shown in the tunnel banner, and
  `--scope` always lets the user restrict further. A plugin cannot widen a TTL, forge a token, or
  reach the relay.
- Deny-by-default where it's cheap: `demo`'s default scope omits `shell`.
- The `rendezvous` plugin's **only** contact with the relay is the unauthenticated, public
  `GET /whoami` (public-IP discovery for peer advertisement) — the same call `share.rs` makes today.
  It never opens a tunnel socket, never sees the secret, carries no token. "Plugin talks to the
  relay" here is one anonymous IP-echo GET, not a breach of the agent-only rule.

## 11. Touch points

`Cargo.toml` (workspace members) + `Cargo.lock` (+ `rmcp`/`schemars`), `crates/tnls-core` (rename),
`crates/tnls-tunnel` (rename, drop bin, `mcp.rs` → rmcp client, `OpenArgs.env`, banner string),
`crates/tnls-plugin` (new, describe types), `crates/tnls` (transform to host, install rustls
provider), new `crates/plugins/rendezvous` (rmcp server + rustls provider) + `crates/plugins/demo`
(rmcp server), `demo.sh`, `README.md`, `.github/workflows/ci.yml` (stale build-artifact comment),
`CLAUDE.md`. The relay binary itself, the Dockerfile, the `fly.toml`, and the CI `deploy` job are
untouched.
