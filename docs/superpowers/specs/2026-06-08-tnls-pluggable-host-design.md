# tnls — Pluggable MCP-Tunnel Host (design spec)

**Date:** 2026-06-08
**Status:** Approved (pre-implementation)
**Scope:** Refactor the workspace so **`tnls` becomes the main binary**: a thin, pluggable host
concerned with (1) creating simple MCP servers, (2) creating tunnels, and (3) dispatching to
subprocess plugins. The BitTorrent file-sharing product becomes the **`rendezvous`** plugin; the
sample server becomes the **`demo`** plugin. The relay is **untouched** here — its rewrite onto
Rocket is a separate spec (`2026-06-08-rocket-relay-design.md`, TBD).

---

## 1. Motivation

Today there are two CLIs: `tunnel` (`tunnel-locker` — a scoped, TTL'd tunnel to *any* local MCP
server) and `tnls` (`tnls` — file sending over BitTorrent, built on top of `tunnel`). The tunnel
primitive is **already a library** (`tunnel_locker::session::open`, taking
`{ server, server_args, ttl, scope, relay }`), and the file-sharing logic is **welded on top of
it** in `share.rs` (seed a file → gather peers → call `session::open` with a hard-coded scope
pointing at a hidden `mcp-serve`).

This refactor *names* that latent structure. `tnls` becomes a host that exposes the tunnel
primitive plus a subprocess-plugin system, and the file-sharing surface moves out into a plugin.
Three concerns map to three library homes:

| Concern | Home | What it is |
|---|---|---|
| create simple MCP servers | `tnls-mcp` (new lib) | the reusable stdio MCP server loop + JSON-RPC scaffolding |
| create tunnels | `tnls-tunnel` (lib) | `session::open` — the **agent** (secret, scope, TTL, relay bridge) |
| pluggable | `tnls` (host bin) | clap external-subcommand dispatch to `tnls-<name>` binaries |

**Security invariant, preserved unchanged:** `tnls` is the **sole agent** — the only holder of the
per-session 32-byte HMAC secret and the only party that connects to the relay. Plugins never see
the secret and never talk to the relay. The agent code lives and is audited in exactly one place.

## 2. Target crate topology

```
crates/
  tnls-core/        (rename of tunnel-locker-core) pure logic: HMAC token, scope filter,
                    per-call decision, TTL parse, frames. Internals untouched.
  tnls-tunnel/      (rename of tunnel-locker, BINARY REMOVED) library only:
                    session::open() — the agent; McpChild; pidfile. ⇐ "create tunnels"
  tnls-mcp/         NEW library: the stdio MCP server — read→dispatch→write loop +
                    initialize/tools/list/tools/call scaffolding. ⇐ "create simple MCP servers"
  tnls/             THE HOST BINARY (bin: tnls). clap. Built-in `open`/`close`/`plugins`;
                    everything else dispatched to tnls-<name>. Owns the tunnel (the agent)
                    via tnls-tunnel. ⇐ "pluggable". Sheds librqbit/reqwest/etc.
  plugins/
    rendezvous/     NEW bin: tnls-rendezvous. Absorbs bittorrent.rs + share + get +
                    the file-sharing MCP server (now on tnls-mcp). Commands:
                    share (tunneled), get/seed/fetch (plain), describe.
    demo/           (rename of mcp-demo) bin: tnls-demo. Minimal plugin: read/shell on
                    tnls-mcp + describe. Proves the trivial path.
  relay/            UNCHANGED (the Rocket rewrite is a separate spec).
```

The host `tnls` no longer depends on `librqbit`, `reqwest`, `local-ip-address`, or
`tokio-tungstenite` — those follow the code into `rendezvous`. The host's deps shrink to clap +
`tnls-tunnel` + `tnls-core` + `rustls` (see below). `tnls-tunnel` keeps the agent's existing deps:
`rustls`, `getrandom`, `hex`, `nix` (signals / close), `humantime` (banner).

**rustls crypto provider — process-global, easy to drop on the floor.** The dropped `tunnel`
`main.rs` installs the rustls ring provider before any `wss://` dial
(`rustls::crypto::ring::default_provider().install_default()`, `tunnel-locker/main.rs:8`). Every
binary that opens a TLS connection must do this at the top of its own `main()` — concretely **both
`tnls`** (it dials the relay via `session::open`) **and `tnls-rendezvous`** (its `get` client dials
a remote `wss://` relay and hits `/whoami` over HTTPS). So the install relocates into `tnls/main.rs`
*and* is added to `tnls-rendezvous`'s `main`; both crates take a `rustls` dep. Missing it is a
runtime panic on first TLS connect, not a compile error — the plan must call it out explicitly.

