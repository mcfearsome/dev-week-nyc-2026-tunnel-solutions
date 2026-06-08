# tnls Phase 2.5 — Tunnel as Peer Rendezvous (design spec)

**Date:** 2026-06-07
**Status:** Approved (pre-implementation)
**Scope:** Make BitTorrent transfers connect *immediately* by carrying the seeder's peer
address(es) through the capability tunnel, instead of waiting on DHT discovery.

## 1. Problem

`tnls get` retrieves the magnet through the tunnel (Phase 2, works), then relies on DHT +
public trackers to find the seeder — which on a single host or same-LAN often never connects
in a useful window (no direct peer hint; tracker hairpin-NAT). Phase 1 proved the transfer
*mechanism* works the instant peers connect directly (`NetOpts.initial_peers`). So the fix is
to **introduce the peers over the control plane we already have.**

## 2. Mechanism

The seeder knows its own librqbit listen address; the tunnel already connects both peers. Pass
the address(es) through `request_file`; the getter feeds them straight to `fetch` as
`initial_peers`.

```
tnls share:  seed binds a known listen port P (UPnP on) → share gathers reachable addrs:
               127.0.0.1:P, <each LAN IP>:P, <public-IP>:P
             → tnls mcp-serve --peer <addr> (repeatable)
tnls get:    request_file → { "magnet": …, "peers": ["…:P", …] }
             → fetch(magnet, NetOpts { initial_peers: peers, … })   ← Phase-1 mechanism, verbatim
```

## 3. Address discovery (`tnls share`)

Advertise three tiers (all at the seeder's listen port `P`):
1. **Loopback** — `127.0.0.1:P` (single-host; also makes the e2e test a real transfer).
2. **LAN IPs** — enumerate the host's non-loopback IPv4 interfaces (same-LAN / VPN). Use a
   small dependency (e.g. `local-ip-address`) or `std`-based interface enumeration; confirm in
   the plan.
3. **Public IP** — learned from a new **`GET /whoami` on the relay** (returns the caller's
   public IP via Fly's `Fly-Client-IP` header, falling back to the socket peer address
   locally). Combined with `P`. Using our own relay keeps discovery in-house — consistent with
   the product's "no third-party trackers" stance — rather than pinging a public echo service.

The seeder's listen port `P`: `share` either sets an explicit `P` in `seed`'s `NetOpts`
(picking a free port by probing `TcpListener::bind(":0")` then reusing it) or queries
librqbit's actual listen address after session creation. The exact accessor is confirmed in
the plan; the interface is "share knows `P`."

UPnP: `seed` already enables `enable_upnp_port_forwarding`; this maps `P`. We advertise
`<public-IP>:P` assuming the IGD preserves the port (the common consumer case).

## 4. Control-plane changes

- **`tnls mcp-serve`** gains a repeatable `--peer <addr>` flag. `request_file` returns content
  text = a JSON object `{ "magnet": "…", "peers": ["ip:port", …] }` (instead of the bare
  magnet string). `list_shares`/`status` unchanged.
- **`tnls get`** (`retrieve_magnet`) parses the JSON content → `(magnet, Vec<SocketAddr>)`;
  invalid/unparseable peer strings are skipped. `run_get` passes the peers as
  `NetOpts.initial_peers` to `fetch`. Backward-compatible parse: if the content is a bare
  magnet string (no JSON), treat `peers` as empty.

## 5. Relay change

Add `GET /whoami` → `200` with the caller's IP as plain text. Read `Fly-Client-IP` if present,
else the connection's peer address. No state, no auth (it only reveals the caller's own IP).
This requires a relay redeploy.

## 6. Honest limit (state in UI + README)

Direct connection succeeds for loopback and LAN always, and for the public address when UPnP
mapping succeeded *and* the router preserves the port. Hard/symmetric NAT still won't connect
directly — true hole-punching is future work. Unreachable advertised peers simply fail
silently in librqbit; DHT/tracker remain as the fallback.

## 7. New gate / testing

- The **Phase 2 e2e test upgrades**: with the seeder advertising `127.0.0.1:P`, a single-host
  `share`→`get` now **completes the transfer**. The test asserts the downloaded bytes equal
  the source (not just "magnet retrieved"), still hermetic (loopback peer, no network).
- `mcp_serve` dispatch unit test: `request_file` returns JSON with `magnet` + `peers`.
- `get` unit test: parse a `{magnet, peers}` content → `(String, Vec<SocketAddr>)`, and the
  bare-magnet fallback.
- `/whoami` unit/integration: returns `Fly-Client-IP` when set, else peer addr.

## 8. Touch points

`relay` (`+/whoami`, redeploy), `bittorrent.rs` (`seed` takes/returns `P`; `NetOpts` already
has `initial_peers`), `share.rs` (gather addrs incl. `/whoami`, pass `--peer`), `mcp_serve.rs`
(`--peer`, JSON `request_file` response), `get.rs` (parse `peers`, feed `initial_peers`).

## 9. Non-goals

True NAT hole-punching / relay-as-data-fallback (the relay never carries file bytes); IPv6
peer advertisement (IPv4 first); authenticating `/whoami`. Hard-NAT cross-internet transfer
remains DHT-best-effort.
