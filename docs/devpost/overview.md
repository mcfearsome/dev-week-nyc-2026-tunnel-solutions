# Project Overview

## Project Name

### tunnel.solutions

## Elevator Pitch

**tnls is a pluggable host for ephemeral, capability-scoped tunnels to local MCP servers.** Wrap a local MCP server, hand a teammate a link scoped to exactly the tools you allow for exactly as long as you allow — and it evaporates on close or TTL. Every capability is a **plugin**: the first, `rendezvous`, sends large files over BitTorrent, so the bytes move peer-to-peer (you don't eat the bandwidth) while the tunnel gates who can fetch and for how long. One audited security boundary — the agent — holds the secret; the relay only moves opaque bytes.

**Live at https://tunnel.locker** · Rust · official MCP SDK (`rmcp`) · no inbound ports.

## Links

- Live relay + landing page: https://tunnel.locker
- Source: https://github.com/mcfearsome/tunnel-solutions
- Design specs + phased implementation plans: `docs/superpowers/`

## Note on architecture state

The project is mid-migration to the host + plugin architecture described here (designed spec-first; renames landed, the plugin extraction and `rmcp` port in flight). The capability-tunnel core and the BitTorrent file-sharing capability both work today and are deployed live at the URL above.