## 3. The plugin contract

### 3.1 `describe`

Every plugin binary answers one tnls-defined invocation, `tnls-<name> describe`, by printing
**static** JSON and exiting. No side effects, no setup:

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

The entire contract is the `tunnel` field of each command:

- **`tunnel: { scope, default_ttl }`** → a *tunneled* command. tnls opens a tunnel with that scope
  and ttl (each overridable by a global flag) and spawns `tnls-<name> <cmd> <args>` as the MCP
  **stdio child** — the agent path.
- **`tunnel: null`** (or absent) → a *plain* command. tnls `exec`-passthroughs
  `tnls-<name> <cmd> <args>` with inherited stdio. **No tunnel, no secret, no relay** — this is
  `get`'s path (a pure client that dials a *remote* tunnel as a headless viewer).

The `demo` manifest is one command:

```json
{ "schema": 1, "name": "demo", "about": "Sample MCP server (read + shell).",
  "commands": [ { "name": "serve", "about": "Serve read/shell over a tunnel.",
                  "tunnel": { "scope": ["read"], "default_ttl": "15m" } } ] }
```

Note `demo`'s default scope is `["read"]` only — `shell` is **deny-by-default** until the user
opts in with `--scope read,shell`. The deny-by-default falls out of the design for free.

### 3.2 Dispatch

```
tnls rendezvous share ./big.zip --ttl 1h
  → tnls-rendezvous describe → "share" is tunnel:{scope:[list_shares,request_file]}
  → session::open({ server:"<resolved tnls-rendezvous path>", server_args:["share","./big.zip"],
                    scope, ttl:1h, relay, env:{TNLS_RELAY:relay} })
        └ spawns `tnls-rendezvous share ./big.zip` as the MCP child; tnls is the agent.

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

(Optional, YAGNI-guarded: an extra search dir via `$TNLS_PLUGIN_PATH`. Not required for v1; the
first-party plugins are found by the current_exe-dir rule.)

### 3.4 Seeder lifetime — the subtle part

Today `run_share` holds the BitTorrent **seeder** alive in the *parent* while `mcp-serve` runs as
a *separate* child (two processes: one seeds, one serves). In the new model tnls is the parent, so
the seeder needs a home.

**Decision: the serve child owns the seeder.** `tnls-rendezvous share ./f` is **one** long-lived
process that, on startup, creates the torrent → gathers peers (loopback + LAN + public IP via
`TNLS_RELAY`'s `/whoami`) → starts seeding → *then* runs the MCP loop answering
`list_shares`/`request_file`/`status`. tnls spawns it, tunnels it, and on teardown (TTL / Ctrl-C /
viewer-gone) kills it → the seeder drops with the process. One lifetime, one kill path.

This deliberately **drops the dynamic `prepare` step** floated during brainstorming. A `prepare`
that seeds, prints a spec, and exits would either kill the seeder on exit, or force a re-seed in
the serve process that picks a *different* port/UPnP mapping than the peers `prepare` already
advertised. Collapsing seed+serve into one process avoids that and makes the plugin command
**standalone-runnable** (`tnls-rendezvous share ./f` is a working stdio MCP server on its own —
exactly how `mcp-demo` already behaves), which keeps e2e tests hermetic. `describe` stays purely
static. (Dynamic prepare is explicit future work — see §9.)

## 4. The `tnls-mcp` library

Both existing servers are the same loop around a `(name, args) → result` function. The library
keeps the loop + JSON-RPC scaffolding; a plugin implements a trait:

```rust
#[async_trait]
pub trait McpServer: Send + Sync {
    fn info(&self) -> ServerInfo;                 // name + version → initialize
    fn tools(&self) -> Vec<Tool>;                 // → tools/list
    async fn call(&self, tool: &str, args: Value) -> Result<Value, String>;  // → tools/call
}

pub async fn serve<S: McpServer>(server: S) -> anyhow::Result<()>;  // owns the stdio loop
pub fn text(s: impl Into<String>) -> Value;        // {content:[{type:"text",text:…}]}
```

`serve` handles `initialize` (from `info()`), `tools/list` (from `tools()`), `tools/call`
(→ `call()`, wrapping `Err` into a `-32000` envelope), `notifications/initialized`, and
unknown-method `-32601` — i.e. everything currently duplicated in `mcp_serve.rs::handle` and
`mcp-demo`'s `dispatch.rs::handle`. The tnls↔plugin boundary stays pure MCP-over-JSON, so
`tnls-mcp`'s `Tool` is independent of `tnls-core`'s `Tool` (which is what tnls parses *back* out of
the handshake for scope filtering — a different layer).

The **demo** server becomes trivial: `struct Demo;` implementing the trait with `read`/`shell`.

The **seeder-lifetime model from §3.4 expresses itself as a struct field**:

```rust
struct ShareServer { share: ShareInfo, _seeder: Seeder }  // _seeder dropped when serve() returns
// rendezvous `share`:  seed → gather_peers → tnls_mcp::serve(ShareServer { share, _seeder }).await
```

The seeder being *owned by the server value* is what ties seeding to the exact process tnls
manages — there is no separate lifetime to reason about.

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

**One additive library change — with internal plumbing.** `tnls_tunnel::session::OpenArgs` gains an
optional `env: Vec<(String, String)>` so tnls can pass `TNLS_RELAY` to the child. It is additive at
the `OpenArgs` boundary (existing `open` callers pass an empty vec), but it threads one level
deeper: `McpChild::spawn(cmd, args)` (today no env handling, `mcp.rs:26`) gains an `env` parameter —
or `session::open` sets the env on the `tokio::process::Command` before spawn — and the single call
site in `session::open` is updated. Not purely a struct field; the plan must plumb it to the child
spawn.

## 6. Migration plan

| From | To | Kind |
|------|----|----|
| `crates/tunnel-locker-core` | `crates/tnls-core` | rename; internals untouched |
| `crates/tunnel-locker` (lib: session/mcp/pidfile) | `crates/tnls-tunnel` | rename; **drop `main.rs`+`cli.rs`** |
| `tunnel-locker/cli.rs::parse_scope` + `main.rs` open/close logic | `crates/tnls` (`open`/`close`) | move |
| `crates/mcp-demo` | `crates/plugins/demo` (bin `tnls-demo`) | rename; refactored onto `tnls-mcp` |
| `crates/tnls/src/{bittorrent,get,share,mcp_serve}.rs` | `crates/plugins/rendezvous` | move |
| — | `crates/tnls-mcp` | **new** lib (extracted serve loop) |
| `crates/tnls` | gutted to the host (dispatch + open/close/plugins) | transform-in-place |
| `crates/relay` | **untouched** | — |

**Order (keeps `cargo test --workspace` green at each step):**

1. Renames (`tnls-core`, `tnls-tunnel`-minus-bin, `plugins/demo`) — mechanical, compiler-checked.
2. Extract `tnls-mcp`; refactor `tnls-demo` onto it.
3. Stand up `plugins/rendezvous` from the moved file-sharing code; rewrite `share` to
   self-seed+serve (drop its `session::open` call).
4. Gut `tnls` → host (dispatch, discovery, `open`/`close`/`plugins`); retire the `tunnel` bin;
   update root `Cargo.toml`, `demo.sh`, `CLAUDE.md`.
5. Re-land tests; satisfy the CI gates locally — `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings` (warnings are hard errors), and
   `cargo test --workspace --locked` on the matrix (`.github/workflows/ci.yml`). `--locked` means
   `Cargo.lock` must be regenerated for the new/renamed crates as part of the change.

## 7. Intentional behavior changes

Everything else is behavior-preserving; these two are deliberate:

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

## 8. Testing

**The safety net sits where the risk does not move.** The two suites guarding the
security-critical code — `tnls-core`'s token/scope/TTL/frame units and `relay`'s
pairing/cross-instance/redis tests — are **untouched** by this spec (core is only renamed; the
relay isn't touched). The highest-stakes logic keeps its full green wall through the entire move;
these same suites also become the contract the Rocket-relay spec must keep green.

- **Untouched:** `tnls-core/*` units, all `relay/tests/*`.
- **Relocated + import-renamed:** `tunnel-locker`'s `mcp_bridge.rs` / `end_to_end.rs` agent-bridge
  tests → `tnls-tunnel/tests` (they drive `session::open` the library, so they survive intact);
  `session.rs` units stay.
- **Rewritten, not relocated:** the `tunnel-locker/cli.rs` *inline* parser tests
  (`scope_splits_and_trims`, `open_parses_flags_not_as_server_args`) assert a `Command::Open` enum
  shape that no longer exists once the host's clap is redesigned (global flags + `open`/`close`/`plugins`
  + an `external_subcommand` catch-all). They are re-authored as new `tnls` parser tests covering
  `tnls open` parsing *and* the plugin-tail forwarding rule (§3.2) — a redesign, not a move.
- **Moved to `plugins/rendezvous/tests`:** today's `tnls/tests/{loopback,end_to_end}.rs` (the
  share→get *real transfer* e2e) + the `get.rs` / `mcp_serve.rs` units. The e2e's existing
  `mcp_exe` override generalizes to "point the host at the built `target/debug/tnls-rendezvous`" —
  same current_exe-dir discovery, still hermetic (loopback peer, no DHT).
- **Moved to `plugins/demo`:** `dispatch.rs`'s read/shell/tool-order tests → tests of the `Demo`
  impl.
- **New, small:** `tnls-mcp` envelope tests (initialize / tools-list / unknown-method /
  notification); `tnls` discovery + describe-routing tests; a 3-line per-plugin sync guard
  asserting every `describe` command parses in that plugin's CLI. Two assertions lock the
  contract's sharp edges: (a) **arg-forwarding** — `tnls rendezvous get <link> --ttl 1h` routes
  `--ttl 1h` into the *plugin tail*, not the host global (§3.2); (b) **seeder lifetime** — dropping
  a `ShareServer` stops the seeding session, pinning the RAII contract §3.4/§4 leans on
  (`bittorrent.rs` `SeedHandle` already drops to stop).

## 9. Non-goals / future work

- **Dynamic `prepare`** (a plugin computing scope/ttl/setup output at runtime that tnls consumes
  before opening the tunnel). Not needed by `demo` or `rendezvous`; add only if a real plugin
  requires it. `describe`'s `schema: 1` leaves room.
- **Dynamic-library / in-process plugins.** Subprocess only.
- **A third-party plugin protocol beyond `schema: 1`** (versioning/negotiation, capability
  discovery richer than `describe`). Out of scope until there's a non-first-party plugin.
- **`$TNLS_PLUGIN_PATH`** extra search dir (optional convenience; current_exe-dir + PATH suffice).
- **The whole-relay-in-Rocket rewrite** — its own spec, with an invariant-preservation acceptance
  gate (no scope logic / no persistence / opaque pass-through / 1 MiB frame cap / aggregate-only
  stats) and the existing relay tests required to pass unchanged.

## 10. Security notes

- tnls is the **sole agent**: only it mints/holds the secret and connects to the relay. Plugins
  are spawned over stdio exactly like any MCP server under `tnls open`; they never see the secret
  or the relay. The agent implementation is unchanged and audited in one place.
- `describe` is **advisory metadata** that can only *narrow*: it selects scope (a capability
  allowlist of tool names) and routes tunnel-vs-plain. A plugin declaring a scope is equivalent to
  a user running `tnls open that-server --scope …` — the scope is shown in the tunnel banner, and
  `--scope` always lets the user restrict further. A plugin cannot widen a TTL, forge a token, or
  reach the relay.
- Deny-by-default where it's cheap: `demo`'s default scope omits `shell`.
- The `rendezvous` plugin's **only** contact with the relay is the unauthenticated, public
  `GET /whoami` (public-IP discovery for peer advertisement) — the same call `share.rs` makes today.
  It never opens a tunnel socket, never sees the secret, carries no token. "Plugin talks to the
  relay" here is one anonymous IP-echo GET, not a breach of the agent-only rule.

## 11. Touch points

`Cargo.toml` (workspace members) + `Cargo.lock`, `crates/tnls-core` (rename), `crates/tnls-tunnel`
(rename, drop bin, `OpenArgs.env` → `McpChild::spawn`, banner string), `crates/tnls-mcp` (new),
`crates/tnls` (transform to host, install rustls provider), new `crates/plugins/rendezvous` (+ rustls
provider) + `crates/plugins/demo`, `demo.sh`, `README.md`, `.github/workflows/ci.yml` (stale comment),
`CLAUDE.md`. The relay binary itself, the Dockerfile, the `fly.toml`, and the CI `deploy` job are
untouched (only ci.yml's stale build-artifact *comment* changes).
